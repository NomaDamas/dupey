#!/usr/bin/env bash
# Unix convenience wrapper around the cross-platform e2e driver.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -q
python3 scripts/e2e.py
