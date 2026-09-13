#!/usr/bin/env python3
"""Validation runner for the sessiondock workspace.

Replaces ad-hoc shell scripts. Discovers Node contract files and Python
HTTP/browser suites at runtime after the fixed Rust checks. Never runs
paid CLIs or touches production data; the underlying tests use synthetic
fixtures and loopback listeners only.

Suites run in parallel (``--jobs``, default 8): the Rust checks as lanes that
share no cargo build directory (debug test → clippy; release build; Windows
check; fmt), then every Node/Python suite through a worker pool — each suite
already picks its own loopback port and temp directories. A suite marked
``# run_validation: serial`` near its top runs alone after the pool (timing
assertions that must not share the machine). ``--jobs 1`` is the old serial
order.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import threading
import time
from datetime import datetime

ROOT = Path(__file__).resolve().parents[1]
SKIP_PY = {
    "append_benchmark.py",
    "read_benchmark.py",
    "native_spans_benchmark.py",
    "native_envelopes_benchmark.py",
    "run_validation.py",
    "provider_parity.py",
}
MAIN_RE = re.compile(r"""if\s+__name__\s*==\s*['"]__main__['"]""")
# Report/utility scripts opt out with this exact comment line near their top.
SKIP_MARK = "# run_validation: skip"
# Suites that must not share the machine with other suites (timing assertions).
SERIAL_MARK = "# run_validation: serial"
# Rust checks that may run at the same time: each lane owns one cargo build
# directory (target/debug, target/release, target/<triple>) or none.
RUST_LANES = [["cargo_test", "cargo_clippy"], ["cargo_build"], ["cargo_check_windows"], ["cargo_fmt"]]

# name, argv, kind, timeout seconds, tags
RUST = [
    ("cargo_test", ["cargo", "test", "--workspace", "--locked"], "rust", 1500, ("rust",)),
    ("cargo_fmt", ["cargo", "fmt", "-p", "sessiondock", "-p", "ptyhost-client", "--check"], "rust", 120, ("rust",)),
    ("cargo_clippy", ["cargo", "clippy", "-p", "sessiondock", "-p", "ptyhost-client",
                      "--all-targets", "--locked", "--", "-D", "warnings"], "rust", 900, ("rust",)),
    ("cargo_check_windows", ["cargo", "check", "--workspace", "--all-targets",
                             "--target", "x86_64-pc-windows-msvc", "--locked"], "rust", 900, ("rust",)),
    ("cargo_build", ["cargo", "build", "--release", "-p", "sessiondock", "--locked"], "rust", 900, ("rust",)),
]


def csv(value):
    return [part.strip() for part in (value or "").split(",") if part.strip()]


def has_flag(text, flag):
    return f"'{flag}'" in text or f'"{flag}"' in text


def find_chromium():
    found = []
    for chrome in (Path.home() / ".cache/ms-playwright").glob("chromium-*/chrome-linux64/chrome"):
        if not chrome.is_file():
            continue
        try:
            found.append((int(chrome.parent.parent.name.split("-", 1)[1]), chrome))
        except ValueError:
            found.append((0, chrome))
    return str(max(found)[1]) if found else None


def source_exists(path):
    p = Path(path)
    return (p if p.is_absolute() else ROOT / p).exists()


def suites(binary, python_source):
    """Build the declarative SUITES list: rust, then node, then python."""
    items = []
    for name, argv, kind, timeout, tags in RUST:
        items.append({"name": name, "argv": argv, "kind": kind, "timeout": timeout, "tags": tags, "skip": None})

    contracts = sorted((ROOT / "tests").glob("*_contract.mjs"))
    node_argv = ["node", "--test"] + [str(p.relative_to(ROOT)) for p in contracts]
    items.append({"name": "node_contracts", "argv": node_argv, "kind": "node", "timeout": 120,
                  "tags": ("node",), "skip": None if contracts else "no tests/*_contract.mjs"})

    py_ok = source_exists(python_source)
    for path in sorted((ROOT / "tests").glob("*.py")):
        if path.name in SKIP_PY:
            continue
        text = path.read_text(encoding="utf-8")
        if not MAIN_RE.search(text) or SKIP_MARK in text.splitlines()[:40]:
            continue
        rel = str(path.relative_to(ROOT))
        argv = ["python3", rel]
        skip = None
        head = text.splitlines()[:40]
        serial = SERIAL_MARK in head
        browser = "playwright" in text
        # A "*_real" suite spawns a real paid CLI: operator tier, out of the
        # routine sweep unless --include-real (they cost money and are timing
        # sensitive; the bench_*_real ones already opt out with SKIP_MARK).
        real = path.stem.endswith("_real")
        if path.name.endswith("_parity.py") or has_flag(text, "--python-source"):
            if py_ok:
                argv += ["--python-source", python_source]
            else:
                skip = f"{python_source} not found"
        if has_flag(text, "--browser"):
            argv.append("--browser")
        if has_flag(text, "--binary"):
            argv += ["--binary", binary]
        items.append({"name": path.stem, "argv": argv, "kind": "python", "timeout": 900,
                      "tags": ("python",), "skip": skip, "serial": serial, "browser": browser,
                      "real": real})
        if path.name == "lifecycle_browser.py":
            items.append({"name": "lifecycle_browser_native_binding",
                          "argv": ["python3", rel, "--native-binding"],
                          "kind": "python", "timeout": 900, "tags": ("python",), "skip": None,
                          "serial": serial, "browser": browser, "real": False})
    return items


def index_label(i, n):
    return f"[{i:{max(3, len(str(n)))}d}/{n}]"


def tail(path, n=3):
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    return lines[-n:]


def run_one(suite, env, log_dir, scale):
    log = log_dir / f"{suite['name']}.log"
    if suite["skip"]:
        log.write_text(f"SKIP {suite['skip']}\n", encoding="utf-8")
        return "SKIP", 0.0, suite["skip"]
    timeout = suite["timeout"] * scale
    started = time.monotonic()
    try:
        with log.open("wb") as fh:
            proc = subprocess.run(suite["argv"], cwd=ROOT, env=env, timeout=timeout,
                                  stdout=fh, stderr=subprocess.STDOUT)
    except subprocess.TimeoutExpired:
        with log.open("ab") as fh:
            fh.write(b"\nTIMEOUT\n")
        return "TIMEOUT", time.monotonic() - started, "TIMEOUT"
    except OSError as exc:
        log.write_text(f"{exc}\n", encoding="utf-8")
        return "FAIL", time.monotonic() - started, str(exc)
    elapsed = time.monotonic() - started
    if proc.returncode == 0:
        return "PASS", elapsed, None
    return "FAIL", elapsed, f"exit {proc.returncode}"


def prepare_env(kind, base, chrome):
    env = dict(base)
    if kind == "python" and chrome and not env.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
        env["PLAYWRIGHT_CHROMIUM_EXECUTABLE"] = chrome
    return env


def dump_json(path, payload):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def failed_names(path):
    rows = json.loads(Path(path).read_text(encoding="utf-8")).get("results") or []
    return {row["name"] for row in rows if row.get("status") in {"FAIL", "TIMEOUT"}}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--only", default="", help="NAME[,NAME…]")
    parser.add_argument("--skip", default="", help="NAME[,NAME…]")
    parser.add_argument("--tags", default="rust,node,python")
    parser.add_argument("--list", action="store_true", help="print the resolved plan and exit")
    parser.add_argument("--log-dir", type=Path)
    parser.add_argument("--timeout-scale", type=float, default=1.0)
    parser.add_argument("--keep-going", action="store_true")
    parser.add_argument("--binary", default="target/release/sessiondock")
    parser.add_argument("--python-source", default="../agenthub")
    parser.add_argument("--json", type=Path, metavar="PATH")
    parser.add_argument("--rerun-failed", type=Path, metavar="PATH")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--jobs", type=int, default=8,
                        help="parallel non-browser suites (Rust lanes + Node/Python pool); 1 = serial")
    parser.add_argument("--browser-jobs", type=int, default=3,
                        help="parallel browser (Chromium) suites; kept low to avoid render contention")
    parser.add_argument("--include-real", action="store_true",
                        help="also run the *_real paid-CLI operator suites (excluded by default)")
    args = parser.parse_args(argv)

    wanted = set(csv(args.tags))
    only = set(csv(args.only)) or None
    skipped = set(csv(args.skip))
    plan = []
    for suite in suites(args.binary, args.python_source):
        if wanted.isdisjoint(suite["tags"]):
            continue
        if only is not None and suite["name"] not in only:
            continue
        if suite["name"] in skipped:
            continue
        if suite.get("real") and not args.include_real:
            continue
        plan.append(suite)

    if args.rerun_failed:
        rerun = failed_names(args.rerun_failed)
        plan = [suite for suite in plan if suite["name"] in rerun]
        if not plan:
            print("no failed suites to rerun")
            return 0

    if args.list:
        for suite in plan:
            line = f"{suite['name']:<36} {suite['kind']:<7} {suite['timeout']:4d}s  {shlex.join(suite['argv'])}"
            if suite["skip"]:
                line += f"  SKIP: {suite['skip']}"
            print(line)
        print(f"{len(plan)} suites")
        if args.json:
            dump_json(args.json, {"plan": [{"name": s["name"], "kind": s["kind"],
                                            "timeout": s["timeout"], "argv": s["argv"]} for s in plan]})
        return 0

    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    log_dir = args.log_dir or (ROOT / "target/validation" / stamp)
    log_dir.mkdir(parents=True, exist_ok=True)
    started = datetime.now().astimezone().isoformat()

    env = os.environ.copy()
    if not shutil.which("cargo", path=env.get("PATH", "")):
        cargo_bin = Path.home() / ".cargo/bin"
        if cargo_bin.is_dir():
            env["PATH"] = str(cargo_bin) + os.pathsep + env.get("PATH", "")
    chrome = find_chromium()

    n = len(plan)
    outcome = {}                                  # name -> (status, elapsed, reason)
    stop = threading.Event()                      # set on the first failure unless --keep-going
    lock = threading.Lock()
    order = {suite["name"]: i for i, suite in enumerate(plan, 1)}

    def execute(suite):
        idx = index_label(order[suite["name"]], n)
        if stop.is_set() and not suite["skip"]:
            return
        with lock:
            print(f"{idx} START {suite['name']}", flush=True)
        if args.dry_run:
            status, elapsed, reason = "DRY", 0.0, None
        else:
            status, elapsed, reason = run_one(suite, prepare_env(suite["kind"], env, chrome),
                                              log_dir, args.timeout_scale)
        with lock:
            extra = f"  {reason}" if reason else ""
            time_s = "" if status == "SKIP" else f"  {elapsed:.1f}s"
            print(f"{idx} {status:<7} {suite['name']}{time_s}{extra}", flush=True)
            if status in {"FAIL", "TIMEOUT"}:
                for line in tail(log_dir / f"{suite['name']}.log"):
                    print(f"    {line}")
                if not args.keep_going:
                    stop.set()
            outcome[suite["name"]] = (status, elapsed, reason)

    def lane(suites_in_lane):
        for suite in suites_in_lane:
            execute(suite)

    jobs = max(1, args.jobs)
    if jobs == 1:
        args.browser_jobs = 1
    rust = [suite for suite in plan if suite["kind"] == "rust"]
    pooled = [suite for suite in plan if suite["kind"] != "rust" and not suite.get("serial")]
    serial = [suite for suite in plan if suite["kind"] != "rust" and suite.get("serial")]
    if jobs == 1:
        lane(plan)
    else:
        by_name = {suite["name"]: suite for suite in rust}
        lanes = [[by_name[name] for name in names if name in by_name] for names in RUST_LANES]
        lanes = [group for group in lanes if group]
        with ThreadPoolExecutor(max_workers=max(1, len(lanes))) as pool:
            list(pool.map(lane, lanes))
        headless = [suite for suite in pooled if not suite.get("browser")]
        browsers = [suite for suite in pooled if suite.get("browser")]
        with ThreadPoolExecutor(max_workers=jobs + args.browser_jobs) as pool:
            futures = [pool.submit(execute, suite) for suite in headless]
            # A separate, smaller lane keeps at most browser_jobs Chromiums up:
            # 8-wide Chromium contention makes click/visibility waits flake.
            def browser_lane():
                with ThreadPoolExecutor(max_workers=args.browser_jobs) as bp:
                    list(bp.map(execute, browsers))
            futures.append(pool.submit(browser_lane))
            for future in futures:
                future.result()
        lane(serial)

    results = [(suite, *outcome[suite["name"]]) for suite in plan if suite["name"] in outcome]
    failed = any(status in {"FAIL", "TIMEOUT"} for _, status, _, _ in results)

    width = max((len(s["name"]) for s, *_ in results), default=4)
    counts = {"PASS": 0, "FAIL": 0, "TIMEOUT": 0, "SKIP": 0}
    print()
    print("summary")
    for suite, status, elapsed, reason in results:
        counts[status] = counts.get(status, 0) + 1
        extra = f"  {reason}" if reason else ""
        time_s = "" if status == "SKIP" else f"{elapsed:8.1f}s"
        print(f"  {suite['name']:<{width}}  {status:<7}{time_s}{extra}")
    dry = f", {counts['DRY']} dry" if counts.get("DRY") else ""
    print(f"  {len(results)} suites: {counts['PASS']} passed, {counts['FAIL']} failed, "
          f"{counts['TIMEOUT']} timeout, {counts['SKIP']} skipped{dry}")
    print(f"  logs: {log_dir}")
    exit_code = 1 if failed else 0
    if args.json:
        json_counts = {k: counts.get(k, 0) for k in ("PASS", "FAIL", "TIMEOUT", "SKIP")}
        if counts.get("DRY"):
            json_counts["DRY"] = counts["DRY"]
        dump_json(args.json, {
            "started": started,
            "finished": datetime.now().astimezone().isoformat(),
            "log_dir": str(log_dir),
            "plan": [s["name"] for s in plan],
            "results": [{"name": s["name"], "kind": s["kind"], "status": st, "elapsed_s": el,
                         "reason": rs, "log": str(log_dir / f"{s['name']}.log")}
                        for s, st, el, rs in results],
            "counts": json_counts,
            "exit_code": exit_code,
        })
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
