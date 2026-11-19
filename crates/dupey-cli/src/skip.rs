//! What `scan` does not read: vendor directories by name, and cloud
//! placeholders by filesystem flag.
//!
//! Directory matching is on a single folder name (case-insensitive), never a
//! path substring. The walk root itself is never skipped, so `dupey scan
//! node_modules` still reads that folder's own files.
//!
//! A cloud placeholder is a file whose bytes live on a remote service (OneDrive
//! "Files On-Demand", iCloud, Google Drive, Dropbox). Opening one makes the
//! provider download it, which can transfer gigabytes during a scan or fail
//! with `EDEADLK` ("Resource deadlock avoided") when the provider is busy.
//! They are skipped and reported, never read.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::Path;

/// Built-in vendor / VCS / tooling directories. Extra names come from
/// `--exclude-dir` and are merged into the same set.
pub const DEFAULT_SKIP_DIR_NAMES: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    ".venv",
    "venv",
    "node_modules",
    "bower_components",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".idea",
    ".vscode",
    ".next",
    ".nuxt",
    ".cache",
    ".gradle",
    "Pods",
];

pub fn skip_set(extra: &[String]) -> HashSet<String> {
    let mut set: HashSet<String> = DEFAULT_SKIP_DIR_NAMES
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    for name in extra {
        if let Some(n) = normalize_dir_name(name) {
            set.insert(n);
        }
    }
    set
}

/// Last path component, lowercased. `./임시/` and `임시` are the same name.
fn normalize_dir_name(name: &str) -> Option<String> {
    let trimmed = name.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return None;
    }
    let component = Path::new(trimmed)
        .file_name()
        .unwrap_or_else(|| OsStr::new(trimmed));
    let lower = component.to_string_lossy().to_lowercase();
    if lower.is_empty() {
        None
    } else {
        Some(lower)
    }
}

/// `FILE_ATTRIBUTE_OFFLINE`: data is not locally available.
#[cfg(any(windows, test))]
const WIN_ATTR_OFFLINE: u32 = 0x0000_1000;
/// `FILE_ATTRIBUTE_RECALL_ON_OPEN`: opening the file triggers a download.
#[cfg(any(windows, test))]
const WIN_ATTR_RECALL_ON_OPEN: u32 = 0x0004_0000;
/// `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`: reading the file triggers a
/// download. Set on OneDrive Files On-Demand and other Cloud Files API
/// placeholders.
#[cfg(any(windows, test))]
const WIN_ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
/// macOS `SF_DATALESS`: the file's data is not on this device.
#[cfg(any(target_os = "macos", test))]
const MACOS_SF_DATALESS: u32 = 0x4000_0000;
/// Extended attribute OneDrive for Mac puts on its zero-filled stub files.
/// Those stubs carry no `SF_DATALESS` flag, so this is the only marker.
#[cfg(target_os = "macos")]
const MACOS_ONEDRIVE_STUB_XATTR: &[u8] = b"com.microsoft.OneDrive.Macbox.PhData\0";

/// Whether Windows file attributes mark a file whose data is not local.
#[cfg(any(windows, test))]
fn windows_attrs_are_placeholder(attrs: u32) -> bool {
    attrs & (WIN_ATTR_OFFLINE | WIN_ATTR_RECALL_ON_OPEN | WIN_ATTR_RECALL_ON_DATA_ACCESS) != 0
}

/// Whether macOS `st_flags` mark a file whose data is not on this device.
#[cfg(any(target_os = "macos", test))]
fn macos_flags_are_placeholder(flags: u32) -> bool {
    flags & MACOS_SF_DATALESS != 0
}

/// Whether `path` carries the OneDrive stub marker. One `getxattr` call, and
/// it never follows a symlink or reads file data.
#[cfg(target_os = "macos")]
fn has_onedrive_stub_marker(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: both strings are NUL-terminated and outlive the call; a null
    // buffer with size 0 only asks for the attribute's length.
    let len = unsafe {
        libc::getxattr(
            c_path.as_ptr(),
            MACOS_ONEDRIVE_STUB_XATTR.as_ptr().cast(),
            std::ptr::null_mut(),
            0,
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    len >= 0
}

/// Whether `path` (with its `metadata`) is a cloud placeholder: a file whose
/// content would have to be downloaded to be read. Always false where the
/// platform has no such marker (Linux).
pub fn is_cloud_placeholder(path: &Path, metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let _ = path;
        windows_attrs_are_placeholder(metadata.file_attributes())
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        macos_flags_are_placeholder(metadata.st_flags()) || has_onedrive_stub_marker(path)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (path, metadata);
        false
    }
}

/// Skip this directory entry (and therefore do not walk its children).
/// Files are never skipped here; `Format::from_path` still decides extract.
pub fn should_skip_dir(
    file_name: &OsStr,
    is_dir: bool,
    depth: usize,
    skip: &HashSet<String>,
) -> bool {
    if depth == 0 || !is_dir {
        return false;
    }
    skip.contains(&file_name.to_string_lossy().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn set(extra: &[&str]) -> HashSet<String> {
        skip_set(&extra.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn skip(name: &str, is_dir: bool, depth: usize, extra: &[&str]) -> bool {
        should_skip_dir(&OsString::from(name), is_dir, depth, &set(extra))
    }

    #[test]
    fn defaults_skip_vendor_and_vcs() {
        assert!(skip("node_modules", true, 1, &[]));
        assert!(skip("Node_Modules", true, 2, &[]));
        assert!(skip(".git", true, 1, &[]));
        assert!(skip("target", true, 1, &[]));
        assert!(!skip("docs", true, 1, &[]));
        assert!(!skip("my_build_notes", true, 1, &[]));
    }

    #[test]
    fn extra_names_merge_case_insensitively() {
        assert!(skip("임시", true, 1, &["임시"]));
        assert!(skip("백업", true, 1, &["./백업/"]));
        assert!(!skip("임시", true, 1, &[]));
    }

    #[test]
    fn root_and_files_are_not_skipped() {
        assert!(!skip("node_modules", true, 0, &[]));
        assert!(!skip("node_modules", false, 1, &[]));
        assert!(!skip("LICENSE.md", false, 2, &[]));
    }

    #[test]
    fn windows_recall_and_offline_attributes_are_placeholders() {
        // 0x400000 is what OneDrive sets on a Files On-Demand placeholder.
        assert!(windows_attrs_are_placeholder(0x0040_0000));
        assert!(windows_attrs_are_placeholder(0x0040_0000 | 0x20)); // + ARCHIVE
        assert!(windows_attrs_are_placeholder(0x0004_0000));
        assert!(windows_attrs_are_placeholder(0x0000_1000));
        // A plain file, a hidden/system file, and a pinned (hydrated) file.
        assert!(!windows_attrs_are_placeholder(0x20));
        assert!(!windows_attrs_are_placeholder(0x20 | 0x2 | 0x4));
        assert!(!windows_attrs_are_placeholder(0x0008_0000)); // PINNED
        assert!(!windows_attrs_are_placeholder(0));
    }

    #[test]
    fn macos_dataless_flag_is_a_placeholder() {
        assert!(macos_flags_are_placeholder(0x4000_0000));
        assert!(macos_flags_are_placeholder(0x4000_0000 | 0x20)); // + SF_ARCHIVED
        assert!(!macos_flags_are_placeholder(0));
        assert!(!macos_flags_are_placeholder(0x0000_8000)); // UF_HIDDEN
    }

    #[test]
    fn ordinary_local_files_are_not_placeholders() {
        let dir = std::env::temp_dir().join(format!("dupey-skip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.png");
        std::fs::write(&file, b"x").unwrap();
        assert!(!is_cloud_placeholder(
            &file,
            &std::fs::metadata(&file).unwrap()
        ));
        assert!(!is_cloud_placeholder(
            &dir,
            &std::fs::metadata(&dir).unwrap()
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_or_dot_extra_names_are_ignored() {
        let s = set(&["", "  ", ".", ".."]);
        assert_eq!(s.len(), DEFAULT_SKIP_DIR_NAMES.len());
    }
}
