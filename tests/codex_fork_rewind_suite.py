#!/usr/bin/env python3
"""HTTP contract: Codex fork whose forked_from_id ≠ history_base.thread_id (batch 35 R2/R3).

R2: history_base.thread_id supplies the inherited prefix [0, end_byte_offset) even
when forked_from_id names a different thread; a null history_base with
forked_from_id is self-contained. R3: list root_sid / fork_depth / created /
title / size follow the forked_from_id chain like Python
CodexAdapter.finalize_sessions (size = own + Σ min(cur.history_base.end_byte_offset
or 0, parent file size)). A mid-line history_base cut is supported:false with a
行边界 warning and GET /api/messages is HTTP 501 JSON. Synthetic Codex rollouts
and an isolated loopback server only.
"""
from __future__ import annotations

import argparse
import json
import tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote

from history_parity import (
    BINARY as DEBUG_BINARY, REPO, Corpus, codex_message, codex_row, encoded,
    get_json, isolated_server)

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
R, Q, A, B = "R", "Q", "A", "B"
R1 = ["root turn one question", "root turn one answer"]
R2 = ["root turn two question", "root turn two answer"]
R3 = ["root turn three question", "root turn three answer"]
OWN_Q = ["fork q own question", "fork q own answer"]
OWN_A = ["rewind a own question", "rewind a own answer"]


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def meta(sid, **extra):
    return codex_row("session_meta", {
        "id": sid, "session_id": sid, "timestamp": "2026-09-11T09:00:00Z",
        "cwd": "/synthetic/fork-rewind", "thread_source": "user", **extra})


def turn(tid, user, asst, n0):
    return [
        codex_row("event_msg", {"type": "task_started", "turn_id": tid}, n0),
        codex_message("user", user, n0 + 1),
        codex_message("assistant", asst, n0 + 2),
        codex_row("event_msg", {"type": "task_complete", "turn_id": tid}, n0 + 3)]


def hbase(thread, offset, exclusive):
    return {"thread_id": thread, "end_byte_offset": offset, "end_ordinal_exclusive": exclusive}


def texts(payload):
    return [m.get("text") for m in payload.get("messages") or []
            if isinstance(m, dict) and m.get("role") in ("user", "assistant")]


def messages(opener, base, uid, want=200):
    route = "/api/messages/" + quote(uid, safe=":")
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


def build(root: Path) -> tuple[Corpus, int, int]:
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)
    t1, t2, t3 = turn("r1", *R1, 1), turn("r2", *R2, 5), turn("r3", *R3, 9)
    head = [meta(R)]
    c1 = sum(len(encoded(row)) for row in head + t1)
    c2 = c1 + sum(len(encoded(row)) for row in t2)
    corpus.put(R, "codex", head + t1 + t2 + t3, R1 + R2 + R3)
    corpus.put(Q, "codex", [
        meta(Q, forked_from_id=R, history_mode="paginated", history_base=hbase(R, c2, 9)),
        *turn("q1", *OWN_Q, 1)], OWN_Q)
    corpus.put(A, "codex", [
        meta(A, forked_from_id=Q, history_mode="paginated", history_base=hbase(R, c1, 5)),
        *turn("a1", *OWN_A, 1)], OWN_A)
    corpus.put(B, "codex", [
        meta(B, forked_from_id=Q, history_mode="paginated",
             history_base=hbase(R, c1 + 1, 5)),
        *turn("b1", "midline b own question", "midline b own answer", 1)], [])
    return corpus, c1, c2


def run(opener, base, corpus: Corpus, c1: int, c2: int):
    data = get_json(opener, base, "/api/sessions?force=1")
    raw = json.dumps(data, ensure_ascii=False).encode()
    if not isinstance(data.get("sessions"), list):
        fail("list", "sessions is not a list", raw)
    rows = {row["sid"]: row for row in data["sessions"] if isinstance(row, dict)}
    for sid in (R, Q, A):
        row = rows.get(sid)
        if not row:
            fail("supported", f"missing row {sid}", raw)
        excerpt = json.dumps(row, ensure_ascii=False).encode()
        if row.get("supported") is not True:
            fail("supported", f"{sid} supported is not true", excerpt)
        warns = row.get("migration_warnings") or []
        if any("不一致" in str(w) or "mismatch" in str(w).lower() for w in warns):
            fail("supported", f"{sid} history_base mismatch warning", excerpt)
    passed("rows R Q A supported:true, no history_base mismatch warning")

    sr, sq, sa = (corpus.paths[s].stat().st_size for s in (R, Q, A))
    a, q = rows[A], rows[Q]
    want_a = sa + min(c1, sq) + min(c2, sr)
    want_q = sq + min(c2, sr)
    if (a.get("root_sid") != R or a.get("fork_depth") != 2
            or a.get("created") != rows[R].get("created") or a.get("title") != rows[R].get("title")
            or a.get("size") != want_a):
        fail("topology", f"A root/depth/created/title/size want root={R} depth=2 "
             f"created={rows[R].get('created')!r} title={rows[R].get('title')!r} size={want_a}",
             json.dumps(a, ensure_ascii=False).encode())
    if q.get("root_sid") != R or q.get("fork_depth") != 1 or q.get("size") != want_q:
        fail("topology", f"Q root/depth/size want root={R} depth=1 size={want_q}",
             json.dumps(q, ensure_ascii=False).encode())
    passed("A/Q root_sid fork_depth created title size (forked_from_id chain)")

    got_a = texts(messages(opener, base, corpus.uid(A))[0])
    if got_a != R1 + OWN_A or any("fork q own" in (t or "") for t in got_a):
        fail("messages", f"A want {R1 + OWN_A} (no Q own turn), got {got_a}")
    got_q = texts(messages(opener, base, corpus.uid(Q))[0])
    if got_q != R1 + R2 + OWN_Q:
        fail("messages", f"Q want {R1 + R2 + OWN_Q}, got {got_q}")
    passed("messages: A inherits R turn1 + own; Q inherits R turn1-2 + own")

    brow = rows.get(B)
    if not brow:
        fail("midline", "missing row B", raw)
    warns = [str(w) for w in (brow.get("migration_warnings") or [])]
    if brow.get("supported") is not False or not any("行边界" in w for w in warns):
        fail("midline", "B want supported:false and a 行边界 warning",
             json.dumps(brow, ensure_ascii=False).encode())
    body, braw = messages(opener, base, corpus.uid(B), want=501)
    if not isinstance(body, dict):
        fail("midline", "B messages 501 is not a JSON object", braw)
    passed("B mid-line cut: supported:false 行边界, messages HTTP 501 JSON")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--fixtures-only", type=Path, metavar="DIR")
    args = parser.parse_args()
    if args.fixtures_only is not None:
        root = args.fixtures_only
        root.mkdir(parents=True, exist_ok=True)
        corpus, _, _ = build(root)
        for sid in (R, Q, A, B):
            print(corpus.paths[sid])
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-codex-fork-rewind-") as tmp:
        corpus, c1, c2 = build(Path(tmp))
        with isolated_server(corpus, args.binary) as (base, opener):
            run(opener, base, corpus, c1, c2)


if __name__ == "__main__":
    main()
