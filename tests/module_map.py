#!/usr/bin/env python3
"""Map the Rust crate src trees as nested Markdown for onboarding.

Walks every member's src tree from the workspace Cargo.toml.
Prints Markdown to stdout. With --write, writes
docs/module-map.md as the last step. Requires Python 3.11+ (stdlib tomllib).
"""
# run_validation: skip
from __future__ import annotations

import argparse
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "docs" / "module-map.md"


def workspace_crates():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
    excluded = {path for pattern in workspace.get("exclude", []) for path in ROOT.glob(pattern)}
    crates = []
    seen = set()
    for pattern in workspace["members"]:
        for member in sorted(ROOT.glob(pattern)):
            if member in excluded or member in seen:
                continue
            seen.add(member)
            manifest = tomllib.loads((member / "Cargo.toml").read_text())
            crates.append((manifest["package"]["name"], (member / "src").relative_to(ROOT).as_posix()))
    return crates


MOD_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
ATTR_RE = re.compile(r"^\s*#!?\[")
INTRO = """# Module map

This file is produced by `tests/module_map.py`. It maps every `.rs` file under
the crate `src` trees below with the first module doc line, line count and
`mod` declarations. Regenerate:

```sh
python3 tests/module_map.py --write
```
"""


def first_doc(rows: list[str]) -> str | None:
    inner = other = None
    for raw in rows:
        stripped = raw.strip()
        if not stripped or ATTR_RE.match(stripped):
            continue
        if stripped.startswith("//!"):
            text = stripped[3:].strip()
            if text and inner is None:
                inner = text
            continue
        if stripped.startswith("//"):
            n = 3 if stripped.startswith("///") else 2
            text = stripped[n:].strip()
            if text and other is None:
                other = text
            continue
        break
    return inner or other


def parse_file(path: Path, src: Path) -> dict:
    text = path.read_text(encoding="utf-8")
    rows = text.splitlines()
    return {
        "rel": path.relative_to(src).as_posix(),
        "doc": first_doc(rows),
        "lines": len(rows),
        "mods": [m.group(1) for raw in rows if (m := MOD_RE.match(raw))],
    }


def collect(src: Path) -> list[dict]:
    if not src.is_dir():
        return []
    return [parse_file(path, src) for path in sorted(src.rglob("*.rs")) if path.is_file()]


def nest(items: list[dict]) -> dict:
    root: dict = {}
    for item in items:
        node = root
        parts = item["rel"].split("/")
        for part in parts[:-1]:
            node = node.setdefault(part, {})
        node[parts[-1]] = item
    return root


def is_file(node) -> bool:
    return isinstance(node, dict) and "lines" in node


def sort_names(node: dict) -> list[str]:
    def key(name: str):
        file = is_file(node[name])
        stem = name[:-3] if file and name.endswith(".rs") else name
        return (stem.lower(), 0 if file else 1)

    return sorted(node, key=key)


def file_line(name: str, item: dict) -> str:
    doc = item["doc"] or "(no module doc)"
    return f"`{name}` — {doc} ({item['lines']} lines)"


def render_tree(node: dict, depth: int) -> list[str]:
    lines = []
    pad = "  " * depth
    for name in sort_names(node):
        value = node[name]
        if is_file(value):
            lines.append(f"{pad}- {file_line(name, value)}")
            if value["mods"]:
                mods = ", ".join(f"`{mod}`" for mod in value["mods"])
                lines.append(f"{pad}  - mods: {mods}")
        else:
            lines.append(f"{pad}- `{name}/`")
            lines.extend(render_tree(value, depth + 1))
    return lines


def render_crate(title: str, rel: str, items: list[dict]) -> list[str]:
    undocumented = sum(1 for item in items if not item["doc"])
    n_lines = sum(item["lines"] for item in items)
    summary = f"`{rel}`: {len(items)} files, {n_lines} lines, {undocumented} undocumented."
    return [f"## {title}", "", summary, ""] + render_tree(nest(items), 0) + [""]


def render(sections: list[tuple[str, str, list[dict]]]) -> str:
    parts = [INTRO.rstrip(), ""]
    for title, rel, items in sections:
        parts.extend(render_crate(title, rel, items))
    return "\n".join(parts).rstrip() + "\n"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="write docs/module-map.md")
    args = parser.parse_args(argv)
    sections = [(title, rel, collect(ROOT / rel)) for title, rel in workspace_crates()]
    markdown = render(sections)
    try:
        sys.stdout.write(markdown)
    except BrokenPipeError:
        return 0
    if args.write:
        OUT.write_text(markdown, encoding="utf-8")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BrokenPipeError:
        raise SystemExit(0)
