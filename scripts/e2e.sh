#!/usr/bin/env bash
# Live e2e: build release, generate a real docx/hwpx/pdf/txt corpus,
# run `dupey scan --json`, assert the documented verification rules.
set -euo pipefail
cd "$(dirname "$0")/.."

CORPUS="${1:-target/e2e-corpus}"

cargo build --release -q
cargo run --release -q -p dupey --example mkfixtures -- "$CORPUS" 1

./target/release/dupey scan "$CORPUS" --json > target/e2e-scan.json

python3 - "$CORPUS" <<'PY'
import json, sys, os

corpus = sys.argv[1]
with open("target/e2e-scan.json") as f:
    out = json.load(f)

fails = []
def check(name, cond, detail=""):
    print(f"{'PASS' if cond else 'FAIL'}  {name}  {detail}")
    if not cond:
        fails.append(name)

files = {os.path.basename(f["path"]): f for f in out["files"]}
fams = out["families"]

def family_of(name):
    for fam in fams:
        if any(m["path"].endswith(name) for m in fam["members"]):
            return fam
    return None

def member(fam, name):
    return next(m for m in fam["members"] if m["path"].endswith(name))

# 1. one-line edit proposal: near >= 0.85
fam = family_of("제안서_최종.docx")
check("proposal family exists", fam is not None)
if fam:
    m = member(fam, "제안서_최종.docx")
    check("one-line edit near >= 0.85", (m["near_score"] or 0) >= 0.85,
          f"near_score={m['near_score']:.3f}")
    check("pick is 제안서_최종.docx (internal time + 최종 token)",
          fam["pick"]["ranked"][0]["path"].endswith("제안서_최종.docx"),
          f"confidence={fam['pick']['confidence']:.2f}")
    check("pick reasons are non-empty", len(fam["pick"]["reasons"]) > 0)

# 2. docx resave: same body, different metadata => exact-equal member
if fam:
    resaved = member(fam, "제안서_재저장.docx")
    check("docx resave is exact-equal", resaved["relation"] == "exact")

# 3. hwpx identical copies => exact family
fam2 = family_of("보고서 사본.hwpx")
check("hwpx exact family", fam2 is not None and fam2["relation"] == "exact")

# 4. pdf one-line edit: near >= 0.85
fam3 = family_of("minutes_v2.pdf")
check("pdf family exists", fam3 is not None)
if fam3:
    m = member(fam3, "minutes_v2.pdf")
    check("pdf one-line edit near >= 0.85", (m["near_score"] or 0) >= 0.85,
          f"near_score={m['near_score']:.3f}")

# 5. contains: draft inside final
fam4 = family_of("계획_최종.txt")
check("contains family exists", fam4 is not None)
if fam4:
    rels = {m["relation"] for m in fam4["members"]}
    check("contains relation present", "contains" in rels, f"relations={rels}")

# 6. newline-only difference => exact equal
check("crlf copy hashes identically",
      files["계획_초안.txt"]["content_hash"] == files["계획_초안_crlf.md"]["content_hash"])

# 7. unrelated memo: in no family, and near score vs proposal < 0.3
check("unrelated memo in no family", family_of("메모.txt") is None)

# 8. hwp one-line edit: near >= 0.85, 최종 pick
fam5 = family_of("운영계획_최종.hwp")
check("hwp family exists", fam5 is not None)
if fam5:
    m = member(fam5, "운영계획_최종.hwp")
    check("hwp one-line edit near >= 0.85", (m["near_score"] or 0) >= 0.85,
          f"near_score={m['near_score']:.3f}")
    check("hwp pick is 최종", fam5["pick"]["ranked"][0]["path"].endswith("운영계획_최종.hwp"))

# 9. pptx identical slides => exact family
fam6 = family_of("발표자료_복사본.pptx")
check("pptx exact family", fam6 is not None and fam6["relation"] == "exact")

# 10. xlsx one-cell edit: near >= 0.85, later internal time wins
fam7 = family_of("예산표_v2.xlsx")
check("xlsx family exists", fam7 is not None)
if fam7:
    m = member(fam7, "예산표_v2.xlsx")
    check("xlsx one-cell edit near >= 0.85", (m["near_score"] or 0) >= 0.85,
          f"near_score={m['near_score']:.3f}")
    check("xlsx pick is v2 (internal time)",
          fam7["pick"]["ranked"][0]["path"].endswith("예산표_v2.xlsx"))

# 11. scanned pdf: no fuzzy signature, reported or skipped gracefully
scanpdf = files.get("scan.pdf")
if scanpdf:
    check("scanned pdf has no fuzzy", scanpdf["fuzzy"] is None)
else:
    err = any(e["path"].endswith("scan.pdf") for e in out["errors"])
    check("scanned pdf surfaces as error", err)

if fails:
    print(f"\n{len(fails)} check(s) failed")
    sys.exit(1)
print(f"\nall checks passed: {len(out['files'])} files, {len(fams)} families")
PY
