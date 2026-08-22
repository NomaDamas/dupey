//! dupey-core: office document families, not a generic file cleaner.
//!
//! Pipeline: `extract` → `exact_hash` + `near_sig` → family → ranked latest
//! candidate. Semantic embeddings are out of scope.

pub mod error;
pub mod exact;
pub mod extract;
pub mod family;
pub mod near;
pub mod rank;

pub use error::{Error, Result};
pub use exact::{byte_hash, byte_hash_hex, exact_hash, exact_hash_hex};
pub use extract::{extract, CanonicalText, DocMeta, Format};
pub use family::{cluster, Family, FamilyMember, Relation, ScannedDoc};
pub use near::{
    containment, containment_at_least, exact_jaccard, near_sig, score, shingles, NearSignature,
    DEFAULT_NEAR_THRESHOLD, MINHASH_CANDIDATE_THRESHOLD, NUM_PERM,
};
pub use rank::{rank, FamilyRanking, MemberSignals, RankSignal, RankedMember};
