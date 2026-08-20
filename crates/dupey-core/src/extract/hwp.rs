//! hwp (binary, HWP 5.x) extraction: BodyText paragraph text.
//!
//! A .hwp is a CFB (OLE compound file). FileHeader flags tell whether
//! BodyText/SectionN streams are deflate-compressed (they almost always
//! are). Records are 4-byte headers (tag id, level, size in dwords;
//! size 0xFFF means an extended 4-byte size follows). Text lives in
//! HWPTAG_PARA_TEXT (67) as UTF-16LE, with control characters standing
//! in for tables, fields, and inline objects; those are dropped since
//! only comparable prose should survive extract.

use std::io::Read;
use std::path::Path;

use super::{CanonicalText, DocMeta, Format};
use crate::error::{Error, Result};

const HWPTAG_PARA_TEXT: u16 = 67;
const FLAG_COMPRESSED: u32 = 1;

pub(crate) fn extract_hwp(path: &Path) -> Result<CanonicalText> {
    let file = std::fs::File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut ole = cfb::CompoundFile::open(file).map_err(|e| extract_err(path, e))?;

    let header = read_stream(path, &mut ole, "/FileHeader")?;
    if header.len() < 40 || &header[0..17] != b"HWP Document File" {
        return Err(extract_err(path, "not a HWP 5.x document"));
    }
    let flags = u32::from_le_bytes(header[36..40].try_into().unwrap());

    let mut sections: Vec<String> = ole
        .walk()
        .filter_map(|e| {
            let p = e.path();
            (p.starts_with("/BodyText")
                && e.is_stream()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("Section")))
            .then(|| p.to_string_lossy().to_string())
        })
        .collect();
    sections.sort();
    if sections.is_empty() {
        return Err(extract_err(path, "no BodyText sections"));
    }

    let mut text = String::new();
    for name in &sections {
        let raw = read_stream(path, &mut ole, name)?;
        let body = if flags & FLAG_COMPRESSED != 0 {
            inflate(path, &raw)?
        } else {
            raw
        };
        text.push_str(&section_text(&body));
    }

    Ok(CanonicalText {
        path: path.to_path_buf(),
        format: Format::Hwp,
        text,
        meta: DocMeta::default(), // binary hwp summary info is skipped for now
    })
}

fn extract_err(path: &Path, e: impl std::fmt::Display) -> Error {
    Error::Extract {
        path: path.to_path_buf(),
        message: e.to_string(),
    }
}

fn read_stream(
    path: &Path,
    ole: &mut cfb::CompoundFile<std::fs::File>,
    name: &str,
) -> Result<Vec<u8>> {
    let mut stream = ole.open_stream(name).map_err(|e| extract_err(path, e))?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .map_err(|e| extract_err(path, e))?;
    Ok(buf)
}

fn inflate(path: &Path, raw: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(raw)
        .read_to_end(&mut out)
        .map_err(|e| extract_err(path, format!("deflate: {e}")))?;
    Ok(out)
}

/// Walk section records; collect UTF-16LE text of HWPTAG_PARA_TEXT,
/// dropping control placeholders, one line per paragraph record.
fn section_text(body: &[u8]) -> String {
    let mut out = String::new();
    let mut pos = 0usize;
    while pos + 4 <= body.len() {
        let header = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap());
        let tag_id = (header & 0x3FF) as u16;
        let mut size = ((header >> 20) & 0xFFF) as usize * 4;
        pos += 4;
        if size == 0xFFF * 4 {
            if pos + 4 > body.len() {
                break;
            }
            size = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
        }
        if pos + size > body.len() {
            break;
        }
        if tag_id == HWPTAG_PARA_TEXT {
            for chunk in body[pos..pos + size].chunks_exact(2) {
                let c = u16::from_le_bytes([chunk[0], chunk[1]]);
                match c {
                    // Control placeholders: fields/tables/objects. Only
                    // real prose characters survive extract.
                    0x0000..=0x001F | 0xE000..=0xF8FF => {}
                    _ => out.push(char::from_u32(c as u32).unwrap_or('\u{FFFD}')),
                }
            }
            out.push('\n');
        }
        pos += size;
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::extract::docx::tests::write_tmp;
    use crate::extract::Format;
    use std::io::Write;

    const BODYTEXT_PARA_TEXT: u16 = 67; // HWPTAG_PARA_TEXT

    /// Build a raw (uncompressed) BodyText/Section0 stream with the given
    /// paragraph texts, HWPTAG_PARA_TEXT records only.
    fn section_stream(paras: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in paras {
            // HWP record sizes are in dwords; real paragraph records pad
            // text up to a 4-byte boundary.
            let mut utf16: Vec<u8> = p
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes())
                .collect();
            while utf16.len() % 4 != 0 {
                utf16.extend_from_slice(&0u16.to_le_bytes());
            }
            let header: u32 =
                BODYTEXT_PARA_TEXT as u32 | (0u32 << 10) | ((utf16.len() as u32 / 4) << 20);
            out.extend_from_slice(&header.to_le_bytes());
            out.extend_from_slice(&utf16);
        }
        out
    }

    /// Minimal real HWP 5.x file: CFB container with a FileHeader whose
    /// flags say BodyText sections are stored uncompressed.
    pub(crate) fn make_hwp(paras: &[&str]) -> Vec<u8> {
        let section = section_stream(paras);
        let mut header = vec![0u8; 256];
        header[0..32].copy_from_slice(b"HWP Document File\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        header[32..36].copy_from_slice(&0x00050100u32.to_le_bytes()); // 5.1.0.0
        header[36..40].copy_from_slice(&0u32.to_le_bytes()); // flags: none
        let cursor = std::io::Cursor::new(Vec::new());
        let mut ole = cfb::CompoundFile::create(cursor).unwrap();
        {
            let mut s = ole.create_stream("FileHeader").unwrap();
            s.write_all(&header).unwrap();
        }
        ole.create_storage("BodyText").unwrap();
        {
            let mut s = ole.create_stream("BodyText/Section0").unwrap();
            s.write_all(&section).unwrap();
        }
        ole.into_inner().into_inner()
    }

    #[test]
    fn extracts_paragraph_text() {
        let bytes = make_hwp(&["사업 계획서", "예산은 3,200만 원이다.", "일정은 9월 시작."]);
        let path = write_tmp("dupey-hwp-text.hwp", &bytes);
        let got = extract_hwp(&path).unwrap();
        assert_eq!(got.format, Format::Hwp);
        assert_eq!(
            got.text,
            "사업 계획서\n예산은 3,200만 원이다.\n일정은 9월 시작.\n"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_non_hwp() {
        let path = write_tmp("dupey-hwp-bad.hwp", b"not a compound file");
        assert!(extract_hwp(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
