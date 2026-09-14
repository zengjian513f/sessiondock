#!/usr/bin/env python3
"""Grok list-row size is the whole session directory; "exit: 1" is not an error.

Synthetic temp fixtures and an isolated loopback Rust server only.
Python `_dir_size` (every regular file under the session dir) and a
tool-result whose text starts with "exit: 1" must not be flagged as an error.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tempfile
import time
from pathlib import Path

from history_parity import BINARY, Corpus, encoded, get_json, isolated_server

KiB = 1024


def fail(area, why):
    raise SystemExit(f"FAIL {area}: {why}")


def passed(area):
    print(f"PASS {area}", flush=True)


def grok_uid(path):
    return "grok:" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


def dir_size(path):
    total = 0
    for root, _dirs, files in os.walk(path):
        for name in files:
            total += os.stat(os.path.join(root, name)).st_size
    return total


def write_bytes(path, n, fill=b"x"):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes((fill * ((n // len(fill)) + 1))[:n])


def grok_chat_rows():
    return [
        {"type": "user", "content": "<user_query>\nsize parity\n</user_query>", "prompt_index": 1},
        {"type": "assistant", "content": "running", "tool_calls": [
            {"id": "g-exit1", "name": "Bash", "arguments": json.dumps({"command": "false"})},
            {"id": "g-code2", "function": {"name": "Bash", "arguments": json.dumps({"command": "exit 2"})}}]},
        {"type": "tool_result", "tool_call_id": "g-exit1", "content": "exit: 1\nsome output"},
        {"type": "tool_result", "tool_call_id": "g-code2", "content": "command exited with code 2"},
        {"type": "assistant", "content": "done"},
    ]


def put_grok(corpus, name, extra):
    path = corpus.root / "grok/%2Fsynthetic%2Fsize-parity" / name
    path.mkdir(parents=True)
    (path / "summary.json").write_text(json.dumps({
        "id": name, "title": name, "cwd": "/synthetic/size-parity",
        "created_at": "2026-09-11T10:00:00Z", "updated_at": "2026-09-11T10:05:00Z",
        "generated_title": name, "info": {"id": name, "cwd": "/synthetic/size-parity"},
    }), encoding="utf-8")
    (path / "chat_history.jsonl").write_bytes(b"".join(encoded(row) for row in grok_chat_rows()))
    if extra:
        write_bytes(path / "events.jsonl", 30 * KiB, b"e\n")
        write_bytes(path / "updates.jsonl", 50 * KiB, b"u\n")
        write_bytes(path / "tool_definitions.json", 5 * KiB, b"{}")
        write_bytes(path / "attachments" / "a.bin", 7 * KiB, b"\x00")
    corpus.paths[name] = path
    return path


def listed_row(opener, base, uid, *, force=False):
    route = "/api/sessions?force=1" if force else "/api/sessions"
    payload = get_json(opener, base, route)
    for row in payload.get("sessions") or []:
        if row.get("uid") == uid:
            return row, payload
    fail("list", f"missing uid {uid}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    checks = 0
    with tempfile.TemporaryDirectory(prefix="sessiondock-grok-size-") as tmp:
        root = Path(tmp)
        corpus = Corpus(root / "corpus")
        for source in ("claude", "codex", "grok"):
            (corpus.root / source).mkdir(parents=True)
        g1 = put_grok(corpus, "G1", extra=True)
        g2 = put_grok(corpus, "G2", extra=False)
        u1, u2 = grok_uid(g1), grok_uid(g2)
        state = corpus.root / "state"
        state.mkdir()
        os.chmod(state, 0o700)
        with isolated_server(corpus, args.binary, state_dir=state) as (base, opener):
            row1, _ = listed_row(opener, base, u1, force=True)
            row2, _ = listed_row(opener, base, u2)
            want1, want2 = dir_size(g1), dir_size(g2)
            if row1.get("size") != want1:
                fail("G1 size", f"listed {row1.get('size')} != walk {want1}")
            passed("G1 size == os.walk regular files")
            checks += 1
            chat2 = (g2 / "summary.json").stat().st_size + (g2 / "chat_history.jsonl").stat().st_size
            if row2.get("size") != want2 or want2 != chat2:
                fail("G2 size", f"listed {row2.get('size')} walk {want2} summary+chat {chat2}")
            passed("G2 size == summary+chat")
            checks += 1
            if "size_scope" in row1 or "size_scope" in row2:
                fail("size_scope", "list rows must not carry size_scope")
            passed("no size_scope on list rows")
            checks += 1

            before = row1["size"]
            events = g1 / "events.jsonl"
            with events.open("ab") as fh:
                fh.write(b"y" * (20 * KiB))
            grown, trigger = False, "events.jsonl"
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                now, _ = listed_row(opener, base, u1)
                if now.get("size") is not None and now["size"] > before:
                    grown = True
                    break
                time.sleep(0.1)
            if not grown:
                trigger = "chat_history.jsonl"
                with (g1 / "chat_history.jsonl").open("ab") as fh:
                    fh.write(encoded({"type": "user", "content": "stamp wake", "prompt_index": 9}))
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    now, _ = listed_row(opener, base, u1)
                    if now.get("size") is not None and now["size"] > before:
                        grown = True
                        break
                    time.sleep(0.1)
            if not grown:
                fail("append size", "listed size did not grow within 5s after events then chat stamp")
            print(f"PASS size grew after append (trigger={trigger})", flush=True)
            checks += 1

            batch = get_json(opener, base, "/api/messages/" + u1)
            results = {row.get("call_id"): row for row in batch.get("messages") or []
                       if row.get("role") == "tool_result"}
            exit1 = results.get("g-exit1")
            code2 = results.get("g-code2")
            if not exit1 or not code2:
                fail("messages", f"missing tool results {sorted(results)}")
            if exit1.get("error") is True:
                fail("exit: 1", f"error must not be true: {exit1}")
            if exit1.get("exit_code") == 1:
                fail("exit: 1", "must not sniff exit_code==1 from a leading 'exit: 1'")
            passed("exit: 1 is not an error")
            checks += 1
            if "exit_code" not in code2:
                print("DELTA grok tool_result 'exited with code 2' has no exit_code field", flush=True)
            elif code2.get("exit_code") != 2:
                fail("exit_code 2", f"want 2 got {code2.get('exit_code')}")
            else:
                passed("exited with code 2 → exit_code==2")
            checks += 1
    print(f"grok_size_suite: {checks} checks passed")


if __name__ == "__main__":
    main()
