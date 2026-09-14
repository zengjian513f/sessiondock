#!/usr/bin/env python3
"""Pagination correctness at scale without Chromium.

Builds one Codex and one Claude session of 3000 non-status records (multiline,
unicode, 40 tiny embedded images), walks every history page on an isolated
loopback server, and checks grant reuse, scope and append-stable live end.
"""
from __future__ import annotations

import argparse
import json
import random
import tempfile
import time
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote, urlencode

from fixture_gen import messages, meta
from history_parity import BINARY, Corpus, claude_row, codex_message, codex_row, encoded, isolated_server

N, IMAGES, CAP = 3000, 40, 16 * 1024 * 1024


def fail(why):
    raise SystemExit(f"FAIL {why}")


def get(opener, base, route, timeout=30):
    try:
        with opener.open(base + route, timeout=timeout) as resp:
            raw = resp.read(CAP + 1)
    except HTTPError as err:
        fail(f"{route} HTTP {err.code}: {err.read(4096)[:240]!r}")
    if len(raw) > CAP:
        fail(f"{route} oversized")
    return json.loads(raw)


def expect(opener, base, route, code):
    try:
        with opener.open(base + route, timeout=10) as resp:
            fail(f"{route} HTTP {resp.status} want {code}")
    except HTTPError as err:
        if err.code != code:
            fail(f"{route} HTTP {err.code} want {code}: {err.read(4096)[:240]!r}")
        err.read(4096)


def msg(uid, extra=""):
    return "/api/messages/" + quote(uid, safe=":") + extra


def page_url(uid, cursor):
    return msg(uid, "/page?" + urlencode({"cursor": cursor}))


def n_images(rows):
    return sum(len(row.get("media") or []) for row in rows)


def pairs(rows):
    return [(row["text"], row["role"]) for row in rows]


def walk(opener, base, uid):
    window = get(opener, base, msg(uid, "?window=1"))
    partial = window.get("partial")
    assert isinstance(partial, dict), partial
    for key in ("head", "tail", "omitted", "cursor"):
        assert key in partial, (key, partial)
    head, tail, omitted, cursor = (partial[k] for k in ("head", "tail", "omitted", "cursor"))
    msgs = window["messages"]
    assert len(msgs) == head + tail and omitted > 0, (len(msgs), partial)
    assert len(msgs) <= 600 and n_images(msgs) <= 128
    restored = list(msgs[:head])
    pos, stop, first, pages = head, head + omitted, None, 0
    while cursor:
        pages += 1
        assert pages <= 64, "pagination did not terminate"
        body = get(opener, base, page_url(uid, cursor))
        for absent in ("meta", "end", "anchor", "version", "activity"):
            assert absent not in body, absent
        chunk, info = body["messages"], body["page"]
        n = len(chunk)
        assert 0 < n <= 200 and n_images(chunk) <= 128, (n, n_images(chunk))
        assert info["cursor"] == cursor and info["start"] == pos, info
        assert info["end"] - info["start"] == n and info["end"] == pos + n, info
        assert info["stop"] == stop, info
        pos += n
        assert info["remaining"] == stop - pos, (info, pos)
        restored.extend(chunk)
        if first is None:
            first = chunk
        nxt = info["next"]
        if nxt is None:
            assert pos == stop, (pos, stop)
            cursor = None
        else:
            assert isinstance(nxt, str) and len(nxt) == 32, nxt
            cursor = nxt
    restored.extend(msgs[head:])
    assert pos == stop and len(restored) == head + omitted + tail
    shown = get(opener, base, msg(uid))["messages"]
    assert pairs(restored) == pairs(shown), uid
    assert pairs(msgs[:head]) == pairs(shown[:head])
    assert pairs(msgs[head:]) == pairs(shown[len(shown) - tail:])
    assert n_images(shown) == IMAGES, n_images(shown)
    skip = "[图片]"
    win = [row["text"] for row in msgs if row["text"] != skip]
    mid = [row["text"] for row in restored[head:head + omitted] if row["text"] != skip]
    assert len(win) == len(set(win)) and len(mid) == len(set(mid))
    assert not set(win) & set(mid), "page overlapped window text"
    return partial["cursor"], first, window["end"], pages, len(shown)


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    rng = random.Random(1)
    sid = "codex-walk"
    header = meta(sid)
    assert header["type"] == "session_meta" and header == codex_row(
        "session_meta", header["payload"], header.get("ordinal", 0))
    corpus.put(sid, "codex", [header, *messages("codex", sid, N, IMAGES, rng)], [])
    sid = "claude-walk"
    rows = messages("claude", sid, N, IMAGES, rng)
    first = rows[0]
    assert first == claude_row(sid, first["type"], first["uuid"], first["parentUuid"],
                               first["message"]["content"])
    corpus.put(sid, "claude", rows, [])
    return corpus


def run(binary):
    with tempfile.TemporaryDirectory(prefix="sessiondock-history-pages-walk-") as tmp:
        corpus = build(Path(tmp))
        # The default page is 2000 events; keep the 200-event walk exact.
        with isolated_server(corpus, binary, extra_env={"SESSIONDOCK_HISTORY_PAGE_EVENTS": "200"}) as (base, opener):
            caps = get(opener, base, "/api/meta")["capabilities"]
            assert caps.get("history_pages") is True, caps
            print("PASS capabilities.history_pages", flush=True)
            started = time.perf_counter()
            grants = {}
            for sid in ("codex-walk", "claude-walk"):
                uid = corpus.uid(sid)
                cursor, first, end, pages, total = walk(opener, base, uid)
                if sid.startswith("codex"):
                    assert total == N, total
                grants[sid] = (uid, cursor, first, end)
                print(f"PASS {sid} {total} display events {pages} pages", flush=True)
            elapsed = time.perf_counter() - started
            print(f"PASS walk {elapsed:.3f}s", flush=True)
            uid, cursor, first, end = grants["codex-walk"]
            replay = get(opener, base, page_url(uid, cursor))
            assert pairs(replay["messages"]) == pairs(first)
            print("PASS replay cursor", flush=True)
            expect(opener, base, page_url(uid, "not-a-grant"), 400)
            print("PASS malformed cursor 400", flush=True)
            expect(opener, base, page_url(grants["claude-walk"][0], cursor), 403)
            print("PASS foreign cursor 403", flush=True)
            with corpus.paths["codex-walk"].open("ab") as fh:
                fh.write(encoded(codex_message("assistant", "APPENDED AFTER GRANT", N + 1)))
            get(opener, base, "/api/sessions?force=1")
            later = get(opener, base, msg(uid, "?window=1"))
            assert later["end"] > end, (later["end"], end)
            still = get(opener, base, page_url(uid, cursor))
            assert pairs(still["messages"]) == pairs(first)
            print("PASS append preserves grant, live end moved", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    try:
        run(args.binary)
    except AssertionError as err:
        fail(str(err))


if __name__ == "__main__":
    main()
