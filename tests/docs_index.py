#!/usr/bin/env python3
"""Index docs/*.md as Markdown tables (title, summary, batches, lines).

Prints the index to stdout. With --write, also writes docs/README.md as
the last step. The generated README is excluded from the index.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"
OUT = DOCS / "README.md"
CLIP = 240
FENCE_RE = re.compile(r"^(\s{0,3})(`{3,}|~{3,})")
H1_RE = re.compile(r"^#\s+(.+?)\s*$")
HEADING_RE = re.compile(r"^#{1,6}\s+")
BATCH_RE = re.compile(r"batch\s+(\d+)|第[一二三四五六七八九十]+批", re.I)
LINK_RE = re.compile(r"!?\[([^\]]*)\]\([^)]*\)")
MIGRATION = {
    "migration.md", "native-input.md", "media-parity.md",
    "performance.md", "append-cache.md",
}
PROCESS = {"delegation.md", "validation.md"}
ORDER = ("合同与设计", "迁移记录", "流程")
INTRO = """# 文档索引

本文件由 `tests/docs_index.py` 生成，列出 `docs/*.md` 的标题、首段摘要、批次与行数（不含本文件）。重新生成：

```sh
python3 tests/docs_index.py --write
```
"""


def visible_lines(text: str):
    closing = None
    for line in text.splitlines():
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
            yield line


def plain(text: str) -> str:
    return LINK_RE.sub(r"\1", text)


def classify(name: str) -> str:
    if name in MIGRATION:
        return "迁移记录"
    return "流程" if name in PROCESS else "合同与设计"


def title_of(lines: list[str], fallback: str) -> str:
    for line in lines:
        match = H1_RE.match(line)
        if match:
            return plain(re.sub(r"\s+#+\s*$", "", match.group(1).strip()))
    return fallback


def first_paragraph(lines: list[str]) -> str:
    buf: list[str] = []
    started = False
    for line in lines:
        stripped = line.strip()
        if not stripped or HEADING_RE.match(line):
            if started:
                break
            continue
        started = True
        buf.append(stripped)
    text = plain(" ".join(buf))
    return text if len(text) <= CLIP else text[:CLIP]


def batches_of(text: str) -> str:
    found = []
    for match in BATCH_RE.finditer(text):
        found.append(match.group(1) or match.group(0))
    return ", ".join(dict.fromkeys(found))


def cell(text: str) -> str:
    return text.replace("|", "\\|").replace("\n", " ")


def collect() -> list[dict]:
    rows = []
    for path in sorted(DOCS.glob("*.md")):
        if path.name == OUT.name:
            continue
        text = path.read_text(encoding="utf-8")
        vis = list(visible_lines(text))
        rows.append({
            "name": path.name,
            "title": title_of(vis, path.stem),
            "summary": first_paragraph(vis),
            "batches": batches_of(text),
            "lines": len(text.splitlines()),
            "section": classify(path.name),
        })
    return rows


def render(rows: list[dict]) -> str:
    parts = [INTRO.rstrip(), ""]
    for section in ORDER:
        items = [row for row in rows if row["section"] == section]
        if not items:
            continue
        parts.append(f"## {section}")
        parts.append("")
        parts.append("| Doc | Title | Summary | Batches | Lines |")
        parts.append("| --- | --- | --- | --- | --- |")
        for row in items:
            doc = f"[{row['name']}]({row['name']})"
            parts.append("| " + " | ".join(cell(value) for value in (
                doc, row["title"], row["summary"], row["batches"], str(row["lines"]),
            )) + " |")
        parts.append("")
    return "\n".join(parts).rstrip() + "\n"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="write docs/README.md")
    args = parser.parse_args(argv)
    markdown = render(collect())
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
