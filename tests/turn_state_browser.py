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
although the transcript says the turn is complete; with a quiet screen, a
finished Claude turn whose Monitor watch or backgrounded command still runs
keeps turning until its end notice or TaskStop, and a watchdog Monitor that
tails an ended command's output file no longer holds it. No model binary,
native CLI home or production host is touched.
"""
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import uuid
from playwright.sync_api import sync_playwright, expect
from history_parity import REPO, BINARY, Corpus, claude_row, codex_row, codex_message, encoded, isolated_server

# Fresh ids per run: a host left by an aborted run must not look like an
# outside instance of the next run's sessions.
CLAUDE_SID = str(uuid.uuid4())
CODEX_SID = str(uuid.uuid4())
FAKE_CLI = """#!/bin/sh
exec /bin/sh -c 'stty -echo 2>/dev/null; printf "RS_SHELL_READY\\n"; while IFS= read -r line; do case "$line" in busy) printf "Working (3s - esc to interrupt)\\n" ;; idle) printf "\\033[2J\\033[HRS_IDLE\\n" ;; *) printf "RS_INPUT_OK\\n" ;; esac; done'
"""
TURN = re.compile(r"\bturn-(working|waiting)\b")


def wait_xterm(page, text):
    page.wait_for_function(
        "text => [...T.views.values()].some(v => v.term?.buffer?.active && Array.from({length: v.term.buffer.active.length},"
        " (_, i) => v.term.buffer.active.getLine(i)?.translateToString() || '').join('\\n').includes(text))",
        arg=text, timeout=15000)


def wait_busy(page, uid, busy):
    try:
        page.wait_for_function("([uid, busy]) => cache.get(uid)?.cli?.instance?.busy === busy",
                               arg=[uid, busy], timeout=20000)
    except Exception:
        raise AssertionError(("cli", busy, page.evaluate("uid => cache.get(uid)?.cli ?? null", uid))) from None


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


def main():
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
        initialized = subprocess.run([str(BINARY), "--initialize-lifecycle", str(root / "ledger")],
            cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        assert initialized.returncode == 0, initialized.stderr.decode()
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
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
                        page.wait_for_function("uid => S.live.has(uid)", arg=uid, timeout=20000)
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
                    expect(badge(codex_uid)).to_have_class(re.compile(r"\bturn-working\b"), timeout=20000)
                    append(codex_path, codex_row("response_item", {"type": "function_call", "name": "request_user_input",
                        "call_id": "call_ask", "arguments": json.dumps({"questions": [{"question": "Go?",
                        "options": [{"label": "Yes"}]}]})}, 3))
                    expect(badge(codex_uid)).to_have_class(re.compile(r"\bturn-waiting\b"), timeout=20000)
                    append(codex_path,
                        codex_row("response_item", {"type": "function_call_output", "call_id": "call_ask", "output": "Yes"}, 4),
                        codex_row("event_msg", {"type": "task_complete", "turn_id": "t1"}, 5))
                    expect(badge(codex_uid)).not_to_have_class(TURN, timeout=20000)
                    assert row_turn(opener, base, codex_uid, codex_path.stat().st_size) == "idle"

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
                    page.wait_for_function("S.agent === 'a1b2c3d4e5f6a7b8c'")
                    expect(page.locator("#a-term")).to_have_attribute("aria-label", re.compile("子代理"))
                    page.evaluate("renderTakeoverBtn()")
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
                    cli = page.evaluate("uid => cache.get(uid).cli", codex_uid)
                    assert cli["instance"] == {"running": True, "busy": False}, cli
                    assert not errors, errors
                    context.close()
                    stop_hosts(root / "host")
            finally:
                browser.close()
                stop_hosts(root / "host")
    print("turn state browser: ok")


if __name__ == "__main__":
    sys.exit(main())
