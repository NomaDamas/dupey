use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use dupey_core::{
    cluster, containment, exact_hash_hex, extract, near_sig, rank, CanonicalText,
    Format, MemberSignals, ScannedDoc, DEFAULT_NEAR_THRESHOLD,
};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "dupey",
    about = "Office document family detector: exact hash, MinHash near-dup, explainable latest candidate",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Extract canonical text and print exact hash + MinHash width
    Fingerprint { path: PathBuf },
    /// Compare two files (exact equality + MinHash score)
    Compare { a: PathBuf, b: PathBuf },
    /// Scan a directory: extract, cluster into families, rank latest candidates
    Scan {
        dir: PathBuf,
        /// Emit the public JSON contract instead of a human summary
        #[arg(long)]
        json: bool,
        /// Family threshold for near/contains (Jaccard estimate)
        #[arg(long, default_value_t = DEFAULT_NEAR_THRESHOLD)]
        threshold: f64,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Fingerprint { path } => fingerprint(&path),
        Command::Compare { a, b } => compare(&a, &b),
        Command::Scan {
            dir,
            json,
            threshold,
        } => scan(&dir, json, threshold),
    }
}

fn fingerprint(path: &Path) -> Result<()> {
    let canon = extract(path).with_context(|| format!("extract {}", path.display()))?;
    let sig = near_sig(&canon.text);
    println!("path\t{}", canon.path.display());
    println!("format\t{:?}", canon.format);
    println!("chars\t{}", canon.text.chars().count());
    println!("exact\t{}", exact_hash_hex(&canon.text));
    println!("minhash_width\t{}", sig.values.len());
    match &canon.meta.modified {
        Some(t) => println!("modified\t{t}"),
        None => println!("modified\t-"),
    }
    match canon.meta.revision {
        Some(r) => println!("revision\t{r}"),
        None => println!("revision\t-"),
    }
    Ok(())
}

fn compare(a: &Path, b: &Path) -> Result<()> {
    let ca = extract(a).with_context(|| format!("extract {}", a.display()))?;
    let cb = extract(b).with_context(|| format!("extract {}", b.display()))?;
    let ha = exact_hash_hex(&ca.text);
    let hb = exact_hash_hex(&cb.text);
    let near = dupey_core::score(&near_sig(&ca.text), &near_sig(&cb.text));
    println!("a\t{}", a.display());
    println!("b\t{}", b.display());
    println!("exact_equal\t{}", ha == hb);
    println!("near_score\t{near:.4}");
    Ok(())
}

// ---------- scan: the public contract ----------

#[derive(Serialize)]
struct FileEntry {
    path: String,
    format: Format,
    content_hash: String,
    /// MinHash signature (128-wide), null for empty/scanned content.
    fuzzy: Option<Vec<u64>>,
    signals: FileSignals,
}

#[derive(Serialize)]
struct FileSignals {
    chars: usize,
    modified: Option<String>,
    revision: Option<u32>,
    fs_mtime: Option<String>,
}

#[derive(Serialize)]
struct ErrorEntry {
    path: String,
    error: String,
}

#[derive(Serialize)]
struct FamilyOut {
    id: u32,
    relation: dupey_core::Relation,
    files: Vec<String>,
    members: Vec<dupey_core::FamilyMember>,
    pick: PickOut,
}

#[derive(Serialize)]
struct PickOut {
    ranked: Vec<dupey_core::RankedMember>,
    reasons: Vec<dupey_core::RankSignal>,
    confidence: f64,
}

#[derive(Serialize)]
struct ScanOut {
    dir: String,
    threshold: f64,
    files: Vec<FileEntry>,
    families: Vec<FamilyOut>,
    errors: Vec<ErrorEntry>,
}

fn scan(dir: &Path, json: bool, threshold: f64) -> Result<()> {
    let t0 = std::time::Instant::now();
    let mut files: Vec<FileEntry> = Vec::new();
    let mut docs: Vec<ScannedDoc> = Vec::new();
    let mut metas: Vec<(CanonicalText, Option<jiff::Timestamp>)> = Vec::new();
    let mut errors: Vec<ErrorEntry> = Vec::new();

    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() && Format::from_path(entry.path()).is_some() {
            paths.push(entry.path().to_path_buf());
        }
    }
    paths.sort();

    for path in paths {
        match extract(&path) {
            Ok(canon) => {
                let fs_mtime = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|d| jiff::SignedDuration::try_from(d).ok())
                    .and_then(|d| jiff::Timestamp::from_duration(d).ok());
                let sig = near_sig(&canon.text);
                let fuzzy = (!canon.text.is_empty()).then(|| sig.values.clone());
                files.push(FileEntry {
                    path: canon.path.display().to_string(),
                    format: canon.format,
                    content_hash: exact_hash_hex(&canon.text),
                    fuzzy,
                    signals: FileSignals {
                        chars: canon.text.chars().count(),
                        modified: canon.meta.modified.map(|t| t.to_string()),
                        revision: canon.meta.revision,
                        fs_mtime: fs_mtime.map(|t| t.to_string()),
                    },
                });
                docs.push(ScannedDoc::from_text(canon.path.clone(), &canon.text));
                metas.push((canon, fs_mtime));
            }
            Err(e) => errors.push(ErrorEntry {
                path: path.display().to_string(),
                error: e.to_string(),
            }),
        }
    }

    if std::env::var_os("DUPEY_TIMING").is_some() {
        eprintln!("extract+sig: {:?}", t0.elapsed());
    }
    let t_cluster = std::time::Instant::now();
    let families = cluster(&docs, threshold);
    if std::env::var_os("DUPEY_TIMING").is_some() {
        eprintln!("cluster: {:?}", t_cluster.elapsed());
    }
    // Shingle sets are already computed in `docs`; reuse them instead of
    // re-shingling per member pair (that made rank O(m^2) re-extraction).
    let doc_index: std::collections::HashMap<&PathBuf, usize> = docs
        .iter()
        .enumerate()
        .map(|(i, d)| (&d.path, i))
        .collect();
    let mut family_out = Vec::new();
    for fam in &families {
        let signals: Vec<MemberSignals> = fam
            .members
            .iter()
            .map(|m| {
                let (canon, fs_mtime) = metas
                    .iter()
                    .find(|(c, _)| c.path == m.path)
                    .map(|(c, t)| (c.clone(), *t))
                    .expect("family member must come from scanned docs");
                let my_shingles = &docs[doc_index[&m.path]].shingles;
                let others: Vec<&Vec<u64>> = fam
                    .members
                    .iter()
                    .filter(|o| o.path != m.path)
                    .filter_map(|o| doc_index.get(&o.path))
                    .map(|&oi| &docs[oi].shingles)
                    .collect();
                let contains_others = !others.is_empty()
                    && others.iter().all(|os| {
                        !os.is_empty() && containment(my_shingles, os) >= threshold
                    });
                let contained_by_other = others.iter().any(|os| {
                    !my_shingles.is_empty() && containment(os, my_shingles) >= threshold
                });
                MemberSignals {
                    path: m.path.clone(),
                    internal_modified: canon.meta.modified,
                    fs_mtime,
                    revision: canon.meta.revision,
                    text_len: canon.text.chars().count(),
                    contains_others,
                    contained_by_other,
                }
            })
            .collect();
        let ranking = rank(fam.id, &signals);
        let relation = if fam
            .members
            .iter()
            .all(|m| m.relation == dupey_core::Relation::Exact)
        {
            dupey_core::Relation::Exact
        } else if fam
            .members
            .iter()
            .any(|m| m.relation == dupey_core::Relation::Near)
        {
            dupey_core::Relation::Near
        } else {
            dupey_core::Relation::Contains
        };
        family_out.push(FamilyOut {
            id: fam.id,
            relation,
            files: fam
                .members
                .iter()
                .map(|m| m.path.display().to_string())
                .collect(),
            members: fam.members.clone(),
            pick: PickOut {
                reasons: ranking
                    .ranked
                    .first()
                    .map(|r| r.reasons.clone())
                    .unwrap_or_default(),
                ranked: ranking.ranked,
                confidence: ranking.confidence,
            },
        });
    }

    let out = ScanOut {
        dir: dir.display().to_string(),
        threshold,
        files,
        families: family_out,
        errors,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("scanned {}\t{} files", out.dir, out.files.len());
        for e in &out.errors {
            println!("error\t{}\t{}", e.path, e.error);
        }
        for f in &out.families {
            println!(
                "family #{}\t{:?}\t{} files\tconfidence {:.2}",
                f.id,
                f.relation,
                f.files.len(),
                f.pick.confidence
            );
            if let Some(top) = f.pick.ranked.first() {
                println!("  pick\t{}", top.path.display());
                for r in &top.reasons {
                    println!("    - {}: {}", r.name, r.detail);
                }
            }
        }
        if out.families.is_empty() {
            println!("no families (all files unique)");
        }
    }
    Ok(())
}
