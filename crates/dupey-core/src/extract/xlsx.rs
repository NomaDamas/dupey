//! xlsx extraction: cell values row-major, tab between cells, newline
//! per row. Shared strings are resolved; styles, calcChain, and other
//! volatile parts are ignored. Provenance from docProps/core.xml.

use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::{normalize_newlines, CanonicalText, DocMeta, Format};
use crate::error::{Error, Result};

pub(crate) fn extract_xlsx(path: &Path) -> Result<CanonicalText> {
    let file = std::fs::File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| extract_err(path, e))?;

    let shared = read_entry(path, &mut zip, "xl/sharedStrings.xml")
        .map(|s| shared_strings(&s))
        .unwrap_or_default();

    let mut sheets: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|e| e.name().to_string()))
        .filter(|name| name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml"))
        .collect();
    sheets.sort();

    let mut text = String::new();
    for name in &sheets {
        let xml = read_entry(path, &mut zip, name)?;
        text.push_str(&sheet_text(&xml, &shared));
    }
    let meta = read_entry(path, &mut zip, "docProps/core.xml")
        .map(|c| core_meta(&c))
        .unwrap_or_default();

    Ok(CanonicalText {
        path: path.to_path_buf(),
        format: Format::Xlsx,
        text: normalize_newlines(&text),
        meta,
    })
}

fn extract_err(path: &Path, e: impl std::fmt::Display) -> Error {
    Error::Extract {
        path: path.to_path_buf(),
        message: e.to_string(),
    }
}

fn read_entry(
    path: &Path,
    zip: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
) -> Result<String> {
    let mut entry = zip.by_name(name).map_err(|e| extract_err(path, e))?;
    let mut buf = String::new();
    entry
        .read_to_string(&mut buf)
        .map_err(|e| extract_err(path, e))?;
    Ok(buf)
}

/// Shared string table: si text in order.
fn shared_strings(xml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut in_t = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"si" => cur = Some(String::new()),
                b"t" => in_t = true,
                _ => {}
            },
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"si" => {
                    if let Some(s) = cur.take() {
                        out.push(s);
                    }
                }
                b"t" => in_t = false,
                _ => {}
            },
            Ok(Event::Text(e)) if in_t => {
                if let (Some(cur), Ok(s)) = (cur.as_mut(), e.decode()) {
                    cur.push_str(&s);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Cells row-major: value of each c (shared-string resolved), tab-joined,
/// newline per row. Inline numbers pass through as text.
fn sheet_text(xml: &str, shared: &[String]) -> String {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut cell_is_shared = false;
    let mut in_v = false;
    let mut in_row = false;
    let mut cells_in_row = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"row" => {
                    in_row = true;
                    cells_in_row = 0;
                }
                b"c" => {
                    cell_is_shared = e
                        .attributes()
                        .flatten()
                        .any(|a| a.key.local_name().as_ref() == b"t" && a.value.as_ref() == b"s");
                    if in_row && cells_in_row > 0 {
                        out.push('\t');
                    }
                    cells_in_row += 1;
                }
                b"v" => in_v = true,
                _ => {}
            },
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"row" => {
                    out.push('\n');
                    in_row = false;
                }
                b"v" => in_v = false,
                _ => {}
            },
            Ok(Event::Text(e)) if in_v => {
                if let Ok(s) = e.decode() {
                    if cell_is_shared {
                        if let Some(v) = s.trim().parse::<usize>().ok().and_then(|i| shared.get(i)) {
                            out.push_str(v);
                        }
                    } else {
                        out.push_str(&s);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

fn core_meta(xml: &str) -> DocMeta {
    let mut reader = Reader::from_str(xml);
    let mut meta = DocMeta::default();
    let mut in_modified = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => in_modified = e.local_name().as_ref() == b"modified",
            Ok(Event::End(_)) => in_modified = false,
            Ok(Event::Text(e)) if in_modified => {
                if let Ok(s) = e.decode() {
                    meta.modified = s.parse::<jiff::Timestamp>().ok();
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::docx::tests::write_tmp;
    use crate::extract::Format;
    use std::io::Write;

    /// Minimal xlsx: shared strings + one sheet with rows of cells.
    fn make_xlsx(strings: &[&str], rows: &[&[usize]], modified: &str) -> Vec<u8> {
        let sst_items: String = strings
            .iter()
            .map(|s| format!("<si><t xml:space=\"preserve\">{s}</t></si>"))
            .collect();
        let sst = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">{sst_items}</sst>"
        );
        let rows_xml: String = rows
            .iter()
            .enumerate()
            .map(|(r, row)| {
                let cells: String = row
                    .iter()
                    .enumerate()
                    .map(|(c, &si)| {
                        format!(
                            "<c r=\"{}{}\" t=\"s\"><v>{si}</v></c>",
                            (b'A' + c as u8) as char,
                            r + 1
                        )
                    })
                    .collect();
                format!("<row r=\"{}\">{cells}</row>", r + 1)
            })
            .collect();
        let sheet = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
             <sheetData>{rows_xml}</sheetData></worksheet>"
        );
        let core = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
             xmlns:dcterms=\"http://purl.org/dc/terms/\">\
             <dcterms:modified xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:type=\"dcterms:W3CDTF\">{modified}</dcterms:modified>\
             </cp:coreProperties>"
        );
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("xl/sharedStrings.xml", opts).unwrap();
            zip.write_all(sst.as_bytes()).unwrap();
            zip.start_file("xl/worksheets/sheet1.xml", opts).unwrap();
            zip.write_all(sheet.as_bytes()).unwrap();
            zip.start_file("docProps/core.xml", opts).unwrap();
            zip.write_all(core.as_bytes()).unwrap();
            zip.start_file("xl/calcChain.xml", opts).unwrap();
            zip.write_all(b"<calcChain/>").unwrap(); // volatile; must be ignored
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn extracts_cell_values_row_major() {
        let bytes = make_xlsx(
            &["항목", "금액", "인건비", "1,200", "서버비", "340"],
            &[&[0, 1], &[2, 3], &[4, 5]],
            "2026-08-01T09:00:00Z",
        );
        let path = write_tmp("dupey-xlsx-text.xlsx", &bytes);
        let got = extract_xlsx(&path).unwrap();
        assert_eq!(got.format, Format::Xlsx);
        assert_eq!(got.text, "항목\t금액\n인건비\t1,200\n서버비\t340\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ignores_calc_chain_and_reads_modified() {
        let bytes = make_xlsx(&["a"], &[&[0]], "2026-08-03T11:00:00Z");
        let path = write_tmp("dupey-xlsx-meta.xlsx", &bytes);
        let got = extract_xlsx(&path).unwrap();
        assert_eq!(got.text, "a\n");
        assert_eq!(got.meta.modified.unwrap().to_string(), "2026-08-03T11:00:00Z");
        let _ = std::fs::remove_file(&path);
    }
}
