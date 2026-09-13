#!/usr/bin/env python3
"""Edge behaviour of history-page and media-page grants.

Synthetic Codex fixtures on an isolated loopback server. Asserts MAX_GRANTS 1024
oldest eviction, GRANT_TTL 10 min store (restart/eviction, not a 10-minute wait),
cross-kind 404, 400/403/409 as coded in sessions/pages.rs.
"""
from __future__ import annotations

import argparse, json, os, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote, urlencode

from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, codex_message, codex_row, isolated_server
from media_browser import image

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
CAP = 8 * 1024 * 1024


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def fetch(opener, base, route, want=200, timeout=20):
    try:
        with opener.open(base + route, timeout=timeout) as resp:
            raw, code = resp.read(CAP + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    if code != want:
        fail(route, f"HTTP {code} want {want}", raw)
    if want != 200:
        return raw
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        fail(route, "not JSON", raw)


def path(uid, kind, cursor, agent=""):
    query = {"cursor": cursor}
    if agent:
        query["agent"] = agent
    return "/api/messages/" + quote(uid, safe=":") + kind + "?" + urlencode(query)


def window(opener, base, uid):
    return fetch(opener, base, "/api/messages/" + quote(uid, safe=":") + "?window=1")


def rewrite(file, old, new):
    raw, stamp = file.read_bytes(), file.stat()
    changed = raw.replace(old, new, 1)
    if changed == raw or len(changed) != len(raw):
        fail("rewrite", f"{old!r} -> {new!r} size {len(changed)}/{len(raw)}")
    file.write_bytes(changed)
    os.utime(file, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    sid = "codex-pages"
    rows = [codex_row("session_meta", {"id": sid, "session_id": sid, "cwd": "/synthetic/grants"}),
            codex_message("user", "EARLY-RECORD-AAAA", 1)]
    rows.extend(codex_message("user" if i % 2 == 0 else "assistant", f"g{i:04d}", i + 1)
                for i in range(1, 1500))
    corpus.put(sid, "codex", rows, [])
    sid = "codex-media"
    content = [{"type": "input_text", "text": "MEDIA-LINE-AAAA"}] + [image("codex") for _ in range(40)]
    corpus.put(sid, "codex", [
        codex_row("session_meta", {"id": sid, "session_id": sid, "cwd": "/synthetic/grants-media"}),
        codex_row("response_item", {"type": "message", "role": "user", "content": content}, 1)], [])
    return corpus


def page_cursor(body, area, raw=b""):
    cur = (body.get("partial") or {}).get("cursor")
    if not isinstance(cur, str) or len(cur) != 32 or body["partial"].get("omitted", 0) <= 0:
        fail(area, f"window did not issue a page grant: {body.get('partial')!r}", raw)
    return cur


def media_cursor(body, area):
    found = [row for row in body.get("messages") or [] if row.get("media_more")]
    if len(found) != 1:
        fail(area, f"want 1 media_more, got {len(found)}")
    more = found[0]["media_more"]
    cur = more.get("cursor")
    if not isinstance(cur, str) or len(cur) != 32 or more.get("total") != 40:
        fail(area, f"media_more {more}")
    return cur


def run(opener, base, corpus, puid, muid):
    page_cur = page_cursor(window(opener, base, puid), "pages-window")
    media_cur = media_cursor(window(opener, base, muid), "media-window")
    fetch(opener, base, path(puid, "/media-page", page_cur), 404)
    fetch(opener, base, path(muid, "/page", media_cur), 404)
    passed("cross-kind cursor 404")
    fetch(opener, base, path(puid, "/page", page_cur, "other"), 403)
    fetch(opener, base, path(muid, "/media-page", media_cur, "other"), 403)
    passed("wrong agent 403")
    first = fetch(opener, base, path(puid, "/page", page_cur))
    again = fetch(opener, base, path(puid, "/page", page_cur))
    if first.get("messages") != again.get("messages") or not first.get("messages"):
        fail("retry", "page messages not identical")
    passed("retry page identical messages")
    a = fetch(opener, base, path(muid, "/media-page", media_cur))
    b = fetch(opener, base, path(muid, "/media-page", media_cur))
    nxt = a.get("page", {}).get("next")
    if not nxt or nxt != b.get("page", {}).get("next"):
        fail("media-next", f"next not stable {a.get('page')} vs {b.get('page')}")
    passed("media page next stable")
    for kind, token in (("/page", page_cur), ("/media-page", media_cur)):
        uid = puid if kind == "/page" else muid
        for bad in (token[:31], token.upper(), token + "0"):
            fetch(opener, base, path(uid, kind, bad), 400)
    passed("malformed cursors 400 (31 hex, uppercase, 33 chars)")
    rewrite(corpus.paths["codex-pages"], b"EARLY-RECORD-AAAA", b"EARLY-RECORD-BBBB")
    fetch(opener, base, path(puid, "/page", page_cur), 409)
    passed("same-size restored-mtime early rewrite → page 409")
    rewrite(corpus.paths["codex-media"], b"MEDIA-LINE-AAAA", b"MEDIA-LINE-BBBB")
    fetch(opener, base, path(muid, "/media-page", media_cur), 409)
    passed("media-line rewrite → media cursor 409")
    first = last = None
    for _ in range(1025):
        last = page_cursor(window(opener, base, puid), "evict-window")
        if first is None:
            first = last
    fetch(opener, base, path(puid, "/page", first), 404)
    body = fetch(opener, base, path(puid, "/page", last))
    if not body.get("messages"):
        fail("evict", "newest grant empty", json.dumps(body).encode())
    passed("1025 windows: first cursor 404 (evicted), newest works")
    return last


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-grants-edge-") as tmp:
        corpus = build(Path(tmp))
        puid, muid = corpus.uid("codex-pages"), corpus.uid("codex-media")
        with isolated_server(corpus, args.binary) as (base, opener):
            keep = run(opener, base, corpus, puid, muid)
        with isolated_server(corpus, args.binary) as (base, opener):
            fetch(opener, base, path(puid, "/page", keep), 404)
            passed("restart drops grants 404")


if __name__ == "__main__":
    main()
