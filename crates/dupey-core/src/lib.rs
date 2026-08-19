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
pub use exact::{exact_hash, exact_hash_hex};
pub use extract::{extract, CanonicalText, Format};
pub use family::{Family, FamilyMember, Relation};
pub use near::{near_sig, score, NearSignature, DEFAULT_NEAR_THRESHOLD, NUM_PERM};
pub use rank::{FamilyRanking, RankSignal, RankedMember};
