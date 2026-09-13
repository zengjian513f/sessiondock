#!/usr/bin/env python3
"""Sequential before/after append_benchmark plus bench_summary tables.

Runs tests/append_benchmark.py against --old then --new (never in parallel),
saves JSONL under --out, and renders comparison tables by importing
tests/bench_summary.py. Optional --envelopes runs native_envelopes_benchmark.py
on the new binary with its defaults. --dry-run prints commands and exits 0.
"""
# run_validation: skip
from __future__ import annotations

import argparse
from contextlib import redirect_stdout
from datetime import datetime, timezone
import hashlib
import io
import os
from pathlib import Path
import shlex
import subprocess
import sys

import bench_summary

ROOT = Path(__file__).resolve().parents[1]


def die(message, code=2):
    print(message, file=sys.stderr)
    raise SystemExit(code)


def repo_path(path):
    path = Path(path)
    return path if path.is_absolute() else ROOT / path


def rel(path):
    path = Path(path).resolve()
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def file_sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def require_binary(raw):
    path = repo_path(raw)
    if not path.is_file() or not os.access(path, os.X_OK):
        die(f"not an executable binary: {raw}")
    return path.resolve()


def warn_load():
    try:
        load1 = os.getloadavg()[0]
    except OSError:
        return
    cpus = os.cpu_count() or 1
    limit = cpus / 2
    if load1 > limit:
        print(f"warning: load average {load1:.2f} > CPUs/2 ({limit:.1f}); "
              "run on an idle machine", file=sys.stderr)


def show(cmd):
    parts = ["python3" if i == 0 else item for i, item in enumerate(cmd)]
    return shlex.join(parts)


def append_cmd(binary, sizes, samples, rss):
    cmd = [sys.executable, "tests/append_benchmark.py", "--binary", rel(binary),
           "--sizes", *[str(n) for n in sizes], "--samples", str(samples)]
    if rss:
        cmd.append("--rss")
    return cmd


def envelope_cmd(binary):
    return [sys.executable, "tests/native_envelopes_benchmark.py",
            "--binary", rel(binary)]


def run_jsonl(cmd, dest):
    dest.parent.mkdir(parents=True, exist_ok=True)
    with dest.open("w", encoding="utf-8") as handle:
        completed = subprocess.run(cmd, cwd=ROOT, stdout=handle)
    if completed.returncode:
        die(f"command failed ({completed.returncode}): {show(cmd)}",
            completed.returncode)


def render(func, ns):
    buf = io.StringIO()
    with redirect_stdout(buf):
        func(ns)
    text = buf.getvalue()
    return text if text.endswith("\n") else text + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--old", type=Path, required=True)
    parser.add_argument("--new", type=Path, required=True)
    parser.add_argument("--sizes", type=int, nargs="+", default=[1000, 10000])
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--rss", action="store_true")
    parser.add_argument("--out", type=Path, default=None,
                        help="default: target/perf/<UTC timestamp>")
    parser.add_argument("--envelopes", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    old = require_binary(args.old)
    new = require_binary(args.new)
    print(f"old sha256: {file_sha256(old)}")
    print(f"new sha256: {file_sha256(new)}")
    warn_load()

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    out = repo_path(args.out or Path("target/perf") / stamp)
    jobs = [
        ("old.jsonl", append_cmd(old, args.sizes, args.samples, args.rss)),
        ("new.jsonl", append_cmd(new, args.sizes, args.samples, args.rss)),
    ]
    if args.envelopes:
        jobs.append(("envelopes.jsonl", envelope_cmd(new)))

    if args.dry_run:
        for _name, cmd in jobs:
            print(show(cmd))
        raise SystemExit(0)

    for name, cmd in jobs:
        print(f"+ {show(cmd)}", flush=True)
        run_jsonl(cmd, out / name)

    text = render(bench_summary.cmd_compare, argparse.Namespace(
        old=out / "old.jsonl", new=out / "new.jsonl",
        label_old="old", label_new="new", json=False))
    if args.envelopes:
        extra = render(bench_summary.cmd_single, argparse.Namespace(
            file=out / "envelopes.jsonl", json=False))
        text = text.rstrip() + "\n\n## Envelopes (new binary)\n\n" + extra
    sys.stdout.write(text)
    summary = out / "summary.md"
    summary.write_text(text, encoding="utf-8")
    print(f"wrote {rel(summary)}", file=sys.stderr)


if __name__ == "__main__":
    main()
