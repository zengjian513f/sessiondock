#!/usr/bin/env python3
"""HTTP contract for Claude `continued_in` on GET /api/sessions.

A is a normal session (no continued_in). B's last record is
{"type":"continued-in","continuedInSessionId":<A sid>,"sessionId":<B sid>} so
B.continued_in == uid(A). C's last record names a sid absent from the list, so
the key is omitted. D has a continued-in record that is not last (a later
user/assistant follows); presence/value equals Python ClaudeAdapter
session_meta + finalize_sessions on the same corpus. Appending continued-in to
A then GET /api/sessions?force=1 adds continued_in. GET /api/messages for B
still works: the continued-in record produces no message (count equals Python
read()). Synthetic fixtures and an isolated loopback server only.
"""
from __future__ import annotations

import argparse, json, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote

from history_parity import (
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, encoded, get_json,
    isolated_server)
from provider_parity import load_adapters

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PYTHON_SOURCE = REPO.parent / "sessiondock"
SIDS = ("A", "B", "C", "D")
ABSENT = "sid-absent"


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}" + (f"; {text[:240]}" if text else ""))


def passed(area):
    print(f"PASS {area}", flush=True)


def continued(sid, dest, ts="2026-09-11T12:00:00Z"):
    return {"type": "continued-in", "sessionId": sid,
            "continuedInSessionId": dest, "timestamp": ts}


def turn(sid, uid, parent, text, ts="2026-09-11T10:00:00Z"):
    user, assistant = uid, uid.replace("u", "a", 1)
    return [claude_row(sid, "user", user, parent, text, timestamp=ts),
            claude_row(sid, "assistant", assistant, user, text + " a", timestamp=ts)]


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)
    corpus.put("A", "claude", turn("A", "u0", None, "normal A"), [])
    corpus.put("B", "claude", turn("B", "u0", None, "source B") + [continued("B", "A")], [])
    corpus.put("C", "claude", turn("C", "u0", None, "source C") + [continued("C", ABSENT)], [])
    corpus.put("D", "claude",
               turn("D", "u0", None, "source D") + [continued("D", "A")]
               + turn("D", "u1", "a0", "later D", ts="2026-09-11T13:00:00Z"), [])
    return corpus


def oracle(adapter, corpus):
    raw = [adapter.session_meta(corpus.paths[sid]) for sid in SIDS]
    if any(row is None for row in raw):
        fail("python-oracle", "session_meta returned None")
    return {row["sid"]: row for row in adapter.finalize_sessions(raw)}


def fetch(opener, base, route):
    try:
        return get_json(opener, base, route)
    except HTTPError as err:
        fail(route, f"HTTP {err.code}", err.read(1024))


def listed(opener, base):
    data = fetch(opener, base, "/api/sessions?force=1")
    rows = {row["sid"]: row for row in data.get("sessions") or []
            if isinstance(row, dict) and row.get("sid")}
    if set(rows) != set(SIDS):
        fail("list", f"listed {sorted(rows)} want {list(SIDS)}",
             json.dumps(data, ensure_ascii=False).encode())
    return rows


def expect_continued(area, row, want):
    has, got = "continued_in" in row, row.get("continued_in")
    if want is None:
        if has:
            fail(area, f"continued_in key present ({got!r}), want omitted",
                 json.dumps(row, ensure_ascii=False).encode())
    elif not has or got != want:
        fail(area, f"continued_in={got!r} want {want!r}",
             json.dumps(row, ensure_ascii=False).encode())
    if "continued_in_sid" in row:
        fail(area, "continued_in_sid must not leak",
             json.dumps(row, ensure_ascii=False).encode())


def run(opener, base, corpus, adapter):
    py = oracle(adapter, corpus)
    rows = listed(opener, base)
    for sid in SIDS:
        expect_continued(sid, rows[sid], py[sid].get("continued_in"))
    expect_continued("B-uid", rows["B"], corpus.uid("A"))
    passed("A-D continued_in matches Python finalize_sessions")
    native, _end = adapter.read(str(corpus.paths["B"]))
    native = [row for row in native if row.get("role") != "status"]  # status rows are Rust `activity`
    body = fetch(opener, base, "/api/messages/" + quote(corpus.uid("B"), safe=":"))
    rust = body.get("messages")
    if not isinstance(rust, list):
        fail("messages", "GET /api/messages needs a messages list",
             json.dumps(body, ensure_ascii=False).encode())
    if len(rust) != len(native):
        fail("messages", f"count rust={len(rust)} python={len(native)}",
             json.dumps(body, ensure_ascii=False).encode())
    passed("B messages count equals Python read(); continued-in is not a message")
    corpus.paths["A"].write_bytes(corpus.paths["A"].read_bytes() + encoded(continued("A", "B")))
    after = listed(opener, base)
    expect_continued("A-append", after["A"], corpus.uid("B"))
    passed("append continued-in to A then force=1 adds continued_in")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--fixtures-only", type=Path, metavar="DIR",
                        help="write the synthetic corpus into DIR, print file paths, exit 0")
    parser.add_argument("--python-source", type=Path, default=PYTHON_SOURCE)
    args = parser.parse_args()
    if args.fixtures_only is not None:
        root = args.fixtures_only.expanduser().resolve()
        root.mkdir(parents=True, exist_ok=True)
        corpus = build(root)
        for path in sorted(p for p in root.rglob("*") if p.is_file()):
            print(path, flush=True)
        for sid in SIDS:
            if sid not in corpus.paths:
                fail("fixtures", f"missing {sid}")
        return
    python_source = args.python_source.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-continued-in-") as tmp:
        corpus = build(Path(tmp))
        adapter = load_adapters(python_source, fixture_root=corpus.root)["claude"]
        with isolated_server(corpus, args.binary) as (base, opener):
            run(opener, base, corpus, adapter)


if __name__ == "__main__":
    main()
