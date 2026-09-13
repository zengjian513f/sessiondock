#!/usr/bin/env python3
"""Python-oracle parity of GET /api/messages for batch-35 Codex fork shapes.

Compares CodexAdapter.read (role, text) of user/assistant messages with
GET /api/messages/{uid} on an isolated loopback server. Asserts batch-35 rules:
R1 first session_meta is identity, later ones skipped (`跳过重复的Codex
session_meta ×N`) with supported true; R2 history_base null + forked_from_id is
self-contained (no inherited prefix, never an error) while history_base
{thread_id:R, end_byte_offset:c} inherits R[0,c) even when forked_from_id names
Q. Corpus: (a) legacy self-contained fork with two extra metas and copied
history, (b) the same with the parent absent, (e) rewind-past-fork A/Q/R,
(f) a plain thread. Status/event rows are skipped on both sides.
"""
from __future__ import annotations

import argparse
import tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote

from history_parity import (
    BINARY as DEBUG_BINARY, REPO, Corpus, codex_message, codex_row, encoded,
    get_json, isolated_server)
from provider_parity import load_adapters

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
CWD = "/synthetic/fork-messages"


def fail(area, why):
    raise SystemExit(f"FAIL {area}: {why}")


def passed(area):
    print(f"PASS {area}", flush=True)


def meta(sid, **extra):
    payload = {"id": sid, "session_id": sid, "timestamp": "2026-09-11T10:00:00Z",
               "cwd": CWD, "thread_source": "user", "forked_from_id": None,
               "history_base": None, **extra}
    return codex_row("session_meta", payload)


def turn(tid, user, assistant, ordinal=1):
    return [
        codex_row("event_msg", {"type": "task_started", "turn_id": tid}, ordinal),
        codex_message("user", user, ordinal + 1),
        codex_message("assistant", assistant, ordinal + 2),
        codex_row("event_msg", {"type": "task_complete", "turn_id": tid}, ordinal + 3),
    ]


def pairs(rows):
    return [(m.get("role"), m.get("text")) for m in rows
            if m.get("role") in ("user", "assistant")]


def build(root: Path) -> Corpus:
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    copied = turn("p1", "legacy prefix q", "legacy prefix a")
    own = turn("f1", "legacy fork q", "legacy fork a", 5)
    corpus.put("codex-legacy-parent", "codex", [
        meta("codex-legacy-parent"), *copied,
        *turn("p2", "parent tail q", "parent tail a", 5)], [])
    # (a) history_base null, parent present, two extra session_meta, copied prefix.
    fork = "codex-legacy-fork"
    corpus.put(fork, "codex", [
        meta(fork, forked_from_id="codex-legacy-parent"),
        meta(fork, forked_from_id="codex-legacy-parent"),
        meta(fork, forked_from_id="codex-legacy-parent"),
        *copied, *own], [])
    # (b) same fork bytes, parent file never written.
    orphan = "codex-orphan-fork"
    corpus.put(orphan, "codex", [
        meta(orphan, forked_from_id="codex-missing-parent"),
        meta(orphan, forked_from_id="codex-missing-parent"),
        meta(orphan, forked_from_id="codex-missing-parent"),
        *copied, *own], [])
    # (e) A.forked_from_id=Q, A.history_base.thread_id=R (rewind past the fork).
    r_prefix = [meta("codex-rewind-R"), *turn("r1", "R prefix q", "R prefix a")]
    cut = sum(len(encoded(row)) for row in r_prefix)
    base_r = {"thread_id": "codex-rewind-R", "end_byte_offset": cut,
              "end_ordinal_exclusive": 5}
    corpus.put("codex-rewind-R", "codex",
               r_prefix + turn("r2", "R tail q", "R tail a", 5), [])
    corpus.put("codex-rewind-Q", "codex", [
        meta("codex-rewind-Q", forked_from_id="codex-rewind-R", history_base=base_r),
        *turn("q1", "Q only q", "Q only a")], [])
    corpus.put("codex-rewind-A", "codex", [
        meta("codex-rewind-A", forked_from_id="codex-rewind-Q", history_base=base_r),
        *turn("a1", "A rewind q", "A rewind a")], [])
    # (f) independent thread.
    corpus.put("codex-plain", "codex", [
        meta("codex-plain"), *turn("t1", "plain q", "plain a")], [])
    return corpus


def compare(sid, path, adapter, opener, base, uid):
    python_msgs, _end = adapter.read(str(path))
    expected = pairs(python_msgs)
    route = "/api/messages/" + quote(uid, safe=":")
    try:
        body = get_json(opener, base, route)
    except HTTPError as err:
        fail(sid, f"HTTP {err.code} {err.read(240)!r}")
    actual = pairs(body.get("messages") or [])
    n = max(len(expected), len(actual))
    for index in range(n):
        left = expected[index] if index < len(expected) else None
        right = actual[index] if index < len(actual) else None
        if left != right:
            fail(sid, f"DIFF index {index} python={left!r} rust={right!r}")
    passed(sid)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--fixtures-only", type=Path, metavar="DIR")
    parser.add_argument("--python-source", type=Path, default=REPO.parent / "agenthub")
    args = parser.parse_args()
    if args.fixtures_only is not None:
        root = args.fixtures_only.expanduser().resolve()
        root.mkdir(parents=True, exist_ok=True)
        corpus = build(root)
        for path in sorted(corpus.paths.values()):
            print(path)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-fork-messages-") as tmp:
        corpus = build(Path(tmp))
        try:
            adapter = load_adapters(
                args.python_source, fixture_root=corpus.root,
                codex_paths=dict(corpus.paths))["codex"]
        except (OSError, RuntimeError) as err:
            fail("python-source", str(err))
        with isolated_server(corpus, args.binary) as (base, opener):
            for sid in sorted(corpus.paths):
                compare(sid, corpus.paths[sid], adapter, opener, base, corpus.uid(sid))


if __name__ == "__main__":
    main()
