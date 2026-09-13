#!/usr/bin/env python3
"""List and fetch a session's media through a running loopback Rust server.

GET /api/messages/{uid}?window=1, print src/alt/lazy/error descriptors,
optionally GET /api/media/{token} and walk media_more pages. Never writes.
Each GET is capped at 32 MiB; oversize bodies abort with a note.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import sys
import time
from urllib.parse import quote, urlencode, urlsplit

MAX_GET = 32 * 1024 * 1024
MAX_JSON = 8 * 1024 * 1024

def die(message, code=1):
    print(message, file=sys.stderr)
    raise SystemExit(code)


def loopback(base):
    parsed = urlsplit(base.rstrip("/"))
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1":
        die("refusing non-loopback --base; use http://127.0.0.1:PORT")
    return parsed.hostname, parsed.port or 80


def request(host, port, path, limit, timeout=30):
    conn = http.client.HTTPConnection(host, port, timeout=timeout)
    started = time.monotonic()
    try:
        conn.request("GET", path)
        resp = conn.getresponse()
        status, ctype = resp.status, resp.getheader("Content-Type") or ""
        length = resp.getheader("Content-Length")
        over = bool(length and length.isdigit() and int(length) > limit)
        chunks, n = [], 0
        while not over:
            block = resp.read(65536)
            if not block:
                break
            if n + len(block) > limit:
                over = True
                break
            chunks.append(block)
            n += len(block)
        ms = int((time.monotonic() - started) * 1000)
        return status, ctype, b"" if over else b"".join(chunks), ms, over
    except (TimeoutError, OSError, http.client.HTTPException) as err:
        die(f"GET {path}: {err}")
    finally:
        conn.close()


def get_json(host, port, path):
    status, _, body, _, truncated = request(host, port, path, MAX_JSON)
    if truncated or status != 200:
        die(f"GET {path} HTTP {status}: {body[:240]!r}")
    try:
        value = json.loads(body)
    except json.JSONDecodeError as err:
        die(f"GET {path}: {err}")
    if not isinstance(value, dict):
        die(f"GET {path}: expected a JSON object")
    return value


def fetch_one(host, port, src):
    if not isinstance(src, str) or not src.startswith("/api/media/") or "://" in src:
        return None
    status, ctype, body, ms, truncated = request(host, port, src, MAX_GET)
    row = {"status": status, "content_type": ctype, "bytes": len(body), "ms": ms,
           "sha256": hashlib.sha256(body).hexdigest()[:12] if body else ""}
    if truncated:
        row.update(bytes=MAX_GET, sha256="", note="aborted after 32 MiB")
    return row


def follow_pages(host, port, uid, agent, cursor):
    extra, seen = [], set()
    while cursor:
        if cursor in seen or len(seen) >= 32:
            die("media_more cursor loop or page cap")
        seen.add(cursor)
        query = {"cursor": cursor, **({"agent": agent} if agent else {})}
        path = f"/api/messages/{quote(uid, safe=':')}/media-page?{urlencode(query)}"
        page = get_json(host, port, path)
        media = page.get("media") if isinstance(page.get("media"), list) else []
        extra.extend(item for item in media if isinstance(item, dict))
        info = page.get("page") if isinstance(page.get("page"), dict) else {}
        if not info.get("next") or info.get("remaining") == 0:
            break
        cursor = info.get("next")
    return extra


def render(messages, counts):
    for msg in messages:
        print(f"message {msg['index']}")
        print("idx  lazy   src       alt        fetch")
        if not msg["descriptors"]:
            print("  (none)")
            continue
        for d in msg["descriptors"]:
            lazy = {True: "true", False: "false"}.get(d.get("lazy"), "-")
            if "status" in d:
                fetch = (f"{d['status']}  {d.get('content_type', '')}  {d.get('bytes', 0)}B  "
                         f"sha256={d.get('sha256', '')}  {d.get('ms', 0)}ms")
                if d.get("note"):
                    fetch += f"  {d['note']}"
            elif d.get("error"):
                fetch = f"error {d['error'].get('status')} {d['error'].get('code', '')}"
            else:
                fetch = ""
            print(f"{d['i']:<3}  {lazy:<5}  {d['src_suffix']:<8}  {d.get('alt', '')}  {fetch}".rstrip())
    parts = " ".join(f"{key}:{n}" for key, n in sorted(counts.items(), key=lambda kv: str(kv[0])))
    print("summary  " + (parts or "(none)"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True, help="http://127.0.0.1:PORT")
    parser.add_argument("--uid", required=True)
    parser.add_argument("--agent", default="")
    parser.add_argument("--fetch", action="store_true")
    parser.add_argument("--follow-more", action="store_true")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    host, port = loopback(args.base)
    query = {"window": "1", **({"agent": args.agent} if args.agent else {})}
    window = get_json(host, port, f"/api/messages/{quote(args.uid, safe=':')}?{urlencode(query)}")
    raw = window.get("messages") if isinstance(window.get("messages"), list) else []
    counts, out = {}, []
    for mi, message in enumerate(raw):
        if not isinstance(message, dict):
            continue
        items = [item for item in (message.get("media") or []) if isinstance(item, dict)]
        more = message.get("media_more") if isinstance(message.get("media_more"), dict) else None
        if args.follow_more and more and more.get("cursor"):
            items += follow_pages(host, port, args.uid, args.agent, more["cursor"])
        rows = []
        for di, item in enumerate(items):
            fetched = fetch_one(host, port, item.get("src")) if args.fetch else None
            err = item.get("error") if isinstance(item.get("error"), dict) else {}
            key = fetched["status"] if fetched else err.get("status", "listed")
            counts[key] = counts.get(key, 0) + 1
            src = item.get("src")
            token = src.rsplit("/", 1)[-1] if isinstance(src, str) and src else ""
            row = {"i": di, "lazy": item.get("lazy"), "src_suffix": token[-8:] if token else "-",
                   "alt": item.get("alt") or ""}
            if err:
                row["error"] = err
            if fetched:
                row.update(fetched)
            rows.append(row)
        out.append({"index": mi, "descriptors": rows})
    if args.json:
        print(json.dumps({"messages": out, "summary": {str(k): v for k, v in counts.items()}},
                         ensure_ascii=False, indent=2))
    else:
        render(out, counts)

if __name__ == "__main__":
    main()
