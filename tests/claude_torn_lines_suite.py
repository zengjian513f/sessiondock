#!/usr/bin/env python3
"""HTTP contract: Claude torn JSONL lines and broken lineage.

A non-JSON line is skipped (`跳过无效的JSONL 记录 ×N`); a parentUuid chain that
reaches a missing uuid stops with `Claude 祖先链在 <uuid> 处中断，之前的记录不在当前时间线`;
a cycle stops with `Claude 祖先链存在循环，已在 <uuid> 处截断`; a last-prompt
leafUuid with no record gives `Claude 声明的叶子 <uuid> 不在记录中`. `supported`
stays true in all three. GET /api/messages for the torn file returns only the
reachable turn; `end` equals the file size (torn bytes still count) and an
incremental cursor read survives the torn line. Synthetic Claude mains and an
isolated loopback server only.
"""
from __future__ import annotations

import argparse
import json
import tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote

from history_parity import (
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, cursor_query, encoded,
    get_json, isolated_server)

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
SIDS = ("torn", "cycle", "leaf", "good")
TORN_LINE = b"\x00" * 4096 + b'"text":"tail of a lost record"}' + b"\n"
SKIP_JSON = "跳过无效的JSONL 记录 ×1"
BROKEN = "Claude 祖先链在 lost-1 处中断，之前的记录不在当前时间线"
CYCLE_PREFIX = "Claude 祖先链存在循环"
LEAF_MISS = "Claude 声明的叶子 missing-9 不在记录中"


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def texts(payload):
    return [m.get("text") for m in payload.get("messages") or []
            if isinstance(m, dict) and m.get("role") in ("user", "assistant")]


def fetch(opener, base, route, want=200):
    try:
        payload = get_json(opener, base, route)
        raw, code = json.dumps(payload, ensure_ascii=False).encode(), 200
    except HTTPError as err:
        raw, code = err.read(4096), err.code
        try:
            payload = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            fail(route, "response is not JSON", raw)
    if code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    return payload, raw


def put_claude(corpus, sid, parts):
    path = corpus.root / "claude/project-history" / f"{sid}.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(
        p if isinstance(p, (bytes, bytearray)) else encoded(p) for p in parts))
    corpus.paths[sid] = path
    return path


def build(root: Path) -> Corpus:
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)
    sid = "torn"
    put_claude(corpus, sid, [
        claude_row(sid, "user", "u1", None, "torn-u1"),
        claude_row(sid, "assistant", "a1", "u1", "torn-a1"),
        TORN_LINE,
        claude_row(sid, "user", "u2", "lost-1", "torn-u2"),
        claude_row(sid, "assistant", "a2", "u2", "torn-a2")])
    sid = "cycle"
    put_claude(corpus, sid, [
        claude_row(sid, "user", "u1", None, "cycle-u1"),
        claude_row(sid, "assistant", "a1", "u1", "cycle-a1"),
        claude_row(sid, "user", "x", "y", "cycle-x"),
        claude_row(sid, "assistant", "y", "x", "cycle-y")])
    sid = "leaf"
    put_claude(corpus, sid, [
        claude_row(sid, "user", "u1", None, "leaf-u1"),
        claude_row(sid, "assistant", "a1", "u1", "leaf-a1"),
        {"type": "last-prompt", "leafUuid": "missing-9", "sessionId": sid,
         "timestamp": "2026-09-11T10:00:00Z"}])
    sid = "good"
    put_claude(corpus, sid, [
        claude_row(sid, "user", "u1", None, "good-u1"),
        claude_row(sid, "assistant", "a1", "u1", "good-a1"),
        claude_row(sid, "user", "u2", "a1", "good-u2"),
        claude_row(sid, "assistant", "a2", "u2", "good-a2")])
    return corpus


def run(opener, base, corpus: Corpus):
    data, raw = fetch(opener, base, "/api/sessions?force=1")
    if not isinstance(data.get("sessions"), list):
        fail("list", "sessions is not a list", raw)
    rows = {row["sid"]: row for row in data["sessions"] if isinstance(row, dict)}
    for sid in SIDS:
        row = rows.get(sid)
        if not row:
            fail("supported", f"missing row {sid}", raw)
        excerpt = json.dumps(row, ensure_ascii=False).encode()
        if row.get("supported") is not True:
            fail("supported", f"{sid} supported is not true", excerpt)
        # A supported list row carries no migration_warnings
        # and every note lives in the detail meta.
        if "migration_warnings" in row:
            fail("warnings", f"{sid} supported row carries migration_warnings", excerpt)
        detail, draw = fetch(opener, base, "/api/messages/" + quote(corpus.uid(sid), safe=":"))
        notes = (detail.get("meta") or {}).get("migration_warnings") or []
        if sid == "cycle":
            if not any(isinstance(w, str) and w.startswith(CYCLE_PREFIX) for w in notes):
                fail("warnings", f"cycle detail missing {CYCLE_PREFIX!r} prefix, got {notes}", draw)
        elif sid == "torn":
            for item in (SKIP_JSON, BROKEN):
                if item not in notes:
                    fail("warnings", f"torn detail missing {item!r}, got {notes}", draw)
        elif sid == "leaf" and LEAF_MISS not in notes:
            fail("warnings", f"leaf detail missing {LEAF_MISS!r}, got {notes}", draw)
        elif sid == "good" and notes:
            fail("warnings", f"good detail has warnings {notes}", draw)
    passed("list supported (torn/cycle/leaf/good); warnings: row counts, detail lineage notes")

    torn_uid = corpus.uid("torn")
    first, fraw = fetch(opener, base, "/api/messages/" + quote(torn_uid, safe=":"))
    got = texts(first)
    if got != ["torn-u2", "torn-a2"]:
        fail("torn-messages", f"want [torn-u2, torn-a2] (u1/a1 before the break), got {got}", fraw)
    passed("torn messages reachable [u2, a2]")

    good, graw = fetch(opener, base, "/api/messages/" + quote(corpus.uid("good"), safe=":"))
    got = texts(good)
    if got != ["good-u1", "good-a1", "good-u2", "good-a2"]:
        fail("good-messages", f"want four texts in order, got {got}", graw)
    passed("good messages four texts")

    size = corpus.paths["torn"].stat().st_size
    if first.get("end") != size:
        fail("torn-end", f"end {first.get('end')} != file size {size}", fraw)
    try:
        query = cursor_query(first)
    except (KeyError, TypeError) as err:
        fail("torn-incremental", f"cursor fields missing: {err}", fraw)
    with corpus.paths["torn"].open("ab") as fh:
        fh.write(encoded(claude_row("torn", "user", "u3", "a2", "torn-u3")))
        fh.write(encoded(claude_row("torn", "assistant", "a3", "u3", "torn-a3")))
    delta, draw = fetch(opener, base, "/api/messages/" + quote(torn_uid, safe=":") + "?" + query)
    got = texts(delta)
    if got != ["torn-u3", "torn-a3"]:
        fail("torn-incremental", f"want [torn-u3, torn-a3], got {got}", draw)
    passed("torn end==size and incremental u3,a3")

    for sid in ("cycle", "leaf"):
        body, braw = fetch(opener, base, "/api/messages/" + quote(corpus.uid(sid), safe=":"))
        if not isinstance(body.get("messages"), list):
            fail("cycle-leaf", f"{sid} messages is not a JSON list", braw)
    passed("cycle leaf messages HTTP 200 JSON list")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--fixtures-only", type=Path, metavar="DIR")
    args = parser.parse_args()
    if args.fixtures_only is not None:
        root = args.fixtures_only
        root.mkdir(parents=True, exist_ok=True)
        corpus = build(root)
        for sid in SIDS:
            print(corpus.paths[sid], flush=True)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-claude-torn-") as tmp:
        corpus = build(Path(tmp))
        with isolated_server(corpus, args.binary) as (base, opener):
            run(opener, base, corpus)


if __name__ == "__main__":
    main()
