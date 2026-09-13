#!/usr/bin/env python3
"""Summarize the archived Python-to-Rust migration history.

Reads section 6 milestone checkboxes (M0–M8 and 第二阶段) and section 7
batch headings. Checkboxes outside section 6 are ignored. Default plan path
is resolved from this script, not the process cwd.
"""
# run_validation: skip
# Historical utility only; current work lives in TODO.md.
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PLAN = ROOT / "MIGRATION_HISTORY.md"
CHECK_RE = re.compile(r"^-\s+\[([ xX])\]\s*(.*)$")
HEADING_RE = re.compile(r"^###\s+(.+?)\s*$")
BATCH_RE = re.compile(r"^第[一二三四五六七八九十百零〇两\d]+批")
SECTION_RE = re.compile(r"^##\s+(\d+)\.")
NEXT_BATCH = "下一批"
CLIP = 100


def clip(text: str) -> str:
    text = " ".join(text.split())
    return text if len(text) <= CLIP else text[:CLIP] + "..."


def percent(done: int, total: int) -> str:
    return f"{0 if total == 0 else round(100 * done / total)}%"


def parse(path: Path) -> dict:
    milestones: list[dict] = []
    current = None
    pending = None
    batches: list[str] = []
    section = None
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.rstrip()
        numbered = SECTION_RE.match(line)
        if numbered:
            section = int(numbered.group(1))
            current = None
            pending = None
            continue
        if line.startswith("## "):
            section = None
            current = None
            pending = None
            continue
        if section == 6:
            heading = HEADING_RE.match(line)
            if heading:
                current = {
                    "name": heading.group(1).strip(),
                    "done": 0,
                    "total": 0,
                    "unchecked": [],
                }
                milestones.append(current)
                pending = None
                continue
            if current is None:
                continue
            checked = CHECK_RE.match(line)
            if checked:
                text = checked.group(2).strip()
                done = checked.group(1).lower() == "x"
                current["total"] += 1
                pending = {"done": done, "text": text}
                if done:
                    current["done"] += 1
                else:
                    current["unchecked"].append(text)
                    pending["index"] = len(current["unchecked"]) - 1
                continue
            if pending is not None and line[:1] in " \t" and line.strip():
                pending["text"] = pending["text"] + " " + line.strip()
                if not pending["done"]:
                    current["unchecked"][pending["index"]] = pending["text"]
                continue
            if not line.strip():
                pending = None
            continue
        if section == 7:
            heading = HEADING_RE.match(line)
            if not heading:
                continue
            name = heading.group(1).strip()
            if name.startswith(NEXT_BATCH):
                section = None
                continue
            if BATCH_RE.match(name):
                batches.append(name)
    for item in milestones:
        item["unchecked"] = [clip(text) for text in item["unchecked"]]
    done = sum(item["done"] for item in milestones)
    total = sum(item["total"] for item in milestones)
    return {
        "milestones": milestones,
        "total": {"done": done, "total": total},
        "latest_batch": batches[-1] if batches else "",
        "batches": len(batches),
    }


def print_table(data: dict) -> None:
    print("| 里程碑 | 完成 | 总数 | 进度 |")
    print("| --- | --- | --- | --- |")
    for item in data["milestones"]:
        print(
            f"| {item['name']} | {item['done']} | {item['total']} | "
            f"{percent(item['done'], item['total'])} |"
        )
    total = data["total"]
    print(
        f"| TOTAL | {total['done']} | {total['total']} | "
        f"{percent(total['done'], total['total'])} |"
    )


def print_unchecked(data: dict) -> None:
    for item in data["milestones"]:
        if not item["unchecked"]:
            continue
        print(item["name"])
        for text in item["unchecked"]:
            print(f"  - {text}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--json", dest="as_json", action="store_true",
        help="print the same summary as JSON",
    )
    parser.add_argument(
        "--unchecked-only", action="store_true",
        help="print only unchecked items grouped by milestone",
    )
    parser.add_argument(
        "--plan", type=Path, default=DEFAULT_PLAN,
        help="historical migration markdown (default: repo-root MIGRATION_HISTORY.md)",
    )
    args = parser.parse_args()
    plan = args.plan if args.plan.is_absolute() else Path.cwd() / args.plan
    if not plan.is_file():
        print(f"plan file not found: {plan}", file=sys.stderr)
        return 2
    data = parse(plan)
    if args.as_json:
        print(json.dumps(data, ensure_ascii=False))
        return 0
    if not args.unchecked_only:
        print_table(data)
        print()
    print_unchecked(data)
    if not args.unchecked_only:
        print()
        print(f"最新批次: {data['latest_batch']}")
        print(f"批次: {data['batches']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
