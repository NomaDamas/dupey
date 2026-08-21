# Direction

dupey addresses the office-folder pattern where nearly identical documents
are repeatedly saved under names such as `final.docx`, `final-final.docx`, and
`really-final.hwp`.

The core remains an independent library with a thin CLI. It is not part of
MinSync: coupling it to a vector indexer would make it harder to reuse from
RAG systems, agents, and document management software.

## In one sentence

**Document-family detection with an explainable canonical candidate**, not a
generic duplicate-file finder.

## Non-goals

- **Semantic embeddings.** Revisions overlap in wording, not merely in topic.
  Semantic similarity can incorrectly group unrelated documents that share a
  template or subject.
- **A byte-hash-only cleaner.** A one-line edit produces a different SHA-256.
- **An absolute "latest file" claim.** Incorrect certainty destroys trust;
  dupey returns a candidate, its signals, and a confidence value.
- **Automatic deletion.**
- **GPU, model-file, or network requirements.**

## Pipeline

```text
file
  -> extract(extension)       # format-specific comparable body text
  -> normalized text
       |-> SHA-256            # exact equality
       `-> shingles -> MinHash # near equality, score 0..1
  -> family (exact | near | contains)
  -> rank (visible signals, not an absolute claim)
```

The MinHash score estimates Jaccard similarity. Office-document revisions
with one or two changed lines often score **0.85-0.99**. Family clustering
starts at **0.90** by default. Different documents that use the same corporate
template may fall around 0.6-0.8.

MinHash is never computed over raw file bytes. Formats such as DOCX are ZIP
containers whose bytes can change on every save even when their content does
not.

## Format-aware extraction

Scoring is shared. New format support belongs in extraction.

| Extension | Keep | Discard or normalize | Status |
| --- | --- | --- | --- |
| `txt`, `md` | Text | Normalize line endings | Available |
| `docx` | Paragraph text | Document properties and revision IDs | Available |
| `pptx` | Slide text | Speaker notes and package metadata | Available |
| `xlsx` | Cell values | Styles and calculation chains | Available |
| `pdf` | Embedded text | Creation metadata | Available |
| `hwp`, `hwpx` | Body text | Presentation metadata | Available |
| Scanned PDF, image | No comparable text without OCR | N/A | Excluded |

## Latest-version signals, not certainty

The top candidate in a family is selected by **modification time only**. An
internal document timestamp wins when present; otherwise dupey uses the
filesystem modification time. Internal time is preferred because downloads
and archive extraction often overwrite filesystem timestamps.

Filename tokens (`v3`, `final`, `copy`), containment, revision count, and
length are not ranking scores. They remain visible in family members and
`files[].signals` so users can judge them directly.

Example interpretation:

> Family 17 has four candidates. The top candidate has the newest internal
> modification time. Confidence: 0.90.

## Why existing tools leave a gap

| Existing category | Missing capability |
| --- | --- |
| jdupes, Czkawka | Exact duplicates only |
| datasketch, text-dedup | Assume text is already extracted |
| Co-Pietje | Forensics without latest-candidate ranking |
| Relativity, Purview | Expensive; canonical choice may favor longest text |
| M-Files | MD5-level duplicate detection |
| HWP/HWPX readers | Parsing without family detection |

The opportunity is not a novel similarity algorithm. It is the combination of
**office-format normalization, family detection, and an explainable canonical
candidate**.

## Technology

- Rust, with no C++ dependency
- MinHash/LSH through [gaoya](https://github.com/serega/gaoya)
- SHA-256 through `sha2`
- In typical team folders, opening compressed documents is more expensive
  than MinHash computation; performance is primarily file-I/O bound.

## Relationship with MinSync

MinSync is a consumer. `content_hash` represents the exact-equality layer.
dupey can provide family IDs and canonical candidates for an agent-facing
source of truth. The dependency direction is MinSync -> `dupey-core`, never
the reverse.
