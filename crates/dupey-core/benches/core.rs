//! Criterion benchmarks for the dupey-core pipeline stages.

use std::io::Write;
use std::path::PathBuf;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use dupey_core::{cluster, extract, near_sig, score, ScannedDoc};

fn proposal() -> String {
    "프로젝트 제안서\n\n1. 배경\n본 제안은 2026년 하반기 사무 자동화 도입을 위한 것이다. \
     현재 팀은 문서가 폴더에 흩어져 있고 최신본을 찾기 어렵다.\n\n2. 범위\n문서 수집, \
     중복 정리, 검색, 권한은 1단계 범위에 포함하지 않는다.\n\n3. 일정\n킥오프는 9월 1일, \
     파일럿은 10월 말까지 진행한다.\n\n4. 예산\n예상 비용은 3,200만 원이다.\n"
        .to_string()
}

fn paragraphs(prefix: &str, n: usize) -> String {
    (1..=n)
        .map(|i| format!("{prefix} 문단 {i}: 이 문서의 {i}번째 고유 내용입니다."))
        .collect::<Vec<_>>()
        .join("\n")
}

fn make_docx(paragraphs: &[&str]) -> Vec<u8> {
    let runs: String = paragraphs
        .iter()
        .map(|p| format!("<w:p><w:r><w:t xml:space=\"preserve\">{p}</w:t></w:r></w:p>"))
        .collect();
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
         <w:body>{runs}</w:body></w:document>"
    );
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(document.as_bytes()).unwrap();
        zip.start_file("docProps/core.xml", opts).unwrap();
        zip.write_all(b"<cp:coreProperties/>").unwrap();
        zip.finish().unwrap();
    }
    buf.into_inner()
}

fn tmp(name: &str, bytes: &[u8]) -> PathBuf {
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn bench_extract(c: &mut Criterion) {
    let mut g = c.benchmark_group("extract");
    let txt = tmp("dupey-bench.txt", paragraphs("벤치", 40).as_bytes());
    let body = paragraphs("벤치", 40);
    let docx_paras: Vec<&str> = body.lines().collect();
    let docx = tmp("dupey-bench.docx", &make_docx(&docx_paras));
    g.bench_function("txt", |b| b.iter(|| extract(&txt).unwrap()));
    g.bench_function("docx", |b| b.iter(|| extract(&docx).unwrap()));
    g.finish();
}

fn bench_near(c: &mut Criterion) {
    let mut g = c.benchmark_group("near");
    for n in [10, 100, 1000] {
        let text = paragraphs("벤치", n);
        g.bench_with_input(BenchmarkId::new("near_sig", n), &text, |b, t| {
            b.iter(|| near_sig(t))
        });
    }
    let a = near_sig(&paragraphs("벤치", 500));
    let bsig = near_sig(&paragraphs("다른", 500));
    g.bench_function("score_128perm", |b| b.iter(|| score(&a, &bsig)));
    g.finish();
}

fn bench_cluster(c: &mut Criterion) {
    let mut g = c.benchmark_group("cluster");
    g.sample_size(20);
    for n in [100, 500] {
        // Half near-dup pairs (edits), half unrelated: realistic team folder.
        let docs: Vec<ScannedDoc> = (0..n)
            .map(|i| {
                let base = paragraphs(&format!("문서{}", i / 2), 20);
                let text = if i % 2 == 0 {
                    base
                } else {
                    base.replace("1번째", "첫번째")
                };
                ScannedDoc::from_text(PathBuf::from(format!("d{i}.docx")), &text)
            })
            .collect();
        g.bench_with_input(BenchmarkId::new("docs", n), &docs, |b, docs| {
            b.iter(|| cluster(docs, 0.90))
        });
    }
    g.finish();
}

criterion_group!(benches, bench_extract, bench_near, bench_cluster);
criterion_main!(benches);
