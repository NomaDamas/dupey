use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// File kinds dupey knows how to talk about.
///
/// Only `Txt` and `Markdown` are extracted in this scaffold. Other variants
/// exist so format routing can land without changing the public enum later.
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
        matches!(self, Self::Txt | Self::Markdown)
    }
}

/// Text that is safe to hash and MinHash: volatile metadata already stripped.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CanonicalText {
    pub path: PathBuf,
    pub format: Format,
    pub text: String,
}

/// Keep only comparable content. Per-format extractors plug in here.
pub fn extract(path: &Path) -> Result<CanonicalText> {
    let format = Format::from_path(path).ok_or_else(|| Error::UnsupportedFormat {
        path: path.to_path_buf(),
    })?;
    match format {
        Format::Txt | Format::Markdown => extract_utf8(path, format),
        _ => Err(Error::UnsupportedFormat {
            path: path.to_path_buf(),
        }),
    }
}

fn extract_utf8(path: &Path, format: Format) -> Result<CanonicalText> {
    let bytes = std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let text = String::from_utf8(bytes).map_err(|_| Error::InvalidUtf8 {
        path: path.to_path_buf(),
    })?;
    Ok(CanonicalText {
        path: path.to_path_buf(),
        format,
        text: normalize_newlines(&text),
    })
}

fn normalize_newlines(text: &str) -> String {
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
    fn normalizes_crlf() {
        let dir = std::env::temp_dir();
        let path = dir.join("dupey-extract-test.txt");
        std::fs::write(&path, "a\r\nb\rc\n").unwrap();
        let got = extract(&path).unwrap();
        assert_eq!(got.text, "a\nb\nc\n");
        let _ = std::fs::remove_file(&path);
    }
}
