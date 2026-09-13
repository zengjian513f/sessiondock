#!/usr/bin/env python3
"""Summarize one run_validation.py log directory as Markdown 验收 evidence.

Reads a runner --json file plus per-suite logs, or an older combined
`### START`/`### END` log via --legacy-log. Missing numbers print n/a.
"""
# run_validation: skip
import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NA = "n/a"
ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
END_RE = re.compile(r"### END (\S+) rc=(-?\d+)\s+(\d+(?:\.\d+)?)s")
CARGO_RE = re.compile(r"test result:\s+\S+\s+(\d+) passed;\s+(\d+) failed;\s+(\d+) ignored")
NODE_RE = re.compile(r"^ℹ\s+(pass|fail)\s+(\d+)\s*$")
HEADS = {"PASS": "pass_lines", "DELTA": "delta_lines", "UNVERIFIED": "unverified_lines"}


def fmt_s(value):
    if value is None:
        return NA
    n = round(value)
    return f"{int(n)}s" if abs(value - n) < 1e-9 else f"{value:.1f}s"


def new_suite(name, kind=None, status=None, elapsed=None, reason=None):
    return {"name": name, "kind": kind, "status": status, "elapsed_s": elapsed,
            "reason": reason, "pass_lines": [], "delta_lines": [],
            "unverified_lines": [], "fail_lines": [], "cargo": None, "node": None}


def fill_from_text(suite, text):
    cargo = node_pass = node_fail = None
    for raw in text.splitlines():
        line = ANSI_RE.sub("", raw).strip()
        if not line:
            continue
        hit = CARGO_RE.search(line)
        if hit:
            cargo = cargo or {"passed": 0, "failed": 0, "ignored": 0}
            cargo["passed"] += int(hit[1])
            cargo["failed"] += int(hit[2])
            cargo["ignored"] += int(hit[3])
            continue
        hit = NODE_RE.match(line)
        if hit:
            node_pass, node_fail = ((int(hit[2]), node_fail) if hit[1] == "pass"
                                   else (node_pass, int(hit[2])))
            continue
        word, _, rest = line.partition(" ")
        if word in HEADS and rest:
            suite[HEADS[word]].append(rest)
        elif line.startswith("FAIL"):
            suite["fail_lines"].append(line[4:].strip() or line)
    suite["cargo"] = cargo
    if node_pass is not None or node_fail is not None:
        suite["node"] = {"pass": node_pass, "fail": node_fail}
    if suite["kind"]:
        return suite
    name = suite["name"] or ""
    if suite["cargo"] or name.startswith("cargo_"):
        suite["kind"] = "rust"
    elif suite["node"] or name.startswith("node"):
        suite["kind"] = "node"
    else:
        suite["kind"] = "python"
    return suite


def parse_legacy(path):
    suites = []
    text = path.read_text(encoding="utf-8", errors="replace")
    for chunk in text.split("### START ")[1:]:
        header, _, rest = chunk.partition("\n")
        suite = new_suite(header.split()[0] if header.split() else "")
        end = END_RE.search(rest)
        if end:
            suite["status"] = "PASS" if end.group(2) == "0" else "FAIL"
            suite["elapsed_s"] = float(end.group(3))
            if suite["status"] == "FAIL":
                suite["reason"] = f"exit {end.group(2)}"
        body = "\n".join(ln for ln in rest.splitlines() if not ln.startswith("### "))
        suites.append(fill_from_text(suite, body))
    return suites


def parse_runner_json(path):
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        print(f"{path}: invalid JSON ({exc})", file=sys.stderr)
        raise SystemExit(2) from exc
    log_dir = Path(data["log_dir"]) if data.get("log_dir") else path.parent
    suites = []
    for row in data.get("results") or []:
        name = row.get("name") or ""
        elapsed = row.get("elapsed_s")
        elapsed = float(elapsed) if isinstance(elapsed, (int, float)) else None
        suite = new_suite(name, row.get("kind"), row.get("status"), elapsed, row.get("reason"))
        raw = row.get("log") or str(log_dir / f"{name}.log")
        text = ""
        for candidate in (Path(raw), ROOT / raw):
            if candidate.is_file():
                text = candidate.read_text(encoding="utf-8", errors="replace")
                break
        suites.append(fill_from_text(suite, text))
    return suites


def add_counts(left, right):
    if not right:
        return left
    out = dict(left or {})
    for key, value in right.items():
        if value is not None:
            out[key] = value if out.get(key) is None else out[key] + value
    return out


def totals(suites):
    counts = {"PASS": 0, "FAIL": 0, "TIMEOUT": 0, "SKIP": 0, "DRY": 0}
    elapsed = rust = node = None
    for suite in suites:
        status = suite["status"]
        if status in counts:
            counts[status] += 1
        if suite["elapsed_s"] is not None:
            elapsed = suite["elapsed_s"] if elapsed is None else elapsed + suite["elapsed_s"]
        if suite["kind"] == "rust":
            rust = add_counts(rust, suite["cargo"])
        elif suite["kind"] == "node":
            node = add_counts(node, suite["node"])
    return counts, elapsed, rust, node


def render(suites):
    counts, elapsed, rust, node = totals(suites)
    n = lambda v: NA if v is None else str(v)  # noqa: E731
    overview = (f"{counts['PASS']} 套通过/{counts['FAIL']} 失败/"
                f"{counts['TIMEOUT']} 超时/{counts['SKIP']} 跳过")
    if counts["DRY"]:
        overview += f"，{counts['DRY']} 套 DRY"
    lines = [f"总览：{overview}，总耗时 {fmt_s(elapsed)}"]
    kinds = {suite["kind"] for suite in suites}
    if "rust" in kinds:
        rust = rust or {}
        extra = "" if rust.get("failed") in (None, 0) else f"，{rust['failed']} 项失败"
        lines.append(f"- Rust：workspace **{n(rust.get('passed'))} 项通过、"
                     f"{n(rust.get('ignored'))} 项默认 ignored**{extra}")
    if "node" in kinds:
        node = node or {}
        lines.append(f"- Node：**{n(node.get('pass'))} 项**通过、{n(node.get('fail'))} 失败")
    if "python" in kinds:
        lines.append("- Python：")
        for suite in suites:
            if suite["kind"] != "python":
                continue
            joined = "；".join(suite["pass_lines"]) or NA
            lines.append(f"  - `{suite['name']}`（{suite['status'] or NA}，{fmt_s(suite['elapsed_s'])}）：{joined}")
            lines.extend(f"    - DELTA {d}" for d in suite["delta_lines"])
            lines.extend(f"    - UNVERIFIED {u}" for u in suite["unverified_lines"])
    failed = [s for s in suites if s["status"] in {"FAIL", "TIMEOUT"} or s["fail_lines"]]
    if failed:
        lines.append("失败：")
        for suite in failed:
            extra = f" {suite['reason']}" if suite["reason"] else ""
            lines.append(f"- `{suite['name']}` {suite['status'] or NA} {fmt_s(suite['elapsed_s'])}{extra}")
            lines.extend(f"  - FAIL {item}" for item in suite["fail_lines"])
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("json_file", nargs="?", type=Path, help="run_validation --json output")
    parser.add_argument("--legacy-log", type=Path, metavar="FILE", help="older combined ### START/END log")
    parser.add_argument("--json", action="store_true", help="print the extracted structure as JSON")
    args = parser.parse_args(argv)
    if bool(args.json_file) == bool(args.legacy_log):
        parser.error("provide exactly one of JSON_FILE or --legacy-log")
    path = args.legacy_log or args.json_file
    if not path.is_file():
        print(f"not a file: {path}", file=sys.stderr)
        return 2
    suites = parse_legacy(path) if args.legacy_log else parse_runner_json(path)
    if args.json:
        counts, elapsed, rust, node = totals(suites)
        json.dump({"counts": counts, "elapsed_s": elapsed, "rust": rust, "node": node,
                   "suites": suites}, sys.stdout, ensure_ascii=False, indent=2)
        sys.stdout.write("\n")
    else:
        print(render(suites))
    return 0


if __name__ == "__main__":
    sys.exit(main())
