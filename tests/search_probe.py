#!/usr/bin/env python3
"""Probe GET /api/search timing and result shape, including NDJSON progress.

Manual loopback aid. Body capped at 16 MiB. Prints status, elapsed ms, result
count, incomplete/partial, errors, first 3 hits (uid, snippet ≤ 80). --progress
uses progress=1 NDJSON and prints each packet's kind/size/elapsed.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import http.client
import json
import sys
import time
from urllib.parse import urlencode, urlsplit

MAX_BODY = 16 * 1024 * 1024


def die(message, code=1):
    print(message, file=sys.stderr)
    raise SystemExit(code)


def loopback(base):
    parsed = urlsplit(base.rstrip("/"))
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1":
        die("refusing non-loopback --base; use http://127.0.0.1:PORT")
    return parsed.hostname, parsed.port or 80


def clip(text, n=80):
    return " ".join(str(text).split())[:n]


def parse_json(raw):
    if not raw:
        return {}
    try:
        value = json.loads(raw)
    except json.JSONDecodeError:
        text = raw.decode("utf-8", "replace") if isinstance(raw, (bytes, bytearray)) else str(raw)
        return {"error": clip(text[:240])}
    return value if isinstance(value, dict) else {"error": clip(value)}


def bounded(resp):
    body = resp.read(MAX_BODY + 1)
    if len(body) > MAX_BODY:
        die("response exceeds 16 MiB")
    return body


def add_packet(packets, line, started, extra):
    data = parse_json(line)
    packets.append({"kind": data.get("type") or "?", "size": len(line) + extra,
                    "elapsed_ms": (time.perf_counter() - started) * 1000, "data": data})


def read_ndjson(resp, started):
    packets, buf, total = [], b"", 0
    while True:
        chunk = resp.read(8192)
        if not chunk:
            break
        total += len(chunk)
        if total > MAX_BODY:
            die("response exceeds 16 MiB")
        buf += chunk
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            if line.strip():
                add_packet(packets, line, started, 1)
    if buf.strip():
        add_packet(packets, buf, started, 0)
    return packets


def result_of(packets):
    for item in reversed(packets):
        data = item["data"]
        if data.get("type") == "result" and isinstance(data.get("data"), dict):
            return data["data"]
        if data.get("type") == "error":
            return {"error": data.get("error"), "code": data.get("code")}
    return {}


def emit(query, status, elapsed_ms, payload, as_json, packets=None):
    rows = payload.get("results")
    count = len(rows) if isinstance(rows, list) else 0
    hits = [{"uid": row.get("uid"), "snippet": clip(row.get("snippet", ""))}
            for row in rows[:3] if isinstance(row, dict)] if isinstance(rows, list) else []
    if status != 200 or ("results" not in payload and payload.get("error") is not None):
        errors = [{"error": payload.get("error"), "code": payload.get("code")}]
    else:
        errors = payload.get("errors") if isinstance(payload.get("errors"), list) else []
    info = {"q": query, "status": status, "elapsed_ms": round(elapsed_ms, 1),
            "results": count, "incomplete": payload.get("incomplete"),
            "partial": payload.get("partial"), "errors": errors, "hits": hits}
    if packets is not None:
        info["packets"] = [{"kind": p["kind"], "size": p["size"],
                            "elapsed_ms": round(p["elapsed_ms"], 1)} for p in packets]
    if as_json:
        print(json.dumps(info, ensure_ascii=False), flush=True)
        return
    print(f"q={query!r} status={status} elapsed={info['elapsed_ms']}ms results={count} "
          f"incomplete={info['incomplete']} partial={info['partial']} "
          f"errors={json.dumps(errors, ensure_ascii=False)}", flush=True)
    for hit in hits:
        print(f"  uid={hit['uid']} snippet={hit['snippet']}", flush=True)
    if packets:
        for p in packets:
            print(f"  packet kind={p['kind']} size={p['size']} elapsed={p['elapsed_ms']:.1f}ms",
                  flush=True)


def probe(host, port, query, args):
    params = {"q": query}
    for flag in ("regex", "case", "word", "progress"):
        if getattr(args, flag):
            params[flag] = "1"
    path = "/api/search?" + urlencode(params)
    conn = http.client.HTTPConnection(host, port, timeout=30)
    try:
        started = time.perf_counter()
        conn.request("GET", path)
        resp = conn.getresponse()
        ctype = (resp.getheader("content-type") or "").lower()
        packets = None
        if args.progress and "ndjson" in ctype:
            packets = read_ndjson(resp, started)
            payload = result_of(packets)
        else:
            payload = parse_json(bounded(resp))
        emit(query, resp.status, (time.perf_counter() - started) * 1000,
             payload, args.json, packets)
    except (TimeoutError, OSError, http.client.HTTPException) as err:
        die(f"GET {path}: {err}")
    finally:
        conn.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True, help="http://127.0.0.1:PORT")
    parser.add_argument("--q", action="append", required=True, help="query (repeatable)")
    for name in ("regex", "case", "word"):
        parser.add_argument("--" + name, action="store_true")
    parser.add_argument("--progress", action="store_true", help="progress=1 NDJSON")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    host, port = loopback(args.base)
    for query in args.q:
        probe(host, port, query, args)


if __name__ == "__main__":
    main()
