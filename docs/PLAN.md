# Plan

Scope discipline matters: unchecked format growth would turn dupey into a
generic file-cleaning utility. The first release focuses on reliable document
families and an explainable latest candidate.

## Completed for v1

- Workspace split into the `dupey-core` library and `dupey` CLI
- UTF-8 extraction and newline normalization for `txt` and `md`
- SHA-256 exact hashing
- 128-permutation gaoya MinHash over character 5-grams
- CLI commands: `fingerprint`, `compare`, and `scan`
- DOCX paragraph extraction with volatile properties removed; internal
  modification time and revision metadata from `core.xml`
- HWPX body-text extraction with internal date metadata
- PDF embedded-text extraction; image-only scans are reported and excluded
  from comparable families
- Exact, near (default threshold 0.90), and contains clustering
- LSH candidates using 64 x 2 bands
- Bottom-k sketch candidates (`k = 64`) for containment involving small
  documents, including a document-frequency filter
- Candidate verification with size/Jaccard lower bounds and merge-intersection
- Latest-candidate ranking by modification time only: internal timestamp
  first, filesystem timestamp second
- Stable public contract through `dupey scan DIR --json`
- Binary HWP extraction from CFB, raw deflate streams, and
  `HWPTAG_PARA_TEXT`, including table-cell text and internal edit time
- PPTX extraction in slide order, excluding speaker notes
- XLSX cell extraction with shared strings and serial-date conversion
- Parallel extraction, hashing, and signature generation with stable ordering
- Live end-to-end fixtures, Criterion benchmarks, and corpus benchmarks

## Next

1. Monitor containment precision on corpora with more than 100,000 documents.
2. Document OCR integration boundaries for scanned PDFs without adding OCR to
   the core.
3. Expand binary HWP field and nested-object extraction where it improves
   comparable prose.

```text
scan DIR
  -> families: [{id, files, relation: exact|near|contains}]
  -> each file: {content_hash, fuzzy, signals}
  -> pick: {ranked, reasons, confidence}
```

## Not in this milestone

- Embeddings
- Automatic-deletion UI
- Perfect support for every office format
- Marketing claims that the latest version is always identified correctly

## Verification targets

- One-line document revision: near score >= 0.85
- Unrelated documents: near score < 0.3
- Identical text with different line endings: identical exact hash
- DOCX re-saved without content changes: identical hash after extraction

## Public API

```text
extract(path) -> CanonicalText
exact_hash(text) -> sha256
near_sig(text) -> minhash
score(signature_a, signature_b) -> 0.0..1.0
cluster(documents, threshold) -> families
rank(family, signals) -> ranked candidates with reasons
```
