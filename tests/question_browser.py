#!/usr/bin/env python3
"""Claude question cards answered from the page reach the right native option.

The fake Claude CLI models the AskUserQuestion menu measured on Claude Code
2.1.283 (Up wraps to "Type something.", Down stops on "Chat about this", the
text row swallows digits and Left/Right). The real `claude-hook` writes each
card; the page clicks options and submits; the fake records what the key
sequence actually selected. Covers the two-question form from its default
state (BUG-20260926-005613-ae8a19: option 1 twice landed on option 3 twice),
the form with every cursor parked on a row that eats keys, and a single
question whose cursor sits on the text row. A fork's AskUserQuestion
PreToolUse (payload with `agent_id`, BUG-20260928-143049-61a198) never opens a
native dialog and must not put a card on the page.
"""

import json
import os
import subprocess
import tempfile
import time
from pathlib import Path
from private_hosts import private_hosts
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright, expect

from history_fixtures import REPO, BINARY, Corpus, isolated_server
from send_browser import create_claude, initialize
from popups import on_popup  # noqa: E402

DEBUG = {}
FORM = [
    {"header": "清理范围", "question": "v8 这批怎么清理？",
     "options": ["全部删除、不留痕", "台账保留、标注作废", "只删文件、台账保留"]},
    {"header": "V2 训练", "question": "V2 怎么处理？",
     "options": ["立即停止并一起删除", "停止但保留 checkpoint", "让它跑完"]},
]
SINGLE = [{"header": "宠物", "question": "选哪个？", "options": ["猫", "狗", "鱼"]}]
DESCRIBED = [
    {"header": "判定标准", "question": "使用哪种判定标准？（δ 为事前门槛）",
     "options": ["点估计≥δ (Recommended)", "点估计>0", "综合判断"],
     "descriptions": ["达到事前规定的门槛，但区间仍含零。\n保留不确定性，等待后续验证。",
                      "覆盖所有正向点估计。\n波动可能改变方向，需要记录说明。",
                      "综合多项证据判断。\n写明理由，供其他人复核。"]},
    {"header": "作用范围", "question": "修改应用于哪些层？",
     "options": ["台账与展示 (Recommended)", "只改台账", "只改展示"],
     "descriptions": ["同步修改台账和页面展示。\n既能保存也能筛选。", "只保存新的判定值。", "只更新列表显示。"]},
]


def hook(state, sid, event, tool, questions, agent_id=None):
    payload = {"hook_event_name": event, "session_id": sid, "tool_name": "AskUserQuestion",
               "tool_use_id": tool, "tool_input": {"questions": [
                   {"header": q["header"], "question": q["question"], "multiSelect": q.get("multiple", False),
                    "options": [{"label": o, "description": q.get("descriptions", [""] * len(q["options"]))[i]}
                                for i, o in enumerate(q["options"])]}
                   for q in questions]}}
    if agent_id is not None:
        payload["agent_id"] = agent_id
    done = subprocess.run([str(BINARY), "claude-hook", "--state-dir", str(state)],
                          input=json.dumps(payload).encode(), capture_output=True, timeout=20)
    assert done.returncode == 0, done.stderr.decode()


def answer(page, root, sid, tool, questions, choices, menu, *, layout=False):
    spec = root / "question.json"
    outcome = Path(str(spec) + ".answers")
    outcome.unlink(missing_ok=True)
    spec.write_text(json.dumps({"questions": [{"question": q["question"], "options": q["options"],
                                              "descriptions": q.get("descriptions")}
                                              for q in questions], **menu}))
    if layout:
        # Before the hook arrives, CHECK must still show the current screen
        # page with each wrapped description attached to its own option.
        deadline = time.monotonic() + 5
        while spec.exists():
            assert time.monotonic() < deadline
            page.wait_for_timeout(50)
        page.wait_for_timeout(100)
        check = page.evaluate('async () => await probeComposerInput(composerUid)')
        visible = check['prompt']['questions'][0]
        assert visible['question'] == questions[0]['question'], visible
        assert [o.get('description') for o in visible['options'][:3]] == questions[0]['descriptions'], visible
        expect(page.locator('#composer-question')).to_be_visible()
        expect(page.locator('#composer-question .question-text')).to_have_text(questions[0]['question'])
        expect(page.locator('#composer-question .question-option small')).to_have_count(3)
        # A matching but non-answerable history form must leave the native
        # controls available, including its text field.
        hook(root / 'state', sid, 'PreToolUse', tool + '-multiple', [{**questions[0], 'multiple': True}])
        expect(page.locator(f'#msgs .live-question[data-call-id="{tool}-multiple"] .question-multiple')).to_be_visible(timeout=20000)
        expect(page.locator('#composer-question')).to_be_visible()
        page.locator('#composer-question .question-text-input').fill('custom answer')
    hook(root / "state", sid, "PreToolUse", tool, questions)
    card = page.locator(f'#msgs .msg.question.live-question[data-call-id="{tool}"]')
    expect(card.locator(".question-item")).to_have_count(len(questions), timeout=20000)
    if layout:
        expect(page.locator('#composer-question')).to_be_hidden()
        expect(page.locator('.live-question:visible')).to_have_count(1)
        expect(card.locator('.question-option small')).to_have_count(sum(len(q['descriptions']) for q in questions))
    for index, choice in enumerate(choices):
        card.locator(f'[data-question-index="{index}"][data-question-option="{choice}"]').click()
        if layout and len(questions) > 1:
            page.evaluate('async () => await probeComposerInput(composerUid)')
            expect(card.locator(f'[data-question-index="{index}"][data-question-option="{choice}"]')).to_have_attribute('aria-pressed', 'true')
            expect(page.locator('.live-question:visible')).to_have_count(1)
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
    with tempfile.TemporaryDirectory(prefix="sessiondock-question-") as temporary, private_hosts(Path(temporary)):
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
        with sync_playwright() as playwright:
            options = {"headless": True}
            if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
                options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
            browser = playwright.chromium.launch(**options)
            try:
                with isolated_server(corpus, BINARY, host_dir=root / "host", lifecycle_dir=root / "ledger",
                                     launcher_config=configuration, 
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
                    on_popup(page, on_dialog)
                    page.goto(base, wait_until="networkidle")
                    DEBUG.update(dialogs=dialogs)
                    receipt = create_claude(page, base, root / "work", open_terminal=False)
                    sid = receipt["declared_sid"]
                    page.fill("#cinput", "hello")
                    with page.expect_response(lambda r: urlsplit(r.url).path == "/api/session/conversation/send", timeout=20000) as sent:
                        page.locator("#csend").click()
                    assert sent.value.status == 200, sent.value.text()
                    page.wait_for_function("S.sel && !S.sel.startsWith('tmux:')", timeout=20000)

                    # A post-turn fork (prompt suggestion) asks: Claude refuses the
                    # tool without a Post hook and the terminal never shows it.
                    hook(root / "state", sid, "PreToolUse", "toolu-fork", SINGLE, agent_id="a1b2c3d4e5f6a7b8")
                    deadline = time.monotonic() + 3
                    while time.monotonic() < deadline:
                        assert page.locator("#msgs .msg.question.live-question").count() == 0, \
                            "a fork's question showed as a live card"
                        page.wait_for_timeout(200)
                    assert not (root / "state/claude-prompts" / f"{sid}.json").exists()
                    # The report: option 1 on both questions from the fresh menu.
                    answer(page, root, sid, "toolu-form-default", FORM, [0, 0], {})
                    # Every cursor parked where keys are eaten or wrap: Review on
                    # Cancel, question 1 on the text row, question 2 on the chat row.
                    answer(page, root, sid, "toolu-form-parked", FORM, [2, 1],
                           {"tab": 2, "cursors": [3, 4], "review": 1})
                    # A single question whose cursor sits on the text row.
                    answer(page, root, sid, "toolu-single-text-row", SINGLE, [1], {"cursors": [3]})
                    # Report regression: descriptions below numbered rows,
                    # and a separator before Chat. CHECK and hook are two
                    # projections of the same question, not two answer forms.
                    for width, height in [(1723, 1039), (390, 844)]:
                        page.set_viewport_size({'width': width, 'height': height})
                        if width == 390:
                            page.evaluate('showMobileDetail()')
                        answer(page, root, sid, f"toolu-described-{width}", DESCRIBED, [0, 1], {}, layout=True)
                        # A single native question has a checkbox header, no
                        # arrows/Submit tab, and an Up/Down navigation footer.
                        answer(page, root, sid, f"toolu-single-described-{width}", DESCRIBED[:1], [0], {}, layout=True)
                    assert not errors and not dialogs, (errors, dialogs)
            finally:
                browser.close()
    print("PASS question_browser: exact native answers from default/parked cursors; desktop/mobile "
          "wrapped descriptions, delayed hooks, one visible form and retained selections")


if __name__ == "__main__":
    main()
