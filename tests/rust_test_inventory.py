#!/usr/bin/env python3
"""Per-target Rust test counts from a cargo test log or the source tree.

--log FILE parses Running unittests/tests and Doc-tests headers plus the
following `test result:` line; tests/<name>.rs is assigned by which crate
directory contains that file. --source counts #[test]/#[tokio::test]/
#[ignore under crates/*/src and crates/*/tests. Prints a Markdown table,
per-crate totals, and a concise Chinese release-note line.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CRATES = ("sessiondock", "ptyhost", "ptyhost-client")
ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
RUN_RE = re.compile(
    r"^\s*Running (?:unittests (src/(?:lib|main)\.rs)|tests/([^/]+)\.rs) \(([^)]+)\)")
DOC_RE = re.compile(r"^\s*Doc-tests (\S+)")
RESULT_RE = re.compile(
    r"test result:\s+\S+\s+(\d+) passed;\s+(\d+) failed;\s+(\d+) ignored")
TEST_RE = re.compile(r"^\s*#\[(?:tokio::)?test\b")
IGNORE_RE = re.compile(r"^\s*#\[ignore\b")
UNIT = {"src/lib.rs": "lib", "src/main.rs": "bin"}
KEYS = ("crate", "target", "passed", "failed", "ignored")


def die(msg):
    print(msg, file=sys.stderr)
    raise SystemExit(2)


def crate_from_test(name):
    hits = [c for c in CRATES if (ROOT / "crates" / c / "tests" / f"{name}.rs").is_file()]
    if len(hits) != 1:
        die(f"cannot resolve tests/{name}.rs to one crate: {hits or 'none'}")
    return hits[0]


def sort_rows(rows):
    order = {c: i for i, c in enumerate(CRATES)}
    rank = {"lib": 0, "bin": 1, "doc": 3}
    return sorted(rows, key=lambda r: (order.get(r["crate"], 99),
                                       rank.get(r["target"], 2), r["target"]))


def parse_log(raw: Path):
    path = next((c for c in (raw, ROOT / raw, Path.cwd() / raw) if c.is_file()), None)
    if path is None:
        die(f"log not found: {raw}")
    rows, pending = [], None
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = ANSI_RE.sub("", line)
        hit = RUN_RE.search(line)
        if hit:
            unit, name, binary = hit.group(1), hit.group(2), hit.group(3)
            crate = (Path(binary).name.rsplit("-", 1)[0].replace("_", "-")
                     if unit else crate_from_test(name))
            pending = (crate, UNIT[unit] if unit else name)
            continue
        hit = DOC_RE.search(line)
        if hit:
            pending = (hit.group(1).replace("_", "-"), "doc")
            continue
        hit = RESULT_RE.search(line)
        if hit and pending:
            rows.append(dict(zip(KEYS, (*pending, *map(int, hit.groups())))))
            pending = None
    return rows


def src_target(crate, path: Path):
    rel = path.relative_to(ROOT / "crates" / crate)
    if rel.parts[0] == "tests":
        return rel.stem if len(rel.parts) == 2 else None
    if rel.parts[0] != "src":
        return None
    if rel.parts[1] == "main.rs":
        return "bin"
    return "lib" if (ROOT / "crates" / crate / "src" / "lib.rs").is_file() else "bin"


def parse_source():
    counts = defaultdict(lambda: [0, 0])
    for crate in CRATES:
        for folder in ("src", "tests"):
            base = ROOT / "crates" / crate / folder
            if not base.is_dir():
                continue
            for path in base.rglob("*.rs"):
                target = src_target(crate, path)
                if not target:
                    continue
                for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
                    counts[(crate, target)][0] += bool(TEST_RE.match(line))
                    counts[(crate, target)][1] += bool(IGNORE_RE.match(line))
    return [dict(zip(KEYS, (c, t, n, 0, ign))) for (c, t), (n, ign) in counts.items()]


def summed(rows):
    out = {}
    for item in rows:
        slot = out.setdefault(item["crate"], {"passed": 0, "failed": 0, "ignored": 0})
        for key in slot:
            slot[key] += item[key]
    return out


def pick(rows, crate, pred):
    return sum(r["passed"] for r in rows if r["crate"] == crate and pred(r["target"]))


def one_liner(rows):
    integ = lambda t: t not in {"lib", "bin", "doc"}
    return (f"server lib {pick(rows, 'sessiondock', lambda t: t == 'lib')}、"
            f"HTTP系列{pick(rows, 'sessiondock', integ)}、"
            f"host{pick(rows, 'ptyhost', lambda t: t in {'lib', 'bin'})}+"
            f"{pick(rows, 'ptyhost', integ)}、"
            f"client{pick(rows, 'ptyhost-client', lambda t: t != 'doc')}+"
            f"doc{pick(rows, 'ptyhost-client', lambda t: t == 'doc')}")


def render(rows, source=False):
    rows, tot = sort_rows(rows), summed(rows)
    if source:
        headers = ("Crate", "Target", "Tests", "Ignored")
        cell = lambda r, t=False: (r["crate"], "(total)" if t else r["target"],
                                   str(r["passed"]), str(r["ignored"]))
    else:
        headers = ("Crate", "Target", "Passed", "Failed", "Ignored")
        cell = lambda r, t=False: (r["crate"], "(total)" if t else r["target"],
                                   str(r["passed"]), str(r["failed"]), str(r["ignored"]))
    body, last = [], None
    for item in rows:
        if last and item["crate"] != last:
            body.append(cell({"crate": last, **tot[last]}, True))
        body.append(cell(item))
        last = item["crate"]
    if last:
        body.append(cell({"crate": last, **tot[last]}, True))
    lines = (["counted in source"] if source else [])
    lines += ["| " + " | ".join(headers) + " |",
              "| " + " | ".join("---" for _ in headers) + " |"]
    lines += ["| " + " | ".join(row) + " |" for row in body]
    return "\n".join(lines + ["", one_liner(rows)]), tot


def payload(mode, rows, tot):
    rows = sort_rows(rows)
    return {"mode": mode, "rows": [{k: r[k] for k in KEYS} for r in rows],
            "totals": [{"crate": c, **tot[c]} for c in CRATES if c in tot],
            "summary": one_liner(rows)}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--log", type=Path, metavar="FILE")
    group.add_argument("--source", action="store_true")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    mode = "source" if args.source else "log"
    rows = parse_source() if args.source else parse_log(args.log)
    text, tot = render(rows, source=args.source)
    if args.json:
        print(json.dumps(payload(mode, rows, tot), ensure_ascii=False, indent=2))
    else:
        print(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
