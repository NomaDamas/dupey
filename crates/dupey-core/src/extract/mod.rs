//! Per-format extraction of canonical, comparable text.
//!
//! Extract strips volatile metadata (docProps, rsId, PDF creation data) so
//! that hashing/MinHash only sees content a human would call "the same
//! document". Scoring is shared; only extract grows per format.

pub mod docx;
pub mod hwpx;
pub mod pdf;
pub mod text;

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
            Self::Txt | Self::Markdown | Self::Docx | Self::Hwpx | Self::Pdf
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
        Format::Pdf => pdf::extract_pdf(path),
        _ => Err(Error::UnsupportedFormat {
            path: path.to_path_buf(),
        }),
    }
}

pub(crate) fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
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
        assert!(!Format::Hwp.extract_ready());
    }
}
