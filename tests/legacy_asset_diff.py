#!/usr/bin/env python3
"""Report how served legacy-web/ differs from frozen reference/legacy-web/.

Walks both trees with no skips and classifies every file as identical,
modified, only-in-legacy or only-in-reference via SHA-256. Modified text
files (.js .css .html .json .webmanifest .md .txt .svg) get added/removed
line counts from a unified diff; binary files report byte sizes.
Intentional baseline changes belong in docs/migration.md. This is a report,
not a validation gate: it always exits 0.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import difflib
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
LEGACY = ROOT / "legacy-web"
REFERENCE = ROOT / "reference" / "legacy-web"
TEXT_SUFFIXES = {".js", ".css", ".html", ".json", ".webmanifest", ".md", ".txt", ".svg"}
STATUS_ORDER = ("modified", "only-in-legacy", "only-in-reference", "identical")
DIFF_LIMIT = 400


def collect(root: Path) -> dict[str, Path]:
    files = {}
    for path in sorted(root.rglob("*")):
        if path.is_file():
            files[path.relative_to(root).as_posix()] = path
    return files


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def is_text(rel: str) -> bool:
    return Path(rel).suffix.lower() in TEXT_SUFFIXES


def read_text(path: Path | None) -> str:
    return "" if path is None else path.read_text(encoding="utf-8", errors="replace")


def unified(old: str, new: str, rel: str) -> list[str]:
    return list(difflib.unified_diff(
        old.splitlines(True), new.splitlines(True),
        fromfile=f"reference/legacy-web/{rel}", tofile=f"legacy-web/{rel}",
    ))


def count_diff(lines: list[str]) -> tuple[int, int]:
    """Count +/- lines inside hunks; skip ---/+++/@@ headers."""
    added = removed = 0
    in_hunk = False
    for line in lines:
        if line.startswith("@@"):
            in_hunk = True
            continue
        if not in_hunk:
            continue
        if line.startswith("+"):
            added += 1
        elif line.startswith("-"):
            removed += 1
    return added, removed


def classify(legacy_files: dict[str, Path], reference_files: dict[str, Path]) -> list[dict]:
    rows = []
    for rel in sorted(set(legacy_files) | set(reference_files)):
        left, right = legacy_files.get(rel), reference_files.get(rel)
        size_l = left.stat().st_size if left else None
        size_r = right.stat().st_size if right else None
        added = removed = None
        if left and right:
            status = "identical" if digest(left) == digest(right) else "modified"
            if status == "modified" and is_text(rel):
                added, removed = count_diff(unified(read_text(right), read_text(left), rel))
        elif left:
            status = "only-in-legacy"
        else:
            status = "only-in-reference"
        rows.append({
            "file": rel, "status": status, "added": added, "removed": removed,
            "size_legacy": size_l, "size_reference": size_r,
        })
    rows.sort(key=lambda row: (STATUS_ORDER.index(row["status"]), row["file"]))
    return rows


def totals(rows: list[dict]) -> dict:
    out = {status: 0 for status in STATUS_ORDER}
    out["added"] = out["removed"] = 0
    for row in rows:
        out[row["status"]] += 1
        if row["added"] is not None:
            out["added"] += row["added"]
        if row["removed"] is not None:
            out["removed"] += row["removed"]
    return out


def size_cell(row: dict) -> str:
    left = "-" if row["size_legacy"] is None else str(row["size_legacy"])
    right = "-" if row["size_reference"] is None else str(row["size_reference"])
    return f"{left}/{right}"


def print_table(rows: list[dict], tot: dict) -> None:
    print("| File | Status | +lines | -lines | Size (legacy/reference) |")
    print("| --- | --- | --- | --- | --- |")
    for row in rows:
        plus = "" if row["added"] is None else row["added"]
        minus = "" if row["removed"] is None else row["removed"]
        print(f"| {row['file']} | {row['status']} | {plus} | {minus} | {size_cell(row)} |")
    print()
    parts = [f"{status}: {tot[status]}" for status in STATUS_ORDER]
    parts += [f"+lines: {tot['added']}", f"-lines: {tot['removed']}"]
    print("totals  " + "  ".join(parts))


def find_row(rows: list[dict], name: str) -> dict | None:
    name = name.replace("\\", "/")
    exact = [row for row in rows if row["file"] == name]
    if exact:
        return exact[0]
    matches = [row for row in rows if Path(row["file"]).name == name]
    return matches[0] if matches else None


def show_diff(row: dict, legacy_files: dict[str, Path], reference_files: dict[str, Path]) -> None:
    rel = row["file"]
    if not is_text(rel):
        print(f"{rel}: binary ({row['status']}) size {size_cell(row)}")
        return
    lines = unified(read_text(reference_files.get(rel)), read_text(legacy_files.get(rel)), rel)
    if not lines:
        print(f"{rel}: {row['status']}, no textual diff")
        return
    for index, line in enumerate(lines):
        if index >= DIFF_LIMIT:
            print(f"... truncated after {DIFF_LIMIT} lines ({len(lines) - DIFF_LIMIT} more)")
            return
        sys.stdout.write(line if line.endswith("\n") else line + "\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="print the report as JSON")
    parser.add_argument("--show-diff", metavar="NAME",
                        help="print unified diff of one file (max 400 lines)")
    args = parser.parse_args()
    legacy_files = collect(LEGACY)
    reference_files = collect(REFERENCE)
    rows = classify(legacy_files, reference_files)
    tot = totals(rows)
    if args.show_diff:
        row = find_row(rows, args.show_diff)
        if row is None:
            print(f"no file matching {args.show_diff!r}")
        else:
            show_diff(row, legacy_files, reference_files)
    elif args.json:
        json.dump({"files": rows, "totals": tot}, sys.stdout, indent=2)
        sys.stdout.write("\n")
    else:
        print_table(rows, tot)
    return 0


if __name__ == "__main__":
    sys.exit(main())
