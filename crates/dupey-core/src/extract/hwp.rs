//! hwp (binary, HWP 5.x) extraction: BodyText paragraph text.
//!
//! A .hwp is a CFB (OLE compound file). FileHeader flags tell whether
//! BodyText/SectionN streams are deflate-compressed (they almost always
//! are). Records are 4-byte headers (tag id, level, size in bytes;
//! size 0xFFF means an extended 4-byte size follows). Text lives in
//! HWPTAG_PARA_TEXT (67) as UTF-16LE, with control characters standing
//! in for tables, fields, and inline objects. Skip their binary payloads,
//! while retaining displayed field values and nested paragraph text.

use std::io::Read;
use std::path::Path;

use super::{CanonicalText, DocMeta, Format};
use crate::error::{Error, Result};

const HWPTAG_PARA_TEXT: u16 = 67;
const FLAG_COMPRESSED: u32 = 1;
const FLAG_PASSWORD: u32 = 1 << 1;
const FLAG_DISTRIBUTABLE: u32 = 1 << 2;

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
    if flags & FLAG_PASSWORD != 0 {
        return Err(extract_err(path, "password-encrypted HWP is not supported"));
    }
    if flags & FLAG_DISTRIBUTABLE != 0 {
        return Err(extract_err(
            path,
            "distribution HWP (encrypted ViewText) is not supported",
        ));
    }

    let mut sections: Vec<(u32, String)> = ole
        .walk()
        .filter_map(|e| {
            let p = e.path();
            if !e.is_stream() || p.parent() != Some(Path::new("/BodyText")) {
                return None;
            }
            let number = p
                .file_name()?
                .to_str()?
                .strip_prefix("Section")?
                .parse()
                .ok()?;
            Some((number, p.to_string_lossy().to_string()))
        })
        .collect();
    sections.sort();
    if sections.is_empty() {
        return Err(extract_err(path, "no BodyText sections"));
    }

    let mut text = String::new();
    for (_, name) in &sections {
        let raw = read_stream(path, &mut ole, name)?;
        let body = if flags & FLAG_COMPRESSED != 0 {
            inflate_raw(path, &raw)?
        } else {
            raw
        };
        let section = section_text(&body).map_err(|e| extract_err(path, format!("{name}: {e}")))?;
        text.push_str(&section);
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
fn summary_meta<F: Read + std::io::Seek>(path: &Path, ole: &mut cfb::CompoundFile<F>) -> DocMeta {
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
        let value_off = section_off
            + u32::from_le_bytes(buf[entry + 4..entry + 8].try_into().unwrap()) as usize;
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

fn inflate_raw(path: &Path, raw: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = flate2::Decompress::new(false);
    let mut out = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        let status = decoder
            .decompress(
                &raw[before_in as usize..],
                &mut buffer,
                flate2::FlushDecompress::None,
            )
            .map_err(|e| extract_err(path, format!("deflate: {e}")))?;
        let written = (decoder.total_out() - before_out) as usize;
        out.extend_from_slice(&buffer[..written]);
        if status == flate2::Status::StreamEnd {
            return Ok(out);
        }
        // Read-based decoders accept an input EOF without the final DEFLATE
        // block. Require StreamEnd so a valid prefix cannot become a hash.
        if decoder.total_in() == before_in && written == 0 {
            return Err(extract_err(path, "deflate: truncated stream"));
        }
    }
}

/// Walk section records; collect UTF-16LE text of HWPTAG_PARA_TEXT at
/// any level (table cells are nested PARA_TEXT), dropping control
/// placeholders. LIST_HEADER marks a new paragraph list (including table
/// cells); paragraph nesting alone does not imply a new cell.
fn section_text(body: &[u8]) -> std::result::Result<String, String> {
    let mut out = String::new();
    let mut pos = 0usize;
    while pos < body.len() {
        let record_pos = pos;
        if body.len() - pos < 4 {
            return Err(format!("truncated record header at byte {pos}"));
        }
        let header = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap());
        let tag_id = (header & 0x3FF) as u16;
        let mut size = ((header >> 20) & 0xFFF) as usize;
        pos += 4;
        if size == 0xFFF {
            if body.len() - pos < 4 {
                return Err(format!(
                    "truncated extended record size at byte {record_pos}"
                ));
            }
            size = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
        }
        if size > body.len() - pos {
            return Err(format!(
                "truncated record {tag_id} at byte {record_pos}: expected {size} bytes, have {}",
                body.len() - pos
            ));
        }
        if tag_id == 72 {
            // HWPTAG_LIST_HEADER
            // Emit every boundary immediately: empty cells can have a
            // PARA_HEADER without any PARA_TEXT, including trailing cells.
            if out.ends_with('\n') {
                out.pop();
            }
            out.push('\t');
        }
        if tag_id == HWPTAG_PARA_TEXT {
            let text = paragraph_text(&body[pos..pos + size])
                .map_err(|e| format!("PARA_TEXT at byte {record_pos}: {e}"))?;
            out.push_str(&text);
            out.push('\n');
        }
        pos += size;
    }
    Ok(out)
}

fn paragraph_text(data: &[u8]) -> std::result::Result<String, String> {
    if !data.len().is_multiple_of(2) {
        return Err("odd UTF-16 byte length".into());
    }
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    let mut text = Vec::new();
    let mut i = 0;
    while i < units.len() {
        let code = units[i];
        // HWP 5 control table: inline and extended controls occupy eight
        // UTF-16 units, including the opening and closing control codes.
        if matches!(code, 1..=9 | 11..=12 | 14..=23) {
            if units.len() - i < 8 || units[i + 7] != code {
                return Err(format!(
                    "truncated or invalid control {code} at UTF-16 unit {i}"
                ));
            }
            if code == 9 {
                text.push(b'\t' as u16);
            }
            i += 8;
            continue;
        }
        match code {
            10 => text.push(b'\n' as u16),
            24 => text.push(b'-' as u16),
            30 | 31 => text.push(b' ' as u16),
            0..=31 => {}          // paragraph end is supplied by section_text
            _ => text.push(code), // preserve surrogate pairs and private-use glyphs
        }
        i += 1;
    }
    String::from_utf16(&text).map_err(|e| format!("invalid UTF-16: {e}"))
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
            let utf16: Vec<u8> = p.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            put_record(&mut out, BODYTEXT_PARA_TEXT, 0, &utf16);
        }
        out
    }

    fn put_record(out: &mut Vec<u8>, tag: u16, level: u32, data: &[u8]) {
        let size = data.len().min(0xFFF) as u32;
        out.extend_from_slice(&(tag as u32 | (level << 10) | (size << 20)).to_le_bytes());
        if size == 0xFFF {
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        }
        out.extend_from_slice(data);
    }

    fn hwp_sections(sections: &[(&str, Vec<u8>)], flags: u32) -> Vec<u8> {
        let mut header = vec![0u8; 256];
        header[..17].copy_from_slice(b"HWP Document File");
        header[32..36].copy_from_slice(&0x05000300u32.to_le_bytes());
        header[36..40].copy_from_slice(&flags.to_le_bytes());
        let mut ole = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new())).unwrap();
        ole.create_stream("FileHeader")
            .unwrap()
            .write_all(&header)
            .unwrap();
        ole.create_storage("BodyText").unwrap();
        for (name, bytes) in sections {
            let body = if flags & FLAG_COMPRESSED != 0 {
                let mut encoder =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(bytes).unwrap();
                encoder.finish().unwrap()
            } else {
                bytes.clone()
            };
            ole.create_stream(format!("BodyText/{name}"))
                .unwrap()
                .write_all(&body)
                .unwrap();
        }
        ole.into_inner().into_inner()
    }

    fn extract_section(name: &str, section: Vec<u8>) -> Result<CanonicalText> {
        let path = write_tmp(name, &hwp_sections(&[("Section0", section)], 0));
        let result = extract_hwp(&path);
        let _ = std::fs::remove_file(path);
        result
    }

    #[test]
    fn byte_sized_records_without_padding() {
        // Literal record headers: tag 67, level 1, sizes 2 and 4 BYTES.
        // Independent of put_record, to avoid teaching fixtures a parser bug.
        let body = vec![
            0x43, 0x04, 0x20, 0x00, b'A', 0, 0x43, 0x04, 0x40, 0x00, b'B', 0, b'C', 0,
        ];
        assert_eq!(
            extract_section("dupey-hwp-byte-size.hwp", body)
                .unwrap()
                .text,
            "A\nBC\n"
        );
    }

    #[test]
    fn daily_report_edits_have_distinct_hashes() {
        for flags in [0, FLAG_COMPRESSED] {
            let mut hashes = Vec::new();
            for (i, (date, value)) in [
                ("2026-09-14", "1,200"),
                ("2026-09-15", "1,200"),
                ("2026-09-14", "1,201"),
            ]
            .iter()
            .enumerate()
            {
                // Non-text records precede the body, as in real HWP files.
                let mut section = Vec::new();
                put_record(&mut section, 66, 0, &[0; 22]);
                section.extend(section_stream_with_table(
                    "일일 보고서",
                    &[date, value],
                    "끝",
                ));
                let path = write_tmp(
                    &format!("dupey-report-{flags}-{i}.hwp"),
                    &hwp_sections(&[("Section0", section)], flags),
                );
                let text = extract_hwp(&path).unwrap().text;
                assert!(text.contains(date) && text.contains(value), "{text:?}");
                hashes.push(crate::exact_hash_hex(&text));
                let _ = std::fs::remove_file(path);
            }
            assert_ne!(hashes[0], hashes[1]);
            assert_ne!(hashes[0], hashes[2]);
        }
    }

    #[test]
    fn extended_text_record_keeps_following_paragraph() {
        let long = "긴 문단".repeat(1000);
        let got = extract_section("dupey-hwp-long.hwp", section_stream(&[&long, "다음"])).unwrap();
        assert_eq!(got.text, format!("{long}\n다음\n"));
    }

    #[test]
    fn controls_skip_payload_but_keep_field_values_and_separators() {
        let mut units = Vec::new();
        // Field payload is binary and may contain printable values or surrogates.
        units.extend([3, 0x4141, 0x4242, 0xD800, 5, 6, 7, 3]);
        units.extend("2026-09-15".encode_utf16());
        units.extend([4, 0, 0, 0, 0, 0, 0, 4]);
        units.extend([9, 100, 0, 0, 0, 0, 0, 9]);
        units.extend("1,201".encode_utf16());
        units.extend([10, 0x41, 30, 0x42, 31, 0x43, 24, 0x44, 13]);
        let data: Vec<_> = units.into_iter().flat_map(u16::to_le_bytes).collect();
        let mut body = Vec::new();
        put_record(&mut body, 67, 1, &data);
        assert_eq!(
            extract_section("dupey-hwp-controls.hwp", body)
                .unwrap()
                .text,
            "2026-09-15\t1,201\nA B C-D\n"
        );
    }

    #[test]
    fn unicode_supplementary_and_private_use_characters_survive() {
        let text = "한글 𠀀 😀 \u{e001}";
        let got = extract_section("dupey-hwp-unicode.hwp", section_stream(&[text])).unwrap();
        assert_eq!(got.text, format!("{text}\n"));
    }

    #[test]
    fn malformed_records_never_return_a_partial_body() {
        let tails = [
            vec![0x43],                      // incomplete record header
            vec![0x43, 0, 0xF0, 0xFF],       // missing extended size
            vec![0x43, 0, 0xA0, 0, 0x41, 0], // short payload
            vec![0x43, 0, 0x10, 0, 0x41],    // odd UTF-16 byte length
            vec![0x43, 0, 0x20, 0, 0, 0xD8], // unpaired surrogate
            vec![0x43, 0, 0x20, 0, 3, 0],    // truncated field control
        ];
        for (i, tail) in tails.into_iter().enumerate() {
            let mut body = section_stream(&["공통 머리말"]);
            body.extend(tail);
            assert!(
                extract_section(&format!("dupey-hwp-truncated-{i}.hwp"), body).is_err(),
                "malformed case {i} returned success"
            );
        }
    }

    #[test]
    fn truncated_deflate_never_returns_a_partial_body() {
        // A non-final stored DEFLATE block contains a whole valid paragraph.
        // Without a final block, returning it would hash an incomplete file.
        let body = section_stream(&["공통 머리말"]);
        let len = body.len() as u16;
        let mut raw = vec![0];
        raw.extend_from_slice(&len.to_le_bytes());
        raw.extend_from_slice(&(!len).to_le_bytes());
        raw.extend_from_slice(&body);
        assert!(inflate_raw(Path::new("truncated.hwp"), &raw).is_err());
        assert!(inflate_raw(Path::new("empty.hwp"), &[]).is_err());
        raw.extend_from_slice(&[1, 0, 0, 255, 255]); // final empty stored block
        assert_eq!(inflate_raw(Path::new("complete.hwp"), &raw).unwrap(), body);
    }

    #[test]
    fn empty_cells_preserve_value_positions() {
        let mut texts = Vec::new();
        for (i, cells) in [["A", "", "B"], ["A", "B", ""], ["", "A", "B"]]
            .iter()
            .enumerate()
        {
            let mut body = section_stream(&["보고서"]);
            for cell in cells {
                put_record(&mut body, 72, 2, &[0; 34]);
                put_record(&mut body, 66, 2, &[0; 22]);
                // Empty cells need not carry a PARA_TEXT record.
                if !cell.is_empty() {
                    let data: Vec<_> = cell.encode_utf16().flat_map(u16::to_le_bytes).collect();
                    put_record(&mut body, 67, 3, &data);
                }
            }
            texts.push(
                extract_section(&format!("dupey-hwp-empty-cells-{i}.hwp"), body)
                    .unwrap()
                    .text,
            );
        }
        assert_ne!(
            crate::exact_hash_hex(&texts[0]),
            crate::exact_hash_hex(&texts[1])
        );
        assert_ne!(
            crate::exact_hash_hex(&texts[0]),
            crate::exact_hash_hex(&texts[2])
        );
        assert_ne!(
            crate::exact_hash_hex(&texts[1]),
            crate::exact_hash_hex(&texts[2])
        );
    }

    #[test]
    fn sections_are_read_in_numeric_order() {
        let sections: Vec<_> = (0..12)
            .map(|i| (format!("Section{i}"), section_stream(&[&i.to_string()])))
            .collect();
        let sections: Vec<_> = sections
            .iter()
            .map(|(n, b)| (n.as_str(), b.clone()))
            .collect();
        let path = write_tmp("dupey-hwp-section-order.hwp", &hwp_sections(&sections, 0));
        let got = extract_hwp(&path).unwrap();
        assert_eq!(
            got.text,
            (0..12).map(|i| format!("{i}\n")).collect::<String>()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn protected_documents_are_explicit_errors() {
        for flags in [2, 4] {
            let path = write_tmp(
                &format!("dupey-hwp-protected-{flags}.hwp"),
                &hwp_sections(&[("Section0", section_stream(&["미리보기"]))], flags),
            );
            assert!(extract_hwp(&path).is_err());
            let _ = std::fs::remove_file(path);
        }
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

    fn make_compressed_hwp(paras: &[&str]) -> Vec<u8> {
        let section = section_stream(paras);
        let mut compressed = Vec::new();
        let mut encoder =
            flate2::write::DeflateEncoder::new(&mut compressed, flate2::Compression::default());
        encoder.write_all(&section).unwrap();
        encoder.finish().unwrap();

        let mut header = vec![0u8; 256];
        header[0..32].copy_from_slice(b"HWP Document File\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        header[32..36].copy_from_slice(&0x00050100u32.to_le_bytes());
        header[36..40].copy_from_slice(&FLAG_COMPRESSED.to_le_bytes());
        let cursor = std::io::Cursor::new(Vec::new());
        let mut ole = cfb::CompoundFile::create(cursor).unwrap();
        {
            let mut s = ole.create_stream("FileHeader").unwrap();
            s.write_all(&header).unwrap();
        }
        ole.create_storage("BodyText").unwrap();
        {
            let mut s = ole.create_stream("BodyText/Section0").unwrap();
            s.write_all(&compressed).unwrap();
        }
        ole.into_inner().into_inner()
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
            let mut s = ole.create_stream("\u{5}HwpSummaryInformation").unwrap();
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
    fn extracts_raw_deflate_compressed_paragraph_text() {
        let bytes =
            make_compressed_hwp(&["압축 사업 계획서", "실제 HWP 5.x는 raw deflate를 사용한다."]);
        let path = write_tmp("dupey-hwp-compressed.hwp", &bytes);
        let got = extract_hwp(&path).unwrap();
        assert_eq!(
            got.text,
            "압축 사업 계획서\n실제 HWP 5.x는 raw deflate를 사용한다.\n"
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
            let utf16: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            put_record(&mut out, tag, level, &utf16);
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
        let section =
            section_stream_with_table("예산 내역", &["인건비", "1,200", "서버비", "340"], "이상.");
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
            assert!(
                got.text.contains(cell),
                "missing cell {cell:?} in {:?}",
                got.text
            );
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
