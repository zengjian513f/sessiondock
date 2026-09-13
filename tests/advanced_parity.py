#!/usr/bin/env python3
"""Advanced synthetic Rust-versus-Python read-model differential.

Broadens the M1 differential with deeper synthetic scenarios: three-level Claude
compaction chains with rewinds, abandoned branches and nested sidechain tools;
Codex forks of forks with rollbacks, duplicate telemetry and a mid-chain
subagent; rich tool rendering (questions, errors, unicode file changes, a
1.5 MiB output and oversized arguments); extra Codex native tool shapes
(quoted local_shell, three-file apply_patch, nested exec_command, web_search
arguments, two-part reasoning); Grok chat/tool rows and summary-only sessions;
the batch-35 real-root shapes (legacy self-contained Codex forks with copied
session_meta records, rewind past the parent's fork point, orphan agents, torn
Claude lines, missing leaf, parent cycle, duplicate-key line); the batch-36
Esc-interrupted Claude turns (tools/thinking under the cut-short input, a
compact boundary above it, the interrupt as the unanswered sibling); the batch-36
Codex multi-chunk tool-output envelopes (every chunk shown, documented DELTA against
the reference's single chunk). Every fixture is generated in one temporary directory; only an
isolated loopback Rust server is launched and only the explicitly selected
Python adapters are imported. No CLI homes, model calls or network access.

Exact equality is asserted where the migration plan claims parity. Documented
intentional differences are asserted and printed as ``DELTA``. Differences
that are not clearly Rust bugs are printed as ``UNVERIFIED`` without failing.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
from urllib.error import HTTPError
from urllib.parse import quote, urlencode

sys.dont_write_bytecode = True

from history_parity import (BINARY as DEBUG_BINARY, REPO, BATCH35_AGENTS, BATCH35_CLAUDE_ROWS, BATCH35_CODEX_ROWS, BATCH35_HIDDEN,
                            CLAUDE_ATTACHMENT_KINDS, Corpus, claude_attachment_chain, claude_control_rows, claude_row,
                            claude_turn_tail_rows, codex_row, codex_telemetry_rows, encoded, get_json, isolated_server, write_batch35_shapes)
from provider_parity import load_adapters, normalized

RELEASE_BINARY = REPO / "target/release" / ("sessiondock.exe" if os.name == "nt" else "sessiondock")
BINARY = RELEASE_BINARY if RELEASE_BINARY.is_file() else DEBUG_BINARY
LARGE_RESPONSE_BYTES = 12 * 1024 * 1024
RICH_FIELDS = ("summary", "changes", "changes_unavailable_reason", "has_stderr", "operation", "event_id", "inferred", "local_shell")


class Report:
    """Collect PASS/DELTA/UNVERIFIED/FAIL lines; only FAIL makes the run non-zero."""

    def __init__(self):
        self.failures = []
        self.deltas = []
        self.unverified_lines = []
        self.passes = []

    def ok(self, line):
        self.passes.append(line)
        print("PASS", line)

    def delta(self, line):
        self.deltas.append(line)
        print("DELTA", line)

    def unverified(self, line):
        self.unverified_lines.append(line)
        print("UNVERIFIED", line)

    def fail(self, line):
        self.failures.append(line)
        print("FAIL", line)


def rich(message):
    result = normalized(message)
    for field in RICH_FIELDS:
        if message.get(field) is not None:
            result[field] = message[field]
    return result


def clipped(value, limit=160):
    text = json.dumps(value, ensure_ascii=False, default=str)
    return text if len(text) <= limit else text[:limit] + f"…(+{len(text) - limit})"


def fetch(opener, base, uid, query="", limit=LARGE_RESPONSE_BYTES):
    url = base + "/api/messages/" + quote(uid, safe=":") + ("?" + query if query else "")
    with opener.open(url, timeout=30) as response:
        raw = response.read(limit + 1)
        assert len(raw) <= limit, "synthetic response exceeds the harness limit"
        value = json.loads(raw)
    assert value.get("meta", {}).get("uid") == uid, "server returned a different synthetic view"
    return value


def fetch_supported(opener, base, sid, uid, query=""):
    """fetch() for a session that must be readable; an HTTP error becomes an
    assertion naming the session and the typed body."""
    try:
        return fetch(opener, base, uid, query)
    except HTTPError as error:
        raise AssertionError(f"{sid}: HTTP {error.code} {error.read(4096).decode('utf-8', 'replace')[:300]}") from None


def expect_status(opener, base, uid, query, status):
    try:
        fetch(opener, base, uid, query)
    except HTTPError as error:
        body = json.loads(error.read(65536))
        assert error.code == status, (uid, query, error.code, body)
        return body.get("error", "")
    raise AssertionError(f"{uid}?{query} returned success instead of HTTP {status}")


def python_read(adapters, source, path, **kwargs):
    native, end = adapters[source].read(str(path), **kwargs)
    messages = [row for row in native if row.get("role") != "status"]
    statuses = [row for row in native if row.get("role") == "status"]
    return messages, statuses, end


def compare_view(report, label, python, rust, *, classify=None, strip_ts=False):
    """Compare a Python adapter read with one Rust HTTP batch.

    ``classify(index, key, left, right)`` may return "unverified", "delta" or
    "known" (a delta already summarized elsewhere) for a differing field to stop
    it from failing the run; anything else fails.
    """
    messages, statuses, end = python
    expected = [rich(row) for row in messages]
    actual = [rich(row) for row in rust["messages"]]
    assert rust.get("partial") in (None, {}), (label, "window must not page this small view", rust.get("partial"))
    issues = []
    noted = []
    if strip_ts:
        # Existing documented parser difference (see grok_parity.py): Python's
        # Grok adapter never emits record timestamps, Rust preserves them.
        python_stamps = sum(row.get("ts") is not None for row in expected)
        rust_stamps = sum(row.get("ts") is not None for row in actual)
        for row in expected + actual:
            row.pop("ts", None)
        if python_stamps != rust_stamps:
            report.delta(f"{label}: record ts Python={python_stamps}/{len(expected)} Rust={rust_stamps}/{len(actual)} messages (Grok parser difference, compared without ts)")
    if len(expected) != len(actual):
        report.fail(f"{label}: message length Python={len(expected)} Rust={len(actual)}")
        for index in range(min(len(expected), len(actual)), max(len(expected), len(actual))):
            side = expected[index:index + 1] or actual[index:index + 1]
            print(f"    extra[{index}] on {'Python' if expected[index:index + 1] else 'Rust'}: {clipped(side[0])}")
        issues.append("length")
    for index, (left, right) in enumerate(zip(expected, actual)):
        if left == right:
            continue
        keys = sorted(key for key in left.keys() | right.keys() if left.get(key) != right.get(key))
        for key in keys:
            verdict = classify(index, key, left, right) if classify else None
            line = (f"{label}: message[{index}] role={left.get('role')}/{right.get('role')} field {key!r}: "
                    f"Python={clipped(left.get(key))} Rust={clipped(right.get(key))}")
            if verdict == "unverified":
                report.unverified(line)
                noted.append(key)
            elif verdict == "delta":
                report.delta(line)
                noted.append(key)
            elif verdict == "known":
                noted.append(key)
            else:
                report.fail(line)
                issues.append(key)
    expected_activity = normalized(statuses[-1]) if statuses else None
    actual_activity = normalized(rust["activity"]) if rust.get("activity") else None
    if strip_ts:
        for row in (expected_activity, actual_activity):
            if row:
                row.pop("ts", None)
    if expected_activity != actual_activity:
        report.fail(f"{label}: latest activity Python={clipped(expected_activity)} Rust={clipped(actual_activity)}")
        issues.append("activity")
    if end != rust.get("end"):
        report.fail(f"{label}: EOF cursor Python={end} Rust={rust.get('end')}")
        issues.append("end")
    counted = sum(row.get("counted") is not False for row in messages)
    if counted != rust.get("message_total"):
        report.fail(f"{label}: counted total Python={counted} Rust={rust.get('message_total')}")
        issues.append("message_total")
    if not issues:
        report.ok(f"{label}: {len(actual)} messages, {counted} counted, activity/cursor/total identical"
                  + (f" apart from {len(noted)} classified field(s)" if noted else ""))
    return not issues


# --------------------------------------------------------------------------
# Scenario 1: Claude compaction chain, rewind, abandoned input, sidechain agent
# --------------------------------------------------------------------------

CLAUDE_AGENT = "claude-chain-agent"


def claude_chain_rows(sid, *, esc_branch):
    def row(kind, uid, parent, text="", **extra):
        return claude_row(sid, kind, uid, parent, text, **extra)

    rows = [
        row("user", "u0", None, "Chain question one"),
        row("assistant", "a0", "u0", [{"type": "text", "text": "Chain answer one"},
                                        {"type": "tool_use", "id": "call-0", "name": "Bash", "input": {"command": "printf chain"}}]),
        row("user", "r0", "a0", [{"type": "tool_result", "tool_use_id": "call-0", "content": "chain"}]),
        row("assistant", "a0b", "r0", "Chain answer one done"),
        {"type": "system", "subtype": "turn_duration", "uuid": "d0", "parentUuid": "a0b", "sessionId": sid,
         "durationMs": 1500, "timestamp": "2026-09-11T10:00:01Z", "isSidechain": False},
        # First compaction: legacy boundary reconnects to the previous tip.
        row("system", "compact1", None, subtype="compact_boundary"),
        row("user", "summary1", "compact1", "INTERNAL COMPACT SUMMARY ONE", isCompactSummary=True),
        row("user", "u1", "summary1", "Chain question two"),
        row("assistant", "a1", "u1", "Chain answer two"),
        # Second compaction: current metadata shape.
        row("system", "compact2", None, compactMetadata={"trigger": "auto", "durationMs": 900}),
        row("user", "summary2", "compact2", "INTERNAL COMPACT SUMMARY TWO", isCompactSummary=True),
        row("user", "u2", "summary2", "Chain question three"),
        row("assistant", "a2", "u2", "Chain answer three"),
        # Completed branch later rewound away by last-prompt.
        row("user", "u3", "a2", "Chain discarded question four"),
        row("assistant", "a3", "u3", "Chain discarded answer four"),
        {"type": "last-prompt", "leafUuid": "a2"},
    ]
    if esc_branch:
        rows += [
            row("user", "u4", "a2", "Chain fast Esc input"),
            row("user", "u5", "a2", "Chain replacement question"),
            row("assistant", "a5", "u5", "Chain replacement answer"),
            # Old-style inline sidechain rows inside the main transcript never
            # decide the main leaf but stay visible as subagent bubbles.
            row("user", "sc-u", None, "Inline sidechain question", isSidechain=True, agentId="inline-agent-1"),
            row("assistant", "sc-a", "sc-u", "Inline sidechain answer", isSidechain=True, agentId="inline-agent-1"),
        ]
    return rows


def build_claude(corpus):
    chain = ["Chain question one", "Chain answer one", {"command": "printf chain"}, "chain", "Chain answer one done", "",
             "已压缩", "Chain question two", "Chain answer two", "已压缩", "Chain question three", "Chain answer three"]
    corpus.put("claude-chain", "claude", claude_chain_rows("claude-chain", esc_branch=False), chain)
    corpus.put("claude-chain-esc", "claude", claude_chain_rows("claude-chain-esc", esc_branch=True),
               chain + ["Chain fast Esc input", "Chain replacement question", "Chain replacement answer",
                        "Inline sidechain question", "Inline sidechain answer"])
    agent_path = corpus.paths["claude-chain"].with_suffix("") / "subagents" / f"agent-{CLAUDE_AGENT}.jsonl"
    agent_path.parent.mkdir(parents=True)

    def agent_row(kind, uid, parent, content):
        return claude_row("claude-chain", kind, uid, parent, content, isSidechain=True, agentId=CLAUDE_AGENT)

    agent_rows = [
        agent_row("user", "ag-u", None, "Agent task: inspect fixture"),
        agent_row("assistant", "ag-a1", "ag-u", [{"type": "text", "text": "Agent reading"},
                                                 {"type": "tool_use", "id": "ag-call-1", "name": "Read", "input": {"file_path": "src/读取.rs", "offset": 3, "limit": 9}}]),
        agent_row("user", "ag-r1", "ag-a1", [{"type": "tool_result", "tool_use_id": "ag-call-1", "content": [{"type": "text", "text": "fn read() {}"}]}]),
        agent_row("assistant", "ag-a2", "ag-r1", [{"type": "thinking", "thinking": "Agent private reasoning"},
                                                  {"type": "tool_use", "id": "ag-call-2", "name": "Bash", "input": {"command": "false"}}]),
        agent_row("user", "ag-r2", "ag-a2", [{"type": "tool_result", "tool_use_id": "ag-call-2", "is_error": True, "content": "Exit code 1\ncommand failed"}]),
        agent_row("assistant", "ag-a3", "ag-r2", [{"type": "tool_use", "id": "ag-call-3", "name": "AskUserQuestion",
                                                   "input": {"questions": [{"header": "Agent", "question": "Agent may continue?", "options": ["yes", "no"]}]}}]),
        agent_row("user", "ag-r3", "ag-a3", [{"type": "tool_result", "tool_use_id": "ag-call-3", "content": "yes"}]),
        agent_row("assistant", "ag-a4", "ag-r3", "Agent finished"),
    ]
    agent_path.write_bytes(b"".join(encoded(row) for row in agent_rows))
    agent_path.with_suffix(".meta.json").write_text(json.dumps({"description": "Synthetic chain worker", "agentType": "worker"}))
    corpus.paths[CLAUDE_AGENT] = agent_path
    corpus.owners[CLAUDE_AGENT] = "claude-chain"
    corpus.expected[CLAUDE_AGENT] = []

    sid = "claude-events"
    # Batch 33: current Claude Code kinds in their observed positions —
    # session-level control records first, an attachment chain between the
    # user record and its reply, atis-latch/file-history-delta/cost-state after.
    env_chain, env_tip = claude_attachment_chain(sid, "e-u0", "e-env", kinds=CLAUDE_ATTACHMENT_KINDS)
    rows = [
        *claude_control_rows(sid),
        claude_row(sid, "user", "e-u0", None, "Events question"),
        *env_chain,
        {"type": "atis-latch", "atis": {"latched": True}, "sessionId": sid},
        claude_row(sid, "assistant", "e-a0", env_tip, "Events answer"),
        *claude_turn_tail_rows(sid, "e-a0", "e-u0"),
        {"type": "system", "subtype": "local_command", "uuid": "e-s1", "parentUuid": "e-a0", "sessionId": sid, "isSidechain": False,
         "timestamp": "2026-09-11T10:00:00Z", "content": "<command-name>/status</command-name>\n<command-args>--verbose</command-args>"},
        {"type": "system", "subtype": "local_command", "uuid": "e-s2", "parentUuid": "e-s1", "sessionId": sid, "isSidechain": False,
         "timestamp": "2026-09-11T10:00:00Z", "content": "<command-name>/compact</command-name>"},
        claude_row(sid, "user", "e-u1", "e-s2", "<bash-input>ls -la</bash-input>"),
        claude_row(sid, "user", "e-u2", "e-u1", "<bash-stdout>total 0\n</bash-stdout><bash-stderr>warn &amp; more</bash-stderr>"),
        claude_row(sid, "user", "e-u3", "e-u2", "<task-notification>\n<status>completed</status>\n<summary>Agent \"worker\" finished</summary>\n<result>done &lt;ok&gt;</result>\n</task-notification>"),
        # Two more unknown attachments before the queued human prompt: the
        # reply's ancestry runs through all of them.
        {"type": "attachment", "uuid": "e-hook", "parentUuid": "e-u3", "sessionId": sid, "isSidechain": False, "timestamp": "2026-09-11T10:00:00Z",
         "attachment": {"type": "hook_success", "hookName": "synthetic"}},
        {"type": "attachment", "uuid": "e-diag", "parentUuid": "e-hook", "sessionId": sid, "isSidechain": False, "timestamp": "2026-09-11T10:00:00Z",
         "attachment": {"type": "diagnostics", "files": []}},
        {"type": "attachment", "uuid": "e-at", "parentUuid": "e-diag", "sessionId": sid, "isSidechain": False, "timestamp": "2026-09-11T10:00:00Z",
         "attachment": {"type": "queued_command", "commandMode": "prompt", "origin": {"kind": "human"}, "prompt": "Queued human prompt"}},
        {"type": "queue-operation", "operation": "enqueue", "content": "Queued text", "timestamp": "2026-09-11T10:00:00Z", "sessionId": sid},
        {"type": "queue-operation", "operation": "remove", "content": "", "timestamp": "2026-09-11T10:00:00Z", "sessionId": sid},
        {"type": "custom-title", "customTitle": "Events renamed", "sessionId": sid},
        {"type": "custom-title", "customTitle": "Events renamed", "sessionId": sid},
        claude_row(sid, "assistant", "e-a1", "e-at", "Events queued answer"),
        claude_row(sid, "user", "e-u4", "e-a1", "[Request interrupted by user]"),
        claude_row(sid, "assistant", "e-a2", "e-u4", "Events final answer"),
        {"type": "atis-latch", "atis": {"latched": False}, "sessionId": sid},
        {"type": "atis-latch", "atis": {"latched": False}, "sessionId": sid},
    ]
    corpus.put(sid, "claude", rows, ["Events question", "Events answer", "/status --verbose", "! ls -la", "total 0\n\nstderr:\nwarn & more",
                                     "子代理完成 · worker", "Queued human prompt", "Queued text", "/rename Events renamed",
                                     "Events queued answer", "Events final answer"])

    sid = "claude-missing-parent"
    corpus.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "Missing parent root question"),
        claude_row(sid, "assistant", "a0", "u0", "Missing parent root answer"),
        claude_row(sid, "user", "u1", "ghost-parent", "Question under a missing ancestor"),
        claude_row(sid, "assistant", "a1", "u1", "Answer under a missing ancestor"),
    ], ["Question under a missing ancestor", "Answer under a missing ancestor"])

    # Batch 35 declared DELTA: a line with a duplicated key. json.loads keeps
    # the last value; Rust skips the whole line as an invalid JSONL record.
    # The record is a leaf nothing references, so only that reply differs.
    sid = "claude-dupkey"
    duplicate = ('{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"%s","cwd":"/synthetic/history",'
                 '"timestamp":"2026-09-11T10:00:00Z","isSidechain":false,'
                 '"message":{"role":"assistant","content":"Duplicate key first"},'
                 '"message":{"role":"assistant","content":"Duplicate key last wins","stop_reason":"end_turn"}}\n' % sid).encode()
    assert duplicate.count(b'"message":') == 2 and json.loads(duplicate)["message"]["content"] == "Duplicate key last wins"
    corpus.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "Dup key root question"),
        claude_row(sid, "assistant", "a0", "u0", "Dup key root answer"),
        claude_row(sid, "user", "u1", "a0", "Dup key question"),
        duplicate], ["Dup key root question", "Dup key root answer", "Dup key question"])


def verify_claude(report, corpus, base, opener, adapters, rows):
    for sid in ("claude-chain", "claude-chain-esc"):
        response = fetch(opener, base, corpus.uid(sid), "window=1")
        assert response["meta"]["sid"] == sid
        shown = [json.loads(row["text"]) if row["role"] == "tool" else row["text"] for row in response["messages"]]
        assert shown == corpus.expected[sid], (sid, shown)
        compacts = [row for row in response["messages"] if row.get("event_kind") == "compact"]
        assert len(compacts) == 2 and all(row.get("counted") is False for row in compacts)
        assert {row["event_id"] for row in compacts} == {"compact:compact1", "compact:compact2"}
        assert not any("discarded" in row["text"] for row in response["messages"]), "rewound branch leaked"
        if sid == "claude-chain-esc":
            escaped = next(row for row in response["messages"] if row["text"] == "Chain fast Esc input")
            assert escaped.get("interrupted") is True and escaped.get("interrupt_reason")
            inline = [row for row in response["messages"] if row["role"].endswith("·subagent")]
            assert [row["name"] for row in inline] == ["inline-a", "inline-a"], "sidechain agent tag is the first eight characters"
        if adapters:
            compare_view(report, f"claude {sid} main view", python_read(adapters, "claude", corpus.paths[sid]), response)
    sid = "claude-events"
    response = fetch(opener, base, corpus.uid(sid), "window=1")
    assert [row["text"] for row in response["messages"]] == corpus.expected[sid], [row["text"] for row in response["messages"]]
    kinds = [(row["role"], row.get("counted", True)) for row in response["messages"]]
    assert kinds == [("user", True), ("assistant", True), ("command", False), ("command", True), ("tool_result", False), ("event", False),
                     ("user", True), ("queue_operation", False), ("command", False), ("assistant", True), ("assistant", True)], kinds
    assert response["activity"]["state"] == "aborted" and response["message_total"] == 6
    assert response["meta"]["title"] == "Events renamed"
    # Batch 33: the current-CLI kinds were skipped like Python, counted per
    # kind on the row, and the reply's lineage resolved through the attachments.
    assert rows[sid].get("supported") is True and response["meta"].get("supported") is True, rows[sid].get("migration_warnings")
    expected_warnings = ["跳过未知的Claude 记录类型：mode ×1", "跳过未知的Claude 记录类型：permission-mode ×1",
                         "跳过未知的Claude 记录类型：bridge-session ×1", "跳过未知的Claude 记录类型：agent-name ×1",
                         *(f"跳过未知的Claude attachment 类型：{kind} ×{2 if kind in ('hook_success', 'diagnostics') else 1}" for kind in CLAUDE_ATTACHMENT_KINDS),
                         "跳过未知的Claude 记录类型：atis-latch ×4", "跳过未知的Claude 记录类型：file-history-delta ×1",
                         "跳过未知的Claude 记录类型：cost-state ×1"]
    # Batch 44 WP-C: the notes live in the detail meta only; a supported list
    # row carries no migration_warnings (Python rows have none).
    assert "migration_warnings" not in rows[sid], rows[sid].get("migration_warnings")
    assert response["meta"].get("migration_warnings") == expected_warnings
    assert len(expected_warnings) <= 32
    answered = next(row for row in response["messages"] if row["text"] == "Events answer")
    assert answered.get("turn_id") == "e-u0", answered
    shell = next(row for row in response["messages"] if row["role"] == "tool_result")
    assert shell["has_stderr"] is True and shell["call_id"] == "local-shell:e-u1" and shell["name"] == "Shell"
    notice = next(row for row in response["messages"] if row["role"] == "event")
    assert notice["event_kind"] == "task" and notice["event_status"] == "completed" and notice["details"] == "done <ok>"
    if adapters:
        compare_view(report, "claude claude-events local commands/shell/notification/queue/rename/interrupt + current-CLI kinds skipped", python_read(adapters, "claude", corpus.paths[sid]), response)

    assert {item["id"] for item in rows["claude-chain"]["agent_items"]} == {CLAUDE_AGENT}
    assert rows["claude-chain"]["agent_items"][0]["title"] == "Synthetic chain worker"
    assert rows["claude-chain"]["agent_items"][0]["type"] == "worker"
    assert not rows["claude-chain-esc"].get("agent_items")
    agent = fetch(opener, base, corpus.uid("claude-chain"), urlencode({"agent": CLAUDE_AGENT, "window": 1}))
    assert agent["meta"]["sid"] == CLAUDE_AGENT and agent["meta"]["parent_title"] == rows["claude-chain"]["title"]
    roles = [row["role"] for row in agent["messages"]]
    assert roles == ["user·subagent", "assistant·subagent", "tool", "tool_result", "thinking", "tool", "tool_result",
                     "question", "answer", "assistant·subagent"], roles
    tools = {(row["role"], row["call_id"]): row for row in agent["messages"] if row.get("call_id")}
    assert tools[("tool", "ag-call-1")]["summary"] == "读 src/读取.rs ⌖3+9"
    assert tools[("tool_result", "ag-call-2")]["error"] is True and tools[("tool_result", "ag-call-2")]["exit_code"] == 1
    assert tools[("question", "ag-call-3")]["questions"][0]["options"][1]["label"] == "no"
    assert tools[("answer", "ag-call-3")]["text"] == "yes"
    assert all(row["name"] == "claude-c" for row in agent["messages"] if row["role"].endswith("·subagent") or row["role"] == "thinking")
    assert not any(row["role"] == "status" for row in agent["messages"])
    if adapters:
        compare_view(report, "claude claude-chain ?agent= nested tools", python_read(adapters, "claude", corpus.paths[CLAUDE_AGENT], agent=CLAUDE_AGENT), agent)
    expect_status(opener, base, corpus.uid(CLAUDE_AGENT), "", 404)
    expect_status(opener, base, corpus.uid("claude-chain-esc"), urlencode({"agent": CLAUDE_AGENT}), 404)

    if adapters:
        mismatches = []
        for sid in ("claude-chain", "claude-chain-esc", "claude-events", "claude-tools"):
            expected = adapters["claude"].session_meta(corpus.paths[sid])
            for key in ("sid", "title", "cwd", "branch", "created", "updated"):
                left, right = expected.get(key), rows[sid].get(key)
                if key in ("created", "updated"):
                    left, right = normalized({"ts": left}).get("ts"), normalized({"ts": right}).get("ts")
                if left != right:
                    mismatches.append(f"{sid}.{key}: Python={clipped(left)} Rust={clipped(right)}")
            left = [(item["id"], item["title"], item["type"]) for item in expected.get("agent_items", [])]
            right = [(item["id"], item["title"], item["type"]) for item in rows[sid].get("agent_items", [])]
            if left != right:
                mismatches.append(f"{sid}.agent_items: Python={left} Rust={right}")
        for line in mismatches:
            report.fail("claude list metadata " + line)
        if not mismatches:
            report.ok("claude list metadata: sid/title(custom-title)/cwd/branch/created/updated/agent_items identical for four sessions")

    sid = "claude-missing-parent"
    # Batch 35 R5: the walk from the tip stops at the unknown parent exactly
    # like Python _active_lineage; the reachable part is the timeline, the
    # session stays supported and the break is a warning (row: the summary
    # sees no lineage; detail: the exact text).
    assert rows[sid].get("supported") is True and "migration_warnings" not in rows[sid], rows[sid]
    response = fetch_supported(opener, base, sid, corpus.uid(sid), "window=1")
    assert [row["text"] for row in response["messages"]] == corpus.expected[sid], [row["text"] for row in response["messages"]]
    assert response["meta"].get("supported") is True
    assert response["meta"].get("migration_warnings") == ["Claude 祖先链在 ghost-parent 处中断，之前的记录不在当前时间线"], response["meta"].get("migration_warnings")
    if adapters:
        compare_view(report, f"claude {sid} reachable timeline above a missing ancestor (batch 35, was DELTA 501)",
                     python_read(adapters, "claude", corpus.paths[sid]), response)
    else:
        report.ok(f"claude {sid}: reachable timeline above a missing ancestor with a warning (Rust-only run)")

    sid = "claude-dupkey"
    response = fetch_supported(opener, base, sid, corpus.uid(sid), "window=1")
    shown = [(row["role"], row["text"]) for row in response["messages"]]
    assert rows[sid].get("supported") is True and "migration_warnings" not in rows[sid], rows[sid]
    assert response["meta"].get("supported") is True and response["meta"].get("migration_warnings") == ["跳过无效的JSONL 记录 ×1"], response["meta"]
    assert [text for _, text in shown] == corpus.expected[sid], shown
    assert response["end"] == corpus.paths[sid].stat().st_size, "the skipped line keeps its bytes in the physical cursor"
    if adapters:
        messages, _, end = python_read(adapters, "claude", corpus.paths[sid])
        python_shown = [(row["role"], row["text"]) for row in messages]
        assert python_shown == shown + [("assistant", "Duplicate key last wins")] and end == response["end"], python_shown
        report.delta(f"claude {sid}: duplicate-key line — Python json.loads is last-key-wins and shows {len(python_shown)} messages "
                     f"(last {python_shown[-1][1]!r}); Rust skips the line as an invalid JSONL record, shows {len(shown)} messages with "
                     f"{response['meta']['migration_warnings']} and the same EOF cursor")
    else:
        report.delta(f"claude {sid}: duplicate-key line skipped by Rust ({response['meta']['migration_warnings']}); Python would keep the last value")


# --------------------------------------------------------------------------
# Scenario 2: Codex fork of a fork, rollback, telemetry, mid-chain subagent
# --------------------------------------------------------------------------

def codex_meta(sid, **extra):
    return codex_row("session_meta", {"id": sid, "session_id": sid, "timestamp": "2026-09-11T09:00:00Z",
                                      "cwd": "/synthetic/advanced", "thread_source": "user", **extra})


def codex_event(kind, **payload):
    return codex_row("event_msg", {"type": kind, **payload})


def build_codex(corpus):
    root_rows = [codex_meta("codex-root"),
                 codex_row("turn_context", {"model": "synthetic-root-model"}),
                 codex_event("task_started", turn_id="root-1"),
                 codex_row("response_item", {"type": "message", "role": "user", "turn_id": "root-1",
                                             "content": [{"type": "input_text", "text": "Root question"}]}),
                 codex_row("response_item", {"type": "reasoning", "turn_id": "root-1",
                                             "summary": [{"type": "summary_text", "text": "Root reasoning"}]}),
                 codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "root-1",
                                             "content": [{"type": "output_text", "text": "Root answer"}]}),
                 codex_event("task_complete", turn_id="root-1", duration_ms=800)]
    cut1 = sum(len(encoded(row)) for row in root_rows)
    corpus.put("codex-root", "codex", root_rows + [
        codex_row("response_item", {"type": "message", "role": "user", "turn_id": "root-2", "content": "Root tail after cut"})],
        ["Root question", "Root reasoning", "Root answer", "Root tail after cut"])

    abort = "<turn_aborted>\nTurn interrupted by user\n</turn_aborted>"
    l1_rows = [codex_meta("codex-l1", forked_from_id="codex-root", history_mode="paginated",
                          history_base={"thread_id": "codex-root", "end_byte_offset": cut1}),
               codex_event("task_started", turn_id="l1-1"),
               codex_row("response_item", {"type": "message", "role": "user", "turn_id": "l1-1", "content": "L1 question"}),
               codex_row("response_item", {"type": "message", "role": "assistant", "phase": "commentary", "turn_id": "l1-1",
                                           "content": "L1 progress before interruption"}),
               codex_row("response_item", {"type": "message", "role": "developer", "turn_id": "l1-1", "content": abort}),
               codex_event("turn_aborted", turn_id="l1-1", reason="user interrupted"),
               codex_event("task_started", turn_id="l1-2"),
               codex_row("response_item", {"type": "message", "role": "user", "turn_id": "l1-2", "content": abort + "\n" + abort + "\nL1 second question"}),
               codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "l1-2",
                                           "content": "L1 second answer"}),
               codex_event("task_complete", turn_id="l1-2", duration_ms=1200)]
    cut2 = sum(len(encoded(row)) for row in l1_rows)
    corpus.put("codex-l1", "codex", l1_rows + [
        codex_row("response_item", {"type": "message", "role": "user", "turn_id": "l1-3", "content": "L1 tail after cut"})],
        ["Root question", "Root reasoning", "Root answer", "L1 question", "L1 progress before interruption",
         "L1 second question", "L1 second answer", "L1 tail after cut"])

    l2_rows = [codex_meta("codex-l2", forked_from_id="codex-l1", history_mode="paginated",
                          history_base={"thread_id": "codex-l1", "end_byte_offset": cut2}),
               # Duplicate telemetry notifications mirror response_items and must stay silent.
               codex_event("user_message", message="L2 question"),
               codex_event("agent_message", message="L2 answer"),
               codex_event("agent_reasoning", text="L2 reasoning"),
               codex_event("token_count", info={"total_token_usage": {"input_tokens": 12}}),
               codex_row("compacted", {"replacement_history": []}, ordinal=7),
               codex_row("compacted", {"replacement_history": []}, ordinal=7),
               codex_row("response_item", {"type": "message", "role": "developer", "content": "<skills_instructions>hidden</skills_instructions>"}),
               codex_row("response_item", {"type": "message", "role": "user", "content": "# AGENTS.md instructions\n<INSTRUCTIONS>hidden</INSTRUCTIONS>"}),
               codex_row("response_item", {"type": "message", "role": "user", "content": "goal hidden",
                                           "internal_chat_message_metadata_passthrough": {"content_item_kinds": ["goal.internal_context"]}}),
               codex_event("task_started", turn_id="l2-1"),
               codex_row("response_item", {"type": "message", "role": "user", "turn_id": "l2-1", "content": "L2 question"}),
               codex_row("response_item", {"type": "reasoning", "turn_id": "l2-1", "summary": [{"type": "summary_text", "text": "L2 reasoning"}]}),
               codex_row("response_item", {"type": "function_call", "name": "exec_command", "call_id": "l2-call", "turn_id": "l2-1",
                                           "arguments": json.dumps({"cmd": "bash -lc 'echo l2'"})}),
               codex_row("response_item", {"type": "function_call_output", "call_id": "l2-call", "turn_id": "l2-1",
                                           "output": "Script completed\nOutput:\n" + json.dumps({"output": "l2", "exit_code": 0, "wall_time_seconds": 0.5})}),
               codex_row("response_item", {"type": "web_search_call", "turn_id": "l2-1"}),
               # Batch 33: current rollouts interleave telemetry/item records.
               *codex_telemetry_rows("l2-1"),
               codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "l2-1", "content": "L2 answer"}),
               codex_event("task_complete", turn_id="l2-1", duration_ms=300),
               codex_row("token_usage_record", {"turn_id": "l2-1", "total_tokens": 30})]
    corpus.put("codex-l2", "codex", l2_rows,
               corpus.expected["codex-l1"][:-1] + ["已压缩", "已压缩", "L2 question", "L2 reasoning", {"cmd": "bash -lc 'echo l2'"}, "l2", {}, "L2 answer"])

    agent = "codex-l1-agent"
    corpus.put(agent, "codex", [
        codex_row("session_meta", {"id": agent, "session_id": "codex-l1", "timestamp": "2026-09-11T09:30:00Z",
                                   "cwd": "/synthetic/advanced/agent", "forked_from_id": "codex-l1",
                                   "thread_source": "subagent", "parent_thread_id": "codex-l1",
                                   "source": {"subagent": {"thread_spawn": {"parent_thread_id": "codex-l1", "depth": 1,
                                                                            "agent_path": "worker/reviewer", "agent_role": "reviewer"}}}}),
        codex_row("turn_context", {"model": "synthetic-agent-model"}),
        codex_event("task_started", turn_id="agent-1"),
        codex_row("response_item", {"type": "message", "role": "user", "turn_id": "agent-1", "content": "Agent assignment from L1",
                                    "internal_chat_message_metadata_passthrough": {"turn_id": "agent-1", "content_item_kinds": ["inter_agent.assignment"]}}),
        codex_event("user_message", message="Agent assignment from L1"),
        codex_row("response_item", {"type": "function_call", "name": "exec_command", "call_id": "agent-call", "turn_id": "agent-1",
                                    "arguments": json.dumps({"cmd": ["zsh", "-lc", "rg agent"]})}),
        codex_row("response_item", {"type": "function_call_output", "call_id": "agent-call", "turn_id": "agent-1", "output": "agent match"}),
        codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "agent-1", "content": "Agent report to L1"}),
        codex_event("agent_message", message="Agent report to L1"),
        codex_event("task_complete", turn_id="agent-1", duration_ms=150),
    ], ["Agent assignment from L1", {"cmd": ["zsh", "-lc", "rg agent"]}, "agent match", "Agent report to L1"], parent="codex-l1")

    nested = "codex-l1-nested-agent"
    corpus.put(nested, "codex", [
        codex_row("session_meta", {"id": nested, "session_id": "codex-l1", "timestamp": "2026-09-11T09:31:00Z",
                                   "cwd": "/synthetic/advanced/agent", "forked_from_id": agent,
                                   "thread_source": "subagent", "parent_thread_id": agent,
                                   "source": {"subagent": {"thread_spawn": {"parent_thread_id": agent, "depth": 2, "agent_nickname": "nested helper"}}}}),
        codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "nested-1", "content": "Nested agent report"}),
    ], ["Nested agent report"], parent="codex-l1")

    corpus.put("codex-orphan-fork", "codex", [
        codex_meta("codex-orphan-fork", forked_from_id="codex-nowhere", history_base={"thread_id": "codex-nowhere", "end_byte_offset": 64}),
        codex_row("response_item", {"type": "message", "role": "user", "turn_id": "orphan-1", "content": "Question on an orphan fork"})], [])

    corpus.put("codex-badcut", "codex", [
        codex_meta("codex-badcut", forked_from_id="codex-root", history_base={"thread_id": "codex-root", "end_byte_offset": cut1 - 7}),
        codex_row("response_item", {"type": "message", "role": "user", "turn_id": "bad-1", "content": "Question after a mid-line cut"})], [])
    return {"cut1": cut1, "cut2": cut2}


def verify_codex(report, corpus, base, opener, adapters, rows, cuts):
    for sid in ("codex-root", "codex-l1", "codex-l2"):
        response = fetch(opener, base, corpus.uid(sid), "window=1")
        assert response["meta"]["sid"] == sid
        shown = [json.loads(row["text"]) if row["role"] == "tool" and row["text"] else row["text"] for row in response["messages"]]
        assert shown == corpus.expected[sid], (sid, shown)
        assert response["end"] == corpus.paths[sid].stat().st_size, (sid, "leaf-local cursor")
        if sid != "codex-root":
            assert not any("tail after cut" in row["text"] and "Root" in row["text"] for row in response["messages"])
            interrupted = next(row for row in response["messages"] if row["text"] == "L1 progress before interruption")
            assert interrupted.get("interrupted") is True and interrupted.get("interrupt_reason") == "user interrupted"
        if sid == "codex-l2":
            compacts = [row for row in response["messages"] if row.get("event_kind") == "compact"]
            assert [row["event_id"] for row in compacts] == ["compact:7", "compact:7"]
            assert all(row.get("counted") is False for row in compacts)
            envelope = next(row for row in response["messages"] if row.get("call_id") == "l2-call" and row["role"] == "tool_result")
            assert envelope["exit_code"] == 0 and envelope["duration_s"] == 0.5 and not envelope.get("error")
            expected_warnings = ["跳过未知的Codex 记录类型：token_usage_record ×2", "跳过未知的Codex event_msg：item_completed ×1",
                                 "跳过未知的Codex response_item：agent_message ×1", "跳过未知的Codex 记录类型：inter_agent_communication_metadata ×1"]
            assert rows[sid].get("supported") is True and "migration_warnings" not in rows[sid], rows[sid].get("migration_warnings")
            assert response["meta"].get("migration_warnings") == expected_warnings
        else:
            assert rows[sid].get("supported") is True and "migration_warnings" not in rows[sid], rows[sid].get("migration_warnings")
        if adapters:
            compare_view(report, f"codex {sid} inherited view", python_read(adapters, "codex", corpus.paths[sid]), response,
                         classify=classify_codex)
    assert rows["codex-l1"]["fork_depth"] == 1 and rows["codex-l2"]["fork_depth"] == 2
    assert rows["codex-l1"]["root_sid"] == rows["codex-l2"]["root_sid"] == "codex-root"
    assert [(item["id"], item["title"], item["type"]) for item in rows["codex-l1"]["agent_items"]] == [
        ("codex-l1-agent", "worker/reviewer", "reviewer"), ("codex-l1-nested-agent", "nested helper", "subagent")]
    assert not rows["codex-root"].get("agent_items") and not rows["codex-l2"].get("agent_items"), "agent ownership leaked across fork levels"
    nested = fetch(opener, base, corpus.uid("codex-l1"), urlencode({"agent": "codex-l1-nested-agent", "window": 1}))
    assert [row["text"] for row in nested["messages"]] == ["Nested agent report"] and nested["meta"]["sid"] == "codex-l1-nested-agent"
    if adapters:
        compare_view(report, "codex codex-l1 ?agent=codex-l1-nested-agent (agent of an agent)", python_read(adapters, "codex", corpus.paths["codex-l1-nested-agent"]), nested)
    agent = fetch(opener, base, corpus.uid("codex-l1"), urlencode({"agent": "codex-l1-agent", "window": 1}))
    assert agent["meta"]["sid"] == "codex-l1-agent"
    shown = [json.loads(row["text"]) if row["role"] == "tool" else row["text"] for row in agent["messages"]]
    assert shown == corpus.expected["codex-l1-agent"], shown
    assert not any("Root" in row["text"] or "L1 " in row["text"] for row in agent["messages"]), "subagent inherited the owner's fork history"
    assert agent["meta"]["model"] == "synthetic-agent-model" and agent["meta"]["cwd"] == "/synthetic/advanced/agent"
    if adapters:
        compare_view(report, "codex codex-l1 ?agent=codex-l1-agent", python_read(adapters, "codex", corpus.paths["codex-l1-agent"]), agent)
    for owner in ("codex-root", "codex-l2"):
        expect_status(opener, base, corpus.uid(owner), urlencode({"agent": "codex-l1-agent"}), 404)
    expect_status(opener, base, corpus.uid("codex-l1-agent"), "", 404)

    if adapters:
        codex_paths = {sid: path for sid, path in corpus.paths.items() if sid.startswith("codex-")}
        native = [adapters["codex"].session_meta(path) for path in codex_paths.values()]
        native = {row["sid"]: row for row in adapters["codex"].finalize_sessions(native)}
        assert set(native) == {"codex-root", "codex-l1", "codex-l2", "codex-badcut", "codex-orphan-fork", "codex-tools",
                               "codex-tools-extra", CHUNKS_SID, *BATCH35_CODEX_ROWS}, sorted(native)
        mismatches = []
        for sid in ("codex-root", "codex-l1", "codex-l2"):
            for key in ("root_sid", "fork_depth", "size", "forked_from_id", "created", "title", "cwd", "model"):
                expected, actual = native[sid].get(key), rows[sid].get(key)
                if key == "created":
                    expected = normalized({"ts": expected}).get("ts")
                    actual = normalized({"ts": actual}).get("ts")
                if expected != actual:
                    mismatches.append(f"{sid}.{key}: Python={clipped(expected)} Rust={clipped(actual)}")
            expected_agents = [(item["id"], item["title"], item["type"]) for item in native[sid].get("agent_items", [])]
            actual_agents = [(item["id"], item["title"], item["type"]) for item in rows[sid].get("agent_items", [])]
            if expected_agents != actual_agents:
                mismatches.append(f"{sid}.agent_items: Python={expected_agents} Rust={actual_agents}")
        for line in mismatches:
            report.fail("codex topology metadata " + line)
        if not mismatches:
            report.ok("codex topology metadata: root/fork depth/size/created/title/cwd/model/agent_items identical for three fork levels")

    sid = "codex-orphan-fork"
    message = expect_status(opener, base, corpus.uid(sid), "window=1", 501)
    assert "父线程不在已配置索引中" in message, message
    assert rows[sid].get("supported") is False, rows[sid]
    if adapters:
        # The synthetic allowlist refuses unknown SIDs; the real adapter returns
        # None from its private lookup and silently shows only the leaf file.
        lookup = adapters["codex"]._find_session_path
        adapters["codex"]._find_session_path = lambda sid: None if sid == "codex-nowhere" else lookup(sid)
        try:
            messages, _, _ = python_read(adapters, "codex", corpus.paths[sid])
        finally:
            adapters["codex"]._find_session_path = lookup
        report.delta(f"codex {sid}: parent thread absent from the configured root; Python silently shows the leaf only "
                     f"({[row['text'] for row in messages]}); Rust lists supported:false and answers HTTP 501 ({message})")
    else:
        report.delta(f"codex {sid}: parent thread absent; Rust lists supported:false and answers HTTP 501 ({message})")

    sid = "codex-badcut"
    message = expect_status(opener, base, corpus.uid(sid), "window=1", 501)
    assert "行边界" in message, message
    if adapters:
        messages, _, _ = python_read(adapters, "codex", corpus.paths[sid])
        report.delta(f"codex {sid}: mid-line history_base cut {cuts['cut1'] - 7}; Python silently drops the partial parent record and shows "
                     f"{len(messages)} messages {[row['text'] for row in messages]}; Rust supported={rows[sid].get('supported')} and HTTP 501 ({message})")
    else:
        report.delta(f"codex {sid}: mid-line history_base cut; Rust supported={rows[sid].get('supported')} and HTTP 501 ({message})")


def classify_codex(index, key, left, right):
    return None


# --------------------------------------------------------------------------
# Scenario 3: rich tool rendering across Claude, Codex and Grok
# --------------------------------------------------------------------------

PATCH = ("*** Begin Patch\n*** Add File: 新建/文件.txt\n+内容一\n+内容二\n*** Update File: 旧/路径.rs\n*** Move to: 新/路径.rs\n@@\n-old\n+new\n"
         "*** Delete File: 删除/文件.md\n-gone\n*** End Patch")
LONG_OUTPUT = "".join(f"line {index:07d} 输出行\n" for index in range(1, 70000))
OVERSIZED_CONTENT = "OVERSIZE START " + ("x" * 1024 + "\n") * 600 + " OVERSIZE END"
assert 1.4 * 1024 * 1024 < len(LONG_OUTPUT.encode()) < 1.7 * 1024 * 1024
assert len(OVERSIZED_CONTENT) > 512 * 1024


def tool_cases():
    """(name, arguments, output, is_error) rows shared by the three providers."""
    question = {"questions": [{"header": "Choose", "question": "Which path?", "multiSelect": True,
                               "options": [{"label": "A", "description": "first"}, "B", {"label": ""}]}]}
    return [
        ("AskUserQuestion", question, {"answers": {"q1": {"answers": ["A", " ", "B"]}}}, False),
        ("AskUserQuestion", question, "The user doesn't want to proceed with this tool use. The tool use was rejected.", True),
        ("Bash", {"command": "false"}, "Exit code 2\ncommand failed", True),
        ("Bash", {"command": "grep missing"}, "grep: exited with code 1", False),
        ("Bash", {"command": "true"}, [], False),
        ("Write", {"file_path": "docs/中文/说明.md", "content": "第一行\n第二行\n"}, "ok", False),
        ("Edit", {"file_path": "src/日本語/main.rs", "old_string": "fn main() {}\n// 旧注释\nkeep", "new_string": "fn main() { println!(\"héllo\"); }\n// 新注释\nkeep"}, "ok", False),
        ("MultiEdit", {"file_path": "notes/émoji 🚀.txt", "edits": [{"old_string": "a\nb", "new_string": "a\nc"}, {"old_string": "x", "new_string": "y"}, {}]}, "ok", False),
        ("apply_patch", {"input": PATCH}, "Done", False),
        ("Bash", {"command": "cat large.log"}, LONG_OUTPUT, False),
        ("Write", {"file_path": "oversize/大文件.txt", "content": OVERSIZED_CONTENT}, "ok", False),
        ("Bash", {"command": "echo mixed"}, [{"type": "text", "text": ""}, {"type": "text", "text": "second block"}, {"type": "text", "text": "third"}], False),
    ]


def build_tools(corpus):
    cases = tool_cases()
    sid = "claude-tools"
    rows = [claude_row(sid, "user", "u", None, "Rich tools question")]
    parent = "u"
    for index, (name, arguments, output, is_error) in enumerate(cases):
        rows.append(claude_row(sid, "assistant", f"a{index}", parent, [{"type": "tool_use", "id": f"c{index}", "name": name, "input": arguments}]))
        rows[-1]["message"]["stop_reason"] = "tool_use"
        block = {"type": "tool_result", "tool_use_id": f"c{index}", "content": output if not isinstance(output, dict) else json.dumps(output)}
        if is_error:
            block["is_error"] = True
        rows.append(claude_row(sid, "user", f"r{index}", f"a{index}", [block]))
        parent = f"r{index}"
    rows.append(claude_row(sid, "assistant", "final", parent, "Rich tools complete"))
    corpus.put(sid, "claude", rows, [])

    sid = "codex-tools"
    rows = [codex_meta(sid), codex_event("task_started", turn_id="tools"),
            codex_row("response_item", {"type": "message", "role": "user", "turn_id": "tools", "content": "Rich tools question"})]
    for index, (name, arguments, output, is_error) in enumerate(cases):
        codex_name = {"AskUserQuestion": "request_user_input", "Bash": "exec_command", "apply_patch": "functions.apply_patch"}.get(name, name)
        codex_arguments = {"cmd": arguments["command"]} if name == "Bash" else arguments
        rows.append(codex_row("response_item", {"type": "function_call", "name": codex_name, "call_id": f"c{index}", "turn_id": "tools",
                                                "arguments": json.dumps(codex_arguments, ensure_ascii=False)}))
        if name == "AskUserQuestion" and is_error:
            codex_output = "Aborted by user"
        elif name == "Bash" and is_error:
            codex_output = "Script completed\nOutput:\n" + json.dumps({"output": output, "exit_code": 2, "wall_time_seconds": 0.75})
        else:
            codex_output = output
        rows.append(codex_row("response_item", {"type": "function_call_output", "call_id": f"c{index}", "turn_id": "tools", "output": codex_output}))
    # Codex-only native shapes: structured local shell actions and custom tool patches.
    rows.append(codex_row("response_item", {"type": "local_shell_call", "call_id": "x1", "turn_id": "tools", "status": "completed",
                                            "action": {"type": "exec", "command": ["bash", "-lc", "ls 目录"], "timeout_ms": 1000}}))
    rows.append(codex_row("response_item", {"type": "local_shell_call_output", "call_id": "x1", "turn_id": "tools", "output": "目录\n"}))
    rows.append(codex_row("response_item", {"type": "custom_tool_call", "name": "apply_patch", "call_id": "x2", "turn_id": "tools", "input": PATCH}))
    rows.append(codex_row("response_item", {"type": "custom_tool_call_output", "call_id": "x2", "turn_id": "tools", "output": "Done"}))
    rows.append(codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "tools", "content": "Rich tools complete"}))
    rows.append(codex_event("task_complete", turn_id="tools", duration_ms=100))
    corpus.put(sid, "codex", rows, [])

    sid = "grok-tools"
    path = corpus.root / "grok/project-tools" / sid
    path.mkdir(parents=True)
    rows = [{"type": "user", "content": "Rich tools question", "prompt_index": 1, "timestamp": "2026-09-11T10:00:00Z"}]
    for index, (name, arguments, output, is_error) in enumerate(cases):
        rows.append({"type": "assistant", "content": "", "timestamp": "2026-09-11T10:00:00Z",
                     "tool_calls": [{"id": f"c{index}", "name": name, "arguments": json.dumps(arguments, ensure_ascii=False)}]})
        rows.append({"type": "tool_result", "tool_call_id": f"c{index}", "timestamp": "2026-09-11T10:00:00Z",
                     "content": output if not isinstance(output, dict) else json.dumps(output)})
    rows.append({"type": "assistant", "content": "Rich tools complete", "timestamp": "2026-09-11T10:00:00Z"})
    (path / "chat_history.jsonl").write_bytes(b"".join(encoded(row) for row in rows))
    (path / "summary.json").write_text(json.dumps({"info": {"id": sid, "cwd": "/synthetic/tools"}, "generated_title": "Rich tools"}))
    corpus.paths[sid] = path
    corpus.expected[sid] = []
    return cases


def classify_tools(source):
    def classify(index, key, left, right):
        # Oversized Write arguments: documented Rust budget warning instead of
        # Python's unbounded diff. Raw arguments stay identical in text.
        if key in ("changes", "changes_unavailable_reason") and right.get("changes_unavailable_reason") and "512 KiB" in right["changes_unavailable_reason"]:
            return "known"  # summarized by the oversized Write DELTA line below
        # Empty tool output: Python invents a "[图片]" placeholder; Rust keeps the empty text.
        if key == "text" and left.get("role") == "tool_result" and left.get("text") == "[图片]" and right.get("text") == "":
            return "delta"
        # Sniffed exit codes: Rust additionally reports exit_code for Codex/Grok
        # text output where Python only sets the error flag. Documented superset.
        if key == "exit_code" and source in ("codex", "grok") and left.get("exit_code") is None and right.get("exit_code") is not None and left.get("error") == right.get("error"):
            return "delta"
        return None
    return classify


def verify_tools(report, corpus, base, opener, adapters, cases):
    for source in ("claude", "codex", "grok"):
        sid = source + "-tools"
        response = fetch(opener, base, grok_uid(corpus.paths[sid]) if source == "grok" else corpus.uid(sid), "window=1")
        messages = response["messages"]
        assert response["meta"].get("supported", True) is not False
        assert messages[-1]["text"] == "Rich tools complete"
        by_call = {}
        for row in messages:
            if row.get("call_id"):
                by_call.setdefault(row["call_id"], []).append(row)
        questions = [row for row in messages if row["role"] == "question"]
        assert len(questions) == 2 and all(row["questions"][0]["multiple"] is True for row in questions)
        assert [option["label"] for option in questions[0]["questions"][0]["options"]] == ["A", "B"]
        if source == "grok":
            assert by_call["c0"][0]["role"] == "tool_result" and by_call["c0"][0]["name"] == "AskUserQuestion"
            assert not questions[0].get("call_id"), "Grok questions carry no call id in either implementation"
        else:
            question, answer = by_call["c0"]
            assert question["role"] == "question" and question["name"] == ("request_user_input" if source == "codex" else "AskUserQuestion")
            assert answer["role"] == "answer" and not answer.get("error")
            assert answer["text"] == ("A、B" if source == "codex" else json.dumps({"answers": {"q1": {"answers": ["A", " ", "B"]}}}))
            cancelled = by_call["c1"][1]
            assert cancelled["role"] == "answer" and cancelled["text"] == "已取消回答" and cancelled["error"] is True
        failed = by_call["c2"][1]
        assert failed["error"] is True and failed["exit_code"] == 2, (source, failed)
        if source == "codex":
            assert failed["duration_s"] == 0.75 and failed["text"] == "Exit code 2\ncommand failed"
        sniffed = by_call["c3"][1]
        assert sniffed["error"] is True, (source, sniffed)
        changes = {row["call_id"]: row["changes"] for row in messages if row["role"] == "tool" and row.get("changes")}
        assert [change["path"] for change in changes["c5"]] == ["docs/中文/说明.md"] and changes["c5"][0]["operation"] == "write"
        assert changes["c6"][0]["path"] == "src/日本語/main.rs" and changes["c6"][0]["added"] == 2 and changes["c6"][0]["removed"] == 2
        assert [change["path"] for change in changes["c7"]] == ["notes/émoji 🚀.txt"] * 3, "an empty edit object still yields an empty change"
        assert [(change["operation"], change["path"], change.get("new_path")) for change in changes["c8"]] == [
            ("add", "新建/文件.txt", None), ("update", "旧/路径.rs", "新/路径.rs"), ("delete", "删除/文件.md", None)]
        assert "é" in changes["c6"][0]["patch"] and "🚀" in changes["c7"][0]["patch"], "unicode paths must survive the diff header"
        long_output = by_call["c9"][1]
        assert long_output["text"] == LONG_OUTPUT and long_output["role"] == "tool_result"
        oversized = by_call["c10"][0]
        assert oversized["changes"] is None and "512 KiB" in oversized["changes_unavailable_reason"]
        assert json.loads(oversized["text"])["content"] == OVERSIZED_CONTENT
        if source == "codex":
            assert by_call["x1"][0]["summary"] == "$ ls 目录" and by_call["x1"][0]["name"] == "local_shell_call"
            assert by_call["x1"][1]["text"] == "目录\n" and by_call["x2"][1]["name"] == "apply_patch"
            assert [change["operation"] for change in by_call["x2"][0]["changes"]] == ["add", "update", "delete"]
        if adapters:
            python = python_read(adapters, source, corpus.paths[sid])
            python_oversized = next(row for row in python[0] if row.get("call_id") == "c10" and row["role"] == "tool")
            assert python_oversized["changes"] and python_oversized["changes"][0]["added"] == OVERSIZED_CONTENT.count("\n") + 1
            compare_view(report, f"{source} rich tools", python, response, classify=classify_tools(source), strip_ts=source == "grok")
            report.delta(f"{source} oversized Write: Python computes {len(python_oversized['changes'])} change with {python_oversized['changes'][0]['added']} added lines; "
                         f"Rust changes=null with '{oversized['changes_unavailable_reason']}' and identical raw arguments")
        else:
            report.delta(f"{source} oversized Write: Rust changes=null with '{oversized['changes_unavailable_reason']}' and identical raw arguments")
        report.ok(f"{source} rich tools: questions/answers/cancellation, tool errors, unicode Write/Edit/MultiEdit/apply_patch changes, "
                  f"{len(LONG_OUTPUT.encode()) // 1024} KiB output intact")


# --------------------------------------------------------------------------
# Scenario 3b: extra Codex-native tool shapes not covered above
# --------------------------------------------------------------------------

EXTRA_PATCH = ("*** Begin Patch\n*** Add File: extra/added.txt\n+new extra content\n"
               "*** Update File: extra/更新.rs\n@@\n-old extra\n+new extra\n"
               "*** Delete File: extra/removed.md\n-deleted extra\n*** End Patch")
EXTRA_SHELL = ["zsh", "-lc", "echo 'quoted 参数 with spaces'"]
EXTRA_SHELL_OUTPUT = "line1\r\nline2 中文\r\nline3"
EXTRA_EXEC_ARGS = {"cmd": "echo \"hello 世界\"", "options": {"note": "say \"hi\"", "retries": 2}}
EXTRA_EXEC_INNER = "grep: exited with code 1\nno matches"
EXTRA_SEARCH = {"query": "rust unicode 搜索", "count": 5}


def build_codex_tools_extra(corpus):
    sid = "codex-tools-extra"
    envelope = json.dumps({"output": EXTRA_SHELL_OUTPUT, "exit_code": 2, "wall_time_seconds": 1.25}, ensure_ascii=False)
    exec_envelope = json.dumps({"output": EXTRA_EXEC_INNER, "wall_time_seconds": 0.5, "session_id": "extra-exec-session"},
                               ensure_ascii=False)
    prefix = "Protocol wait: session extra-exec still running after 12s; diagnostics=" + ("X" * 160)
    rows = [
        codex_meta(sid),
        codex_event("task_started", turn_id="extra"),
        codex_row("response_item", {"type": "message", "role": "user", "turn_id": "extra", "content": "Extra Codex tools"}),
        codex_row("response_item", {"type": "reasoning", "turn_id": "extra",
                                    "summary": [{"type": "summary_text", "text": "First extra thought"},
                                                {"type": "summary_text", "text": "Second extra 思考"}]}),
        codex_row("response_item", {"type": "local_shell_call", "call_id": "extra-shell", "turn_id": "extra",
                                    "status": "completed",
                                    "action": {"type": "exec", "command": EXTRA_SHELL, "timeout_ms": 2000}}),
        codex_row("response_item", {"type": "local_shell_call_output", "call_id": "extra-shell", "turn_id": "extra",
                                    "output": envelope}),
        codex_row("response_item", {"type": "custom_tool_call", "name": "apply_patch", "call_id": "extra-patch",
                                    "turn_id": "extra", "input": EXTRA_PATCH}),
        codex_row("response_item", {"type": "custom_tool_call_output", "call_id": "extra-patch", "turn_id": "extra",
                                    "output": "Done"}),
        codex_row("response_item", {"type": "function_call", "name": "exec_command", "call_id": "extra-exec",
                                    "turn_id": "extra", "arguments": json.dumps(EXTRA_EXEC_ARGS, ensure_ascii=False)}),
        codex_row("response_item", {"type": "function_call_output", "call_id": "extra-exec", "turn_id": "extra",
                                    "output": prefix + "\nOutput:\n" + exec_envelope}),
        codex_row("response_item", {"type": "web_search_call", "turn_id": "extra",
                                    "arguments": json.dumps(EXTRA_SEARCH, ensure_ascii=False)}),
        codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "extra",
                                    "content": "Extra tools complete"}),
        codex_event("task_complete", turn_id="extra", duration_ms=50),
    ]
    corpus.put(sid, "codex", rows, [])


def verify_codex_tools_extra(report, corpus, base, opener, adapters):
    sid = "codex-tools-extra"
    response = fetch(opener, base, corpus.uid(sid), "window=1")
    messages = response["messages"]
    assert response["meta"].get("supported", True) is not False
    assert messages[-1]["text"] == "Extra tools complete"
    by_call = {}
    for row in messages:
        if row.get("call_id"):
            by_call.setdefault(row["call_id"], []).append(row)
    thinking = [row for row in messages if row["role"] == "thinking"]
    assert len(thinking) == 1 and thinking[0]["text"] == "First extra thought\nSecond extra 思考", thinking
    shell_tool, shell_out = by_call["extra-shell"]
    assert shell_tool["role"] == "tool" and shell_tool["name"] == "local_shell_call"
    assert shell_tool["summary"] == "$ echo 'quoted 参数 with spaces'"
    assert shell_out["role"] == "tool_result" and shell_out["text"] == EXTRA_SHELL_OUTPUT
    assert "\r\n" in shell_out["text"] and shell_out["exit_code"] == 2 and shell_out["error"] is True
    assert shell_out["duration_s"] == 1.25
    patch_tool, patch_out = by_call["extra-patch"]
    assert patch_tool["name"] == "apply_patch" and patch_out["text"] == "Done"
    assert [(change["operation"], change["path"]) for change in patch_tool["changes"]] == [
        ("add", "extra/added.txt"), ("update", "extra/更新.rs"), ("delete", "extra/removed.md")]
    exec_tool, exec_out = by_call["extra-exec"]
    assert exec_tool["name"] == "exec_command" and json.loads(exec_tool["text"]) == EXTRA_EXEC_ARGS
    assert exec_tool["summary"] == '$ echo "hello 世界"'
    assert exec_out["text"] == EXTRA_EXEC_INNER and exec_out["error"] is True and exec_out["duration_s"] == 0.5
    assert "Protocol wait" not in exec_out["text"]
    search = next(row for row in messages if row.get("name") == "web_search_call")
    assert json.loads(search["text"]) == EXTRA_SEARCH and "\n  " in search["text"], search["text"]
    if adapters:
        python = python_read(adapters, "codex", corpus.paths[sid])
        compare_view(report, "codex extra tools", python, response, classify=classify_tools("codex"))
    report.ok("codex extra tools: quoted unicode local_shell + CRLF envelope, three-file apply_patch, "
              "exec_command nested args + prefixed Output:\\n, web_search_call arguments, two-part reasoning")


# --------------------------------------------------------------------------
# Scenario 4: Grok summary + chat with tool_result rows, summary-only session
# --------------------------------------------------------------------------

def build_grok(corpus):
    base = corpus.root / "grok/%2Fsynthetic%2Fadvanced"
    chat = base / "grok-chat"
    chat.mkdir(parents=True)
    (chat / "summary.json").write_text(json.dumps({
        "info": {"id": "grok-chat", "cwd": "/synthetic/advanced"}, "generated_title": "Grok advanced chat",
        "session_summary": "unused", "created_at": "2026-09-11T10:00:00Z", "last_active_at": "2026-09-11T10:05:00Z",
        "current_model_id": "synthetic-grok", "agent_name": "synthetic-agent"}), encoding="utf-8")
    rows = [
        {"type": "user", "content": "<user_query>\nGrok question one\n</user_query>", "prompt_index": 1},
        {"type": "reasoning", "summary": [{"type": "summary_text", "text": "Grok reasoning"}]},
        {"type": "assistant", "content": "Grok running a command", "tool_calls": [
            {"id": "g1", "name": "Bash", "arguments": json.dumps({"command": "/bin/sh -c 'ls 目录'"})},
            {"id": "g2", "function": {"name": "Read", "arguments": json.dumps({"file_path": "docs/说明.md"})}}]},
        {"type": "tool_result", "tool_call_id": "g1", "content": "目录 listing"},
        {"type": "tool_result", "tool_call_id": "g2", "content": [{"type": "text", "text": "Read failed: exit status 3"}]},
        {"type": "tool_result", "tool_call_id": "unknown-call", "content": "orphan output"},
        {"type": "user", "content": "Grok question two", "prompt_index": 2},
        {"type": "assistant", "content": "Grok final answer"},
        {"type": "user", "content": "<image_files>\n1. /synthetic/advanced/shot.png\n</image_files>\n\n<user_query>\n[Image #1] Grok question with an image list\n</user_query>", "prompt_index": 4},
        {"type": "user", "content": "<user_query>\n  indented first line\nsecond line\n</user_query>", "prompt_index": 5},
        {"type": "assistant", "content": "Grok answer after envelopes"},
        {"type": "user", "content": "<environment_context>hidden</environment_context>", "prompt_index": 3},
        {"type": "system", "content": "Grok system notice"},
        # Batch 33: unknown Grok record kinds are skipped like Python's if/elif chain.
        {"type": "usage", "prompt_tokens": 3, "completion_tokens": 4},
        {"type": "usage", "prompt_tokens": 5, "completion_tokens": 6},
        {"type": "checkpoint", "id": "synthetic"},
    ]
    (chat / "chat_history.jsonl").write_bytes(b"".join(encoded(row) for row in rows))
    corpus.paths["grok-chat"] = chat
    corpus.expected["grok-chat"] = ["Grok question one", "Grok reasoning", "Grok running a command", {"command": "/bin/sh -c 'ls 目录'"},
                                    {"file_path": "docs/说明.md"}, "目录 listing", "Read failed: exit status 3", "orphan output",
                                    "Grok question two", "Grok final answer", "[Image #1] Grok question with an image list",
                                    "  indented first line\nsecond line", "Grok answer after envelopes", "Grok system notice"]
    summary_only = base / "grok-summary-only"
    summary_only.mkdir()
    (summary_only / "summary.json").write_text(json.dumps({"info": {"id": "grok-summary-only", "cwd": "/synthetic/advanced"},
                                                           "generated_title": "Grok summary only"}), encoding="utf-8")
    corpus.paths["grok-summary-only"] = summary_only
    corpus.expected["grok-summary-only"] = []


def grok_uid(path):
    return "grok:" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


def classify_grok(index, key, left, right):
    # Same documented superset as the tool scenario: sniffed exit codes.
    if key == "exit_code" and left.get("exit_code") is None and right.get("exit_code") is not None and left.get("error") == right.get("error"):
        return "delta"
    if key == "text" and left.get("role") == "user" and right.get("turn_id") in ("prompt:4", "prompt:5"):
        return "known"  # user_query envelope handling, summarized separately above
    return None


def verify_grok(report, corpus, base, opener, adapters, rows):
    response = fetch(opener, base, grok_uid(corpus.paths["grok-chat"]), "window=1")
    shown = [json.loads(row["text"]) if row["role"] == "tool" else row["text"] for row in response["messages"]]
    python_envelopes = ["[Image #1] Grok question with an image list", "  indented first line\nsecond line"]
    envelopes = [row["text"] for row in response["messages"] if row.get("turn_id") in ("prompt:4", "prompt:5") and row["role"] == "user"]
    expected = list(corpus.expected["grok-chat"])
    for index, text in enumerate(python_envelopes):
        expected[expected.index(text)] = envelopes[index]
    assert shown == expected, shown
    if envelopes[0] != python_envelopes[0]:
        # Raw protocol XML shown as the user's words is a rendering bug, not a policy delta.
        report.fail(f"grok grok-chat <image_files>+<user_query> envelope: Rust renders {envelopes[0]!r}; Python strips it to {python_envelopes[0]!r}")
    if envelopes[1] != python_envelopes[1]:
        report.unverified(f"grok grok-chat <user_query> whitespace: Rust trims all surrounding whitespace {envelopes[1]!r}; "
                          f"Python removes exactly one leading/trailing newline {python_envelopes[1]!r}")
    tools = {row.get("call_id"): row for row in response["messages"] if row["role"] == "tool"}
    assert tools["g1"]["summary"] == "$ ls 目录" and tools["g2"]["summary"] == "读 docs/说明.md"
    results = [row for row in response["messages"] if row["role"] == "tool_result"]
    assert results[0]["name"] == "Bash" and not results[0].get("error")
    assert results[1]["name"] == "Read" and results[1]["error"] is True
    assert results[2].get("name") is None and results[2]["call_id"] == "unknown-call"
    assert [row.get("turn_id") for row in response["messages"] if row["role"] == "user"] == ["prompt:1", "prompt:2", "prompt:4", "prompt:5"]
    assert rows["grok-chat"]["title"] == "Grok advanced chat" and rows["grok-chat"]["model"] == "synthetic-grok"
    assert rows["grok-chat"].get("supported") is True and "migration_warnings" not in rows["grok-chat"]
    assert response["meta"].get("migration_warnings") == ["跳过未知的Grok 记录类型：usage ×2", "跳过未知的Grok 记录类型：checkpoint ×1"], response["meta"].get("migration_warnings")
    if adapters:
        compare_view(report, "grok grok-chat summary+chat", python_read(adapters, "grok", corpus.paths["grok-chat"]), response, classify=classify_grok, strip_ts=True)
        expected = adapters["grok"].session_meta(corpus.paths["grok-chat"])
        for field in ("sid", "title", "cwd", "model", "branch"):
            assert expected.get(field) == rows["grok-chat"].get(field), (field, expected.get(field), rows["grok-chat"].get(field))
        for field in ("created", "updated"):
            assert normalized({"ts": expected.get(field)}).get("ts") == normalized({"ts": rows["grok-chat"].get(field)}).get("ts"), field

    summary_only = corpus.paths["grok-summary-only"]
    listed = rows.get("grok-summary-only")
    python_meta = adapters["grok"].session_meta(summary_only) if adapters else None
    if listed is None:
        report.delta("grok grok-summary-only: Rust omits the summary-only session from /api/sessions (plan: 只有 summary 未有 chat 的会话暂不列表); "
                     + (f"Python session_meta lists it as {python_meta['title']!r}" if python_meta else "Python lists it"))
        expect_status(opener, base, grok_uid(summary_only), "window=1", 404)
    else:
        response = fetch(opener, base, grok_uid(summary_only), "window=1")
        assert response["messages"] == [] and response["end"] == 0 and response["meta"]["chat_exists"] is False
        assert listed["title"] == "Grok summary only"
        if python_meta:
            assert python_meta["title"] == listed["title"] and python_meta["sid"] == listed["sid"]
            messages, _, end = python_read(adapters, "grok", summary_only)
            assert messages == [] and end == 0
        report.ok("grok grok-summary-only: Rust now lists the summary-only session with an empty body like Python "
                  "(the plan table row '只有 summary 未有 chat 的会话暂不列表' is stale)")


# --------------------------------------------------------------------------
# Scenario 5: batch-35 real-root shapes (Python reads them; Rust must match)
# --------------------------------------------------------------------------

BATCH35_ROW_WARNINGS = {
    "codex-legacy-root": [], "codex-legacy-mid": ["跳过重复的Codex session_meta ×1"],
    "codex-legacy-fork": ["跳过重复的Codex session_meta ×2"], "codex-legacy-orphan": ["跳过重复的Codex session_meta ×2"],
    "codex-rewind-root": [], "codex-rewind-q": [], "codex-rewind-a": [],
    "claude-torn": ["跳过无效的JSONL 记录 ×1"], "claude-lost-leaf": [], "claude-cycle": [],
}
BATCH35_DETAIL_WARNINGS = {
    **BATCH35_ROW_WARNINGS,
    "claude-torn": ["跳过无效的JSONL 记录 ×1", "Claude 祖先链在 torn-lost 处中断，之前的记录不在当前时间线"],
    "claude-lost-leaf": ["Claude 声明的叶子 never-written 不在记录中"],
    "codex-copied-agent": ["跳过重复的Codex session_meta ×1"],
}


def verify_batch35(report, corpus, base, opener, adapters, rows):
    hidden = {"codex-copied-orphan-agent", "claude-orphan-agent"}
    assert hidden == corpus.hidden == set(BATCH35_HIDDEN), sorted(corpus.hidden)
    for sid in sorted(hidden):
        assert sid not in rows, (sid, "orphan agent must not be a list row (Python never lists it)")
        expect_status(opener, base, corpus.uid(sid), "window=1", 501)
    report.ok("batch-35 orphan agents (Claude sidecar without owner, Codex subagent with copied meta and absent parent): not listed, physical UID HTTP 501")
    assert {item["id"] for item in rows["codex-legacy-root"].get("agent_items") or []} == set(BATCH35_AGENTS), rows["codex-legacy-root"].get("agent_items")
    for sid in (*BATCH35_CODEX_ROWS, *BATCH35_CLAUDE_ROWS, *BATCH35_AGENTS):
        uid, query = corpus.view(sid)
        response = fetch_supported(opener, base, sid, uid, (query + "&" if query else "") + "window=1")
        assert response["meta"]["sid"] == sid
        if sid not in BATCH35_AGENTS:
            assert rows[sid].get("supported") is True and "migration_warnings" not in rows[sid], (sid, rows[sid].get("migration_warnings"))
        assert response["meta"].get("supported") is True, (sid, response["meta"].get("migration_warnings"))
        warnings = response["meta"].get("migration_warnings") or []
        if sid == "claude-cycle":
            assert warnings in ([f"Claude 祖先链存在循环，已在 {uid} 处截断"] for uid in ("a1", "u1")), warnings
        else:
            assert sorted(warnings) == sorted(BATCH35_DETAIL_WARNINGS[sid]), (sid, warnings)
        assert [row["text"] for row in response["messages"]] == corpus.expected[sid], (sid, [row["text"] for row in response["messages"]])
        assert response["end"] == corpus.paths[sid].stat().st_size, (sid, "leaf-local cursor")
        if adapters:
            source = uid.split(":", 1)[0]
            compare_view(report, f"{source} {sid} batch-35 shape", python_read(adapters, source, corpus.paths[sid]), response)
    for sid, root_sid, depth in (("codex-legacy-mid", "codex-legacy-root", 1), ("codex-legacy-fork", "codex-legacy-root", 2),
                                 ("codex-rewind-q", "codex-rewind-root", 1), ("codex-rewind-a", "codex-rewind-root", 2)):
        assert rows[sid].get("root_sid") == root_sid and rows[sid].get("fork_depth") == depth, (sid, rows[sid].get("root_sid"), rows[sid].get("fork_depth"))
        assert rows[sid]["created"] == rows[root_sid]["created"] and rows[sid]["title"] == rows[root_sid]["title"], sid
    assert rows["codex-legacy-orphan"].get("root_sid") is None and rows["codex-legacy-orphan"]["forked_from_id"] == "codex-legacy-gone"
    if adapters:
        codex_paths = {sid: path for sid, path in corpus.paths.items() if sid.startswith("codex-")}
        native = [adapters["codex"].session_meta(path) for path in codex_paths.values()]
        native = {row["sid"]: row for row in adapters["codex"].finalize_sessions(native)}
        mismatches = []
        for sid in BATCH35_CODEX_ROWS:
            for key in ("root_sid", "fork_depth", "size", "forked_from_id", "created", "title", "cwd"):
                expected, actual = native[sid].get(key), rows[sid].get(key)
                if key == "created":
                    expected = normalized({"ts": expected}).get("ts")
                    actual = normalized({"ts": actual}).get("ts")
                if expected != actual:
                    mismatches.append(f"{sid}.{key}: Python={clipped(expected)} Rust={clipped(actual)}")
            expected_agents = [(item["id"], item["title"], item["type"]) for item in native[sid].get("agent_items", [])]
            actual_agents = [(item["id"], item["title"], item["type"]) for item in rows[sid].get("agent_items", [])]
            if expected_agents != actual_agents:
                mismatches.append(f"{sid}.agent_items: Python={expected_agents} Rust={actual_agents}")
        for line in mismatches:
            report.fail("batch-35 codex list metadata " + line)
        if not mismatches:
            report.ok("batch-35 codex list metadata: root_sid/fork_depth/size/created/title/cwd/agent_items identical to finalize_sessions for "
                      "legacy self-contained forks (parent present/absent), rewind past the parent's fork point and the copied-meta subagent")
    report.ok("batch-35 shapes: first session_meta is the identity, later copies counted; null history_base forks read alone; "
              "history_base.thread_id is the physical parent and forked_from_id the logical chain; torn line / missing leaf / parent cycle are warnings")


# --------------------------------------------------------------------------

# --------------------------------------------------------------------------
# Scenario 7 (batch 36, WP-D): Esc-interrupted Claude turns with tools,
# thinking, a compact boundary above them, and the interrupt-as-sibling
# corner. Expectations derived from the Python adapter (d16c5e1).
# --------------------------------------------------------------------------

BATCH36_SIDS = ("claude-esc-deep", "claude-esc-compact", "claude-esc-sibling")


def build_claude_interrupts(corpus):
    def row(sid, kind, uid, parent, text="", **extra):
        return claude_row(sid, kind, uid, parent, text, timestamp="2026-09-10T10:00:00Z", **extra)

    def duration(sid, uid, parent):
        return {"type": "system", "subtype": "turn_duration", "uuid": uid, "parentUuid": parent, "sessionId": sid,
                "durationMs": 900, "timestamp": "2026-09-10T10:00:01Z", "isSidechain": False}

    # (a) The interrupted turn holds text + tool_use, a tool_result, thinking
    # and a second reply before the native interrupt record; the replacement
    # hangs off the previous turn_duration.
    sid = "claude-esc-deep"
    corpus.put(sid, "claude", [
        row(sid, "user", "u0", None, "Deep root question"),
        row(sid, "assistant", "a0", "u0", "Deep root answer"),
        duration(sid, "t0", "a0"),
        row(sid, "user", "u1", "t0", "Deep cut-short question"),
        row(sid, "assistant", "a1", "u1", [{"type": "text", "text": "Deep calling"},
                                            {"type": "tool_use", "id": "deep-call", "name": "Bash", "input": {"command": "printf deep"}}]),
        row(sid, "user", "r1", "a1", [{"type": "tool_result", "tool_use_id": "deep-call", "content": "deep"}]),
        row(sid, "assistant", "a1b", "r1", [{"type": "thinking", "thinking": "Deep private reasoning"},
                                             {"type": "text", "text": "Deep more"}]),
        row(sid, "user", "stop", "a1b", "[Request interrupted by user for tool use]"),
        row(sid, "user", "u2", "t0", "Deep replacement question"),
        row(sid, "assistant", "a2", "u2", "Deep replacement answer"),
    ], ["Deep root question", "Deep root answer", "", "Deep cut-short question", "Deep calling", {"command": "printf deep"}, "deep",
        "Deep private reasoning", "Deep more", "Deep replacement question", "Deep replacement answer"])
    # (b) The same interrupted turn after a compact boundary and under a
    # last-prompt that names the replacement's reply.
    sid = "claude-esc-compact"
    corpus.put(sid, "claude", [
        row(sid, "user", "u0", None, "Compact esc root question"),
        row(sid, "assistant", "a0", "u0", "Compact esc root answer"),
        row(sid, "system", "compact", None, compactMetadata={"trigger": "auto", "durationMs": 500}),
        row(sid, "user", "summary", "compact", "INTERNAL COMPACT SUMMARY", isCompactSummary=True),
        row(sid, "user", "u1", "summary", "Compact esc cut-short question"),
        row(sid, "assistant", "a1", "u1", "Compact esc partial answer"),
        row(sid, "user", "stop", "a1", "[Request interrupted by user]"),
        row(sid, "user", "u2", "summary", "Compact esc replacement question"),
        row(sid, "assistant", "a2", "u2", "Compact esc replacement answer"),
        {"type": "last-prompt", "leafUuid": "a2"},
    ], ["Compact esc root question", "Compact esc root answer", "已压缩", "Compact esc cut-short question", "Compact esc partial answer",
        "Compact esc replacement question", "Compact esc replacement answer"])
    # (c) The interrupt record is itself the unanswered sibling of the
    # replacement: only the native `aborted`, no bubble, no turn of its own.
    sid = "claude-esc-sibling"
    corpus.put(sid, "claude", [
        row(sid, "user", "u0", None, "Sibling esc root question"),
        row(sid, "assistant", "a0", "u0", "Sibling esc root answer"),
        row(sid, "user", "stop", "a0", "[Request interrupted by user]"),
        row(sid, "user", "u1", "a0", "Sibling esc replacement question"),
        row(sid, "assistant", "a1", "u1", "Sibling esc replacement answer"),
    ], ["Sibling esc root question", "Sibling esc root answer", "Sibling esc replacement question", "Sibling esc replacement answer"])


def verify_claude_interrupts(report, corpus, base, opener, adapters, rows):
    for sid in BATCH36_SIDS:
        assert rows[sid].get("supported") is True and "migration_warnings" not in rows[sid], (sid, rows[sid].get("migration_warnings"))
        response = fetch_supported(opener, base, sid, corpus.uid(sid), "window=1")
        assert response["meta"].get("supported") is True and response["meta"].get("migration_warnings") == [], (sid, response["meta"])
        shown = [json.loads(row["text"]) if row["role"] == "tool" else row["text"] for row in response["messages"]]
        assert shown == corpus.expected[sid], (sid, shown)
        assert not any("[Request interrupted" in row["text"] for row in response["messages"]), (sid, "interrupt record must be a status")
        assert response["end"] == corpus.paths[sid].stat().st_size, (sid, "leaf-local cursor")
        interrupted = [(row["role"], row["text"]) for row in response["messages"] if row.get("interrupted")]
        turns = [row.get("turn_id") for row in response["messages"]]
        if sid == "claude-esc-deep":
            assert interrupted == [("user", "Deep cut-short question")], interrupted
            roles = [row["role"] for row in response["messages"]]
            assert roles == ["user", "assistant", "event", "user", "assistant", "tool", "tool_result", "thinking", "assistant", "user", "assistant"], roles
            assert turns == ["u0", "u0", "u0", "u1", "u1", "u1", "u1", "u1", "u1", "u2", "u2"], turns
            assert response["messages"][3].get("interrupt_reason") == "输入已中断，未进入当前 Claude 分支"
            assert response["message_total"] == 10
        elif sid == "claude-esc-compact":
            assert interrupted == [("user", "Compact esc cut-short question")], interrupted
            assert turns == ["u0", "u0", "u0", "u1", "u1", "u2", "u2"], turns
            assert response["messages"][2]["event_kind"] == "compact" and response["messages"][2].get("counted") is False
        else:
            assert interrupted == [], interrupted
            assert turns == ["u0", "u0", "u1", "u1"], turns
        assert response["activity"]["state"] == "working" and response["activity"]["turn_id"] == ("u1" if sid == "claude-esc-sibling" else "u2"), (sid, response["activity"])
        if adapters:
            compare_view(report, f"claude {sid} interrupted turn (batch 36)", python_read(adapters, "claude", corpus.paths[sid]), response)
    report.ok("batch-36 Claude interrupted turns: tools/thinking under the cut-short input stay visible under its turn id, the native interrupt "
              "record is the aborted status, a compact boundary and last-prompt above it change nothing, an interrupt as the unanswered sibling renders only the status")


# --------------------------------------------------------------------------
# Scenario 7 (batch 36, WP-G): Codex multi-chunk tool-output envelopes
# --------------------------------------------------------------------------

CHUNKS_SID = "codex-envelope-chunks"
CHUNKS_HEADER = "Script completed\nWall time 0.4 seconds\nOutput:\n"
CHUNKS_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="


def chunk_part(cid, output, wall, **extra):
    return {"type": "input_text", "text": json.dumps({"chunk_id": cid, "wall_time_seconds": wall, "original_token_count": 2,
                                                       "output": output, **extra}, ensure_ascii=False)}


def chunk_cases():
    """(call_id, parts, python_chunk_index) — the real `exec` shapes: a header
    part, one stringified envelope per streamed chunk, then a trailing empty
    part, `{}` or an image. The reference `_tool_output` decodes exactly one
    chunk: the last one when the joined text ends with a chunk (its reversed
    object-line search parses that suffix), otherwise the first envelope part."""
    text_part = lambda text: {"type": "input_text", "text": text}
    image = {"type": "input_image", "image_url": f"data:image/png;base64,{CHUNKS_PNG}"}
    return [
        ("two", [text_part(CHUNKS_HEADER), chunk_part("a", "first\n", 0.25, exit_code=0), chunk_part("b", "second\n", 0.5, exit_code=0)], -1),
        ("three-empty-tail", [text_part(CHUNKS_HEADER), chunk_part("a", "one\n", 0.25, exit_code=0), chunk_part("b", "two\n", 0.25, session_id=7),
                              chunk_part("c", "three\n", 0.5, exit_code=0), text_part("")], -1),
        ("last-exit-image", [text_part(CHUNKS_HEADER), chunk_part("a", "PASS a\n", 0.001, exit_code=0), chunk_part("b", "compiling\r\n", 1.0, session_id=9),
                             chunk_part("c", "ssh: timed out\r\n", 0.002, exit_code=255), image], 0),
        ("five-brace-tail", [text_part(CHUNKS_HEADER), *[chunk_part(f"c{n}", f"line {n}\n", 0.1, exit_code=0) for n in range(5)], text_part("{}")], 0),
        ("prints-between", [text_part(CHUNKS_HEADER), text_part("--- 1 ---"), chunk_part("a", "alpha\n", 0.5, exit_code=1), text_part("--- 2 ---"),
                            chunk_part("b", "beta\n", 0.5, exit_code=0)], -1),
    ]


def build_codex_chunks(corpus):
    rows = [codex_meta(CHUNKS_SID), codex_event("task_started", turn_id="chunks"),
            codex_row("response_item", {"type": "message", "role": "user", "turn_id": "chunks", "content": "Chunked exec results"})]
    for call_id, parts, _ in chunk_cases():
        rows.append(codex_row("response_item", {"type": "custom_tool_call", "name": "exec", "call_id": call_id, "turn_id": "chunks",
                                                "input": 'text(await tools.exec_command({cmd: "printf chunks"}))'}))
        rows.append(codex_row("response_item", {"type": "custom_tool_call_output", "call_id": call_id, "turn_id": "chunks", "output": parts}))
    rows.append(codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "chunks",
                                            "content": "Chunked exec complete"}))
    rows.append(codex_event("task_complete", turn_id="chunks", duration_ms=40))
    corpus.put(CHUNKS_SID, "codex", rows, [])


def chunk_expectations():
    """Per call id: the envelopes, Rust's aggregate and the single chunk Python shows."""
    out = {}
    for call_id, parts, python_index in chunk_cases():
        envelopes = [json.loads(part["text"]) for part in parts if part.get("type") == "input_text"
                     and part["text"].startswith("{") and "chunk_id" in part["text"]]
        exit_codes = [env["exit_code"] for env in envelopes if "exit_code" in env]
        rust = {"text": "".join(env["output"] for env in envelopes), "exit_code": exit_codes[-1] if exit_codes else None,
                "duration_s": sum(env["wall_time_seconds"] for env in envelopes)}
        chosen = envelopes[python_index]
        python = {"text": chosen["output"], "exit_code": chosen.get("exit_code"), "duration_s": chosen["wall_time_seconds"]}
        for side in (rust, python):
            side["error"] = side["exit_code"] is not None and side["exit_code"] != 0
        out[call_id] = (rust, python, len(envelopes))
    return out


def classify_codex_chunks(rust_rows):
    expectations = chunk_expectations()

    def classify(index, key, left, right):
        # Documented DELTA (docs/migration.md, batch 36 WP-G): Python shows one
        # chunk's output/exit_code/wall time, Rust the concatenation of every
        # chunk with the last exit code and the summed duration. Anything
        # else on these rows is a real difference.
        row = rust_rows[index]
        if row.get("role") != "tool_result" or row.get("call_id") not in expectations:
            return None
        rust, python, _ = expectations[row["call_id"]]
        if key not in ("text", "exit_code", "duration_s", "error"):
            return None
        # normalized() drops a false `error`; exit_code 0 stays a value.
        expected_left = python[key] if not (key == "error" and python[key] is False) else None
        expected_right = rust[key] if not (key == "error" and rust[key] is False) else None
        if key == "duration_s":
            close = lambda a, b: a is not None and b is not None and abs(a - b) < 1e-9
            return "delta" if close(left.get(key), expected_left) and close(right.get(key), expected_right) else None
        return "delta" if left.get(key) == expected_left and right.get(key) == expected_right else None
    return classify


def verify_codex_chunks(report, corpus, base, opener, adapters, rows):
    assert rows[CHUNKS_SID].get("supported") is True and "migration_warnings" not in rows[CHUNKS_SID], rows[CHUNKS_SID]
    response = fetch_supported(opener, base, CHUNKS_SID, corpus.uid(CHUNKS_SID), "window=1")
    messages = response["messages"]
    assert messages[-1]["text"] == "Chunked exec complete"
    results = {row["call_id"]: row for row in messages if row["role"] == "tool_result"}
    expectations = chunk_expectations()
    assert set(results) == set(expectations), sorted(results)
    for call_id, (rust, _, count) in expectations.items():
        row = results[call_id]
        assert row["text"] == rust["text"], (call_id, row["text"])
        assert row.get("exit_code") == rust["exit_code"] and row["error"] is rust["error"], (call_id, row)
        assert abs(row["duration_s"] - rust["duration_s"]) < 1e-9, (call_id, row["duration_s"], rust["duration_s"])
        assert row["name"] == "exec" and row["turn_id"] == "chunks" and count >= 2, (call_id, row)
        assert CHUNKS_HEADER not in row["text"] and "--- 1 ---" not in row["text"] and "{}" not in row["text"], (call_id, row["text"])
    assert len(results["last-exit-image"].get("media") or []) == 1 and CHUNKS_PNG not in json.dumps(response), "trailing image part is media"
    assert "media" not in results["two"]
    if adapters:
        python = python_read(adapters, "codex", corpus.paths[CHUNKS_SID])
        python_results = {row["call_id"]: row for row in python[0] if row.get("role") == "tool_result"}
        for call_id, (_, expected, _) in expectations.items():
            assert python_results[call_id]["text"] == expected["text"], (call_id, "reference adapter changed its chunk choice", python_results[call_id]["text"])
        compare_view(report, f"codex {CHUNKS_SID} multi-chunk envelopes (batch 36)", python, response, classify=classify_codex_chunks(messages))
        report.delta(f"codex {CHUNKS_SID}: Python `_tool_output` shows one chunk per result (last chunk when the joined text ends with a chunk, "
                     f"first envelope part otherwise), Rust concatenates every chunk's output in part order with the last exit_code and summed duration_s")
    report.ok(f"codex {CHUNKS_SID}: {len(expectations)} multi-chunk results (2/3/5 chunks, exit_code only on the last, trailing empty/{{}}/image part, "
              "prints between chunks) show every chunk in order; header and prints never enter the text")


def build_corpus(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    build_claude(corpus)
    cuts = build_codex(corpus)
    cases = build_tools(corpus)
    build_codex_tools_extra(corpus)
    build_grok(corpus)
    write_batch35_shapes(corpus)
    build_claude_interrupts(corpus)
    build_codex_chunks(corpus)
    return corpus, cuts, cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python-source", type=Path, help="Python checkout for the adapter-only differential half")
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    report = Report()
    with tempfile.TemporaryDirectory(prefix="sessiondock-advanced-parity-") as temporary:
        corpus, cuts, cases = build_corpus(Path(temporary))
        before = {path: path.read_bytes() for path in corpus.root.rglob("*") if path.is_file()}
        adapters = None
        if args.python_source:
            codex_paths = {sid: path for sid, path in corpus.paths.items() if sid.startswith("codex-")}
            adapters = load_adapters(args.python_source, fixture_root=corpus.root, codex_paths=codex_paths)
        with isolated_server(corpus, args.binary) as (base, opener):
            listed = get_json(opener, base, "/api/sessions?force=1")
            rows = {row["sid"]: row for row in listed["sessions"]}
            expected_public = {"claude-chain", "claude-chain-esc", "claude-events", "claude-missing-parent", "claude-dupkey", "codex-root", "codex-l1",
                               "codex-l2", "codex-badcut", "codex-orphan-fork", "claude-tools", "codex-tools", "codex-tools-extra", "grok-tools",
                               "grok-chat", *BATCH35_CODEX_ROWS, *BATCH35_CLAUDE_ROWS, *BATCH36_SIDS, CHUNKS_SID}
            assert expected_public <= set(rows), sorted(expected_public - set(rows))
            assert not (set(rows) - expected_public - {"grok-summary-only"}), ("unexpected rows (orphan agents must not be listed)",
                                                                                sorted(set(rows) - expected_public - {"grok-summary-only"}))
            verify_claude(report, corpus, base, opener, adapters, rows)
            verify_codex(report, corpus, base, opener, adapters, rows, cuts)
            verify_tools(report, corpus, base, opener, adapters, cases)
            verify_codex_tools_extra(report, corpus, base, opener, adapters)
            verify_grok(report, corpus, base, opener, adapters, rows)
            verify_batch35(report, corpus, base, opener, adapters, rows)
            verify_claude_interrupts(report, corpus, base, opener, adapters, rows)
            verify_codex_chunks(report, corpus, base, opener, adapters, rows)
            report.ok("current-CLI kinds (batch 33): Claude control/attachment records, Codex telemetry/item records and unknown Grok "
                      "records are skipped like Python with counted migration_warnings; claude-events/codex-l2/grok-chat stay supported")
        assert all(path.read_bytes() == old for path, old in before.items()), "reads modified synthetic native history"
    print(f"SUMMARY {len(report.passes)} PASS, {len(report.deltas)} DELTA, {len(report.unverified_lines)} UNVERIFIED, {len(report.failures)} FAIL"
          + ("" if adapters else " (Rust-only run: pass --python-source ../agenthub for the differential half)"))
    if report.failures:
        raise SystemExit("Unexpected Rust/Python differences: " + "; ".join(report.failures[:20]))


if __name__ == "__main__":
    main()
