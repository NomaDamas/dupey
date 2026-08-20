//! pptx extraction: ppt/slides/slideN.xml text runs (a:t), in slide
//! order. Speaker notes (ppt/notesSlides) are not body text. Internal
//! provenance from docProps/core.xml, same as docx.

use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::{normalize_newlines, CanonicalText, DocMeta, Format};
use crate::error::{Error, Result};

pub(crate) fn extract_pptx(path: &Path) -> Result<CanonicalText> {
    let file = std::fs::File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| extract_err(path, e))?;

    let mut slides: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|e| e.name().to_string()))
        .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
        .collect();
    slides.sort_by_key(|n| slide_number(n));

    let mut text = String::new();
    for name in &slides {
        let xml = read_entry(path, &mut zip, name)?;
        text.push_str(&slide_text(&xml));
    }
    let meta = read_entry(path, &mut zip, "docProps/core.xml")
        .map(|c| core_meta(&c))
        .unwrap_or_default();

    Ok(CanonicalText {
        path: path.to_path_buf(),
        format: Format::Pptx,
        text: normalize_newlines(&text),
        meta,
    })
}

fn slide_number(name: &str) -> u32 {
    name.trim_start_matches("ppt/slides/slide")
        .trim_end_matches(".xml")
        .parse()
        .unwrap_or(0)
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

/// a:t runs concatenated, one line per a:p.
fn slide_text(xml: &str) -> String {
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
                        "modified" => meta.modified = s.parse::<jiff::Timestamp>().ok(),
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
    use crate::extract::docx::tests::write_tmp;
    use crate::extract::Format;
    use std::io::Write;

    /// Minimal pptx: N slides with the given texts (one shape each).
    pub(crate) fn make_pptx(slides: &[&str], modified: &str) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (i, text) in slides.iter().enumerate() {
                let slide = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                     <p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" \
                     xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">\
                     <p:cSld><p:spTree><p:sp><p:txBody>\
                     <a:p><a:r><a:t>{text}</a:t></a:r></a:p>\
                     </p:txBody></p:sp></p:spTree></p:cSld></p:sld>"
                );
                zip.start_file(format!("ppt/slides/slide{}.xml", i + 1), opts)
                    .unwrap();
                zip.write_all(slide.as_bytes()).unwrap();
            }
            let core = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                 <cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
                 xmlns:dcterms=\"http://purl.org/dc/terms/\">\
                 <dcterms:modified xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:type=\"dcterms:W3CDTF\">{modified}</dcterms:modified>\
                 <cp:revision>4</cp:revision></cp:coreProperties>"
            );
            zip.start_file("docProps/core.xml", opts).unwrap();
            zip.write_all(core.as_bytes()).unwrap();
            zip.start_file("ppt/notesSlides/notesSlide1.xml", opts).unwrap();
            let notes = "<p:notes xmlns:p=\"x\" xmlns:a=\"y\"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>노트는 본문이 아니다</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>";
            zip.write_all(notes.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn extracts_slide_text_in_order() {
        let bytes = make_pptx(
            &["1분기 실적", "매출 12억, 전분기 대비 8% 증가", "2분기 목표"],
            "2026-08-01T09:00:00Z",
        );
        let path = write_tmp("dupey-pptx-text.pptx", &bytes);
        let got = extract_pptx(&path).unwrap();
        assert_eq!(got.format, Format::Pptx);
        assert_eq!(got.text, "1분기 실적\n매출 12억, 전분기 대비 8% 증가\n2분기 목표\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ignores_notes_and_reads_modified() {
        let bytes = make_pptx(&["슬라이드 본문"], "2026-08-02T10:00:00Z");
        let path = write_tmp("dupey-pptx-meta.pptx", &bytes);
        let got = extract_pptx(&path).unwrap();
        assert!(!got.text.contains("노트"), "notes are not body text");
        assert_eq!(got.meta.modified.unwrap().to_string(), "2026-08-02T10:00:00Z");
        let _ = std::fs::remove_file(&path);
    }
}
