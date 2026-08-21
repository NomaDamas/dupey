//! Cluster documents into families (exact / near / contains).
//!
//! exact groups by SHA-256 of canonical text, near uses LSH over MinHash
//! signatures at the family threshold (default 0.90), contains uses
//! shingle containment for the draft-inside-final case. Documents with no
//! comparable text (e.g. scanned PDFs) never cluster.

use std::path::PathBuf;

use crate::near::{containment, containment_at_least, near_sig, score, shingles, NearSignature};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Relation {
    Exact,
    Near,
    Contains,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FamilyMember {
    pub path: PathBuf,
    pub exact_hash: String,
    pub relation: Relation,
    /// MinHash Jaccard estimate against the family anchor.
    pub near_score: Option<f64>,
    /// Shingle containment against the family anchor, when computed.
    pub containment: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Family {
    pub id: u32,
    pub members: Vec<FamilyMember>,
}

/// Bottom-k sketch width: k smallest shingle hashes per document.
/// If one document's k-sketch is a subset of another's shingles, it is a
/// containment candidate (recall-friendly; candidates are verified).
pub const SKETCH_K: usize = 64;

/// One scanned document ready for clustering.
#[derive(Debug, Clone)]
pub struct ScannedDoc {
    pub path: PathBuf,
    pub exact_hash: String,
    pub sig: NearSignature,
    pub shingles: Vec<u64>,
    /// k smallest shingle hashes; inverted index key for containment
    /// candidate generation.
    pub sketch: Vec<u64>,
}

impl ScannedDoc {
    pub fn from_text(path: PathBuf, text: &str) -> Self {
        let shingles = shingles(text);
        Self::from_precomputed(path, crate::exact_hash_hex(text), near_sig(text), shingles)
    }

    pub fn from_precomputed(
        path: PathBuf,
        exact_hash: String,
        sig: NearSignature,
        shingles: Vec<u64>,
    ) -> Self {
        Self {
            path,
            exact_hash,
            sig,
            sketch: shingles.iter().take(SKETCH_K).copied().collect(),
            shingles,
        }
    }

    /// Scanned PDFs and empty files have no comparable content.
    fn comparable(&self) -> bool {
        !self.shingles.is_empty()
    }
}

/// Group comparable documents into families of 2+ members.
///
/// LSH runs at a loose candidate threshold (0.5) and every candidate pair
/// is verified with the exact MinHash estimate, so LSH approximation can
/// only cost recall, never precision. Contains candidates need the loose
/// threshold: a draft inside a final can sit at Jaccard 0.5-0.7.
pub fn cluster(docs: &[ScannedDoc], threshold: f64) -> Vec<Family> {
    // gaoya requires bands x width == signature length (128). 32x4
    // keeps template pairs (~0.8 Jaccard) from flooding candidates;
    // containment recall is covered by the bottom-k sketch index, so
    // LSH only needs to be reliable at the near threshold.
    const LSH_BANDS: usize = 32;
    const LSH_BAND_WIDTH: usize = 4;
    const LSH_CANDIDATE_THRESHOLD: f64 = 0.5;

    let comp: Vec<usize> = (0..docs.len()).filter(|&i| docs[i].comparable()).collect();
    let mut uf = UnionFind::new(docs.len());

    // exact: same canonical SHA-256.
    let mut by_hash: std::collections::HashMap<&str, Vec<usize>> = std::collections::HashMap::new();
    for &i in &comp {
        by_hash.entry(&docs[i].exact_hash).or_default().push(i);
    }
    for group in by_hash.values() {
        for &i in &group[1..] {
            uf.union(group[0], i);
        }
    }

    // near / contains: candidate generation, every candidate verified.
    // LSH covers near; a bottom-k inverted index covers containment of
    // documents too small for LSH bands to see (draft inside final).
    let mut index =
        gaoya::minhash::MinHashIndex::new(LSH_BANDS, LSH_BAND_WIDTH, LSH_CANDIDATE_THRESHOLD);
    for &i in &comp {
        index.insert(i, docs[i].sig.values.clone());
    }

    let mut sketch_postings: std::collections::HashMap<u64, Vec<usize>> =
        std::collections::HashMap::new();
    for &i in &comp {
        for &h in &docs[i].sketch {
            sketch_postings.entry(h).or_default().push(i);
        }
    }
    // Drop ubiquitous sketch values (padding shingles, shared
    // boilerplate): containment at 0.9 needs hundreds of shared
    // shingles, so a value present in many docs discriminates nothing.
    // The 64-doc cap keeps draft-vs-many-revisions queries intact.
    let max_df = 64;
    sketch_postings.retain(|_, posting| posting.len() <= max_df);

    let mut candidates: std::collections::BTreeSet<(usize, usize)> =
        std::collections::BTreeSet::new();
    for &i in &comp {
        for &j in index.query(&docs[i].sig.values) {
            if j != i {
                candidates.insert((i.min(j), i.max(j)));
            }
        }
    }
    for &i in &comp {
        for &h in &docs[i].sketch {
            if let Some(posting) = sketch_postings.get(&h) {
                for &j in posting {
                    // Skip pairs already unioned by LSH/exact; popular
                    // sketch values otherwise make candidate collection
                    // quadratic on template-heavy corpora.
                    if j != i && uf.find(i) != uf.find(j) {
                        candidates.insert((i.min(j), i.max(j)));
                    }
                }
            }
        }
    }

    for (i, j) in candidates {
        if uf.find(i) == uf.find(j) {
            continue;
        }
        let s = score(&docs[i].sig, &docs[j].sig);
        if s >= threshold {
            uf.union(i, j);
            continue;
        }
        // Cheap necessary conditions before the merge intersect:
        // containment(b in a) >= t needs |a| >= t|b| and, since matching
        // minima land in both signatures, a MinHash estimate at least
        // ~t. The 0.1/0.2 slack covers 128-perm estimation error; the
        // full intersect decides.
        let (la, lb) = (docs[i].shingles.len() as f64, docs[j].shingles.len() as f64);
        if la >= threshold * lb
            && s + 0.1 >= threshold * lb / (la + lb - threshold * lb)
            && s >= threshold - 0.5
        {
            let c = containment_at_least(&docs[i].shingles, &docs[j].shingles, threshold);
            if c >= threshold {
                uf.union(i, j);
                continue;
            }
        }
        if lb >= threshold * la
            && s + 0.1 >= threshold * la / (la + lb - threshold * la)
            && s >= threshold - 0.5
        {
            let c = containment_at_least(&docs[j].shingles, &docs[i].shingles, threshold);
            if c >= threshold {
                uf.union(i, j);
            }
        }
    }

    // Components with 2+ members become families.
    let mut components: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for &i in &comp {
        components.entry(uf.find(i)).or_default().push(i);
    }
    let mut groups: Vec<Vec<usize>> = components.into_values().filter(|g| g.len() >= 2).collect();
    groups.sort_by_key(|g| g[0]);

    groups
        .into_iter()
        .enumerate()
        .map(|(id, group)| {
            let anchor = group[0];
            let members = group
                .iter()
                .map(|&i| member_against_anchor(docs, i, anchor, threshold))
                .collect();
            Family {
                id: id as u32,
                members,
            }
        })
        .collect()
}

/// Explainable per-member relation against the family anchor.
fn member_against_anchor(
    docs: &[ScannedDoc],
    i: usize,
    anchor: usize,
    threshold: f64,
) -> FamilyMember {
    if i == anchor || docs[i].exact_hash == docs[anchor].exact_hash {
        return FamilyMember {
            path: docs[i].path.clone(),
            exact_hash: docs[i].exact_hash.clone(),
            relation: Relation::Exact,
            near_score: Some(1.0),
            containment: Some(1.0),
        };
    }
    let s = score(&docs[i].sig, &docs[anchor].sig);
    let c_in = containment(&docs[anchor].shingles, &docs[i].shingles);
    let c_out = containment(&docs[i].shingles, &docs[anchor].shingles);
    let c = c_in.max(c_out);
    let relation = if s >= threshold {
        Relation::Near
    } else if c >= threshold {
        Relation::Contains
    } else {
        // Joined transitively through another member.
        Relation::Near
    };
    FamilyMember {
        path: docs[i].path.clone(),
        exact_hash: docs[i].exact_hash.clone(),
        relation,
        near_score: Some(s),
        containment: Some(c),
    }
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[rb.max(ra)] = ra.min(rb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::near::DEFAULT_NEAR_THRESHOLD;

    fn doc(name: &str, text: &str) -> ScannedDoc {
        ScannedDoc::from_text(PathBuf::from(name), text)
    }

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

    #[test]
    fn exact_group() {
        let t = proposal();
        let docs = vec![doc("a.docx", &t), doc("b 복사본.docx", &t)];
        let fams = cluster(&docs, DEFAULT_NEAR_THRESHOLD);
        assert_eq!(fams.len(), 1);
        assert_eq!(fams[0].members.len(), 2);
        assert!(fams[0]
            .members
            .iter()
            .all(|m| m.relation == Relation::Exact));
    }

    #[test]
    fn near_family_above_threshold() {
        let a = proposal();
        let b = a.replace("3,200만 원", "3,500만 원");
        let docs = vec![doc("제안서.docx", &a), doc("제안서_최종.docx", &b)];
        let fams = cluster(&docs, DEFAULT_NEAR_THRESHOLD);
        assert_eq!(fams.len(), 1);
        let near = fams[0]
            .members
            .iter()
            .find(|m| m.relation == Relation::Near)
            .expect("one member should join by near");
        assert!(near.near_score.unwrap() >= 0.85);
    }

    #[test]
    fn contains_family() {
        let draft = paragraphs("초안", 10);
        // Appendix uses wholly different vocabulary so Jaccard stays well
        // below the near threshold while containment stays 1.0.
        let appendix = [
            "부록 A: 분기별 매출 측정 결과와 원자재 단가 표를 수록한다.",
            "부록 B: 현장 설문 응답 원문과 면담 메모를 옮긴다.",
            "부록 C: 참고 문헌 목록과 인용 출처를 덧붙인다.",
            "부록 D: 운영 체크리스트와 검수 서명란을 첨부한다.",
            "부록 E: 시설 도면 축척과 배선 경로 요약을 싣는다.",
        ]
        .join("\n");
        let final_doc = format!("{draft}\n{appendix}");
        let docs = vec![
            doc("보고서_초안.docx", &draft),
            doc("보고서_최종.docx", &final_doc),
        ];
        let fams = cluster(&docs, DEFAULT_NEAR_THRESHOLD);
        assert_eq!(fams.len(), 1, "draft inside final must still be family");
        let contains = fams[0]
            .members
            .iter()
            .find(|m| m.relation == Relation::Contains)
            .expect("one member should join by contains");
        assert!(contains.containment.unwrap() >= 0.9);
        // Jaccard is too low for near: this is what contains is for.
        assert!(contains.near_score.unwrap() < DEFAULT_NEAR_THRESHOLD);
    }

    #[test]
    fn unrelated_docs_form_no_family() {
        let docs = vec![
            doc("a.txt", &proposal()),
            doc(
                "b.txt",
                "오늘 점심은 김치찌개다. 오후에는 운동을 하고 책을 읽는다.",
            ),
        ];
        assert!(cluster(&docs, DEFAULT_NEAR_THRESHOLD).is_empty());
    }

    #[test]
    fn empty_text_never_clusters() {
        // Two scanned PDFs: identical (empty) text, but no content to compare.
        let docs = vec![doc("scan1.pdf", ""), doc("scan2.pdf", "")];
        assert!(cluster(&docs, DEFAULT_NEAR_THRESHOLD).is_empty());
    }

    #[test]
    fn transitive_near_merges() {
        let a = paragraphs("버전", 10);
        let b = a.replace("1번째", "첫번째");
        let c = b.replace("10번째", "열th");
        let docs = vec![doc("v1.docx", &a), doc("v2.docx", &b), doc("v3.docx", &c)];
        let fams = cluster(&docs, DEFAULT_NEAR_THRESHOLD);
        assert_eq!(fams.len(), 1);
        assert_eq!(fams[0].members.len(), 3);
    }

    #[test]
    fn contains_found_even_when_lsh_misses() {
        // A draft inside a much larger final: Jaccard is far below the
        // near threshold, so LSH alone can miss the pair. The bottom-k
        // sketch index must surface it as a containment candidate.
        let draft = paragraphs("초안", 10);
        let mut final_doc = draft.clone();
        for pool in 0..30 {
            final_doc.push('\n');
            final_doc.push_str(&paragraphs(&format!("부록{pool}"), 8));
        }
        let docs = vec![doc("초안.docx", &draft), doc("최종.docx", &final_doc)];
        let fams = cluster(&docs, DEFAULT_NEAR_THRESHOLD);
        assert_eq!(fams.len(), 1, "draft in big final must cluster via sketch");
        assert!(fams[0]
            .members
            .iter()
            .any(|m| m.relation == Relation::Contains));
    }

    #[test]
    fn sketch_size_bounded() {
        let big = paragraphs("대형", 2000);
        let d = doc("big.txt", &big);
        assert_eq!(d.sketch.len(), SKETCH_K);
    }

    #[test]
    fn sketch_is_prefix_of_sorted_shingles() {
        let d = doc("a.txt", &paragraphs("x", 20));
        assert_eq!(d.sketch, d.shingles[..d.sketch.len()].to_vec());
    }

    #[test]
    fn from_precomputed_reuses_scan_fingerprint() {
        let text = proposal();
        let exact_hash = crate::exact_hash_hex(&text);
        let sig = near_sig(&text);
        let shingles = shingles(&text);
        let expected_sig = sig.clone();
        let expected_shingles = shingles.clone();

        let d = ScannedDoc::from_precomputed(
            PathBuf::from("prepared.txt"),
            exact_hash.clone(),
            sig,
            shingles,
        );

        assert_eq!(d.path, PathBuf::from("prepared.txt"));
        assert_eq!(d.exact_hash, exact_hash);
        assert_eq!(d.sig, expected_sig);
        assert_eq!(
            d.sketch,
            expected_shingles[..expected_shingles.len().min(SKETCH_K)].to_vec()
        );
        assert_eq!(d.shingles, expected_shingles);
    }

    #[test]
    fn multiple_families() {
        let p = proposal();
        let p2 = p.replace("3,200만 원", "3,500만 원");
        let r = paragraphs("보고", 10);
        let docs = vec![
            doc("제안서.docx", &p),
            doc("제안서_최종.docx", &p2),
            doc("보고서.docx", &r),
            doc("보고서 사본.docx", &r),
            doc("메모.txt", "내일 회의실 예약하기"),
        ];
        let fams = cluster(&docs, DEFAULT_NEAR_THRESHOLD);
        assert_eq!(fams.len(), 2);
        let mut sizes: Vec<usize> = fams.iter().map(|f| f.members.len()).collect();
        sizes.sort();
        assert_eq!(sizes, vec![2, 2]);
        // IDs are stable and unique.
        assert_ne!(fams[0].id, fams[1].id);
    }
}
