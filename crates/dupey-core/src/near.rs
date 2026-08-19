use gaoya::minhash::{compute_minhash_similarity, MinHasher, MinHasher64V1};
use gaoya::text::whitespace_split;

/// Number of MinHash permutations (signature length).
pub const NUM_PERM: usize = 128;

/// Character n-gram size used to shingle canonical text.
pub const CHAR_NGRAM: usize = 5;

/// Default Jaccard threshold for treating two docs as one family.
pub const DEFAULT_NEAR_THRESHOLD: f64 = 0.90;

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

fn char_ngrams(text: &str, n: usize) -> impl Iterator<Item = String> + '_ {
    let normalized: String = whitespace_split(&text.to_lowercase()).collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = normalized.chars().collect();
    let len = chars.len();
    let take = if len < n { 1.max(len) } else { len.saturating_sub(n) + 1 };
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
    fn unrelated_is_low() {
        let a = proposal();
        let b = "오늘 점심은 김치찌개다. 오후에는 운동을 하고 책을 읽는다.";
        let s = score(&near_sig(&a), &near_sig(b));
        assert!(s < 0.3, "expected unrelated score, got {s}");
    }
}
