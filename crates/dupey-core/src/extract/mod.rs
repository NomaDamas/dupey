//! Per-format extraction of canonical, comparable text.
//!
//! Extract strips volatile metadata (docProps, rsId, PDF creation data) so
//! that hashing/MinHash only sees content a human would call "the same
//! document". Scoring is shared; only extract grows per format.

pub mod docx;
pub mod hwp;
pub mod hwpx;
pub mod pdf;
pub mod pptx;
pub mod text;
pub mod xlsx;

use std::path::{Path, PathBuf};

use jiff::Timestamp;

use crate::error::{Error, Result};

/// File kinds dupey knows how to talk about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Txt,
    Markdown,
    Docx,
    Pptx,
    Xlsx,
    Pdf,
    Hwp,
    Hwpx,
}

impl Format {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "txt" => Some(Self::Txt),
            "md" | "markdown" => Some(Self::Markdown),
            "docx" => Some(Self::Docx),
            "pptx" => Some(Self::Pptx),
            "xlsx" => Some(Self::Xlsx),
            "pdf" => Some(Self::Pdf),
            "hwp" => Some(Self::Hwp),
            "hwpx" => Some(Self::Hwpx),
            _ => None,
        }
    }

    pub fn extract_ready(self) -> bool {
        matches!(
            self,
            Self::Txt
                | Self::Markdown
                | Self::Docx
                | Self::Hwpx
                | Self::Hwp
                | Self::Pdf
                | Self::Pptx
                | Self::Xlsx
        )
    }
}

/// In-file provenance signals. Preferred over filesystem mtime, which
/// downloads and unzip operations clobber.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DocMeta {
    /// Internal modified timestamp (docx core.xml, hwpx content.hpf,
    /// PDF /ModDate). None when the format carries none.
    pub modified: Option<Timestamp>,
    /// Save/revision counter (docx cp:revision). Weak signal.
    pub revision: Option<u32>,
}

/// Text that is safe to hash and MinHash: volatile metadata already stripped.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CanonicalText {
    pub path: PathBuf,
    pub format: Format,
    pub text: String,
    pub meta: DocMeta,
}

/// Keep only comparable content. Per-format extractors plug in here.
pub fn extract(path: &Path) -> Result<CanonicalText> {
    let format = Format::from_path(path).ok_or_else(|| Error::UnsupportedFormat {
        path: path.to_path_buf(),
    })?;
    match format {
        Format::Txt | Format::Markdown => text::extract_utf8(path, format),
        Format::Docx => docx::extract_docx(path),
        Format::Hwpx => hwpx::extract_hwpx(path),
        Format::Hwp => hwp::extract_hwp(path),
        Format::Pptx => pptx::extract_pptx(path),
        Format::Xlsx => xlsx::extract_xlsx(path),
        Format::Pdf => pdf::extract_pdf(path),
    }
}

/// Upper bound on the text bytes `extract` may materialise for `path`,
/// without extracting anything: the file size, or for ZIP containers the
/// sum of declared uncompressed entry sizes (central directory only), so a
/// small file that inflates hugely is still visible to memory gating.
pub fn extract_size_hint(path: &Path) -> u64 {
    let file_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let zipped = matches!(
        Format::from_path(path),
        Some(Format::Docx | Format::Pptx | Format::Xlsx | Format::Hwpx)
    );
    if !zipped {
        return file_len;
    }
    let declared = std::fs::File::open(path)
        .ok()
        .and_then(|f| zip::ZipArchive::new(f).ok())
        .map(|mut zip| {
            (0..zip.len())
                .filter_map(|i| zip.by_index_raw(i).ok().map(|e| e.size()))
                .fold(0u64, u64::saturating_add)
        })
        .unwrap_or(0);
    file_len.max(declared)
}

/// CRLF and lone CR become LF. One pass into a buffer sized up front.
pub(crate) fn normalize_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(cr) = rest.find('\r') {
        out.push_str(&rest[..cr]);
        out.push('\n');
        rest = &rest[cr + 1..];
        rest = rest.strip_prefix('\n').unwrap_or(rest);
    }
    out.push_str(rest);
    out
}

/// Read one ZIP entry as UTF-8 text into a buffer sized from the entry's
/// declared length. Growing a large string by doubling instead leaves a
/// trail of freed blocks the system allocator may keep cached, which on
/// macOS accumulated to gigabytes across a few large documents.
pub(crate) fn read_zip_entry(
    path: &Path,
    zip: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
) -> Result<String> {
    use std::io::Read;
    let extract_err = |e: &dyn std::fmt::Display| Error::Extract {
        path: path.to_path_buf(),
        message: e.to_string(),
    };
    let mut entry = zip.by_name(name).map_err(|e| extract_err(&e))?;
    let mut buf = String::new();
    // A declared size is only a hint; if it is absurd, grow as before.
    let _ = buf.try_reserve_exact(usize::try_from(entry.size()).unwrap_or(0));
    entry
        .read_to_string(&mut buf)
        .map_err(|e| extract_err(&e))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_from_path() {
        assert_eq!(Format::from_path(Path::new("a.DOCX")), Some(Format::Docx));
        assert_eq!(Format::from_path(Path::new("n.hwpx")), Some(Format::Hwpx));
        assert_eq!(Format::from_path(Path::new("x.bin")), None);
    }

    #[test]
    fn extract_ready_formats() {
        for f in [
            Format::Txt,
            Format::Markdown,
            Format::Docx,
            Format::Hwpx,
            Format::Pdf,
        ] {
            assert!(f.extract_ready(), "{f:?} should be extract-ready");
        }
        assert!(Format::Hwp.extract_ready());
        assert!(Format::Pptx.extract_ready());
        assert!(Format::Xlsx.extract_ready());
    }

    #[test]
    fn newline_normalization_matches_its_definition() {
        // CRLF first, then any lone CR, as two plain replacements would do.
        for s in [
            "",
            "\r",
            "\n",
            "a\r",
            "\r\r\n",
            "\r\n\n",
            "\n\r",
            "\r\n\r\n",
            "가\r\n나\r다\n",
        ] {
            let expected = s.replace("\r\n", "\n").replace('\r', "\n");
            assert_eq!(normalize_newlines(s), expected, "{s:?}");
        }
    }

    #[test]
    fn size_hint_sees_through_zip_compression() {
        // 4 MB of repetitive text deflates to a few KB: the file size alone
        // would let a compressed bomb skip the extraction memory gate.
        let paragraph = "같은 문단 same paragraph ".repeat(200_000);
        let bytes = docx::tests::make_docx(&[&paragraph], "2026-01-01T00:00:00Z", 1);
        let path = docx::tests::write_tmp("dupey-size-hint-bomb.docx", &bytes);
        let hint = extract_size_hint(&path);
        let _ = std::fs::remove_file(&path);
        assert!((bytes.len() as u64) < 1 << 20, "fixture must compress well");
        assert!(
            hint >= paragraph.len() as u64,
            "hint {hint} below text size"
        );
    }

    #[test]
    fn size_hint_is_file_length_for_plain_and_unreadable_files() {
        let path = docx::tests::write_tmp("dupey-size-hint.txt", b"hello world");
        assert_eq!(extract_size_hint(&path), 11);
        let _ = std::fs::remove_file(&path);
        // A .docx that is not a zip falls back to its byte length.
        let path = docx::tests::write_tmp("dupey-size-hint-broken.docx", b"not a zip");
        assert_eq!(extract_size_hint(&path), 9);
        let _ = std::fs::remove_file(&path);
        assert_eq!(extract_size_hint(Path::new("/nonexistent/dupey.docx")), 0);
    }
}
