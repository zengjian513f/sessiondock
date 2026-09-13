#!/usr/bin/env python3
"""Structural checks for the archived migration history.

§7 `### 第N批：` (Chinese numerals 一…九十九) must increase without gaps;
last H3 is `### 下一批的具体入口`; 第十一批+ need 验收/测试 and N项; §6
checkboxes match `^- [( |x)] `; tables, ≤1200-char lines, relative links.
CLI: migration_history_lint.py [--plan PATH] [--json]
"""
# run_validation: skip
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PLAN = ROOT / "MIGRATION_HISTORY.md"
NEXT_H3 = "### 下一批的具体入口"
MAX_LEN = 1200
DIGITS = {ch: i for i, ch in enumerate("一二三四五六七八九", 1)}
SECTION_RE = re.compile(r"^##\s+(\d+)\.")
BATCH_RE = re.compile(r"^### 第([一二三四五六七八九十]+)批：")
CHECK_RE = re.compile(r"^- \[( |x)\] ")
BULLET_RE = re.compile(r"^[-*]\s+")
COUNT_RE = re.compile(r"\d+\s*项")
sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_docs_links  # noqa: E402


def chinese_num(text: str):
    """Parse 一…九十九. Return None when the numeral is not in that range."""
    if text == "十":
        return 10
    if len(text) == 1:
        return DIGITS.get(text)
    if text.startswith("十") and len(text) == 2:
        ones = DIGITS.get(text[1])
        return None if ones is None else 10 + ones
    if text.count("十") != 1:
        return None
    left, right = text.split("十")
    tens = DIGITS.get(left)
    if tens is None:
        return None
    if not right:
        return tens * 10
    ones = DIGITS.get(right)
    return None if ones is None else tens * 10 + ones


def n_cols(line: str) -> int:
    return len(line.strip().strip("|").split("|"))


def note(out: list, line: int, category: str, message: str) -> None:
    out.append({"line": line, "category": category, "message": message})


def lint(path: Path) -> list[dict]:
    out: list[dict] = []
    section = None
    milestone = False
    table_cols = None
    batches: list[tuple] = []
    current = None
    last_h3, last_h3_line = None, 0
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if raw.startswith("|"):
            width = n_cols(raw)
            if table_cols is None:
                table_cols = width
            elif width != table_cols:
                note(out, number, "table", f"expected {table_cols} columns, found {width}")
        else:
            table_cols = None
            if len(raw) > MAX_LEN:
                note(out, number, "line-length", f"{len(raw)} characters (max {MAX_LEN})")
        headed = SECTION_RE.match(raw)
        if headed:
            section, milestone, current = int(headed.group(1)), False, None
            continue
        if raw.startswith("## "):
            section, milestone, current = None, False, None
            continue
        if raw.startswith("### "):
            last_h3, last_h3_line, current = raw.rstrip(), number, None
            if section == 6:
                milestone = True
            match = BATCH_RE.match(raw) if section == 7 else None
            if match:
                batches.append((number, chinese_num(match.group(1)), match.group(1), []))
                current = len(batches) - 1
            continue
        if current is not None:
            batches[current][3].append(raw)
        if section == 6 and milestone and BULLET_RE.match(raw) and not CHECK_RE.match(raw):
            note(out, number, "checkbox", r"must match '^- \[( |x)\] ' exactly")
    if last_h3 != NEXT_H3:
        shown = last_h3 if last_h3 is not None else "(none)"
        note(out, last_h3_line, "next-entry", f"{shown}; expected {NEXT_H3}")
    start = batches[0][1] if batches else None
    for index, (line, num, raw_num, body) in enumerate(batches):
        if num is None:
            note(out, line, "batch-order", f"unparsed Chinese numeral {raw_num}")
        elif start is not None and num != start + index:
            note(out, line, "batch-order", f"expected {start + index}, found {num}")
        if num is None or num < 11:
            continue
        text = "\n".join(body)
        missing = []
        if not any(word in text for word in ("验收", "测试", "检查", "通过")):
            missing.append("验收/测试/检查/通过")
        if not COUNT_RE.search(text):
            missing.append("N项")
        if missing:
            note(out, line, "batch-evidence", f"第{raw_num}批 missing {', '.join(missing)}")
    _, broken = check_docs_links.check_file(path, ROOT, {})
    for item in broken:
        note(out, item["line"], "link", f"{item['target']} — {item['reason']}")
    out.sort(key=lambda item: (item["line"], item["category"]))
    return out


def relpath(path: Path) -> str:
    try:
        return path.resolve().relative_to(ROOT.resolve()).as_posix()
    except ValueError:
        return str(path)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--plan", type=Path, default=DEFAULT_PLAN,
        help="historical migration markdown (default: repo-root MIGRATION_HISTORY.md)",
    )
    parser.add_argument("--json", dest="as_json", action="store_true",
                        help="print findings as JSON")
    args = parser.parse_args(argv)
    plan = args.plan if args.plan.is_absolute() else Path.cwd() / args.plan
    if not plan.is_file():
        print(f"plan file not found: {plan}", file=sys.stderr)
        return 2
    findings = lint(plan)
    rel = relpath(plan)
    if args.as_json:
        json.dump({"plan": rel, "findings": findings}, sys.stdout, ensure_ascii=False)
        sys.stdout.write("\n")
    else:
        for item in findings:
            print(f"{rel}:{item['line']}: {item['category']}: {item['message']}")
        print(f"{len(findings)} finding(s)")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
