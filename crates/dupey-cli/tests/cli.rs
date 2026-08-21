//! Integration tests for the `dupey` binary against the public
//! `scan DIR --json` contract.

use std::path::{Path, PathBuf};
use std::process::Command;

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

const PROPOSAL: &str = "프로젝트 제안서\n\n1. 배경\n본 제안은 2026년 하반기 사무 자동화 도입을 위한 것이다. \
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
fn scan_finds_near_family_and_picks_final() {
    let edited = PROPOSAL.replace("3,200만 원", "3,500만 원");
    let dir = fixture_dir(
        "near",
        &[
            ("제안서.txt", PROPOSAL),
            ("제안서_최종.txt", &edited),
            ("무관.txt", "오늘 점심은 김치찌개다. 산책을 하고 일찍 잔다."),
        ],
    );
    let v = scan_json(&dir);
    let families = v["families"].as_array().unwrap();
    assert_eq!(families.len(), 1);
    let fam = &families[0];
    assert_eq!(fam["files"].as_array().unwrap().len(), 2);
    assert_eq!(fam["relation"], "near");
    assert_eq!(
        fam["pick"]["ranked"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("제안서_최종.txt"),
        true
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
