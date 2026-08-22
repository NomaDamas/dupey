//! Integration tests for the `dupey` binary against the public
//! `scan DIR --json` contract.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

fn dupey() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dupey"))
}

fn fixture_dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dupey-cli-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (fname, body) in files {
        let path = dir.join(fname);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }
    dir
}

fn scan_json(dir: &Path) -> serde_json::Value {
    scan_json_with(dir, &[])
}

fn scan_json_with(dir: &Path, extra: &[&str]) -> serde_json::Value {
    let out = dupey()
        .arg("scan")
        .arg(dir)
        .arg("--json")
        .args(extra)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "scan failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn set_mtime(path: &Path, unix_secs: u64) {
    let t = UNIX_EPOCH + Duration::from_secs(unix_secs);
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

fn scanned_paths(v: &serde_json::Value) -> Vec<String> {
    v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().replace('\\', "/"))
        .collect()
}

fn unsupported_cjk_pdf() -> Vec<u8> {
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

fn empty_page_pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
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

const PROPOSAL: &str =
    "프로젝트 제안서\n\n1. 배경\n본 제안은 2026년 하반기 사무 자동화 도입을 위한 것이다. \
     현재 팀은 문서가 폴더에 흩어져 있고 최신본을 찾기 어렵다.\n\n2. 범위\n문서 수집, \
     중복 정리, 검색, 권한은 1단계 범위에 포함하지 않는다.\n\n3. 일정\n킥오프는 9월 1일, \
     파일럿은 10월 말까지 진행한다.\n\n4. 예산\n예상 비용은 3,200만 원이다.\n";

#[test]
fn scan_json_contract_shape() {
    let edited = PROPOSAL.replace("3,200만 원", "3,500만 원");
    let dir = fixture_dir(
        "contract",
        &[
            ("제안서.docx.txt", PROPOSAL),
            ("제안서_최종.md", &edited),
            ("메모.txt", "내일 회의실 예약하기"),
        ],
    );
    let v = scan_json(&dir);

    // Public contract keys.
    assert!(v["files"].is_array());
    assert!(v["families"].is_array());
    let file = &v["files"][0];
    for key in ["path", "format", "content_hash", "fuzzy", "signals"] {
        assert!(file.get(key).is_some(), "file missing {key}");
    }
    let family = &v["families"][0];
    for key in ["id", "files", "relation", "pick"] {
        assert!(family.get(key).is_some(), "family missing {key}");
    }
    let pick = &family["pick"];
    assert!(pick["ranked"].is_array());
    assert!(pick["confidence"].is_number());
    assert!(pick["ranked"][0]["reasons"].is_array());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scan_rejects_nonexistent_directory() {
    let dir = std::env::temp_dir().join("dupey-cli-does-not-exist");
    let _ = std::fs::remove_dir_all(&dir);

    let out = dupey().arg("scan").arg(&dir).output().unwrap();

    assert!(!out.status.success(), "missing scan path must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("scan path does not exist"),
        "unexpected error: {stderr}"
    );
}

#[test]
fn scan_finds_near_family_and_picks_newer_mtime() {
    let edited = PROPOSAL.replace("3,200만 원", "3,500만 원");
    let dir = fixture_dir(
        "near",
        &[
            ("제안서.txt", PROPOSAL),
            ("제안서_최종.txt", &edited),
            ("무관.txt", "오늘 점심은 김치찌개다. 산책을 하고 일찍 잔다."),
        ],
    );
    // Filename says 최종 is later; filesystem time says the opposite.
    set_mtime(&dir.join("제안서_최종.txt"), 1_700_000_000);
    set_mtime(&dir.join("제안서.txt"), 1_800_000_000);
    let v = scan_json(&dir);
    let families = v["families"].as_array().unwrap();
    assert_eq!(families.len(), 1);
    let fam = &families[0];
    assert_eq!(fam["files"].as_array().unwrap().len(), 2);
    assert_eq!(fam["relation"], "near");
    let pick = fam["pick"]["ranked"][0]["path"].as_str().unwrap();
    assert!(
        pick.ends_with("제안서.txt"),
        "pick must follow mtime, not 최종 token: {pick}"
    );
    let reasons = fam["pick"]["ranked"][0]["reasons"].as_array().unwrap();
    assert!(
        reasons.iter().all(|r| r["name"] != "filename"),
        "filename must not rank: {reasons:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scan_exact_duplicate_group() {
    let dir = fixture_dir(
        "exact",
        &[("보고서.txt", PROPOSAL), ("보고서 사본.txt", PROPOSAL)],
    );
    let v = scan_json(&dir);
    let families = v["families"].as_array().unwrap();
    assert_eq!(families.len(), 1);
    assert_eq!(families[0]["relation"], "exact");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Deterministic pseudo-text with no shared template vocabulary, mirroring
/// the core fixture: different seeds share almost no character 5-grams.
fn noise(seed: u64, lines: usize) -> String {
    const SYLLABLES: &[char] = &[
        '가', '나', '다', '라', '마', '바', '사', '아', '자', '차', '카', '타', '파', '하', '거',
        '너', '더', '러', '머', '버', '서', '어', '저', '처', '커', '터', '퍼', '허',
    ];
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(7);
    let mut out = String::new();
    for _ in 0..lines {
        for _ in 0..40 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            out.push(SYLLABLES[(state >> 33) as usize % SYLLABLES.len()]);
        }
        out.push('\n');
    }
    out
}

/// Two documents sharing one boilerplate block, differing only in a short
/// unique body: the corporate-template shape from issue #3.
fn template_siblings() -> Vec<(String, String)> {
    let boilerplate = noise(1, 93);
    (0..2)
        .map(|i| {
            (
                format!("양식_{i}.txt"),
                format!("{boilerplate}{}", noise(100 + i as u64, 7)),
            )
        })
        .collect()
}

#[test]
fn scan_keeps_template_siblings_out_of_one_family() {
    let siblings = template_siblings();
    let files: Vec<(&str, &str)> = siblings
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_str()))
        .collect();
    let dir = fixture_dir("template-siblings", &files);

    let v = scan_json(&dir);
    assert!(
        v["families"].as_array().unwrap().is_empty(),
        "shared-template siblings must not merge at the default contains gate: {v}"
    );

    // The same pair merges once contains is told to use the near threshold,
    // which proves the gate is what kept them apart.
    let relaxed = scan_json_with(&dir, &["--contains-threshold", "0.90"]);
    assert_eq!(
        relaxed["families"].as_array().unwrap().len(),
        1,
        "lowering only the contains threshold must merge them: {relaxed}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scan_exposes_thresholds_and_join_evidence() {
    let draft = noise(7, 40);
    let final_doc = format!("{draft}{}", noise(8, 40));
    let dir = fixture_dir(
        "evidence",
        &[("계획_초안.txt", &draft), ("계획_최종.txt", &final_doc)],
    );
    let v = scan_json(&dir);

    assert_eq!(v["threshold"], 0.90, "near threshold stays reported: {v}");
    assert_eq!(v["contains_threshold"], 0.96, "{v}");

    let family = &v["families"][0];
    let edges = family["edges"].as_array().expect("family exposes edges");
    assert_eq!(edges.len(), 1, "{edges:?}");
    let edge = &edges[0];
    assert_eq!(edge["relation"], "contains");
    assert!(
        edge["a"].as_str().unwrap().ends_with("계획_최종.txt"),
        "a is the container: {edge}"
    );
    assert!(
        edge["b"].as_str().unwrap().ends_with("계획_초안.txt"),
        "b is the contained: {edge}"
    );
    assert!(edge["containment"].as_f64().unwrap() >= 0.96, "{edge}");
    assert!(edge["jaccard"].as_f64().unwrap() < 0.90, "{edge}");

    // Every member names the counterpart it actually matched.
    for member in family["members"].as_array().unwrap() {
        assert_eq!(member["relation"], "contains", "{member}");
        let joined = member["joined_with"].as_str().expect("joined_with is set");
        assert_ne!(joined, member["path"].as_str().unwrap());
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scan_labels_split_relations_as_mixed() {
    let draft = noise(7, 40);
    let final_doc = format!("{draft}{}", noise(8, 40));
    let edited = draft.replacen('가', "나", 3);
    let dir = fixture_dir(
        "mixed",
        &[
            ("계획_초안.txt", &draft),
            ("계획_최종.txt", &final_doc),
            ("계획_초안_수정.txt", &edited),
        ],
    );
    let v = scan_json(&dir);
    let family = &v["families"][0];
    assert_eq!(
        family["relation"], "mixed",
        "a family joined by two relations must not be folded into one: {family}"
    );
    let relations: Vec<&str> = family["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["relation"].as_str().unwrap())
        .collect();
    assert!(relations.contains(&"near"), "{relations:?}");
    assert!(relations.contains(&"contains"), "{relations:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scan_groups_byte_identical_empty_pdfs() {
    let dir = fixture_dir("empty-pdf-exact", &[("scan-a.pdf", "")]);
    std::fs::write(dir.join("scan-a.pdf"), empty_page_pdf()).unwrap();
    std::fs::copy(dir.join("scan-a.pdf"), dir.join("scan-b.pdf")).unwrap();

    let v = scan_json(&dir);
    let families = v["families"].as_array().unwrap();
    assert_eq!(families.len(), 1, "{v}");
    assert_eq!(families[0]["relation"], "exact");
    assert_eq!(families[0]["files"].as_array().unwrap().len(), 2);
    assert!(v["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|file| file["fuzzy"].is_null()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fingerprint_and_compare_still_work() {
    let dir = fixture_dir("basic", &[("a.txt", PROPOSAL)]);
    let out = dupey()
        .args(["fingerprint"])
        .arg(dir.join("a.txt"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("exact\t"), "{stdout}");
    assert!(
        stdout.contains("modified\t"),
        "fingerprint shows meta: {stdout}"
    );

    let out = dupey()
        .args(["compare"])
        .arg(dir.join("a.txt"))
        .arg(dir.join("a.txt"))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("exact_equal\ttrue"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scan_skips_default_vendor_dirs() {
    let dir = fixture_dir(
        "skip-vendor",
        &[
            ("제안서.txt", PROPOSAL),
            ("docs/메모.txt", "내일 회의실 예약하기\n"),
            ("node_modules/pkg/LICENSE.md", "MIT License\n"),
            (".git/README.md", "git readme\n"),
            ("build/junk.txt", "build artifact\n"),
            ("my_build_notes/keep.txt", "not a vendor dir\n"),
        ],
    );
    let paths = scanned_paths(&scan_json(&dir));
    assert!(paths.iter().any(|p| p.ends_with("제안서.txt")), "{paths:?}");
    assert!(paths.iter().any(|p| p.ends_with("메모.txt")), "{paths:?}");
    assert!(
        paths.iter().any(|p| p.ends_with("keep.txt")),
        "substring build must not skip my_build_notes: {paths:?}"
    );
    assert!(
        paths.iter().all(|p| !p.contains("/node_modules/")),
        "{paths:?}"
    );
    assert!(paths.iter().all(|p| !p.contains("/.git/")), "{paths:?}");
    assert!(paths.iter().all(|p| !p.contains("/build/")), "{paths:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scan_exclude_dir_adds_names() {
    let dir = fixture_dir(
        "skip-extra",
        &[
            ("keep.txt", PROPOSAL),
            ("임시/버림.txt", PROPOSAL),
            ("백업/old.txt", PROPOSAL),
        ],
    );
    let paths = scanned_paths(&scan_json_with(
        &dir,
        &["--exclude-dir", "임시", "--exclude-dir", "./백업/"],
    ));
    assert_eq!(paths.len(), 1, "{paths:?}");
    assert!(paths[0].ends_with("keep.txt"), "{paths:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scan_root_named_like_vendor_is_still_read() {
    let parent = std::env::temp_dir().join("dupey-cli-root-skip");
    let _ = std::fs::remove_dir_all(&parent);
    let dir = parent.join("node_modules");
    std::fs::create_dir_all(dir.join("nested/node_modules")).unwrap();
    std::fs::write(dir.join("inside.txt"), PROPOSAL).unwrap();
    std::fs::write(dir.join("nested/node_modules/LICENSE.md"), "MIT\n").unwrap();
    let paths = scanned_paths(&scan_json(&dir));
    assert!(
        paths.iter().any(|p| p.ends_with("inside.txt")),
        "walk root named node_modules must still be scanned: {paths:?}"
    );
    assert!(
        paths.iter().all(|p| !p.contains("/nested/node_modules/")),
        "{paths:?}"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn scan_continues_after_problem_pdf() {
    let dir = fixture_dir("pdf-error", &[("kept.txt", PROPOSAL)]);
    std::fs::write(dir.join("unsupported-korean.pdf"), unsupported_cjk_pdf()).unwrap();
    std::fs::write(dir.join("broken.pdf"), b"%PDF-1.4\nnot a document").unwrap();

    let v = scan_json(&dir);
    assert!(
        v["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"].as_str().unwrap().ends_with("kept.txt")),
        "successful files must still be emitted: {v}"
    );
    assert_eq!(v["files"].as_array().unwrap().len(), 2, "{v}");
    assert_eq!(v["errors"].as_array().unwrap().len(), 1, "{v}");
    assert!(
        v["errors"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("broken.pdf"),
        "the malformed PDF must be isolated in errors[]: {v}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
