#!/usr/bin/env python3
"""Verify relative Markdown links and GitHub-style heading anchors.

Default scan from the repo root (this file's parent): README.md, AGENTS.md,
TODO.md, docs/*.md, crates/ptyhost-client/README.md and
reference/README.md. Skip fenced code blocks and http(s)/mailto: targets.
Relative targets resolve against the linking file; missing files or heading
anchors are reported. Duplicate headings get -1, -2 suffixes.

CLI: check_docs_links.py [PATHS…] [--json] [--quiet]
"""
# run_validation: skip
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]
DEFAULTS = (
    "README.md", "AGENTS.md", "TODO.md", "docs/*.md",
    "crates/ptyhost-client/README.md", "reference/README.md",
)
FENCE_RE = re.compile(r"^(\s{0,3})(`{3,}|~{3,})")
HEADING_RE = re.compile(r"^(#{1,6})\s+(.+?)\s*$")
LINK_RE = re.compile(
    r"\[(?:[^\]\n]+)\]\(\s*(<[^>\n]+>|[^)\s]+)(?:\s+(?:\"[^\"]*\"|'[^']*'))?\s*\)"
)
PUNCT_RE = re.compile(r"[^\w\s-]", re.UNICODE)
MD_LINK_RE = re.compile(r"\[([^\]]+)\]\([^)]*\)")
INLINE_CODE_RE = re.compile(r"`[^`\n]*`")


def default_paths(root: Path) -> list[Path]:
    found = []
    for pattern in DEFAULTS:
        if "*" in pattern:
            found.extend(sorted(p for p in root.glob(pattern) if p.is_file()))
        else:
            path = root / pattern
            if path.is_file():
                found.append(path)
    return found


def visible_lines(text: str):
    closing = None
    for number, line in enumerate(text.splitlines(), 1):
        match = FENCE_RE.match(line)
        if match:
            marker = match.group(2)
            if closing is None:
                closing = (marker[0], len(marker))
                continue
            if marker[0] == closing[0] and len(marker) >= closing[1]:
                closing = None
                continue
        if closing is None:
            yield number, line


def slugify(text: str) -> str:
    text = PUNCT_RE.sub("", MD_LINK_RE.sub(r"\1", text).lower().strip())
    return text.replace(" ", "-")


def headings_in(path: Path) -> set[str]:
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return set()
    used: set[str] = set()
    for _, line in visible_lines(text):
        match = HEADING_RE.match(line)
        if not match:
            continue
        plain = re.sub(r"\s+#+\s*$", "", match.group(2).strip())
        if not plain:
            continue
        base = slugify(plain)
        slug, n = base, 1
        while slug in used:
            slug = f"{base}-{n}"
            n += 1
        used.add(slug)
    return used


def relpath(path: Path, root: Path) -> str:
    try:
        return path.resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return str(path)


def check_file(path: Path, root: Path, slug_cache: dict) -> tuple[int, list[dict]]:
    broken, checked = [], 0
    for number, line in visible_lines(path.read_text(encoding="utf-8")):
        for match in LINK_RE.finditer(INLINE_CODE_RE.sub("", line)):
            raw = match.group(1)
            href = raw[1:-1].strip() if raw.startswith("<") and raw.endswith(">") else raw
            if href.lower().startswith(("http://", "https://", "mailto:")):
                continue
            checked += 1
            href_u = unquote(href.strip())
            if "#" in href_u:
                dest_raw, anchor = href_u.split("#", 1)
            else:
                dest_raw, anchor = href_u, None
            dest = path.resolve() if not dest_raw else (path.parent / dest_raw).resolve()
            reason = None
            if dest_raw == "" and not anchor:
                reason = "empty target"
            elif dest_raw and not dest.exists():
                reason = "file not found"
            elif anchor is not None:
                if not dest.is_file():
                    reason = "target is not a file"
                else:
                    slugs = slug_cache.setdefault(dest, headings_in(dest))
                    if anchor not in slugs:
                        reason = f"unknown anchor #{anchor}"
            if reason:
                broken.append({
                    "file": relpath(path, root), "line": number,
                    "target": href, "reason": reason,
                })
    return checked, broken


def resolve_inputs(paths: list[str], root: Path) -> list[Path]:
    if not paths:
        return default_paths(root)
    out = []
    for item in paths:
        path = Path(item)
        out.append(path if path.is_absolute() else Path.cwd() / path)
    return out


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", help="Markdown files (default: repo docs)")
    parser.add_argument("--json", action="store_true", help="print JSON summary")
    parser.add_argument("--quiet", action="store_true", help="suppress per-link lines")
    args = parser.parse_args(argv)
    slug_cache: dict[Path, set[str]] = {}
    checked, broken = 0, []
    for path in resolve_inputs(args.paths, ROOT):
        if not path.is_file():
            broken.append({
                "file": relpath(path, ROOT), "line": 0,
                "target": str(path), "reason": "scan target is not a file",
            })
            continue
        count, items = check_file(path, ROOT, slug_cache)
        checked += count
        broken.extend(items)
    if args.json:
        json.dump({"checked": checked, "broken": broken}, sys.stdout, ensure_ascii=False)
        sys.stdout.write("\n")
    else:
        if not args.quiet:
            for item in broken:
                print(f"{item['file']}:{item['line']}: {item['target']} — {item['reason']}")
        print(f"{len(broken)} broken link(s)")
    return 1 if broken else 0


if __name__ == "__main__":
    sys.exit(main())
