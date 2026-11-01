use std::hash::{DefaultHasher, Hash, Hasher};

use gaoya::minhash::{compute_minhash_similarity, MinHasher, MinHasher64V1};
use gaoya::text::whitespace_split;

/// Number of MinHash permutations (signature length).
pub const NUM_PERM: usize = 128;

/// Character n-gram size used to shingle canonical text.
pub const CHAR_NGRAM: usize = 5;

/// Default Jaccard threshold for treating two docs as one family.
pub const DEFAULT_NEAR_THRESHOLD: f64 = 0.90;
/// Default containment threshold for the "draft inside final" relation.
///
/// Containment divides by the smaller document, so the same number is a far
/// weaker bar than Jaccard: a shared corporate template can occupy 90% of a
/// short document without the two being versions of each other. Contains
/// therefore gets its own, stricter gate, sized for the plain-addition case
/// (draft kept verbatim, appendix added) that this relation exists to catch.
pub const DEFAULT_CONTAINS_THRESHOLD: f64 = 0.96;
/// Default Jaccard floor a `contains` pair must also clear.
///
/// Containment ignores everything outside the smaller document, so without a
/// floor a short fragment is "contained" in every long document that quotes
/// it, and those fragments chain unrelated files into one component. At 0.4
/// the container can still be roughly 2.5x the contained document, which
/// covers draft-plus-appendix while excluding fragment-scale matches.
pub const DEFAULT_CONTAINS_MIN_JACCARD: f64 = 0.40;
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
    let normalized = normalize_for_ngrams(text);
    let values = hasher.create_signature(char_ngrams(&normalized, CHAR_NGRAM));
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
    /// Compact the buffer once it holds this many hashes, so a huge,
    /// repetitive document never materialises one hash per n-gram.
    const COMPACT_AT: usize = 1 << 22;
    let normalized = normalize_for_ngrams(text);
    let mut grams = char_ngrams(&normalized, CHAR_NGRAM);
    // Sized up front: growing by doubling leaves a trail of freed large
    // blocks that the system allocator may keep cached for the whole scan.
    let mut v: Vec<u64> = Vec::with_capacity(grams.len().min(COMPACT_AT));
    let mut compact_at = COMPACT_AT;
    while let Some(g) = grams.next() {
        // DefaultHasher is deterministic across calls; RandomState is
        // not, and would make cross-document comparison meaningless.
        let mut h = DefaultHasher::new();
        g.hash(&mut h);
        v.push(h.finish());
        if v.len() >= compact_at {
            v.sort_unstable();
            v.dedup();
            compact_at = COMPACT_AT.max(v.len() * 2);
            let wanted = compact_at.min(v.len() + grams.len());
            v.reserve_exact(wanted.saturating_sub(v.len()));
        }
    }
    v.sort_unstable();
    v.dedup();
    // Dedup keeps the pre-dedup capacity; every document's shingles stay
    // resident until clustering ends, so release the slack.
    v.shrink_to_fit();
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

/// Every overlap metric for one document pair, from a single merge intersect.
///
/// Jaccard and both containment directions share one numerator, so computing
/// them together costs the same as computing any one of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overlap {
    pub shared: usize,
    pub jaccard: f64,
    /// |A ∩ B| / |B|: how much of B lives inside A.
    pub b_in_a: f64,
    /// |A ∩ B| / |A|: how much of A lives inside B.
    pub a_in_b: f64,
}

impl Overlap {
    /// The stronger containment direction.
    pub fn max_containment(&self) -> f64 {
        self.b_in_a.max(self.a_in_b)
    }
}

/// Compute Jaccard and both containment directions for sorted, deduplicated
/// shingle hashes in one pass.
pub fn overlap(a: &[u64], b: &[u64]) -> Overlap {
    overlap_at_least(a, b, 0).expect("a zero floor is always reachable")
}

/// [`overlap`] that abandons the intersect once `min_shared` is provably out
/// of reach.
///
/// The remaining shingles on either side cap how much overlap is still
/// possible, so a pair that cannot clear the caller's floor is dropped
/// without scanning the rest of either vector. The test is exact: nothing
/// that could have reached `min_shared` is discarded.
pub fn overlap_at_least(a: &[u64], b: &[u64], min_shared: usize) -> Option<Overlap> {
    let mut shared = 0usize;
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        if shared + (a.len() - i).min(b.len() - j) < min_shared {
            return None;
        }
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
    if shared < min_shared {
        return None;
    }
    let union = a.len() + b.len() - shared;
    let ratio = |num: usize, den: usize| {
        if den == 0 {
            0.0
        } else {
            num as f64 / den as f64
        }
    };
    Some(Overlap {
        shared,
        jaccard: if union == 0 {
            1.0
        } else {
            shared as f64 / union as f64
        },
        b_in_a: ratio(shared, b.len()),
        a_in_b: ratio(shared, a.len()),
    })
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

/// Lowercase, split on ASCII punctuation/whitespace, rejoin with single
/// spaces. Built in one pass without an intermediate token vector.
fn normalize_for_ngrams(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    for token in whitespace_split(&lower) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(token);
    }
    out
}

/// Sliding windows of `n` chars over `s` as borrowed slices (a single
/// shorter window when `s` has fewer than `n` chars, none when empty).
/// The window count is known up front, so collecting allocates once.
fn char_ngrams(s: &str, n: usize) -> CharNgrams<'_> {
    let windows = match s.chars().count() {
        0 => 0,
        chars if chars < n => 1,
        chars => chars - n + 1,
    };
    let mut head = s.char_indices();
    for _ in 0..n {
        if head.next().is_none() {
            break;
        }
    }
    CharNgrams {
        s,
        tail: s.char_indices(),
        head,
        remaining: windows,
    }
}

struct CharNgrams<'a> {
    s: &'a str,
    tail: std::str::CharIndices<'a>,
    /// Positioned `n` chars ahead of `tail`; its next byte offset is the
    /// end of the current window.
    head: std::str::CharIndices<'a>,
    remaining: usize,
}

impl<'a> Iterator for CharNgrams<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let (start, _) = self.tail.next()?;
        let end = self.head.next().map_or(self.s.len(), |(end, _)| end);
        Some(&self.s[start..end])
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for CharNgrams<'_> {}

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
    fn ngram_windows_report_their_exact_count() {
        // Exact sizes let the MinHash and shingle buffers be allocated once
        // instead of grown by doubling.
        for s in [
            "",
            "ab",
            "abcde",
            "abcdef",
            "가나다라마바사",
            "x y z w v u t s",
        ] {
            let mut grams = char_ngrams(s, 5);
            let total = char_ngrams(s, 5).count();
            assert_eq!(grams.len(), total, "{s:?}");
            for left in (0..total).rev() {
                grams.next();
                assert_eq!(grams.size_hint(), (left, Some(left)), "{s:?}");
            }
            assert_eq!(grams.next(), None);
        }
    }

    #[test]
    fn ngram_windows_cover_char_boundaries() {
        fn grams(s: &str) -> Vec<&str> {
            char_ngrams(s, 5).collect()
        }
        assert!(grams("").is_empty());
        assert_eq!(grams("ab"), ["ab"]);
        assert_eq!(grams("abcde"), ["abcde"]);
        assert_eq!(grams("abcdef"), ["abcde", "bcdef"]);
        assert_eq!(grams("가나다라마바"), ["가나다라마", "나다라마바"]);
    }

    #[test]
    fn shingles_dedup_survives_compaction() {
        // Longer than the compaction threshold with heavy repetition.
        let text = "abcdefgh ".repeat(700_000);
        let a = shingles(&text);
        let b = shingles(&"abcdefgh ".repeat(10));
        assert_eq!(a, b);
        assert_eq!(a.capacity(), a.len());
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
