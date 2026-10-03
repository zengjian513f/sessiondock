#!/usr/bin/env python3
"""A rewind made in Claude's own TUI reaches the conversation view.

Claude Code 2.1.284 keeps a double-Esc "restore conversation" in memory only:
the JSONL is untouched until the next input, the rewound input returns to the
editor and the transcript is redrawn up to it (BUG-20260929-075643-bc6def).
Against the fake Claude CLI through the real legacy UI this checks that the
server follows that screen (the rewound input and its answer leave the view,
with a one-line notice, and the editor text is not called Esc-returned), that
the next input settles it natively, and that recalling an answered input that
is still on screen neither rewinds the view nor claims an Esc return.
"""
from browser_runtime import js
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright, expect

from history_parity import REPO, BINARY, Corpus, isolated_server
from send_browser import SETTINGS, create_claude, initialize, wait_history

FIRST = "Please list the three largest request sources of the last week with their context sizes."
SECOND = "Now explain why the orchestrator session never compacts even when it passes the window."
THIRD = "Instead describe only the smallest safe change to the automatic compaction threshold."
NOTICE = "已同步终端里的回滚，显示到回滚点为止"


def user_bubbles(page, text):
    return page.locator("#msgs .msg[data-role=user]:not(.queued-send)").filter(has_text=text)


def main():
    if os.name != "posix":
        raise SystemExit("CLI rewind browser acceptance requires POSIX.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-cli-rewind-") as temporary:
        root = Path(temporary).resolve()
        for name in ["host", "work", "work/claude-area", "ledger", "state", "bin", "home", "claude", "codex", "grok"]:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        python = Path(subprocess.check_output(["/bin/sh", "-c", "command -v python3"]).decode().strip()).resolve()
        wrapper = root / "bin/fake-claude"
        wrapper.write_text("#!/bin/sh\nexec %s %s \"$@\"\n" % (python, REPO / "tests/fake_claude_cli.py"))
        wrapper.chmod(0o700)
        configuration = root / "launcher.json"
        configuration.touch(mode=0o600)
        configuration.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
            "host_dir": str(root / "host"), "adapters": [], "profiles": [
                {"id": "claude-cli-v1", "source": "claude", "executable": str(wrapper),
                 "args": ["--settings", SETTINGS, "--reply"], "new_args": ["--session-id", "{session_id}"],
                 "resume_args": ["--resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TERM": "xterm-256color",
                         "LANG": "C.UTF-8", "SESSIONDOCK_TEST_CLAUDE_ROOT": str(root / "claude"),
                         "SESSIONDOCK_TEST_CLAUDE_REWIND": str(root / "rewind")}}]}))
        initialize("--initialize-lifecycle", root / "ledger")
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration, state_dir=root / "state",
                                     file_roots=(root / "work",), file_write_roots=(root / "work",)) as (base, _):
                    errors = []
                    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
                    context.route("**/*", lambda route: route.continue_()
                                  if route.request.url.startswith(base + "/") else route.abort())
                    page = context.new_page()
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(base, wait_until="networkidle")
                    receipt = create_claude(page, base, root / "work", open_terminal=False)
                    page.wait_for_function(js("composerUid && !composerDraft().loading && takenOver(composerUid)", 'runtime.composer.composerUid && !runtime.composer.composerDraft().loading && runtime.terminal.takenOver(runtime.composer.composerUid)'))

                    def send(text):
                        expect(page.locator("#csend")).to_be_enabled(timeout=10000)
                        page.fill("#cinput", text)
                        with page.expect_response(lambda r: urlsplit(r.url).path == "/api/session/conversation/send",
                                                  timeout=20000) as response:
                            page.locator("#csend").click()
                        assert response.value.status == 200, response.value.text()
                        wait_history(page, "OK: " + text)

                    send(FIRST)
                    page.wait_for_function(js("S.sel && !S.sel.startsWith('tmux:')", "runtime.core.state.selection.sel && !runtime.core.state.selection.sel.startsWith('tmux:')"), timeout=20000)
                    native = page.evaluate(js("S.sel", 'runtime.core.state.selection.sel'))
                    send(SECOND)
                    jsonl = root / "claude/project-history" / f"{receipt['declared_sid']}.jsonl"
                    before = jsonl.read_bytes()

                    # The TUI rewinds to before SECOND: the view follows it without
                    # any native record, and the editor text is no Esc return.
                    (root / "rewind").touch()
                    page.evaluate(js("uid => sendToSession(null, ['Escape'], uid)", "uid => runtime.composer.sendToSession(null, ['Escape'], uid)"), native)
                    expect(page.locator("#timeline-pin-notice")).to_have_text(NOTICE, timeout=15000)
                    expect(page.locator("#timeline-pin-notice")).to_have_attribute("data-cli", "true")
                    expect(page.locator("#timeline-pin-clear")).to_have_count(0)
                    expect(user_bubbles(page, SECOND)).to_have_count(0)
                    expect(page.locator("#msgs .msg").filter(has_text="OK: " + SECOND)).to_have_count(0)
                    expect(user_bubbles(page, FIRST)).to_have_count(1)
                    expect(page.locator("#msgs .msg").filter(has_text="OK: " + FIRST)).to_have_count(1)
                    expect(page.locator(".returned-to-cli")).to_have_count(0)
                    status = page.locator("#composer-input-status")
                    expect(status).to_contain_text("终端输入框里已有未发送的文字", timeout=10000)
                    assert "Esc" not in status.inner_text(), status.inner_text()
                    assert jsonl.read_bytes() == before, "a TUI rewind writes nothing native"
                    (root / "rewind").unlink()
                    # A reload keeps the synced view (the pin is server state).
                    page.reload(wait_until="networkidle")
                    page.locator(f'#side .item[data-uid="{native}"]').click()
                    expect(page.locator("#timeline-pin-notice")).to_have_text(NOTICE, timeout=15000)
                    expect(user_bubbles(page, SECOND)).to_have_count(0)

                    # The next input settles it natively: it chains below FIRST's
                    # answer, SECOND stays off the timeline, the notice goes away.
                    page.evaluate(js("uid => sendToSession(null, ['C-u'], uid)", "uid => runtime.composer.sendToSession(null, ['C-u'], uid)"), native)
                    send(THIRD)
                    expect(page.locator("#timeline-pin-notice")).to_have_count(0, timeout=15000)
                    expect(user_bubbles(page, SECOND)).to_have_count(0)
                    expect(user_bubbles(page, THIRD)).to_have_count(1)
                    records = [json.loads(line) for line in jsonl.read_text().splitlines()]
                    by_text = {r["message"]["content"] if isinstance(r["message"]["content"], str)
                               else r["message"]["content"][0]["text"]: r for r in records}
                    assert by_text[THIRD]["parentUuid"] == by_text["OK: " + FIRST]["uuid"], records

                    # Recalling THIRD while its prompt is still on screen is no
                    # rewind and no Esc return, although SEND retired that text.
                    page.evaluate(js("([uid, text]) => sendToSession(null, [text], uid)", '([uid, text]) => runtime.composer.sendToSession(null, [text], uid)'), [native, THIRD])
                    expect(status).to_contain_text("终端输入框里已有未发送的文字", timeout=10000)
                    deadline = time.monotonic() + 4
                    while time.monotonic() < deadline:
                        assert page.locator(".returned-to-cli").count() == 0
                        assert page.locator("#timeline-pin-notice").count() == 0
                        page.wait_for_timeout(200)
                    expect(user_bubbles(page, THIRD)).to_have_count(1)
                    assert "Esc" not in status.inner_text(), status.inner_text()
                    assert not errors, errors
                    print("PASS rewind_cli_browser: TUI rewind synced, next input settles it, recall is no rewind")
            finally:
                browser.close()


if __name__ == "__main__":
    main()
