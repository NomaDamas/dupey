//! Latest-candidate ranking by modified time only. Not a verdict.
//!
//! Per file, internal modified time (docx core.xml, hwpx content.hpf,
//! PDF /ModDate) is preferred over that file's filesystem mtime, which
//! downloads and unzips clobber. Filename tokens, containment, revision,
//! and length do not score; those stay on the scan payload for the user.

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
    /// Unique latest time → high; tied times → 0.5 coin flip.
    pub confidence: f64,
}

/// Per-member evidence collected during scan/extract.
#[derive(Debug, Clone, Default)]
pub struct MemberSignals {
    pub path: PathBuf,
    /// In-file modified time (docx core.xml, hwpx content.hpf, PDF /ModDate).
    pub internal_modified: Option<Timestamp>,
    pub fs_mtime: Option<Timestamp>,
}

fn effective_time(m: &MemberSignals) -> Option<Timestamp> {
    m.internal_modified.or(m.fs_mtime)
}

/// Rank a family's members by effective modified time, latest first.
pub fn rank(family_id: u32, signals: &[MemberSignals]) -> FamilyRanking {
    let times: Vec<Option<Timestamp>> = signals.iter().map(effective_time).collect();
    let latest = times.iter().flatten().max().copied();
    let unique_latest = match latest {
        Some(max_t) => times.iter().filter(|t| **t == Some(max_t)).count() == 1,
        None => false,
    };

    let mut scored: Vec<(Option<Timestamp>, RankedMember)> = signals
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let is_latest = unique_latest && times[i] == latest;
            let mut reasons = Vec::new();
            if is_latest {
                if let Some(t) = m.internal_modified {
                    reasons.push(RankSignal {
                        name: "internal_modified".into(),
                        detail: format!("파일 내부 수정시각이 가장 늦음 ({t})"),
                    });
                } else if let Some(t) = m.fs_mtime {
                    reasons.push(RankSignal {
                        name: "fs_mtime".into(),
                        detail: format!("파일시스템 수정시각이 가장 늦음 ({t})"),
                    });
                }
            }
            (
                times[i],
                RankedMember {
                    path: m.path.clone(),
                    rank: 0,
                    score: if is_latest { 1.0 } else { 0.0 },
                    reasons,
                },
            )
        })
        .collect();

    scored.sort_by(|a, b| match (a.0, b.0) {
        (Some(ta), Some(tb)) => tb.cmp(&ta).then_with(|| a.1.path.cmp(&b.1.path)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.path.cmp(&b.1.path),
    });
    let mut ranked: Vec<RankedMember> = scored.into_iter().map(|(_, m)| m).collect();
    for (idx, m) in ranked.iter_mut().enumerate() {
        m.rank = idx as u32 + 1;
    }

    let confidence = if unique_latest { 0.9 } else { 0.5 };

    FamilyRanking {
        family_id,
        ranked,
        confidence,
    }
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

    fn reason_names(r: &RankedMember) -> Vec<&str> {
        r.reasons.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn later_copy_beats_older_final_filename() {
        let mut newer_copy = member("제안서_복사본.docx");
        newer_copy.internal_modified = Some(ts("2026-08-10T09:00:00Z"));
        let mut older_final = member("제안서_최종.docx");
        older_final.internal_modified = Some(ts("2026-08-01T09:00:00Z"));
        let r = rank(0, &[older_final, newer_copy]);
        assert_eq!(top(&r).path, PathBuf::from("제안서_복사본.docx"));
        assert!(!reason_names(top(&r)).contains(&"filename"));
    }

    #[test]
    fn later_v1_beats_older_v3() {
        let mut v1 = member("보고서_v1.docx");
        v1.internal_modified = Some(ts("2026-08-10T09:00:00Z"));
        let mut v3 = member("보고서_v3.docx");
        v3.internal_modified = Some(ts("2026-08-01T09:00:00Z"));
        let r = rank(0, &[v3, v1]);
        assert_eq!(top(&r).path, PathBuf::from("보고서_v1.docx"));
    }

    #[test]
    fn later_draft_beats_older_container() {
        let mut draft = member("보고서_초안.docx");
        draft.internal_modified = Some(ts("2026-08-10T09:00:00Z"));
        let mut final_doc = member("보고서_최종.docx");
        final_doc.internal_modified = Some(ts("2026-08-01T09:00:00Z"));
        let r = rank(0, &[final_doc, draft]);
        assert_eq!(top(&r).path, PathBuf::from("보고서_초안.docx"));
        assert!(!reason_names(top(&r)).contains(&"contains"));
    }

    #[test]
    fn filename_does_not_break_time_ties() {
        let a = member("제안서_최종.docx");
        let b = member("제안서_복사본.docx");
        let r = rank(0, &[b, a]);
        assert!(
            r.confidence <= 0.5,
            "no times: coin flip, got {}",
            r.confidence
        );
        assert!(!reason_names(top(&r)).iter().any(|n| *n == "filename"));
    }

    #[test]
    fn coin_flip_has_low_confidence() {
        let a = member("a.docx");
        let b = member("b.docx");
        let r = rank(0, &[a, b]);
        assert!(r.confidence <= 0.5, "tie must not be confident");
    }

    #[test]
    fn unique_latest_time_is_confident() {
        let mut a = member("제안서_복사본.docx");
        a.internal_modified = Some(ts("2026-08-05T09:00:00Z"));
        let mut b = member("제안서_최종_v3.docx");
        b.internal_modified = Some(ts("2026-08-01T09:00:00Z"));
        let r = rank(0, &[b, a]);
        assert_eq!(top(&r).path, PathBuf::from("제안서_복사본.docx"));
        assert!(r.confidence > 0.7, "got {}", r.confidence);
        let names = reason_names(top(&r));
        assert_eq!(names, vec!["internal_modified"]);
    }

    #[test]
    fn fs_mtime_ranks_when_no_internal_time() {
        let mut a = member("a.txt");
        a.fs_mtime = Some(ts("2026-08-01T09:00:00Z"));
        let mut b = member("b.txt");
        b.fs_mtime = Some(ts("2026-08-05T09:00:00Z"));
        let r = rank(0, &[a, b]);
        assert_eq!(top(&r).path, PathBuf::from("b.txt"));
        assert_eq!(reason_names(top(&r)), vec!["fs_mtime"]);
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
