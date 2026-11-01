use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

fn words(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn para(mut content: Vec<u8>, count: u16) -> Vec<u8> {
    let mut out = vec![0; 43 + 187 + 14];
    out[1..3].copy_from_slice(&(count + 1).to_le_bytes());
    out[3..5].copy_from_slice(&1u16.to_le_bytes());
    out.append(&mut content);
    out.extend(words(&[13]));
    out
}

fn list(content: Vec<u8>) -> Vec<u8> {
    [content, vec![0; 43]].concat()
}

fn text_para(text: &str) -> Vec<u8> {
    para(
        words(&text.encode_utf16().collect::<Vec<_>>()),
        text.len() as u16,
    )
}

fn document(paragraphs: Vec<u8>, compressed: bool) -> Vec<u8> {
    let mut out = b"HWP Document File V3.00 \x1a\x01\x02\x03\x04\x05".to_vec();
    assert_eq!(out.len(), 30);
    out.resize(30 + 128 + 1008, 0);
    out[30 + 124] = u8::from(compressed);
    let body = [vec![0; 16], list(paragraphs)].concat();
    if compressed {
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&body).unwrap();
        out.extend(encoder.finish().unwrap());
    } else {
        out.extend(body);
    }
    out
}

fn extract(bytes: &[u8]) -> Result<String, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "dupey-hwp3-{}-{}.hwp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, bytes).unwrap();
    let result = dupey_core::extract::extract(&path)
        .map(|c| c.text)
        .map_err(|e| e.to_string());
    std::fs::remove_file(path).unwrap();
    result
}

fn object(code: u16, info: Vec<u8>, payload: Vec<u8>) -> Vec<u8> {
    [words(&[code, 0, 0, code]), info, payload].concat()
}

#[test]
fn hwp3_plain_and_compressed_korean() {
    let content = para(
        words(&[0xb7a9, 0xb7a9, 32, 0xa5a1, 0x89a1, 0xace1, 32, 49]),
        8,
    );
    for compressed in [false, true] {
        assert_eq!(
            extract(&document(content.clone(), compressed)).unwrap(),
            "일일 보고서 1\n"
        );
    }
}

#[test]
fn hwp3_table_values_and_empty_cells_remain_distinct() {
    let table = |cells: &[&str]| {
        let mut info = vec![0; 84];
        info[80..82].copy_from_slice(&(cells.len() as u16).to_le_bytes());
        let mut payload = vec![0; cells.len() * 27];
        for cell in cells {
            payload.extend(list(text_para(cell)));
        }
        payload.extend(list(Vec::new())); // Caption.
        document(para(object(10, info, payload), 4), false)
    };
    let a = extract(&table(&["100", "", "200"])).unwrap();
    let b = extract(&table(&["100", "200", ""])).unwrap();
    let c = extract(&table(&["101", "", "200"])).unwrap();
    assert!(a.contains("100") && a.contains("200"));
    assert_ne!(a, b);
    assert_ne!(a, c);
}

#[test]
fn hwp3_date_controls_are_content() {
    let date = |day| {
        let mut values = vec![8];
        let mut format: Vec<u16> = "1-02-03".encode_utf16().collect();
        format.resize(40, 0);
        values.extend(format);
        values.extend([2026, 9, 2, day, 13, 5, 8]);
        document(para(words(&values), 4), false)
    };
    let a = extract(&date(15)).unwrap();
    let b = extract(&date(16)).unwrap();
    assert!(a.contains("2026-09-15"), "{a:?}");
    assert_ne!(a, b);
}

#[test]
fn hwp3_nested_header_and_picture_caption() {
    let header = object(16, vec![0; 10], list(text_para("Header")));
    let picture = object(11, vec![0; 348], list(text_para("Caption")));
    let bytes = document(para([header, picture, words(&[65])].concat(), 9), false);
    let text = extract(&bytes).unwrap();
    assert!(
        text.contains("Header") && text.contains("Caption") && text.contains('A'),
        "{text:?}"
    );
}

#[test]
fn hwp3_truncated_or_encrypted_documents_do_not_return_partial_text() {
    let plain = document(text_para("visible"), false);
    for length in [30, 100, 1165, plain.len() - 1, plain.len() - 43] {
        assert!(
            extract(&plain[..length]).is_err(),
            "accepted length {length}"
        );
    }
    let compressed = document(text_para("visible"), true);
    assert!(extract(&compressed[..compressed.len() - 2]).is_err());
    let mut encrypted = plain;
    encrypted[30 + 96] = 1;
    assert!(extract(&encrypted).unwrap_err().contains("password"));
}

#[test]
fn hwp3_cross_reference_target_and_displayed_value() {
    let target = object(29, vec![], vec![]);
    let mut target = target;
    target[2..6].copy_from_slice(&38u32.to_le_bytes());
    target.extend([0; 38]);
    let reference = |value: u16| {
        let mut data = vec![0; 46];
        data[0] = 1;
        data[40..42].copy_from_slice(&2u16.to_le_bytes());
        data.extend(words(&[value]));
        let mut control = words(&[29, 48, 0, 29]);
        control.extend(data);
        document(para([target.clone(), control].concat(), 8), false)
    };
    assert_eq!(extract(&reference(65)).unwrap(), "A\n");
    assert_eq!(extract(&reference(66)).unwrap(), "B\n");
}

#[test]
fn hwp3_outline_keeps_custom_bullets_and_decorations() {
    let outline = |bullet: u16, prefix: u16, suffix: u16| {
        let mut data = vec![0; 60];
        data[..2].copy_from_slice(&1u16.to_le_bytes());
        data[2] = 128; // User-defined outline/bullet.
        data[18..20].copy_from_slice(&bullet.to_le_bytes());
        data[32..34].copy_from_slice(&prefix.to_le_bytes());
        data[34..36].copy_from_slice(&suffix.to_le_bytes());
        document(para([words(&[28]), data, words(&[28])].concat(), 32), false)
    };
    let a = extract(&outline(0x343b, 65, 66)).unwrap();
    assert!(a.contains('A') && a.contains('B'));
    assert_ne!(a, extract(&outline(0x343c, 65, 66)).unwrap());
    assert_ne!(a, extract(&outline(0x343b, 67, 66)).unwrap());
}

#[test]
fn hwp3_inline_values_and_character_shapes() {
    let content = words(&[
        18, 4, 123, 18, 9, 100, 0, 9, 23, 65, 66, 0, 23, 30, 30, 24, 200, 24,
    ]);
    let count = 4 + 4 + 5 + 2 + 3;
    let normal = document(para(content.clone(), count), false);
    let mut paragraph = para(content, count);
    paragraph[5] = 1;
    let mut shapes = vec![0; 32];
    shapes.extend(vec![1; usize::from(count)]);
    paragraph.splice(244..244, shapes);
    assert_eq!(extract(&normal).unwrap(), "123\tAB -\n");
    assert_eq!(
        extract(&document(paragraph, false)).unwrap(),
        extract(&normal).unwrap()
    );
}

#[test]
fn hwp3_all_truncations_and_corrupt_terminators_fail() {
    let bytes = document(text_para("A"), false);
    for end in 0..bytes.len() {
        assert!(extract(&bytes[..end]).is_err(), "accepted {end}");
    }
    let mut corrupt = bytes;
    let end = corrupt.len() - 45;
    corrupt[end..end + 2].copy_from_slice(&65u16.to_le_bytes());
    assert!(extract(&corrupt).is_err());
}
