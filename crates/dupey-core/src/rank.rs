//! Explainable latest/canonical ranking. Not a verdict.
//!
//! Signals (planned): internal modified time, content containment,
//! filename tokens (`v3`, `최종`, `복사본`), OOXML revision, weak length.

use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RankSignal {
    pub name: String,
    pub detail: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RankedMember {
    pub path: PathBuf,
    pub rank: u32,
    pub confidence: f64,
    pub reasons: Vec<RankSignal>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FamilyRanking {
    pub family_id: u32,
    pub ranked: Vec<RankedMember>,
}
