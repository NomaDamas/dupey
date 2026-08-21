# dupey

Office **document family** detector. Not a generic duplicate-file cleaner.

`extract` (per extension) → SHA-256 exact hash → MinHash near-dup score → family cluster → **explainable** latest/canonical ranking.

Semantic embeddings are out of scope. Auto-delete is out of scope.

Status: v1 pipeline works. `txt` / `md` / `docx` / `hwpx` / binary `hwp` / `pptx` / `xlsx` / `pdf` (embedded text) extract, exact + near + contains clustering (LSH + bottom-k sketch candidates), and the `scan --json` public contract are live. Scanned PDFs are out of the comparable pipeline.

```bash
dupey scan ./docs --json          # the public contract
dupey scan ./docs --exclude-dir 임시 --exclude-dir 백업
dupey fingerprint proposal.docx   # canonical text hash + internal metadata
dupey compare 최종.docx 찐최종.docx
```

`scan` does not descend into vendor/VCS/tooling folders (`node_modules`, `.git`, `target`, `dist`, `build`, …). Add more folder **names** with repeatable `--exclude-dir`.


```jsonc
// scan DIR --json
{
  "files":    [{ "path", "format", "content_hash", "fuzzy", "signals" }],
  "families": [{ "id", "files", "relation", "members",
                 "pick": { "ranked", "reasons", "confidence" } }],
  "errors":   [{ "path", "error" }]
}
```

Latest pick inside a family is **modified time only** (internal document time beats that file's filesystem mtime). Filename tokens, containment, revision, and length are not scores — they stay on the payload for the user to judge. Tied times are a coin flip (confidence 0.5).

## Develop

```bash
cargo test --workspace            # unit + integration (real binary, real fixtures)
./scripts/e2e.sh                  # live e2e on generated docx/hwpx/pdf corpus
cargo bench -p dupey-core         # criterion: extract / near_sig / cluster
./scripts/bench.sh 10             # corpus scan benchmark (10 x 100 files)
```

Library crate: [`dupey-core`](crates/dupey-core). CLI binary: `dupey`.

See [docs/DIRECTION.md](docs/DIRECTION.md) (why this exists) and [docs/PLAN.md](docs/PLAN.md) (what lands next).

License: MIT.
