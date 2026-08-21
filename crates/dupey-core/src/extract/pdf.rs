//! pdf extraction: embedded text only, via pdf_oxide (pure Rust).
//!
//! Creation metadata (Producer, Creator, CreationDate) is dropped.
//! /ModDate is kept as internal provenance. Scanned PDFs yield empty
//! text: they cannot take this pipeline and callers must treat them as
//! having no comparable content.

use std::any::Any;
use std::path::Path;

use super::{normalize_newlines, CanonicalText, DocMeta, Format};
use crate::error::{Error, Result};
use pdf_oxide::PdfDocument;

pub(crate) fn extract_pdf(path: &Path) -> Result<CanonicalText> {
    let extraction = std::panic::catch_unwind(|| extract_document_text(path));
    let text = match extraction {
        Ok(Ok(text)) => text,
        Ok(Err(message)) => {
            return Err(Error::Extract {
                path: path.to_path_buf(),
                message,
            });
        }
        Err(payload) => {
            return Err(Error::Extract {
                path: path.to_path_buf(),
                message: format!("PDF extractor panicked: {}", panic_message(&*payload)),
            });
        }
    };
    Ok(CanonicalText {
        path: path.to_path_buf(),
        format: Format::Pdf,
        text: normalize_newlines(&text),
        meta: pdf_meta(path),
    })
}

fn extract_document_text(path: &Path) -> std::result::Result<String, String> {
    let document = PdfDocument::open(path).map_err(|error| error.to_string())?;
    if document.is_encrypted() && !document.is_authenticated() {
        return Err("encrypted PDF requires a valid password".to_string());
    }
    let page_count = document.page_count().map_err(|error| error.to_string())?;
    let mut text = String::new();
    for page in 0..page_count {
        if page > 0 {
            text.push('\x0c');
        }
        let page_text = document
            .extract_text(page)
            .map_err(|error| format!("page {}: {error}", page + 1))?;
        text.push_str(&page_text);
    }
    Ok(text)
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic")
}

/// /ModDate from the Info dictionary, parsed into a Timestamp.
/// Missing timezone is treated as UTC.
fn pdf_meta(path: &Path) -> DocMeta {
    let mut meta = DocMeta::default();
    let Ok(doc) = lopdf::Document::load(path) else {
        return meta;
    };
    let Ok(info_ref) = doc.trailer.get(b"Info").and_then(|o| o.as_reference()) else {
        return meta;
    };
    let Ok(lopdf::Object::Dictionary(info)) = doc.get_object(info_ref) else {
        return meta;
    };
    if let Ok(lopdf::Object::String(raw, _)) = info.get(b"ModDate") {
        meta.modified = parse_pdf_date(&String::from_utf8_lossy(raw));
    }
    meta
}

/// PDF date `D:YYYYMMDDHHmmSSOHH'mm'` -> ISO 8601 timestamp.
fn parse_pdf_date(raw: &str) -> Option<jiff::Timestamp> {
    let d = raw.strip_prefix("D:").unwrap_or(raw);
    let digits: String = d.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 14 {
        return None;
    }
    let tz = if d.contains('+') || d.contains('-') || d.ends_with('Z') {
        let sign = if d.contains('-') { '-' } else { '+' };
        if d.ends_with('Z') {
            "Z".to_string()
        } else {
            let tz_digits: String = d
                .rsplit(sign)
                .next()
                .unwrap_or("")
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect();
            if tz_digits.len() >= 4 {
                format!("{sign}{}:{}", &tz_digits[..2], &tz_digits[2..4])
            } else {
                "Z".to_string()
            }
        }
    } else {
        "Z".to_string()
    };
    let iso = format!(
        "{}-{}-{}T{}:{}:{}{}",
        &digits[0..4],
        &digits[4..6],
        &digits[6..8],
        &digits[8..10],
        &digits[10..12],
        &digits[12..14],
        tz
    );
    iso.parse::<jiff::Timestamp>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::docx::tests::write_tmp;
    use crate::extract::Format;

    /// Build a minimal, valid single-page PDF with the given text lines.
    fn make_pdf(lines: &[&str], mod_date: Option<&str>) -> Vec<u8> {
        let mut content = String::from("BT /F1 12 Tf 12 TL 72 720 Td\n");
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                content.push_str("T* ");
            }
            content.push_str(&format!("({line}) Tj\n"));
        }
        content.push_str("ET\n");

        let info = mod_date
            .map(|d| format!("<< /ModDate ({d}) /Producer (dupey-test) >>"))
            .unwrap_or_else(|| "<< /Producer (dupey-test) >>".to_string());
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
                .to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            ),
            info,
        ];

        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", i + 1, body));
        }
        let xref_at = pdf.len();
        let n = objects.len() + 1;
        pdf.push_str(&format!("xref\n0 {n}\n0000000000 65535 f \n"));
        for off in offsets {
            pdf.push_str(&format!("{off:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {n} /Root 1 0 R /Info 6 0 R >>\nstartxref\n{xref_at}\n%%EOF\n"
        ));
        pdf.into_bytes()
    }

    #[test]
    fn extracts_embedded_text() {
        let bytes = make_pdf(
            &[
                "Project proposal",
                "Budget is 3200 won",
                "Kickoff in September",
            ],
            Some("D:20260801090000Z"),
        );
        let path = write_tmp("dupey-pdf-text.pdf", &bytes);
        let got = extract_pdf(&path).unwrap();
        assert_eq!(got.format, Format::Pdf);
        for line in [
            "Project proposal",
            "Budget is 3200 won",
            "Kickoff in September",
        ] {
            assert!(
                got.text.contains(line),
                "missing {line:?} in {:?}",
                got.text
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reads_mod_date() {
        let bytes = make_pdf(&["a"], Some("D:20260801093000Z"));
        let path = write_tmp("dupey-pdf-meta.pdf", &bytes);
        let got = extract_pdf(&path).unwrap();
        assert_eq!(
            got.meta.modified.unwrap().to_string(),
            "2026-08-01T09:30:00Z"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ignores_producer_metadata() {
        let a = make_pdf(&["same body"], Some("D:20260801090000Z"));
        let b = make_pdf(&["same body"], None);
        let pa = write_tmp("dupey-pdf-a.pdf", &a);
        let pb = write_tmp("dupey-pdf-b.pdf", &b);
        let ta = extract_pdf(&pa).unwrap();
        let tb = extract_pdf(&pb).unwrap();
        assert_eq!(ta.text, tb.text);
        assert_eq!(crate::exact_hash(&ta.text), crate::exact_hash(&tb.text));
        let _ = std::fs::remove_file(&pa);
        let _ = std::fs::remove_file(&pb);
    }

    #[test]
    fn unsupported_cjk_encoding_never_panics() {
        let bytes = make_unsupported_cjk_pdf();
        let path = write_tmp("dupey-pdf-uniks.pdf", &bytes);
        let result = std::panic::catch_unwind(|| extract_pdf(&path));
        assert!(
            result.is_ok(),
            "PDF extraction must return Result, not panic"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_pdf_returns_extract_error() {
        let path = write_tmp("dupey-pdf-malformed.pdf", b"%PDF-1.4\nnot a document");
        let result = extract_pdf(&path);
        assert!(
            matches!(result, Err(Error::Extract { .. })),
            "malformed PDF must be reported as an extraction error: {result:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Minimal Type0/CID font PDF that makes pdf-extract 0.12 panic with
    /// `unsupported encoding UniKS-UCS2-H`.
    fn make_unsupported_cjk_pdf() -> Vec<u8> {
        let content = "BT /F1 12 Tf 72 720 Td <0041> Tj ET\n";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 4 0 R >> >> /Contents 6 0 R >>"
                .to_string(),
            "<< /Type /Font /Subtype /Type0 /BaseFont /TestKorean \
             /Encoding /UniKS-UCS2-H /DescendantFonts [5 0 R] >>"
                .to_string(),
            "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /TestKorean \
             /CIDSystemInfo << /Registry (Adobe) /Ordering (Korea1) /Supplement 1 >> \
             /DW 1000 >>"
                .to_string(),
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            ),
        ];

        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", i + 1, body));
        }
        let xref_at = pdf.len();
        let n = objects.len() + 1;
        pdf.push_str(&format!("xref\n0 {n}\n0000000000 65535 f \n"));
        for off in offsets {
            pdf.push_str(&format!("{off:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {n} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n"
        ));
        pdf.into_bytes()
    }
}
