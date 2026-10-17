#!/usr/bin/env python3
"""Native SDK fork/revert/compact/review through Chromium and a fake daemon.

Uses private synthetic rollouts and Unix sockets. No real CLI or model.
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace

from codex_sdk_browser import THREAD, server
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, REPO, Corpus, codex_message, codex_row, encoded
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env

FORK = "01a1sdk0-0000-7000-8000-000000000099"
TURN1 = "01a1a000-0000-7000-8000-000000000001"
TURN2 = "01a1a000-0000-7000-8000-000000000002"


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    rows = [codex_row("session_meta", {"id": THREAD, "cwd": "/synthetic/sdk-actions/项目-long-workspace",
                                      "timestamp": "2026-09-11T10:00:00Z"}),
            codex_row("turn_context", {"turn_id": TURN1}),
            codex_message("user", "第一轮 retained question with enough context"),
            codex_message("assistant", "retained answer")]
    retained = b"".join(encoded(row) for row in rows)
    rows += [codex_row("turn_context", {"turn_id": TURN2}),
             codex_message("user", "第二轮 remove this question"),
             codex_message("assistant", "remove this answer")]
    corpus.put("sdk", "codex", rows, [])
    return corpus, retained


def environment(root, binary):
    for name in ("state", "host", "ledger", "proc"):
        (root / name).mkdir(mode=0o700)
    config = root / "launcher.json"
    config.write_text(json.dumps({"schema": 2, "host_binary": str(REPO / "target/debug/ptyhost"),
        "host_dir": str(root / "host"), "profiles": [{"id": "codex-cli", "source": "codex",
        "executable": sys.executable, "args": [str(REPO / "tests/fake_model_cli.py")]}]}))
    config.chmod(0o600)
    subprocess.run([str(binary.resolve()), "--initialize-lifecycle", str(root / "ledger")],
                   check=True, capture_output=True, timeout=20)
    return {"SESSIONDOCK_PROC_ROOT": str(root / "proc"), "SESSIONDOCK_STATE_DIR": str(root / "state"),
            "SESSIONDOCK_PTYHOST_DIR": str(root / "host"), "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"),
            "SESSIONDOCK_LAUNCHER_CONFIG": str(config), "SESSIONDOCK_FILE_ROOTS": str(root),
            "SESSIONDOCK_FILE_WRITE_ROOTS": str(root)}


class ActionsServer(FakeAppServer):
    def __init__(self, socket_path, rollout, retained):
        super().__init__(socket_path, rollout, THREAD)
        self.retained = retained
        self.source_path = rollout
        self.paths = {THREAD: rollout}

    async def _handle(self, writer, message):
        method, mid, params = message.get("method"), message.get("id"), message.get("params") or {}
        if method not in {"thread/fork", "thread/revert", "thread/turns/list", "thread/compact/start", "review/start", "turn/start"}:
            if method == "thread/resume":
                self.thread_id = params["threadId"]
                self.rollout = self.paths[self.thread_id]
            return await super()._handle(writer, message)
        reply = lambda result: self.send(writer, {"id": mid, "result": result})
        if method == "thread/turns/list":
            assert params["itemsView"] == "summary" and params["sortDirection"] == "desc", params
            turns = [{"id": TURN2, "status": "completed", "items": [{"type": "userMessage", "id": "u2",
                       "content": [{"type": "text", "text": "第二轮 remove this question"}]}]}]
            if params.get("cursor"):
                assert params["cursor"] == "earlier-page"
                turns = [{"id": TURN1, "status": "completed", "items": [{"type": "userMessage", "id": "u1",
                          "content": [{"type": "text", "text": "第一轮 retained question with enough context"}]}]}]
            await reply({"data": turns, "nextCursor": None if params.get("cursor") else "earlier-page"})
        elif method == "thread/revert":
            assert params == {"threadId": THREAD, "beforeTurnId": TURN2}, params
            self.source_path.write_bytes(self.retained)
            await self.notify("thread/reverted", {})
            await reply({"thread": {"id": THREAD, "turns": [], "status": {"type": "idle"}}})
        elif method == "thread/fork":
            assert params["threadId"] == THREAD and params["excludeTurns"] is True, params
            if params.get("lastTurnId") == "reject-this-turn":
                await self.send(writer, {"id": mid, "error": {"code": -32602, "message": "turn not found"}})
                return
            branch = self.source_path.parent / ("rollout-2026-10-17T12-00-00-" + FORK + ".jsonl")
            branch_rows = [codex_row("session_meta", {"id": FORK, "forked_from_id": THREAD,
                "cwd": "/synthetic/sdk-actions/项目-long-workspace", "timestamp": "2026-10-17T12:00:00Z"}),
                codex_message("user", "分叉后的 retained question"), codex_message("assistant", "fork retained answer")]
            branch.write_bytes(b"".join(encoded(row) for row in branch_rows))
            self.paths[FORK] = branch
            await reply({"thread": {"id": FORK, "path": str(branch), "cwd": "/synthetic/sdk-actions/项目-long-workspace",
                         "createdAt": 1792238400, "status": {"type": "idle"}, "turns": []},
                         "model": "fake-sol", "reasoningEffort": "high"})
        elif method == "thread/compact/start":
            await reply({})
            await self.notify("item/started", {"turnId": "compact-turn", "item": {
                "id": "compact-item", "type": "contextCompaction"}})
        elif method == "review/start":
            assert params["delivery"] == "inline", params
            await reply({"turn": {"id": "review-turn", "status": "inProgress", "items": []}, "reviewThreadId": THREAD})
            await self.notify("item/completed", {"turnId": "review-turn", "item": {
                "id": "review-entered", "type": "enteredReviewMode", "review": "working tree review"}})
            await self.notify("item/completed", {"turnId": "review-turn", "item": {
                "id": "review-exited", "type": "exitedReviewMode", "review": "Synthetic review result: no findings"}})
        elif method == "turn/start":
            await reply({"turn": {"id": "plain-chat", "status": "inProgress", "items": []}})
            await self.notify("item/completed", {"turnId": "plain-chat", "item": {
                "id": "plain-answer", "type": "agentMessage", "text": "Plain chat accepted"}})


def exercise(browser, base, corpus, fake, width, theme, screenshots, mode="node"):
    from playwright.sync_api import expect
    label = f"{mode}-{width}-{theme}"
    print(label + ": native actions and slash commands", flush=True)
    context = browser.new_context(viewport={"width": width, "height": 844 if width < 500 else 900},
                                  color_scheme=theme, service_workers="block")
    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
    page = context.new_page()
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(base, wait_until="networkidle")
    row = page.locator('#side .item').filter(has_text="第一轮 retained question")
    source_uid = row.get_attribute("data-uid")
    row.click()
    pane = page.locator("#session-content-view.sdk-pane")
    expect(pane.locator('[data-sdk-action="revert"]')).to_be_enabled()
    expect(pane.locator(".sdk-history")).to_contain_text("remove this answer")
    box, send = pane.locator("#sdk-input"), pane.locator("#sdk-send")

    def choose_action(action):
        button = pane.locator(f'[data-sdk-action="{action}"]')
        if not button.is_visible(): pane.locator('#sdk-more').click()
        button.click()

    # The menu selects actual native turn IDs, including additional pages.
    choose_action("revert")
    dialog = page.locator("#sdk-action-dialog")
    expect(dialog).to_contain_text("不回滚工作区文件")
    expect(dialog.locator("#sdk-revert-turn")).to_have_value(TURN2)
    dialog.get_by_role("button", name="加载更早轮次").click()
    expect(dialog.locator("#sdk-revert-turn option")).to_have_count(2)
    if screenshots:
        page.screenshot(path=str(screenshots / f"actions-{label}-revert.png"))
    dialog.get_by_role("button", name="回退", exact=True).click()
    expect(dialog).to_have_count(0)
    expect(pane.locator(".sdk-history")).to_contain_text("retained answer")
    expect(pane.locator(".sdk-history")).not_to_contain_text("remove this answer")

    box.fill("/revert " + TURN2)
    send.click()
    confirmation = page.locator("dialog.app-popup[open]")
    expect(confirmation).to_contain_text("不回滚工作区文件")
    confirmation.get_by_role("button", name="取消", exact=True).click()
    expect(pane.locator('[data-sdk-action="revert"]')).to_be_enabled()
    expect(box).to_have_value("/revert " + TURN2)
    assert len(fake.methods("thread/revert")) == 1

    choose_action("compact")
    expect(pane.locator(".sdk-live")).to_contain_text("正在压缩上下文")
    expect(pane.locator(".sdk-live")).not_to_contain_text("上下文已压缩")
    progress = pane.get_by_role("progressbar", name="正在压缩上下文")
    expect(progress).to_be_visible()
    assert progress.get_attribute("aria-valuenow") is None, "native compaction has no percentage"
    if screenshots:
        page.screenshot(path=str(screenshots / f"actions-{label}-compacting.png"))
    page.reload(wait_until="networkidle")
    expect(progress).to_be_visible()
    expect(pane.locator(".sdk-live")).not_to_contain_text("上下文已压缩")
    asyncio.run_coroutine_threadsafe(fake.notify("item/completed", {
        "turnId": "compact-turn", "item": {"id": "compact-item", "type": "contextCompaction"}
    }), fake.loop).result(5)
    expect(pane.locator(".sdk-live")).to_contain_text("上下文已压缩")
    expect(progress).to_have_count(0)
    page.reload(wait_until="networkidle")
    expect(pane.locator(".sdk-live")).to_contain_text("上下文已压缩")
    expect(progress).to_have_count(0)
    assert fake.methods("thread/compact/start")[-1]["params"] == {"threadId": THREAD}
    box.fill("/compact extra")
    send.click()
    expect(pane.locator(".sdk-status")).to_contain_text("无需参数")
    expect(box).to_have_value("/compact extra")
    assert len(fake.methods("thread/compact/start")) == 1

    choose_action("review")
    dialog = page.locator("#sdk-action-dialog")
    dialog.locator("#sdk-review-target").select_option("custom")
    dialog.locator("#sdk-review-detail").fill("检查异常处理和中文路径")
    if screenshots:
        page.screenshot(path=str(screenshots / f"actions-{label}-review.png"))
    dialog.get_by_role("button", name="开始审查", exact=True).click()
    expect(pane.locator(".sdk-live")).to_contain_text("Synthetic review result: no findings")
    expect(pane.locator('[data-sdk-action="revert"]')).to_be_enabled()
    assert fake.methods("review/start")[-1]["params"]["target"] == {"type": "custom", "instructions": "检查异常处理和中文路径"}
    for command, target in [("/review uncommitted", {"type": "uncommittedChanges"}),
                            ("/review branch main", {"type": "baseBranch", "branch": "main"}),
                            ("/review commit abc123", {"type": "commit", "sha": "abc123"})]:
        before = len(fake.methods("review/start"))
        box.fill(command)
        send.click()
        expect(box).to_have_value("")
        fake.wait_for(lambda: len(fake.methods("review/start")) == before + 1)
        assert fake.methods("review/start")[-1]["params"]["target"] == target

    box.fill("please explain /compact as ordinary chat")
    send.click()
    try:
        expect(pane.locator(".sdk-live")).to_contain_text("Plain chat accepted")
    except AssertionError:
        print("chat failed:", pane.inner_text(), flush=True)
        raise
    fake.wait_for(lambda: fake.methods("turn/start"))
    assert fake.methods("turn/start")[-1]["params"]["input"][0]["text"] == "please explain /compact as ordinary chat"
    expect(pane.locator(".sdk-live")).to_contain_text("Plain chat accepted")

    # A rejected native operation preserves the user's command and session.
    box.fill("/fork reject-this-turn")
    send.click()
    expect(pane.locator(".sdk-status")).to_contain_text("turn not found")
    expect(box).to_have_value("/fork reject-this-turn")
    assert page.evaluate("S.sel") == source_uid

    box.fill("/fork " + TURN1)
    send.click()
    page.wait_for_function("uid => S.sel !== uid", arg=source_uid)
    expect(page.locator(".sdk-history")).to_contain_text("fork retained answer")
    expect(page.locator("#sdk-model")).to_have_value("fake-sol")
    assert fake.methods("thread/fork")[-1]["params"]["lastTurnId"] == TURN1
    new_uid = page.evaluate("S.sel")
    assert new_uid.startswith("codex:") and new_uid != source_uid
    assert ("~" in new_uid) == (mode == "hub"), "fork lost its Hub namespace"
    expect(page.locator(f'#side .item[data-uid="{source_uid}"]')).to_have_count(1)
    expect(page.locator(f'#side .item[data-uid="{new_uid}"]')).to_have_count(1)
    if width < 500:
        page.get_by_role("button", name="返回会话列表", exact=True).click()
    expect(page.locator(f'#side .item[data-uid="{source_uid}"]')).to_be_visible()
    expect(page.locator(f'#side .item[data-uid="{new_uid}"]')).to_be_visible()
    if screenshots:
        page.screenshot(path=str(screenshots / f"actions-{label}-fork.png"))
    page.locator(f'#side .item[data-uid="{source_uid}"]').click()
    expect(page.locator(".sdk-history")).to_contain_text("retained answer")
    if width < 500:
        page.get_by_role("button", name="返回会话列表", exact=True).click()
    page.locator(f'#side .item[data-uid="{new_uid}"]').click()
    page.reload(wait_until="networkidle")
    expect(page.locator(".sdk-history")).to_contain_text("fork retained answer")
    assert page.evaluate("S.sel") == new_uid
    expect(page.locator(f'#side .item[data-uid="{source_uid}"]')).to_have_count(1)
    expect(page.locator(f'#side .item[data-uid="{new_uid}"]')).to_have_count(1)
    assert not errors, errors
    context.close()


def main():
    from playwright.sync_api import sync_playwright
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    parser.add_argument("--hub-binary", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sd-sdk-actions-") as temporary, sync_playwright() as playwright:
        options = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            options["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**options)
        try:
            for mode, width, theme in [("node", 1280, "light"), ("node", 390, "dark"),
                                      ("node", 1280, "dark"), ("node", 390, "light"),
                                      ("hub", 1280, "light")]:
                root = Path(temporary) / f"{mode}-{width}-{theme}"
                root.mkdir()
                corpus, retained = build(root)
                with ActionsServer(root / "app.sock", corpus.paths["sdk"], retained) as fake:
                    port = free_port()
                    env = environment(root, args.binary)
                    if mode == "hub":
                        env.update(node_env(root, port, "127.0.0.0/8"))
                    with server(corpus, args.binary, root / "app.sock", env) as base:
                        if mode == "node":
                            exercise(browser, base, corpus, fake, width, theme, args.screenshots)
                        else:
                            hub_root = root / "hub"
                            hub_root.mkdir()
                            hub_binary = args.hub_binary or args.binary.with_name("sessiondock-hub")
                            hub = Hub(hub_binary.resolve(), hub_root, [SimpleNamespace(name="SdkNode", token=TOKEN, port=port)])
                            hub.start()
                            try:
                                exercise(browser, f"http://127.0.0.1:{hub.port}", corpus, fake,
                                         width, theme, args.screenshots, mode)
                            finally:
                                hub.stop()
        finally:
            browser.close()
    print("PASS SDK native actions: paginated revert/history refresh, compact event, inline review targets/results, "
          "slash parsing/errors, fork identity/navigation/reload; desktop/mobile light/dark and real Hub namespace")


if __name__ == "__main__":
    main()
