//! hwpx extraction: Contents/section*.xml body text only.
//!
//! OWPML text lives in `hp:t` inside `hp:p` runs. View metadata
//! (META-INF, settings, preview images) is ignored. Internal provenance
//! comes from Contents/content.hpf OPF metadata (dc:date).

use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::{normalize_newlines, CanonicalText, DocMeta, Format};
use crate::error::{Error, Result};

pub(crate) fn extract_hwpx(path: &Path) -> Result<CanonicalText> {
    let file = std::fs::File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| extract_err(path, e))?;

    let mut sections: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|e| e.name().to_string()))
        .filter(|name| {
            name.starts_with("Contents/section") && name.ends_with(".xml")
        })
        .collect();
    sections.sort();
    if sections.is_empty() {
        return Err(extract_err(path, "no Contents/section*.xml entries"));
    }

    let mut text = String::new();
    for name in &sections {
        let xml = read_entry(path, &mut zip, name)?;
        text.push_str(&section_text(&xml));
    }
    let meta = read_entry(path, &mut zip, "Contents/content.hpf")
        .map(|hpf| hpf_meta(&hpf))
        .unwrap_or_default();

    Ok(CanonicalText {
        path: path.to_path_buf(),
        format: Format::Hwpx,
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

/// Body text of one section: concat `hp:t`, one line per `hp:p`.
fn section_text(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if e.local_name().as_ref() == b"t" {
                    in_text = true;
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"t" => in_text = false,
                b"p" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Text(e)) if in_text => {
                if let Ok(s) = e.decode() {
                    out.push_str(&s);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

/// dc:date from Contents/content.hpf OPF metadata.
fn hpf_meta(xml: &str) -> DocMeta {
    let mut reader = Reader::from_str(xml);
    let mut meta = DocMeta::default();
    let mut in_date = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => in_date = e.local_name().as_ref() == b"date",
            Ok(Event::End(_)) => in_date = false,
            Ok(Event::Text(e)) if in_date => {
                if let Ok(s) = e.decode() {
                    meta.modified = s.trim().parse::<jiff::Timestamp>().ok();
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

    /// Build a minimal real hwpx (zip) with one section of paragraphs.
    fn make_hwpx(paragraphs: &[&str], date: &str) -> Vec<u8> {
        let runs: String = paragraphs
            .iter()
            .map(|p| {
                format!(
                    "<hp:p><hp:run><hp:secPr/><hp:ctrl/><hp:t>{p}</hp:t></hp:run></hp:p>"
                )
            })
            .collect();
        let section = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <hs:sec xmlns:hs=\"http://www.hancom.co.kr/hwpml/2011/section\" \
             xmlns:hp=\"http://www.hancom.co.kr/hwpml/2011/paragraph\">{runs}</hs:sec>"
        );
        let hpf = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <opf:package xmlns:opf=\"http://www.idpf.org/2007/opf\" \
             xmlns:dc=\"http://purl.org/dc/elements/1.1/\" unique-identifier=\"id\">\
             <opf:metadata><dc:title>테스트</dc:title><dc:date>{date}</dc:date></opf:metadata>\
             <opf:manifest><opf:item id=\"section0\" href=\"Contents/section0.xml\"/></opf:manifest>\
             </opf:package>"
        );
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("mimetype", opts).unwrap();
            zip.write_all(b"application/hwp+zip").unwrap();
            zip.start_file("Contents/content.hpf", opts).unwrap();
            zip.write_all(hpf.as_bytes()).unwrap();
            zip.start_file("Contents/section0.xml", opts).unwrap();
            zip.write_all(section.as_bytes()).unwrap();
            // View metadata; extract must ignore it.
            zip.start_file("META-INF/container.xml", opts).unwrap();
            zip.write_all(b"<container/>").unwrap();
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn extracts_section_text() {
        let bytes = make_hwpx(
            &["사업 계획서", "예산은 3,200만 원이다.", "일정은 9월 시작."],
            "2026-08-01T09:00:00Z",
        );
        let path = write_tmp("dupey-hwpx-text.hwpx", &bytes);
        let got = extract_hwpx(&path).unwrap();
        assert_eq!(got.format, Format::Hwpx);
        assert_eq!(
            got.text,
            "사업 계획서\n예산은 3,200만 원이다.\n일정은 9월 시작.\n"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reads_internal_date() {
        let bytes = make_hwpx(&["a"], "2026-08-01T09:00:00Z");
        let path = write_tmp("dupey-hwpx-meta.hwpx", &bytes);
        let got = extract_hwpx(&path).unwrap();
        assert_eq!(
            got.meta.modified.unwrap().to_string(),
            "2026-08-01T09:00:00Z"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ignores_view_metadata() {
        let a = make_hwpx(&["same body"], "2026-08-01T09:00:00Z");
        let b = make_hwpx(&["same body"], "2026-08-02T09:00:00Z");
        let pa = write_tmp("dupey-hwpx-a.hwpx", &a);
        let pb = write_tmp("dupey-hwpx-b.hwpx", &b);
        let ta = extract_hwpx(&pa).unwrap();
        let tb = extract_hwpx(&pb).unwrap();
        assert_eq!(ta.text, tb.text);
        assert_eq!(crate::exact_hash(&ta.text), crate::exact_hash(&tb.text));
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);
    }
}
