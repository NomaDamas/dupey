# dupey

Office **document family** detector. Not a generic duplicate-file cleaner.

`extract` (per extension) → SHA-256 exact hash → MinHash near-dup score → family cluster → **explainable** latest/canonical ranking.

Semantic embeddings are out of scope. Auto-delete is out of scope.

Status: scaffold. `txt` / `md` extract + exact hash + MinHash compare work. `docx` / `hwpx` / `pdf` extract is planned.

```bash
cargo run -p dupey -- fingerprint notes.txt
cargo run -p dupey -- compare 최종.txt 찐최종.txt
cargo run -p dupey -- scan ./docs
```

Library crate: [`dupey-core`](crates/dupey-core). CLI binary: `dupey`.

See [docs/DIRECTION.md](docs/DIRECTION.md) (why this exists) and [docs/PLAN.md](docs/PLAN.md) (what lands next).

License: MIT.
