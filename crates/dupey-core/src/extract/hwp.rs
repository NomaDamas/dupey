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

    let meta = summary_meta(path, &mut ole);

    Ok(CanonicalText {
        path: path.to_path_buf(),
        format: Format::Hwp,
        text,
        meta,
    })
}

/// \005HwpSummaryInformation: OLE property set. We read PIDSI_EDITTIME
/// (10, VT_FILETIME) as the internal modified-time signal.
fn summary_meta(path: &Path, ole: &mut cfb::CompoundFile<std::fs::File>) -> DocMeta {
    let mut meta = DocMeta::default();
    let Ok(mut stream) = ole.open_stream("/\u{5}HwpSummaryInformation") else {
        return meta;
    };
    let mut buf = Vec::new();
    if stream.read_to_end(&mut buf).is_err() || buf.len() < 48 {
        return meta;
    }
    // Header: byte order, version, system id, clsid, set count, fmtid,
    // section offset. Minimal validation; a malformed stream yields None.
    if u16::from_le_bytes([buf[0], buf[1]]) != 0xFFFE {
        return meta;
    }
    let section_off = u32::from_le_bytes(buf[44..48].try_into().unwrap()) as usize;
    if section_off + 8 > buf.len() {
        return meta;
    }
    let prop_count =
        u32::from_le_bytes(buf[section_off + 4..section_off + 8].try_into().unwrap()) as usize;
    for i in 0..prop_count {
        let entry = section_off + 8 + i * 8;
        if entry + 8 > buf.len() {
            return meta;
        }
        let prop_id = u32::from_le_bytes(buf[entry..entry + 4].try_into().unwrap());
        let value_off =
            section_off + u32::from_le_bytes(buf[entry + 4..entry + 8].try_into().unwrap()) as usize;
        if prop_id != 10 || value_off + 12 > buf.len() {
            continue;
        }
        let vt = u32::from_le_bytes(buf[value_off..value_off + 4].try_into().unwrap());
        if vt != 0x0040 {
            // VT_FILETIME
            return meta;
        }
        let ft = u64::from_le_bytes(buf[value_off + 4..value_off + 12].try_into().unwrap());
        meta.modified = filetime_to_timestamp(ft);
        return meta;
    }
    let _ = path;
    meta
}

/// Windows FILETIME (100ns ticks since 1601-01-01 UTC) -> Timestamp.
fn filetime_to_timestamp(ft: u64) -> Option<jiff::Timestamp> {
    const TICKS_PER_SEC: u64 = 10_000_000;
    const EPOCH_OFFSET_SECS: i64 = 11_644_473_600; // 1601 -> 1970
    let secs = (ft / TICKS_PER_SEC) as i64 - EPOCH_OFFSET_SECS;
    let nanos = ((ft % TICKS_PER_SEC) * 100) as i64;
    jiff::Timestamp::new(secs, nanos as i32).ok()
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

const HWPTAG_LIST_HEADER: u16 = 72;

/// Walk section records; collect UTF-16LE text of HWPTAG_PARA_TEXT at
/// any level (table cells are nested PARA_TEXT), dropping control
/// placeholders. Table cell boundaries (LIST_HEADER) become tabs so cell
/// text survives as comparable content; one line per paragraph record.
fn section_text(body: &[u8]) -> String {
    let mut out = String::new();
    let mut pos = 0usize;
    let mut cells_in_row = 0usize;
    while pos + 4 <= body.len() {
        let header = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap());
        let tag_id = (header & 0x3FF) as u16;
        let level = (header >> 10) & 0x3FF;
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
        match tag_id {
            HWPTAG_PARA_TEXT => {
                if level > 0 {
                    // Nested paragraph: a table cell's text. Tabs between
                    // cells instead of the paragraph newline.
                    if cells_in_row > 0 && out.ends_with('\n') {
                        out.pop();
                        out.push('\t');
                    }
                    cells_in_row += 1;
                } else {
                    cells_in_row = 0;
                }
                for chunk in body[pos..pos + size].chunks_exact(2) {
                    let c = u16::from_le_bytes([chunk[0], chunk[1]]);
                    match c {
                        // Control placeholders: fields/objects. Only real
                        // prose characters survive extract.
                        0x0000..=0x001F | 0xE000..=0xF8FF => {}
                        _ => out.push(char::from_u32(c as u32).unwrap_or('\u{FFFD}')),
                    }
                }
                out.push('\n');
            }
            _ => {}
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

    /// OLE property set stream with a single VT_FILETIME property.
    fn property_set(prop_id: u32, filetime: u64) -> Vec<u8> {
        let mut out = Vec::new();
        // byte order + version + system id + clsid
        out.extend_from_slice(&0xFFFEu16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x0A20u32.to_le_bytes());
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(&1u32.to_le_bytes()); // num property sets
        out.extend_from_slice(&[0u8; 16]); // FMTID0
        let section_offset = 48u32;
        out.extend_from_slice(&section_offset.to_le_bytes());
        // section; offsets inside it are relative to the section start
        let props_start = 8u32; // section size + prop count
        let value_offset = props_start + 8; // after one id/offset entry
        let section_size = value_offset + 12; // VT_FILETIME: type + u64 + pad
        out.extend_from_slice(&section_size.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes()); // prop count
        out.extend_from_slice(&prop_id.to_le_bytes());
        out.extend_from_slice(&value_offset.to_le_bytes());
        out.extend_from_slice(&0x0040u32.to_le_bytes()); // VT_FILETIME
        out.extend_from_slice(&filetime.to_le_bytes());
        out
    }

    /// 2026-08-01T09:00:00Z as a Windows FILETIME (100ns since 1601).
    const FILETIME_2026_08_01: u64 = 0x01DD_2194_21FB_A800;

    /// Minimal real HWP 5.x file: CFB container with a FileHeader whose
    /// flags say BodyText sections are stored uncompressed.
    pub(crate) fn make_hwp(paras: &[&str]) -> Vec<u8> {
        make_hwp_with_summary(paras, None)
    }

    pub(crate) fn make_hwp_with_summary(paras: &[&str], edittime: Option<u64>) -> Vec<u8> {
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
        if let Some(ft) = edittime {
            let mut s = ole
                .create_stream("\u{5}HwpSummaryInformation")
                .unwrap();
            s.write_all(&property_set(10, ft)).unwrap();
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
    fn reads_summary_edit_time() {
        let bytes = make_hwp_with_summary(&["a"], Some(FILETIME_2026_08_01));
        let path = write_tmp("dupey-hwp-meta.hwp", &bytes);
        let got = extract_hwp(&path).unwrap();
        assert_eq!(
            got.meta.modified.unwrap().to_string(),
            "2026-08-01T09:00:00Z"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_summary_is_no_meta() {
        let bytes = make_hwp(&["a"]);
        let path = write_tmp("dupey-hwp-nometa.hwp", &bytes);
        let got = extract_hwp(&path).unwrap();
        assert_eq!(got.meta.modified, None);
        let _ = std::fs::remove_file(&path);
    }

    /// Section stream with a table: TABLE record followed by cell
    /// LIST_HEADER/PARA_TEXT chains at deeper levels.
    fn section_stream_with_table(before: &str, cells: &[&str], after: &str) -> Vec<u8> {
        const TABLE: u16 = 75;
        const LIST_HEADER: u16 = 72;
        let mut out = Vec::new();
        let mut put = |tag: u16, level: u32, text: &str| {
            let mut utf16: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            while utf16.len() % 4 != 0 {
                utf16.extend_from_slice(&0u16.to_le_bytes());
            }
            let header: u32 = tag as u32 | (level << 10) | ((utf16.len() as u32 / 4) << 20);
            out.extend_from_slice(&header.to_le_bytes());
            out.extend_from_slice(&utf16);
        };
        put(BODYTEXT_PARA_TEXT, 0, before);
        put(TABLE, 0, "");
        for cell in cells {
            put(LIST_HEADER, 1, "");
            put(BODYTEXT_PARA_TEXT, 2, cell);
        }
        put(BODYTEXT_PARA_TEXT, 0, after);
        out
    }

    #[test]
    fn table_cells_are_extracted() {
        let mut header = vec![0u8; 256];
        header[0..32].copy_from_slice(b"HWP Document File\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        header[32..36].copy_from_slice(&0x00050100u32.to_le_bytes());
        let section = section_stream_with_table(
            "예산 내역",
            &["인건비", "1,200", "서버비", "340"],
            "이상.",
        );
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
        let path = write_tmp("dupey-hwp-table.hwp", &ole.into_inner().into_inner());
        let got = extract_hwp(&path).unwrap();
        assert!(got.text.contains("예산 내역"), "{:?}", got.text);
        for cell in ["인건비", "1,200", "서버비", "340"] {
            assert!(got.text.contains(cell), "missing cell {cell:?} in {:?}", got.text);
        }
        assert!(got.text.contains("이상."), "{:?}", got.text);
        // Cells are tab-separated like xlsx rows.
        assert!(
            got.text.contains("인건비\t1,200\t서버비\t340"),
            "cells should be tab-separated: {:?}",
            got.text
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
