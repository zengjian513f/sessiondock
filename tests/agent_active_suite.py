#!/usr/bin/env python3
"""HTTP contract for GET /api/sessions `agent_items[].active/created/updated`.

Claude sidecar `active` is true only when the last turn is not closed
(`stop_reason` in {end_turn} closes it; `refusal` does not) and the owner has
no later `<task-notification>` naming that agent. Codex subagent `active`
follows the last `event_msg` kind: task_started/turn_started open,
task_complete/turn_complete/turn_aborted closed. `created`/`updated` are ISO
instants with created ≤ updated; a Codex item's `updated` is its last record
timestamp. Append + GET ?force=1 must flip a-stream and s-run to inactive.
Sidecars and subagents are never top-level list rows.
"""
from __future__ import annotations

import argparse, json, tempfile
from datetime import datetime, timezone
from pathlib import Path
from urllib.error import HTTPError

from history_parity import (
    BINARY as DEBUG_BINARY, REPO, Corpus, batch35_agent_meta, claude_row,
    codex_message, codex_row, encoded, get_json, isolated_server)

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
CWD = "/synthetic/agent-active"
FILES = ("O", "a-done", "a-stream", "a-killed", "a-refusal", "P", "s-run", "s-done", "s-abort")
O_ACTIVE = (("a-done", False), ("a-stream", True), ("a-killed", False), ("a-refusal", True))
P_ACTIVE = (("s-run", True), ("s-done", False), ("s-abort", False))
S_RUN_LAST = "2026-09-11T11:10:00.000Z"
NOTICE = ("<task-notification>\n<task-id>{agent}</task-id>\n"
          "<status>{status}</status>\n<summary>Agent finished</summary>\n"
          "</task-notification>")


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    extra = f"; {text[:240]}" if text else ""
    raise SystemExit(f"FAIL {area}: {why}{extra}")


def passed(area):
    print(f"PASS {area}", flush=True)


def stamp(row, ts):
    row["timestamp"] = ts
    return row


def parse_iso(value):
    if not isinstance(value, str) or not value.strip():
        return None
    try:
        dt = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return dt if dt.tzinfo else dt.replace(tzinfo=timezone.utc)


def notice(sid, uid, parent, agent, ts, status):
    return claude_row(sid, "user", uid, parent, NOTICE.format(agent=agent, status=status),
                      timestamp=ts, cwd=CWD)


def side_turn(agent, user_ts, asst_ts, stop_reason):
    extra = {"isSidechain": True, "agentId": agent, "cwd": CWD}
    user = claude_row("O", "user", agent + "-u", None, agent + " q", timestamp=user_ts, **extra)
    asst = claude_row("O", "assistant", agent + "-a", agent + "-u", agent + " a",
                      timestamp=asst_ts, **extra)
    asst["message"]["stop_reason"] = stop_reason
    return [user, asst]


def put_bytes(corpus, key, path, rows):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(encoded(row) for row in rows))
    corpus.paths[key] = path
    return path


def event(kind, ts, ordinal, turn):
    return stamp(codex_row("event_msg", {"type": kind, "turn_id": turn}, ordinal), ts)


def agent_meta(sid, owner, ts):
    return stamp(batch35_agent_meta(sid, owner, ts), ts)


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)
    corpus.put("O", "claude", [
        claude_row("O", "user", "o-u", None, "owner O", cwd=CWD, timestamp="2026-09-11T12:00:00Z"),
        claude_row("O", "assistant", "o-a", "o-u", "owner a", cwd=CWD, timestamp="2026-09-11T12:00:01Z"),
        notice("O", "o-kill", "o-a", "a-killed", "2026-09-11T12:40:00Z", "killed")], [])
    claude_dir = corpus.paths["O"].with_suffix("") / "subagents"
    put_bytes(corpus, "a-done", claude_dir / "agent-a-done.jsonl",
              side_turn("a-done", "2026-09-11T12:10:00Z", "2026-09-11T12:11:00Z", "end_turn"))
    put_bytes(corpus, "a-stream", claude_dir / "agent-a-stream.jsonl",
              side_turn("a-stream", "2026-09-11T12:20:00Z", "2026-09-11T12:21:00Z", None))
    put_bytes(corpus, "a-killed", claude_dir / "agent-a-killed.jsonl",
              side_turn("a-killed", "2026-09-11T12:30:00Z", "2026-09-11T12:31:00Z", None))
    put_bytes(corpus, "a-refusal", claude_dir / "agent-a-refusal.jsonl",
              side_turn("a-refusal", "2026-09-11T12:50:00Z", "2026-09-11T12:51:00Z", "refusal"))
    pts = "2026-09-11T10:00:00Z"
    corpus.put("P", "codex", [
        stamp(codex_row("session_meta", {
            "id": "P", "session_id": "P", "timestamp": pts, "cwd": CWD, "thread_source": "user"}), pts),
        stamp(codex_message("user", "parent P", 1), pts)], [])
    put_bytes(corpus, "s-run", root / "codex/2026/09/11/rollout-s-run.jsonl", [
        agent_meta("s-run", "P", "2026-09-11T11:00:00Z"), event("task_started", S_RUN_LAST, 1, "t-run")])
    put_bytes(corpus, "s-done", root / "codex/2026/09/11/rollout-s-done.jsonl", [
        agent_meta("s-done", "P", "2026-09-11T11:20:00Z"),
        event("task_started", "2026-09-11T11:21:00Z", 1, "t-done"),
        event("task_complete", "2026-09-11T11:22:00Z", 2, "t-done")])
    put_bytes(corpus, "s-abort", root / "codex/2026/09/11/rollout-s-abort.jsonl", [
        agent_meta("s-abort", "P", "2026-09-11T11:30:00Z"),
        event("task_started", "2026-09-11T11:31:00Z", 1, "t-abort"),
        event("turn_aborted", "2026-09-11T11:32:00Z", 2, "t-abort")])
    return corpus


def ok(opener, base, route, area):
    try:
        return get_json(opener, base, route)
    except HTTPError as err:
        fail(area, f"HTTP {err.code} (want 200)", err.read(4096))


def listed(opener, base, corpus, area):
    data = ok(opener, base, "/api/sessions?force=1", area)
    rows = data.get("sessions")
    if not isinstance(rows, list):
        fail(area, "sessions is not a list", json.dumps(data).encode())
    by = {row["sid"]: row for row in rows if isinstance(row, dict) and row.get("sid")}
    if set(by) != {"O", "P"}:
        fail(area, f"listed {sorted(by)} want ['O', 'P']", json.dumps(sorted(by)).encode())
    if by["O"].get("uid") != corpus.uid("O") or by["P"].get("uid") != corpus.uid("P"):
        fail(area, "O/P uid mismatch", json.dumps({"O": by["O"].get("uid"), "P": by["P"].get("uid")}).encode())
    return by


def expect_items(items, expected, area):
    if not isinstance(items, list):
        fail(area, "agent_items is not a list", json.dumps(items).encode())
    got = {item.get("id"): item for item in items if isinstance(item, dict) and item.get("id")}
    want_ids = [aid for aid, _ in expected]
    if set(got) != set(want_ids):
        fail(area, f"ids {sorted(got)} want {want_ids}", json.dumps(items).encode())
    for aid, want in expected:
        item = got[aid]
        if item.get("active") is not want:
            fail(area, f"{aid} active={item.get('active')!r} want {want}", json.dumps(item).encode())
        created, updated = parse_iso(item.get("created")), parse_iso(item.get("updated"))
        if created is None or updated is None:
            fail(area, f"{aid} created/updated must be ISO strings", json.dumps(item).encode())
        if created > updated:
            fail(area, f"{aid} created > updated", json.dumps(item).encode())
    passed(area)
    return got


def last_record_ts(path):
    return json.loads(path.read_bytes().splitlines()[-1]).get("timestamp")


def append_row(path, row):
    with path.open("ab") as fh:
        fh.write(encoded(row))


def run(opener, base, corpus):
    by = listed(opener, base, corpus, "list")
    passed("list top-level O,P only")
    expect_items(by["O"].get("agent_items"), O_ACTIVE, "claude agent_items")
    p_items = expect_items(by["P"].get("agent_items"), P_ACTIVE, "codex agent_items")
    want, got = parse_iso(last_record_ts(corpus.paths["s-run"])), parse_iso(p_items["s-run"].get("updated"))
    if want is None or got != want:
        fail("s-run updated", f"updated={p_items['s-run'].get('updated')!r} want last record {last_record_ts(corpus.paths['s-run'])!r}",
             json.dumps(p_items["s-run"]).encode())
    passed("s-run updated is last record timestamp")

    append_row(corpus.paths["s-run"], event("task_complete", "2026-09-11T11:15:00.000Z", 2, "t-run"))
    by = listed(opener, base, corpus, "s-run flip")
    expect_items(by["P"].get("agent_items"), (("s-run", False), ("s-done", False), ("s-abort", False)),
                 "s-run flipped inactive")

    append_row(corpus.paths["O"], notice("O", "o-stream", "o-kill", "a-stream", "2026-09-11T12:45:00Z", "completed"))
    by = listed(opener, base, corpus, "a-stream flip")
    expect_items(by["O"].get("agent_items"),
                 (("a-done", False), ("a-stream", False), ("a-killed", False), ("a-refusal", True)),
                 "a-stream flipped inactive")


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
        for key in FILES:
            print(corpus.paths[key])
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-agent-active-") as tmp:
        corpus = build(Path(tmp))
        with isolated_server(corpus, args.binary) as (base, opener):
            run(opener, base, corpus)


if __name__ == "__main__":
    main()
