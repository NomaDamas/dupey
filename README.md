<p align="center">
  <img src="assets/dupey-logo.svg" alt="dupey logo" width="280">
</p>

<h1 align="center">dupey</h1>

<p align="center">
  Find document families, not just byte-for-byte duplicates.
</p>

<p align="center">
  <a href="https://crates.io/crates/dupey"><img src="https://img.shields.io/crates/v/dupey.svg" alt="crates.io"></a>
  <a href="https://docs.rs/dupey-core"><img src="https://docs.rs/dupey-core/badge.svg" alt="docs.rs"></a>
  <a href="https://github.com/NomaDamas/dupey/actions/workflows/release.yml"><img src="https://github.com/NomaDamas/dupey/actions/workflows/release.yml/badge.svg" alt="release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT license"></a>
</p>

`dupey` extracts comparable text from office documents, detects exact and
near-duplicate files, groups them into families, and explains which file is
the best latest-version candidate.

It does **not** use embeddings, upload files, or delete anything.

## Install

```bash
cargo install dupey
```

Requires Rust 1.91 or newer.

## Quick start

```bash
# Scan a folder and print a readable summary
dupey scan ./documents

# Emit the stable JSON contract
dupey scan ./documents --json

# Ignore additional folder names
dupey scan ./documents --exclude-dir archive --exclude-dir scratch

# Inspect one document
dupey fingerprint ./documents/proposal.docx

# Compare two versions directly
dupey compare ./documents/proposal.docx ./documents/proposal-final.docx
```

`scan` skips common vendor, VCS, and build folders such as `node_modules`,
`.git`, `target`, `dist`, and `build`.

## What dupey detects

| Relation | Meaning |
| --- | --- |
| `exact` | Extracted document content is identical. |
| `near` | Documents have high lexical overlap after format-aware extraction. |
| `contains` | One document substantially contains another. |

Supported input:

| Format | Extraction |
| --- | --- |
| `txt`, `md` | UTF-8 text with normalized newlines |
| `docx` | Paragraph text and internal modification metadata |
| `hwp`, `hwpx` | Comparable body text and available internal timestamps |
| `pptx` | Slide text, excluding speaker notes |
| `xlsx` | Cell values with shared-string and date handling |
| `pdf` | Embedded text; image-only scans are reported but not compared |

## How it works

```text
document
  -> format-aware text extraction
  -> normalized comparable text
       |-> SHA-256 exact hash
       `-> character shingles + MinHash
  -> exact / near / contains family
  -> explainable latest-candidate ranking
```

Near-duplicate detection is lexical, not semantic. This keeps results local,
fast, and understandable while avoiding unrelated documents that merely share
a topic.

## Latest-candidate ranking

Within a family, dupey ranks files by modification time:

1. the document's internal modification time, when available;
2. otherwise, the filesystem modification time.

Filename tokens, revision counters, containment, and document length are
reported as context but are not hidden ranking weights. A result is a
candidate with reasons and confidence, never a claim of absolute truth.

## JSON output

```jsonc
{
  "files": [
    {
      "path": "documents/proposal.docx",
      "format": "docx",
      "content_hash": "...",
      "fuzzy": ["..."],
      "signals": {
        "chars": 1842,
        "modified": "2026-08-20T09:30:00Z",
        "revision": 7,
        "fs_mtime": "2026-08-20T09:31:12Z"
      }
    }
  ],
  "families": [
    {
      "id": 1,
      "relation": "near",
      "files": ["documents/proposal.docx", "documents/proposal-final.docx"],
      "members": ["..."],
      "pick": {
        "ranked": ["..."],
        "reasons": ["..."],
        "confidence": 0.9
      }
    }
  ],
  "errors": []
}
```

The exact machine-readable schema is defined by `dupey scan DIR --json`.

## Library

The reusable engine is published as
[`dupey-core`](https://crates.io/crates/dupey-core). Its public API exposes
format extraction, exact hashing, MinHash signatures, family clustering, and
ranking without depending on the CLI.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
./scripts/e2e.sh
```

See [Contributing](docs/CONTRIBUTING.md), [Direction](docs/DIRECTION.md), and
[Plan](docs/PLAN.md) for project details.

## Releasing

Maintainers do not edit the version manually. Run the **Prepare release**
workflow in GitHub Actions and enter the next version without a leading `v`,
for example `0.1.1`.

The workflow updates the workspace manifest and lockfile, runs the release
checks, commits and pushes the version bump to `main`, creates the matching
`v0.1.1` tag and GitHub Release, and starts the OIDC-backed crates.io publish
workflow. The tag, source commit, GitHub Release, and published packages
therefore all refer to the same version.

## License

[MIT](LICENSE)
