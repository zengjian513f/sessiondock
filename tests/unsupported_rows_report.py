#!/usr/bin/env python3
# run_validation: skip
"""Operator tool: summarize supported:false list rows of a running server.

GET /api/sessions JSON only (never session files). Groups unsupported rows by
(source, last migration_warnings line — the fatal reason). Batch 35 (R1–R5)
keeps these as supported:true skip notes, so they belong in the non-fatal
table, not as last-line reasons: Codex later session_meta counted as
`跳过重复的Codex session_meta ×N`; history_base null + forked_from_id is
self-contained (no inherited prefix, never an error); history_base
{thread_id, end_byte_offset} inherits [0, c) of that thread even when
forked_from_id names another; list root_sid/fork_depth/created/title/size
follow the forked_from_id chain (a missing parent ends it); orphan agents
are absent from the list (messages GET is 501); Claude torn JSON
(`跳过无效的JSONL 记录 ×N`), truncated ancestor, cycle, and missing
last-prompt leaf stay supported:true with the listed skip notes.
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from collections import Counter, defaultdict
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, build_opener

PREFERRED = ("claude", "codex", "grok")


def die(message, code=2):
    print(message, file=sys.stderr, flush=True)
    raise SystemExit(code)


def as_str(value, fallback=""):
    return value if isinstance(value, str) and value else fallback


def warning_lines(row):
    raw = row.get("migration_warnings")
    if not isinstance(raw, list):
        return []
    out = []
    for item in raw:
        out.append(item if isinstance(item, str) else json.dumps(item, ensure_ascii=False))
    return out


def example(row):
    uid, path = as_str(row.get("uid")), as_str(row.get("path"))
    if uid and path:
        return f"{uid} {path}"
    return uid or path or "?"


def fetch(url, timeout):
    opener = build_opener(ProxyHandler({}))
    t0 = time.perf_counter()
    try:
        with opener.open(url, timeout=timeout) as resp:
            raw, code = resp.read(), resp.status
    except HTTPError as err:
        die(f"HTTP {err.code}: {err.read(4096)[:240]!r}")
    except (URLError, TimeoutError, OSError) as err:
        die(f"{url}: {err}")
    elapsed = time.perf_counter() - t0
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError as err:
        die(f"response is not JSON: {err}")
    if code != 200:
        die(f"HTTP {code}")
    if not isinstance(payload, dict) or not isinstance(payload.get("sessions"), list):
        die("expected {sessions: [...]} JSON")
    return payload["sessions"], elapsed


def emit(*parts):
    print(" | ".join(str(part) for part in parts), flush=True)


def show_examples(items, limit):
    if limit <= 0:
        return ""
    text = ", ".join(items[:limit])
    extra = len(items) - min(len(items), limit)
    return f"{text} +{extra}" if extra > 0 else text


def report(sessions, elapsed, url, limit):
    groups = defaultdict(list)
    totals = defaultdict(lambda: {"rows": 0, "supported": 0, "unsupported": 0})
    nonfatal = Counter()
    for row in sessions:
        if not isinstance(row, dict):
            continue
        source = as_str(row.get("source"), "?")
        lines = warning_lines(row)
        totals[source]["rows"] += 1
        if row.get("supported") is True:
            totals[source]["supported"] += 1
            nonfatal.update(line for line in lines if line)
        elif row.get("supported") is False:
            totals[source]["unsupported"] += 1
            groups[(source, lines[-1] if lines else "")].append(example(row))
    ranked = sorted(groups.items(), key=lambda item: (-len(item[1]), item[0][0], item[0][1]))
    print("unsupported (source, last warning)", flush=True)
    emit("count", "source", "reason", "examples")
    group_rows = []
    for (source, reason), items in ranked:
        group_rows.append({"count": len(items), "source": source, "reason": reason, "examples": items})
        emit(len(items), source, reason or "(none)", show_examples(items, limit))
    if not ranked:
        emit(0, "-", "(none)", "")
    print("totals", flush=True)
    emit("source", "rows", "supported", "unsupported")
    order = [name for name in PREFERRED if name in totals]
    order += sorted(name for name in totals if name not in PREFERRED)
    total_rows = {"rows": 0, "supported": 0, "unsupported": 0}
    by_source = {}
    for name in order:
        counts = totals[name]
        by_source[name] = dict(counts)
        emit(name, counts["rows"], counts["supported"], counts["unsupported"])
        for key in total_rows:
            total_rows[key] += counts[key]
    emit("all", total_rows["rows"], total_rows["supported"], total_rows["unsupported"])
    print(f"elapsed {elapsed:.3f}s  GET {url}", flush=True)
    print("non-fatal warnings on supported rows (top 15)", flush=True)
    emit("count", "line")
    top = [{"count": n, "line": line} for line, n in nonfatal.most_common(15)]
    for item in top:
        emit(item["count"], item["line"])
    if not top:
        emit(0, "(none)")
    return {
        "url": url,
        "elapsed_s": round(elapsed, 6),
        "groups": group_rows,
        "totals": by_source,
        "all": total_rows,
        "nonfatal": top,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="http://127.0.0.1:8741")
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--json", metavar="PATH", help="write the report JSON")
    parser.add_argument("--show", type=int, default=3, help="example paths per group")
    parser.add_argument("--no-force", action="store_true", help="omit ?force=1")
    args = parser.parse_args()
    url = args.base.rstrip("/") + "/api/sessions"
    if not args.no_force:
        url += "?force=1"
    sessions, elapsed = fetch(url, args.timeout)
    payload = report(sessions, elapsed, url, max(args.show, 0))
    if args.json:
        try:
            with open(args.json, "w", encoding="utf-8") as handle:
                json.dump(payload, handle, ensure_ascii=False, indent=2)
                handle.write("\n")
        except OSError as err:
            die(f"--json {args.json}: {err}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
