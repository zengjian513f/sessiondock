#!/usr/bin/env python3
"""Sample one process (and optional direct children) VmRSS/VmHWM over time.

Linux procfs companion to append_benchmark.py --rss for manual performance
sessions. Reads /proc only; never signals the target. Direct children come
from /proc/<pid>/task/*/children. Stops after --duration or when the process
exits.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import csv
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

MIB = 1024 * 1024


def die(message, code=1):
    print(message, file=sys.stderr)
    raise SystemExit(code)


def mib(nbytes):
    return f"{nbytes / MIB:.1f}MiB"


def proc_memory(pid):
    """VmRSS/VmHWM bytes from /proc/<pid>/status (same parse as server_memory)."""
    result = {"pid": int(pid)}
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        key, _, value = line.partition(":")
        if key == "Name":
            result["name"] = value.strip()
        if key in ("VmRSS", "VmHWM"):
            count, unit = value.split()
            assert unit == "kB"
            result[key] = int(count) * 1024
    assert set(result) >= {"pid", "name", "VmRSS", "VmHWM"}
    return result


def readable_error(pid, exc):
    if isinstance(exc, FileNotFoundError):
        return f"pid {pid} is not readable: no such process"
    if isinstance(exc, PermissionError):
        return f"pid {pid} is not readable: permission denied"
    if isinstance(exc, AssertionError):
        return f"pid {pid} is not readable: /proc status lacks Name/VmRSS/VmHWM"
    return f"pid {pid} is not readable: {exc}"


def children_of(pid):
    found = set()
    try:
        tasks = Path(f"/proc/{pid}/task").iterdir()
    except OSError:
        return []
    for entry in tasks:
        try:
            text = (entry / "children").read_text()
        except OSError:
            continue
        found.update(int(token) for token in text.split())
    found.discard(int(pid))
    return sorted(found)


def emit(elapsed, row, quiet, child=False):
    if quiet:
        return
    body = f"rss={mib(row['VmRSS'])} hwm={mib(row['VmHWM'])}"
    extra = f" child pid={row['pid']} name={row['name']}" if child else ""
    print(f"t={elapsed:.1f}s{extra} {body}", flush=True)


def append_csv(writer, stream, when, row):
    if writer is None:
        return
    writer.writerow([when, row["pid"], row["name"], row["VmRSS"], row["VmHWM"]])
    stream.flush()


def watch(pid, interval, duration, with_children, writer, stream, quiet):
    try:
        proc_memory(pid)
    except (OSError, AssertionError) as exc:
        die(readable_error(pid, exc))
    samples = []
    started = time.monotonic()
    while True:
        elapsed = time.monotonic() - started
        try:
            row = proc_memory(pid)
        except FileNotFoundError:
            print(f"pid {pid} exited", flush=True)
            break
        except (OSError, AssertionError) as exc:
            die(readable_error(pid, exc))
        samples.append(row)
        when = datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")
        emit(elapsed, row, quiet)
        append_csv(writer, stream, when, row)
        if with_children:
            for child in children_of(pid):
                try:
                    child_row = proc_memory(child)
                except (OSError, AssertionError):
                    continue
                emit(elapsed, child_row, quiet, child=True)
                append_csv(writer, stream, when, child_row)
        remaining = duration - (time.monotonic() - started)
        if remaining <= 0:
            break
        time.sleep(min(interval, remaining))
    if not samples:
        die(f"pid {pid} exited before a sample was collected")
    rss = [row["VmRSS"] for row in samples]
    hwm = [row["VmHWM"] for row in samples]
    print(f"min rss={mib(min(rss))} max rss={mib(max(rss))} "
          f"final rss={mib(rss[-1])} peak hwm={mib(max(hwm))}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--interval", type=float, default=0.5)
    parser.add_argument("--duration", type=float, default=60)
    parser.add_argument("--children", action="store_true")
    parser.add_argument("--csv", type=Path)
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()
    if not Path("/proc/self/status").is_file():
        die("rss_watch.py requires Linux procfs", 2)
    if args.pid <= 0:
        parser.error("--pid must be a positive process id")
    if args.interval <= 0 or args.duration <= 0:
        parser.error("--interval and --duration must be positive")
    stream = writer = None
    if args.csv is not None:
        args.csv.parent.mkdir(parents=True, exist_ok=True)
        new = not args.csv.exists() or args.csv.stat().st_size == 0
        stream = args.csv.open("a", newline="")
        writer = csv.writer(stream, lineterminator="\n")
        if new:
            writer.writerow(["timestamp", "pid", "name", "vmrss_bytes", "vmhwm_bytes"])
            stream.flush()
    try:
        watch(args.pid, args.interval, args.duration, args.children, writer, stream, args.quiet)
    finally:
        if stream is not None:
            stream.close()


if __name__ == "__main__":
    main()
