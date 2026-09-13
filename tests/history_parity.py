#!/usr/bin/env python3
"""Synthetic advanced-history acceptance and optional Python/Rust differential check.

Build the Rust server first. This script generates every native file in a new
temporary directory and launches only that server on loopback. It never scans
CLI homes, invokes a model, imports Python index/runtime state, or uses HTTP
proxies. --python-source explicitly opts into adapter-only differential checks.
history_browser.py reuses this same generated corpus and isolated server.
"""

from __future__ import annotations

import argparse
from contextlib import contextmanager
from dataclasses import dataclass, field
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import ProxyHandler, build_opener

from provider_parity import MAX_RESPONSE_BYTES, NoRedirects, api, compare, load_adapters, normalized


REPO = Path(__file__).resolve().parents[1]
BINARY = REPO / "target/debug" / ("sessiondock.exe" if os.name == "nt" else "sessiondock")


def encoded(record):
    return (json.dumps(record, ensure_ascii=False, separators=(",", ":")) + "\n").encode()


def claude_row(sid, kind, uid, parent, text="", **extra):
    row = {"type": kind, "uuid": uid, "parentUuid": parent, "sessionId": sid,
           "cwd": "/synthetic/history", "timestamp": "2026-09-11T10:00:00Z",
           "isSidechain": False, **extra}
    if kind in {"user", "assistant"}:
        row["message"] = {"role": kind, "content": text}
        if kind == "assistant":
            row["message"]["stop_reason"] = "end_turn"
    elif text:
        row["content"] = text
    return row


# Record/attachment kinds current CLI versions write (observed 2026-09-12 on
# Claude Code 2.1.269 / Codex rollouts; shapes only, no real data copied). The
# read model skips them like the Python adapters and reports counted warnings.
CLAUDE_ATTACHMENT_KINDS = (
    "hook_success", "environment", "model", "language", "deferred_tools_delta",
    "agent_listing_delta", "mcp_instructions_delta", "skill_listing", "auto_mode",
    "instructions", "session_context", "date", "remote_session_change", "prompt_snapshot",
    "deferred_tools_record", "edited_text_file", "diagnostics", "hook_blocking_error",
    "file", "task_status")


def claude_control_rows(sid):
    """Session-level records a Claude Code 2.1.x main transcript starts with."""
    return [
        {"type": "mode", "mode": "default", "sessionId": sid},
        {"type": "permission-mode", "permissionMode": "plan", "sessionId": sid},
        {"type": "bridge-session", "bridgeSessionId": f"bridge-{sid}", "lastSequenceNum": 3,
         "ownerAccountUuid": "synthetic-account", "ownerOrganizationUuid": "synthetic-org", "sessionId": sid},
        {"type": "agent-name", "agentName": "synthetic-agent", "sessionId": sid},
    ]


def claude_attachment_chain(sid, parent, prefix, kinds=CLAUDE_ATTACHMENT_KINDS, ts="2026-09-11T10:00:00Z"):
    """Attachment records chained by uuid/parentUuid after a user record; the
    reply's parentUuid must be the last attachment (returned as ``tip``)."""
    rows = []
    for index, kind in enumerate(kinds):
        uid = f"{prefix}-{index:02d}"
        rows.append({"type": "attachment", "uuid": uid, "parentUuid": parent, "sessionId": sid,
                     "cwd": "/synthetic/history", "timestamp": ts, "isSidechain": False,
                     "attachment": {"type": kind, "synthetic": True}})
        parent = uid
    return rows, parent


def claude_turn_tail_rows(sid, message_id, snapshot_id):
    """Records Claude Code appends after an assistant reply."""
    return [
        {"type": "atis-latch", "atis": {"latched": True}, "sessionId": sid},
        {"type": "file-history-delta", "backup": {"synthetic": True}, "messageId": message_id,
         "snapshotMessageId": snapshot_id, "timestamp": "2026-09-11T10:00:01Z",
         "trackingPath": "synthetic.txt"},
        {"type": "cost-state", "modelUsage": {"synthetic-model": {"inputTokens": 1, "outputTokens": 1}},
         "totalCostUSD": 0.0001, "sessionId": sid},
    ]


def codex_telemetry_rows(turn_id):
    """Rollout records current Codex versions interleave with a turn."""
    return [
        codex_row("token_usage_record", {"turn_id": turn_id, "total_tokens": 12}),
        codex_row("event_msg", {"type": "item_completed", "turn_id": turn_id,
                                "item": {"type": "agent_message", "synthetic": True}}),
        codex_row("response_item", {"type": "agent_message", "turn_id": turn_id,
                                    "content": "duplicate of the assistant message"}),
        codex_row("inter_agent_communication_metadata", {"turn_id": turn_id, "synthetic": True}),
    ]


def codex_row(kind, payload, ordinal=0):
    return {"type": kind, "payload": payload, "ordinal": ordinal,
            "timestamp": "2026-09-11T10:00:00Z"}


def codex_message(role, text, ordinal=1):
    return codex_row("response_item", {"type": "message", "role": role,
        "content": [{"type": "input_text" if role == "user" else "output_text", "text": text}]}, ordinal)


def torn_line(record, nul=4096, tail=48):
    """One crash-time torn JSONL line (batch 35): a zero-filled block followed by
    the tail of a record and its newline. json.loads rejects it; Python skips it."""
    return b"\x00" * nul + encoded(record)[-tail:]


def unreadable_claude_row(sid, uid, parent):
    """A Claude record neither implementation can read (scalar message.content):
    the canonical unsupported fixture since batch 33."""
    return {"type": "user", "uuid": uid, "parentUuid": parent, "sessionId": sid,
            "timestamp": "2026-09-11T10:00:00Z", "isSidechain": False,
            "message": {"role": "user", "content": 42}}


def unreadable_codex_row(ordinal=1):
    """A Codex message record neither implementation can read (scalar content)."""
    return codex_row("response_item", {"type": "message", "role": "user", "content": 42}, ordinal)


@dataclass
class Corpus:
    root: Path
    paths: dict[str, Path] = field(default_factory=dict)
    expected: dict[str, list[str]] = field(default_factory=dict)
    owners: dict[str, str] = field(default_factory=dict)
    # Native files that are never a list row (orphan agents, batch 35 R4).
    hidden: set[str] = field(default_factory=set)

    def put(self, sid, source, rows, expected, *, parent=None):
        if source == "claude":
            path = self.root / "claude/project-history" / f"{sid}.jsonl"
        else:
            path = self.root / "codex/2026/09/11" / f"rollout-{sid}.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        # A bytes element is written verbatim (torn/duplicate-key lines).
        path.write_bytes(b"".join(row if isinstance(row, bytes) else encoded(row) for row in rows))
        self.paths[sid] = path
        self.expected[sid] = expected
        if parent:
            self.owners[sid] = parent
        return path

    def uid(self, sid):
        path = self.paths[sid]
        source = "claude" if path.is_relative_to(self.root / "claude") else "codex"
        return source + ":" + hashlib.sha1(str(path).encode()).hexdigest()[:16]

    def view(self, sid):
        owner = self.owners.get(sid)
        return (self.uid(owner), urlencode({"agent": sid})) if owner else (self.uid(sid), "")


# Batch 35: shapes observed on the real read roots (795 rows) that the Python
# adapters read and the pre-batch-35 Rust backend rejected. Shapes only; every
# expectation below was derived by running the Python adapter on these files.
BATCH35_CODEX_ROWS = ("codex-legacy-root", "codex-legacy-mid", "codex-legacy-fork", "codex-legacy-orphan",
                      "codex-rewind-root", "codex-rewind-q", "codex-rewind-a")
BATCH35_CLAUDE_ROWS = ("claude-torn", "claude-lost-leaf", "claude-cycle")
BATCH35_ROWS = BATCH35_CODEX_ROWS + BATCH35_CLAUDE_ROWS
BATCH35_AGENTS = {"codex-copied-agent": "codex-legacy-root"}
BATCH35_HIDDEN = ("codex-copied-orphan-agent", "claude-orphan-agent")


def batch35_meta(sid, ts="2026-09-11T09:00:00Z", **extra):
    return codex_row("session_meta", {"id": sid, "session_id": sid, "timestamp": ts,
                                      "cwd": "/synthetic/history", **extra})


def batch35_agent_meta(sid, owner, ts="2026-09-11T09:30:00Z"):
    return codex_row("session_meta", {
        "id": sid, "session_id": owner, "timestamp": ts, "cwd": "/synthetic/history",
        "forked_from_id": owner, "thread_source": "subagent", "parent_thread_id": owner,
        "source": {"subagent": {"thread_spawn": {"parent_thread_id": owner,
                                                 "agent_path": "/root/" + sid, "agent_role": "reviewer"}}}})


def write_batch35_shapes(corpus: Corpus):
    """Write the nine batch-35 shapes into ``corpus``; returns the sids written.

    (a) legacy self-contained Codex fork (own meta with ``history_base`` null,
        copied ancestor metas two levels deep, copied history, own turn), parent
        present; (b) the same with the parent absent; (c) a subagent rollout with
        the parent's meta copied, parent present; (d) the same with the parent
        absent (never listed); (e) rewind past the parent's own fork point
        (``forked_from_id`` ≠ ``history_base.thread_id``); (f) a Claude sidecar
        without an owner file (never listed); (g) a Claude file with one torn
        NUL-filled line whose lost uuid a later record names as parent; (h) a
        Claude ``last-prompt`` leaf that is not in the graph; (i) a Claude
        parentUuid cycle.
    """
    written = []
    texts = lambda rows: [row["payload"]["content"][0]["text"] for row in rows]

    def crow(sid, kind, uid, parent, text="", **extra):
        # Older than every other synthetic Claude row: suites that open "the
        # first supported row" keep picking the ordinary sessions first.
        return claude_row(sid, kind, uid, parent, text, timestamp="2026-09-10T10:00:00Z", **extra)

    # (a) Old-style fork chain root → mid → fork; every child copies its
    # ancestors' session_meta records and the whole history it continues.
    root_turn = [codex_message("user", "Legacy root question", 1), codex_message("assistant", "Legacy root answer", 2)]
    mid_turn = [codex_message("user", "Legacy mid question", 3), codex_message("assistant", "Legacy mid answer", 4)]
    fork_turn = [codex_message("user", "Legacy fork question", 5), codex_message("assistant", "Legacy fork answer", 6)]
    root_meta = batch35_meta("codex-legacy-root", "2026-09-11T08:00:00Z")
    mid_meta = batch35_meta("codex-legacy-mid", "2026-09-11T08:30:00Z", forked_from_id="codex-legacy-root", history_base=None)
    fork_meta = batch35_meta("codex-legacy-fork", "2026-09-11T09:00:00Z", forked_from_id="codex-legacy-mid", history_base=None)
    corpus.put("codex-legacy-root", "codex", [root_meta, *root_turn], texts(root_turn))
    corpus.put("codex-legacy-mid", "codex", [mid_meta, root_meta, *root_turn, *mid_turn], texts(root_turn + mid_turn))
    corpus.put("codex-legacy-fork", "codex", [fork_meta, mid_meta, root_meta, *root_turn, *mid_turn, *fork_turn],
               texts(root_turn + mid_turn + fork_turn))
    # (b) The same shape whose ancestors were deleted from the root.
    gone_turn = [codex_message("user", "Legacy gone question", 1), codex_message("assistant", "Legacy gone answer", 2)]
    orphan_turn = [codex_message("user", "Legacy orphan question", 3), codex_message("assistant", "Legacy orphan answer", 4)]
    corpus.put("codex-legacy-orphan", "codex", [
        batch35_meta("codex-legacy-orphan", "2026-09-11T09:10:00Z", forked_from_id="codex-legacy-gone", history_base=None),
        batch35_meta("codex-legacy-gone", "2026-09-11T08:10:00Z", forked_from_id="codex-legacy-gone-root", history_base=None),
        batch35_meta("codex-legacy-gone-root", "2026-09-11T08:05:00Z"),
        *gone_turn, *orphan_turn], texts(gone_turn + orphan_turn))
    # (c)/(d) Subagent rollouts that copied the parent's session_meta.
    corpus.put("codex-copied-agent", "codex", [batch35_agent_meta("codex-copied-agent", "codex-legacy-root"), root_meta,
                                              codex_message("assistant", "Synthetic codex-copied-agent answer")],
               ["Synthetic codex-copied-agent answer"], parent="codex-legacy-root")
    corpus.put("codex-copied-orphan-agent", "codex", [
        batch35_agent_meta("codex-copied-orphan-agent", "codex-legacy-gone"),
        batch35_meta("codex-legacy-gone", "2026-09-11T08:10:00Z"),
        codex_message("assistant", "Synthetic codex-copied-orphan-agent answer")], [])
    corpus.hidden.add("codex-copied-orphan-agent")
    # (e) A rewinds past Q's own fork point: history_base names the physical
    # file R and an offset before Q's cut; forked_from_id names Q.
    rewind_head = [codex_message("user", "Rewind root question", 1), codex_message("assistant", "Rewind root answer", 2)]
    rewind_more = [codex_message("user", "Rewind root second question", 3), codex_message("assistant", "Rewind root second answer", 4)]
    rewind_root_meta = batch35_meta("codex-rewind-root", "2026-09-11T08:00:00Z")
    cut = sum(len(encoded(row)) for row in [rewind_root_meta, *rewind_head])
    cut2 = cut + sum(len(encoded(row)) for row in rewind_more)
    corpus.put("codex-rewind-root", "codex", [rewind_root_meta, *rewind_head, *rewind_more, codex_message("user", "Rewind root tail", 5)],
               texts(rewind_head + rewind_more) + ["Rewind root tail"])
    q_turn = [codex_message("user", "Rewind Q question", 1), codex_message("assistant", "Rewind Q answer", 2)]
    corpus.put("codex-rewind-q", "codex", [
        batch35_meta("codex-rewind-q", "2026-09-11T08:30:00Z", forked_from_id="codex-rewind-root", history_mode="paginated",
                     history_base={"thread_id": "codex-rewind-root", "end_byte_offset": cut2}), *q_turn],
        texts(rewind_head + rewind_more + q_turn))
    a_turn = [codex_message("user", "Rewind A question", 1), codex_message("assistant", "Rewind A answer", 2)]
    corpus.put("codex-rewind-a", "codex", [
        batch35_meta("codex-rewind-a", "2026-09-11T09:00:00Z", forked_from_id="codex-rewind-q", history_mode="paginated",
                     history_base={"thread_id": "codex-rewind-root", "end_byte_offset": cut}), *a_turn],
        texts(rewind_head + a_turn))
    # (f) A Claude sidecar whose owner transcript is gone.
    agent = "claude-orphan-agent"
    agent_path = corpus.root / "claude/project-history/claude-orphan-owner/subagents" / f"agent-{agent}.jsonl"
    agent_path.parent.mkdir(parents=True, exist_ok=True)
    agent_path.write_bytes(b"".join(encoded(row) for row in [
        crow("claude-orphan-owner", "user", "orphan-u", None, "Claude orphan agent question", isSidechain=True, agentId=agent),
        crow("claude-orphan-owner", "assistant", "orphan-a", "orphan-u", "Claude orphan agent answer", isSidechain=True, agentId=agent)]))
    agent_path.with_suffix(".meta.json").write_text(json.dumps({"description": "Synthetic orphan child", "agentType": "reviewer"}))
    corpus.paths[agent] = agent_path
    corpus.expected[agent] = []
    corpus.hidden.add(agent)
    # (g) A torn line replaces the record "torn-lost"; the next user record
    # still names it as parent, so the timeline is the part reachable from
    # the tip (Python _active_lineage stops at the unknown parent).
    sid = "claude-torn"
    corpus.put(sid, "claude", [
        crow(sid, "user", "u0", None, "Torn line root question"),
        crow(sid, "assistant", "a0", "u0", "Torn line root answer"),
        torn_line(crow(sid, "assistant", "torn-lost", "a0", "Torn line lost answer")),
        crow(sid, "user", "u1", "torn-lost", "Torn line question after the gap"),
        crow(sid, "assistant", "a1", "u1", "Torn line answer after the gap")],
        ["Torn line question after the gap", "Torn line answer after the gap"])
    # (h) last-prompt names a leaf that was never written: the active set is
    # exactly that uuid, so no graph record is on the timeline.
    sid = "claude-lost-leaf"
    corpus.put(sid, "claude", [
        crow(sid, "user", "u0", None, "Lost leaf question"),
        crow(sid, "assistant", "a0", "u0", "Lost leaf answer"),
        {"type": "last-prompt", "leafUuid": "never-written"}], [])
    # (i) u1 and a1 point at each other; the walk from the tip stops at the
    # first revisited uuid and the root turn is not on the timeline.
    sid = "claude-cycle"
    corpus.put(sid, "claude", [
        crow(sid, "user", "u0", None, "Cycle root question"),
        crow(sid, "assistant", "a0", "u0", "Cycle root answer"),
        crow(sid, "user", "u1", "a1", "Cycle question"),
        crow(sid, "assistant", "a1", "u1", "Cycle answer")],
        ["Cycle question", "Cycle answer"])
    written.extend(BATCH35_ROWS)
    written.extend(BATCH35_AGENTS)
    written.extend(BATCH35_HIDDEN)
    return written


# Batch 36 (WP-D): Esc-interrupted Claude turns stay visible (Python d16c5e1).
# Shapes only; every expectation was derived by running the Python adapter
# on these files (texts in file order, statuses excluded; `""` is the
# turn_duration event).
BATCH36_ROWS = ("claude-esc-replied", "claude-esc-id", "claude-esc-offshoot", "claude-esc-deferred")
CLAUDE_INTERRUPT_TEXT = "[Request interrupted by user]"
CLAUDE_INTERRUPT_REASON = "输入已中断，未进入当前 Claude 分支"


def claude_turn_duration(sid, uid, parent, duration_ms=1500):
    return {"type": "system", "subtype": "turn_duration", "uuid": uid, "parentUuid": parent, "sessionId": sid,
            "durationMs": duration_ms, "timestamp": "2026-09-10T10:00:01Z", "isSidechain": False}


def write_batch36_shapes(corpus: Corpus):
    """Write the four interrupted-turn shapes into ``corpus``; returns the sids.

    (a) the assistant already replied, Esc wrote the native interrupt record,
        the next input hangs off the previous ``turn_duration`` (the interrupted
        turn used to look like a completed old branch); (b) the same with the
        interrupt marked by ``interruptedMessageId`` only; (c) inputs typed
        during the interrupt below the interrupt record and below an attachment
        (two offshoot levels, each an abandoned input of its own); (d) an
        interrupted turn with a deferred ``aborted`` next to a plain unanswered
        sibling whose ``aborted`` follows it directly.
    """
    def crow(sid, kind, uid, parent, text="", **extra):
        return claude_row(sid, kind, uid, parent, text, timestamp="2026-09-10T10:00:00Z", **extra)

    def head(sid, label):
        return [crow(sid, "user", "u0", None, f"{label} root question"),
                crow(sid, "assistant", "a0", "u0", f"{label} root answer"),
                claude_turn_duration(sid, "t0", "a0"),
                crow(sid, "user", "u1", "t0", f"{label} cut-short question"),
                crow(sid, "assistant", "a1", "u1", f"{label} partial answer")]

    def tail(sid, label):
        return [crow(sid, "user", "u2", "t0", f"{label} replacement question"),
                crow(sid, "assistant", "a2", "u2", f"{label} replacement answer")]

    def texts(label, *middle):
        return [f"{label} root question", f"{label} root answer", "", f"{label} cut-short question", f"{label} partial answer",
                *middle, f"{label} replacement question", f"{label} replacement answer"]

    sid, label = "claude-esc-replied", "Esc replied"
    corpus.put(sid, "claude", [*head(sid, label), crow(sid, "user", "stop", "a1", CLAUDE_INTERRUPT_TEXT), *tail(sid, label)],
               texts(label))
    sid, label = "claude-esc-id", "Esc id"
    corpus.put(sid, "claude", [*head(sid, label), crow(sid, "user", "stop", "a1", "stopped", interruptedMessageId="msg_synthetic"),
                               *tail(sid, label)], texts(label))
    sid, label = "claude-esc-offshoot", "Esc offshoot"
    corpus.put(sid, "claude", [
        *head(sid, label), crow(sid, "user", "stop", "a1", CLAUDE_INTERRUPT_TEXT),
        crow(sid, "user", "u1b", "stop", f"{label} typed during the interrupt"),
        {"type": "attachment", "uuid": "att", "parentUuid": "u1b", "sessionId": sid, "isSidechain": False,
         "timestamp": "2026-09-10T10:00:00Z", "attachment": {"type": "total_tokens_reminder"}},
        crow(sid, "user", "u1c", "att", f"{label} typed again"), *tail(sid, label)],
        texts(label, f"{label} typed during the interrupt", f"{label} typed again"))
    sid, label = "claude-esc-deferred", "Esc deferred"
    corpus.put(sid, "claude", [
        *head(sid, label), crow(sid, "user", "stop", "a1", CLAUDE_INTERRUPT_TEXT), *tail(sid, label),
        crow(sid, "user", "u3", "a2", f"{label} second unanswered"),
        crow(sid, "user", "u4", "a2", f"{label} second replacement"),
        crow(sid, "assistant", "a4", "u4", f"{label} second final")],
        texts(label) + [f"{label} second unanswered", f"{label} second replacement", f"{label} second final"])
    return list(BATCH36_ROWS)


def build_corpus(root: Path) -> Corpus:
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)

    # last-prompt chooses the old leaf even though a completed sibling is later
    # on disk; compact reconnects only that selected pre-compact history.
    for sid, compact in (("claude-branch", None), ("claude-compact", "legacy"),
                         ("claude-current-compact", "current")):
        def row(kind, uid, parent, text="", **extra):
            return claude_row(sid, kind, uid, parent, text, **extra)
        rows = [row("user", "u0", None, "Claude common question"),
                row("assistant", "a0", "u0", "Claude selected answer"),
                row("user", "old-u", "a0", "Claude discarded completed question"),
                row("assistant", "old-a", "old-u", "Claude discarded completed answer"),
                {"type": "last-prompt", "leafUuid": "a0"}]
        texts = ["Claude common question", "Claude selected answer"]
        if compact:
            extra = {"subtype": "compact_boundary"} if compact == "legacy" else {
                "compactMetadata": {"trigger": "manual", "durationMs": 1200}}
            rows += [row("system", "compact", None, **extra),
                     row("user", "summary", "compact", "INTERNAL COMPACT SUMMARY", isCompactSummary=True),
                     row("user", "compact-u", "summary", "Claude post compact question"),
                     row("assistant", "compact-a", "compact-u", "Claude post compact answer")]
            texts += ["已压缩", "Claude post compact question", "Claude post compact answer"]
        corpus.put(sid, "claude", rows, texts)

    sid = "claude-abandoned"
    rows = [claude_row(sid, "user", "u0", None, "Claude abandoned common"),
            claude_row(sid, "assistant", "a0", "u0", "Claude abandoned base answer"),
            claude_row(sid, "user", "escaped", "a0", "Claude fast Esc input"),
            claude_row(sid, "user", "replacement", "a0", "Claude replacement input"),
            claude_row(sid, "assistant", "replacement-a", "replacement", "Claude replacement answer")]
    corpus.put(sid, "claude", rows, [row["message"]["content"] for row in rows])

    # Claude Code 2.1.x `-p` one-shot then `--resume`: queue operations, the
    # user record, an attachment chain, atis-latch, the reply whose parent is
    # the last attachment, then turn-tail records; a resumed second turn.
    sid = "claude-cli-current"
    rows = [
        {"type": "queue-operation", "operation": "enqueue", "content": "Claude one-shot prompt",
         "sessionId": sid, "timestamp": "2026-09-11T10:00:00Z"},
        {"type": "queue-operation", "operation": "popAll", "sessionId": sid, "timestamp": "2026-09-11T10:00:00Z"},
        *claude_control_rows(sid),
        claude_row(sid, "user", "u0", None, "Claude one-shot prompt"),
    ]
    chain, tip = claude_attachment_chain(sid, "u0", "at0")
    rows += chain
    rows += [{"type": "atis-latch", "atis": {"latched": True}, "sessionId": sid},
             claude_row(sid, "assistant", "a0", tip, "Claude one-shot answer"),
             *claude_turn_tail_rows(sid, "a0", "u0"),
             claude_row(sid, "user", "u1", "a0", "Claude resumed prompt")]
    chain, tip = claude_attachment_chain(sid, "u1", "at1", kinds=("environment", "hook_success", "environment"))
    rows += chain
    rows += [claude_row(sid, "assistant", "a1", tip, "Claude resumed answer"),
             *claude_turn_tail_rows(sid, "a1", "u1")]
    corpus.put(sid, "claude", rows, ["Claude one-shot prompt", "", "Claude one-shot prompt", "Claude one-shot answer",
                                     "Claude resumed prompt", "Claude resumed answer"])

    # Claude subagent files have their own lineage and are never top-level rows.
    agent = "claude-agent-one"
    agent_path = corpus.paths["claude-branch"].with_suffix("") / "subagents" / f"agent-{agent}.jsonl"
    agent_path.parent.mkdir(parents=True)
    agent_path.write_bytes(b"".join(encoded(row) for row in [
        claude_row("claude-branch", "user", "agent-u", None, "Claude agent question", isSidechain=True, agentId=agent),
        claude_row("claude-branch", "assistant", "agent-a", "agent-u", "Claude agent answer", isSidechain=True, agentId=agent)]))
    agent_path.with_suffix(".meta.json").write_text(json.dumps({"description": "Synthetic Claude child", "agentType": "reviewer"}))
    corpus.paths[agent] = agent_path
    corpus.expected[agent] = ["Claude agent question", "Claude agent answer"]
    corpus.owners[agent] = "claude-branch"

    def meta(sid, **extra):
        return codex_row("session_meta", {"id": sid, "session_id": sid,
            "timestamp": "2026-09-11T09:00:00Z", "cwd": "/synthetic/history", **extra})

    parent_rows = [meta("codex-parent"), codex_message("user", "Codex parent prefix OLD"),
                   codex_message("assistant", "Codex inherited answer", 2)]
    cutoff = sum(len(encoded(row)) for row in parent_rows)
    corpus.put("codex-parent", "codex", parent_rows + [codex_message("user", "Codex discarded parent tail", 3)],
               ["Codex parent prefix OLD", "Codex inherited answer", "Codex discarded parent tail"])
    child_rows = [meta("codex-fork", forked_from_id="codex-parent", history_mode="paginated",
                      history_base={"thread_id": "codex-parent", "end_byte_offset": cutoff}),
                  codex_message("user", "Codex fork question"),
                  codex_message("assistant", "Codex fork answer", 2)]
    child = corpus.put("codex-fork", "codex", child_rows,
                       ["Codex parent prefix OLD", "Codex inherited answer", "Codex fork question", "Codex fork answer"])
    corpus.put("codex-grandchild", "codex", [meta("codex-grandchild", forked_from_id="codex-fork",
        history_base={"thread_id": "codex-fork", "end_byte_offset": child.stat().st_size}),
        codex_message("assistant", "Codex nested fork answer")],
        corpus.expected["codex-fork"] + ["Codex nested fork answer"])

    # Current Codex rollout: telemetry/item records and an agent_message item
    # around an ordinary turn.
    corpus.put("codex-cli-current", "codex", [
        meta("codex-cli-current"),
        codex_row("event_msg", {"type": "task_started", "turn_id": "t1"}),
        codex_row("response_item", {"type": "message", "role": "user", "turn_id": "t1",
                                    "content": [{"type": "input_text", "text": "Codex current question"}]}),
        *codex_telemetry_rows("t1"),
        codex_row("response_item", {"type": "message", "role": "assistant", "phase": "final_answer", "turn_id": "t1",
                                    "content": [{"type": "output_text", "text": "Codex current answer"}]}),
        codex_row("event_msg", {"type": "task_complete", "turn_id": "t1", "duration_ms": 5}),
        codex_row("token_usage_record", {"turn_id": "t1", "total_tokens": 20}),
    ], ["Codex current question", "Codex current answer"])

    for sid, owner, attached in (("codex-agent", "codex-parent", True),
                                ("codex-nested-agent", "codex-agent", True),
                                ("codex-orphan-agent", "codex-missing", False),
                                ("codex-cycle-a", "codex-cycle-b", False),
                                ("codex-cycle-b", "codex-cycle-a", False)):
        corpus.put(sid, "codex", [meta(sid, session_id="codex-parent", forked_from_id=owner,
            thread_source="subagent", parent_thread_id=owner,
            source={"subagent": {"thread_spawn": {"parent_thread_id": owner,
                "agent_path": "/root/" + sid, "agent_role": "reviewer"}}}),
            codex_message("assistant", "Synthetic " + sid + " answer")],
            ["Synthetic " + sid + " answer"], parent="codex-parent" if attached else None)
    corpus.hidden.add("codex-orphan-agent")
    write_batch35_shapes(corpus)
    write_batch36_shapes(corpus)
    return corpus


@contextmanager
def isolated_server(corpus: Corpus, executable: Path = BINARY, *, state_dir: Path | None = None, file_roots: tuple[Path, ...] = (), file_write_roots: tuple[Path, ...] = (), host_dir: Path | None = None, lifecycle_dir: Path | None = None, launcher_config: Path | None = None, audit_dir: Path | None = None, trash_dir: Path | None = None, delivery_dir: Path | None = None, extra_env: dict[str, str] | None = None):
    executable = executable.resolve(strict=True)
    environment = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    environment.update({"SESSIONDOCK_BIND": f"127.0.0.1:{port}",
                        "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web")})
    if state_dir is not None:
        assert state_dir.resolve().is_relative_to(corpus.root.resolve())
        environment["SESSIONDOCK_STATE_DIR"] = str(state_dir)
    if file_roots:
        assert all(path.resolve().is_relative_to(corpus.root.resolve()) for path in file_roots)
        environment["SESSIONDOCK_FILE_ROOTS"] = os.pathsep.join(str(path) for path in file_roots)
    if file_write_roots:
        assert all(path.resolve().is_relative_to(corpus.root.resolve()) for path in file_write_roots)
        environment["SESSIONDOCK_FILE_WRITE_ROOTS"] = os.pathsep.join(str(path) for path in file_write_roots)
    if host_dir is not None:
        assert host_dir.resolve().is_relative_to(corpus.root.resolve())
        environment["SESSIONDOCK_PTYHOST_DIR"] = str(host_dir)
    for name, path in [("LIFECYCLE_DIR", lifecycle_dir), ("LAUNCHER_CONFIG", launcher_config), ("AUDIT_DIR", audit_dir), ("TRASH_DIR", trash_dir), ("DELIVERY_DIR", delivery_dir)]:
        if path is not None:
            assert path.resolve().is_relative_to(corpus.root.resolve())
            environment["SESSIONDOCK_" + name] = str(path)
    for source in ("claude", "codex", "grok"):
        environment["SESSIONDOCK_" + source.upper() + "_ROOT"] = str(corpus.root / source)
    # Explicit SESSIONDOCK_* knobs a suite wants (the caller's own environment is stripped above).
    for key, value in (extra_env or {}).items():
        assert key.startswith("SESSIONDOCK_"), key
        environment[key] = value
    opener = build_opener(ProxyHandler({}), NoRedirects())
    # Logs belong to this invocation; don't retain an unbounded PIPE or block
    # the child when diagnostics exceed a pipe's capacity.
    with tempfile.TemporaryFile(mode="w+b") as log:
        process = subprocess.Popen([str(executable)], cwd=REPO, env=environment, stdout=log, stderr=log)
        try:
            for _ in range(150):
                if process.poll() is not None:
                    raise AssertionError(f"isolated Rust server exited early ({process.returncode})")
                try:
                    get_json(opener, base, "/api/health")
                    break
                except (OSError, URLError):
                    time.sleep(0.05)
            else:
                raise AssertionError("isolated Rust health check timed out")
            yield base, opener
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()  # Only the exact child launched by this context.
                    process.wait(timeout=5)
                    raise AssertionError("isolated Rust server did not shut down within five seconds")


def get_json(opener, base, route):
    assert route.startswith("/api/") and not route.startswith("//")
    with opener.open(base + route, timeout=10) as response:
        raw = response.read(MAX_RESPONSE_BYTES + 1)
        assert len(raw) <= MAX_RESPONSE_BYTES, "oversized synthetic response"
        return json.loads(raw)


def open_supported(opener, base, sid, uid, query=""):
    """GET /api/messages for a session that must be readable; an HTTP error
    becomes an assertion naming the session and the typed body."""
    try:
        return api(opener, base, uid, query)
    except HTTPError as error:
        raise AssertionError(f"{sid}: HTTP {error.code} {error.read(4096).decode('utf-8', 'replace')[:300]}") from None


def cursor_query(snapshot, **extra):
    return urlencode({"start": snapshot["end"], "head": snapshot["version"]["head"],
                      "anchor": snapshot["anchor"], **extra})


def visible(response):
    return [row["text"] for row in response["messages"]]


def verify(corpus, base, opener, python_source=None):
    public_sids = {"claude-branch", "claude-compact", "claude-current-compact", "claude-abandoned",
                   "claude-cli-current", "codex-parent", "codex-fork", "codex-grandchild", "codex-cli-current",
                   *BATCH35_ROWS, *BATCH36_ROWS}
    # Ownership cycles stay visible as unsupported rows (batch 35 R4 KEEP).
    unsupported_agents = {"codex-cycle-a", "codex-cycle-b"}
    # Agents whose owner is not in the root are not rows at all (Python
    # behaviour, batch 35 R4); their physical UID still answers a typed 501.
    hidden_agents = {"codex-orphan-agent", *BATCH35_HIDDEN}
    assert hidden_agents == corpus.hidden, sorted(corpus.hidden)
    listed = get_json(opener, base, "/api/sessions?force=1")
    rows = {row["sid"]: row for row in listed["sessions"]}
    assert set(rows) == public_sids | unsupported_agents, ("synthetic public topology", sorted(set(rows) ^ (public_sids | unsupported_agents)))
    for sid in unsupported_agents:
        assert rows[sid].get("supported") is False and rows[sid].get("migration_warnings"), (sid, "explicit unsupported topology")
        try:
            api(opener, base, corpus.uid(sid))
        except HTTPError as error:
            assert error.code == 501, (sid, error.code)
        else:
            raise AssertionError("unsupported native topology returned fake supported history")
    for sid in sorted(hidden_agents):
        assert sid not in rows, (sid, "orphan agent must not be a list row")
        try:
            api(opener, base, corpus.uid(sid))
        except HTTPError as error:
            assert error.code == 501, (sid, "orphan agent UID", error.code)
            body = json.loads(error.read(65536))
            assert body.get("error") and body.get("code"), (sid, "typed 501 body", body)
        else:
            raise AssertionError("orphan agent UID returned fake supported history")
    # Batch 35: real-root shapes Python reads. Identity is the first
    # session_meta (later copies are counted, not fatal); a null history_base
    # fork is self-contained; a torn line / broken lineage is a warning.
    for sid, expected_warnings in (
            ("codex-legacy-root", []),
            ("codex-legacy-mid", ["跳过重复的Codex session_meta ×1"]),
            ("codex-legacy-fork", ["跳过重复的Codex session_meta ×2"]),
            ("codex-legacy-orphan", ["跳过重复的Codex session_meta ×2"]),
            ("codex-rewind-root", []), ("codex-rewind-q", []), ("codex-rewind-a", []),
            ("claude-torn", ["跳过无效的JSONL 记录 ×1"]),
            ("claude-lost-leaf", []), ("claude-cycle", [])):
        assert rows[sid].get("supported") is True, (sid, rows[sid].get("migration_warnings"))
        # Batch 44 WP-C: supported rows carry no migration_warnings (Python has
        # none); the notes are asserted on the detail meta below.
        assert "migration_warnings" not in rows[sid], (sid, rows[sid].get("migration_warnings"))
        del expected_warnings
        assert "cursor" in rows[sid], (sid, "supported rows carry a list cursor")
    detail = open_supported(opener, base, "codex-copied-agent", corpus.uid("codex-legacy-root"), urlencode({"agent": "codex-copied-agent"}))
    assert detail["meta"].get("supported") is True and detail["meta"].get("migration_warnings") == ["跳过重复的Codex session_meta ×1"], detail["meta"]
    for sid, expected_warnings in (
            ("codex-legacy-mid", ["跳过重复的Codex session_meta ×1"]),
            ("codex-legacy-fork", ["跳过重复的Codex session_meta ×2"]),
            ("codex-legacy-orphan", ["跳过重复的Codex session_meta ×2"]),
            ("codex-rewind-a", []),
            ("claude-torn", ["跳过无效的JSONL 记录 ×1", "Claude 祖先链在 torn-lost 处中断，之前的记录不在当前时间线"]),
            ("claude-lost-leaf", ["Claude 声明的叶子 never-written 不在记录中"])):
        detail = open_supported(opener, base, sid, corpus.uid(sid))
        assert detail["meta"].get("supported") is True, (sid, detail["meta"].get("migration_warnings"))
        assert sorted(detail["meta"].get("migration_warnings") or []) == sorted(expected_warnings), (sid, detail["meta"].get("migration_warnings"))
    detail = open_supported(opener, base, "claude-cycle", corpus.uid("claude-cycle"))
    cycle_warnings = detail["meta"].get("migration_warnings")
    assert detail["meta"].get("supported") is True and cycle_warnings in (
        [f"Claude 祖先链存在循环，已在 {uid} 处截断"] for uid in ("a1", "u1")), cycle_warnings
    for sid, root_sid, depth in (("codex-legacy-mid", "codex-legacy-root", 1), ("codex-legacy-fork", "codex-legacy-root", 2),
                                 ("codex-rewind-q", "codex-rewind-root", 1), ("codex-rewind-a", "codex-rewind-root", 2)):
        assert rows[sid].get("root_sid") == root_sid and rows[sid].get("fork_depth") == depth, (sid, rows[sid].get("root_sid"), rows[sid].get("fork_depth"))
        assert rows[sid]["created"] == rows[root_sid]["created"] and rows[sid]["title"] == rows[root_sid]["title"], sid
    assert rows["codex-legacy-orphan"].get("root_sid") is None and rows["codex-legacy-orphan"].get("fork_depth") is None, rows["codex-legacy-orphan"]
    assert rows["codex-legacy-orphan"]["forked_from_id"] == "codex-legacy-gone"
    assert {item["id"] for item in rows["codex-legacy-root"]["agent_items"]} == {"codex-copied-agent"}, rows["codex-legacy-root"].get("agent_items")
    assert not rows["codex-legacy-fork"].get("agent_items") and not rows["codex-legacy-orphan"].get("agent_items")
    # Batch 33: unknown record/attachment kinds of current CLI versions are
    # skipped like Python; the row stays supported and counts what it skipped.
    for sid, expected_warnings in (
            ("claude-cli-current", [
                "跳过未知的Claude 记录类型：mode ×1", "跳过未知的Claude 记录类型：permission-mode ×1",
                "跳过未知的Claude 记录类型：bridge-session ×1", "跳过未知的Claude 记录类型：agent-name ×1",
                *(f"跳过未知的Claude attachment 类型：{kind} ×{3 if kind == 'environment' else 2 if kind == 'hook_success' else 1}"
                  for kind in CLAUDE_ATTACHMENT_KINDS),
                "跳过未知的Claude 记录类型：atis-latch ×3", "跳过未知的Claude 记录类型：file-history-delta ×2",
                "跳过未知的Claude 记录类型：cost-state ×2"]),
            ("codex-cli-current", [
                "跳过未知的Codex 记录类型：token_usage_record ×2", "跳过未知的Codex event_msg：item_completed ×1",
                "跳过未知的Codex response_item：agent_message ×1",
                "跳过未知的Codex 记录类型：inter_agent_communication_metadata ×1"])):
        assert rows[sid].get("supported") is True, (sid, rows[sid].get("migration_warnings"))
        assert "migration_warnings" not in rows[sid], (sid, rows[sid].get("migration_warnings"))
        assert "cursor" in rows[sid], (sid, "supported rows carry a list cursor")
        detail = api(opener, base, corpus.uid(sid))
        assert detail["meta"].get("supported") is True and detail["meta"].get("migration_warnings") == expected_warnings, sid
    assert {item["id"] for item in rows["codex-parent"]["agent_items"]} == {"codex-agent", "codex-nested-agent"}
    assert not rows["codex-fork"].get("agent_items"), "fork must not inherit a parent's subagent ownership"
    assert {item["id"] for item in rows["claude-branch"]["agent_items"]} == {"claude-agent-one"}
    for sid, depth in (("codex-fork", 1), ("codex-grandchild", 2)):
        assert rows[sid]["root_sid"] == "codex-parent"
        assert rows[sid]["fork_depth"] == depth
        assert rows[sid]["created"] == rows["codex-parent"]["created"]

    # Batch 36: the Esc-interrupted turn is visible with `interrupted` on its
    # input only, keeps its own turn id, and the `aborted` of a turn with
    # native records below it is the interrupt record's (activity stays the
    # replacement's `working`). Expectations from the Python adapter.
    for sid in BATCH36_ROWS:
        assert rows[sid].get("supported") is True and "migration_warnings" not in rows[sid], (sid, rows[sid].get("migration_warnings"))
        detail = open_supported(opener, base, sid, corpus.uid(sid))
        assert detail["meta"].get("supported") is True and detail["meta"].get("migration_warnings") == [], (sid, detail["meta"])
        assert visible(detail) == corpus.expected[sid], (sid, visible(detail))
        by_text = {row["text"]: row for row in detail["messages"] if row["text"]}
        label = corpus.expected[sid][0].removesuffix(" root question")
        cut = by_text[f"{label} cut-short question"]
        assert cut.get("interrupted") is True and cut.get("interrupt_reason") == CLAUDE_INTERRUPT_REASON and cut.get("turn_id") == "u1", (sid, cut)
        partial = by_text[f"{label} partial answer"]
        assert partial.get("interrupted") is None and partial.get("turn_id") == "u1", (sid, partial)
        assert by_text[f"{label} replacement question"].get("turn_id") == "u2", sid
        assert "stopped" not in by_text and CLAUDE_INTERRUPT_TEXT not in by_text, (sid, "the interrupt record is a status, never a bubble")
        interrupted = [row["text"] for row in detail["messages"] if row.get("interrupted")]
        expected_interrupted = {
            "claude-esc-replied": [f"{label} cut-short question"],
            "claude-esc-id": [f"{label} cut-short question"],
            "claude-esc-offshoot": [f"{label} cut-short question", f"{label} typed during the interrupt", f"{label} typed again"],
            "claude-esc-deferred": [f"{label} cut-short question", f"{label} second unanswered"],
        }[sid]
        assert interrupted == expected_interrupted, (sid, interrupted)
        assert detail["activity"]["state"] == "working", (sid, detail["activity"])
        assert detail["activity"]["turn_id"] == ("u4" if sid == "claude-esc-deferred" else "u2"), (sid, detail["activity"])
    offshoot = open_supported(opener, base, "claude-esc-offshoot", corpus.uid("claude-esc-offshoot"))
    turns = {row["text"]: row.get("turn_id") for row in offshoot["messages"] if row.get("interrupted")}
    assert turns == {"Esc offshoot cut-short question": "u1", "Esc offshoot typed during the interrupt": "u1b",
                     "Esc offshoot typed again": "u1c"}, turns

    adapters = None
    if python_source:
        codex_paths = {sid: path for sid, path in corpus.paths.items() if sid.startswith("codex-")}
        adapters = load_adapters(python_source, fixture_root=corpus.root, codex_paths=codex_paths)
        native_rows = [adapters["codex"].session_meta(path) for path in codex_paths.values()]
        native_rows = {row["sid"]: row for row in adapters["codex"].finalize_sessions(native_rows)}
        assert set(native_rows) == {sid for sid in public_sids if sid.startswith("codex-")}
        for sid, native in native_rows.items():
            for key in ("root_sid", "fork_depth", "size", "forked_from_id", "created", "title"):
                expected, actual = native.get(key), rows[sid].get(key)
                if key == "created":
                    expected = normalized({"ts": expected}).get("ts")
                    actual = normalized({"ts": actual}).get("ts")
                assert expected == actual, (sid, "metadata", key)
            assert {item["id"] for item in native.get("agent_items", [])} == {
                item["id"] for item in rows[sid].get("agent_items", [])}, (sid, "agent topology")

    issues = []
    for sid in sorted(public_sids | corpus.owners.keys()):
        uid, query = corpus.view(sid)
        response = open_supported(opener, base, sid, uid, query)
        assert response["meta"]["sid"] == sid, (sid, "native SID")
        assert visible(response) == corpus.expected[sid], (sid, "visible history")
        assert response["end"] == corpus.paths[sid].stat().st_size, (sid, "leaf-local cursor")
        if sid == "claude-abandoned":
            escaped = next(row for row in response["messages"] if row["text"] == "Claude fast Esc input")
            assert escaped.get("interrupted") is True and escaped.get("interrupt_reason")
        if "compact" in sid:
            events = [row for row in response["messages"] if row.get("event_kind") == "compact"]
            assert len(events) == 1 and events[0].get("counted") is False
        if adapters:
            source = uid.split(":", 1)[0]
            read_kwargs = {"agent": sid} if source == "claude" and sid in corpus.owners else {}
            issues.extend(compare(sid, corpus.paths[sid], adapters[source], response, read_kwargs=read_kwargs))
    assert not issues, "\n".join(issues)

    for sid in corpus.owners:
        try:
            api(opener, base, corpus.uid(sid))
        except HTTPError as error:
            assert error.code == 404, (sid, "hidden physical agent UID", error.code)
        else:
            raise AssertionError("attached agent bypassed its owner-bound view selector")

    for owner, agent in (("codex-fork", "codex-agent"), ("codex-parent", "codex-orphan-agent"),
                         ("codex-parent", "codex-cycle-a"), ("claude-compact", "claude-agent-one")):
        try:
            api(opener, base, corpus.uid(owner), urlencode({"agent": agent}))
        except HTTPError as error:
            assert error.code == 404, (owner, agent, error.code)
        else:
            raise AssertionError("cross-owner/orphan/cycle agent access succeeded")

    # Menus depend on siblings as well as the selected transcript. A refresh
    # must invalidate already-cached main AND subagent detail metadata even
    # when neither selected native file changed.
    owner_uid = corpus.uid("codex-parent")
    agent_query = urlencode({"agent": "codex-agent"})
    api(opener, base, owner_uid)
    api(opener, base, owner_uid, agent_query)
    late = corpus.put("codex-late-agent", "codex", [codex_row("session_meta", {
        "id": "codex-late-agent", "session_id": "codex-parent",
        "timestamp": "2026-09-11T09:00:01Z", "cwd": "/synthetic/history",
        "thread_source": "subagent", "parent_thread_id": "codex-parent",
        "agent_path": "Synthetic late agent"}),
        codex_message("assistant", "Synthetic late agent answer")],
        ["Synthetic late agent answer"], parent="codex-parent")
    get_json(opener, base, "/api/sessions?force=1")
    for query in ("", agent_query):
        current = api(opener, base, owner_uid, query)
        assert {item["id"] for item in current["meta"]["agent_items"]} == {
            "codex-agent", "codex-nested-agent", "codex-late-agent"}, "cached detail lost new sibling menu entry"
    late.write_bytes(late.read_bytes().replace(b'"Synthetic late agent"', b'"Synthetic renamed agent"'))
    get_json(opener, base, "/api/sessions?force=1")
    for query in ("", agent_query):
        current = api(opener, base, owner_uid, query)
        item = next(item for item in current["meta"]["agent_items"] if item["id"] == "codex-late-agent")
        assert item["title"] == "Synthetic renamed agent", "cached sibling menu title did not refresh"
    late.unlink()  # Only the synthetic fixture created above in this invocation.
    get_json(opener, base, "/api/sessions?force=1")
    for query in ("", agent_query):
        current = api(opener, base, owner_uid, query)
        assert {item["id"] for item in current["meta"]["agent_items"]} == {
            "codex-agent", "codex-nested-agent"}, "cached detail retained deleted sibling menu entry"

    # Changing an ancestor without changing byte length must invalidate an
    # already-issued leaf cursor. Parent files' old tails must remain excluded.
    uid = corpus.uid("codex-fork")
    before = api(opener, base, uid)
    path = corpus.paths["codex-parent"]
    original = path.read_bytes()
    changed = original.replace(b"Codex parent prefix OLD", b"Codex parent prefix NEW")
    assert changed != original and len(changed) == len(original)
    path.write_bytes(changed)
    after = api(opener, base, uid, cursor_query(before))
    assert after["reset"] is True, "same-length inherited prefix rewrite needs reset"
    assert visible(after) == [text.replace("prefix OLD", "prefix NEW") for text in corpus.expected["codex-fork"]]
    with corpus.paths["codex-fork"].open("ab") as stream:
        stream.write(encoded(codex_message("assistant", "Codex incremental leaf answer", 3)))
    delta = api(opener, base, uid, cursor_query(after))
    assert delta["reset"] is False and visible(delta) == ["Codex incremental leaf answer"]
    # Records after a fork's fixed cutoff belong only to the parent view. An
    # unknown record kind there is skipped with a warning (batch 33, like
    # Python), so is a duplicate session_meta (batch 35 R1: the first one is
    # the identity, later copies are counted), and a hard failure (scalar
    # content, which Python cannot read either) makes only the parent
    # unsupported; none may poison the valid inherited prefix or change its
    # identity and cause a spurious reset in the child.
    with path.open("ab") as stream:
        stream.write(encoded(codex_row("unknown_synthetic_history_record", {})))
    parent_view = api(opener, base, corpus.uid("codex-parent"))
    assert parent_view["meta"].get("supported") is True
    assert parent_view["meta"].get("migration_warnings") == ["跳过未知的Codex 记录类型：unknown_synthetic_history_record ×1"], parent_view["meta"].get("migration_warnings")
    unchanged = api(opener, base, uid, cursor_query(delta))
    assert unchanged["reset"] is False and visible(unchanged) == [], "cutoff-external unknown tail poisoned inherited history"
    with path.open("ab") as stream:
        stream.write(encoded(codex_row("session_meta", {"id": "codex-parent", "session_id": "codex-parent"})))
    parent_view = open_supported(opener, base, "codex-parent (appended duplicate session_meta)", corpus.uid("codex-parent"))
    assert parent_view["meta"].get("supported") is True and parent_view["meta"]["sid"] == "codex-parent"
    assert sorted(parent_view["meta"].get("migration_warnings") or []) == [
        "跳过未知的Codex 记录类型：unknown_synthetic_history_record ×1", "跳过重复的Codex session_meta ×1"], parent_view["meta"].get("migration_warnings")
    assert visible(parent_view) == [text.replace("prefix OLD", "prefix NEW") for text in corpus.expected["codex-parent"]], visible(parent_view)
    unchanged = api(opener, base, uid, cursor_query(unchanged))
    assert unchanged["reset"] is False and visible(unchanged) == [], "cutoff-external duplicate session_meta poisoned inherited history"
    with path.open("ab") as stream:
        stream.write(encoded(unreadable_codex_row(4)))
    if adapters:
        try:
            adapters["codex"].read(str(path))
        except TypeError:
            pass
        else:
            raise AssertionError("the Python adapter read the scalar-content Codex record; pick a shape it cannot read")
    try:
        api(opener, base, corpus.uid("codex-parent"))
    except HTTPError as error:
        assert error.code == 501, ("parent scalar content", error.code)
    else:
        raise AssertionError("parent scalar content returned fake supported history")
    unchanged = api(opener, base, uid, cursor_query(unchanged))
    assert unchanged["reset"] is False and visible(unchanged) == [], "cutoff-external tail poisoned inherited history"
    print("PASS advanced synthetic history: selected Claude branches/compact/abandoned, Codex inherited prefixes/forks/subagents, rewrite reset and leaf-only delta")
    print("PASS current-CLI record kinds: Claude mode/permission-mode/bridge-session/agent-name/atis-latch/file-history-delta/cost-state + 20 attachment kinds and Codex token_usage_record/inter_agent_communication_metadata/item_completed/agent_message are skipped with counted warnings, sessions stay supported")
    print("PASS batch-35 real-root shapes: first session_meta is the Codex identity (copies counted), null history_base forks are self-contained, "
          "rewind past the parent's fork point reads history_base.thread_id and decorates along forked_from_id, orphan agents are not rows (typed 501), "
          "torn lines / broken Claude lineage are warnings, ownership cycles stay visible unsupported", flush=True)
    print("PASS batch-36 Claude interrupted turns: an Esc after the assistant replied keeps the whole turn visible "
          "(input interrupted:true, own turn id, aborted from the native interrupt record), interruptedMessageId marks the interrupt, "
          "inputs typed during the interrupt are abandoned at every level, a plain unanswered sibling still aborts right after itself", flush=True)
    if adapters:
        print("PASS explicit Python adapter differential: advanced message/activity/count/cursor subset and Codex topology metadata (incl. batch-35 rows)")
        print("Intentional safety delta: cyclic agent ownership stays visible as unsupported (HTTP 501) instead of Python's silent hiding; orphan agents are hidden like Python")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python-source", type=Path)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-history-") as temporary:
        corpus = build_corpus(Path(temporary))
        with isolated_server(corpus, args.binary) as (base, opener):
            verify(corpus, base, opener, args.python_source)


if __name__ == "__main__":
    main()
