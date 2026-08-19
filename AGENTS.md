# dupey

Office document family detector. Independent of MinSync.

## Core Principles

- **Families, not cleaners**: exact SHA + MinHash near-dup + explainable latest ranking. No auto-delete.
- **Extract then hash**: never MinHash raw bytes of zip/OOXML/PDF.
- **No embeddings** in this crate. Lexical overlap only.
- **Latest is a score**, never a boolean SSOT claim.
- **Rust native**: gaoya for MinHash. No C++.
- **Format work is extract**: scoring is shared. v1 formats: txt/md (now), docx, hwpx, pdf text.
- **MinSync consumes this crate**; this crate does not depend on MinSync.

See `docs/DIRECTION.md` and `docs/PLAN.md`.
