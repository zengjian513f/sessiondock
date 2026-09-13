#!/usr/bin/env python3
"""Keep AGENTS.md, docs/validation.md and the test directory consistent.

Checks discovered Python suites and *_contract.mjs are named in AGENTS.md or
docs/validation.md; AGENTS.md test/doc paths and cargo --test names exist;
every docs/*.md is linked from a hub or another doc; utility scripts carry
the runner skip marker.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run_validation import MAIN_RE, ROOT, SKIP_MARK, SKIP_PY

AGENTS = ROOT / "AGENTS.md"
VALIDATION = ROOT / "docs" / "validation.md"
DOCS = ROOT / "docs"
HUBS = (
    ROOT / "README.md",
    AGENTS,
    ROOT / "BACKEND_MIRGRATION_PLAN.md",
    DOCS / "README.md",
)
TEST_RE = re.compile(r"tests/[A-Za-z0-9][A-Za-z0-9_.-]*\.(?:py|mjs|rs)")
CARGO_RE = re.compile(r"--test\s+([A-Za-z_][A-Za-z0-9_]*)(?=\s|$)")
DOC_RE = re.compile(r"docs/[A-Za-z0-9][A-Za-z0-9_.-]*\.md")
LINK_RE = re.compile(r"\[[^\]\n]+\]\(\s*<?([^)\s>#]+)")
SCHEME_RE = re.compile(r"^[a-z][a-z0-9+.-]*:", re.I)
TOOLS = (
    "_check", "_dump", "_summary", "_status", "_ledger", "_diff", "_index",
    "_watch", "_probe", "_gen", "_table", "_reference", "_runs",
)


def add(findings, category, path):
    findings.append({"category": category, "path": path})


def unique(items):
    return list(dict.fromkeys(items))


def discovered_py():
    found = []
    for path in sorted((ROOT / "tests").glob("*.py")):
        if path.name in SKIP_PY:
            continue
        text = path.read_text(encoding="utf-8")
        if not MAIN_RE.search(text) or SKIP_MARK in text.splitlines()[:40]:
            continue
        found.append(path)
    return found


def cargo_exists(name):
    if (ROOT / "tests" / f"{name}.rs").is_file():
        return True
    return any(ROOT.glob(f"crates/*/tests/{name}.rs")) or any(
        ROOT.glob(f"crates/*/tests/{name}/main.rs")
    )


def link_sources():
    paths = []
    for path in list(HUBS) + sorted(DOCS.glob("*.md")):
        if path.is_file() and path not in paths:
            paths.append(path)
    return paths


def linked_doc_names():
    names = set()
    docs_root = DOCS.resolve()
    for src in link_sources():
        for match in LINK_RE.finditer(src.read_text(encoding="utf-8")):
            target = match.group(1).strip()
            if SCHEME_RE.match(target):
                continue
            dest = (src.parent / target).resolve()
            try:
                rel = dest.relative_to(docs_root)
            except ValueError:
                continue
            if len(rel.parts) == 1 and rel.suffix.lower() == ".md":
                names.add(rel.name)
    return names


def collect():
    findings = []
    agents = AGENTS.read_text(encoding="utf-8")
    validation = VALIDATION.read_text(encoding="utf-8") if VALIDATION.is_file() else ""
    named = agents + "\n" + validation

    for path in discovered_py():
        if path.name not in named:
            add(findings, "undocumented", str(path.relative_to(ROOT)))
    for path in sorted((ROOT / "tests").glob("*_contract.mjs")):
        if path.name not in named:
            add(findings, "undocumented", str(path.relative_to(ROOT)))

    for rel in unique(TEST_RE.findall(agents)):
        if not (ROOT / rel).is_file():
            add(findings, "missing-test", rel)
    for name in unique(CARGO_RE.findall(agents)):
        if not cargo_exists(name):
            add(findings, "missing-cargo-test", name)
    for rel in unique(DOC_RE.findall(agents)):
        if not (ROOT / rel).is_file():
            add(findings, "missing-doc", rel)

    linked = linked_doc_names()
    for path in sorted(DOCS.glob("*.md")):
        if path.name not in linked:
            add(findings, "unlinked-doc", str(path.relative_to(ROOT)))

    for path in sorted((ROOT / "tests").glob("*.py")):
        if not any(path.stem.endswith(suffix) for suffix in TOOLS):
            continue
        if path.name in validation:
            continue
        lines = path.read_text(encoding="utf-8").splitlines()[:40]
        if SKIP_MARK not in lines:
            add(findings, "missing-skip", str(path.relative_to(ROOT)))
    return findings


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="print findings as JSON")
    args = parser.parse_args(argv)
    findings = collect()
    if args.json:
        json.dump({"findings": findings}, sys.stdout, ensure_ascii=False)
        sys.stdout.write("\n")
    else:
        for item in findings:
            print(f"{item['category']}: {item['path']}")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
