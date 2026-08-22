#!/usr/bin/env python3
"""Cross-platform live e2e checks for the release dupey CLI."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
TARGET = ROOT / "target"
CORPUS = TARGET / "e2e-corpus"
EXE = ".exe" if os.name == "nt" else ""
DUPEY = TARGET / "release" / f"dupey{EXE}"


def run(*args: object, stdout=None) -> None:
    subprocess.run(
        [str(arg) for arg in args],
        cwd=ROOT,
        check=True,
        stdout=stdout,
    )


def check(name: str, condition: bool, detail: str = "") -> None:
    print(f"{'PASS' if condition else 'FAIL'}  {name}  {detail}")
    if not condition:
        failures.append(name)


shutil.rmtree(CORPUS, ignore_errors=True)
run(
    "cargo",
    "run",
    "--release",
    "--locked",
    "-q",
    "-p",
    "dupey",
    "--example",
    "mkfixtures",
    "--",
    CORPUS,
    "1",
)

(CORPUS / "node_modules" / "pkg").mkdir(parents=True)
(CORPUS / ".git").mkdir()
(CORPUS / "node_modules" / "pkg" / "LICENSE.md").write_text(
    "MIT License dummy from vendor\n", encoding="utf-8"
)
(CORPUS / ".git" / "README.md").write_text(
    "should not be scanned\n", encoding="utf-8"
)

scan_path = TARGET / "e2e-scan.json"
with scan_path.open("wb") as output:
    run(DUPEY, "scan", CORPUS, "--json", stdout=output)
with scan_path.open(encoding="utf-8") as source:
    result = json.load(source)

failures: list[str] = []
files = {Path(file["path"]).name: file for file in result["files"]}
families = result["families"]


def family_of(name: str):
    return next(
        (
            family
            for family in families
            if any(Path(member["path"]).name == name for member in family["members"])
        ),
        None,
    )


def member(family, name: str):
    return next(
        item for item in family["members"] if Path(item["path"]).name == name
    )


proposal = family_of("제안서_최종.docx")
check("proposal family exists", proposal is not None)
if proposal:
    proposal_final = member(proposal, "제안서_최종.docx")
    check(
        "one-line edit near >= 0.85",
        (proposal_final["near_score"] or 0) >= 0.85,
        f"near_score={proposal_final['near_score']:.3f}",
    )
    check(
        "proposal pick follows internal time",
        Path(proposal["pick"]["ranked"][0]["path"]).name == "제안서_최종.docx",
        f"confidence={proposal['pick']['confidence']:.2f}",
    )
    check("proposal pick has reasons", bool(proposal["pick"]["reasons"]))
    check(
        "docx resave is exact-equal",
        member(proposal, "제안서_재저장.docx")["relation"] == "exact",
    )

hwpx = family_of("보고서 사본.hwpx")
check("hwpx exact family", hwpx is not None and hwpx["relation"] == "exact")

pdf = family_of("minutes_v2.pdf")
check("pdf family exists", pdf is not None)
if pdf:
    pdf_final = member(pdf, "minutes_v2.pdf")
    check(
        "pdf one-line edit near >= 0.85",
        (pdf_final["near_score"] or 0) >= 0.85,
        f"near_score={pdf_final['near_score']:.3f}",
    )

contained = family_of("계획_최종.txt")
check("contains family exists", contained is not None)
if contained:
    relations = {item["relation"] for item in contained["members"]}
    check("contains relation present", "contains" in relations, str(relations))

check(
    "crlf copy hashes identically",
    files["계획_초안.txt"]["content_hash"]
    == files["계획_초안_crlf.md"]["content_hash"],
)
check("unrelated memo in no family", family_of("메모.txt") is None)

hwp = family_of("운영계획_최종.hwp")
check("hwp family exists", hwp is not None)
if hwp:
    hwp_final = member(hwp, "운영계획_최종.hwp")
    check(
        "hwp one-line edit near >= 0.85",
        (hwp_final["near_score"] or 0) >= 0.85,
        f"near_score={hwp_final['near_score']:.3f}",
    )
    check(
        "hwp pick is final",
        Path(hwp["pick"]["ranked"][0]["path"]).name == "운영계획_최종.hwp",
    )

pptx = family_of("발표자료_복사본.pptx")
check("pptx exact family", pptx is not None and pptx["relation"] == "exact")

xlsx = family_of("예산표_v2.xlsx")
check("xlsx family exists", xlsx is not None)
if xlsx:
    xlsx_final = member(xlsx, "예산표_v2.xlsx")
    check(
        "xlsx one-cell edit near >= 0.85",
        (xlsx_final["near_score"] or 0) >= 0.85,
        f"near_score={xlsx_final['near_score']:.3f}",
    )
    check(
        "xlsx pick follows internal time",
        Path(xlsx["pick"]["ranked"][0]["path"]).name == "예산표_v2.xlsx",
    )

scan_pdf = files.get("scan.pdf")
if scan_pdf:
    check("scanned pdf has no fuzzy", scan_pdf["fuzzy"] is None)
else:
    check(
        "scanned pdf surfaces as error",
        any(Path(error["path"]).name == "scan.pdf" for error in result["errors"]),
    )

all_parts = [
    {part.casefold() for part in Path(item["path"]).parts}
    for item in [*result["files"], *result["errors"]]
]
check("skips node_modules", all("node_modules" not in parts for parts in all_parts))
check("skips .git", all(".git" not in parts for parts in all_parts))

if failures:
    print(f"\n{len(failures)} check(s) failed", file=sys.stderr)
    raise SystemExit(1)
print(f"\nall checks passed: {len(result['files'])} files, {len(families)} families")
