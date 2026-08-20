//! Generate a real on-disk corpus with known families for e2e runs and
//! benchmarks: `cargo run -p dupey --example mkfixtures -- OUT_DIR [N]`
//!
//! N (default 1) scales the filler document count for benchmarks; the
//! named family fixtures are always written.

use std::io::Write;
use std::path::Path;

const PROPOSAL: &[&str] = &[
    "프로젝트 제안서",
    "1. 배경",
    "본 제안은 2026년 하반기 사무 자동화 도입을 위한 것이다. 현재 팀은 문서가 폴더에 흩어져 있고 최신본을 찾기 어렵다.",
    "2. 범위",
    "문서 수집, 중복 정리, 검색을 1단계 범위로 한다. 권한 관리는 포함하지 않는다.",
    "3. 일정",
    "킥오프는 9월 1일, 파일럿은 10월 말까지 진행한다.",
    "4. 예산",
    "예상 비용은 3,200만 원이다.",
];

const MINUTES: &[&str] = &[
    "주간 회의록",
    "일시: 2026년 8월 3일 10시. 참석: 기획팀 전원.",
    "안건 1. 하반기 일정 조율. 파일럿 시작일을 10월 20일로 확정했다.",
    "안건 2. 예산 집행 보고. 7월 집행률은 62%이다.",
    "안건 3. 문서 정리 규칙. 최종본 파일명 규칙을 다음 주까지 정한다.",
];

fn paragraphs(prefix: &str, n: usize) -> String {
    (1..=n)
        .map(|i| format!("{prefix} 문단 {i}: 이 문서의 {i}번째 고유 내용입니다."))
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_docx(path: &Path, paragraphs: &[&str], modified: &str, revision: u32) {
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
         <dcterms:modified xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:type=\"dcterms:W3CDTF\">{modified}</dcterms:modified>\
         <cp:revision>{revision}</cp:revision></cp:coreProperties>"
    );
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("word/document.xml", opts).unwrap();
    zip.write_all(document.as_bytes()).unwrap();
    zip.start_file("docProps/core.xml", opts).unwrap();
    zip.write_all(core.as_bytes()).unwrap();
    zip.start_file("docProps/app.xml", opts).unwrap();
    zip.write_all(b"<Properties><Application>mkfixtures</Application></Properties>")
        .unwrap();
    zip.finish().unwrap();
}

fn write_hwpx(path: &Path, paragraphs: &[&str], date: &str) {
    let runs: String = paragraphs
        .iter()
        .map(|p| format!("<hp:p><hp:run><hp:t>{p}</hp:t></hp:run></hp:p>"))
        .collect();
    let section = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <hs:sec xmlns:hs=\"http://www.hancom.co.kr/hwpml/2011/section\" \
         xmlns:hp=\"http://www.hancom.co.kr/hwpml/2011/paragraph\">{runs}</hs:sec>"
    );
    let hpf = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <opf:package xmlns:opf=\"http://www.idpf.org/2007/opf\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
         <opf:metadata><dc:title>t</dc:title><dc:date>{date}</dc:date></opf:metadata>\
         </opf:package>"
    );
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("mimetype", opts).unwrap();
    zip.write_all(b"application/hwp+zip").unwrap();
    zip.start_file("Contents/content.hpf", opts).unwrap();
    zip.write_all(hpf.as_bytes()).unwrap();
    zip.start_file("Contents/section0.xml", opts).unwrap();
    zip.write_all(section.as_bytes()).unwrap();
    zip.finish().unwrap();
}

fn write_hwp(path: &Path, paras: &[&str]) {
    let mut section = Vec::new();
    for p in paras {
        let mut utf16: Vec<u8> = p.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        while utf16.len() % 4 != 0 {
            utf16.extend_from_slice(&0u16.to_le_bytes());
        }
        let header: u32 = 67u32 | ((utf16.len() as u32 / 4) << 20);
        section.extend_from_slice(&header.to_le_bytes());
        section.extend_from_slice(&utf16);
    }
    let mut header = vec![0u8; 256];
    header[0..32].copy_from_slice(b"HWP Document File\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
    header[32..36].copy_from_slice(&0x00050100u32.to_le_bytes());
    let file = std::fs::File::create(path).unwrap();
    let mut ole = cfb::CompoundFile::create(file).unwrap();
    {
        let mut s = ole.create_stream("FileHeader").unwrap();
        s.write_all(&header).unwrap();
    }
    ole.create_storage("BodyText").unwrap();
    {
        let mut s = ole.create_stream("BodyText/Section0").unwrap();
        s.write_all(&section).unwrap();
    }
}

fn write_pptx(path: &Path, slides: &[&str], modified: &str) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (i, text) in slides.iter().enumerate() {
        let slide = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" \
             xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">\
             <p:cSld><p:spTree><p:sp><p:txBody>\
             <a:p><a:r><a:t>{text}</a:t></a:r></a:p>\
             </p:txBody></p:sp></p:spTree></p:cSld></p:sld>"
        );
        zip.start_file(format!("ppt/slides/slide{}.xml", i + 1), opts).unwrap();
        zip.write_all(slide.as_bytes()).unwrap();
    }
    let core = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
         xmlns:dcterms=\"http://purl.org/dc/terms/\">\
         <dcterms:modified xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:type=\"dcterms:W3CDTF\">{modified}</dcterms:modified>\
         </cp:coreProperties>"
    );
    zip.start_file("docProps/core.xml", opts).unwrap();
    zip.write_all(core.as_bytes()).unwrap();
    zip.finish().unwrap();
}

fn write_xlsx(path: &Path, strings: &[&str], rows: &[&[usize]], modified: &str) {
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
                    format!("<c r=\"{}{}\" t=\"s\"><v>{si}</v></c>", (b'A' + c as u8) as char, r + 1)
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
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("xl/sharedStrings.xml", opts).unwrap();
    zip.write_all(sst.as_bytes()).unwrap();
    zip.start_file("xl/worksheets/sheet1.xml", opts).unwrap();
    zip.write_all(sheet.as_bytes()).unwrap();
    zip.start_file("docProps/core.xml", opts).unwrap();
    zip.write_all(core.as_bytes()).unwrap();
    zip.finish().unwrap();
}

fn write_pdf(path: &Path, lines: &[&str], mod_date: Option<&str>) {
    let mut content = String::from("BT /F1 12 Tf 12 TL 72 720 Td\n");
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            content.push_str("T* ");
        }
        content.push_str(&format!("({line}) Tj\n"));
    }
    content.push_str("ET\n");
    let info = match mod_date {
        Some(d) => format!("<< /ModDate ({d}) /Producer (mkfixtures) >>"),
        None => "<< /Producer (mkfixtures) >>".to_string(),
    };
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
            .to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
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
    std::fs::write(path, pdf).unwrap();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("usage: mkfixtures OUT_DIR [N]"));
    let scale: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Family 1: docx proposal, one-line edit, plus a resaved copy whose
    // body is identical (extract must make it exact-equal to the original).
    let edited: Vec<String> = PROPOSAL
        .iter()
        .map(|p| p.replace("3,200만 원", "3,500만 원"))
        .collect();
    let edited: Vec<&str> = edited.iter().map(|s| s.as_str()).collect();
    write_docx(&dir.join("제안서.docx"), PROPOSAL, "2026-08-01T09:00:00Z", 3);
    write_docx(&dir.join("제안서_재저장.docx"), PROPOSAL, "2026-08-04T09:00:00Z", 9);
    write_docx(&dir.join("제안서_최종.docx"), &edited, "2026-08-05T09:00:00Z", 12);

    // Family 2: hwpx identical copies.
    write_hwpx(&dir.join("보고서.hwpx"), MINUTES, "2026-08-02T09:00:00Z");
    write_hwpx(&dir.join("보고서 사본.hwpx"), MINUTES, "2026-08-03T09:00:00Z");

    // Family 3: pdf one-line edit (ASCII text for the Type1 font).
    let v1 = ["Weekly sync notes", "Pilot starts Oct 20", "Budget burn 62 percent", "File naming rule due next week", "Action items in the tracker"];
    let v2 = ["Weekly sync notes", "Pilot starts Oct 27", "Budget burn 62 percent", "File naming rule due next week", "Action items in the tracker"];
    write_pdf(&dir.join("minutes.pdf"), &v1, Some("D:20260803090000Z"));
    write_pdf(&dir.join("minutes_v2.pdf"), &v2, Some("D:20260806090000Z"));

    // Family 4: txt contains (final = draft + appendix).
    let draft = paragraphs("초안", 10);
    let appendix = [
        "부록 A: 분기별 매출 측정 결과와 원자재 단가 표를 수록한다.",
        "부록 B: 현장 설문 응답 원문과 면담 메모를 옮긴다.",
        "부록 C: 참고 문헌 목록과 인용 출처를 덧붙인다.",
        "부록 D: 운영 체크리스트와 검수 서명란을 첨부한다.",
        "부록 E: 시설 도면 축척과 배선 경로 요약을 싣는다.",
    ]
    .join("\n");
    std::fs::write(dir.join("계획_초안.txt"), &draft).unwrap();
    std::fs::write(dir.join("계획_최종.txt"), format!("{draft}\n{appendix}")).unwrap();

    // Same content, newline-only difference => exact equal.
    let crlf = draft.replace('\n', "\r\n");
    std::fs::write(dir.join("계획_초안_crlf.md"), &crlf).unwrap();

    // Family 5: hwp (binary) one-line edit. Enough body for the
    // 5-gram MinHash to stay above the near threshold.
    let hwp_v1 = [
        "하반기 운영 계획",
        "인력 충원은 개발 1명과 디자인 1명으로 진행한다.",
        "장비 도입 예산은 800만 원을 배정한다.",
        "일정은 분기별 검토 회의에서 확정한다.",
        "보안 점검은 외부 업체에 맡긴다.",
        "교육 예산은 팀당 50만 원이다.",
    ];
    let hwp_v2 = [
        "하반기 운영 계획",
        "인력 충원은 개발 2명과 디자인 1명으로 진행한다.",
        "장비 도입 예산은 800만 원을 배정한다.",
        "일정은 분기별 검토 회의에서 확정한다.",
        "보안 점검은 외부 업체에 맡긴다.",
        "교육 예산은 팀당 50만 원이다.",
    ];
    write_hwp(&dir.join("운영계획.hwp"), &hwp_v1);
    write_hwp(&dir.join("운영계획_최종.hwp"), &hwp_v2);

    // Family 6: pptx identical slides, different internal timestamps.
    let slides = ["분기 실적 발표", "매출 12억, 전분기 대비 8퍼센트 증가", "다음 분기 목표와 리스크"];
    write_pptx(&dir.join("발표자료.pptx"), &slides, "2026-08-01T09:00:00Z");
    write_pptx(&dir.join("발표자료_복사본.pptx"), &slides, "2026-08-02T09:00:00Z");

    // Family 7: xlsx one-cell edit.
    let strings = [
        "항목", "금액", "인건비", "1,200", "서버비", "340", "라이선스", "210",
        "광고비", "480", "비품", "95", "회의비", "60", "1,350",
        "택배비", "42", "도서구입", "38", "소모품", "27", "통신비", "88",
        "수수료", "15", "복리후생", "120", "교통비", "76",
    ];
    let rows_v1: Vec<&[usize]> = vec![
        &[0, 1], &[2, 3], &[4, 5], &[6, 7], &[8, 9], &[10, 11], &[12, 13],
        &[14, 15], &[16, 17], &[18, 19], &[20, 21], &[22, 23], &[24, 25], &[26, 27],
    ];
    let mut v2_flat: Vec<Vec<usize>> = rows_v1.iter().map(|r| r.to_vec()).collect();
    v2_flat[1][1] = 14;
    let rows_v2: Vec<&[usize]> = v2_flat.iter().map(|r| r.as_slice()).collect();
    write_xlsx(&dir.join("예산표.xlsx"), &strings, &rows_v1, "2026-08-01T09:00:00Z");
    write_xlsx(&dir.join("예산표_v2.xlsx"), &strings, &rows_v2, "2026-08-04T09:00:00Z");

    // Unrelated doc and a scanned-style PDF (no embedded text).
    std::fs::write(dir.join("메모.txt"), "오늘 점심은 김치찌개다. 산책을 하고 일찍 잔다.").unwrap();
    std::fs::write(dir.join("scan.pdf"), b"%PDF-1.4\n").unwrap(); // broken/empty on purpose

    // Filler docs for benchmarks: unique per-doc tokens (no shared
    // sentence templates), so random pairs share nothing and the scan
    // time measured is extract + cluster candidate generation, not
    // verifying thousands of template-twin pairs. Family behavior is
    // covered by the named fixtures above.
    fn synth_doc(seed: usize) -> String {
        (0..8)
            .map(|k| format!("근거{seed:06}에따라절차{:06}를정리하고담당확인을남긴다", seed.wrapping_mul(8 + k) % 100000))
            .collect::<Vec<_>>()
            .join("\n")
    }
    for i in 0..scale * 100 {
        let body = synth_doc(i);
        match i % 4 {
            0 => std::fs::write(dir.join(format!("filler_{i:04}.txt")), body).unwrap(),
            1 => std::fs::write(dir.join(format!("filler_{i:04}.md")), body).unwrap(),
            2 => {
                let paras: Vec<&str> = body.lines().collect();
                write_docx(&dir.join(format!("filler_{i:04}.docx")), &paras, "2026-08-01T00:00:00Z", 1)
            }
            _ => {
                let paras: Vec<&str> = body.lines().collect();
                write_hwpx(&dir.join(format!("filler_{i:04}.hwpx")), &paras, "2026-08-01T00:00:00Z")
            }
        }
    }
    eprintln!("fixtures written to {}", dir.display());
}
