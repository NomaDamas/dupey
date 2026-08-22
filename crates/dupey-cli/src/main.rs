mod skip;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use dupey_core::{
    byte_hash_hex, cluster_with_config, containment, exact_hash_hex, extract, near_sig, rank,
    shingles, CanonicalText, ClusterConfig, Format, MemberSignals, NearSignature, ScannedDoc,
    DEFAULT_CONTAINS_MIN_JACCARD, DEFAULT_CONTAINS_THRESHOLD, DEFAULT_NEAR_THRESHOLD,
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
        /// Exact shingle Jaccard required to join two files as `near`
        #[arg(long, default_value_t = DEFAULT_NEAR_THRESHOLD)]
        threshold: f64,
        /// Shingle containment required to join two files as `contains`.
        /// Separate from --threshold, and stricter by default: containment
        /// divides by the smaller file, so a shared template alone can fill
        /// 90% of a short document.
        #[arg(long, default_value_t = DEFAULT_CONTAINS_THRESHOLD)]
        contains_threshold: f64,
        /// Jaccard floor a `contains` pair must also clear, so a short
        /// fragment quoted by many long files cannot chain them together
        #[arg(long, default_value_t = DEFAULT_CONTAINS_MIN_JACCARD)]
        contains_min_jaccard: f64,
        /// Extra directory names to skip (folder name, not a path). Repeatable.
        /// Merged with builtins: node_modules, .git, target, dist, build, ...
        #[arg(long = "exclude-dir", value_name = "NAME", action = clap::ArgAction::Append)]
        exclude_dir: Vec<String>,
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
            contains_threshold,
            contains_min_jaccard,
            exclude_dir,
        } => scan(
            &dir,
            json,
            ClusterConfig {
                near_threshold: threshold,
                contains_threshold,
                contains_min_jaccard,
            },
            &exclude_dir,
        ),
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
    let sa = dupey_core::shingles(&ca.text);
    let sb = dupey_core::shingles(&cb.text);
    let near = dupey_core::score(&near_sig(&ca.text), &near_sig(&cb.text));
    let jaccard = dupey_core::exact_jaccard(&sa, &sb);
    let a_in_b = containment(&sb, &sa);
    let b_in_a = containment(&sa, &sb);
    println!("a\t{}", a.display());
    println!("b\t{}", b.display());
    println!("exact_equal\t{}", ha == hb);
    println!("near_score\t{near:.4}");
    println!("jaccard\t{jaccard:.4}");
    println!("containment_a_in_b\t{a_in_b:.4}");
    println!("containment_b_in_a\t{b_in_a:.4}");
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
    /// `mixed` when members joined by different relations: the family label
    /// is never collapsed into whichever relation wins a priority list.
    relation: dupey_core::FamilyLabel,
    files: Vec<String>,
    members: Vec<dupey_core::FamilyMember>,
    /// Every verified pair behind this family, so a caller can see which
    /// comparison produced the merge instead of trusting the label.
    edges: Vec<dupey_core::FamilyEdge>,
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
    /// Jaccard threshold for `near`.
    threshold: f64,
    /// Containment threshold for `contains`.
    contains_threshold: f64,
    /// Jaccard floor for `contains`.
    contains_min_jaccard: f64,
    files: Vec<FileEntry>,
    families: Vec<FamilyOut>,
    errors: Vec<ErrorEntry>,
}

struct PreparedScan {
    canon: CanonicalText,
    fs_mtime: Option<jiff::Timestamp>,
    exact_hash: String,
    byte_hash: Option<String>,
    sig: NearSignature,
    shingles: Vec<u64>,
    chars: usize,
}

fn scan(dir: &Path, json: bool, config: ClusterConfig, extra_exclude: &[String]) -> Result<()> {
    anyhow::ensure!(dir.exists(), "scan path does not exist: {}", dir.display());

    let t0 = std::time::Instant::now();
    let mut files: Vec<FileEntry> = Vec::new();
    let mut docs: Vec<ScannedDoc> = Vec::new();
    let mut metas: Vec<(CanonicalText, Option<jiff::Timestamp>)> = Vec::new();
    let mut errors: Vec<ErrorEntry> = Vec::new();

    let skip = skip::skip_set(extra_exclude);
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| {
            !skip::should_skip_dir(e.file_name(), e.file_type().is_dir(), e.depth(), &skip)
        })
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() && Format::from_path(entry.path()).is_some() {
            paths.push(entry.path().to_path_buf());
        }
    }
    paths.sort();

    // Extract + hash + signature is the scan bottleneck; it is pure I/O
    // and CPU per file, so it parallelizes cleanly. Order is preserved.
    use rayon::prelude::*;
    let results: Vec<dupey_core::Result<PreparedScan>> = paths
        .par_iter()
        .map(|path| {
            let canon = extract(path)?;
            let byte_hash = if canon.text.is_empty() {
                Some(byte_hash_hex(&std::fs::read(path).map_err(|source| {
                    dupey_core::Error::Io {
                        path: path.to_path_buf(),
                        source,
                    }
                })?))
            } else {
                None
            };
            let fs_mtime = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| jiff::SignedDuration::try_from(d).ok())
                .and_then(|d| jiff::Timestamp::from_duration(d).ok());
            let exact_hash = exact_hash_hex(&canon.text);
            let sig = near_sig(&canon.text);
            let shingles = shingles(&canon.text);
            let chars = canon.text.chars().count();
            Ok(PreparedScan {
                canon,
                fs_mtime,
                exact_hash,
                byte_hash,
                sig,
                shingles,
                chars,
            })
        })
        .collect();
    for (path, result) in paths.iter().zip(results) {
        match result {
            Ok(prepared) => {
                let PreparedScan {
                    canon,
                    fs_mtime,
                    exact_hash,
                    byte_hash,
                    sig,
                    shingles,
                    chars,
                } = prepared;
                let fuzzy = (!canon.text.is_empty()).then(|| sig.values.clone());
                let content_hash = byte_hash.clone().unwrap_or_else(|| exact_hash.clone());
                files.push(FileEntry {
                    path: canon.path.display().to_string(),
                    format: canon.format,
                    content_hash,
                    fuzzy,
                    signals: FileSignals {
                        chars,
                        modified: canon.meta.modified.map(|t| t.to_string()),
                        revision: canon.meta.revision,
                        fs_mtime: fs_mtime.map(|t| t.to_string()),
                    },
                });
                docs.push(match byte_hash {
                    Some(byte_hash) => ScannedDoc::from_precomputed_with_byte_hash(
                        canon.path.clone(),
                        byte_hash.clone(),
                        byte_hash,
                        sig,
                        shingles,
                    ),
                    None => {
                        ScannedDoc::from_precomputed(canon.path.clone(), exact_hash, sig, shingles)
                    }
                });
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
    let families = cluster_with_config(&docs, &config);
    if std::env::var_os("DUPEY_TIMING").is_some() {
        eprintln!("cluster: {:?}", t_cluster.elapsed());
    }
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
                MemberSignals {
                    path: m.path.clone(),
                    internal_modified: canon.meta.modified,
                    fs_mtime,
                }
            })
            .collect();
        let ranking = rank(fam.id, &signals);
        family_out.push(FamilyOut {
            id: fam.id,
            relation: fam.label(),
            files: fam
                .members
                .iter()
                .map(|m| m.path.display().to_string())
                .collect(),
            members: fam.members.clone(),
            edges: fam.edges.clone(),
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
        threshold: config.near_threshold,
        contains_threshold: config.contains_threshold,
        contains_min_jaccard: config.contains_min_jaccard,
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
            for m in &f.members {
                match (&m.joined_with, m.jaccard, m.containment) {
                    (Some(other), Some(jaccard), Some(containment)) => println!(
                        "  {}\t{:?}\tjaccard {:.3}\tcontainment {:.3}\tvs {}",
                        m.path.display(),
                        m.relation,
                        jaccard,
                        containment,
                        other.display()
                    ),
                    _ => println!("  {}\t{:?}", m.path.display(), m.relation),
                }
            }
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
