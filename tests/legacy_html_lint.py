#!/usr/bin/env python3
"""Static checks for served and frozen legacy HTML.

legacy-web/*.html and reference/legacy-web/*.html: duplicate ids,
<script src>/<link href> files missing under that tree (ignore http(s)
and data: URLs), inline on* handlers, index.html placeholders
__AGENTHUB_MODE__/__AGENTHUB_HOSTNAME__/__AGENTHUB_ASSET_VERSION__,
<meta name="agenthub-capabilities">, <img> without alt, and
<a target="_blank"> without rel=noopener.
CLI: legacy_html_lint.py [--json] [--strict]
"""
# run_validation: skip
from __future__ import annotations

import argparse
import json
import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlparse

ROOT = Path(__file__).resolve().parents[1]
TREES = (ROOT / "legacy-web", ROOT / "reference" / "legacy-web")
PLACEHOLDERS = (
    "__AGENTHUB_MODE__",
    "__AGENTHUB_HOSTNAME__",
    "__AGENTHUB_ASSET_VERSION__",
)
KINDS = (
    "duplicate-id", "missing-file", "inline-handler", "missing-placeholder",
    "missing-capabilities", "img-alt", "target-blank",
)
FATAL = {"duplicate-id", "missing-file"}
REMOTE = ("http://", "https://", "data:", "//")


def rel(path: Path) -> str:
    return path.resolve().relative_to(ROOT).as_posix()


def note(out, kind, path, line, msg=""):
    out.append({"kind": kind, "file": rel(path), "line": line, "message": msg})


def local_path(url: str) -> str | None:
    text = (url or "").strip()
    if not text or text.lower().startswith(REMOTE):
        return None
    parsed = urlparse(text)
    if parsed.scheme or parsed.netloc:
        return None
    path = unquote(parsed.path)
    return path if path and not path.endswith("/") else None


class Page(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.ids: list[tuple[int, str]] = []
        self.refs: list[tuple[int, str]] = []
        self.handlers: list[tuple[int, str]] = []
        self.imgs: list[int] = []
        self.blanks: list[int] = []
        self.caps: int | None = None

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)

    def handle_starttag(self, tag, attrs):
        line = self.getpos()[0]
        amap = {key: value or "" for key, value in attrs}
        ident = amap.get("id")
        if ident:
            self.ids.append((line, ident))
        if tag == "script":
            src = local_path(amap.get("src", ""))
            if src:
                self.refs.append((line, src))
        elif tag == "link":
            href = local_path(amap.get("href", ""))
            if href:
                self.refs.append((line, href))
        elif tag == "img" and "alt" not in amap:
            self.imgs.append(line)
        elif tag == "a" and amap.get("target", "").lower() == "_blank":
            if "noopener" not in amap.get("rel", "").lower().split():
                self.blanks.append(line)
        elif tag == "meta" and amap.get("name") == "agenthub-capabilities":
            self.caps = line
        for key, _ in attrs:
            if key.startswith("on") and key[2:].isalpha():
                self.handlers.append((line, key))


def exists_under(tree: Path, html: Path, src: str) -> bool:
    base = tree if src.startswith("/") else html.parent
    try:
        resolved = (base / src.lstrip("/")).resolve()
        resolved.relative_to(tree.resolve())
        return resolved.is_file()
    except (OSError, ValueError):
        return False


def check_file(path: Path, tree: Path, out: list) -> None:
    text = path.read_text(encoding="utf-8", errors="replace")
    page = Page()
    page.feed(text)
    page.close()
    seen: dict[str, int] = {}
    for line, ident in page.ids:
        if ident in seen:
            note(out, "duplicate-id", path, line, f"{ident} first at {seen[ident]}")
        else:
            seen[ident] = line
    for line, src in page.refs:
        if not exists_under(tree, path, src):
            note(out, "missing-file", path, line, src)
    for line, key in page.handlers:
        note(out, "inline-handler", path, line, key)
    if path.name == "index.html":
        for token in PLACEHOLDERS:
            if token not in text:
                note(out, "missing-placeholder", path, 1, token)
    if page.caps is None:
        note(out, "missing-capabilities", path, 1,
             'meta name="agenthub-capabilities"')
    for line in page.imgs:
        note(out, "img-alt", path, line, "<img> missing alt")
    for line in page.blanks:
        note(out, "target-blank", path, line,
             'target="_blank" without rel=noopener')


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="print JSON report")
    parser.add_argument(
        "--strict", action="store_true",
        help="exit 1 on any finding, not only duplicate ids / missing files",
    )
    args = parser.parse_args(argv)
    findings: list[dict] = []
    for tree in TREES:
        if not tree.is_dir():
            continue
        for path in sorted(p for p in tree.glob("*.html") if p.is_file()):
            check_file(path, tree, findings)
    if args.json:
        json.dump({"findings": findings}, sys.stdout, ensure_ascii=False, indent=2)
        sys.stdout.write("\n")
    elif not findings:
        print("0 findings", flush=True)
    else:
        for kind in KINDS:
            rows = [item for item in findings if item["kind"] == kind]
            if not rows:
                continue
            print(kind, flush=True)
            for item in rows:
                extra = f" {item['message']}" if item["message"] else ""
                print(f"  {item['file']}:{item['line']}{extra}", flush=True)
    fatal = any(item["kind"] in FATAL for item in findings)
    return 1 if fatal or (args.strict and findings) else 0


if __name__ == "__main__":
    raise SystemExit(main())
