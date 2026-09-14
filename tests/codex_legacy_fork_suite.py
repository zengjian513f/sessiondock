#!/usr/bin/env python3
"""HTTP contract for legacy self-contained Codex forks.

R1: the first session_meta is the identity; later session_meta records are
skipped as exactly `跳过重复的Codex session_meta ×N` and `supported` stays true.
R2: history_base null + forked_from_id set → the file is self-contained (no
inherited prefix, never an error).
R3: list rows walk forked_from_id
(a missing parent ends the chain; size = own + Σ min(end_byte_offset or 0,
parent.size); history_base null therefore adds 0).
"""
from __future__ import annotations

import argparse, json, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote

from history_parity import (
    BINARY as DEBUG_BINARY, REPO, Corpus, codex_message, codex_row, isolated_server)

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
CWD = "/synthetic/legacy-fork"
SIDS = ("root-1", "mid-1", "leaf-1", "orphan-1")
DUP = "跳过重复的Codex session_meta ×{}"


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def fetch(opener, base, route, want=200):
    try:
        with opener.open(base + route, timeout=15) as resp:
            raw, code = resp.read(2 * 1024 * 1024 + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    if code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)
    return payload, raw


def stamp(row, ts):
    row["timestamp"] = ts
    return row


def meta(sid, ts, **extra):
    payload = {"id": sid, "session_id": sid, "timestamp": ts, "cwd": CWD,
               "thread_source": "user", "history_base": None, "forked_from_id": None,
               **extra}
    return stamp(codex_row("session_meta", payload), ts)


def turn(tid, user, assistant, ts, n):
    return [
        stamp(codex_row("event_msg", {"type": "task_started", "turn_id": tid}, n), ts),
        stamp(codex_message("user", user, n + 1), ts),
        stamp(codex_message("assistant", assistant, n + 2), ts),
        stamp(codex_row("event_msg", {"type": "task_complete", "turn_id": tid}, n + 3), ts),
    ]


def texts(rows):
    out = []
    for row in rows:
        payload = row.get("payload") or {}
        if row.get("type") != "response_item" or payload.get("type") != "message":
            continue
        if payload.get("role") not in ("user", "assistant"):
            continue
        block = (payload.get("content") or [{}])[0]
        out.append(block.get("text") or "")
    return out


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)
    root_meta = meta("root-1", "2026-09-11T10:00:00Z")
    root_turns = (turn("root-t1", "Root question one", "Root answer one",
                       "2026-09-11T10:00:01Z", 1)
                  + turn("root-t2", "Root question two", "Root answer two",
                         "2026-09-11T10:00:05Z", 5))
    root_rows = [root_meta, *root_turns]
    corpus.put("root-1", "codex", root_rows, texts(root_rows))
    mid_meta = meta("mid-1", "2026-09-11T11:00:00Z", forked_from_id="root-1")
    mid_own = turn("mid-t1", "Mid own question", "Mid own answer",
                   "2026-09-11T11:00:01Z", 10)
    mid_rows = [mid_meta, root_meta, *root_turns, *mid_own]
    corpus.put("mid-1", "codex", mid_rows, texts(mid_rows))
    leaf_meta = meta("leaf-1", "2026-09-11T12:00:00Z", forked_from_id="mid-1")
    leaf_own = turn("leaf-t1", "Leaf own question", "Leaf own answer",
                    "2026-09-11T12:00:01Z", 20)
    leaf_rows = [leaf_meta, mid_meta, root_meta, *root_turns, *mid_own, *leaf_own]
    corpus.put("leaf-1", "codex", leaf_rows, texts(leaf_rows))
    orphan_meta = meta("orphan-1", "2026-09-11T13:00:00Z", forked_from_id="gone-1")
    gone_meta = meta("gone-1", "2026-09-11T09:00:00Z")
    gone_turn = turn("gone-t1", "Gone prefix question", "Gone prefix answer",
                     "2026-09-11T09:00:01Z", 1)
    orphan_own = turn("orphan-t1", "Orphan own question", "Orphan own answer",
                      "2026-09-11T13:00:01Z", 10)
    orphan_rows = [orphan_meta, gone_meta, *gone_turn, *orphan_own]
    corpus.put("orphan-1", "codex", orphan_rows, texts(orphan_rows))
    return corpus


def visible(payload):
    return [row.get("text") for row in payload.get("messages") or []
            if row.get("role") in ("user", "assistant")]


def check_messages(opener, base, corpus, sid, area):
    payload, raw = fetch(opener, base, "/api/messages/" + quote(corpus.uid(sid), safe=":"))
    got, want = visible(payload), corpus.expected[sid]
    if got != want:
        fail(area, f"{sid} texts {got} want {want}", raw)
    passed(area)


def run(opener, base, corpus):
    data, raw = fetch(opener, base, "/api/sessions?force=1")
    sessions = data.get("sessions")
    if not isinstance(sessions, list) or not isinstance(data.get("sig"), str) or not data["sig"]:
        fail("list", "need sessions list and nonempty sig", raw)
    rows = {row["sid"]: row for row in sessions if isinstance(row, dict) and row.get("sid")}
    if set(rows) != set(SIDS):
        fail("list", f"listed {sorted(rows)} want {list(SIDS)}", raw)
    want_warn = {"root-1": [], "mid-1": [DUP.format(1)],
                 "leaf-1": [DUP.format(2)], "orphan-1": [DUP.format(1)]}
    for sid, expected in want_warn.items():
        row = rows[sid]
        excerpt = json.dumps(row, ensure_ascii=False).encode()
        if row.get("supported") is not True:
            fail("r1", f"{sid} supported={row.get('supported')}", excerpt)
        # The duplicate-meta notes live in the detail meta only.
        if "migration_warnings" in row:
            fail("r1", f"{sid} supported row carries migration_warnings", excerpt)
        detail, draw = fetch(opener, base, "/api/messages/" + quote(corpus.uid(sid), safe=":"))
        got = (detail.get("meta") or {}).get("migration_warnings") or []
        if got != expected:
            fail("r1", f"{sid} detail migration_warnings {got} want {expected}", draw)
    passed("r1 supported + duplicate session_meta warnings (detail meta)")

    root, mid, leaf, orphan = (rows[s] for s in SIDS)
    leaf_size = corpus.paths["leaf-1"].stat().st_size
    if (leaf.get("root_sid") != "root-1" or leaf.get("fork_depth") != 2
            or leaf.get("created") != root.get("created")
            or leaf.get("title") != root.get("title")
            or leaf.get("size") != leaf_size):
        fail("r3", (f"LEAF root_sid={leaf.get('root_sid')} depth={leaf.get('fork_depth')} "
                    f"created={leaf.get('created')!r} title={leaf.get('title')!r} "
                    f"size={leaf.get('size')} want root created/title, depth 2, size {leaf_size}"),
             json.dumps(leaf, ensure_ascii=False).encode())
    if mid.get("root_sid") != "root-1" or mid.get("fork_depth") != 1:
        fail("r3", f"MID root_sid={mid.get('root_sid')} depth={mid.get('fork_depth')}",
             json.dumps(mid, ensure_ascii=False).encode())
    if orphan.get("root_sid") not in (None, "") or orphan.get("fork_depth") is not None:
        fail("r3", f"ORPHAN root_sid={orphan.get('root_sid')!r} depth={orphan.get('fork_depth')!r}",
             json.dumps(orphan, ensure_ascii=False).encode())
    if orphan.get("title") != "Gone prefix question":
        fail("r3", f"ORPHAN title {orphan.get('title')!r} want first user message",
             json.dumps(orphan, ensure_ascii=False).encode())
    if "2026-09-11T13:00:00" not in str(orphan.get("created")):
        fail("r3", f"ORPHAN created {orphan.get('created')!r} want own meta timestamp",
             json.dumps(orphan, ensure_ascii=False).encode())
    if orphan.get("created") == root.get("created"):
        fail("r3", "ORPHAN created must not inherit ROOT",
             json.dumps(orphan, ensure_ascii=False).encode())
    passed("r3 fork chain created/title/size; orphan ends at missing parent")

    check_messages(opener, base, corpus, "leaf-1",
                   "r2 LEAF messages are the physical file (copied history, no inherit)")
    check_messages(opener, base, corpus, "orphan-1",
                   "r2 ORPHAN messages are the physical file")

    again, araw = fetch(opener, base, "/api/sessions?force=1")
    if again.get("sig") != data["sig"]:
        fail("sig", "force=1 sig must be stable with no file changes", araw)
    passed("sig stable across two force=1 lists")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--fixtures-only", type=Path, metavar="DIR",
                        help="write the synthetic corpus into DIR and exit 0")
    args = parser.parse_args()
    if args.fixtures_only is not None:
        root = args.fixtures_only
        root.mkdir(parents=True, exist_ok=True)
        corpus = build(root)
        for sid in SIDS:
            print(corpus.paths[sid], flush=True)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-codex-legacy-fork-") as tmp:
        corpus = build(Path(tmp))
        with isolated_server(corpus, args.binary) as (base, opener):
            run(opener, base, corpus)


if __name__ == "__main__":
    main()
