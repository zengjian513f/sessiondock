#!/usr/bin/env python3
"""Cross-check HTTP route inventory from the Rust router, route ledger, and legacy-web."""
# run_validation: skip
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ROUTER = ROOT / "crates/sessiondock/src/api/mod.rs"
# The hub is a separate binary with its own dispatcher; its
# self-answered routes are declared in `HUB_ROUTES` there. Everything else the
# hub serves is proxied to one node, so only these need cross-checking.
HUB_ROUTER = ROOT / "crates/sessiondock/src/api/hub.rs"
HUB_ROUTE_RE = re.compile(r'\(\s*"(?:GET|POST|DELETE|ANY)"\s*,\s*"([^"]+)"\s*\)')
LEDGER = ROOT / "docs/route-ledger.md"
LEGACY = ROOT / "legacy-web"
UID_EXPR = "${encodeURIComponent(uid)}"
ROUTE_RE = re.compile(r'\.route\(\s*"([^"]+)"\s*,\s*(?:get|post|put|delete|patch|any)\s*\(')
LIST_RE = re.compile(r"for path in \[([^\]]+)\]")
SECTION_RE = re.compile(r"^# HTTP route ledger\n(.*)", re.M | re.S)
TICK_RE = re.compile(r"`(/api/[^`]+)`")


def api_path(raw: str) -> str:
    raw = raw.strip().rstrip("/")
    if not raw.startswith("/"):
        raw = "/" + raw
    if not raw.startswith("/api"):
        raw = "/api" + raw
    return raw


def segments(path: str) -> list[str]:
    return [part for part in path.strip("/").split("/") if part]


def is_tmpl(part: str) -> bool:
    return part.startswith("{") and part.endswith("}") and len(part) > 2


def loose_match(left: str, right: str) -> bool:
    a, b = segments(left), segments(right)
    if len(a) != len(b):
        return False
    for x, y in zip(a, b):
        if x != y and not is_tmpl(x) and not is_tmpl(y):
            return False
    return True


def rust_inventory(text: str) -> dict[str, str]:
    implemented = {api_path(path) for path in ROUTE_RE.findall(text)}
    block = LIST_RE.search(text)
    stub = set()
    if block:
        stub = {api_path(path) for path in re.findall(r'"([^"]+)"', block.group(1))}
    # Single stubs outside the list: `.route("/x", any(not_implemented))`.
    stub |= {api_path(path) for path in
             re.findall(r'\.route\(\s*"([^"]+)"\s*,\s*any\(not_implemented\)', text)}
    status = {path: "implemented" for path in implemented if path not in stub}
    status.update({path: "501" for path in stub})
    return status


def hub_inventory(text: str) -> set[str]:
    """The hub binary's self-answered routes (`api::hub::HUB_ROUTES`).

    `{nid}`/`{*path}` template segments become `{param}` so they match the
    legacy `/api/nodes/${id}/…` calls exactly like the node router's paths.
    """
    routes = set()
    for path in HUB_ROUTE_RE.findall(text):
        normalized = "/".join("{param}" if is_tmpl(part.replace("*", "")) else part
                              for part in path.split("/"))
        routes.add(api_path(normalized))
    return routes


def ledger_inventory(text: str) -> dict[str, tuple[str, str]]:
    section = SECTION_RE.search(text)
    found: dict[str, tuple[str, str]] = {}
    if not section:
        return found
    for line in section.group(1).splitlines():
        if not line.startswith("|"):
            continue
        cols = [col.strip() for col in line.strip().strip("|").split("|")]
        if len(cols) < 3 or cols[0] in {"Area", "---"} or cols[0].startswith("---"):
            continue
        group, iface, status = cols[0], cols[1], cols[-1]
        for token in TICK_RE.findall(iface):
            path = api_path(token)
            found.setdefault(path, (group, status))
    return found


def skip_expr(text: str, index: int) -> int:
    depth = 0
    while index < len(text):
        char = text[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            index += 1
            if depth == 0:
                return index
            continue
        index += 1
    return index


def legacy_uses(folder: Path) -> dict[str, set[str]]:
    used: dict[str, set[str]] = defaultdict(set)
    for source in sorted(folder.glob("*.js")):
        text = source.read_text(encoding="utf-8")
        index, n = 0, len(text)
        while index < n:
            quote = text[index]
            if quote in "'\"`" and text.startswith("api/", index + 1):
                index += 1
                buf: list[str] = []
                while index < n and text[index] not in {quote, "?"}:
                    if text.startswith(UID_EXPR, index):
                        buf.append("{uid}")
                        index += len(UID_EXPR)
                        continue
                    if text.startswith("${", index):
                        buf.append("{param}")
                        index = skip_expr(text, index + 1)
                        continue
                    buf.append(text[index])
                    index += 1
                path = api_path("".join(buf))
                if path not in {"", "/api"}:
                    used[path].add(source.name)
            index += 1
    return used


def find_row(rows: list[dict], path: str, loose: bool = False) -> dict | None:
    for row in rows:
        if path == row["route"]:
            return row
    if not loose:
        return None
    for row in rows:
        if loose_match(path, row["route"]):
            return row
    return None


def build() -> dict:
    rust = rust_inventory(ROUTER.read_text(encoding="utf-8"))
    hub = hub_inventory(HUB_ROUTER.read_text(encoding="utf-8")) if HUB_ROUTER.exists() else set()
    ledger = ledger_inventory(LEDGER.read_text(encoding="utf-8"))
    legacy = legacy_uses(LEGACY)
    rows: list[dict] = []
    for path, status in rust.items():
        rows.append(
            {"route": path, "rust": status, "ledger_status": "-", "group": "-", "legacy": []}
        )
    # Hub-only routes (settings-page writes, aggregates the node never serves).
    for path in sorted(hub - set(rust)):
        rows.append({"route": path, "rust": "hub", "ledger_status": "hub mode", "group": "Hub", "legacy": []})
    for path, (group, status) in ledger.items():
        row = find_row(rows, path, loose=False)
        if row is None:
            row = {"route": path, "rust": "missing", "ledger_status": "-", "group": "-", "legacy": []}
            rows.append(row)
        row["ledger_status"] = status
        row["group"] = group
    for path, files in legacy.items():
        row = find_row(rows, path, loose=True)
        if row is None:
            row = {"route": path, "rust": "missing", "ledger_status": "-", "group": "-", "legacy": []}
            rows.append(row)
        row["legacy"] = sorted(set(row["legacy"]) | files)
    rows.sort(key=lambda row: row["route"])
    summary = {
        "implemented": sum(row["rust"] == "implemented" for row in rows),
        "501": sum(row["rust"] == "501" for row in rows),
        "in ledger but absent from router": sum(
            row["ledger_status"] != "-" and row["rust"] == "missing" for row in rows
        ),
        "used by legacy but absent from router": sum(
            bool(row["legacy"]) and row["rust"] == "missing" for row in rows
        ),
    }
    return {"routes": rows, "summary": summary}


def render_table(data: dict) -> str:
    lines = [
        "| Route | Rust | Ledger status | Used by legacy |",
        "| --- | --- | --- | --- |",
    ]
    for row in data["routes"]:
        files = ", ".join(row["legacy"]) if row["legacy"] else "-"
        lines.append(f"| {row['route']} | {row['rust']} | {row['ledger_status']} | {files} |")
    summary = data["summary"]
    lines.append("")
    lines.append(f"implemented: {summary['implemented']}")
    lines.append(f"501: {summary['501']}")
    lines.append(f"in ledger but absent from router: {summary['in ledger but absent from router']}")
    lines.append(
        "used by legacy but absent from router: "
        f"{summary['used by legacy but absent from router']}"
    )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description="Cross-check HTTP route inventory.")
    parser.add_argument("--json", action="store_true", help="print the same report as JSON")
    args = parser.parse_args()
    try:
        data = build()
        if args.json:
            json.dump(data, sys.stdout, ensure_ascii=False, indent=2)
            sys.stdout.write("\n")
        else:
            print(render_table(data))
    except Exception as exc:  # noqa: BLE001 — report must still exit 0
        print(exc, file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
