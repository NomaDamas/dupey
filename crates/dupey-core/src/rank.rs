//! Explainable latest/canonical ranking. Not a verdict.
//!
//! Signals: internal modified time (beats filesystem mtime), content
//! containment, filename tokens (`v3`, `최종`, `복사본`, ...), OOXML
//! revision, weak length. Scores, reasons, and a margin-based confidence
//! are published; dupey never claims a single source of truth.

use std::path::PathBuf;

use jiff::Timestamp;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RankSignal {
    pub name: String,
    pub detail: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RankedMember {
    pub path: PathBuf,
    pub rank: u32,
    pub score: f64,
    pub reasons: Vec<RankSignal>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FamilyRanking {
    pub family_id: u32,
    /// Ranked latest candidates, best first.
    pub ranked: Vec<RankedMember>,
    /// Margin-based confidence of the #1 pick, 0.5 = coin flip.
    pub confidence: f64,
}

/// Per-member evidence collected during scan/extract.
#[derive(Debug, Clone, Default)]
pub struct MemberSignals {
    pub path: PathBuf,
    /// In-file modified time (docx core.xml, hwpx content.hpf, PDF /ModDate).
    pub internal_modified: Option<Timestamp>,
    pub fs_mtime: Option<Timestamp>,
    pub revision: Option<u32>,
    pub text_len: usize,
    /// This member's text contains (>= threshold) every other member's.
    pub contains_others: bool,
    /// This member is contained in some other member.
    pub contained_by_other: bool,
}

const W_INTERNAL_TIME: f64 = 3.0;
const W_FS_TIME: f64 = 1.0;
const W_CONTAINS: f64 = 2.0;
const W_FILENAME: f64 = 1.0;
const W_REVISION: f64 = 0.5;
const W_LENGTH: f64 = 0.25;

const POSITIVE_TOKENS: &[&str] = &["최종", "최최종", "찐", "final", "완료", "정본"];
const NEGATIVE_TOKENS: &[&str] = &[
    "복사본", "사본", "copy", "old", "draft", "초안", "백업", "backup", "원본",
];

/// Rank a family's members as latest-candidate picks with public reasons.
pub fn rank(family_id: u32, signals: &[MemberSignals]) -> FamilyRanking {
    let max_score = W_INTERNAL_TIME + W_CONTAINS + 2.0 * W_FILENAME + W_REVISION + W_LENGTH;

    let internal_times: Vec<Timestamp> =
        signals.iter().filter_map(|s| s.internal_modified).collect();
    let revisions: Vec<u32> = signals.iter().filter_map(|s| s.revision).collect();
    let versions: Vec<Option<u32>> = signals.iter().map(|s| version_of(&s.path)).collect();
    let max_version = versions.iter().flatten().max().copied();

    let mut scored: Vec<RankedMember> = signals
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut score = 0.0;
            let mut reasons = Vec::new();

            if let Some(t) = m.internal_modified {
                if internal_times.len() > 1 && internal_times.iter().max() == Some(&t)
                    && internal_times.iter().min() < Some(&t)
                {
                    score += W_INTERNAL_TIME;
                    reasons.push(RankSignal {
                        name: "internal_modified".into(),
                        detail: format!("파일 내부 수정시각이 가장 늦음 ({t})"),
                    });
                }
            } else if let Some(t) = m.fs_mtime {
                // Filesystem mtime only counts when the file itself carries
                // no time; downloads and unzips clobber it.
                let fs_times: Vec<Timestamp> = signals
                    .iter()
                    .filter(|s| s.internal_modified.is_none())
                    .filter_map(|s| s.fs_mtime)
                    .collect();
                if fs_times.len() > 1 && fs_times.iter().max() == Some(&t)
                    && fs_times.iter().min() < Some(&t)
                {
                    score += W_FS_TIME;
                    reasons.push(RankSignal {
                        name: "fs_mtime".into(),
                        detail: format!("파일시스템 수정시각이 가장 늦음 ({t})"),
                    });
                }
            }

            if m.contains_others {
                score += W_CONTAINS;
                reasons.push(RankSignal {
                    name: "contains".into(),
                    detail: "본문이 다른 후보를 포함함".into(),
                });
            }

            let (fscore, toks) = filename_score(&m.path, versions[i], max_version);
            if fscore != 0.0 {
                score += fscore;
                reasons.push(RankSignal {
                    name: "filename".into(),
                    detail: toks.join(", "),
                });
            }

            if let Some(r) = m.revision {
                if revisions.len() > 1 && revisions.iter().max() == Some(&r)
                    && revisions.iter().min() < Some(&r)
                {
                    score += W_REVISION;
                    reasons.push(RankSignal {
                        name: "revision".into(),
                        detail: format!("저장 횟수가 가장 많음 ({r})"),
                    });
                }
            }

            let lens: Vec<usize> = signals.iter().map(|s| s.text_len).collect();
            if m.text_len > 0
                && lens.iter().max() == Some(&m.text_len)
                && lens.iter().min() < Some(&m.text_len)
            {
                score += W_LENGTH;
                reasons.push(RankSignal {
                    name: "length".into(),
                    detail: format!("본문이 가장 김 ({}자, 약한 신호)", m.text_len),
                });
            }

            RankedMember {
                path: m.path.clone(),
                rank: 0,
                score,
                reasons,
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
    });
    for (idx, m) in scored.iter_mut().enumerate() {
        m.rank = idx as u32 + 1;
    }

    let margin = match (scored.first(), scored.get(1)) {
        (Some(top), Some(second)) => top.score - second.score,
        (Some(_), None) => max_score,
        _ => 0.0,
    };
    let confidence = (0.5 + 0.5 * margin / max_score).clamp(0.05, 0.95);

    FamilyRanking {
        family_id,
        ranked: scored,
        confidence,
    }
}

/// Filename token evidence: positive latest-ish tokens, negative
/// copy/draft-ish tokens, and `vN` version numbers compared across the
/// family. Contribution is capped at +/- 2 * W_FILENAME.
fn filename_score(
    path: &std::path::Path,
    version: Option<u32>,
    max_version: Option<u32>,
) -> (f64, Vec<String>) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut score = 0.0;
    let mut toks = Vec::new();
    for t in POSITIVE_TOKENS {
        if name.contains(t) {
            score += W_FILENAME;
            toks.push(format!("'{t}'(+)") );
        }
    }
    for t in NEGATIVE_TOKENS {
        if name.contains(t) {
            score -= W_FILENAME;
            toks.push(format!("'{t}'(-)"));
        }
    }
    if let (Some(v), Some(max)) = (version, max_version) {
        if v == max {
            score += W_FILENAME;
            toks.push(format!("버전 v{v}(+)"));
        } else {
            toks.push(format!("버전 v{v}(0, 최고는 v{max})"));
        }
    }
    (score.clamp(-2.0 * W_FILENAME, 2.0 * W_FILENAME), toks)
}

/// Extract `v3` / `V12` style version numbers from a filename.
fn version_of(path: &std::path::Path) -> Option<u32> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let stem = name.rsplit('.').skip(1).next().unwrap_or(&name);
    let bytes = stem.as_bytes();
    let mut best = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'v' {
            let digits: String = bytes[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .map(|c| *c as char)
                .collect();
            if !digits.is_empty() {
                best = digits.parse::<u32>().ok().or(best);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn member(name: &str) -> MemberSignals {
        MemberSignals {
            path: PathBuf::from(name),
            ..Default::default()
        }
    }

    fn top(r: &FamilyRanking) -> &RankedMember {
        &r.ranked[0]
    }

    #[test]
    fn internal_time_wins_over_fs_mtime() {
        // a: older internal time, newer fs mtime. b: newer internal time.
        let mut a = member("a.docx");
        a.internal_modified = Some(ts("2026-08-01T09:00:00Z"));
        a.fs_mtime = Some(ts("2026-08-10T09:00:00Z"));
        let mut b = member("b.docx");
        b.internal_modified = Some(ts("2026-08-05T09:00:00Z"));
        b.fs_mtime = Some(ts("2026-08-02T09:00:00Z"));
        let r = rank(0, &[a, b]);
        assert_eq!(top(&r).path, PathBuf::from("b.docx"));
        assert!(top(&r)
            .reasons
            .iter()
            .any(|s| s.name == "internal_modified"));
    }

    #[test]
    fn filename_tokens_break_ties() {
        let a = member("제안서_최종.docx");
        let b = member("제안서_복사본.docx");
        let r = rank(0, &[b, a]);
        assert_eq!(top(&r).path, PathBuf::from("제안서_최종.docx"));
        assert!(top(&r).reasons.iter().any(|s| s.name == "filename"));
    }

    #[test]
    fn version_token_scores_positive() {
        let a = member("보고서_v3.docx");
        let b = member("보고서_v1.docx");
        let r = rank(0, &[b, a]);
        assert_eq!(top(&r).path, PathBuf::from("보고서_v3.docx"));
    }

    #[test]
    fn container_beats_contained() {
        let mut draft = member("보고서_초안.docx");
        draft.contained_by_other = true;
        let mut final_doc = member("보고서_최종.docx");
        final_doc.contains_others = true;
        let r = rank(0, &[draft, final_doc]);
        assert_eq!(top(&r).path, PathBuf::from("보고서_최종.docx"));
        assert!(top(&r).reasons.iter().any(|s| s.name == "contains"));
    }

    #[test]
    fn coin_flip_has_low_confidence() {
        let a = member("a.docx");
        let b = member("b.docx");
        let r = rank(0, &[a, b]);
        assert!(r.confidence <= 0.5, "tie must not be confident");
    }

    #[test]
    fn clear_winner_is_confident() {
        let mut a = member("제안서_최종_v3.docx");
        a.internal_modified = Some(ts("2026-08-05T09:00:00Z"));
        a.contains_others = true;
        a.revision = Some(9);
        let mut b = member("제안서_복사본.docx");
        b.internal_modified = Some(ts("2026-08-01T09:00:00Z"));
        b.contained_by_other = true;
        b.revision = Some(2);
        let r = rank(0, &[b, a]);
        assert_eq!(top(&r).path, PathBuf::from("제안서_최종_v3.docx"));
        assert!(r.confidence > 0.7, "got {}", r.confidence);
        // reasons are explainable: at least time + contains + filename
        let names: Vec<&str> = top(&r).reasons.iter().map(|s| s.name.as_str()).collect();
        for expected in ["internal_modified", "contains", "filename"] {
            assert!(names.contains(&expected), "missing reason {expected}");
        }
    }

    #[test]
    fn ranks_are_sequential_and_cover_all_members() {
        let mut a = member("a.docx");
        a.internal_modified = Some(ts("2026-08-01T09:00:00Z"));
        let mut b = member("b.docx");
        b.internal_modified = Some(ts("2026-08-03T09:00:00Z"));
        let c = member("c.docx");
        let r = rank(0, &[a, b, c]);
        let ranks: Vec<u32> = r.ranked.iter().map(|m| m.rank).collect();
        assert_eq!(ranks, vec![1, 2, 3]);
    }
}
