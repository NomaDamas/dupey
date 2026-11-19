//! Plain text / Markdown extraction: UTF-8 or BOM-marked UTF-16, newline
//! normalization.

use std::path::Path;

use super::{normalize_newlines, CanonicalText, DocMeta, Format};
use crate::error::{Error, Result};

/// UTF-8, or UTF-16 announced by a byte-order mark (what Notepad's "Unicode"
/// save and many Windows tools write). A UTF-8 BOM is dropped: it is not
/// content, and a copy saved with one is still the same document. Anything
/// else that is not UTF-8 is refused rather than guessed.
fn decode(bytes: Vec<u8>) -> Option<String> {
    match bytes.as_slice() {
        [0xFF, 0xFE, rest @ ..] => decode_utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => decode_utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => std::str::from_utf8(rest).ok().map(str::to_owned),
        _ => String::from_utf8(bytes).ok(),
    }
}

fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units = bytes.chunks_exact(2).map(|b| unit([b[0], b[1]]));
    char::decode_utf16(units)
        .collect::<std::result::Result<String, _>>()
        .ok()
}

pub(crate) fn extract_utf8(path: &Path, format: Format) -> Result<CanonicalText> {
    let bytes = std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let text = decode(bytes).ok_or_else(|| Error::InvalidUtf8 {
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

    fn write(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("dupey-text-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn utf16(text: &str, little_endian: bool, bom: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if bom {
            out.extend(if little_endian {
                [0xFF, 0xFE]
            } else {
                [0xFE, 0xFF]
            });
        }
        for unit in text.encode_utf16() {
            out.extend(if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        out
    }

    #[test]
    fn utf16_files_with_a_bom_are_decoded() {
        // Notepad's "Unicode" save and many Windows tools write UTF-16 + BOM.
        let text = "제안서 v2\r\n한국어 문서 😀\r\n";
        for (name, bytes) in [
            ("le.txt", utf16(text, true, true)),
            ("be.txt", utf16(text, false, true)),
        ] {
            let got = extract_utf8(&write(name, &bytes), Format::Txt).unwrap();
            assert_eq!(got.text, "제안서 v2\n한국어 문서 😀\n", "{name}");
        }
    }

    #[test]
    fn utf16_and_utf8_copies_of_a_document_are_the_same_text() {
        let text = "분기 보고서\n본문 첫 줄\n";
        let a = extract_utf8(&write("a.txt", text.as_bytes()), Format::Txt).unwrap();
        let b = extract_utf8(&write("b.txt", &utf16(text, true, true)), Format::Txt).unwrap();
        let c = extract_utf8(
            &write(
                "c.txt",
                &[&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat(),
            ),
            Format::Txt,
        )
        .unwrap();
        assert_eq!(a.text, b.text);
        assert_eq!(a.text, c.text, "a UTF-8 BOM is not content");
    }

    #[test]
    fn utf16_without_a_bom_is_not_guessed() {
        // Korean text as BOM-less UTF-16LE is not valid UTF-8. With no BOM
        // the encoding is unknowable, so it stays an error instead of a guess.
        let bytes = utf16("한국어 문서", true, false);
        assert!(extract_utf8(&write("nobom.txt", &bytes), Format::Txt).is_err());
    }

    #[test]
    fn broken_utf16_is_an_error() {
        let mut bytes = utf16("ok", true, true);
        bytes.extend([0x00, 0xD8]); // a lone high surrogate
        assert!(extract_utf8(&write("lone.txt", &bytes), Format::Txt).is_err());
        let odd = [&[0xFF, 0xFE][..], &[0x41, 0x00, 0x42]].concat(); // odd byte count
        assert!(extract_utf8(&write("odd.txt", &odd), Format::Txt).is_err());
    }

    #[test]
    fn invalid_utf8_without_a_bom_is_still_an_error() {
        assert!(extract_utf8(&write("bad.txt", &[0x80, 0x81, 0x82]), Format::Txt).is_err());
    }

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
