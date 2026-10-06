"""Shared fixture helpers extracted from the former `history_parity.py` (removed 2026-10-06
with the non-browser suites); imported by browser suites."""
# run_validation: skip
from __future__ import annotations
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
from urllib.error import URLError
from urllib.parse import urlencode
from urllib.request import ProxyHandler, build_opener
from http_fixtures import MAX_RESPONSE_BYTES, NoRedirects
from frontend_paths import frontend_dir


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
    """One crash-time torn JSONL line: a zero-filled block followed by
    the tail of a record and its newline. json.loads rejects it; Python skips it."""
    return b"\x00" * nul + encoded(record)[-tail:]


@dataclass
class Corpus:
    root: Path
    paths: dict[str, Path] = field(default_factory=dict)
    expected: dict[str, list[str]] = field(default_factory=dict)
    owners: dict[str, str] = field(default_factory=dict)
    # Native files that are never a list row (orphan agents).
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


# Shapes observed on the real read roots (795 rows) that the Python
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


# Esc-interrupted Claude turns stay visible (Python d16c5e1).
# Shapes only; every expectation was derived by running the Python adapter
# on these files (texts in file order, statuses excluded; `""` is the
# turn_duration event).
BATCH36_ROWS = ("claude-esc-replied", "claude-esc-id", "claude-esc-offshoot", "claude-esc-deferred")


CLAUDE_INTERRUPT_TEXT = "[Request interrupted by user]"


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
def isolated_server(corpus: Corpus, executable: Path = BINARY, *, state_dir: Path | None = None, file_roots: tuple[Path, ...] = (), file_write_roots: tuple[Path, ...] = (), host_dir: Path | None = None, lifecycle_dir: Path | None = None, launcher_config: Path | None = None, audit_dir: Path | None = None, trash_dir: Path | None = None, extra_env: dict[str, str] | None = None):
    executable = executable.resolve(strict=True)
    environment = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    environment.update({"SESSIONDOCK_BIND": f"127.0.0.1:{port}",
                        "SESSIONDOCK_WEB_DIR": str(frontend_dir())})
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
    for name, path in [("LIFECYCLE_DIR", lifecycle_dir), ("LAUNCHER_CONFIG", launcher_config), ("AUDIT_DIR", audit_dir), ("TRASH_DIR", trash_dir)]:
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
                    log.seek(0)
                    details = log.read(8192).decode("utf-8", "replace")
                    raise AssertionError(f"isolated Rust server exited early ({process.returncode}): {details}")
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


def cursor_query(snapshot, **extra):
    return urlencode({"start": snapshot["end"], "head": snapshot["version"]["head"],
                      "anchor": snapshot["anchor"], **extra})
