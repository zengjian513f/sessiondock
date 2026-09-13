#!/usr/bin/env python3
"""Inventory numeric limit constants in the Rust server sources.

Scans crates/sessiondock/src and crates/ptyhost-client/src for
`const NAME: usize|u64|u32|u16|u8|i64|Duration = EXPR`. Skips paths
containing /tests, files ending in _tests.rs, and #[cfg(test)] mod tests.
CLI: budget_table.py [--json] [--filter SUBSTR] [--sort name|value|file]
"""
# run_validation: skip
from __future__ import annotations

import argparse
import ast
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCAN = ("crates/sessiondock/src", "crates/ptyhost-client/src")
CONST_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*"
    r"(usize|u64|u32|u16|u8|i64|Duration)\s*=\s*(.*)$")
MOD_TESTS_RE = re.compile(r"^mod\s+tests\s*[;{]")
DUR_RE = re.compile(r"^Duration\s*::\s*from_(secs|millis)\s*\((.*)\)\s*$", re.S)
HELPERS = ("KIB", "MIB", "GIB")
BYTE_HINT = ("BYTES", "LIMIT", "MIB", "SIZE", "BUDGET", "LEN")
UNITS = ((1024 ** 3, "GiB"), (1024 ** 2, "MiB"), (1024, "KiB"))


def _div(left, right):
    if right == 0:
        raise ZeroDivisionError
    if isinstance(left, int) and isinstance(right, int):
        return left // right
    return left / right


BINOPS = {ast.Add: lambda a, b: a + b, ast.Sub: lambda a, b: a - b,
          ast.Mult: lambda a, b: a * b, ast.Div: _div}


def visible_lines(text):
    lines = text.splitlines()
    index, total = 0, len(lines)
    while index < total:
        if lines[index].strip() == "#[cfg(test)]":
            nxt = index + 1
            while nxt < total and not lines[nxt].strip():
                nxt += 1
            body = lines[nxt].strip() if nxt < total else ""
            if MOD_TESTS_RE.match(body):
                if "{" not in body:
                    index = nxt + 1
                    continue
                depth = 0
                for cursor in range(nxt, total):
                    depth += lines[cursor].count("{") - lines[cursor].count("}")
                    if depth <= 0:
                        index = cursor + 1
                        break
                else:
                    index = total
                continue
        yield index + 1, lines[index]
        index += 1


def collect_consts(text, rel):
    rows, found, index = list(visible_lines(text)), [], 0
    while index < len(rows):
        lineno, line = rows[index]
        match = CONST_RE.match(line)
        if not match:
            index += 1
            continue
        parts, end = [match.group(3)], index
        while ";" not in "".join(parts) and end + 1 < len(rows):
            end += 1
            parts.append(rows[end][1])
        chunks, cursor = [], index - 1
        while cursor >= 0 and rows[cursor][1].strip().startswith("///"):
            chunks.append(rows[cursor][1].strip()[3:].strip())
            cursor -= 1
        chunks.reverse()
        found.append({
            "name": match.group(1), "file": rel, "line": lineno, "doc": " ".join(chunks),
            "raw": " ".join(part.strip() for part in parts).split(";", 1)[0].strip(),
        })
        index = end + 1
    return found


def ast_value(node):
    if isinstance(node, ast.Expression):
        return ast_value(node.body)
    if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)):
        return node.value
    if isinstance(node, ast.UnaryOp) and isinstance(node.op, (ast.UAdd, ast.USub)):
        value = ast_value(node.operand)
        return value if isinstance(node.op, ast.UAdd) else -value
    if isinstance(node, ast.BinOp) and type(node.op) in BINOPS:
        return BINOPS[type(node.op)](ast_value(node.left), ast_value(node.right))
    raise ValueError("unsafe")


def eval_expr(expr, helpers):
    text, unit = expr.strip(), None
    duration = DUR_RE.match(text)
    if duration:
        unit = "s" if duration.group(1) == "secs" else "ms"
        text = duration.group(2).strip()
    for name, value in helpers.items():
        text = re.sub(rf"\b{name}\b", str(value), text)
    try:
        value = ast_value(ast.parse(re.sub(r"(?<=\d)_(?=\d)", "", text), mode="eval"))
    except (SyntaxError, ValueError, ZeroDivisionError, RecursionError):
        return "?", unit
    if isinstance(value, float) and value.is_integer():
        value = int(value)
    return value, unit


def humanize(name, value, unit, doc):
    if value == "?" or unit:
        return "?" if value == "?" else f"{value}{unit}"
    blob = f"{name} {doc}".upper()
    if not isinstance(value, int) or value < 0 or not any(h in blob for h in BYTE_HINT):
        return str(value)
    for size, label in UNITS:
        if value % size == 0:
            return f"{value // size} {label}"
    return str(value)


def scan():
    items = []
    for base in SCAN:
        root = ROOT / base
        if not root.is_dir():
            continue
        for path in sorted(root.rglob("*.rs")):
            rel = path.relative_to(ROOT).as_posix()
            if "/tests" in f"/{rel}" or rel.endswith("_tests.rs"):
                continue
            try:
                rows = collect_consts(path.read_text(encoding="utf-8"), rel)
            except (OSError, UnicodeDecodeError):
                continue
            helpers = {}
            pending = {row["name"]: row["raw"] for row in rows if row["name"] in HELPERS}
            for name in HELPERS:
                if name not in pending:
                    continue
                value, _unit = eval_expr(pending[name], helpers)
                if value != "?":
                    helpers[name] = value
            for row in rows:
                value, unit = eval_expr(row["raw"], helpers)
                row["value"] = value
                row["human"] = humanize(row["name"], value, unit, row["doc"])
                items.append(row)
    return items


def sort_rows(rows, key):
    if key == "name":
        return sorted(rows, key=lambda row: (row["name"], row["file"], row["line"]))
    if key == "value":
        return sorted(rows, key=lambda row: (
            row["value"] == "?", 0 if row["value"] == "?" else row["value"],
            row["file"], row["line"]))
    return sorted(rows, key=lambda row: (row["file"], row["line"]))


def markdown(rows):
    def cell(text):
        return str(text).replace("|", "\\|").replace("\n", " ")
    lines = ["| Constant | Value | Human | File:line | Doc |", "| --- | --- | --- | --- | --- |"]
    for row in rows:
        loc = f"{row['file']}:{row['line']}"
        lines.append(
            f"| {cell(row['name'])} | {cell(row['value'])} | {cell(row['human'])} "
            f"| {cell(loc)} | {cell(row['doc'])} |")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Inventory Rust numeric limit constants.")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--filter", default="", metavar="SUBSTR")
    parser.add_argument("--sort", choices=("name", "value", "file"))
    args = parser.parse_args()
    try:
        rows = scan()
        if args.filter:
            rows = [row for row in rows if any(
                args.filter in row[field] for field in ("name", "file", "doc", "raw"))]
        rows = sort_rows(rows, args.sort)
        if args.json:
            keys = ("name", "raw", "value", "human", "file", "line", "doc")
            payload = [{key: row[key] for key in keys} for row in rows]
            sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
        else:
            sys.stdout.write(markdown(rows) + "\n")
    except Exception as exc:  # noqa: BLE001 — report must still exit 0
        print(exc, file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
