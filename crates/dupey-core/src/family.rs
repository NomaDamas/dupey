//! Cluster documents into families (exact / near / contains).
//!
//! exact groups by SHA-256 of canonical text, near uses LSH over MinHash
//! candidates verified with exact shingle Jaccard at the near threshold
//! (default 0.90), contains uses shingle containment at its own, stricter
//! threshold (default 0.96) for the draft-inside-final case. Both relations
//! are computed for every candidate pair, and every verified pair is kept as
//! a [`FamilyEdge`] so callers can see why a family exists instead of being
//! handed a single collapsed label. Documents with no comparable text only
//! cluster when their original bytes are exact matches.

use std::path::PathBuf;

use rayon::prelude::*;

use crate::near::{
    near_sig, overlap_at_least, score, shingles, NearSignature, DEFAULT_CONTAINS_MIN_JACCARD,
    DEFAULT_CONTAINS_THRESHOLD, DEFAULT_NEAR_THRESHOLD, MINHASH_CANDIDATE_THRESHOLD,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Relation {
    Exact,
    Near,
    Contains,
}

/// How a whole family holds together.
///
/// `Mixed` exists so a family that is part near and part contains is never
/// folded into whichever label happens to win a priority list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FamilyLabel {
    Exact,
    Near,
    Contains,
    Mixed,
}

/// One verified pair inside a family: the evidence that produced a merge.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FamilyEdge {
    pub relation: Relation,
    /// For `contains`, the container. Otherwise the lower-indexed document.
    pub a: PathBuf,
    /// For `contains`, the document living inside `a`.
    pub b: PathBuf,
    /// MinHash Jaccard estimate for the pair.
    pub near_score: f64,
    /// Exact shingle Jaccard for the pair.
    pub jaccard: f64,
    /// `|a ∩ b| / |b|`: how much of `b` lives inside `a`.
    pub containment: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FamilyMember {
    pub path: PathBuf,
    pub exact_hash: String,
    /// Relation of this member's strongest verified edge: how it actually
    /// joined the family, never a fallback guess.
    pub relation: Relation,
    /// The other endpoint of that edge.
    pub joined_with: Option<PathBuf>,
    /// MinHash Jaccard estimate against `joined_with`.
    pub near_score: Option<f64>,
    /// Exact shingle Jaccard against `joined_with`.
    pub jaccard: Option<f64>,
    /// Shingle containment against `joined_with`.
    pub containment: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Family {
    pub id: u32,
    pub members: Vec<FamilyMember>,
    /// Every verified pair in this family, in stable path order.
    pub edges: Vec<FamilyEdge>,
}

impl Family {
    /// The family-wide relation, or `Mixed` when members joined differently.
    pub fn label(&self) -> FamilyLabel {
        let mut relations = self.members.iter().map(|m| m.relation);
        let first = match relations.next() {
            Some(r) => r,
            None => return FamilyLabel::Mixed,
        };
        if relations.any(|r| r != first) {
            return FamilyLabel::Mixed;
        }
        match first {
            Relation::Exact => FamilyLabel::Exact,
            Relation::Near => FamilyLabel::Near,
            Relation::Contains => FamilyLabel::Contains,
        }
    }
}

/// Thresholds for [`cluster`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterConfig {
    /// Exact shingle Jaccard required for a `near` merge.
    pub near_threshold: f64,
    /// Shingle containment required for a `contains` merge. Kept separate
    /// from `near_threshold` because containment divides by the smaller
    /// document and is therefore much easier to satisfy.
    pub contains_threshold: f64,
    /// Jaccard floor a `contains` pair must also clear.
    ///
    /// Containment alone says nothing about size: a two-page fragment is
    /// fully "contained" in a fifty-page report it shares a header with.
    /// Left unbounded, such fragments bridge unrelated documents into one
    /// enormous component. The floor keeps contains to pairs that are still
    /// substantially the same document.
    pub contains_min_jaccard: f64,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            near_threshold: DEFAULT_NEAR_THRESHOLD,
            contains_threshold: DEFAULT_CONTAINS_THRESHOLD,
            contains_min_jaccard: DEFAULT_CONTAINS_MIN_JACCARD,
        }
    }
}

impl ClusterConfig {
    pub fn new(near_threshold: f64, contains_threshold: f64) -> Self {
        Self {
            near_threshold,
            contains_threshold,
            ..Self::default()
        }
    }
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
    pub byte_hash: Option<String>,
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
            byte_hash: None,
            sig,
            sketch: shingles.iter().take(SKETCH_K).copied().collect(),
            shingles,
        }
    }

    pub fn from_precomputed_with_byte_hash(
        path: PathBuf,
        exact_hash: String,
        byte_hash: String,
        sig: NearSignature,
        shingles: Vec<u64>,
    ) -> Self {
        let mut doc = Self::from_precomputed(path, exact_hash, sig, shingles);
        doc.byte_hash = Some(byte_hash);
        doc
    }

    /// Scanned PDFs and empty files have no comparable content.
    fn comparable(&self) -> bool {
        !self.shingles.is_empty()
    }
}

/// Group comparable documents into families of 2+ members.
///
/// LSH runs at a loose MinHash candidate threshold (0.8) and a bottom-k
/// sketch index covers containment candidates LSH bands cannot see. Every
/// candidate is then verified in parallel: one merge intersect yields the
/// exact Jaccard and both containment directions at once, so near and
/// contains are always both evaluated for the same pair. Verified pairs are
/// unioned sequentially in sorted order, which keeps the result identical
/// run to run regardless of how the work was scheduled.
pub fn cluster(docs: &[ScannedDoc], config: &ClusterConfig) -> Vec<Family> {
    let threshold = config.near_threshold;
    let candidate_threshold = MINHASH_CANDIDATE_THRESHOLD.min(threshold);
    // gaoya requires bands x width == signature length (128). 16x8
    // retrieves a MinHash-0.8 pair with ~94.7% probability while
    // suppressing the much larger population below the candidate gate.
    const LSH_BANDS: usize = 16;
    const LSH_BAND_WIDTH: usize = 8;
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
    // Empty extraction cannot support fuzzy comparison, but byte-identical
    // files are still exact duplicates (for example, copied scanned PDFs).
    let mut by_byte_hash: std::collections::HashMap<&str, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, doc) in docs.iter().enumerate() {
        if !doc.comparable() {
            if let Some(hash) = doc.byte_hash.as_deref() {
                by_byte_hash.entry(hash).or_default().push(i);
            }
        }
    }
    for group in by_byte_hash.values() {
        for &i in &group[1..] {
            uf.union(group[0], i);
        }
    }

    // near / contains: candidate generation, every candidate verified.
    // LSH covers near; a bottom-k inverted index covers containment of
    // documents too small for LSH bands to see (draft inside final).
    let mut index =
        gaoya::minhash::MinHashIndex::new(LSH_BANDS, LSH_BAND_WIDTH, candidate_threshold);
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
                    // Skip pairs already unioned as exact copies; popular
                    // sketch values otherwise make candidate collection
                    // quadratic on template-heavy corpora.
                    if j != i && uf.find(i) != uf.find(j) {
                        candidates.insert((i.min(j), i.max(j)));
                    }
                }
            }
        }
    }

    let candidates: Vec<(usize, usize)> = candidates.into_iter().collect();

    let mut edges: Vec<VerifiedEdge> = Vec::new();
    for &i in &comp {
        // Exact copies are evidence too: record them so every member of an
        // exact family can point at the twin it matched.
        if let Some(group) = by_hash.get(docs[i].exact_hash.as_str()) {
            if group[0] != i {
                edges.push(VerifiedEdge::exact(group[0], i));
            }
        }
    }
    // Byte-identical files with no comparable text are exact duplicates as
    // well, and their members need the same evidence trail.
    for group in by_byte_hash.values() {
        for &i in &group[1..] {
            edges.push(VerifiedEdge::exact(group[0], i));
        }
    }

    // Verification is the expensive stage and pairs are independent, so a
    // chunk of candidates is verified in parallel and then unioned
    // sequentially in sorted order. Chunking keeps the sequential loop's
    // best optimisation -- skipping pairs already joined by an earlier
    // merge -- while still filling every core. Chunk size is a constant and
    // the candidate list is sorted, so the outcome never depends on thread
    // scheduling or on how many threads rayon happens to use.
    const VERIFY_CHUNK: usize = 512;
    // A merge intersect costs about one pass over both shingle vectors.
    // Below this much total work, rayon's fork/join costs more than the
    // comparisons it spreads out, so short documents stay on one thread.
    const PARALLEL_MIN_WORK: usize = 1 << 20;
    for chunk in candidates.chunks(VERIFY_CHUNK) {
        let pending: Vec<(usize, usize)> = chunk
            .iter()
            .copied()
            .filter(|&(i, j)| uf.find(i) != uf.find(j))
            .collect();
        let work: usize = pending
            .iter()
            .map(|&(i, j)| docs[i].shingles.len() + docs[j].shingles.len())
            .sum();
        let verify = |&(i, j): &(usize, usize)| verify_pair(docs, i, j, config);
        let verified: Vec<Option<VerifiedEdge>> = if work >= PARALLEL_MIN_WORK {
            pending.par_iter().map(verify).collect()
        } else {
            pending.iter().map(verify).collect()
        };
        for edge in verified.into_iter().flatten() {
            uf.union(edge.a, edge.b);
            edges.push(edge);
        }
    }

    // Components with 2+ members become families.
    let mut components: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, doc) in docs.iter().enumerate() {
        if doc.comparable() || doc.byte_hash.is_some() {
            components.entry(uf.find(i)).or_default().push(i);
        }
    }
    let mut groups: Vec<Vec<usize>> = components.into_values().filter(|g| g.len() >= 2).collect();
    groups.sort_by_key(|g| g[0]);

    // Bucket edges by family in one pass: scanning every edge per family
    // would be quadratic on corpora with many families.
    let mut family_of: Vec<Option<usize>> = vec![None; docs.len()];
    for (id, group) in groups.iter().enumerate() {
        for &i in group {
            family_of[i] = Some(id);
        }
    }
    let mut edges_by_family: Vec<Vec<VerifiedEdge>> = vec![Vec::new(); groups.len()];
    for edge in &edges {
        if let Some(id) = family_of[edge.a].or(family_of[edge.b]) {
            edges_by_family[id].push(*edge);
        }
    }

    let mut families: Vec<Family> = groups
        .into_iter()
        .zip(edges_by_family)
        .enumerate()
        .map(|(id, (group, mut family_edges))| {
            family_edges.sort_by(|x, y| {
                edge_rank(y)
                    .partial_cmp(&edge_rank(x))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| (x.a, x.b).cmp(&(y.a, y.b)))
            });
            let members = group
                .iter()
                .map(|&i| member_from_edges(docs, i, &family_edges))
                .collect();
            Family {
                id: id as u32,
                members,
                edges: family_edges.iter().map(|e| e.materialize(docs)).collect(),
            }
        })
        .collect();
    families.sort_by_key(|f| f.id);
    families
}

/// Verify one candidate pair against both relations.
///
/// A single merge intersect produces Jaccard and both containment
/// directions, so near and contains are always evaluated together and the
/// pair is described by whichever relation it actually satisfies.
fn verify_pair(
    docs: &[ScannedDoc],
    i: usize,
    j: usize,
    config: &ClusterConfig,
) -> Option<VerifiedEdge> {
    if docs[i].exact_hash == docs[j].exact_hash {
        // Already covered by an exact edge; a second near edge for the same
        // pair would be duplicate evidence.
        return None;
    }
    let (a, b) = (&docs[i].shingles, &docs[j].shingles);
    // Smallest intersection that could satisfy either relation:
    //   jaccard  = s / (|a| + |b| - s) >= t   <=>  s >= t(|a| + |b|) / (1 + t)
    //   contains = s / min(|a|, |b|)   >= c   <=>  s >= c * min(|a|, |b|)
    // and contains additionally has to clear the Jaccard floor. Anything
    // below both relations' floors is not a family under any label, so the
    // intersect can stop as soon as it becomes unreachable.
    let total = (a.len() + b.len()) as f64;
    let jaccard_floor = |t: f64| t * total / (1.0 + t);
    let near_floor = jaccard_floor(config.near_threshold);
    let contains_floor = (config.contains_threshold * a.len().min(b.len()) as f64)
        .max(jaccard_floor(config.contains_min_jaccard));
    let min_shared = near_floor.min(contains_floor).ceil().max(0.0) as usize;
    let stats = overlap_at_least(a, b, min_shared)?;
    let near_score = score(&docs[i].sig, &docs[j].sig);
    if stats.jaccard >= config.near_threshold {
        return Some(VerifiedEdge {
            relation: Relation::Near,
            a: i,
            b: j,
            near_score,
            jaccard: stats.jaccard,
            containment: stats.max_containment(),
        });
    }
    if stats.max_containment() >= config.contains_threshold
        && stats.jaccard >= config.contains_min_jaccard
    {
        // b_in_a means every shingle of j sits inside i: i is the container.
        let (container, contained) = if stats.b_in_a >= stats.a_in_b {
            (i, j)
        } else {
            (j, i)
        };
        return Some(VerifiedEdge {
            relation: Relation::Contains,
            a: container,
            b: contained,
            near_score,
            jaccard: stats.jaccard,
            containment: stats.max_containment(),
        });
    }
    None
}

/// A verified pair while clustering: document indices, not paths, so the hot
/// loops compare and sort integers instead of cloning path buffers.
#[derive(Debug, Clone, Copy)]
struct VerifiedEdge {
    relation: Relation,
    a: usize,
    b: usize,
    near_score: f64,
    jaccard: f64,
    containment: f64,
}

impl VerifiedEdge {
    fn exact(i: usize, j: usize) -> Self {
        Self {
            relation: Relation::Exact,
            a: i,
            b: j,
            near_score: 1.0,
            jaccard: 1.0,
            containment: 1.0,
        }
    }

    fn touches(&self, i: usize) -> bool {
        self.a == i || self.b == i
    }

    fn other(&self, i: usize) -> usize {
        if self.a == i {
            self.b
        } else {
            self.a
        }
    }

    fn materialize(&self, docs: &[ScannedDoc]) -> FamilyEdge {
        FamilyEdge {
            relation: self.relation,
            a: docs[self.a].path.clone(),
            b: docs[self.b].path.clone(),
            near_score: self.near_score,
            jaccard: self.jaccard,
            containment: self.containment,
        }
    }
}

/// Evidence strength: exact beats near beats contains, then score.
fn edge_rank(edge: &VerifiedEdge) -> f64 {
    let base = match edge.relation {
        Relation::Exact => 2.0,
        Relation::Near => 1.0,
        Relation::Contains => 0.0,
    };
    base + edge.jaccard.max(edge.containment)
}

/// Describe a member by its strongest verified edge instead of guessing a
/// relation against an anchor it may never have been compared with.
fn member_from_edges(docs: &[ScannedDoc], i: usize, edges: &[VerifiedEdge]) -> FamilyMember {
    // `edges` is pre-sorted strongest first, so the first incident edge is
    // this member's best evidence.
    match edges.iter().find(|e| e.touches(i)) {
        Some(edge) => FamilyMember {
            path: docs[i].path.clone(),
            exact_hash: docs[i].exact_hash.clone(),
            relation: edge.relation,
            joined_with: Some(docs[edge.other(i)].path.clone()),
            near_score: Some(edge.near_score),
            jaccard: Some(edge.jaccard),
            containment: Some(edge.containment),
        },
        None => FamilyMember {
            path: docs[i].path.clone(),
            exact_hash: docs[i].exact_hash.clone(),
            relation: Relation::Near,
            joined_with: None,
            near_score: None,
            jaccard: None,
            containment: None,
        },
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
    use crate::near::{
        containment, exact_jaccard, score, DEFAULT_NEAR_THRESHOLD, MINHASH_CANDIDATE_THRESHOLD,
    };
    use std::path::Path;

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

    /// Deterministic pseudo-text with no shared template vocabulary.
    /// Two different seeds share almost no character 5-grams, which lets a
    /// test dial containment by mixing a shared block with unique bodies.
    fn noise(seed: u64, lines: usize) -> String {
        const SYLLABLES: &[char] = &[
            '가', '나', '다', '라', '마', '바', '사', '아', '자', '차', '카', '타', '파', '하',
            '거', '너', '더', '러', '머', '버', '서', '어', '저', '처', '커', '터', '퍼', '허',
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

    /// `n` documents that share one boilerplate block and differ only in a
    /// short unique body: the corporate-template shape from issue #3.
    fn template_siblings(n: usize, shared_lines: usize, body_lines: usize) -> Vec<ScannedDoc> {
        let boilerplate = noise(1, shared_lines);
        (0..n)
            .map(|i| {
                doc(
                    &format!("양식_{i}.docx"),
                    &format!("{boilerplate}{}", noise(100 + i as u64, body_lines)),
                )
            })
            .collect()
    }

    #[test]
    fn contains_threshold_is_separate_from_near_threshold() {
        let docs = template_siblings(2, 93, 7);
        let observed = containment(&docs[0].shingles, &docs[1].shingles)
            .max(containment(&docs[1].shingles, &docs[0].shingles));
        assert!(
            (DEFAULT_NEAR_THRESHOLD..DEFAULT_CONTAINS_THRESHOLD).contains(&observed),
            "fixture must land between the two thresholds, got {observed}"
        );

        assert!(
            cluster(&docs, &ClusterConfig::default()).is_empty(),
            "shared-template siblings must not pass the stricter contains gate"
        );

        let permissive = ClusterConfig {
            contains_threshold: DEFAULT_NEAR_THRESHOLD,
            ..ClusterConfig::default()
        };
        assert_eq!(
            cluster(&docs, &permissive).len(),
            1,
            "the same pair merges once contains uses the near threshold"
        );
    }

    #[test]
    fn template_siblings_do_not_form_one_family() {
        // Issue #3: eight distinct API manuals on one corporate template.
        let docs = template_siblings(8, 93, 7);
        let families = cluster(&docs, &ClusterConfig::default());
        assert!(
            families.is_empty(),
            "expected no family, got {:?}",
            families.iter().map(|f| f.members.len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn plain_addition_still_joins_by_contains() {
        // Draft wholly inside the final: containment 1.0, the case contains
        // exists for. A stricter contains threshold must not break it.
        let draft = noise(7, 40);
        let final_doc = format!("{draft}{}", noise(8, 40));
        let docs = vec![doc("초안.docx", &draft), doc("최종.docx", &final_doc)];
        let families = cluster(&docs, &ClusterConfig::default());
        assert_eq!(families.len(), 1);
        assert!(families[0]
            .members
            .iter()
            .any(|m| m.relation == Relation::Contains));
    }

    #[test]
    fn family_records_the_edge_that_joined_each_pair() {
        let draft = noise(7, 40);
        let final_doc = format!("{draft}{}", noise(8, 40));
        let docs = vec![doc("초안.docx", &draft), doc("최종.docx", &final_doc)];
        let families = cluster(&docs, &ClusterConfig::default());

        let edges = &families[0].edges;
        assert_eq!(edges.len(), 1, "one verified pair means one edge");
        let edge = &edges[0];
        assert_eq!(edge.relation, Relation::Contains);
        assert_eq!(edge.a, PathBuf::from("최종.docx"), "a is the container");
        assert_eq!(edge.b, PathBuf::from("초안.docx"), "b is the contained");
        assert!(edge.containment >= DEFAULT_CONTAINS_THRESHOLD);
        assert!(edge.jaccard < DEFAULT_NEAR_THRESHOLD);
    }

    #[test]
    fn member_reports_its_real_evidence_not_a_near_fallback() {
        // draft ⊂ final joins by contains; an edit of the draft joins by
        // near. No member may claim `near` without a near-grade Jaccard.
        let draft = noise(7, 40);
        let final_doc = format!("{draft}{}", noise(8, 40));
        let edited = draft.replacen('가', "나", 3);
        let docs = vec![
            doc("최종.docx", &final_doc),
            doc("초안.docx", &draft),
            doc("초안_수정.docx", &edited),
        ];
        let families = cluster(&docs, &ClusterConfig::default());
        assert_eq!(families.len(), 1);
        assert_eq!(families[0].members.len(), 3);

        for member in &families[0].members {
            let joined = member
                .joined_with
                .as_ref()
                .expect("every member of a family has evidence");
            assert_ne!(joined, &member.path);
            if member.relation == Relation::Near {
                assert!(
                    member.jaccard.unwrap() >= DEFAULT_NEAR_THRESHOLD,
                    "{} claims near at jaccard {:?}",
                    member.path.display(),
                    member.jaccard
                );
            }
        }

        // 최종.docx only ever matched by containment, so that is what it
        // must report -- with the counterpart named.
        let container = families[0]
            .members
            .iter()
            .find(|m| m.path == Path::new("최종.docx"))
            .unwrap();
        assert_eq!(container.relation, Relation::Contains);
        assert!(container
            .joined_with
            .as_deref()
            .is_some_and(|p| p.to_string_lossy().starts_with("초안")));
        assert!(container.containment.unwrap() >= DEFAULT_CONTAINS_THRESHOLD);
    }

    #[test]
    fn family_label_is_mixed_when_members_join_differently() {
        let draft = noise(7, 40);
        let final_doc = format!("{draft}{}", noise(8, 40));
        let edited = draft.replacen('가', "나", 3);
        let docs = vec![
            doc("최종.docx", &final_doc),
            doc("초안.docx", &draft),
            doc("초안_수정.docx", &edited),
        ];
        let families = cluster(&docs, &ClusterConfig::default());
        assert_eq!(families[0].label(), FamilyLabel::Mixed);
    }

    #[test]
    fn exact_copies_are_recorded_as_exact_edges() {
        let t = proposal();
        let docs = vec![doc("a.docx", &t), doc("b 복사본.docx", &t)];
        let families = cluster(&docs, &ClusterConfig::default());
        assert_eq!(families[0].label(), FamilyLabel::Exact);
        assert_eq!(families[0].edges.len(), 1);
        assert_eq!(families[0].edges[0].relation, Relation::Exact);
        assert_eq!(families[0].edges[0].jaccard, 1.0);
    }

    #[test]
    fn clustering_is_deterministic_under_parallel_verification() {
        let mut docs = template_siblings(6, 60, 40);
        docs.extend([
            doc("초안.docx", &noise(7, 40)),
            doc("최종.docx", &format!("{}{}", noise(7, 40), noise(8, 40))),
        ]);
        let first = cluster(&docs, &ClusterConfig::default());
        for _ in 0..8 {
            let again = cluster(&docs, &ClusterConfig::default());
            assert_eq!(first.len(), again.len());
            for (a, b) in first.iter().zip(&again) {
                assert_eq!(a.id, b.id);
                let pa: Vec<_> = a.members.iter().map(|m| &m.path).collect();
                let pb: Vec<_> = b.members.iter().map(|m| &m.path).collect();
                assert_eq!(pa, pb);
                assert_eq!(a.edges.len(), b.edges.len());
                assert_eq!(a.label(), b.label());
            }
        }
    }

    #[test]
    fn exact_group() {
        let t = proposal();
        let docs = vec![doc("a.docx", &t), doc("b 복사본.docx", &t)];
        let fams = cluster(&docs, &ClusterConfig::default());
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
        let fams = cluster(&docs, &ClusterConfig::default());
        assert_eq!(fams.len(), 1);
        let near = fams[0]
            .members
            .iter()
            .find(|m| m.relation == Relation::Near)
            .expect("one member should join by near");
        assert!(near.near_score.unwrap() >= 0.85);
    }

    #[test]
    fn minhash_candidate_below_final_threshold_uses_exact_jaccard() {
        let base = (0..240)
            .map(|i| format!("문서고유문장{i:04}"))
            .collect::<Vec<_>>();
        let base_text = base.join(" ");
        let base_doc = doc("draft.hwp", &base_text);
        let variant_text = (1..80)
            .find_map(|changed| {
                let mut variant = base.clone();
                for (i, word) in variant.iter_mut().take(changed).enumerate() {
                    *word = format!("수정문장{i:04}");
                }
                let text = variant.join(" ");
                let candidate = doc("candidate.hwp", &text);
                let estimate = score(&base_doc.sig, &candidate.sig);
                let jaccard = exact_jaccard(&base_doc.shingles, &candidate.shingles);
                ((MINHASH_CANDIDATE_THRESHOLD..DEFAULT_NEAR_THRESHOLD).contains(&estimate)
                    && jaccard >= DEFAULT_NEAR_THRESHOLD)
                    .then_some(text)
            })
            .expect("fixture should expose a MinHash false negative near the threshold");

        let docs = vec![base_doc, doc("final.hwp", &variant_text)];
        let families = cluster(&docs, &ClusterConfig::default());
        assert_eq!(families.len(), 1);
        let near = families[0]
            .members
            .iter()
            .find(|member| member.path == Path::new("final.hwp"))
            .unwrap();
        assert_eq!(near.relation, Relation::Near);
        assert!(near.near_score.unwrap() < DEFAULT_NEAR_THRESHOLD);
        assert!(near.jaccard.unwrap() >= DEFAULT_NEAR_THRESHOLD);
    }

    #[test]
    fn minhash_candidate_above_final_threshold_needs_exact_jaccard() {
        let base_text = paragraphs("기준", 30);
        let extended_text = format!("{base_text}\n{}", paragraphs("부록추가내용", 40));
        let base_doc = doc("base.hwp", &base_text);
        let mut candidate = doc("different.hwp", &extended_text);
        assert!(exact_jaccard(&base_doc.shingles, &candidate.shingles) < DEFAULT_NEAR_THRESHOLD);
        assert!(containment(&candidate.shingles, &base_doc.shingles) >= DEFAULT_NEAR_THRESHOLD);
        // Simulate a MinHash overestimate deterministically: final near
        // judgment must still use the complete shingle sets.
        candidate.sig = base_doc.sig.clone();
        let docs = vec![base_doc, candidate];
        let families = cluster(&docs, &ClusterConfig::default());
        assert_eq!(families.len(), 1);
        let member = families[0]
            .members
            .iter()
            .find(|member| member.path == Path::new("different.hwp"))
            .unwrap();
        assert_eq!(member.relation, Relation::Contains);
        assert!(member.near_score.unwrap() >= DEFAULT_NEAR_THRESHOLD);
        assert!(member.jaccard.unwrap() < DEFAULT_NEAR_THRESHOLD);
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
        let fams = cluster(&docs, &ClusterConfig::default());
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
        assert!(cluster(&docs, &ClusterConfig::default()).is_empty());
    }

    #[test]
    fn byte_identical_empty_text_clusters_as_exact() {
        let empty = near_sig("");
        let shingles = shingles("");
        let docs = vec![
            ScannedDoc::from_precomputed_with_byte_hash(
                PathBuf::from("scan1.pdf"),
                crate::exact_hash_hex(""),
                crate::byte_hash_hex(b"pdf"),
                empty.clone(),
                shingles.clone(),
            ),
            ScannedDoc::from_precomputed_with_byte_hash(
                PathBuf::from("scan2.pdf"),
                crate::exact_hash_hex(""),
                crate::byte_hash_hex(b"pdf"),
                empty,
                shingles,
            ),
        ];
        let fams = cluster(&docs, &ClusterConfig::default());
        assert_eq!(fams.len(), 1);
        assert!(fams[0]
            .members
            .iter()
            .all(|member| member.relation == Relation::Exact));
    }

    #[test]
    fn byte_different_empty_text_does_not_cluster() {
        let docs = vec![
            ScannedDoc::from_precomputed_with_byte_hash(
                PathBuf::from("scan1.pdf"),
                crate::exact_hash_hex(""),
                crate::byte_hash_hex(b"pdf-a"),
                near_sig(""),
                shingles(""),
            ),
            ScannedDoc::from_precomputed_with_byte_hash(
                PathBuf::from("scan2.pdf"),
                crate::exact_hash_hex(""),
                crate::byte_hash_hex(b"pdf-b"),
                near_sig(""),
                shingles(""),
            ),
        ];
        assert!(cluster(&docs, &ClusterConfig::default()).is_empty());
    }

    #[test]
    fn transitive_near_merges() {
        // Long enough that a one-token edit stays above the near threshold:
        // this test is about transitivity, not about a near-miss pair being
        // rescued by containment.
        let a = paragraphs("버전", 40);
        let b = a.replace("1번째", "첫번째");
        let c = b.replace("10번째", "열th");
        let docs = vec![doc("v1.docx", &a), doc("v2.docx", &b), doc("v3.docx", &c)];
        let fams = cluster(&docs, &ClusterConfig::default());
        assert_eq!(fams.len(), 1);
        assert_eq!(fams[0].members.len(), 3);
    }

    #[test]
    fn a_shared_fragment_does_not_chain_unrelated_documents() {
        // A short quoted fragment is fully contained in every long document
        // that carries it. Containment alone would union them all into one
        // component; the Jaccard floor keeps them apart.
        let fragment = noise(50, 4);
        let docs = vec![
            doc("조각.txt", &fragment),
            doc(
                "보고서_A.pdf",
                &format!("{}{fragment}{}", noise(11, 60), noise(12, 60)),
            ),
            doc(
                "보고서_B.pdf",
                &format!("{}{fragment}{}", noise(21, 60), noise(22, 60)),
            ),
        ];
        let overlap = crate::near::overlap(&docs[0].shingles, &docs[1].shingles);
        assert!(
            overlap.max_containment() >= DEFAULT_CONTAINS_THRESHOLD,
            "fixture must be genuinely contained, got {}",
            overlap.max_containment()
        );
        assert!(
            overlap.jaccard < DEFAULT_CONTAINS_MIN_JACCARD,
            "fixture must be fragment-scale, got {}",
            overlap.jaccard
        );

        assert!(
            cluster(&docs, &ClusterConfig::default()).is_empty(),
            "a fragment must not bridge unrelated documents"
        );
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
        let fams = cluster(&docs, &ClusterConfig::default());
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
        let fams = cluster(&docs, &ClusterConfig::default());
        assert_eq!(fams.len(), 2);
        let mut sizes: Vec<usize> = fams.iter().map(|f| f.members.len()).collect();
        sizes.sort();
        assert_eq!(sizes, vec![2, 2]);
        // IDs are stable and unique.
        assert_ne!(fams[0].id, fams[1].id);
    }
}
