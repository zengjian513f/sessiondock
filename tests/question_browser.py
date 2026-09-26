#!/usr/bin/env python3
"""Claude question cards answered from the page reach the right native option.

The fake Claude CLI models the AskUserQuestion menu measured on Claude Code
2.1.283 (Up wraps to "Type something.", Down stops on "Chat about this", the
text row swallows digits and Left/Right). The real `claude-hook` writes each
card; the page clicks options and submits; the fake records what the key
sequence actually selected. Covers the two-question form from its default
state (BUG-20260926-005613-ae8a19: option 1 twice landed on option 3 twice),
the form with every cursor parked on a row that eats keys, and a single
question whose cursor sits on the text row.
"""
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright, expect

from history_parity import REPO, BINARY, Corpus, isolated_server
from send_browser import create_claude, initialize

DEBUG = {}
FORM = [
    {"header": "清理范围", "question": "v8 这批怎么清理？",
     "options": ["全部删除、不留痕", "台账保留、标注作废", "只删文件、台账保留"]},
    {"header": "V2 训练", "question": "V2 怎么处理？",
     "options": ["立即停止并一起删除", "停止但保留 checkpoint", "让它跑完"]},
]
SINGLE = [{"header": "宠物", "question": "选哪个？", "options": ["猫", "狗", "鱼"]}]


def hook(state, sid, event, tool, questions):
    payload = {"hook_event_name": event, "session_id": sid, "tool_name": "AskUserQuestion",
               "tool_use_id": tool, "tool_input": {"questions": [
                   {"header": q["header"], "question": q["question"], "multiSelect": False,
                    "options": [{"label": o, "description": ""} for o in q["options"]]}
                   for q in questions]}}
    done = subprocess.run([str(BINARY), "claude-hook", "--state-dir", str(state)],
                          input=json.dumps(payload).encode(), capture_output=True, timeout=20)
    assert done.returncode == 0, done.stderr.decode()


def answer(page, root, sid, tool, questions, choices, menu):
    spec = root / "question.json"
    outcome = Path(str(spec) + ".answers")
    outcome.unlink(missing_ok=True)
    spec.write_text(json.dumps({"questions": [{"question": q["question"], "options": q["options"]}
                                              for q in questions], **menu}))
    hook(root / "state", sid, "PreToolUse", tool, questions)
    card = page.locator(f'#msgs .msg.question.live-question[data-call-id="{tool}"]')
    expect(card.locator(".question-item")).to_have_count(len(questions), timeout=20000)
    for index, choice in enumerate(choices):
        card.locator(f'[data-question-index="{index}"][data-question-option="{choice}"]').click()
    if len(questions) > 1:
        card.locator(".question-submit").click()
    deadline = time.monotonic() + 15
    while not outcome.exists():
        assert time.monotonic() < deadline, ("the fake CLI never closed its menu", spec.exists(), DEBUG)
        page.wait_for_timeout(100)  # keeps the context route handler serviced
    got = json.loads(outcome.read_text())
    want = {q["question"]: q["options"][c] for q, c in zip(questions, choices)}
    assert got == {"outcome": "submitted", "answers": want}, (got, want)
    hook(root / "state", sid, "PostToolUse", tool, questions)


def main():
    if os.name != "posix":
        raise SystemExit("Question-card browser acceptance currently requires POSIX.")
    with tempfile.TemporaryDirectory(prefix="sessiondock-question-") as temporary:
        root = Path(temporary).resolve()
        for name in ["host", "work", "work/claude-area", "ledger", "delivery", "state", "bin", "home", "claude", "codex", "grok"]:
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
                 "args": ["--reply"], "new_args": ["--session-id", "{session_id}"],
                 "resume_args": ["--resume", "{sid}"],
                 "env": {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TERM": "xterm-256color",
                         "LANG": "C.UTF-8", "SESSIONDOCK_TEST_CLAUDE_ROOT": str(root / "claude"),
                         "SESSIONDOCK_TEST_CLAUDE_QUESTION": str(root / "question.json")}}]}))
        initialize("--initialize-lifecycle", root / "ledger")
        initialize("--initialize-delivery", root / "delivery")
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration, delivery_dir=root / "delivery",
                                     state_dir=root / "state") as (base, _):
                    errors = []
                    context = browser.new_context(viewport={"width": 390, "height": 844}, service_workers="block")
                    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
                    page = context.new_page()
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    dialogs = []

                    def on_dialog(dialog):
                        dialogs.append(dialog.message)
                        dialog.accept()
                    page.on("dialog", on_dialog)
                    page.goto(base, wait_until="networkidle")
                    DEBUG.update(dialogs=dialogs)
                    receipt = create_claude(page, base, root / "work", open_terminal=False)
                    sid = receipt["declared_sid"]
                    page.fill("#cinput", "hello")
                    with page.expect_response(lambda r: urlsplit(r.url).path == "/api/session/conversation/send", timeout=20000) as sent:
                        page.locator("#csend").click()
                    assert sent.value.status == 200, sent.value.text()
                    page.wait_for_function("S.sel && !S.sel.startsWith('tmux:')", timeout=20000)

                    # The report: option 1 on both questions from the fresh menu.
                    answer(page, root, sid, "toolu-form-default", FORM, [0, 0], {})
                    # Every cursor parked where keys are eaten or wrap: Review on
                    # Cancel, question 1 on the text row, question 2 on the chat row.
                    answer(page, root, sid, "toolu-form-parked", FORM, [2, 1],
                           {"tab": 2, "cursors": [3, 4], "review": 1})
                    # A single question whose cursor sits on the text row.
                    answer(page, root, sid, "toolu-single-text-row", SINGLE, [1], {"cursors": [3]})
                    assert not errors and not dialogs, (errors, dialogs)
            finally:
                browser.close()
    print("PASS question_browser: two-question form from the default and parked cursors, and a single "
          "question from the text row, each select exactly the clicked options in the fake Claude menu")


if __name__ == "__main__":
    main()
