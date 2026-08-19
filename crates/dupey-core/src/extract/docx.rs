//! docx extraction: word/document.xml paragraph text only.
//!
//! Drops docProps, rsIds, styles, settings. Deleted tracked changes live in
//! `w:delText` and are excluded by construction (we only read `w:t`).
//! Internal provenance comes from docProps/core.xml (dcterms:modified,
//! cp:revision).

use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::{normalize_newlines, CanonicalText, DocMeta, Format};
use crate::error::{Error, Result};

pub(crate) fn extract_docx(path: &Path) -> Result<CanonicalText> {
    let file = std::fs::File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| extract_err(path, e))?;
    let document_xml = read_entry(path, &mut zip, "word/document.xml")?;
    let core_xml = read_entry(path, &mut zip, "docProps/core.xml").ok();
    Ok(CanonicalText {
        path: path.to_path_buf(),
        format: Format::Docx,
        text: normalize_newlines(&document_text(&document_xml)),
        meta: core_xml.as_deref().map(core_meta).unwrap_or_default(),
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

/// Paragraph text from word/document.xml: concat `w:t` runs, one line per
/// `w:p`. `w:delText`, `w:instrText`, headers/footers are not read.
fn document_text(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"t" => in_text = true,
                _ => {}
            },
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

/// dcterms:modified + cp:revision from docProps/core.xml.
fn core_meta(xml: &str) -> DocMeta {
    let mut reader = Reader::from_str(xml);
    let mut meta = DocMeta::default();
    let mut tag: Option<&'static str> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                tag = match e.local_name().as_ref() {
                    b"modified" => Some("modified"),
                    b"revision" => Some("revision"),
                    _ => None,
                }
            }
            Ok(Event::End(_)) => tag = None,
            Ok(Event::Text(e)) => {
                if let (Some(tag), Ok(s)) = (tag, e.decode()) {
                    match tag {
                        "modified" => {
                            meta.modified = s.parse::<jiff::Timestamp>().ok();
                        }
                        "revision" => meta.revision = s.trim().parse::<u32>().ok(),
                        _ => {}
                    }
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
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal real docx (zip) with the given paragraphs.
    pub(crate) fn make_docx(paragraphs: &[&str], modified: &str, revision: u32) -> Vec<u8> {
        let runs: String = paragraphs
            .iter()
            .map(|p| format!("<w:p><w:r><w:t xml:space=\"preserve\">{p}</w:t></w:r></w:p>"))
            .collect();
        let document = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
             <w:body>{runs}</w:body></w:document>"
        );
        let core = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
             xmlns:dcterms=\"http://purl.org/dc/terms/\">\
             <dcterms:modified xsi:type=\"dcterms:W3CDTF\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">{modified}</dcterms:modified>\
             <cp:revision>{revision}</cp:revision></cp:coreProperties>"
        );
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("word/document.xml", opts).unwrap();
            zip.write_all(document.as_bytes()).unwrap();
            zip.start_file("docProps/core.xml", opts).unwrap();
            zip.write_all(core.as_bytes()).unwrap();
            // Volatile noise a real docx carries; extract must ignore it.
            zip.start_file("docProps/app.xml", opts).unwrap();
            zip.write_all(b"<Properties><Application>dupey-test</Application></Properties>")
                .unwrap();
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    pub(crate) fn write_tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn extracts_paragraph_text() {
        let bytes = make_docx(
            &["제안서 초안", "예산은 3,200만 원이다.", "일정은 9월 시작."],
            "2026-08-01T09:00:00Z",
            3,
        );
        let path = write_tmp("dupey-docx-text.docx", &bytes);
        let got = extract_docx(&path).unwrap();
        assert_eq!(got.format, Format::Docx);
        assert_eq!(
            got.text,
            "제안서 초안\n예산은 3,200만 원이다.\n일정은 9월 시작.\n"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reads_internal_modified_and_revision() {
        let bytes = make_docx(&["a"], "2026-08-01T09:00:00Z", 7);
        let path = write_tmp("dupey-docx-meta.docx", &bytes);
        let got = extract_docx(&path).unwrap();
        assert_eq!(
            got.meta.modified.unwrap().to_string(),
            "2026-08-01T09:00:00Z"
        );
        assert_eq!(got.meta.revision, Some(7));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ignores_volatile_parts() {
        // Same content, different app.xml noise and zip metadata => same text.
        let a = make_docx(&["same body"], "2026-08-01T09:00:00Z", 1);
        let b = make_docx(&["same body"], "2026-08-02T09:00:00Z", 9);
        let pa = write_tmp("dupey-docx-a.docx", &a);
        let pb = write_tmp("dupey-docx-b.docx", &b);
        let ta = extract_docx(&pa).unwrap();
        let tb = extract_docx(&pb).unwrap();
        assert_eq!(ta.text, tb.text);
        assert_eq!(
            crate::exact_hash(&ta.text),
            crate::exact_hash(&tb.text),
            "resaved docx with same body must hash identically"
        );
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);
    }
}
