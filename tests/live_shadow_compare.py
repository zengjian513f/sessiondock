#!/usr/bin/env python3
# run_validation: skip
"""Operator tool: compare GET /api/live of the running Rust and Python services.

Read-only GET via urllib (ProxyHandler({}), no proxy). Both services answer
{"uids": [...], "tmux_uids": [...], "started_at": {uid: epoch}}; Rust may add
enabled/known/partial. If Rust enabled is false, print SKIP rust live disabled
and exit 0. Report uid sets only-in-Python / only-in-Rust / common, tmux_uids
differences, and started_at differences beyond --tolerance-s. For each uid
present on only one side, print source/sid/title from GET /api/sessions of
whichever side lists it. Exit 1 if the uid sets differ, else 0.
"""
from __future__ import annotations

import argparse
import json
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, build_opener

CAP = 8 * 1024 * 1024


def fail(area, why):
    print(f"FAIL {area}: {why}", flush=True)
    raise SystemExit(1)


def passed(area):
    print(f"PASS {area}", flush=True)


def pull(opener, url, timeout):
    try:
        with opener.open(url, timeout=timeout) as resp:
            raw, code = resp.read(CAP + 1), resp.status
    except HTTPError as err:
        return None, f"HTTP {err.code} {err.read(4096)[:240]!r}"
    except (URLError, TimeoutError, OSError) as err:
        return None, str(err)
    if len(raw) > CAP:
        return None, f"oversized {len(raw)} bytes"
    if code != 200:
        return None, f"HTTP {code}"
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError as err:
        return None, f"not JSON: {err}"
    if not isinstance(payload, dict):
        return None, "expected a JSON object"
    return payload, None


def fetch(opener, url, timeout):
    payload, err = pull(opener, url, timeout)
    if err:
        fail(url, err)
    return payload


def as_uids(value, area):
    if value is None:
        return []
    if not isinstance(value, list):
        fail(area, f"expected list, got {type(value).__name__}")
    out = []
    for item in value:
        if not isinstance(item, str) or not item:
            fail(area, f"non-string uid {item!r}")
        out.append(item)
    return out


def as_started(value, area):
    if value is None:
        return {}
    if not isinstance(value, dict):
        fail(area, f"expected object, got {type(value).__name__}")
    out = {}
    for key, raw in value.items():
        if not isinstance(key, str) or not key:
            fail(area, f"non-string uid {key!r}")
        try:
            out[key] = float(raw)
        except (TypeError, ValueError):
            fail(area, f"{key} started_at={raw!r}")
    return out


def session_index(payload):
    rows = payload.get("sessions") if isinstance(payload, dict) else None
    if not isinstance(rows, list):
        return {}
    out = {}
    for row in rows:
        if not isinstance(row, dict):
            continue
        uid = row.get("uid")
        if isinstance(uid, str) and uid and uid not in out:
            out[uid] = row
    return out


def describe(uid, rust_idx, py_idx, prefer):
    row = (py_idx if prefer == "python" else rust_idx).get(uid)
    if row is None:
        row = rust_idx.get(uid) or py_idx.get(uid)
    if row is None:
        return f"{uid} (not in /api/sessions)"
    source = row.get("source") if isinstance(row.get("source"), str) else ""
    sid = row.get("sid") if isinstance(row.get("sid"), str) else ""
    title = row.get("title") if isinstance(row.get("title"), str) else ""
    return f"{uid} source={source} sid={sid} title={title}"


def emit_set(label, items):
    print(f"{label}: {len(items)}", flush=True)
    for uid in items:
        print(f"  {uid}", flush=True)


def compare(rust, python, rust_idx, py_idx, tolerance):
    ru = as_uids(rust.get("uids"), "rust uids")
    pu = as_uids(python.get("uids"), "python uids")
    rs, ps = set(ru), set(pu)
    rust_only = sorted(rs - ps)
    python_only = sorted(ps - rs)
    common = sorted(rs & ps)
    emit_set("uids python_only", python_only)
    emit_set("uids rust_only", rust_only)
    emit_set("uids common", common)

    rt = as_uids(rust.get("tmux_uids"), "rust tmux_uids")
    pt = as_uids(python.get("tmux_uids"), "python tmux_uids")
    rts, pts = set(rt), set(pt)
    emit_set("tmux_uids python_only", sorted(pts - rts))
    emit_set("tmux_uids rust_only", sorted(rts - pts))
    emit_set("tmux_uids common", sorted(rts & pts))

    rstart = as_started(rust.get("started_at"), "rust started_at")
    pstart = as_started(python.get("started_at"), "python started_at")
    started = []
    for uid in common:
        left, right = pstart.get(uid), rstart.get(uid)
        if left is None and right is None:
            continue
        if left is None or right is None or abs(left - right) > tolerance:
            delta = None if left is None or right is None else abs(left - right)
            started.append({"uid": uid, "python": left, "rust": right, "delta": delta})
            print(
                f"started_at DIFF {uid} python={left} rust={right} delta={delta}",
                flush=True,
            )
    if not started:
        print(f"started_at diffs (beyond {tolerance}s): 0", flush=True)

    differing = []
    for uid in python_only:
        line = describe(uid, rust_idx, py_idx, "python")
        differing.append({"uid": uid, "side": "python_only", "line": line})
        print(f"uid python_only {line}", flush=True)
    for uid in rust_only:
        line = describe(uid, rust_idx, py_idx, "rust")
        differing.append({"uid": uid, "side": "rust_only", "line": line})
        print(f"uid rust_only {line}", flush=True)

    report = {
        "uids": {"python_only": python_only, "rust_only": rust_only, "common": common},
        "tmux_uids": {
            "python_only": sorted(pts - rts),
            "rust_only": sorted(rts - pts),
            "common": sorted(rts & pts),
        },
        "started_at": started,
        "differing": differing,
    }
    report["uid_mismatch"] = bool(rust_only or python_only)
    if report["uid_mismatch"]:
        print(
            f"FAIL live uids: python_only={len(python_only)} rust_only={len(rust_only)}",
            flush=True,
        )
    else:
        passed("live uids")
    return report


def load_sessions(opener, base, timeout):
    url = base.rstrip("/") + "/api/sessions"
    payload, err = pull(opener, url, timeout)
    if err:
        print(f"sessions {url} unavailable: {err}", flush=True)
        return {}
    return session_index(payload)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust", default="http://127.0.0.1:8741")
    parser.add_argument("--python", default="http://127.0.0.1:8710")
    parser.add_argument("--timeout", type=float, default=10)
    parser.add_argument("--json", metavar="PATH")
    parser.add_argument("--tolerance-s", type=float, default=2.0, dest="tolerance")
    args = parser.parse_args(argv)
    opener = build_opener(ProxyHandler({}))
    rust_url = args.rust.rstrip("/") + "/api/live"
    python_url = args.python.rstrip("/") + "/api/live"
    rust = fetch(opener, rust_url, args.timeout)
    if rust.get("enabled") is False:
        print("SKIP rust live disabled", flush=True)
        if args.json:
            payload = {"skip": "rust live disabled", "rust": rust_url, "python": python_url}
            try:
                with open(args.json, "w", encoding="utf-8") as handle:
                    json.dump(payload, handle, ensure_ascii=False, indent=2)
                    handle.write("\n")
            except OSError as err:
                fail("json", str(err))
        return 0
    python = fetch(opener, python_url, args.timeout)
    rust_idx = load_sessions(opener, args.rust, args.timeout)
    py_idx = load_sessions(opener, args.python, args.timeout)
    report = compare(rust, python, rust_idx, py_idx, args.tolerance)
    report.update(rust=rust_url, python=python_url, skip=False)
    if args.json:
        try:
            with open(args.json, "w", encoding="utf-8") as handle:
                json.dump(report, handle, ensure_ascii=False, indent=2)
                handle.write("\n")
        except OSError as err:
            fail("json", str(err))
    return 1 if report["uid_mismatch"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
