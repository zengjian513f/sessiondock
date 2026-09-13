#!/usr/bin/env python3
"""Python-oracle parity for Claude interrupted-turn rules (pyhead d16c5e1).

Ports pyhead tests/test_adapters.py `test_interrupted_sibling_with_tools_stays_visible`
and `test_unanswered_sibling_branch_remains_visible_as_aborted` (fast Esc), plus a
two-level offshoot (abandoned user → assistant → tool → tool_result → unanswered
user) and a deferred abort (abandoned user whose descendants include the native
interrupt record). Asserts the full (role, text, turn_id, interrupted,
interrupt_reason) sequence of `ClaudeAdapter.read` — status rows excluded (they are Rust `activity`), same
call as tests/advanced_parity.py `python_read` — against GET /api/messages/{uid}
(non-status messages plus the final activity). An Esc interrupt (`interruptedMessageId`
or text `[Request interrupted by user]` / `[Request interrupted by user for tool
use]`) marks the nearest user ancestor a later prompt rewound past as
abandoned-but-visible: `interrupted:true`, `interrupt_reason:"输入已中断，未进入当前
Claude 分支"`, `turn_id` = that user's uuid; offshoot descendants stay visible;
an `aborted` status follows unless deferred to the native interrupt record.
"""
from __future__ import annotations

import argparse
import sys
import tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote

sys.dont_write_bytecode = True

from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, REPO, Corpus, get_json, isolated_server)
from provider_parity import load_adapters  # noqa: E402

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PYTHON_SOURCE = REPO.parent / "sessiondock"
CASES = ("interrupted-sibling", "fast-escape", "two-level-offshoot", "deferred-abort")


def fail(area, why):
    raise SystemExit(f"FAIL {area}: {why}")


def passed(area):
    print(f"PASS {area}", flush=True)


def rec(sid, kind, uid, parent, text="", **extra):
    row = {"type": kind, "uuid": uid, "parentUuid": parent, "sessionId": sid,
           "isSidechain": False, "timestamp": "2026-09-12T12:00:00Z", **extra}
    if kind in {"user", "assistant"} and "message" not in extra:
        row["message"] = {"role": kind, "content": text}
    return row


def tool_assistant(sid, uid, parent, text, call, name, payload):
    return rec(sid, "assistant", uid, parent, message={
        "role": "assistant", "content": [
            {"type": "text", "text": text},
            {"type": "tool_use", "id": call, "name": name, "input": payload}]})


def tool_result(sid, uid, parent, call, text):
    return rec(sid, "user", uid, parent, message={
        "role": "user", "content": [
            {"type": "tool_result", "tool_use_id": call, "content": text}]})


def interrupt_row(sid, uid, parent, **extra):
    return rec(sid, "user", uid, parent, "[Request interrupted by user]", **extra)


def build(root: Path) -> Corpus:
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)
    sid = "interrupted-sibling"
    corpus.put(sid, "claude", [
        rec(sid, "user", "u0", None, "共同开头"),
        rec(sid, "assistant", "a0", "u0", "全部搜了一遍"),
        rec(sid, "system", "t0", "a0", "", subtype="turn_duration"),
        rec(sid, "user", "u-work", "t0", "第一条，明显前后矛盾"),
        rec(sid, "assistant", "a-work", "u-work", "开始核对文档"),
        interrupt_row(sid, "interrupt", "a-work"),
        rec(sid, "user", "u-replace", "t0", "原来写需要授权"),
        rec(sid, "assistant", "a-replace", "u-replace", "已改正")], [])
    sid = "fast-escape"
    corpus.put(sid, "claude", [
        rec(sid, "user", "u0", None, "共同开头", timestamp="2026-08-27T17:26:23Z"),
        rec(sid, "assistant", "a0", "u0", "共同回答", timestamp="2026-08-27T17:26:23Z"),
        rec(sid, "user", "cancelled", "a0", "快速 Esc 的输入", timestamp="2026-08-27T17:26:23Z"),
        {"type": "attachment", "uuid": "reminder", "parentUuid": "cancelled",
         "sessionId": sid, "isSidechain": False,
         "attachment": {"type": "total_tokens_reminder"}},
        rec(sid, "user", "replacement", "a0", "之后的新输入", timestamp="2026-08-27T17:26:23Z"),
        rec(sid, "assistant", "answer", "replacement", "新回答",
            timestamp="2026-08-27T17:26:23Z")], [])
    sid = "two-level-offshoot"
    corpus.put(sid, "claude", [
        rec(sid, "user", "u0", None, "共同开头"),
        rec(sid, "assistant", "a0", "u0", "共同回答"),
        rec(sid, "user", "u-abandoned", "a0", "放弃的第一层输入"),
        tool_assistant(sid, "a-work", "u-abandoned", "开始核对", "t1", "Read",
                       {"file_path": "/synthetic/a.txt"}),
        tool_result(sid, "u-result", "a-work", "t1", "file body"),
        rec(sid, "user", "u-unanswered", "u-result", "第二层未回答输入"),
        interrupt_row(sid, "interrupt", "a-work"),
        rec(sid, "user", "u-replace", "a0", "之后的新输入"),
        rec(sid, "assistant", "a-replace", "u-replace", "新回答")], [])
    sid = "deferred-abort"
    corpus.put(sid, "claude", [
        rec(sid, "user", "u0", None, "共同开头"),
        rec(sid, "assistant", "a0", "u0", "共同回答"),
        rec(sid, "user", "u-abandoned", "a0", "被中断的输入"),
        rec(sid, "assistant", "a-work", "u-abandoned", "进行中的回答"),
        interrupt_row(sid, "interrupt", "a-work", interruptedMessageId="msg_1"),
        rec(sid, "user", "u-replace", "a0", "之后的新输入"),
        rec(sid, "assistant", "a-replace", "u-replace", "新回答")], [])
    return corpus


def fields(row):
    return (row.get("role"), row.get("text"), row.get("turn_id"),
            row.get("interrupted"), row.get("interrupt_reason"))


def compare(sid, path, adapter, opener, base, uid):
    native, _end = adapter.read(str(path))
    # Rust keeps status rows out of `messages`; the last one is `activity`
    # (the contract tests/advanced_parity.py compare_view checks).
    expected = [fields(row) for row in native if row.get("role") != "status"]
    statuses = [row for row in native if row.get("role") == "status"]
    route = "/api/messages/" + quote(uid, safe=":")
    try:
        body = get_json(opener, base, route)
    except HTTPError as err:
        fail(sid, f"HTTP {err.code} {err.read(240)!r}")
    actual = [fields(row) for row in body.get("messages") or []]
    for index in range(max(len(expected), len(actual))):
        left = expected[index] if index < len(expected) else None
        right = actual[index] if index < len(actual) else None
        if left != right:
            fail(sid, f"DIFF index {index} python={left!r} rust={right!r}")
    want = (statuses[-1].get("text"), statuses[-1].get("turn_id")) if statuses else None
    activity = body.get("activity") or None
    got = (activity.get("text"), activity.get("turn_id")) if activity else None
    if want != got:
        fail(sid, f"DIFF activity python={want!r} rust={got!r}")
    passed(sid)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--fixtures-only", type=Path, metavar="DIR",
                        help="write the synthetic corpus into DIR, print file paths, exit 0")
    parser.add_argument("--python-source", type=Path, default=PYTHON_SOURCE)
    args = parser.parse_args()
    if args.fixtures_only is not None:
        root = args.fixtures_only.expanduser().resolve()
        root.mkdir(parents=True, exist_ok=True)
        corpus = build(root)
        for sid in CASES:
            print(corpus.paths[sid])
        return
    python_source = args.python_source.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-claude-interrupt-") as tmp:
        corpus = build(Path(tmp))
        try:
            adapter = load_adapters(python_source, fixture_root=corpus.root)["claude"]
        except (OSError, RuntimeError) as err:
            fail("python-source", str(err))
        with isolated_server(corpus, args.binary) as (base, opener):
            for sid in CASES:
                compare(sid, corpus.paths[sid], adapter, opener, base, corpus.uid(sid))


if __name__ == "__main__":
    main()
