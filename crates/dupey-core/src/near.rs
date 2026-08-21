use std::hash::{DefaultHasher, Hash, Hasher};

use gaoya::minhash::{compute_minhash_similarity, MinHasher, MinHasher64V1};
use gaoya::text::whitespace_split;

/// Number of MinHash permutations (signature length).
pub const NUM_PERM: usize = 128;

/// Character n-gram size used to shingle canonical text.
pub const CHAR_NGRAM: usize = 5;

/// Default Jaccard threshold for treating two docs as one family.
pub const DEFAULT_NEAR_THRESHOLD: f64 = 0.90;
/// MinHash estimate required before paying for exact shingle Jaccard.
pub const MINHASH_CANDIDATE_THRESHOLD: f64 = 0.80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearSignature {
    pub values: Vec<u64>,
}

impl NearSignature {
    pub fn jaccard(&self, other: &Self) -> f64 {
        compute_minhash_similarity(&self.values, &other.values)
    }
}

/// Build a 128-wide MinHash over character 5-grams of canonical text.
pub fn near_sig(text: &str) -> NearSignature {
    let hasher = MinHasher64V1::new(NUM_PERM);
    let grams = char_ngrams(text, CHAR_NGRAM);
    let values = hasher.create_signature(grams);
    NearSignature { values }
}

pub fn score(a: &NearSignature, b: &NearSignature) -> f64 {
    a.jaccard(b)
}

/// Sorted, deduped hashed character n-grams. Used for containment
/// checks ("A contains B") that a Jaccard estimate cannot express.
/// Sorted vectors make containment a merge intersect, which stays cheap
/// even on template-heavy corpora with many candidate pairs.
pub fn shingles(text: &str) -> Vec<u64> {
    let mut v: Vec<u64> = char_ngrams(text, CHAR_NGRAM)
        .map(|g| {
            // DefaultHasher is deterministic across calls; RandomState is
            // not, and would make cross-document comparison meaningless.
            let mut h = DefaultHasher::new();
            g.hash(&mut h);
            h.finish()
        })
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Exact Jaccard similarity for sorted, deduplicated shingle hashes.
pub fn exact_jaccard(a: &[u64], b: &[u64]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut shared = 0usize;
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = a.len() + b.len() - shared;
    shared as f64 / union as f64
}

/// |A ∩ B| / |B|: how much of B lives inside A.
pub fn containment(a: &[u64], b: &[u64]) -> f64 {
    containment_at_least(a, b, 0.0)
}

/// Merge-intersect containment with early exit once `threshold` becomes
/// reachable or unreachable. containment(a, b) >= threshold ?
/// Returns the exact containment; callers compare against the threshold.
pub fn containment_at_least(a: &[u64], b: &[u64], threshold: f64) -> f64 {
    if b.is_empty() {
        return 0.0;
    }
    let needed = (b.len() as f64 * threshold).ceil() as usize;
    let mut shared = 0usize;
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
        if threshold > 0.0 {
            let remaining = b.len() - j;
            if shared + remaining < needed {
                return 0.0; // unreachable
            }
        }
    }
    shared as f64 / b.len() as f64
}

fn char_ngrams(text: &str, n: usize) -> impl Iterator<Item = String> + '_ {
    let normalized: String = whitespace_split(&text.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    let chars: Vec<char> = normalized.chars().collect();
    let len = chars.len();
    let take = if len < n {
        usize::from(len > 0)
    } else {
        len - n + 1
    };
    (0..take).map(move |i| {
        let end = (i + n).min(len);
        chars[i..end].iter().collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal() -> String {
        "프로젝트 제안서\n\n1. 배경\n본 제안은 2026년 하반기 사무 자동화 도입을 위한 것이다. \
         현재 팀은 문서가 폴더에 흩어져 있고 최신본을 찾기 어렵다.\n\n2. 범위\n문서 수집, \
         중복 정리, 검색, 권한은 1단계 범위에 포함하지 않는다.\n\n3. 일정\n킥오프는 9월 1일, \
         파일럿은 10월 말까지 진행한다.\n\n4. 예산\n예상 비용은 3,200만 원이다.\n"
            .to_string()
    }

    #[test]
    fn identical_is_one() {
        let t = proposal();
        assert!((score(&near_sig(&t), &near_sig(&t)) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn one_line_edit_stays_high() {
        let a = proposal();
        let b = a.replace("3,200만 원", "3,500만 원");
        let s = score(&near_sig(&a), &near_sig(&b));
        assert!(s >= 0.85, "expected near-dup score, got {s}");
    }

    #[test]
    fn shingles_support_containment() {
        let draft = proposal();
        let final_doc = format!("{draft}5. 부록\n참고 표와 체크리스트를 덧붙인다.\n");
        let a = shingles(&final_doc);
        let b = shingles(&draft);
        assert!((containment(&a, &b) - 1.0).abs() < 1e-9);
        assert!(containment(&b, &a) < 1.0);
    }

    #[test]
    fn empty_text_has_no_shingles() {
        assert!(shingles("").is_empty());
    }

    #[test]
    fn unrelated_is_low() {
        let a = proposal();
        let b = "오늘 점심은 김치찌개다. 오후에는 운동을 하고 책을 읽는다.";
        let s = score(&near_sig(&a), &near_sig(b));
        assert!(s < 0.3, "expected unrelated score, got {s}");
    }

    #[test]
    fn exact_jaccard_is_computed_from_complete_shingle_sets() {
        assert!((exact_jaccard(&[1, 2, 3, 4], &[1, 2, 3, 5]) - 0.6).abs() < 1e-9);
        assert_eq!(exact_jaccard(&[], &[]), 1.0);
        assert_eq!(exact_jaccard(&[1], &[]), 0.0);
    }
}
