#!/usr/bin/env python3
"""HTTP contract for orphan sub-agents.

Claude sidecars without an owner file and Codex subagents whose
parent_thread_id is not indexed are absent from GET /api/sessions.
GET /api/messages of their physical uid is HTTP 501 JSON (error or
message). Agents with an indexed owner appear only in that owner's
agent_items; deleting the owner also drops its sidecar from the list.
"""
from __future__ import annotations

import argparse, json, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote, urlencode

from history_parity import (
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, codex_message, codex_row,
    encoded, get_json, isolated_server)

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
ORDER = ("O", "a1", "a2", "P", "S1", "S2")
CWD = "/synthetic/orphan"


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    extra = f"; {text[:240]}" if text else ""
    raise SystemExit(f"FAIL {area}: {why}{extra}")


def passed(area):
    print(f"PASS {area}", flush=True)


def ok(opener, base, route, area):
    try:
        return get_json(opener, base, route)
    except HTTPError as err:
        fail(area, f"HTTP {err.code} (want 200)", err.read(4096))


def messages_route(uid, **query):
    path = "/api/messages/" + quote(uid, safe=":")
    return path + ("?" + urlencode(query) if query else "")


def put_bytes(corpus, key, path, rows):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(encoded(row) for row in rows))
    corpus.paths[key] = path
    return path


def claude_turn(sid, prefix, text, **extra):
    return [
        claude_row(sid, "user", prefix + "-u", None, text + " q", **extra),
        claude_row(sid, "assistant", prefix + "-a", prefix + "-u", text + " a", **extra),
    ]


def meta(sid, session_id=None, **extra):
    payload = {
        "id": sid, "session_id": session_id or sid, "forked_from_id": extra.pop("forked_from_id", None),
        "thread_source": extra.pop("thread_source", "user"), "history_base": extra.pop("history_base", None),
        "cwd": CWD, "timestamp": "2026-09-11T10:00:00Z", **extra}
    return codex_row("session_meta", payload)


def spawn(parent, nickname):
    return {"subagent": {"thread_spawn": {
        "parent_thread_id": parent, "depth": 1, "agent_path": "/root/x",
        "agent_nickname": nickname, "agent_role": None}}}


def turn(tid, user, assistant, ordinal=1):
    return [
        codex_row("event_msg", {"type": "task_started", "turn_id": tid}, ordinal),
        codex_message("user", user, ordinal + 1),
        codex_message("assistant", assistant, ordinal + 2),
        codex_row("event_msg", {"type": "task_complete", "turn_id": tid}, ordinal + 3),
    ]


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)
    claude = root / "claude" / "proj"
    put_bytes(corpus, "O", claude / "O.jsonl", claude_turn("O", "o", "owner O", cwd=CWD))
    put_bytes(corpus, "a1", claude / "O" / "subagents" / "agent-a1.jsonl",
              claude_turn("O", "a1", "sidecar a1", isSidechain=True, agentId="a1", cwd=CWD))
    put_bytes(corpus, "a2", claude / "GONE" / "subagents" / "agent-a2.jsonl",
              claude_turn("GONE", "a2", "orphan a2", isSidechain=True, agentId="a2", cwd=CWD))
    p_meta = meta("P")
    put_bytes(corpus, "P", root / "codex/2026/09/11/rollout-P.jsonl",
              [p_meta, *turn("t-p", "parent P q", "parent P a")])
    s1_meta = meta("S1", session_id="P", forked_from_id="P", thread_source="subagent",
                   parent_thread_id="P", source=spawn("P", "Name"))
    put_bytes(corpus, "S1", root / "codex/2026/09/11/rollout-S1.jsonl",
              [s1_meta, p_meta, *turn("t-s1", "S1 agent q", "S1 agent a")])
    s2_meta = meta("S2", session_id="nope", forked_from_id="nope", thread_source="subagent",
                   parent_thread_id="nope", source=spawn("nope", "Orphan"))
    put_bytes(corpus, "S2", root / "codex/2026/09/11/rollout-S2.jsonl",
              [s2_meta, *turn("t-s2", "S2 orphan q", "S2 orphan a")])
    corpus.owners["S1"] = "P"
    return corpus


def texts_of(body):
    return [row.get("text") for row in body.get("messages") or [] if isinstance(row, dict)]


def expect_501(opener, base, uid, area):
    route = messages_route(uid)
    try:
        with opener.open(base + route, timeout=15) as resp:
            raw, code = resp.read(4096), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    if code != 501:
        fail(area, f"HTTP {code} (want 501)", raw)
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        fail(area, "501 body is not JSON", raw)
    if not isinstance(payload, dict) or not (payload.get("error") or payload.get("message")):
        fail(area, "501 JSON needs error or message", raw)


def run(opener, base, corpus):
    data = ok(opener, base, "/api/sessions?force=1", "list")
    rows = data.get("sessions")
    if not isinstance(rows, list):
        fail("list", "sessions is not a list", json.dumps(data).encode())
    by = {row["sid"]: row for row in rows if isinstance(row, dict) and row.get("sid")}
    if set(by) != {"O", "P"}:
        fail("list", f"listed {sorted(by)} want ['O', 'P']", json.dumps(sorted(by)).encode())
    if by["O"].get("uid") != corpus.uid("O") or by["P"].get("uid") != corpus.uid("P"):
        fail("list", "O/P uid mismatch", json.dumps({"O": by["O"].get("uid"), "P": by["P"].get("uid")}).encode())
    passed("list top-level O,P only")

    o_items = by["O"].get("agent_items") or []
    p_items = by["P"].get("agent_items") or []
    if len(o_items) != 1 or not isinstance(o_items[0], dict) or o_items[0].get("id") != "a1":
        fail("agent_items", "O.agent_items must be one item id a1", json.dumps(o_items).encode())
    if len(p_items) != 1 or not isinstance(p_items[0], dict) or p_items[0].get("id") != "S1":
        fail("agent_items", "P.agent_items must be one item id S1", json.dumps(p_items).encode())
    if by["P"].get("agents") != 1:
        fail("agent_items", f"P.agents={by['P'].get('agents')!r} want 1", json.dumps(by["P"]).encode())
    passed("owner agent_items")

    a1 = ok(opener, base, messages_route(corpus.uid("O"), agent="a1"), "agent-view")
    if "sidecar a1 q" not in texts_of(a1):
        fail("agent-view", "O?agent=a1 missing sidecar messages", json.dumps(a1.get("messages")).encode())
    s1 = ok(opener, base, messages_route(corpus.uid("P"), agent="S1"), "agent-view")
    if "S1 agent q" not in texts_of(s1):
        fail("agent-view", "P?agent=S1 missing subagent messages", json.dumps(s1.get("messages")).encode())
    passed("agent views 200")

    expect_501(opener, base, corpus.uid("a2"), "orphan-claude")
    expect_501(opener, base, corpus.uid("S2"), "orphan-codex")
    passed("orphan uid 501")

    corpus.paths["O"].unlink()
    data = ok(opener, base, "/api/sessions?force=1", "owner-deleted")
    rows = data.get("sessions") if isinstance(data.get("sessions"), list) else []
    by = {row["sid"]: row for row in rows if isinstance(row, dict) and row.get("sid")}
    a1_uid = corpus.uid("a1")
    if set(by) != {"P"} or any(row.get("uid") == a1_uid for row in rows):
        fail("owner-deleted", f"listed {sorted(by)} want ['P'] (a1 must not surface)",
             json.dumps(sorted(by)).encode())
    passed("owner-deleted sidecar absent")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--fixtures-only", type=Path, metavar="DIR",
                        help="write the synthetic corpus into DIR, print paths, exit 0")
    args = parser.parse_args()
    if args.fixtures_only is not None:
        root = args.fixtures_only.resolve()
        root.mkdir(parents=True, exist_ok=True)
        corpus = build(root)
        for key in ORDER:
            print(corpus.paths[key])
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-orphan-agents-") as tmp:
        corpus = build(Path(tmp))
        with isolated_server(corpus, args.binary) as (base, opener):
            run(opener, base, corpus)


if __name__ == "__main__":
    main()
