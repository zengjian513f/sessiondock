#!/usr/bin/env python3
# run_validation: skip
"""Operator tool: compare GET /api/search latency and result uids of two
running services (SessionDock vs Python agenthub). Read-only GET only
(urllib ProxyHandler({}), no proxy). Not part of the validation sweep.
Never writes files.
"""
from __future__ import annotations

import argparse
import json
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import ProxyHandler, build_opener

CAP = 16 * 1024 * 1024
TIMEOUT = 120
DEFAULTS = (
    ("ddp_guard", {}),
    ("guard", {"word": "1"}),
    ("agenthub.*rust", {"regex": "1"}),
    ("zzqqxx_no_such_token", {}),
)


def fail(area, why):
    print(f"FAIL {area}: {why}", flush=True)
    raise SystemExit(1)


def passed(area):
    print(f"PASS {area}", flush=True)


def parse_body(raw):
    text = raw.decode("utf-8")
    try:
        payload = json.loads(text) if text else {}
        if isinstance(payload, dict) and "results" in payload:
            return payload["results"]
    except json.JSONDecodeError:
        payload = None
    last = None
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError as err:
            fail("body", f"not JSON/NDJSON: {err}")
        if isinstance(obj, dict) and "results" in obj:
            last = obj["results"]
    if last is None:
        fail("body", "no results field")
    return last


def result_uids(rows, area):
    if not isinstance(rows, list):
        fail(area, f"results not list: {type(rows).__name__}")
    out = []
    for item in rows:
        if not isinstance(item, dict):
            fail(area, "result not object")
        uid = item.get("uid")
        if not isinstance(uid, str) or not uid:
            uid = item.get("id")
        if not isinstance(uid, str) or not uid:
            fail(area, "missing uid/id")
        out.append(uid)
    return out


def search(opener, base, query, options):
    params = {"q": query, "limit": "60", **options}
    url = base.rstrip("/") + "/api/search?" + urlencode(params)
    t0 = time.perf_counter()
    try:
        with opener.open(url, timeout=TIMEOUT) as resp:
            raw, code = resp.read(CAP + 1), resp.status
    except HTTPError as err:
        fail(url, f"HTTP {err.code} {err.read(4096)[:240]!r}")
    except (URLError, TimeoutError, OSError) as err:
        fail(url, str(err))
    elapsed = time.perf_counter() - t0
    if len(raw) > CAP:
        fail(url, f"oversized {len(raw)} bytes")
    if code != 200:
        fail(url, f"HTTP {code}")
    uids = result_uids(parse_body(raw), url)
    return elapsed, uids


def fmt_opts(options):
    return ",".join(f"{k}={v}" for k, v in options.items()) or "-"


def parse_queries(raw):
    if not raw:
        return list(DEFAULTS)
    return [(q, {}) for q in raw]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust", default="http://127.0.0.1:8741")
    parser.add_argument("--python", default="http://127.0.0.1:8710")
    parser.add_argument("--limit-s", type=float, default=10.0)
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--queries", action="append", default=[])
    args = parser.parse_args(argv)
    if args.rounds < 1:
        fail("args", "rounds must be >= 1")
    opener = build_opener(ProxyHandler({}))
    queries = parse_queries(args.queries)
    print(
        f"{'round':<6} {'query':<24} {'opts':<12} {'rust_s':>8} {'py_s':>8} "
        f"{'n_rust':>6} {'n_py':>6} match",
        flush=True,
    )
    last_slow = []
    last_diff = []
    max_rust = {}
    for rnd in range(1, args.rounds + 1):
        peak = 0.0
        for query, options in queries:
            rs, ru = search(opener, args.rust, query, options)
            ps, pu = search(opener, args.python, query, options)
            peak = max(peak, rs)
            match = "SAME" if ru == pu else "DIFF"
            print(
                f"{rnd:<6} {query:<24} {fmt_opts(options):<12} {rs:8.3f} "
                f"{ps:8.3f} {len(ru):6d} {len(pu):6d} {match}",
                flush=True,
            )
            if rnd == args.rounds:
                if rs > args.limit_s:
                    last_slow.append((query, rs))
                if ru != pu:
                    last_diff.append(query)
        max_rust[rnd] = peak
        print(f"max rust_s round {rnd}: {peak:.3f}", flush=True)

    ok = True
    if last_slow:
        ok = False
        for query, rs in last_slow:
            print(
                f"FAIL rust latency: {query!r} {rs:.3f}s > {args.limit_s}",
                flush=True,
            )
    else:
        passed(f"rust latency last round <= {args.limit_s}s")
    if last_diff:
        ok = False
        fail("uids", f"DIFF queries: {last_diff}")
    passed("result uids")
    if not ok:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
