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

    // Unrelated doc and a scanned-style PDF (no embedded text).
    std::fs::write(dir.join("메모.txt"), "오늘 점심은 김치찌개다. 산책을 하고 일찍 잔다.").unwrap();
    std::fs::write(dir.join("scan.pdf"), b"%PDF-1.4\n").unwrap(); // broken/empty on purpose

    // Filler docs for benchmarks: unrelated content, no families.
    for i in 0..scale * 100 {
        let body = paragraphs(&format!("필러{i}"), 8);
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
