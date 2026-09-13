#!/usr/bin/env python3
"""Subscribe to GET /api/watch SSE for one session and print each event.

Manual debugging aid: GET /api/messages/{uid}?window=1, then open /api/watch
with start/head/anchor from that window. Loopback (127.0.0.1) only.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import http.client
import json
import sys
import time
from datetime import datetime
from urllib.parse import quote, urlencode, urlsplit

MAX_EVENT = 2 * 1024 * 1024


def die(message, code=1):
    print(message, file=sys.stderr)
    raise SystemExit(code)


def stamp():
    now = datetime.now()
    return f"{now:%H:%M:%S}.{now.microsecond // 1000:03d}"


def loopback(base):
    parsed = urlsplit(base.rstrip("/"))
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1":
        die("refusing non-loopback --base; use http://127.0.0.1:PORT")
    return parsed.hostname, parsed.port or 80


def get_json(host, port, path, timeout=10):
    conn = http.client.HTTPConnection(host, port, timeout=timeout)
    try:
        conn.request("GET", path, headers={"Accept": "application/json"})
        resp = conn.getresponse()
        body = resp.read(MAX_EVENT + 1)
        if resp.status != 200:
            die(f"GET {path} HTTP {resp.status}: {body[:240]!r}")
        if len(body) > MAX_EVENT:
            die(f"GET {path}: response exceeds {MAX_EVENT} bytes")
        value = json.loads(body)
        if not isinstance(value, dict):
            die(f"GET {path}: expected a JSON object")
        return value
    except (TimeoutError, OSError, http.client.HTTPException, json.JSONDecodeError) as err:
        die(f"GET {path}: {err}")
    finally:
        conn.close()


def summarize(payload):
    messages = payload.get("messages") if isinstance(payload, dict) else None
    texts = []
    if isinstance(messages, list):
        for row in messages[:4]:
            if isinstance(row, dict) and row.get("text"):
                texts.append(str(row["text"]).replace("\n", " ")[:80])
    keys = sorted(payload) if isinstance(payload, dict) else []
    return {"keys": keys, "reset": payload.get("reset") if isinstance(payload, dict) else None,
            "n": len(messages) if isinstance(messages, list) else 0,
            "start": payload.get("start") if isinstance(payload, dict) else None,
            "end": payload.get("end") if isinstance(payload, dict) else None, "texts": texts}


def emit(name, payload, as_json):
    info, ts = summarize(payload), stamp()
    if as_json:
        print(json.dumps({"ts": ts, "event": name, "keys": info["keys"], "reset": info["reset"],
                          "n": info["n"], "start": info["start"], "end": info["end"],
                          "data": payload}, ensure_ascii=False), flush=True)
        return
    extra = f" texts={'; '.join(info['texts'])}" if info["texts"] else ""
    print(f"{ts} {name} keys={','.join(info['keys'])} reset={info['reset']} n={info['n']} "
          f"start={info['start']} end={info['end']}{extra}", flush=True)


def emit_keep(text, as_json):
    ts = stamp()
    if as_json:
        print(json.dumps({"ts": ts, "event": "keepalive", "data": text}, ensure_ascii=False),
              flush=True)
    else:
        print(f"{ts} keepalive {text}", flush=True)


def consume(conn, resp, deadline, args):
    name, data, comments, size, count = "message", [], [], 0, 0
    while True:
        if args.max_events and count >= args.max_events:
            return
        left = deadline - time.monotonic()
        if left <= 0:
            return
        if conn.sock is not None:
            conn.sock.settimeout(left)
        try:
            raw = resp.readline(65536)
        except (TimeoutError, OSError):
            return
        if not raw:
            return
        size += len(raw)
        if size > MAX_EVENT:
            die("SSE event exceeds 2 MiB")
        line = raw.decode("utf-8", "replace").rstrip("\r\n")
        if line == "":
            if data:
                joined = "\n".join(data)
                try:
                    payload = json.loads(joined)
                except json.JSONDecodeError:
                    payload = {"raw": joined[:400]}
                emit(name, payload, args.json)
                count += 1
            elif comments and args.verbose:
                emit_keep(" | ".join(comments), args.json)
            name, data, comments, size = "message", [], [], 0
            continue
        if line.startswith(":"):
            comments.append(line[1:].lstrip() or "(empty)")
            continue
        key, sep, value = line.partition(":")
        if value.startswith(" "):
            value = value[1:]
        if key == "event" and value:
            name = value
        elif key == "data":
            data.append(value)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True, help="http://127.0.0.1:PORT")
    parser.add_argument("--uid", required=True)
    parser.add_argument("--agent", default="")
    parser.add_argument("--duration", type=float, default=30)
    parser.add_argument("--max-events", type=int, default=0)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--verbose", action="store_true")
    args = parser.parse_args()
    if args.duration <= 0 or args.max_events < 0:
        parser.error("--duration must be positive and --max-events >= 0")
    host, port = loopback(args.base)
    uid = quote(args.uid, safe=":")
    query = {"window": "1"}
    if args.agent:
        query["agent"] = args.agent
    window = get_json(host, port, f"/api/messages/{uid}?{urlencode(query)}")
    try:
        cursor = {"uid": args.uid, "start": window["end"],
                  "head": window["version"]["head"], "anchor": window["anchor"]}
    except (KeyError, TypeError) as err:
        die(f"window missing end/version.head/anchor: {err}; keys={sorted(window)}")
    if args.agent:
        cursor["agent"] = args.agent
    path = "/api/watch?" + urlencode(cursor)
    deadline = time.monotonic() + args.duration
    conn = http.client.HTTPConnection(host, port, timeout=args.duration)
    try:
        conn.request("GET", path, headers={"Accept": "text/event-stream"})
        resp = conn.getresponse()
        if resp.status != 200:
            die(f"GET {path} HTTP {resp.status}: {resp.read(1024)[:240]!r}")
        consume(conn, resp, deadline, args)
    except (TimeoutError, OSError, http.client.HTTPException) as err:
        if not isinstance(err, TimeoutError):
            die(f"watch socket: {err}")
    finally:
        conn.close()


if __name__ == "__main__":
    main()
