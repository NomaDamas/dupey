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
        std::fs::write(dir.join(fname), body).unwrap();
    }
    dir
}

fn scan_json(dir: &Path) -> serde_json::Value {
    let out = dupey()
        .args(["scan"])
        .arg(dir)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "scan failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
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
        &[
            ("보고서.txt", PROPOSAL),
            ("보고서 사본.txt", PROPOSAL),
        ],
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
    let out = dupey().args(["fingerprint"]).arg(dir.join("a.txt")).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("exact\t"), "{stdout}");
    assert!(stdout.contains("modified\t"), "fingerprint shows meta: {stdout}");

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
