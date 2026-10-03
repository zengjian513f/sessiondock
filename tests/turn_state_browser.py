#!/usr/bin/env python3
"""Sidebar and header turn state: working / waiting / idle while a CLI runs.

Two synthetic sessions (Claude and Codex) are resumed through the console
button with a fake CLI (a plain shell), so both are really live. Native
records appended to the session that is *not* open drive its sidebar dot
through the list row's `turn` (docs/read-model.md): working pulses, an open
question tool turns amber, a finished or interrupted turn is a still dot, and
`!` shell records start no turn, and a finished main turn whose background
subagent still runs keeps turning. For the open session the CLI state object's
`instance.busy` (docs/cli-state.md) wins: typing into the console makes the
fake CLI print Codex's busy footer, and the header and sidebar dots pulse
although the transcript says the turn is complete; Codex's live background
terminal status keeps both dots pulsing after task_complete, survives reload,
and clears when the terminals end (quoted status and editor text do not count).
With a quiet screen, a
finished Claude turn whose Monitor watch or backgrounded command still runs
keeps turning until its end notice or TaskStop, and a watchdog Monitor that
tails an ended command's output file no longer holds it. No model binary,
native CLI home or production host is touched.
"""
from browser_runtime import js
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import threading
import uuid
from playwright.sync_api import sync_playwright, expect
from history_parity import REPO, BINARY, Corpus, claude_row, codex_row, codex_message, encoded, isolated_server

# Fresh ids per run: a host left by an aborted run must not look like an
# outside instance of the next run's sessions.
CLAUDE_SID = str(uuid.uuid4())
CODEX_SID = str(uuid.uuid4())
CODEX_AGENT = str(uuid.uuid4())
FAKE_CLI = """#!/bin/sh
stty -echo 2>/dev/null
printf "RS_SHELL_READY\\n"
while IFS= read -r line; do
  case "$line" in
    busy) printf "Working (3s - esc to interrupt)\\n" ;;
    idle) printf "\\033[2J\\033[HRS_IDLE\\n" ;;
    background|background-many|background-wrapped|background-quota|background-quota-wrapped|background-zero|quoted|quoted-quota|draft)
      printf "\\033[2J\\033[HRS_SHELL_READY\\nRS_SCREEN_%s\\nSynthetic completed answer\\n\\n" "$line"
      status="1 background terminal running · /ps to view · /stop to close"
      case "$line" in
        background-many) status="2 background terminals running · /ps to view · /stop to close" ;;
        background-wrapped) status="1 background terminal running · /ps to view
· /stop to close" ;;
        background-zero) status="0 background terminals running · /ps to view · /stop to close" ;;
      esac
      if [ "$line" = draft ]; then
        printf "› %s\\n" "$status"
      else
        printf "%s\\n" "$status"
        case "$line" in quoted|quoted-quota) printf "Synthetic quoted tool output ends here\\n" ;; esac
        case "$line" in
          background-quota|quoted-quota)
            printf "\\n  ⚠ weekly limit: 11%% left · resets at 11:26 PM on 7 Oct · /status\\n" ;;
          background-quota-wrapped)
            printf "\\n  ⚠ weekly limit: 11%% left · resets at 11:26 PM\\non 7 Oct · /status\\n" ;;
        esac
        printf "\\n› \\033[2mAsk Codex to do anything\\033[0m\\n"
      fi
      printf "\\n  GPT-6-Astra high · Context 73%% used · Main [default]\\n  ? for shortcuts\\n"
      ;;
    *) printf "RS_INPUT_OK\\n" ;;
  esac
done
"""
TURN = re.compile(r"\bturn-(working|waiting)\b")


def wait_xterm(page, text):
    page.wait_for_function(
        js("text => [...T.views.values()].some(v => v.term?.buffer?.active && Array.from({length: v.term.buffer.active.length},"
        " (_, i) => v.term.buffer.active.getLine(i)?.translateToString() || '').join('\\n').includes(text))", "text => [...runtime.terminal.state.views.values()].some(v => v.term?.buffer?.active && Array.from({length: v.term.buffer.active.length}, (_, i) => v.term.buffer.active.getLine(i)?.translateToString() || '').join('\\n').includes(text))"),
        arg=text, timeout=15000)


def wait_busy(page, uid, busy):
    try:
        page.wait_for_function(js("([uid, busy]) => cache.get(uid)?.cli?.instance?.busy === busy", '([uid, busy]) => runtime.core.cache.cache.get(uid)?.cli?.instance?.busy === busy'),
                               arg=[uid, busy], timeout=20000)
    except Exception:
        raise AssertionError(("cli", busy, page.evaluate(js("uid => cache.get(uid)?.cli ?? null", 'uid => runtime.core.cache.cache.get(uid)?.cli ?? null'), uid))) from None


def stop_hosts(host_dir):
    """Ends the ptyhost processes this run started (matched by their --dir)."""
    marker = f"--dir\0{host_dir}\0".encode()
    for entry in Path("/proc").iterdir():
        try:
            if entry.name.isdigit() and marker in (entry / "cmdline").read_bytes():
                os.kill(int(entry.name), signal.SIGTERM)
        except (OSError, ProcessLookupError):
            pass


def append(path, *rows):
    with path.open("ab") as stream:
        for row in rows:
            stream.write(encoded(row))


def row_turn(opener, base, uid, size):
    """The list row once the index has summarized the file at `size` bytes."""
    rows = json.loads(opener.open(base + "/api/sessions?force=1", timeout=10).read())["sessions"]
    row = next(row for row in rows if row["uid"] == uid)
    assert row["size"] == size, (row["size"], size)
    return row.get("turn")


def main(binary=BINARY):
    if os.name != "posix":
        raise SystemExit("Real launch acceptance currently requires POSIX; no Windows/macOS claim.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-turn-state-") as temporary:
        root = Path(temporary).resolve()
        for name in ["host", "work", "ledger", "state", "bin", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        work = str(root / "work")
        corpus = Corpus(root)
        claude_path = corpus.put(CLAUDE_SID, "claude", [
            claude_row(CLAUDE_SID, "user", "u0", None, "Synthetic Claude turn target", cwd=work),
            claude_row(CLAUDE_SID, "assistant", "a0", "u0", "Synthetic answer", cwd=work)], [])
        codex_path = corpus.put(CODEX_SID, "codex", [
            codex_row("session_meta", {"id": CODEX_SID, "cwd": work}),
            codex_message("user", "Synthetic Codex turn target")], [])
        claude_uid, codex_uid = corpus.uid(CLAUDE_SID), corpus.uid(CODEX_SID)
        # Inherited closed turn in the head, resumed turn in the middle, and
        # one record larger than the tail window: reproduces the report's
        # active:false / detail activity:working disagreement on a cold index.
        agent_meta = codex_row("session_meta", {"id": CODEX_AGENT, "cwd": work,
            "thread_source": "subagent", "parent_thread_id": CODEX_SID,
            "source": {"subagent": {"thread_spawn": {
                "parent_thread_id": CODEX_SID, "agent_path": "/root/long_turn"}}}})
        padding = codex_row("world_state", {"padding": "x" * (1200 * 1024)})
        agent_rows = [agent_meta,
            codex_row("event_msg", {"type": "task_complete", "turn_id": "inherited"}),
            codex_row("world_state", {"padding": "x" * (110 * 1024)}),
            codex_row("event_msg", {"type": "task_started", "turn_id": "long-agent"}),
            codex_message("assistant", "Synthetic long agent working"), padding]
        (root / "bin/fake-cli").write_text(FAKE_CLI)
        (root / "bin/fake-cli").chmod(0o700)
        environment = {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
            "host_dir": str(root / "host"), "adapters": [], "profiles": [
                {"id": "claude-cli-v1", "source": "claude", "executable": str(root / "bin/fake-cli"),
                 "args": [], "new_args": ["--session-id", "{session_id}"],
                 "resume_args": ["--resume", "{sid}"], "env": environment},
                {"id": "codex-cli-v1", "source": "codex", "executable": str(root / "bin/fake-cli"),
                 "args": [], "resume_args": ["resume", "{sid}"], "env": environment}]}))
        initialized = subprocess.run([str(binary), "--initialize-lifecycle", str(root / "ledger")],
            cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        assert initialized.returncode == 0, initialized.stderr.decode()
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, binary, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration, state_dir=root / "state") as (base, opener):
                    errors = []
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                    page = context.new_page()
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(base, wait_until="networkidle")
                    badge = lambda uid: page.locator(f'#side .item[data-uid="{uid}"] > .ico > .item-status')
                    header = page.locator("#dlive")

                    # Both sessions start with a finished turn and no process.
                    assert row_turn(opener, base, claude_uid, claude_path.stat().st_size) == "idle"
                    assert row_turn(opener, base, codex_uid, codex_path.stat().st_size) is None
                    for uid in [claude_uid, codex_uid]:
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                        expect(page.locator("#msgs")).to_contain_text("turn target")
                        with page.expect_response(lambda response: response.url.endswith("/api/term/takeover")) as taken:
                            page.locator("#a-term").click()
                        assert taken.value.status == 200, taken.value.text()
                        assert "needs_confirm" not in taken.value.json(), taken.value.text()
                        wait_xterm(page, "RS_SHELL_READY")
                        page.wait_for_function(js("uid => S.live.has(uid)", 'uid => runtime.core.state.live.live.has(uid)'), arg=uid, timeout=20000)
                    expect(badge(claude_uid)).to_be_visible()
                    expect(badge(claude_uid)).not_to_have_class(TURN)
                    expect(badge(claude_uid)).to_have_attribute("title", re.compile("空闲"))

                    # ---- Claude is not open: its dot follows the list row's turn.
                    append(claude_path, claude_row(CLAUDE_SID, "user", "u1", "a0", "Synthetic next input", cwd=work))
                    expect(badge(claude_uid)).to_have_class(re.compile(r"\bturn-working\b"), timeout=20000)
                    expect(badge(claude_uid)).to_have_attribute("title", re.compile("正在处理"))
                    ask = claude_row(CLAUDE_SID, "assistant", "a1", "u1", [{"type": "tool_use", "id": "toolu_ask",
                        "name": "AskUserQuestion", "input": {"questions": [{"question": "Pick one?",
                        "options": [{"label": "A"}, {"label": "B"}]}]}}], cwd=work)
                    ask["message"]["stop_reason"] = "tool_use"
                    append(claude_path, ask)
                    expect(badge(claude_uid)).to_have_class(re.compile(r"\bturn-waiting\b"), timeout=20000)
                    expect(badge(claude_uid)).to_have_attribute("title", re.compile("等待回答"))
                    expect(badge(claude_uid)).to_have_text(re.compile(r'^\?\d*$'))
                    expect(badge(claude_uid)).to_have_css('background-color', 'rgb(251, 191, 36)')
                    append(claude_path,
                        claude_row(CLAUDE_SID, "user", "u2", "a1", [{"type": "tool_result", "tool_use_id": "toolu_ask",
                                                                      "content": "A"}], cwd=work),
                        claude_row(CLAUDE_SID, "assistant", "a2", "u2", "Synthetic final answer", cwd=work),
                        claude_row(CLAUDE_SID, "system", "s2", "a2", subtype="turn_duration", durationMs=1200, cwd=work))
                    expect(badge(claude_uid)).not_to_have_class(TURN, timeout=20000)
                    expect(badge(claude_uid)).to_have_attribute("title", re.compile("空闲"))
                    # A `!` shell command runs no model turn.
                    append(claude_path,
                        claude_row(CLAUDE_SID, "user", "u3", "s2", "<bash-input>ls</bash-input>", cwd=work),
                        claude_row(CLAUDE_SID, "user", "u4", "u3", "<bash-stdout>file</bash-stdout><bash-stderr></bash-stderr>", cwd=work))
                    assert row_turn(opener, base, claude_uid, claude_path.stat().st_size) == "idle"
                    # Esc: the interrupt mark ends the turn it cut off.
                    append(claude_path, claude_row(CLAUDE_SID, "user", "u5", "u4", "Synthetic cut input", cwd=work))
                    expect(badge(claude_uid)).to_have_class(re.compile(r"\bturn-working\b"), timeout=20000)
                    append(claude_path, claude_row(CLAUDE_SID, "user", "u6", "u5", [{"type": "text",
                        "text": "[Request interrupted by user]"}], cwd=work))
                    assert row_turn(opener, base, claude_uid, claude_path.stat().st_size) == "aborted"
                    expect(badge(claude_uid)).not_to_have_class(TURN, timeout=20000)
                    # A finished main turn waiting on a background subagent is still turning.
                    agents = claude_path.parent / CLAUDE_SID / "subagents"
                    agents.mkdir(parents=True)
                    (agents / "agent-a1b2c3d4e5f6a7b8c.meta.json").write_text(
                        json.dumps({"agentType": "general-purpose", "description": "Synthetic background job"}))
                    sidecar = agents / "agent-a1b2c3d4e5f6a7b8c.jsonl"
                    append(sidecar, claude_row(CLAUDE_SID, "user", "b0", None, "Synthetic background job",
                                               isSidechain=True, agentId="a1b2c3d4e5f6a7b8c", cwd=work))
                    expect(badge(claude_uid)).to_have_class(re.compile(r"\bturn-working\b"), timeout=20000)
                    rows = json.loads(opener.open(base + "/api/sessions?force=1", timeout=10).read())["sessions"]
                    row = next(row for row in rows if row["uid"] == claude_uid)
                    assert row["turn"] == "working" and row["agent_items"][0]["active"] is True, row
                    append(sidecar, claude_row(CLAUDE_SID, "assistant", "b1", "b0", "Synthetic background result",
                                               isSidechain=True, agentId="a1b2c3d4e5f6a7b8c", cwd=work))
                    expect(badge(claude_uid)).not_to_have_class(TURN, timeout=20000)

                    # ---- Codex leaves the view (Claude opens): rollout boundaries drive it.
                    page.locator(f'#side .item[data-uid="{claude_uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic final answer")
                    append(codex_path, codex_row("event_msg", {"type": "task_started", "turn_id": "t1"}, 2))
                    append(codex_path, padding)
                    expect(badge(codex_uid)).to_have_class(re.compile(r"\bturn-working\b"), timeout=20000)
                    append(codex_path, codex_row("response_item", {"type": "function_call", "name": "request_user_input",
                        "call_id": "call_ask", "arguments": json.dumps({"questions": [{"question": "Go?",
                        "options": [{"label": "Yes"}]}]})}, 3))
                    expect(badge(codex_uid)).to_have_class(re.compile(r"\bturn-waiting\b"), timeout=20000)
                    append(codex_path,
                        codex_row("response_item", {"type": "function_call_output", "call_id": "call_ask", "output": "Yes"}, 4),
                        codex_row("event_msg", {"type": "task_complete", "turn_id": "t1"}, 5))
                    append(codex_path, padding)
                    expect(badge(codex_uid)).not_to_have_class(TURN, timeout=20000)
                    assert row_turn(opener, base, codex_uid, codex_path.stat().st_size) == "idle"

                    # ---- Long Codex subagent: native/list/detail agree and
                    # both the sidebar and the view menu display its green dot.
                    agent_path = corpus.put(CODEX_AGENT, "codex", agent_rows, [], parent=CODEX_SID)
                    page.locator(f'#side .item[data-uid="{codex_uid}"]:not(.agent)').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic Codex turn target")
                    agent_dot = page.locator(f'#side .item.agent[data-agent="{CODEX_AGENT}"] .item-status')
                    expect(agent_dot).to_have_class(re.compile(r"\bvisible\b"), timeout=20000)
                    page.locator("#a-view-switch").click()
                    choice = page.locator(f'#session-view-menu button[data-agent="{CODEX_AGENT}"]')
                    expect(choice.locator(".view-live")).to_be_visible()
                    choice.click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic long agent working")
                    page.wait_for_function(js("cache.get(viewKey(S.sel,S.agent))?.activity?.state === 'working'", "runtime.core.cache.cache.get(runtime.viewKey(runtime.core.state.selection.sel,runtime.core.state.selection.agent))?.activity?.state === 'working'"))

                    def agent_active(expected):
                        rows = json.loads(opener.open(base + "/api/sessions?force=1", timeout=10).read())["sessions"]
                        row = next(row for row in rows if row["uid"] == codex_uid)
                        agent = next(item for item in row["agent_items"] if item["id"] == CODEX_AGENT)
                        assert agent["active"] is expected, agent

                    agent_active(True)
                    append(agent_path, codex_row("event_msg", {"type": "task_complete", "turn_id": "long-agent"}), padding)
                    agent_active(False)
                    expect(agent_dot).not_to_have_class(re.compile(r"\bvisible\b"), timeout=20000)
                    page.locator("#a-view-switch").click()
                    expect(choice.locator(".view-live")).to_have_count(0)
                    page.locator('#session-view-menu button[data-agent=""]').click()
                    # A partial start record changes nothing until its LF is committed.
                    start = encoded(codex_row("event_msg", {"type": "turn_started", "turn_id": "wake"}))
                    with agent_path.open("ab") as stream:
                        stream.write(start[:-1])
                    agent_active(False)
                    with agent_path.open("ab") as stream:
                        stream.write(b"\n")
                    append(agent_path, padding)
                    agent_active(True)
                    expect(agent_dot).to_have_class(re.compile(r"\bvisible\b"), timeout=20000)
                    # Replacing the inode resets the saved boundary, including
                    # when the replacement has a completion buried in its middle.
                    replacement = agent_path.with_suffix(".replacement")
                    replacement.write_bytes(encoded(agent_meta) + encoded(codex_row("world_state", {
                        "padding": "x" * (110 * 1024)})) + encoded(codex_row("event_msg", {
                            "type": "turn_aborted", "turn_id": "replacement"})) + encoded(padding))
                    replacement.replace(agent_path)
                    agent_active(False)
                    expect(agent_dot).not_to_have_class(re.compile(r"\bvisible\b"), timeout=20000)
                    # Same-size rewrites also invalidate the scalar cache.
                    original = agent_path.read_bytes()
                    rewritten = original.replace(b'"turn_aborted"', b'"task_started"')
                    assert len(rewritten) == len(original)
                    agent_path.write_bytes(rewritten)
                    agent_active(True)
                    expect(agent_dot).to_have_class(re.compile(r"\bvisible\b"), timeout=20000)
                    agent_path.write_bytes(original)
                    agent_active(False)
                    expect(agent_dot).not_to_have_class(re.compile(r"\bvisible\b"), timeout=20000)
                    # A live CLI can append between metadata and scalar reads.
                    # Refresh the real sidebar while it grows: the main row must
                    # retain its native identity and committed open-turn state.
                    append(codex_path, codex_row("event_msg", {"type":"task_started", "turn_id":"append-race"}))
                    stop_append = threading.Event()
                    suffix = encoded(codex_row("world_state", {"padding":"x" * 16384}))
                    def grow_native():
                        with codex_path.open("ab", buffering=0) as stream:
                            for _ in range(2000):
                                if stop_append.is_set(): break
                                stream.write(suffix)
                                stop_append.wait(.003)
                    writer = threading.Thread(target=grow_native)
                    writer.start()
                    try:
                        for _ in range(30):
                            page.evaluate(js("async () => await loadSessions(true)", 'async () => await runtime.core.list.loadSessions(true)'))
                            expect(page.locator(f'#side .item[data-uid="{codex_uid}"]:not(.agent)')).to_have_count(1)
                            row = page.evaluate(js("uid => S.sessions.find(row => row.uid === uid)", 'uid => runtime.core.state.catalog.sessions.find(row => row.uid === uid)'), codex_uid)
                            assert row and row["sid"] == CODEX_SID and row["turn"] == "working", row
                    finally:
                        stop_append.set(); writer.join(timeout=5)
                    append(codex_path, codex_row("event_msg", {"type":"task_complete", "turn_id":"append-race"}))
                    page.locator(f'#side .item[data-uid="{claude_uid}"]:not(.agent)').click()

                    # ---- Claude open, quiet screen: a finished turn waiting on its background
                    # tasks (a Monitor watch, a backgrounded command) keeps turning.
                    wait_busy(page, claude_uid, False)
                    expect(header).not_to_have_class(TURN)

                    leaf = ["u6"]

                    def finished_turn(tag, *records):
                        """Appends the records, an answer and `turn_duration`, chained after the last leaf."""
                        answer = claude_row(CLAUDE_SID, "assistant", f"{tag}-a", None, "Synthetic wait", cwd=work)
                        done = claude_row(CLAUDE_SID, "system", f"{tag}-s", None, subtype="turn_duration",
                                          durationMs=900, cwd=work)
                        for record in (*records, answer, done):
                            if record["type"] == "queue-operation":
                                for key in ["uuid", "parentUuid", "isSidechain", "cwd"]:
                                    record.pop(key, None)
                                continue
                            record["parentUuid"], leaf[0] = leaf[0], record["uuid"]
                        append(claude_path, *records, answer, done)

                    def background_row():
                        rows = json.loads(opener.open(base + "/api/sessions?force=1", timeout=10).read())["sessions"]
                        row = next(row for row in rows if row["uid"] == claude_uid)
                        assert row["size"] == claude_path.stat().st_size, row
                        return row.get("turn"), row.get("background")

                    def notification(task, event, status=""):
                        return (f"<task-notification>\n<task-id>{task}</task-id>\n{status}"
                                f"<summary>Monitor event: \"Synthetic watch\"</summary>\n<event>{event}</event>\n"
                                "</task-notification>")

                    watch = claude_row(CLAUDE_SID, "assistant", "m0", None, [{"type": "tool_use", "id": "toolu_watch",
                        "name": "Monitor", "input": {"description": "Synthetic watch", "command": "true"}}], cwd=work)
                    watch["message"]["stop_reason"] = "tool_use"
                    finished_turn("m1", watch, claude_row(CLAUDE_SID, "user", "m0r", "m0", [{"type": "tool_result",
                        "tool_use_id": "toolu_watch", "content": "Monitor started (task bsynwatch1)"}],
                        toolUseResult={"taskId": "bsynwatch1", "timeoutMs": 1800000, "persistent": False}, cwd=work))
                    expect(header).to_have_class(re.compile(r"\bturn-working\b"), timeout=20000)
                    expect(header).to_have_attribute("title", re.compile("正在处理"))
                    expect(badge(claude_uid)).to_have_class(re.compile(r"\bturn-working\b"))
                    assert background_row() == ("working", 1)
                    # A progress event queued into the CLI does not end the watch.
                    event = notification("bsynwatch1", "outputs=10/40")
                    finished_turn("m2", claude_row(CLAUDE_SID, "queue-operation", "m2q", None, event, operation="enqueue"),
                                  claude_row(CLAUDE_SID, "user", "m2u", None, event, origin={"kind": "task-notification"},
                                             cwd=work))
                    assert background_row() == ("working", 1)
                    expect(header).to_have_class(re.compile(r"\bturn-working\b"))
                    # The stream-ended notice (it carries a status) ends it.
                    finished_turn("m3", claude_row(CLAUDE_SID, "user", "m3u", None,
                        notification("bsynwatch1", "done", "<status>completed</status>\n"), cwd=work))
                    expect(header).not_to_have_class(TURN, timeout=20000)
                    expect(badge(claude_uid)).not_to_have_class(TURN)
                    assert background_row() == ("idle", None)
                    # A backgrounded command runs until TaskStop stops it.
                    finished_turn("m4", claude_row(CLAUDE_SID, "user", "m4u", None, [{"type": "tool_result",
                        "tool_use_id": "toolu_bg", "content": "Command running in background with ID: bsynbash1."}],
                        toolUseResult={"stdout": "", "backgroundTaskId": "bsynbash1"}, cwd=work))
                    expect(header).to_have_class(re.compile(r"\bturn-working\b"), timeout=20000)
                    finished_turn("m5", claude_row(CLAUDE_SID, "user", "m5u", None, [{"type": "tool_result",
                        "tool_use_id": "toolu_stop", "content": "Successfully stopped task: bsynbash1"}],
                        toolUseResult={"message": "Successfully stopped task: bsynbash1", "task_id": "bsynbash1",
                                       "task_type": "local_bash"}, cwd=work))
                    expect(header).not_to_have_class(TURN, timeout=20000)
                    assert background_row() == ("idle", None)
                    # A watchdog Monitor tailing a backgrounded command's output file waits on
                    # nothing once that command ends, although its tail stays alive until expiry.
                    finished_turn("m6", claude_row(CLAUDE_SID, "user", "m6u", None, [{"type": "tool_result",
                        "tool_use_id": "toolu_bash2", "content": "Command running in background with ID: bsynbash2."}],
                        toolUseResult={"stdout": "", "backgroundTaskId": "bsynbash2"}, cwd=work))
                    dog = claude_row(CLAUDE_SID, "assistant", "m7", None, [{"type": "tool_use", "id": "toolu_dog",
                        "name": "Monitor", "input": {"description": "watchdog: synthetic run",
                        "command": "tail -F -n +1 /tmp/claude-1000/p/s/tasks/bsynbash2.output | grep --line-buffered PASS"}}],
                        cwd=work)
                    dog["message"]["stop_reason"] = "tool_use"
                    finished_turn("m8", dog, claude_row(CLAUDE_SID, "user", "m7r", "m7", [{"type": "tool_result",
                        "tool_use_id": "toolu_dog", "content": "Monitor started (task bsyndog1)"}],
                        toolUseResult={"taskId": "bsyndog1", "timeoutMs": 1800000, "persistent": False}, cwd=work))
                    expect(header).to_have_class(re.compile(r"\bturn-working\b"), timeout=20000)
                    assert background_row() == ("working", 2)
                    finished_turn("m9", claude_row(CLAUDE_SID, "user", "m9u", None,
                        notification("bsynbash2", "exit 0", "<status>completed</status>\n"), cwd=work))
                    expect(header).not_to_have_class(TURN, timeout=20000)
                    expect(badge(claude_uid)).not_to_have_class(TURN)
                    assert background_row() == ("idle", None)

                    # ---- A subagent of the live session has no composer: the parent's
                    # console button repaint must not bring back the parent's editor or its
                    # CLI input notice.
                    if page.locator("#termpane").is_visible():
                        page.locator("#a-term").click()
                    expect(page.locator("#composer")).to_be_visible()
                    page.locator('#side .item.agent[data-agent="a1b2c3d4e5f6a7b8c"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic background result")
                    page.wait_for_function(js("S.agent === 'a1b2c3d4e5f6a7b8c'", "runtime.core.state.selection.agent === 'a1b2c3d4e5f6a7b8c'"))
                    expect(page.locator("#a-term")).to_have_attribute("aria-label", re.compile("子代理"))
                    page.evaluate(js("renderTakeoverBtn()", 'runtime.takeover.renderTakeoverBtn()'))
                    expect(page.locator("#composer")).to_be_hidden()
                    expect(page.locator("#composer-input-status")).to_be_hidden()
                    page.locator(f'#side .item[data-uid="{claude_uid}"]:not(.agent)').click()
                    expect(page.locator("#composer")).to_be_visible()

                    # ---- Codex open: the screen's busy footer outranks the finished transcript.
                    page.locator(f'#side .item[data-uid="{codex_uid}"]').click()
                    expect(page.locator("#msgs")).to_contain_text("Synthetic Codex turn target")
                    wait_busy(page, codex_uid, False)
                    expect(header).not_to_have_class(TURN)
                    if not page.locator("#termpane").is_visible():
                        page.locator("#a-term").click()
                    wait_xterm(page, "RS_SHELL_READY")
                    page.locator("#xterm").click()
                    page.keyboard.type("busy")
                    page.keyboard.press("Enter")
                    wait_xterm(page, "esc to interrupt")
                    wait_busy(page, codex_uid, True)
                    expect(header).to_have_class(re.compile(r"\bturn-working\b"))
                    expect(header).to_have_attribute("title", re.compile("正在处理"))
                    expect(badge(codex_uid)).to_have_class(re.compile(r"\bturn-working\b"))
                    page.keyboard.type("idle")
                    page.keyboard.press("Enter")
                    wait_xterm(page, "RS_IDLE")
                    wait_busy(page, codex_uid, False)
                    expect(header).not_to_have_class(TURN)
                    expect(badge(codex_uid)).not_to_have_class(TURN)
                    expect(badge(codex_uid)).to_have_attribute("title", re.compile("空闲"))
                    # The CLI state object carries the field for every observed session.
                    cli = page.evaluate(js("uid => cache.get(uid).cli", 'uid => runtime.core.cache.cache.get(uid).cli'), codex_uid)
                    assert cli["instance"] == {"running": True, "busy": False}, cli

                    # BUG-20261001-133105-146ce9: detached work no longer
                    # appears in the TUI or below its CLI, but carries the
                    # native thread identity. It overrides a quiet screen for
                    # both dots; a permanent code-mode helper does not.
                    task_env = {"PATH": "/usr/bin:/bin", "CODEX_SESSION_ID": CODEX_SID,
                                "CODEX_THREAD_ID": CODEX_SID}
                    helper = subprocess.Popen(["/synthetic/codex-code-mode-host", "60"],
                                              executable="/bin/sleep", env=task_env)
                    try:
                        page.evaluate(js("refreshLive(true)", 'runtime.core.live.refreshLive(true)'))
                        expect(header).not_to_have_class(TURN)
                        task = subprocess.Popen(["/bin/sleep", "60"], env=task_env, start_new_session=True)
                        try:
                            page.evaluate(js("refreshLive(true)", 'runtime.core.live.refreshLive(true)'))
                            wait_busy(page, codex_uid, False)
                            expect(header).to_have_class(re.compile(r"\bturn-working\b"))
                            expect(badge(codex_uid)).to_have_class(re.compile(r"\bturn-working\b"))
                            page.reload(wait_until="networkidle")
                            page.wait_for_function(js("uid => S.sel === uid && S.live.has(uid)", 'uid => runtime.core.state.selection.sel === uid && runtime.core.state.live.live.has(uid)'), arg=codex_uid)
                            wait_busy(page, codex_uid, False)
                            expect(header).to_have_class(re.compile(r"\bturn-working\b"))
                        finally:
                            task.terminate()
                            task.wait(timeout=5)
                        page.evaluate(js("refreshLive(true)", 'runtime.core.live.refreshLive(true)'))
                        expect(header).not_to_have_class(TURN)
                        expect(badge(codex_uid)).not_to_have_class(TURN)
                    finally:
                        helper.terminate()
                        helper.wait(timeout=5)
                    print("PASS quiet TUI with detached owned work and permanent helper isolation")

                    # BUG-20261001-082816-f14dc6: the turn is complete but
                    # Codex still owns a background terminal. Read the live
                    # footer through the host, CLI state and watch stream.
                    # BUG-20261001-100155-ba71d1: the quota banner between
                    # the live status and composer must not hide background work.
                    for command in ["background", "background-many", "background-wrapped",
                                    "background-quota", "background-quota-wrapped"]:
                        page.locator("#xterm").click()
                        page.keyboard.type(command)
                        page.keyboard.press("Enter")
                        wait_xterm(page, "background terminal")
                        wait_busy(page, codex_uid, True)
                        assert row_turn(opener, base, codex_uid, codex_path.stat().st_size) == "idle"
                        expect(header).to_have_class(re.compile(r"\bturn-working\b"))
                        expect(badge(codex_uid)).to_have_class(re.compile(r"\bturn-working\b"))
                        assert page.evaluate("getComputedStyle(document.querySelector('#dlive')).animationName") != "none"
                        page.reload(wait_until="networkidle")
                        page.wait_for_function(js("uid => S.sel === uid && S.live.has(uid)", 'uid => runtime.core.state.selection.sel === uid && runtime.core.state.live.live.has(uid)'), arg=codex_uid)
                        wait_busy(page, codex_uid, True)
                        expect(header).to_have_class(re.compile(r"\bturn-working\b"))
                        if not page.locator("#termpane").is_visible():
                            page.locator("#a-term").click()
                        wait_xterm(page, "RS_SHELL_READY")
                        # A normal input remains writable while a background
                        # terminal runs; activity is not an input veto.
                        cli = page.evaluate(js("uid => cache.get(uid).cli", 'uid => runtime.core.cache.cache.get(uid).cli'), codex_uid)
                        assert cli["input"]["state"] == "ready", cli

                    # Phone terminal resizing wraps the model/context footer;
                    # both list and detail must retain the observed busy state.
                    for width in (608, 390, 320):
                        observed = page.evaluate(js("uid => cache.get(uid).cli.observed_at", 'uid => runtime.core.cache.cache.get(uid).cli.observed_at'), codex_uid)
                        page.set_viewport_size({"width": width, "height": 780})
                        if page.locator(".mobile-back").is_visible():
                            page.locator(".mobile-back").click()
                        expect(badge(codex_uid)).to_be_visible()
                        page.locator(f'#side .item[data-uid="{codex_uid}"]').click()
                        page.wait_for_function(js("([uid, at]) => cache.get(uid)?.cli?.observed_at > at", '([uid, at]) => runtime.core.cache.cache.get(uid)?.cli?.observed_at > at'),
                                               arg=[codex_uid, observed], timeout=20000)
                        wait_busy(page, codex_uid, True)
                        expect(header).to_be_visible()
                        expect(header).to_have_class(re.compile(r"\bturn-working\b"))
                        assert header.evaluate("e => getComputedStyle(e).animationName") == "turn-pulse"
                        assert header.evaluate("""e => {
                            const r = e.getBoundingClientRect();
                            return document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2) === e;
                        }""")
                        page.locator(".mobile-back").click()
                        expect(badge(codex_uid)).to_have_class(re.compile(r"\bturn-working\b"))
                    page.locator(f'#side .item[data-uid="{codex_uid}"]').click()
                    page.emulate_media(reduced_motion="reduce")
                    assert header.evaluate("e => getComputedStyle(e).animationName") == "none"
                    assert header.evaluate("e => getComputedStyle(e).boxShadow") != "none"
                    page.emulate_media(reduced_motion="no-preference")
                    page.set_viewport_size({"width": 1280, "height": 900})
                    if not page.locator("#termpane").is_visible():
                        page.locator("#a-term").click()
                    wait_xterm(page, "RS_SHELL_READY")

                    for command in ["quoted", "quoted-quota", "draft", "background-zero", "idle"]:
                        observed = page.evaluate(js("uid => cache.get(uid).cli.observed_at", 'uid => runtime.core.cache.cache.get(uid).cli.observed_at'), codex_uid)
                        page.locator("#xterm").click()
                        page.keyboard.type(command)
                        page.keyboard.press("Enter")
                        wait_xterm(page, "RS_IDLE" if command == "idle" else "RS_SCREEN_" + command)
                        page.wait_for_function(js("([uid, at]) => cache.get(uid)?.cli?.observed_at > at", '([uid, at]) => runtime.core.cache.cache.get(uid)?.cli?.observed_at > at'),
                                               arg=[codex_uid, observed], timeout=20000)
                        wait_busy(page, codex_uid, False)
                        expect(header).not_to_have_class(TURN)
                        expect(badge(codex_uid)).not_to_have_class(TURN)
                    print("PASS Codex background terminal activity, reload, completion and quoted/draft/zero isolation")
                    assert not errors, errors
                    context.close()
                    stop_hosts(root / "host")
            finally:
                browser.close()
                stop_hosts(root / "host")
    print("turn state browser: ok")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    sys.exit(main(parser.parse_args().binary))
