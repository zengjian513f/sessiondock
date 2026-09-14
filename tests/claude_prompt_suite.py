#!/usr/bin/env python3
"""HTTP contract of the live `prompt` field: `sessiondock claude-hook`,
`--write-bridge-settings`, `/api/messages` and `/api/watch` `prompt` /
`prompt_only` packets, and the "cleared once the native answer is recorded"
rule. Synthetic Claude/Codex fixtures, loopback only, no CLI, no Chromium.

Python reference: `sessiondock/claude_bridge.py` (hook file semantics) and
`server._claude_prompt` / `_session_prompt` (JSON shape, clearing rule).
"""
from __future__ import annotations

import http.client
import json
import os
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, Corpus, claude_row, codex_message, codex_row,
    cursor_query, encoded, isolated_server)
from sse_suite import get, open_watch, sse_next  # noqa: E402

REPO = Path(__file__).resolve().parents[1]
NAME = "sessiondock.exe" if os.name == "nt" else "sessiondock"
RELEASE = REPO / "target/release" / NAME
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
SID = "0aaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
CODEX_SID = "0bbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
TOOL = "toolu_01WPGquestion"


def fail(why, body=""):
    if not isinstance(body, str):
        body = json.dumps(body, ensure_ascii=False, default=str)
    raise SystemExit(f"FAIL {why}; {body[:400]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def hook(state_dir, payload, *args, env=None):
    """Run the hook subcommand like Claude does: JSON on stdin, no output, exit 0."""
    done = subprocess.run([str(BINARY), "claude-hook", *args], input=payload,
                          env=env if env is not None else {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                                                           "SESSIONDOCK_STATE_DIR": str(state_dir)},
                          capture_output=True, timeout=20)
    if done.returncode != 0 or done.stdout or done.stderr:
        fail("claude-hook must be silent and exit 0", done.stderr.decode("utf-8", "replace") or done.stdout)
    return done


def question_payload(sid=SID, tool=TOOL, event="PreToolUse"):
    return json.dumps({
        "hook_event_name": event, "session_id": sid, "tool_name": "AskUserQuestion",
        "tool_use_id": tool, "cwd": "/synthetic/history",
        "tool_input": {"questions": [{
            "header": "颜色", "question": "选哪个？", "multiSelect": False,
            "options": [{"label": "红", "description": "暖色"}, "蓝"],
        }]},
    }).encode()


def settle_payload(event, sid=SID, tool=TOOL):
    return json.dumps({"hook_event_name": event, "session_id": sid, "tool_use_id": tool,
                       "tool_name": "AskUserQuestion", "tool_response": {}}).encode()


def base_rows():
    return [
        claude_row(SID, "user", "u0", None, "which colour?"),
        claude_row(SID, "assistant", "a0", "u0", "answer"),
    ]


def question_rows():
    """The native records Claude writes once the dialog is answered."""
    ask = claude_row(SID, "assistant", "a1", "a0")
    ask["message"] = {"role": "assistant", "content": [{
        "type": "tool_use", "id": TOOL, "name": "AskUserQuestion",
        "input": {"questions": [{"header": "颜色", "question": "选哪个？", "multiSelect": False,
                                 "options": [{"label": "红", "description": "暖色"},
                                             {"label": "蓝", "description": ""}]}]}}]}
    answered = claude_row(SID, "user", "u1", "a1")
    answered["message"] = {"role": "user", "content": [{
        "type": "tool_result", "tool_use_id": TOOL, "content": "User selected: 红"}]}
    return [ask, answered]


def main():
    with tempfile.TemporaryDirectory(prefix="sessiondock-prompt-") as temporary:
        root = Path(temporary).resolve()
        corpus = Corpus(root)
        corpus.put(SID, "claude", base_rows(), ["which colour?", "answer"])
        corpus.put(CODEX_SID, "codex", [
            codex_row("session_meta", {"id": CODEX_SID, "session_id": CODEX_SID, "cwd": "/synthetic/history"}),
            codex_message("user", "hello codex", 1), codex_message("assistant", "hi", 2)], ["hello codex", "hi"])
        (root / "grok").mkdir()
        state = root / "state"
        state.mkdir(mode=0o700)
        prompts_dir = state / "claude-prompts"
        prompt_file = prompts_dir / f"{SID}.json"

        # --- the hook subcommand alone (no service): file semantics like claude_bridge.py
        hook(state, b"not json")
        hook(state, b"[1,2]")
        hook(state, question_payload(sid="../evil"))
        if prompts_dir.exists():
            fail("garbage or path-like session ids must not create the prompt directory")
        # No state dir at all: still silent, still 0, writes nowhere.
        hook(state, question_payload(), env={"PATH": os.environ.get("PATH", "/usr/bin:/bin")})
        if prompts_dir.exists():
            fail("a hook without a state directory must write nothing")
        hook(state, question_payload(), "--state-dir", str(state), env={"PATH": os.environ.get("PATH", "/usr/bin:/bin")})
        if not prompt_file.is_file():
            fail("PreToolUse must record the card", str(prompt_file))
        mode = stat.S_IMODE(prompt_file.stat().st_mode)
        if os.name != "nt" and (mode != 0o600 or stat.S_IMODE(prompts_dir.stat().st_mode) != 0o700):
            fail("prompt file/dir must be private", f"file={oct(mode)} dir={oct(stat.S_IMODE(prompts_dir.stat().st_mode))}")
        card = json.loads(prompt_file.read_text())
        want = {"version": 1, "source": "claude", "id": TOOL, "state": "waiting",
                "questions": [{"header": "颜色", "question": "选哪个？", "multiple": False,
                               "options": [{"label": "红", "description": "暖色"}, {"label": "蓝", "description": ""}]}]}
        if {k: v for k, v in card.items() if k != "created"} != want or not isinstance(card.get("created"), int):
            fail("card shape differs from claude_bridge.py", card)
        hook(state, question_payload())  # a repeated PreToolUse rewrites (new `created`)
        if json.loads(prompt_file.read_text())["state"] != "waiting":
            fail("repeated PreToolUse keeps the card waiting")
        hook(state, settle_payload("PostToolUse", tool="another-tool"))
        if json.loads(prompt_file.read_text())["state"] != "waiting":
            fail("PostToolUse of another tool_use_id must not settle the card")
        hook(state, settle_payload("PostToolUseFailure"))
        settled = json.loads(prompt_file.read_text())
        if settled["state"] != "cancelled" or not isinstance(settled.get("settled"), int):
            fail("PostToolUseFailure must mark cancelled and keep the file", settled)
        hook(state, json.dumps({"hook_event_name": "SessionEnd", "session_id": SID}).encode())
        if prompt_file.exists():
            fail("SessionEnd must clear the card")
        leftovers = [p.name for p in prompts_dir.iterdir()]
        if leftovers:
            fail("no temp files may remain", leftovers)
        passed("claude-hook: silent exit 0, private files, waiting/submitted/cancelled/clear semantics")

        # --- --write-bridge-settings
        settings = root / "claude-bridge-settings.json"
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "SESSIONDOCK_STATE_DIR": str(state)}
        done = subprocess.run([str(BINARY), "--write-bridge-settings", str(settings)], env=env,
                              capture_output=True, timeout=20)
        if done.returncode != 0 or not settings.is_file():
            fail("--write-bridge-settings", done.stderr.decode("utf-8", "replace"))
        if os.name != "nt" and stat.S_IMODE(settings.stat().st_mode) != 0o600:
            fail("settings file must be 0600")
        document = json.loads(settings.read_text())
        hooks = document["hooks"]
        for event in ("PreToolUse", "PostToolUse", "PostToolUseFailure"):
            entry = hooks[event][0]
            if entry.get("matcher") != "AskUserQuestion" or entry["hooks"][0]["type"] != "command":
                fail(f"{event} hook shape", document)
            if entry["hooks"][0]["args"] != ["claude-hook", "--state-dir", str(state)]:
                fail(f"{event} hook args must carry the state dir", entry)
            if not Path(entry["hooks"][0]["command"]).is_absolute():
                fail("hook command must be absolute", entry)
        for event in ("SessionStart", "SessionEnd"):
            if "matcher" in hooks[event][0] or hooks[event][0]["hooks"][0]["args"][0] != "claude-hook":
                fail(f"{event} hook shape", document)
        if "permissionDecision" in settings.read_text():
            fail("the bridge settings must be passive")
        relative = subprocess.run([str(BINARY), "--write-bridge-settings", "relative.json"], env=env,
                                  cwd=root, capture_output=True, timeout=20)
        if relative.returncode != 0 or not (root / "relative.json").is_file():
            fail("relative settings path must resolve from cwd")
        unset = subprocess.run([str(BINARY), "--write-bridge-settings", str(root / "x.json")],
                               env={"PATH": env["PATH"]}, capture_output=True, timeout=20)
        if unset.returncode == 0:
            fail("--write-bridge-settings without SESSIONDOCK_STATE_DIR must fail")
        passed("--write-bridge-settings: 0600 exec-form hooks pointing at this binary and the state dir")

        # --- the service: /api/messages prompt field and /api/watch packets
        with isolated_server(corpus, BINARY, state_dir=state) as (base, opener):
            host, port = base.replace("http://", "").split(":")
            port = int(port)
            uid = corpus.uid(SID)
            codex_uid = corpus.uid(CODEX_SID)
            q = quote(uid, safe=":")
            body = get(host, port, f"/api/messages/{q}")
            if "prompt" not in body or body["prompt"] is not None:
                fail("main view without a card: prompt must be present and null", body)
            codex_body = get(host, port, f"/api/messages/{quote(codex_uid, safe=':')}")
            if "prompt" not in codex_body or codex_body["prompt"] is not None:
                fail("Codex main view without an instance: prompt null", codex_body)
            meta = get(host, port, "/api/meta")
            if meta["capabilities"].get("metadata") is not True:
                fail("state dir with claude-prompts/ must still open the metadata store", meta)

            # Open the watch before the card appears.
            conn, resp = open_watch(host, port, uid, body)
            try:
                first = sse_next(conn, resp, time.monotonic() + 5)
                if not first or first.get("event") != "message" or first["data"].get("prompt") is not None:
                    fail("first watch packet must carry prompt:null", first)
                hook(state, question_payload())
                frame = sse_next(conn, resp, time.monotonic() + 5)
                if not frame or frame["data"].get("prompt_only") is not True:
                    fail("a new card must arrive as a prompt_only packet", frame)
                card = frame["data"]["prompt"]
                if card.get("id") != TOOL or card.get("state") != "waiting" or card["questions"][0]["question"] != "选哪个？":
                    fail("prompt_only card shape", frame)
                body = get(host, port, f"/api/messages/{q}")
                if body["prompt"] != card:
                    fail("/api/messages prompt must equal the card", body)
                page = get(host, port, f"/api/messages/{q}?window=1")
                if page["prompt"] != card:
                    fail("windowed read carries the card too", page)
                # A subagent view never carries the field (Python: only `if not agent`).
                # (No agent in this fixture: the query still must not 500.)
                hook(state, settle_payload("PostToolUse"))
                frame = sse_next(conn, resp, time.monotonic() + 5)
                if not frame or frame["data"].get("prompt_only") is not True or frame["data"]["prompt"].get("state") != "submitted":
                    fail("settling must arrive as prompt_only submitted", frame)
                if not prompt_file.is_file():
                    fail("a settled card stays on disk until the native answer lands")
                # The native answer lands: messages packet carries prompt:null and the file is gone.
                with corpus.paths[SID].open("ab") as fh:
                    fh.write(b"".join(encoded(r) for r in question_rows()))
                    fh.flush(); os.fsync(fh.fileno())
                frame = None
                deadline = time.monotonic() + 6
                while time.monotonic() < deadline:
                    frame = sse_next(conn, resp, deadline)
                    if frame and frame.get("event") == "message" and not frame["data"].get("prompt_only"):
                        break
                if not frame or frame["data"].get("prompt_only"):
                    fail("expected a messages packet after the answer landed", frame)
                roles = [m.get("role") for m in frame["data"].get("messages", [])]
                if "answer" not in roles or frame["data"].get("prompt") is not None:
                    fail("the messages packet with the answer must clear the card", frame)
                if prompt_file.exists():
                    fail("the card file must be deleted once the answer is recorded")
                body = get(host, port, f"/api/messages/{q}")
                if body["prompt"] is not None or not any(m.get("role") == "answer" and m.get("call_id") == TOOL for m in body["messages"]):
                    fail("after the answer: prompt null, answer visible", body)
            finally:
                conn.close()
            passed("/api/messages + /api/watch: prompt field, prompt_only packets, cleared by the native answer")

            # A stale card whose answer is already recorded is cleared on the first read.
            hook(state, question_payload())
            body = get(host, port, f"/api/messages/{q}")
            if body["prompt"] is not None or prompt_file.exists():
                fail("a card whose tool_use_id is already answered is cleared on read", body)
            # A card of a different tool id stays even though older answers exist.
            hook(state, question_payload(tool="toolu_02fresh"))
            body = get(host, port, f"/api/messages/{q}")
            if not body["prompt"] or body["prompt"]["id"] != "toolu_02fresh":
                fail("a fresh card is shown", body)
            hook(state, json.dumps({"hook_event_name": "SessionStart", "session_id": SID, "source": "resume"}).encode())
            body = get(host, port, f"/api/messages/{q}")
            if body["prompt"] is not None:
                fail("SessionStart clears the card", body)
            passed("clearing rule: answered tool_use_id beats a waiting file; SessionStart clears")

        # Restart with the prompts directory present: the metadata store must still open.
        with isolated_server(corpus, BINARY, state_dir=state) as (base, opener):
            host, port = base.replace("http://", "").split(":")
            meta = get(host, int(port), "/api/meta")
            if meta["capabilities"].get("metadata") is not True:
                fail("restart with claude-prompts/ present", meta)
        passed("restart: claude-prompts/ is an accepted state-dir entry")
    print("PASS claude_prompt_suite: hook subcommand, bridge settings, prompt field, SSE prompt_only, clearing rule, restart")


if __name__ == "__main__":
    main()
