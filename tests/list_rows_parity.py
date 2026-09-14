#!/usr/bin/env python3
"""Session-list row parity: every /api/sessions row field must equal the Python adapters' list_sessions derivation on the same synthetic corpus.

Builds a fixture_gen corpus plus head/tail edge sessions and the batch-35
real-root shapes (legacy self-contained Codex forks with copied session_meta
records, a rewind past the parent's fork point, copied-meta subagents, orphan
agents, a torn Claude line, a missing leaf, a parent cycle), lists each source
through the Python adapters (advanced_parity load + shadow_compare bind),
fetches isolated GET /api/sessions?force=1, and compares uid/source/sid/title/
cwd/created/updated/size/model/branch/forked_from_id/root_sid/fork_depth/
agent_items/supported/migration_warnings plus `continued_in` and
every `agent_items[]` entry's `active`/`created`/`updated`. Same UTC instant
is equal. Remaining `updated` timestamp mismatches (mtime vs last-record,
seconds vs millis) are the documented DELTA; everything else DIFF. Orphan
agents must be absent on both sides and the batch-35 rows must carry exactly
the expected warnings (`跳过重复的Codex session_meta ×N`, `跳过无效的JSONL 记录 ×N`).
The batch-36 shapes are the Python `ClaudeAgentItemTests` / Codex subagent /
`continued_in` cases: done/streamed/resumed/killed/fresh/foreground sidecars
with the owner's notices, late notice copies + refusal, Codex
running/done/aborted/resumed, a `continued-in` tail record resolved, dangling
and self-referencing.
"""
from __future__ import annotations

import argparse, json, sys, tempfile
from datetime import datetime, timezone
from pathlib import Path

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from fixture_gen import SOURCES, generate
from history_parity import (BATCH35_ROWS, BINARY as DEBUG, REPO, Corpus, claude_row, codex_message,
                            codex_row, encoded, get_json, isolated_server)
from provider_parity import adapter_module, load_adapters
from python_oracle import source_file

RELEASE = REPO / "target/release" / DEBUG.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG
FIELDS = ("uid", "source", "sid", "title", "cwd", "created", "updated", "size",
          "model", "branch", "forked_from_id", "root_sid", "fork_depth", "agent_items", "supported",
          "migration_warnings", "continued_in", "agent_states")
DELTA = {"updated"}
# Python has no migration_warnings, and neither does a
# supported Rust row: the non-fatal counts live in the detail meta only (see
# advanced_parity / history_parity). Both sides therefore compare as "absent"
# on supported rows; an unsupported Rust row carries its fatal reason.


def clip(v, n=96):
    t = v if isinstance(v, str) else json.dumps(v, ensure_ascii=False, default=str)
    return t if len(t) <= n else t[:n] + "…"


def parse_ts(v):
    try:
        return datetime.fromisoformat(str(v).replace("Z", "+00:00")).astimezone(timezone.utc)
    except (TypeError, ValueError):
        return None


def instant(v):
    parsed = parse_ts(v)
    return parsed.isoformat(timespec="milliseconds") if parsed else v


def value(row, key):
    if key == "agent_items":
        return sorted(str(x.get("id")) for x in (row.get("agent_items") or []) if x.get("id"))
    if key == "agent_states":
        # `active` is a plain bool on both sides; created/updated
        # are the same instant (Python local-tz ISO, Rust UTC millis).
        return {str(x.get("id")): {"active": x.get("active"), "created": instant(x.get("created")),
                                   "updated": instant(x.get("updated"))}
                for x in (row.get("agent_items") or []) if x.get("id")}
    if key == "supported":
        return False if row.get("supported") is False else True
    if key == "migration_warnings":
        if row.get("supported") is False:  # Rust unsupported row: the fatal reason
            return list(row.get("migration_warnings") or [])
        return None  # supported rows carry none on either side
    v = row.get(key)
    return None if v in (None, "") else v


def bind(python_source, root):
    inst = load_adapters(python_source, fixture_root=root)
    mod = adapter_module(inst)
    mod.media.register_path = lambda *a, **k: None
    for name, methods in (("claude", ("list_sessions",)), ("grok", ("list_sessions",)),
                          ("codex", ("list_sessions", "scan_sessions", "_find_session_path"))):
        cls = getattr(mod, name.title() + "Adapter")
        for method in methods:
            setattr(inst[name], method, getattr(cls, method).__get__(inst[name]))
    return inst


def python_rows(adapters, source):
    """Python `index._finalize`: Claude `list_sessions` yields raw `_meta` rows and
    `finalize_sessions` turns `continued_in_sid` into `continued_in`; Codex
    `list_sessions` is already finalized."""
    rows = adapters[source].list_sessions() or []
    finalize = getattr(adapters[source], "finalize_sessions", None)
    if source == "claude" and finalize:
        rows = finalize(rows)
    return rows


def pad(sid, nbytes, text="Head Generated Title Please Ignore", **kw):
    rows, n, size = [], 0, 0
    while size < nbytes:
        body = text if n == 0 else "P" * 2048
        row = claude_row(sid, "user", f"p{n}", None, body, **kw)
        rows.append(row)
        size += len(encoded(row))
        n += 1
    return rows


def edges(corpus, python_source):
    sid = "claude-tail-title"
    rows = pad(sid, 600 * 1024)
    rows.append({"type": "custom-title", "customTitle": "Tail Custom Title",
                 "sessionId": sid, "timestamp": "2026-09-11T12:00:00Z"})
    corpus.put(sid, "claude", rows, [])
    sid = "claude-huge-head"
    corpus.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "HugeHeadTitle " + "X" * (96 * 1024)),
        claude_row(sid, "assistant", "a0", "u0", "tiny reply")], [])
    sid = "claude-tail-cwd"
    rows = [claude_row(sid, "user", f"h{i}", None, f"head {i}", cwd="") for i in range(40)]
    rows += [claude_row(sid, "user", "rare", None, "rare", cwd="/rare-cwd")]
    rows += [claude_row(sid, "user", f"c{i}", None, f"common {i}", cwd="/common-cwd") for i in range(5)]
    corpus.put(sid, "claude", rows, [])
    parent = [codex_row("session_meta", {"id": "codex-edge-parent", "session_id": "codex-edge-parent",
               "timestamp": "2026-09-11T09:00:00Z", "cwd": "/synthetic/edge"}),
              codex_message("user", "Codex edge parent"),
              codex_message("assistant", "parent a", 2)]
    cut = sum(len(encoded(r)) for r in parent)
    corpus.put("codex-edge-parent", "codex", parent, [])
    corpus.put("codex-edge-fork", "codex", [
        codex_row("session_meta", {"id": "codex-edge-fork", "session_id": "codex-edge-fork",
                   "timestamp": "2026-09-11T11:00:00Z", "cwd": "/synthetic/edge",
                   "forked_from_id": "codex-edge-parent", "history_mode": "paginated",
                   "history_base": {"thread_id": "codex-edge-parent", "end_byte_offset": cut}}),
        codex_message("user", "fork q")], [])
    text = source_file(python_source, "adapters.py").read_text(encoding="utf-8")
    block = text[text.find("class CodexAdapter"):text.find("class GrokAdapter")]
    if "ai-title" in block or "aiTitle" in block:
        sid = "codex-tail-rename"
        corpus.put(sid, "codex", [
            codex_row("session_meta", {"id": sid, "session_id": sid,
                       "timestamp": "2026-09-11T09:00:00Z", "cwd": "/synthetic/edge"}),
            codex_message("user", "rename base"),
            {"type": "ai-title", "payload": {"aiTitle": "Codex Tail Rename"},
             "timestamp": "2026-09-11T12:00:00Z"}], [])
    gdir = corpus.root / "grok/%2Fsynthetic%2Fedge" / "grok-edge"
    gdir.mkdir(parents=True)
    (gdir / "summary.json").write_text(json.dumps({
        "generated_title": "Grok edge title", "session_summary": "edge",
        "info": {"id": "grok-edge", "cwd": "/synthetic/edge"},
        "created_at": "2026-09-11T08:00:00Z", "updated_at": "2026-09-11T08:30:00Z",
        "current_model_id": "grok-test", "agent_name": "grok-branch"}), encoding="utf-8")
    (gdir / "chat_history.jsonl").write_bytes(encoded({
        "type": "user", "content": "hello grok", "prompt_index": 0,
        "timestamp": "2026-09-11T08:00:01Z"}))
    batch36(corpus)


def notice_text(agent, status):
    return (f"<task-notification>\n<task-id>{agent}</task-id>\n<status>{status}</status>\n"
            "<summary>Agent finished</summary>\n</task-notification>")


def notice(ts, agent, status, shape="attachment"):
    text = notice_text(agent, status)
    if shape == "user":
        return {"type": "user", "timestamp": ts, "message": {"role": "user", "content": text}}
    return {"type": "attachment", "timestamp": ts,
            "attachment": {"type": "queued_command", "commandMode": "task-notification",
                           "prompt": text, "timestamp": ts}}


def agent_result(ts, agent, status, tool_use_id):
    return {"type": "user", "timestamp": ts,
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": tool_use_id,
                 "content": [{"type": "text", "text": "Async agent launched successfully"
                              if status == "async_launched" else "报告"}]}]},
            "toolUseResult": {"status": status, "agentId": agent}}


def sidecar(corpus, owner, agent, rows, tool_use_id="toolu_" + "x" * 20):
    path = corpus.paths[owner].with_suffix("") / "subagents" / f"agent-{agent}.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(encoded({"isSidechain": True, "agentId": agent, **row}) for row in rows))
    path.with_suffix(".meta.json").write_text(json.dumps({
        "agentType": "general-purpose", "description": f"任务 {agent}", "toolUseId": tool_use_id}))
    corpus.paths[agent] = path


def a_user(ts, text="继续"):
    return {"type": "user", "timestamp": ts, "message": {"role": "user", "content": text}}


def a_assistant(ts, stop_reason, kind="text"):
    block = ({"type": "text", "text": "结论"} if kind == "text"
             else {"type": "tool_use", "id": "toolu_call", "name": "Bash", "input": {}})
    return {"type": "assistant", "timestamp": ts,
            "message": {"role": "assistant", "stop_reason": stop_reason, "content": [block]}}


def a_tool_result(ts):
    return {"type": "user", "timestamp": ts, "message": {"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "toolu_call", "content": "ok"}]}}


def batch36(corpus):
    """The Python `ClaudeAgentItemTests`, Codex subagent turn-state and `continued_in` cases."""
    sid = "claude-agents-owner"
    corpus.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "主任务", cwd="/tmp/project", timestamp="2026-09-12T00:00:00.000Z"),
        notice("2026-09-12T00:12:00.100Z", "done", "completed"),
        notice("2026-09-12T00:22:00.100Z", "streamed", "completed", shape="user"),
        notice("2026-09-12T00:31:00.100Z", "resumed", "failed"),
        notice("2026-09-12T00:52:00.000Z", "killed", "killed"),
        agent_result("2026-09-12T01:00:00.100Z", "fresh", "async_launched", "toolu_fresh"),
        agent_result("2026-09-12T01:12:00.200Z", "foreground", "completed", "toolu_fg")], [])
    sidecar(corpus, sid, "done", [a_user("2026-09-12T00:10:00.000Z"),
                                  a_assistant("2026-09-12T00:10:05.000Z", "tool_use", "tool"),
                                  a_tool_result("2026-09-12T00:10:06.000Z"),
                                  a_assistant("2026-09-12T00:12:00.000Z", "end_turn")])
    sidecar(corpus, sid, "streamed", [a_user("2026-09-12T00:20:00.000Z"),
                                      a_assistant("2026-09-12T00:22:00.000Z", None)])
    sidecar(corpus, sid, "resumed", [a_user("2026-09-12T00:30:00.000Z"),
                                     a_assistant("2026-09-12T00:31:00.000Z", None),
                                     a_user("2026-09-12T00:40:00.000Z", "再来一次"),
                                     a_assistant("2026-09-12T00:41:00.000Z", "tool_use", "tool")])
    sidecar(corpus, sid, "killed", [a_user("2026-09-12T00:50:00.000Z"),
                                    a_assistant("2026-09-12T00:51:00.000Z", "tool_use", "tool")])
    sidecar(corpus, sid, "fresh", [a_user("2026-09-12T01:00:00.000Z")], tool_use_id="toolu_fresh")
    sidecar(corpus, sid, "foreground", [a_user("2026-09-12T01:10:00.000Z"),
                                        a_assistant("2026-09-12T01:12:00.000Z", None)], tool_use_id="toolu_fg")
    sid = "claude-agents-late"
    text = notice_text("worker", "completed")
    corpus.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "主任务", cwd="/tmp/project", timestamp="2026-09-12T00:00:00.000Z"),
        {"type": "queue-operation", "operation": "enqueue", "timestamp": "2026-09-12T00:19:00.650Z", "content": text},
        {"type": "queue-operation", "operation": "remove", "timestamp": "2026-09-12T00:21:00.400Z", "content": text},
        notice("2026-09-12T00:19:00.650Z", "worker", "completed"),
        {"type": "queue-operation", "operation": "enqueue", "timestamp": "2026-09-12T00:33:00.000Z", "content": text},
        notice("2026-09-12T00:35:00.000Z", "worker", "completed", shape="user")], [])
    sidecar(corpus, sid, "worker", [a_user("2026-09-12T00:10:00.000Z"),
                                    a_assistant("2026-09-12T00:19:00.600Z", "end_turn"),
                                    a_user("2026-09-12T00:19:00.700Z", "追加一条任务"),
                                    a_assistant("2026-09-12T00:19:30.000Z", "refusal"),
                                    a_assistant("2026-09-12T00:20:59.000Z", "tool_use", "tool")])
    sid = "claude-agents-stopped"
    corpus.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "主任务", cwd="/tmp/project", timestamp="2026-09-12T00:00:00.000Z"),
        notice("2026-09-12T00:19:00.650Z", "worker", "completed"),
        notice("2026-09-12T00:40:00.000Z", "worker", "failed")], [])
    sidecar(corpus, sid, "worker", [a_user("2026-09-12T00:10:00.000Z"),
                                    a_assistant("2026-09-12T00:20:59.000Z", "tool_use", "tool")])

    parent_id = "codex-agents-parent"

    def event(ts, kind, **payload):
        return {"type": "event_msg", "timestamp": ts, "payload": {"type": kind, "turn_id": "t", **payload}}

    def agent_meta(name):
        return {"type": "session_meta", "timestamp": "2026-09-12T01:00:00Z",
                "payload": {"id": name, "session_id": parent_id, "parent_thread_id": parent_id,
                            "thread_source": "subagent",
                            "source": {"subagent": {"thread_spawn": {"parent_thread_id": parent_id, "depth": 1,
                                                                     "agent_path": f"/root/{name}"}}},
                            "timestamp": "2026-09-12T01:00:00Z", "cwd": "/tmp/project"}}

    corpus.put(parent_id, "codex", [
        {"type": "session_meta", "timestamp": "2026-09-12T00:59:00Z",
         "payload": {"id": parent_id, "session_id": parent_id, "thread_source": "user",
                     "timestamp": "2026-09-12T00:59:00Z", "cwd": "/tmp/project"}}], [])
    corpus.put("codex-agent-running", "codex", [agent_meta("codex-agent-running"),
        event("2026-09-12T01:01:00Z", "task_started"),
        {"type": "response_item", "timestamp": "2026-09-12T01:02:00Z", "payload": {"type": "reasoning"}},
        {"type": "event_msg", "timestamp": "2026-09-12T01:03:00Z", "payload": {"type": "token_count"}}], [])
    corpus.put("codex-agent-done", "codex", [agent_meta("codex-agent-done"),
        event("2026-09-12T01:01:00Z", "task_started"),
        event("2026-09-12T01:10:00Z", "task_complete", completed_at=1789166200)], [])
    corpus.put("codex-agent-aborted", "codex", [agent_meta("codex-agent-aborted"),
        event("2026-09-12T01:01:00Z", "task_started"),
        event("2026-09-12T01:05:00Z", "turn_aborted", reason="interrupted")], [])
    corpus.put("codex-agent-resumed", "codex", [agent_meta("codex-agent-resumed"),
        event("2026-09-12T01:01:00Z", "task_started"),
        event("2026-09-12T01:05:00Z", "task_complete"),
        event("2026-09-12T01:20:00Z", "task_started"),
        {"type": "response_item", "timestamp": "2026-09-12T01:21:00Z", "payload": {"type": "message"}}], [])

    def continued(sid, target, ts):
        return {"type": "continued-in", "sessionId": sid, "continuedInSessionId": target, "timestamp": ts}

    corpus.put("claude-continued-parent", "claude", [
        claude_row("claude-continued-parent", "user", "u0", None, "hello", cwd="/repo", timestamp="2026-09-12T10:00:00Z"),
        continued("claude-continued-parent", "claude-continued-child", "2026-09-12T12:00:00Z")], [])
    corpus.put("claude-continued-child", "claude", [
        claude_row("claude-continued-child", "user", "u0", None, "continued", cwd="/repo", timestamp="2026-09-12T12:00:01Z")], [])
    corpus.put("claude-continued-dangling", "claude", [
        claude_row("claude-continued-dangling", "user", "u0", None, "hello", cwd="/repo", timestamp="2026-09-12T10:00:00Z"),
        continued("claude-continued-dangling", "claude-nowhere", "2026-09-12T12:00:00Z")], [])
    corpus.put("claude-continued-self", "claude", [
        claude_row("claude-continued-self", "user", "u0", None, "hello", cwd="/repo", timestamp="2026-09-12T10:00:00Z"),
        continued("claude-continued-self", "claude-continued-self", "2026-09-12T12:00:00Z")], [])


def classify(field, a, b, lv, rv):
    if lv == rv:
        return None
    if field in ("created", "updated"):
        lt, rt = parse_ts(a.get(field)), parse_ts(b.get(field))
        if lt and rt and lt == rt:
            return None
        if field in DELTA and lt and rt:
            return "DELTA"
    return "DIFF"


def compare(py, rs):
    counts = {"PASS": 0, "DELTA": 0, "DIFF": 0}
    for uid in sorted(set(py) | set(rs)):
        a, b = py.get(uid), rs.get(uid)
        if not a or not b:
            print(f"DIFF {uid} missing python={clip(a)} rust={clip(b)}", flush=True)
            counts["DIFF"] += 1
            continue
        notes = []
        for field in FIELDS:
            lv, rv = value(a, field), value(b, field)
            kind = classify(field, a, b, lv, rv)
            if kind:
                shown = (a.get(field), b.get(field)) if field in ("created", "updated") else (lv, rv)
                notes.append((kind, field, shown[0], shown[1]))
        worst = "DIFF" if any(k == "DIFF" for k, *_ in notes) else ("DELTA" if notes else "PASS")
        if worst == "PASS":
            print(f"PASS {uid} {a.get('sid')}", flush=True)
        else:
            for kind, field, lv, rv in notes:
                if worst == "DIFF" and kind != "DIFF":
                    continue
                print(f"{kind} {uid} {field} python={clip(lv)} rust={clip(rv)}", flush=True)
        counts[worst] += 1
    return counts


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--python-source", type=Path, required=True)
    p.add_argument("--binary", type=Path, default=BINARY)
    args = p.parse_args(argv)
    python_source = args.python_source.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-list-rows-") as tmp:
        root = Path(tmp)
        generate(root, SOURCES, 3, 50, 0, 1, 1, 1, batch35=True)
        corpus = Corpus(root)
        edges(corpus, python_source)
        adapters = bind(python_source, root)
        py = {row["uid"]: row for src in SOURCES for row in python_rows(adapters, src)}
        with isolated_server(corpus, args.binary) as (base, opener):
            listed = get_json(opener, base, "/api/sessions?force=1")
        counts = compare(py, {row["uid"]: row for row in listed.get("sessions") or []})
    print(f"SUMMARY {counts['PASS']} PASS, {counts['DELTA']} DELTA, {counts['DIFF']} DIFF", flush=True)
    raise SystemExit(1 if counts["DIFF"] else 0)


if __name__ == "__main__":
    main()
