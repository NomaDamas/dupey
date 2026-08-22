//! Criterion benchmarks for the dupey-core pipeline stages.

use std::io::Write;
use std::path::PathBuf;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use dupey_core::{cluster_with_config, extract, near_sig, score, ClusterConfig, ScannedDoc};

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
    const POOLS: &[&[&str]] = &[
        &[
            "예산", "집행", "결산", "감사", "회계", "송금", "세금", "청구",
        ],
        &[
            "일정",
            "마일스톤",
            "킥오프",
            "검수",
            "배포",
            "회귀",
            "데모",
            "리허설",
        ],
        &[
            "인사", "채용", "면접", "평가", "승진", "교육", "연차", "조직",
        ],
        &[
            "계약", "조항", "위약", "갱신", "해지", "서명", "날인", "검토",
        ],
        &[
            "서버", "배치", "로그", "지표", "알림", "장애", "복구", "용량",
        ],
    ];
    for n in [100, 500] {
        // Near-dup pairs (edits) on per-pool vocabulary, rest unrelated:
        // realistic team folder.
        let docs: Vec<ScannedDoc> = (0..n)
            .map(|i| {
                let pool = POOLS[i % POOLS.len()];
                let doc = (0..20)
                    .map(|k| {
                        let w1 = pool[(i + k) % pool.len()];
                        let w2 = pool[(i * 3 + k + 1) % pool.len()];
                        format!("{w1} 항목 {i}번 문서 {k}절: {w2} 기준과 절차를 정리한다.")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = if i % 2 == 0 {
                    doc
                } else {
                    doc.replace("정리한다", "정리했다")
                };
                ScannedDoc::from_text(PathBuf::from(format!("d{i}.docx")), &text)
            })
            .collect();
        g.bench_with_input(BenchmarkId::new("docs", n), &docs, |b, docs| {
            b.iter(|| cluster_with_config(docs, &ClusterConfig::default()))
        });
    }
    g.finish();
}

criterion_group!(benches, bench_extract, bench_near, bench_cluster);
criterion_main!(benches);
