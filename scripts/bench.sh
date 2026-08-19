#!/usr/bin/env bash
# Corpus scan benchmark: generate N*100 filler docs (mixed txt/md/docx/hwpx)
# plus the family fixtures, then time `dupey scan --json` end to end.
set -euo pipefail
cd "$(dirname "$0")/.."

SCALE="${1:-10}"   # default 1000 files
CORPUS="target/bench-corpus"

cargo build --release -q
cargo run --release -q -p dupey --example mkfixtures -- "$CORPUS" "$SCALE"
FILES=$(find "$CORPUS" -type f | wc -l | tr -d ' ')

echo "== dupey scan benchmark: $FILES files =="
/usr/bin/time -h ./target/release/dupey scan "$CORPUS" --json > target/bench-scan.json

python3 - <<PY
import json
with open("target/bench-scan.json") as f:
    out = json.load(f)
print(f"files={len(out['files'])} families={len(out['families'])} errors={len(out['errors'])}")
PY
