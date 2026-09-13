#!/usr/bin/env python3
"""Cross-check numeric budgets in docs against Rust source constants.

Uses budget_table.scan() then scans docs/*.md, README.md, AGENTS.md and
BACKEND_MIRGRATION_PLAN.md. Reports unmatched figures and uncited constants.
Exit 0 unless --strict.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from budget_table import HELPERS, ROOT, scan  # noqa: E402

DOCS = ("README.md", "AGENTS.md", "BACKEND_MIRGRATION_PLAN.md")
FIGURE_RE = re.compile(
    r"(\d+(?:\.\d+)?)\s*(MiB|KiB|GiB|MB|KB|ms|s|秒|项|条|张|层|个|events?|items?|grants?)"
    r"(?![A-Za-z])")
NEAR_RE = re.compile(r"limit|budget|at most|最多|上限|预算|≤|cap", re.I)
FENCE_RE = re.compile(r"^(\s{0,3})(`{3,}|~{3,})")
COUNT_HINTS = ("GRANT", "ITEMS", "ENTRIES")
COUNT_UNITS = {"项", "item", "items", "grant", "grants"}
BYTE_UNIT = {"kib": 1024, "kb": 1024, "mib": 1024 ** 2, "mb": 1024 ** 2, "gib": 1024 ** 3}
TIME_UNIT = {"ms", "s", "秒"}
ALIAS = {"kb": "kib", "mb": "mib", "秒": "s"}


def doc_paths():
    found = [ROOT / name for name in DOCS if (ROOT / name).is_file()]
    found.extend(sorted(p for p in (ROOT / "docs").glob("*.md") if p.is_file()))
    return found


def visible_lines(text):
    closing = None
    for number, line in enumerate(text.splitlines(), 1):
        match = FENCE_RE.match(line)
        if match:
            mark = (match.group(2)[0], len(match.group(2)))
            if closing is None:
                closing = mark
            elif mark[0] == closing[0] and mark[1] >= closing[1]:
                closing = None
            continue
        if closing is None:
            yield number, line


def fold(text):
    text = re.sub(r"\s+", "", str(text)).lower()
    for src, dst in ALIAS.items():
        if text.endswith(src):
            return text[: -len(src)] + dst
    return text


def near(prev, line, nxt, start, end):
    prefix = (prev or "") + "\n"
    blob = prefix + line + "\n" + (nxt or "")
    lo, hi = max(0, len(prefix) + start - 60), min(len(blob), len(prefix) + end + 60)
    return bool(NEAR_RE.search(blob[lo:hi]))


def classify(num, unit):
    lowered, value = unit.lower(), float(num)
    whole = int(value) if value.is_integer() else None
    if lowered in BYTE_UNIT:
        raw = value * BYTE_UNIT[lowered]
        return "byte", int(raw) if raw == int(raw) else None, ()
    if lowered in TIME_UNIT:
        return "time", whole, ()
    hints = COUNT_HINTS if lowered in COUNT_UNITS or unit in COUNT_UNITS else ()
    return "count", whole, hints


def matches(row, kind, size, hints, figure):
    if hints and not any(token in row["name"].upper() for token in hints):
        return False
    human, value = str(row["human"]), row["value"]
    if kind == "byte":
        return (value != "?" and size is not None and value == size) or fold(human) == fold(figure)
    if kind == "time":
        return fold(human) == fold(figure)
    return human == str(size) or (value == size and fold(human) == str(size))


def snippet(line, start, end, width=40):
    lo, hi = max(0, start - width), min(len(line), end + width)
    text = line[lo:hi].strip()
    return ("…" if lo else "") + text + ("…" if hi < len(line) else "")


def collect_figures():
    found = []
    for path in doc_paths():
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        rows, rel = list(visible_lines(text)), path.relative_to(ROOT).as_posix()
        for index, (number, line) in enumerate(rows):
            if "p50" in line or "p95" in line or "→" in line:
                continue
            prev = rows[index - 1][1] if index else ""
            nxt = rows[index + 1][1] if index + 1 < len(rows) else ""
            for match in FIGURE_RE.finditer(line):
                if not near(prev, line, nxt, match.start(), match.end()):
                    continue
                kind, size, hints = classify(match.group(1), match.group(2))
                found.append({
                    "file": rel, "line": number, "figure": match.group(0),
                    "context": snippet(line, match.start(), match.end()),
                    "kind": kind, "size": size, "hints": hints,
                })
    return found


def report(consts, figures):
    unmatched, cited = [], set()
    for fig in figures:
        hits = [row for row in consts
                if matches(row, fig["kind"], fig["size"], fig["hints"], fig["figure"])]
        if hits:
            cited.update((row["name"], row["file"]) for row in hits)
        else:
            unmatched.append(fig)
    uncited = [
        row for row in consts
        if row["name"] not in HELPERS and row["value"] != "?"
        and (row["name"], row["file"]) not in cited
    ]
    unmatched.sort(key=lambda item: (item["file"], item["line"], item["figure"]))
    uncited.sort(key=lambda row: (row["name"], row["file"], row["line"]))
    return unmatched, uncited


def main():
    parser = argparse.ArgumentParser(description="Cross-check doc budgets vs Rust constants.")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--strict", action="store_true",
                        help="exit 1 when unmatched or uncited findings exist")
    args = parser.parse_args()
    unmatched, uncited = report(scan(), collect_figures())
    if args.json:
        payload = {
            "unmatched": [{k: item[k] for k in ("file", "line", "figure", "context")}
                          for item in unmatched],
            "uncited": [{k: row[k] for k in ("name", "value", "file")} for row in uncited],
        }
        json.dump(payload, sys.stdout, ensure_ascii=False)
        sys.stdout.write("\n")
    else:
        for item in unmatched:
            print(f'UNMATCHED {item["file"]}:{item["line"]} "{item["figure"]}" {item["context"]}')
        for row in uncited:
            print(f'UNCITED {row["name"]} {row["value"]} {row["file"]}')
        if not unmatched and not uncited:
            print("0 unmatched, 0 uncited")
    return 1 if args.strict and (unmatched or uncited) else 0


if __name__ == "__main__":
    raise SystemExit(main())
