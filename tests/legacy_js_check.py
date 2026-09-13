#!/usr/bin/env python3
"""Fast static sanity check for served legacy-web/*.js (not vendor/).

node --check syntax; top-level function/async function definitions;
duplicates within/across files; possibly unused names (js+html identifier
search); console.log( outside comments; capability flag uses vs state.rs.
CLI: legacy_js_check.py [--json] [--strict]
"""
# run_validation: skip
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LEGACY = ROOT / "legacy-web"
STATE = ROOT / "crates/sessiondock/src/state.rs"
# Conditional flags are assigned in lib.rs (`capabilities["x"] = …`) after the
# static declaration in state.rs; both sources count as declared.
LIB = ROOT / "crates/sessiondock/src/lib.rs"
FUNC_RE = re.compile(r"^(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(")
CONFIG_RE = re.compile(r"SessionDockCapabilities\.config\.([A-Za-z_][\w]*)")
ALLOWS_RE = re.compile(r"SessionDockCapabilities\.allows\(\s*['\"]([A-Za-z_][\w]*)['\"]\s*\)")
CAP_BODY_RE = re.compile(r"pub fn capabilities\(\) -> Value \{(.+?)^\}", re.M | re.S)
CAP_KEY_RE = re.compile(r'"([A-Za-z_][\w]*)"\s*:')
LOG_RE = re.compile(r"console\.log\(")


def rel(path: Path) -> str:
    return path.resolve().relative_to(ROOT).as_posix()


def syntax_error(path: Path) -> str | None:
    try:
        proc = subprocess.run(
            ["node", "--check", str(path)], capture_output=True, text=True,
            encoding="utf-8", errors="replace", timeout=15,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return str(exc)
    if proc.returncode:
        return (proc.stderr or proc.stdout or f"exit {proc.returncode}").strip()
    return None


def code_lines(text: str):
    """Yield (line, code) with // and /* */ comments stripped."""
    in_block = False
    for number, line in enumerate(text.splitlines(), 1):
        i, n, out = 0, len(line), []
        while i < n:
            if in_block:
                end = line.find("*/", i)
                if end < 0:
                    break
                i, in_block = end + 2, False
                continue
            pair = line[i:i + 2]
            if pair == "//":
                break
            if pair == "/*":
                in_block, i = True, i + 2
                continue
            out.append(line[i])
            i += 1
        yield number, "".join(out)


def functions_in(path: Path, text: str) -> list[dict]:
    name = rel(path)
    return [
        {"file": name, "line": n, "name": match.group(1)}
        for n, line in enumerate(text.splitlines(), 1)
        if (match := FUNC_RE.match(line))
    ]


def rust_keys(text: str) -> list[str]:
    body = CAP_BODY_RE.search(text)
    return list(dict.fromkeys(CAP_KEY_RE.findall(body.group(1) if body else text)))


def possibly_unused(functions: list[dict], lines: list[tuple]) -> list[dict]:
    defs: dict[str, set] = defaultdict(set)
    for item in functions:
        defs[item["name"]].add((item["file"], item["line"]))
    unused = []
    for item in functions:
        skip, pat = defs[item["name"]], re.compile(rf"\b{re.escape(item['name'])}\b")
        if not any((f, n) not in skip and pat.search(text) for f, n, text in lines):
            unused.append(item)
    return unused


def section(title: str, rows: list[str]) -> None:
    print(title)
    print("\n".join(f"  {row}" for row in rows) if rows else "  (none)")


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="print JSON report")
    parser.add_argument("--strict", action="store_true",
                        help="exit 1 on any finding, not only syntax/duplicates")
    args = parser.parse_args(argv)

    js_paths = sorted(p for p in LEGACY.glob("*.js") if p.is_file())
    html_paths = sorted(p for p in LEGACY.glob("*.html") if p.is_file())
    texts = {p: p.read_text(encoding="utf-8", errors="replace") for p in js_paths}
    syntax, functions, logs = [], [], []
    flags: Counter[str] = Counter()
    for path in js_paths:
        text = texts[path]
        err = syntax_error(path)
        if err:
            syntax.append({"file": rel(path), "error": err})
        functions.extend(functions_in(path, text))
        for number, code in code_lines(text):
            if LOG_RE.search(code):
                logs.append({"file": rel(path), "line": number})
        flags.update(CONFIG_RE.findall(text))
        flags.update(ALLOWS_RE.findall(text))

    by_name: dict[str, list] = defaultdict(list)
    for item in functions:
        by_name[item["name"]].append(item)
    duplicates = [
        {"name": name, "locations": locs}
        for name, locs in sorted(by_name.items()) if len(locs) > 1
    ]
    corpus = []
    extra_html = [
        (p, p.read_text(encoding="utf-8", errors="replace")) for p in html_paths
    ]
    for path, text in list(texts.items()) + extra_html:
        name = rel(path)
        corpus.extend((name, n, line) for n, line in enumerate(text.splitlines(), 1))
    unused = possibly_unused(functions, corpus)
    declared = rust_keys(STATE.read_text(encoding="utf-8") if STATE.is_file() else "")
    if LIB.is_file():
        assigned = re.findall(r'capabilities\["([a-z_]+)"\]\s*=', LIB.read_text(encoding="utf-8"))
        declared = sorted(set(declared) | set(assigned))
    undeclared = sorted(set(flags) - set(declared))
    unused_declared = sorted(set(declared) - set(flags))
    capabilities = {
        "counts": dict(sorted(flags.items())),
        "undeclared": undeclared,
        "unused_declared": unused_declared,
        "declared": declared,
    }
    report = {
        "syntax": syntax, "duplicates": duplicates, "unused": unused,
        "console_log": logs, "capabilities": capabilities,
    }
    if args.json:
        json.dump(report, sys.stdout, ensure_ascii=False, indent=2)
        sys.stdout.write("\n")
    else:
        section("syntax", [f"{item['file']}: {item['error']}" for item in syntax])
        section("duplicates", [
            f"{item['name']}: " + ", ".join(
                f"{loc['file']}:{loc['line']}" for loc in item["locations"]
            ) for item in duplicates
        ])
        section("possibly unused", [
            f"{item['file']}:{item['line']} {item['name']}" for item in unused
        ])
        section("console.log", [f"{item['file']}:{item['line']}" for item in logs])
        cap_rows = [f"{key}={count}" for key, count in capabilities["counts"].items()]
        cap_rows.append("JS not declared by Rust: " + (", ".join(undeclared) or "(none)"))
        cap_rows.append("declared but never used: " + (", ".join(unused_declared) or "(none)"))
        section("capability flags", cap_rows)
    extra = unused or logs or undeclared or unused_declared
    return 1 if syntax or duplicates or (args.strict and extra) else 0


if __name__ == "__main__":
    sys.exit(main())
