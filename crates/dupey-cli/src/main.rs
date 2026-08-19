use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use dupey_core::{exact_hash_hex, extract, near_sig, score, Format};

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
    Fingerprint {
        path: PathBuf,
    },
    /// Compare two files (txt/md in this scaffold)
    Compare {
        a: PathBuf,
        b: PathBuf,
    },
    /// List files dupey will eventually scan (format routing only)
    Scan {
        dir: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Fingerprint { path } => fingerprint(&path),
        Command::Compare { a, b } => compare(&a, &b),
        Command::Scan { dir } => scan(&dir),
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
    Ok(())
}

fn compare(a: &Path, b: &Path) -> Result<()> {
    let ca = extract(a).with_context(|| format!("extract {}", a.display()))?;
    let cb = extract(b).with_context(|| format!("extract {}", b.display()))?;
    let ha = exact_hash_hex(&ca.text);
    let hb = exact_hash_hex(&cb.text);
    let near = score(&near_sig(&ca.text), &near_sig(&cb.text));
    println!("a\t{}", a.display());
    println!("b\t{}", b.display());
    println!("exact_equal\t{}", ha == hb);
    println!("near_score\t{near:.4}");
    Ok(())
}

fn scan(dir: &Path) -> Result<()> {
    let mut n = 0usize;
    let mut ready = 0usize;
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        if let Some(fmt) = Format::from_path(entry.path()) {
            n += 1;
            if fmt.extract_ready() {
                ready += 1;
            }
            println!(
                "{}\t{:?}\t{}",
                if fmt.extract_ready() { "ready" } else { "planned" },
                fmt,
                entry.path().display()
            );
        }
    }
    eprintln!("matched {n} known formats, extract-ready {ready}");
    Ok(())
}
