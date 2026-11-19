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
the best latest-version candidate. Images (`png`, `jpg`, `gif`, `bmp`, `webp`,
`tif`, `avif`) are grouped the same way by their decoded pixels and a perceptual hash.

It does **not** use embeddings, upload files, or delete anything.

## Install

```bash
cargo install dupey
```

Requires Rust 1.93 or newer (the pure-Rust AVIF decoder needs it).

## Install the agent skill

If you use a coding agent to review and organize duplicate or near-duplicate
documents, install the dupey Agent Skill. It teaches the agent how to run
dupey, interpret `exact`, `near`, `contains`, and `mixed` correctly, and
prepare a reviewed cleanup plan instead of deleting files from a score alone.

```bash
npx skills add NomaDamas/dupey
```

Ask your agent to use the `dupey-document-family` skill before it reorganizes
document folders.

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

# Compare two images (pixel equality + perceptual distance)
dupey compare ./photos/a.png ./photos/a-small.jpg

# Only look at images (documents are not read)
dupey scan ./photos --images-only

# Loosen or tighten the image `near` match (default 0.90 = at most 12 of 128 bits)
dupey scan ./photos --image-threshold 0.85
```

`scan` skips common vendor, VCS, and build folders such as `node_modules`,
`.git`, `target`, `dist`, and `build`.

## What dupey detects

| Relation | Meaning |
| --- | --- |
| `exact` | Extracted document content is identical. |
| `near` | Documents have high lexical overlap after format-aware extraction. |
| `contains` | One document substantially contains another. |

For images, `exact` means identical decoded pixels and `near` means a close
perceptual hash (see [Images](#images)). `contains` never applies to images.

Supported input:

| Format | Extraction |
| --- | --- |
| `txt`, `md` | UTF-8 text with normalized newlines |
| `docx` | Paragraph text and internal modification metadata |
| `hwp` | HWP 3.0 (legacy binary) and HWP 5.x (CFB), with compressed/uncompressed body text |
| `hwpx` | Comparable body text and available internal timestamps |
| `pptx` | Slide text, excluding speaker notes |
| `xlsx` | Cell values with shared-string and date handling |
| `pdf` | Embedded text; image-only scans are reported but not compared |

`exact` compares extracted text, including HWP dates, displayed field values,
and table-cell text. It does not assert identical formatting or embedded images.
When extraction has no non-whitespace text, `scan`, `compare`, and `fingerprint`
use original file bytes for exact matching; fuzzy comparison is unavailable.
Malformed HWP records or compressed streams, password-encrypted documents,
and distribution documents are reported as extraction errors.

Legacy HWP 3.0 files start with `HWP Document File V3.00`, not the CFB signature.
They are detected automatically and decoded with their own Korean character
encoding. Extraction includes tables, textboxes (including grouped drawings),
headers/footers, notes, captions, date codes, and displayed reference values.
Outline numbers use normalized notation retaining custom bullets and decorations;
this is text extraction, not a page renderer. Unsupported characters or malformed
nested structures produce errors instead of a hash of incomplete text.

**HWP reindexing:** Regenerate stored HWP fingerprints and scan results created
with 0.1.2 or earlier. Those versions misread record byte lengths as four-byte
units and could silently hash incomplete or empty text. Corrected extraction
changes HWP content hashes and similarity scores.

## Images

| Format | Handling |
| --- | --- |
| `png`, `jpg`/`jpeg`, `gif`, `bmp`, `webp`, `tif`/`tiff` | Decoded, EXIF orientation applied, then hashed |
| `avif` | Decoded by a pure-Rust AV1 decoder; `irot`/`imir` applied, alpha composited over white for the hash |

Same rule as documents: dupey never hashes raw file bytes. A PNG re-saved as
BMP, or a JPEG with rewritten metadata, is `exact` because its decoded RGBA
pixels hash identically. The codec is chosen from file content, not the
extension; animated files contribute their first frame.

`near` uses a 128-bit perceptual hash computed in the same single pass over
the pixels: a 64-bit DCT hash (survives resizing, recompression, and
brightness shifts) plus a 64-bit gradient hash (rejects structurally
different pictures). Similarity is `1 - hamming / 128`; `--image-threshold`
defaults to 0.90 (at most 12 differing bits). Candidates come from
multi-index hashing, which finds every pair within the threshold without
comparing all pairs, and each is verified by an exact bit count.

Limits, stated plainly:

- Solid-colour and other detail-free images have no perceptual hash; they
  only match when pixel-identical, so a black and a white image never merge.
- Rotated, mirrored, and cropped copies are not detected (EXIF orientation
  tags are honoured, pixel rotation is not undone).
- Hashing is luminance-based: a recoloured copy with the same shapes can
  match.
- HEIC/HEIF and SVG are not decoded; such files are ignored by `scan`. HEIC
  needs an HEVC decoder, which is encumbered by patents, so it is left out on
  purpose.
- Lossless AVIF stored as RGB (identity matrix, e.g. `heif-enc -L`,
  `avifenc --lossless`) is reported in `errors[]`: the decoder converts it as
  YCbCr and would return wrong colours, and a wrong hash is worse than none.
- The pure-Rust AV1 decoder mis-decodes some photos stored by SVT-AV1 (the
  encoder behind `ffmpeg -c:v libsvtav1`). On 5 real photos encoded at default
  settings, near matches to the source at the default threshold were: libaom
  (`heif-enc`, `avifenc`) 5/5, SVT-AV1 8-bit 3/5, SVT-AV1 10-bit 0/5. Identical
  copies of any AVIF always match each other, so exact duplicates are
  unaffected; only cross-format or re-encoded near matches of SVT files can
  be missed.
- AVIF decoding is about 2-3x slower than JPEG (12 MP: ~375 ms, ~90 MB
  resident); large AVIFs use the scan's memory-bounded path.
- An image whose header declares more than 1 GiB of decoded pixels is reported
  in `errors[]` instead of decoded, so a tiny hostile file cannot exhaust memory.
- Images rank by filesystem modification time only, and `scan --json` adds
  `signals.width` / `signals.height`, `image_hash`, and `hamming` evidence so
  you can prefer the highest-resolution copy yourself.

Image families and document families never mix; family ids are unique across
both.

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

`scan` keeps memory bounded on large folders. Each document keeps only a
sorted prefix of its shingle set in memory (about an eighth of it); the rest
goes to an anonymous temporary file in the system temp directory (`TMPDIR`
on macOS and Linux) that the operating system deletes when the scan ends.
An exact upper bound computed from the prefixes rules out most candidate
pairs, so only likely matches read their full sets back. Families are
identical to keeping everything in memory. Expect roughly 1.5 GB of
temporary disk for 80,000 office documents.

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
      // "exact" | "near" | "contains", or "mixed" when members joined
      // by different relations
      "relation": "mixed",
      "files": ["documents/proposal.docx", "documents/proposal-final.docx"],
      // each member names the file it actually matched, and how
      "members": [
        {
          "path": "documents/proposal-final.docx",
          "relation": "contains",
          "joined_with": "documents/proposal.docx",
          "near_score": 0.62,
          "jaccard": 0.58,
          "containment": 0.98,
          "exact_hash": "..."
        }
      ],
      // every verified pair behind this family; for "contains",
      // a is the container and b the contained document
      "edges": [
        {
          "relation": "contains",
          "a": "documents/proposal-final.docx",
          "b": "documents/proposal.docx",
          "near_score": 0.62,
          "jaccard": 0.58,
          "containment": 0.98
        }
      ],
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

`threshold`, `contains_threshold`, and `contains_min_jaccard` are echoed at
the top level so a consumer can see which gates produced the families.

The exact machine-readable schema is defined by `dupey scan DIR --json`.

### Why near and contains have separate gates

`near` compares with Jaccard, whose denominator is the union of both
documents. `contains` compares with containment, whose denominator is only
the smaller document, so the same number is a far weaker bar: a shared
corporate template can fill 90% of a short document without the two being
versions of each other. `contains` therefore has its own, stricter threshold
(`--contains-threshold`, default 0.96) plus a Jaccard floor
(`--contains-min-jaccard`, default 0.40) that stops a short fragment quoted
by many long files from chaining them into one family.

## Library

The reusable engine is published as
[`dupey-core`](https://crates.io/crates/dupey-core). Its public API exposes
format extraction, exact hashing, MinHash signatures, family clustering, and
ranking without depending on the CLI.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace            # unit + integration (real binary, real fixtures)
python scripts/e2e.py             # cross-platform live e2e (Linux/macOS/Windows)
./scripts/e2e.sh                  # Unix convenience wrapper
cargo build --workspace --release --locked
cargo bench -p dupey-core         # criterion: extract / near_sig / cluster
./scripts/bench.sh 10             # corpus scan benchmark (10 x 100 files)
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
