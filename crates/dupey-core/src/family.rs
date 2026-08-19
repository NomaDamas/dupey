//! Cluster documents into families (exact / near / contains).
//!
//! Clustering and LSH indexing land in a later slice. This module only
//! defines the public shapes so CLI and docs can talk about one contract.

use std::path::PathBuf;

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
    pub near_score: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Family {
    pub id: u32,
    pub members: Vec<FamilyMember>,
}
