mod budget;
mod skip;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use dupey_core::{
    byte_hash_hex, cluster_with_config, containment, exact_hash_hex, extract, extract_size_hint,
    near_sig, rank, shingles, CanonicalText, ClusterConfig, DocMeta, Format, MemberSignals,
    NearSignature, ScannedDoc, ShingleStore, Shingles, DEFAULT_CONTAINS_MIN_JACCARD,
    DEFAULT_CONTAINS_THRESHOLD, DEFAULT_NEAR_THRESHOLD,
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
    println!("exact\t{}", comparison_hash(&canon)?);
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

/// Empty extraction proves no content equality; use the same original-byte
/// fallback as scan instead of comparing two hashes of the empty string.
fn comparison_hash(canon: &CanonicalText) -> Result<String> {
    if canon.text.trim().is_empty() {
        let bytes =
            std::fs::read(&canon.path).with_context(|| format!("read {}", canon.path.display()))?;
        Ok(byte_hash_hex(&bytes))
    } else {
        Ok(exact_hash_hex(&canon.text))
    }
}

fn compare(a: &Path, b: &Path) -> Result<()> {
    let ca = extract(a).with_context(|| format!("extract {}", a.display()))?;
    let cb = extract(b).with_context(|| format!("extract {}", b.display()))?;
    let ha = comparison_hash(&ca)?;
    let hb = comparison_hash(&cb)?;
    let sa = dupey_core::shingles(&ca.text);
    let sb = dupey_core::shingles(&cb.text);
    let comparable = !sa.is_empty() && !sb.is_empty();
    let near = if comparable {
        dupey_core::score(&near_sig(&ca.text), &near_sig(&cb.text))
    } else {
        0.0
    };
    let jaccard = if comparable {
        dupey_core::exact_jaccard(&sa, &sb)
    } else {
        0.0
    };
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
struct FileEntry<'a> {
    path: String,
    format: Format,
    content_hash: String,
    /// MinHash signature (128-wide), null for empty/scanned content.
    /// Borrowed from the clustered document instead of kept twice.
    fuzzy: Option<&'a [u64]>,
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
struct ScanOut<'a> {
    dir: String,
    /// Jaccard threshold for `near`.
    threshold: f64,
    /// Containment threshold for `contains`.
    contains_threshold: f64,
    /// Jaccard floor for `contains`.
    contains_min_jaccard: f64,
    files: Vec<FileEntry<'a>>,
    families: Vec<FamilyOut>,
    errors: Vec<ErrorEntry>,
}

/// Everything the scan keeps per file after extraction. The extracted text
/// itself is dropped as soon as its hashes and shingles exist.
struct DocInfo {
    path: PathBuf,
    format: Format,
    meta: DocMeta,
}

struct PreparedScan {
    info: DocInfo,
    fs_mtime: Option<jiff::Timestamp>,
    exact_hash: String,
    byte_hash: Option<String>,
    has_text: bool,
    sig: NearSignature,
    shingles: Shingles,
    chars: usize,
}

/// Why one file could not be prepared.
enum PrepareError {
    /// The file itself: reported in `errors[]` while the scan goes on.
    File(dupey_core::Error),
    /// The temporary shingle store: the whole scan fails.
    Store(std::io::Error),
}

impl From<dupey_core::Error> for PrepareError {
    fn from(e: dupey_core::Error) -> Self {
        Self::File(e)
    }
}

/// Files whose extracted text may exceed this many bytes (the file size, or
/// the declared uncompressed size for ZIP formats) leave the rayon pool for
/// a few dedicated threads under a byte budget. Extract + hash transients
/// (text copies, MinHash input hashes) run to ~12x the text size, so several
/// such files in flight at once dominate peak memory.
const LARGE_FILE_BYTES: u64 = 8 << 20;
/// Estimated transient bytes per byte of size hint.
const TRANSIENT_BYTES_PER_HINT_BYTE: u64 = 12;
/// Dedicated threads for large files.
const LARGE_FILE_THREADS: usize = 4;
/// Estimated transient bytes the large-file threads may have in flight. A
/// file estimated above it runs alone.
const LARGE_FILE_BUDGET: u64 = 1 << 30;

/// Extract, hash, and fingerprint one file. The extracted text is dropped
/// before returning; only hashes, metadata, and the shingle prefix stay in
/// memory, the rest of the shingle set goes to `store`.
fn prepare(path: &Path, store: &Arc<ShingleStore>) -> Result<PreparedScan, PrepareError> {
    let canon = extract(path)?;
    let byte_hash = if canon.text.trim().is_empty() {
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
    let shingles = Shingles::stored(shingles(&canon.text), store).map_err(PrepareError::Store)?;
    let chars = canon.text.chars().count();
    // Whitespace-only text has no shingles and nothing to compare fuzzily.
    let has_text = !shingles.is_empty();
    Ok(PreparedScan {
        info: DocInfo {
            path: canon.path,
            format: canon.format,
            meta: canon.meta,
        },
        fs_mtime,
        exact_hash,
        byte_hash,
        has_text,
        sig,
        shingles,
        chars,
    })
}

/// Prepare every path, keeping results in `paths` order. Small files run on
/// the rayon pool; large ones are handed, as they are found, to dedicated
/// threads that only start one when its estimated transient fits the
/// budget. Rayon workers never block, so extractors that use rayon
/// internally cannot deadlock.
fn prepare_all(
    paths: &[PathBuf],
    store: &Arc<ShingleStore>,
) -> Vec<Result<PreparedScan, PrepareError>> {
    use rayon::prelude::*;
    let (large_tx, large_rx) = std::sync::mpsc::channel::<(usize, u64)>();
    let large_rx = std::sync::Mutex::new(large_rx);
    let budget = budget::MemoryBudget::new(LARGE_FILE_BUDGET);
    std::thread::scope(|s| {
        let large_threads: Vec<_> = (0..LARGE_FILE_THREADS)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let next = large_rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        let Ok((i, cost)) = next else { break };
                        let _permit = budget.acquire(cost);
                        done.push((i, prepare(&paths[i], store)));
                    }
                    done
                })
            })
            .collect();
        let mut results: Vec<Option<Result<PreparedScan, PrepareError>>> = paths
            .par_iter()
            .enumerate()
            .map_with(large_tx, |large_tx, (i, path)| {
                let hint = extract_size_hint(path);
                if hint >= LARGE_FILE_BYTES {
                    let cost = hint.saturating_mul(TRANSIENT_BYTES_PER_HINT_BYTE);
                    large_tx
                        .send((i, cost))
                        .expect("large-file threads outlive the small-file pass");
                    None
                } else {
                    Some(prepare(path, store))
                }
            })
            .collect();
        // Every sender is dropped once the pass above ends, so the large-file
        // threads drain the channel and stop.
        for thread in large_threads {
            let done = thread
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            for (i, result) in done {
                results[i] = Some(result);
            }
        }
        results
            .into_iter()
            .map(|r| r.expect("every path is prepared exactly once"))
            .collect()
    })
}

fn scan(dir: &Path, json: bool, config: ClusterConfig, extra_exclude: &[String]) -> Result<()> {
    anyhow::ensure!(dir.exists(), "scan path does not exist: {}", dir.display());

    let t0 = std::time::Instant::now();
    // File rows are completed with their fuzzy signature once `docs` stops
    // growing; `has_text` records which rows get one.
    let mut file_rows: Vec<(FileEntry<'static>, bool)> = Vec::new();
    let mut docs: Vec<ScannedDoc> = Vec::new();
    let mut metas: Vec<(DocInfo, Option<jiff::Timestamp>)> = Vec::new();
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
    // Shingle sets past their in-memory prefix go to a temporary file the
    // OS deletes when the scan ends; clustering reads them back on demand.
    let store = Arc::new(ShingleStore::temp().context("creating the temporary shingle store")?);
    let results = prepare_all(&paths, &store);
    for (path, result) in paths.iter().zip(results) {
        match result {
            Ok(prepared) => {
                let PreparedScan {
                    info,
                    fs_mtime,
                    exact_hash,
                    byte_hash,
                    has_text,
                    sig,
                    shingles,
                    chars,
                } = prepared;

                let content_hash = byte_hash.clone().unwrap_or_else(|| exact_hash.clone());
                file_rows.push((
                    FileEntry {
                        path: info.path.display().to_string(),
                        format: info.format,
                        content_hash,
                        fuzzy: None,
                        signals: FileSignals {
                            chars,
                            modified: info.meta.modified.map(|t| t.to_string()),
                            revision: info.meta.revision,
                            fs_mtime: fs_mtime.map(|t| t.to_string()),
                        },
                    },
                    has_text,
                ));
                docs.push(match byte_hash {
                    Some(byte_hash) => ScannedDoc::from_precomputed_with_byte_hash(
                        info.path.clone(),
                        byte_hash.clone(),
                        byte_hash,
                        sig,
                        shingles,
                    ),
                    None => {
                        ScannedDoc::from_precomputed(info.path.clone(), exact_hash, sig, shingles)
                    }
                });
                metas.push((info, fs_mtime));
            }
            Err(PrepareError::File(e)) => errors.push(ErrorEntry {
                path: path.display().to_string(),
                error: e.to_string(),
            }),
            Err(PrepareError::Store(e)) => {
                return Err(e).context("writing shingles to the temporary shingle store");
            }
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
    for fam in families {
        let signals: Vec<MemberSignals> = fam
            .members
            .iter()
            .map(|m| {
                let (canon, fs_mtime) = metas
                    .iter()
                    .find(|(c, _)| c.path == m.path)
                    .map(|(c, t)| (c, *t))
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
            members: fam.members,
            edges: fam.edges,
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

    let files: Vec<FileEntry<'_>> = file_rows
        .into_iter()
        .zip(&docs)
        .map(|((row, has_text), doc)| {
            let mut row: FileEntry<'_> = row;
            row.fuzzy = has_text.then_some(doc.sig.values.as_slice());
            row
        })
        .collect();

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
        // Stream straight to stdout: the pretty document can run to
        // hundreds of megabytes and never needs to exist as one string.
        let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
        serde_json::to_writer_pretty(&mut stdout, &out)?;
        writeln!(stdout)?;
        stdout.flush()?;
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
