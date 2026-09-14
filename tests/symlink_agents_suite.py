#!/usr/bin/env python3
"""HTTP contract: Claude subagent aliases follow normal path handling.

A continued session may link the origin's sidecar into its own subagents/
directory. In-root and external aliases to regular sidecar files are listed;
dangling aliases and directory targets are skipped.
"""
from __future__ import annotations

import argparse, json, os, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote, urlencode

from history_parity import (
    BINARY, Corpus, claude_row, encoded, get_json, isolated_server)
CWD = "/synthetic/symlink-agents"
P_SID, Q_SID = "P", "Q"
A_ID = "aaaaaaaaaaaaaaaaa"
BAD = ("bbbbbbbbbbbbbbbbb", "ccccccccccccccccc", "ddddddddddddddddd")
CHECKS = 0


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    extra = f"; {text[:240]}" if text else ""
    raise SystemExit(f"FAIL {area}: {why}{extra}")


def passed(area):
    global CHECKS
    CHECKS += 1
    print(f"PASS {area}", flush=True)


def ok(opener, base, route, area):
    try:
        return get_json(opener, base, route)
    except HTTPError as err:
        fail(area, f"HTTP {err.code} (want 200)", err.read(4096))


def messages_route(uid, **query):
    path = "/api/messages/" + quote(uid, safe=":")
    return path + ("?" + urlencode(query) if query else "")


def turn(sid, agent, text):
    extra = {"isSidechain": True, "agentId": agent, "cwd": CWD}
    return [
        claude_row(sid, "user", agent + "-u", None, text + " q", **extra),
        claude_row(sid, "assistant", agent + "-a", agent + "-u", text + " a", **extra),
    ]


def owner_rows(sid, text):
    return [
        claude_row(sid, "user", sid + "-u", None, text + " q", cwd=CWD),
        claude_row(sid, "assistant", sid + "-a", sid + "-u", text + " a", cwd=CWD),
    ]


def put_bytes(path, rows):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(encoded(row) for row in rows))


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)
    corpus.put(P_SID, "claude", owner_rows(P_SID, "owner P"), [])
    corpus.put(Q_SID, "claude", owner_rows(Q_SID, "owner Q"), [])
    sidecar = corpus.paths[P_SID].with_suffix("") / "subagents" / f"agent-{A_ID}.jsonl"
    put_bytes(sidecar, turn(P_SID, A_ID, "sidecar a"))
    q_agents = corpus.paths[Q_SID].with_suffix("") / "subagents"
    q_agents.mkdir(parents=True)
    os.symlink(os.path.relpath(sidecar, q_agents), q_agents / f"agent-{A_ID}.jsonl")
    outside = root / "outside" / "x.jsonl"
    put_bytes(outside, turn(Q_SID, BAD[0], "outside b"))
    os.symlink(outside.resolve(), q_agents / f"agent-{BAD[0]}.jsonl")
    os.symlink(q_agents / "missing-target.jsonl", q_agents / f"agent-{BAD[1]}.jsonl")
    target_dir = root / "outside" / "dir-target"
    target_dir.mkdir(parents=True, exist_ok=True)
    os.symlink(target_dir, q_agents / f"agent-{BAD[2]}.jsonl")
    return corpus


def agent_ids(row):
    items = row.get("agent_items")
    if items is None:
        items = []
    if not isinstance(items, list):
        fail("agent_items", "agent_items is not a list", json.dumps(items).encode())
    return [item.get("id") for item in items if isinstance(item, dict)]


def expect_json_error(opener, base, route, area):
    try:
        with opener.open(base + route, timeout=15) as resp:
            raw, code = resp.read(4096), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    if code not in (404, 501):
        fail(area, f"HTTP {code} (want 404 or 501)", raw)
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        fail(area, "error body is not JSON", raw)
    if not isinstance(payload, dict) or not (payload.get("error") or payload.get("message")):
        fail(area, "JSON needs error or message", raw)
    print(f"NOTE {area} HTTP {code}", flush=True)
    passed(area)
    return code


def run(opener, base, corpus):
    data = ok(opener, base, "/api/sessions?force=1", "list")
    rows = data.get("sessions")
    if not isinstance(rows, list):
        fail("list", "sessions is not a list", json.dumps(data).encode())
    by = {row["sid"]: row for row in rows if isinstance(row, dict) and row.get("sid")}
    if set(by) != {P_SID, Q_SID}:
        fail("list", f"listed {sorted(by)} want {sorted([P_SID, Q_SID])}", json.dumps(sorted(by)).encode())
    passed("list P,Q only")

    p_ids, q_ids = agent_ids(by[P_SID]), agent_ids(by[Q_SID])
    if p_ids != [A_ID]:
        fail("P agents", f"ids {p_ids} want [{A_ID}]", json.dumps(by[P_SID]).encode())
    if by[P_SID].get("agents") not in (1, None):
        fail("P agents", f"agents={by[P_SID].get('agents')!r} want 1", json.dumps(by[P_SID]).encode())
    passed("P lists in-root sidecar")
    if q_ids != [A_ID, BAD[0]]:
        fail("Q agents", f"ids {q_ids} want [{A_ID}, {BAD[0]}]", json.dumps(by[Q_SID]).encode())
    if by[Q_SID].get("agents") != 2:
        fail("Q agents", f"agents={by[Q_SID].get('agents')!r} want 2", json.dumps(by[Q_SID]).encode())
    passed("Q lists regular-file symlink targets")

    p_view = ok(opener, base, messages_route(corpus.uid(P_SID), agent=A_ID), "P agent view")
    q_view = ok(opener, base, messages_route(corpus.uid(Q_SID), agent=A_ID), "Q agent view")
    p_n = len(p_view.get("messages") or [])
    q_n = len(q_view.get("messages") or [])
    if p_n < 2 or p_n != q_n:
        fail("agent view", f"P messages={p_n} Q messages={q_n} (want equal, ≥2)",
             json.dumps({"p": p_n, "q": q_n}).encode())
    passed("Q in-root symlink agent view matches P")

    outside_view = ok(opener, base, messages_route(corpus.uid(Q_SID), agent=BAD[0]), "Q external agent view")
    if len(outside_view.get("messages") or []) < 2:
        fail("Q external agent view", "external regular-file alias was not readable")
    passed("Q external regular-file symlink agent view")

    q_uid = corpus.uid(Q_SID)
    for aid in BAD[1:]:
        expect_json_error(opener, base, messages_route(q_uid, agent=aid), f"Q?agent={aid}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-symlink-agents-") as tmp:
        corpus = build(Path(tmp))
        with isolated_server(corpus, args.binary) as (base, opener):
            run(opener, base, corpus)
    print(f"symlink_agents_suite: {CHECKS} checks passed")


if __name__ == "__main__":
    main()
