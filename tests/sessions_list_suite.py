#!/usr/bin/env python3
"""Contract coverage for GET /api/sessions (shape, topology, sig, cursor, scale).

Synthetic temp fixtures and an isolated loopback Rust server only. The list is
the lazy index (docs/read-model.md): no session cap, 1001 physical sessions are
listed in full; a supported row carries the physical `cursor: {end, head}` and
gains `anchor` only once the session has been opened (docs/history-pages.md).
Batch 35: a Codex file with copied ancestor session_meta records and a Claude
file with one torn NUL line list as supported with the exact counted warnings
(`跳过重复的Codex session_meta ×N`, `跳过无效的JSONL 记录 ×N`); orphan agents
(Claude sidecar without owner, Codex subagent whose parent is absent) are not
rows and their physical UID is a typed 501.
Batch 36: every `agent_items[]` entry carries `active` — a Claude sidecar whose
last turn is open and whose owner has no later stop notice, a Codex subagent
whose last turn-boundary event is `task_started`; a Claude row whose tail
names a `continued-in` session carries `continued_in` (the uid) only when that
session is listed.
"""
from __future__ import annotations

import argparse, json, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote
from history_parity import (
    BINARY as DEBUG_BINARY, REPO, Corpus, batch35_agent_meta, claude_attachment_chain, claude_control_rows,
    claude_row, claude_turn_tail_rows, codex_message, codex_row, encoded, isolated_server, torn_line)

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
KEYS = ("uid", "source", "sid", "title", "cwd", "created", "updated", "size")
LISTED = {"claude-parent", "claude-cli-current", "codex-parent", "codex-fork", "grok-summary",
          "codex-dup-meta", "claude-torn-tail", "claude-live-owner", "claude-continued-parent",
          "claude-continued-child", "claude-continued-dangling"}
# Batch 35 R4: never rows; their physical UID answers a typed 501.
ORPHANS = ("claude-orphan", "codex-orphan-agent")
DUP_META_WARNINGS = ["跳过重复的Codex session_meta ×2"]
TORN_WARNINGS = ["跳过无效的JSONL 记录 ×1"]
CLI_WARNINGS = ["跳过未知的Claude 记录类型：mode ×1", "跳过未知的Claude 记录类型：permission-mode ×1",
                "跳过未知的Claude 记录类型：bridge-session ×1", "跳过未知的Claude 记录类型：agent-name ×1",
                "跳过未知的Claude attachment 类型：environment ×1", "跳过未知的Claude attachment 类型：hook_success ×1",
                "跳过未知的Claude 记录类型：atis-latch ×2", "跳过未知的Claude 记录类型：file-history-delta ×1",
                "跳过未知的Claude 记录类型：cost-state ×1"]


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


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    cwd, sid = "/synthetic/中文项目", "claude-parent"
    corpus.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "你好世界", cwd=cwd, gitBranch="main",
                   timestamp="2026-09-11T12:00:00Z"),
        claude_row(sid, "assistant", "a0", "u0", "收到", cwd=cwd, gitBranch="main",
                   timestamp="2026-09-11T12:00:01Z")], [])
    agent = "claude-agent"
    agent_path = corpus.paths[sid].with_suffix("") / "subagents" / f"agent-{agent}.jsonl"
    agent_path.parent.mkdir(parents=True)
    agent_path.write_bytes(b"".join(encoded(row) for row in [
        claude_row(sid, "user", "au", None, "agent q", isSidechain=True, agentId=agent),
        claude_row(sid, "assistant", "aa", "au", "agent a", isSidechain=True, agentId=agent)]))
    agent_path.with_suffix(".meta.json").write_text(
        json.dumps({"description": "sidecar", "agentType": "reviewer"}))
    (root / "claude/project-history/empty-zero.jsonl").write_bytes(b"")
    # Batch 33: a current Claude Code 2.1.x transcript (control records, an
    # attachment chain between the user record and its reply, turn-tail
    # records) is listed as supported with counted skip warnings.
    sid = "claude-cli-current"
    chain, tip = claude_attachment_chain(sid, "c0", "c0-att", kinds=("environment", "hook_success"),
                                         ts="2026-09-11T12:30:00Z")
    corpus.put(sid, "claude", [
        *claude_control_rows(sid),
        claude_row(sid, "user", "c0", None, "current CLI prompt", cwd=cwd, timestamp="2026-09-11T12:30:00Z"),
        *chain,
        {"type": "atis-latch", "atis": {"latched": True}, "sessionId": sid},
        claude_row(sid, "assistant", "c-a0", tip, "current CLI reply", cwd=cwd, timestamp="2026-09-11T12:30:01Z"),
        *claude_turn_tail_rows(sid, "c-a0", "c0")], [])

    def meta(name, ts, **extra):
        return stamp(codex_row("session_meta", {
            "id": name, "session_id": name, "timestamp": ts, "cwd": "/synthetic/中文cwd",
            **extra}), ts)

    parent = [meta("codex-parent", "2026-09-11T10:00:00Z"),
              stamp(codex_message("user", "Codex parent prefix"), "2026-09-11T10:00:01Z"),
              stamp(codex_message("assistant", "Codex inherited", 2), "2026-09-11T10:00:02Z")]
    cutoff = sum(len(encoded(row)) for row in parent)
    corpus.put("codex-parent", "codex", parent + [
        stamp(codex_message("user", "tail", 3), "2026-09-11T10:00:03Z")], [])
    corpus.put("codex-fork", "codex", [
        meta("codex-fork", "2026-09-11T11:00:00Z", forked_from_id="codex-parent",
             history_mode="paginated",
             history_base={"thread_id": "codex-parent", "end_byte_offset": cutoff}),
        stamp(codex_message("user", "fork q"), "2026-09-11T11:00:01Z"),
        stamp(codex_message("assistant", "fork a", 2), "2026-09-11T11:00:02Z")], [])
    gdir = root / "grok/%2Fsynthetic%2F%E4%B8%AD%E6%96%87" / "grok-summary"
    gdir.mkdir(parents=True)
    (gdir / "summary.json").write_text(json.dumps({
        "generated_title": "Grok 摘要标题", "session_summary": "only summary",
        "info": {"id": "grok-summary", "cwd": "/synthetic/中文"},
        "created_at": "2026-09-11T08:00:00Z", "updated_at": "2026-09-11T08:30:00Z",
        "current_model_id": "grok-test", "agent_name": "grok-branch"}), encoding="utf-8")
    corpus.paths["grok-summary"] = gdir
    # Batch 35: an old-style self-contained Codex fork (own meta with null
    # history_base, then the two ancestors' session_meta records copied in,
    # the parent gone), a Claude transcript with one torn NUL line whose
    # lineage stays intact, and two orphan agents.
    corpus.put("codex-dup-meta", "codex", [
        meta("codex-dup-meta", "2026-09-11T09:30:00Z", forked_from_id="codex-dup-gone", history_base=None),
        meta("codex-dup-gone", "2026-09-11T09:10:00Z", forked_from_id="codex-dup-gone-root", history_base=None),
        meta("codex-dup-gone-root", "2026-09-11T09:00:00Z"),
        stamp(codex_message("user", "dup meta q"), "2026-09-11T09:30:01Z"),
        stamp(codex_message("assistant", "dup meta a", 2), "2026-09-11T09:30:02Z")], [])
    sid = "claude-torn-tail"
    corpus.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "torn tail q", cwd=cwd, timestamp="2026-09-11T11:00:00Z"),
        claude_row(sid, "assistant", "a0", "u0", "torn tail a", cwd=cwd, timestamp="2026-09-11T11:00:01Z"),
        torn_line(claude_row(sid, "assistant", "torn-lost", "a0", "lost", cwd=cwd)),
        claude_row(sid, "user", "u1", "a0", "torn tail q2", cwd=cwd, timestamp="2026-09-11T11:00:02Z"),
        claude_row(sid, "assistant", "a1", "u1", "torn tail a2", cwd=cwd, timestamp="2026-09-11T11:00:03Z")], [])
    orphan = root / "claude/project-history/claude-orphan-owner/subagents/agent-claude-orphan.jsonl"
    orphan.parent.mkdir(parents=True)
    orphan.write_bytes(b"".join(encoded(row) for row in [
        claude_row("claude-orphan-owner", "user", "ou", None, "orphan q", isSidechain=True, agentId="claude-orphan"),
        claude_row("claude-orphan-owner", "assistant", "oa", "ou", "orphan a", isSidechain=True, agentId="claude-orphan")]))
    orphan.with_suffix(".meta.json").write_text(json.dumps({"description": "orphan sidecar", "agentType": "reviewer"}))
    corpus.paths["claude-orphan"] = orphan
    corpus.put("codex-orphan-agent", "codex", [
        stamp(batch35_agent_meta("codex-orphan-agent", "codex-gone", "2026-09-11T09:40:00Z"), "2026-09-11T09:40:00Z"),
        stamp(codex_message("assistant", "orphan agent a"), "2026-09-11T09:40:01Z")], [])
    for sid in ORPHANS:
        corpus.hidden.add(sid)
    # Batch 36: an owner with a running sidecar (open turn, no notice), a
    # stopped one (open turn, a later notice) and a finished one (end_turn); a
    # Codex subagent still in its turn; a continued-in pair and a dangling one.
    sid = "claude-live-owner"
    notice = ("<task-notification>\n<task-id>stopped</task-id>\n<status>completed</status>\n"
              "<summary>Agent finished</summary>\n</task-notification>")
    corpus.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "live owner", cwd=cwd, timestamp="2026-09-11T14:00:00Z"),
        {"type": "attachment", "timestamp": "2026-09-11T14:20:00Z",
         "attachment": {"type": "queued_command", "commandMode": "task-notification",
                        "prompt": notice, "timestamp": "2026-09-11T14:20:00Z"}}], [])
    for agent, rows in (
            ("running", [claude_row(sid, "user", "ru", None, "run", isSidechain=True, agentId="running",
                                    timestamp="2026-09-11T14:05:00Z"),
                         {"type": "assistant", "isSidechain": True, "agentId": "running", "timestamp": "2026-09-11T14:06:00Z",
                          "message": {"role": "assistant", "stop_reason": "tool_use",
                                      "content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {}}]}}]),
            ("stopped", [claude_row(sid, "user", "su", None, "stop", isSidechain=True, agentId="stopped",
                                    timestamp="2026-09-11T14:10:00Z"),
                         {"type": "assistant", "isSidechain": True, "agentId": "stopped", "timestamp": "2026-09-11T14:11:00Z",
                          "message": {"role": "assistant", "stop_reason": None,
                                      "content": [{"type": "text", "text": "done"}]}}]),
            ("finished", [claude_row(sid, "user", "fu", None, "fin", isSidechain=True, agentId="finished",
                                     timestamp="2026-09-11T14:15:00Z"),
                          claude_row(sid, "assistant", "fa", "fu", "fin a", isSidechain=True, agentId="finished",
                                     timestamp="2026-09-11T14:16:00Z")])):
        path = corpus.paths[sid].with_suffix("") / "subagents" / f"agent-{agent}.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"".join(encoded(row) for row in rows))
        path.with_suffix(".meta.json").write_text(json.dumps({"description": agent, "agentType": "worker"}))
        corpus.paths[agent] = path
    corpus.put("codex-live-agent", "codex", [
        stamp(batch35_agent_meta("codex-live-agent", "codex-parent", "2026-09-11T10:30:00Z"), "2026-09-11T10:30:00Z"),
        {"type": "event_msg", "timestamp": "2026-09-11T10:31:00Z", "payload": {"type": "task_started", "turn_id": "t"}},
        stamp(codex_message("assistant", "live agent a"), "2026-09-11T10:32:00Z")], [], parent="codex-parent")
    corpus.put("claude-continued-parent", "claude", [
        claude_row("claude-continued-parent", "user", "u0", None, "before compact", cwd=cwd, timestamp="2026-09-11T15:00:00Z"),
        {"type": "continued-in", "sessionId": "claude-continued-parent", "continuedInSessionId": "claude-continued-child",
         "timestamp": "2026-09-11T15:30:00Z"}], [])
    corpus.put("claude-continued-child", "claude", [
        claude_row("claude-continued-child", "user", "u0", None, "after compact", cwd=cwd, timestamp="2026-09-11T15:30:01Z")], [])
    corpus.put("claude-continued-dangling", "claude", [
        claude_row("claude-continued-dangling", "user", "u0", None, "dangling", cwd=cwd, timestamp="2026-09-11T15:00:00Z"),
        {"type": "continued-in", "sessionId": "claude-continued-dangling", "continuedInSessionId": "claude-gone",
         "timestamp": "2026-09-11T15:30:00Z"}], [])
    return corpus


def run(opener, base, corpus):
    data, raw = fetch(opener, base, "/api/sessions")
    if not isinstance(data.get("sessions"), list) or not isinstance(data.get("sig"), str):
        fail("shape", "need sessions list and nonempty sig", raw)
    if not data["sig"] or not isinstance(data.get("built_at"), (int, float)):
        fail("shape", "sig empty or built_at not a number", raw)
    for row in data["sessions"]:
        excerpt = json.dumps(row, ensure_ascii=False).encode()
        if not isinstance(row, dict) or any(k not in row for k in KEYS):
            fail("shape", "row missing uid/source/sid/title/cwd/created/updated/size", excerpt)
        if any(not isinstance(row[k], str) or not row[k] for k in KEYS if k != "size"):
            fail("shape", "string fields must be nonempty str", excerpt)
        if not isinstance(row["size"], int) or row["size"] < 0:
            fail("shape", "size must be a non-negative int", excerpt)
        for opt in ("model", "branch"):
            if row.get(opt) is not None and not isinstance(row[opt], str):
                fail("shape", f"{opt} present but not str", excerpt)
    passed("list snapshot shape")
    rows = {row["sid"]: row for row in data["sessions"]}
    if set(rows) != LISTED:
        fail("topology", f"listed {sorted(rows)} (0-byte must be omitted)", raw)
    claude, fork, grok = rows["claude-parent"], rows["codex-fork"], rows["grok-summary"]
    if "你好" not in claude["title"] or "中文" not in claude["cwd"]:
        fail("topology", "unicode claude title/cwd", raw)
    if "中文" not in grok["cwd"] or "摘要" not in grok["title"]:
        fail("topology", "unicode grok title/cwd", raw)
    if fork.get("forked_from_id") != "codex-parent" or fork.get("root_sid") != "codex-parent":
        fail("topology", "fork forked_from_id/root_sid", json.dumps(fork).encode())
    items = claude.get("agent_items") or []
    if not any(isinstance(item, dict) and item.get("id") == "claude-agent" for item in items):
        fail("topology", "claude parent missing agent_items", json.dumps(claude).encode())
    body, braw = fetch(opener, base, "/api/messages/" + quote(grok["uid"], safe=":"))
    if body.get("messages") != [] or body.get("end") != 0:
        fail("topology", "summary-only grok empty-body semantics", braw)
    passed("topology fork agents grok-summary unicode empty-zero")
    current = rows["claude-cli-current"]
    if current.get("supported") is not True or "migration_warnings" in current:
        fail("current-cli", "expected supported:true without row warnings (notes live in the detail meta)", json.dumps(current, ensure_ascii=False).encode())
    cursor = current.get("cursor")
    if not isinstance(cursor, dict) or not isinstance(cursor.get("end"), int) or not str(cursor.get("head", "")).startswith("rs-m2-1:") \
            or "anchor" in cursor or current["title"] != "current CLI prompt":
        fail("current-cli", "unopened supported row needs the physical list cursor {end, head} (no anchor) and the user title", json.dumps(current, ensure_ascii=False).encode())
    body, braw = fetch(opener, base, "/api/messages/" + quote(current["uid"], safe=":"))
    texts = [row.get("text") for row in body.get("messages", []) if row.get("role") in ("user", "assistant")]
    if texts != ["current CLI prompt", "current CLI reply"] or body["meta"].get("migration_warnings") != CLI_WARNINGS:
        fail("current-cli", "detail must read through the attachment chain and repeat the warnings", braw)
    if body["meta"]["cursor"]["end"] != cursor["end"] or body["meta"]["cursor"]["head"] != cursor["head"]:
        fail("current-cli", "detail cursor must share the list row's physical part", braw)
    passed("current-cli kinds skipped: supported:true, detail carries migration_warnings")
    for sid, warnings, texts in (("codex-dup-meta", DUP_META_WARNINGS, ["dup meta q", "dup meta a"]),
                                 ("claude-torn-tail", TORN_WARNINGS, ["torn tail q", "torn tail a", "torn tail q2", "torn tail a2"])):
        row = rows[sid]
        if row.get("supported") is not True or "migration_warnings" in row:
            fail("batch-35", f"{sid}: expected supported:true without row warnings", json.dumps(row, ensure_ascii=False).encode())
        if not isinstance(row.get("cursor"), dict) or "anchor" in row["cursor"]:
            fail("batch-35", f"{sid}: unopened supported row needs the physical list cursor", json.dumps(row, ensure_ascii=False).encode())
        detail, draw = fetch(opener, base, "/api/messages/" + quote(row["uid"], safe=":"))
        shown = [r.get("text") for r in detail.get("messages", []) if r.get("role") in ("user", "assistant")]
        if shown != texts or detail["meta"].get("supported") is not True or detail["meta"].get("migration_warnings") != warnings:
            fail("batch-35", f"{sid}: detail must read the file with the same warnings (got {shown})", draw)
        if detail.get("end") != corpus.paths[sid].stat().st_size:
            fail("batch-35", f"{sid}: skipped lines keep their bytes in the cursor", draw)
    dup = rows["codex-dup-meta"]
    if dup.get("sid") != "codex-dup-meta" or dup.get("forked_from_id") != "codex-dup-gone" or dup.get("root_sid") or dup.get("fork_depth"):
        fail("batch-35", "first session_meta is the identity; an absent parent means no root_sid/fork_depth", json.dumps(dup, ensure_ascii=False).encode())
    if dup.get("title") != "dup meta q":
        fail("batch-35", "self-contained legacy fork keeps its own title", json.dumps(dup, ensure_ascii=False).encode())
    for sid in ORPHANS:
        if sid in rows:
            fail("batch-35", f"{sid}: orphan agent must not be a list row", json.dumps(rows[sid], ensure_ascii=False).encode())
        err, eraw = fetch(opener, base, "/api/messages/" + quote(corpus.uid(sid), safe=":"), want=501)
        if not isinstance(err.get("error"), str) or not err["error"] or not isinstance(err.get("code"), str) or not err["code"]:
            fail("batch-35", f"{sid}: orphan agent UID must be a typed 501 JSON error", eraw)
    passed("batch-35: copied session_meta ×2 and torn line ×1 counted (supported:true, detail readable), orphan agents hidden with typed 501")
    for row in data["sessions"]:
        for item in row.get("agent_items") or []:
            if not isinstance(item.get("active"), bool) or not isinstance(item.get("created"), str) \
                    or not isinstance(item.get("updated"), str):
                fail("batch-36", f"{row['sid']}: every agent item needs bool active and created/updated", json.dumps(item, ensure_ascii=False).encode())
        if "continued_in" in row and row["sid"] != "claude-continued-parent":
            fail("batch-36", f"{row['sid']}: continued_in only where the tail names a listed session", json.dumps(row, ensure_ascii=False).encode())
    live = {item["id"]: item for item in rows["claude-live-owner"].get("agent_items") or []}
    if {k: v.get("active") for k, v in live.items()} != {"running": True, "stopped": False, "finished": False}:
        fail("batch-36", "claude sidecars: open turn without a stop notice is active, a later notice or end_turn is not", json.dumps(live, ensure_ascii=False).encode())
    if live["running"]["updated"] != "2026-09-11T14:06:00.000Z" or live["running"]["created"] != "2026-09-11T14:05:00.000Z":
        fail("batch-36", "claude sidecar created/updated are its first/last record times", json.dumps(live["running"], ensure_ascii=False).encode())
    if any(item.get("active") is not False for item in claude.get("agent_items") or []):
        fail("batch-36", "claude-parent's end_turn sidecar is not active", json.dumps(claude, ensure_ascii=False).encode())
    codex_live = {item["id"]: item for item in rows["codex-parent"].get("agent_items") or []}
    if codex_live.get("codex-live-agent", {}).get("active") is not True or codex_live["codex-live-agent"]["updated"] != "2026-09-11T10:32:00.000Z":
        fail("batch-36", "codex subagent after task_started is active with the last record time", json.dumps(codex_live, ensure_ascii=False).encode())
    detail, draw = fetch(opener, base, "/api/messages/" + quote(rows["claude-live-owner"]["uid"], safe=":") + "?agent=running")
    view_items = {item["id"]: item for item in detail["meta"].get("agent_items") or []}
    if detail["meta"].get("agent_id") != "running" or view_items.get("running", {}).get("active") is not True:
        fail("batch-36", "the agent view meta keeps the owner's agent_items with active (Python session_view)", draw)
    if rows["claude-continued-parent"].get("continued_in") != rows["claude-continued-child"]["uid"]:
        fail("batch-36", "continued-in tail record resolves to the listed continuation's uid", json.dumps(rows["claude-continued-parent"], ensure_ascii=False).encode())
    for sid in ("claude-continued-child", "claude-continued-dangling"):
        if "continued_in" in rows[sid]:
            fail("batch-36", f"{sid}: no continued_in without a listed target", json.dumps(rows[sid], ensure_ascii=False).encode())
    passed("batch-36: agent_items active (claude stop notices, codex turn events), agent view meta, continued_in resolved/absent")
    opened, oraw = fetch(opener, base, "/api/sessions?force=1")
    if opened.get("sig") != data["sig"]:
        fail("cursor", "opening a session must not change the list signature", oraw)
    now = {row["sid"]: row for row in opened.get("sessions") or []}["claude-cli-current"]
    if now.get("cursor") != body["meta"]["cursor"]:
        fail("cursor", f"opened row must carry the view anchor: row={now.get('cursor')} detail={body['meta']['cursor']}", oraw)
    passed("list cursor: physical before open, anchor after open, sig unchanged")
    same, sraw = fetch(opener, base, "/api/sessions?sig=" + quote(data["sig"]))
    if same != {"unchanged": True, "sig": data["sig"]}:
        fail("sig", "expected {unchanged:true,sig}", sraw)
    rebuilt, rraw = fetch(opener, base, "/api/sessions?force=1")
    if rebuilt.get("sig") != data["sig"] or "sessions" not in rebuilt:
        fail("sig", "force=1 must rebuild the full snapshot", rraw)
    passed("sig unchanged and force rebuild")
    # Batch 36: a stop notice appended to the owner flips its running sidecar
    # (incremental scan of the owner file), and a later sidecar record flips
    # it back (woken by SendMessage); both change the signature.
    owner = corpus.paths["claude-live-owner"]
    wake = ("<task-notification>\n<task-id>running</task-id>\n<status>killed</status>\n"
            "<summary>Agent finished</summary>\n</task-notification>")
    with owner.open("ab") as fh:
        fh.write(encoded({"type": "attachment", "timestamp": "2026-09-11T14:30:00Z",
                          "attachment": {"type": "queued_command", "commandMode": "task-notification",
                                         "prompt": wake, "timestamp": "2026-09-11T14:30:00Z"}}))
    stopped, sraw = fetch(opener, base, "/api/sessions?force=1")
    live = {item["id"]: item for item in {row["sid"]: row for row in stopped["sessions"]}["claude-live-owner"]["agent_items"]}
    if live["running"]["active"] is not False or stopped["sig"] == rebuilt["sig"]:
        fail("batch-36", "a stop notice appended to the owner must flip active and the sig", sraw)
    with corpus.paths["running"].open("ab") as fh:
        fh.write(encoded(claude_row("claude-live-owner", "user", "ru2", None, "again", isSidechain=True,
                                    agentId="running", timestamp="2026-09-11T14:40:00Z")))
    woken, wraw = fetch(opener, base, "/api/sessions?force=1")
    live = {item["id"]: item for item in {row["sid"]: row for row in woken["sessions"]}["claude-live-owner"]["agent_items"]}
    if live["running"]["active"] is not True or woken["sig"] == stopped["sig"]:
        fail("batch-36", "a sidecar record later than the notice must flip active back", wraw)
    passed("batch-36: appended stop notice and later wake-up flip active with the sig")
    with corpus.paths["claude-parent"].open("ab") as fh:
        fh.write(encoded(claude_row(
            "claude-parent", "user", "u1", "a0", "append", cwd="/synthetic/中文项目",
            timestamp="2026-09-11T13:00:00Z")))
    after, araw = fetch(opener, base, "/api/sessions?force=1")
    now = {row["sid"]: row for row in after.get("sessions") or []}.get("claude-parent")
    if after.get("sig") == woken["sig"] or not now:
        fail("append", "sig must change after append", araw)
    if now["size"] <= claude["size"] or now["updated"] <= claude["updated"]:
        fail("append", "row updated/size must grow", araw)
    passed("append changes sig updated size")
    # No registry in this state directory: every debug view is empty and
    # the ordinary view hides nothing (docs/read-model.md "debug_run").
    view, vraw = fetch(opener, base, "/api/sessions?debug_run=x")
    if view.get("sessions") != [] or not view.get("sig"):
        fail("debug_run", "expected an empty signed view for an unknown run", vraw)
    passed("debug_run unknown run → empty view")
    seq = after["sessions"]
    for left, right in zip(seq, seq[1:]):
        if left["updated"] < right["updated"] or (
                left["updated"] == right["updated"] and left["uid"] > right["uid"]):
            fail("order", "expected updated desc, uid asc", araw)
    passed("order updated descending")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-sessions-list-") as tmp:
        root = Path(tmp)
        corpus = build(root / "main")
        with isolated_server(corpus, args.binary) as (base, opener):
            run(opener, base, corpus)
        over = Corpus(root / "over")
        for source in ("claude", "codex", "grok"):
            (over.root / source).mkdir(parents=True)
        budget = over.root / "claude" / "budget"
        budget.mkdir()
        for i in range(1001):
            (budget / f"{i:04d}.jsonl").write_bytes(b"{}\n")
        with isolated_server(over, args.binary) as (base, opener):
            payload, raw = fetch(opener, base, "/api/sessions?force=1")
            if len(payload.get("sessions") or []) != 1001:
                fail("scale", f"want 1001 rows (no session cap), got {len(payload.get('sessions') or [])}", raw)
        passed("1001 sessions listed (no session cap)")


if __name__ == "__main__":
    main()
