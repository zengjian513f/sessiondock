#!/usr/bin/env python3
"""Bounded load probe for GET /api/watch SSE subscriber limits.

Opens N concurrent loopback subscriptions, records HTTP status / first event /
503-429 JSON codes, appends one native record, and counts deltas within 5 s.
"""
# run_validation: skip
import argparse
import http.client
import json
import os
import sys
import tempfile
import threading
import time
from pathlib import Path
from urllib.parse import quote, urlencode, urlsplit

from history_parity import (
    BINARY, Corpus, codex_message, codex_row, cursor_query, encoded, isolated_server)

BUDGET, DELTA, CAP = 30, 5, 2 * 1024 * 1024


def die(message, code=1):
    print(message, file=sys.stderr)
    raise SystemExit(code)


def loopback(base):
    parsed = urlsplit(base.rstrip("/"))
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1":
        die("refusing non-loopback --base; use http://127.0.0.1:PORT")
    return parsed.hostname, parsed.port or 80


def fetch_json(host, port, path):
    conn = http.client.HTTPConnection(host, port, timeout=8)
    try:
        conn.request("GET", path, headers={"Accept": "application/json"})
        resp = conn.getresponse()
        body = resp.read(CAP + 1)
        if resp.status != 200:
            die(f"GET {path} HTTP {resp.status}: {body[:200]!r}")
        return json.loads(body)
    finally:
        conn.close()


def synthetic(root: Path):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    sid = "watch-load"
    rows = [codex_row("session_meta", {"id": sid, "session_id": sid, "cwd": "/synthetic/watch-load"})]
    rows += [codex_message("user" if i % 2 == 0 else "assistant", f"watch-load {i}", i + 1)
             for i in range(20)]
    corpus.put(sid, "codex", rows, [])
    return corpus, sid


def read_event(conn, resp, deadline):
    data, size = [], 0
    while time.monotonic() < deadline:
        if conn.sock is not None:
            conn.sock.settimeout(max(deadline - time.monotonic(), 0.05))
        try:
            raw = resp.readline(65536)
        except (TimeoutError, OSError, http.client.HTTPException):
            return None
        if not raw or (size := size + len(raw)) > CAP:
            return None
        line = raw.decode("utf-8", "replace").rstrip("\r\n")
        if line == "":
            if not data:
                continue
            try:
                return json.loads("\n".join(data))
            except json.JSONDecodeError:
                return {"raw": True}
        if line.startswith(":"):
            continue
        key, _, value = line.partition(":")
        if key == "data":
            data.append(value[1:] if value.startswith(" ") else value)
    return None


def worker(host, port, path, row, gate, appended, first_dl, state, until):
    conn = http.client.HTTPConnection(host, port, timeout=max(0.5, first_dl - time.monotonic()))
    try:
        conn.request("GET", path, headers={"Accept": "text/event-stream"})
        resp = conn.getresponse()
        row["status"] = resp.status
        if resp.status != 200:
            try:
                row["code"] = json.loads(resp.read(8192)).get("code") or ""
            except (json.JSONDecodeError, TypeError, AttributeError, ValueError):
                row["code"] = ""
            return
        row["first"] = read_event(conn, resp, first_dl) is not None
        _ready(gate, row)
        if appended.wait(timeout=max(0, until - time.monotonic())) and row["first"]:
            row["delta"] = read_event(conn, resp, state["delta_dl"]) is not None
    except (TimeoutError, OSError, http.client.HTTPException, ValueError) as err:
        row["code"] = row["code"] or type(err).__name__
    finally:
        _ready(gate, row)
        conn.close()


def _ready(gate, row):
    with gate:
        if not row["ready"]:
            row["ready"] = True
            gate.notify_all()


def probe(base, uid, native: Path, args):
    if "/.local/share/agenthub" in str(native.resolve()):
        die("refusing production native path")
    host, port = loopback(base)
    until = time.monotonic() + BUDGET
    extra = {"uid": uid, **({"agent": args.agent} if args.agent else {})}
    q = {"window": "1", **({"agent": args.agent} if args.agent else {})}
    snap = fetch_json(host, port, f"/api/messages/{quote(uid, safe=':')}?{urlencode(q)}")
    try:
        path = "/api/watch?" + cursor_query(snap, **extra)
    except (KeyError, TypeError) as err:
        die(f"window missing end/version.head/anchor: {err}")
    n, gate, appended = args.subscribers, threading.Condition(), threading.Event()
    rows = [{"status": 0, "code": "", "first": False, "delta": False, "ready": False} for _ in range(n)]
    state = {"delta_dl": until}
    first_dl = min(until - DELTA - 1, time.monotonic() + 12)
    threads = [threading.Thread(
        target=worker, args=(host, port, path, row, gate, appended, first_dl, state, until))
        for row in rows]
    for thread in threads:
        thread.start()
    with gate:
        while time.monotonic() < first_dl and not all(row["ready"] for row in rows):
            gate.wait(timeout=0.1)
    with native.open("ab") as fh:
        fh.write(encoded(codex_message("user", "watch-load-delta", 99)))
        fh.flush()
        os.fsync(fh.fileno())
    state["delta_dl"] = min(until, time.monotonic() + DELTA)
    appended.set()
    for thread in threads:
        thread.join(timeout=max(0, until - time.monotonic()))
    print("id\tstatus\tcode\tfirst\tdelta")
    for i, row in enumerate(rows):
        print(f"{i}\t{row['status']}\t{row['code'] or '-'}\t{int(row['first'])}\t{int(row['delta'])}")
    adm = sum(row["status"] == 200 for row in rows)
    rej = sum(1 for row in rows if row["status"] in (503, 429) and row["code"])
    print(f"admitted={adm} rejected={rej} first={sum(r['first'] for r in rows)} "
          f"delta={sum(r['delta'] for r in rows)} other={n - adm - rej}", flush=True)
    if args.expect_max is None:
        return 0
    if adm <= args.expect_max and adm + rej == n:
        return 0
    die(f"--expect-max {args.expect_max}: admitted={adm} explicit_reject={rej} n={n}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-host", action="store_true")
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--subscribers", type=int, default=40)
    parser.add_argument("--expect-max", type=int)
    parser.add_argument("--base")
    parser.add_argument("--uid")
    parser.add_argument("--agent", default="")
    parser.add_argument("--native", type=Path)
    args = parser.parse_args()
    if args.subscribers < 1 or (args.expect_max is not None and args.expect_max < 0):
        parser.error("--subscribers >= 1 and --expect-max >= 0")
    if args.self_host:
        with tempfile.TemporaryDirectory(prefix="sessiondock-watch-load-") as tmp:
            corpus, sid = synthetic(Path(tmp))
            with isolated_server(corpus, args.binary) as (base, _opener):
                raise SystemExit(probe(base, corpus.uid(sid), corpus.paths[sid], args))
    if not (args.base and args.uid and args.native):
        parser.error("need --self-host or --base/--uid/--native")
    raise SystemExit(probe(args.base, args.uid, args.native, args))


if __name__ == "__main__":
    main()
