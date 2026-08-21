//! Directory names `scan` does not descend into.
//!
//! Matching is on a single folder name (case-insensitive), never a path
//! substring. The walk root itself is never skipped, so `dupey scan
//! node_modules` still reads that folder's own files.

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
    fn empty_or_dot_extra_names_are_ignored() {
        let s = set(&["", "  ", ".", ".."]);
        assert_eq!(s.len(), DEFAULT_SKIP_DIR_NAMES.len());
    }
}
