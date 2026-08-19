//! Plain text / Markdown extraction: UTF-8, newline normalization.

use std::path::Path;

use super::{normalize_newlines, CanonicalText, DocMeta, Format};
use crate::error::{Error, Result};

pub(crate) fn extract_utf8(path: &Path, format: Format) -> Result<CanonicalText> {
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
        meta: DocMeta::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_crlf() {
        let dir = std::env::temp_dir();
        let path = dir.join("dupey-extract-test.txt");
        std::fs::write(&path, "a\r\nb\rc\n").unwrap();
        let got = extract_utf8(&path, Format::Txt).unwrap();
        assert_eq!(got.text, "a\nb\nc\n");
        assert_eq!(got.meta, DocMeta::default());
        let _ = std::fs::remove_file(&path);
    }
}
