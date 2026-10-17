#!/usr/bin/env python3
"""SDK modes, policies, goals and native shell controls through Chromium.

Private fake app-server only: no shell, model, CLI, account or production data.
Exercises desktop/mobile, light/dark, actual controls and slash commands, live
goal notifications, failures preserving drafts, reload and daemon reconnect.
"""
from __future__ import annotations

import argparse
import asyncio
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build, server
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY


class ControlsServer(FakeAppServer):
    def __init__(self, *args):
        super().__init__(*args)
        self.goal = None
        self.fail_read = True
        self.settings.update(approvalPolicy="on-request", sandboxPolicy={"type": "readOnly", "networkAccess": False},
                             collaborationMode={"mode": "default", "settings": {"model": "fake-sol", "reasoning_effort": "high", "developer_instructions": None}})

    async def _handle(self, writer, message):
        method, mid, params = message.get("method"), message.get("id"), message.get("params") or {}
        async def reply(result):
            await self.send(writer, {"id": mid, "result": result})
        if method == "thread/resume":
            await reply({"thread": {"id": self.thread_id, "status": {"type": "idle"}, "turns": []},
                         "model": self.settings["model"], "reasoningEffort": self.settings["effort"],
                         "approvalPolicy": self.settings["approvalPolicy"], "sandbox": self.settings["sandboxPolicy"],
                         "collaborationMode": self.settings["collaborationMode"]})
        elif method == "thread/settings/update":
            self.settings.update({key: params[key] for key in ("approvalPolicy", "sandboxPolicy", "collaborationMode") if key in params})
            await self.notify("thread/settings/updated", {"threadSettings": self.settings.copy()})
            await reply({})
        elif method == "thread/goal/get":
            if self.fail_read:
                await self.send(writer, {"id": mid, "error": {"code": -32000, "message": "Synthetic goal read unavailable"}})
            else:
                await reply({"goal": self.goal})
        elif method == "thread/goal/set":
            assert params["origin"] == "user", params
            if self.goal is None:
                assert params.get("objective"), "new goal needs objective"
                self.goal = {"threadId": self.thread_id, "objective": "", "status": "active", "tokensUsed": 0,
                             "tokenBudget": None, "timeUsedSeconds": 0, "createdAt": 1792224000, "updatedAt": 1792224000}
            self.goal.update({key: params[key] for key in ("objective", "status", "tokenBudget") if key in params})
            await self.notify("thread/goal/updated", {"goal": self.goal.copy()})
            await reply({"goal": self.goal.copy()})
        elif method == "thread/goal/clear":
            assert params["origin"] == "user", params
            self.goal = None
            await self.notify("thread/goal/cleared", {})
            await reply({"cleared": True})
        elif method == "thread/shellCommand":
            if params["command"] == "synthetic-error":
                await self.send(writer, {"id": mid, "error": {"code": -32000, "message": "Synthetic shell failed"}})
                return
            await reply({})
            command = {"id": f"shell-{len(self.methods(method))}", "type": "commandExecution", "command": params["command"],
                       "status": "inProgress", "aggregatedOutput": "", "exitCode": None, "durationMs": None}
            await self.notify("item/started", {"turnId": "shell-turn", "item": command})
            await self.notify("item/commandExecution/outputDelta", {"turnId": "shell-turn", "itemId": command["id"], "delta": "synthetic shell output\n"})
            await self.notify("item/completed", {"turnId": "shell-turn", "item": {**command, "status": "completed", "aggregatedOutput": "synthetic shell output\n", "exitCode": 0, "durationMs": 10}})
        else:
            await super()._handle(writer, message)

    def external_goal_usage(self):
        async def update():
            self.goal["tokensUsed"] = 123
            await self.notify("thread/goal/updated", {"goal": self.goal.copy()})
        asyncio.run_coroutine_threadsafe(update(), self.loop).result(timeout=5)

    def disconnect(self):
        async def close():
            for writer in list(self.clients):
                writer.close()
                await writer.wait_closed()
        asyncio.run_coroutine_threadsafe(close(), self.loop).result(timeout=5)


def exercise(browser, base, corpus, fake, width, theme, screenshots):
    label = f"{width}-{theme}"
    print(label + ": mode / policies / goal / shell / failures / reconnect", flush=True)
    context = browser.new_context(viewport={"width": width, "height": 844 if width < 500 else 900}, color_scheme=theme, service_workers="block")
    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
    page = context.new_page()
    page.set_default_timeout(10000)
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(base, wait_until="domcontentloaded")
    page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
    expect(page.locator("#sdk-input")).to_be_visible()
    mode, box = page.locator("#sdk-mode"), page.locator("#sdk-input")
    expect(mode).to_have_value(fake.settings["collaborationMode"]["mode"])
    starts = len(fake.methods("turn/start"))
    mode.select_option("plan")
    page.wait_for_function("CodexSdkView.summary?.settings?.collaborationMode?.mode === 'plan'")
    expect(mode).to_have_value("plan")
    changed = fake.methods("thread/settings/update")[-1]["params"]
    assert changed["collaborationMode"]["settings"] == {"model": "fake-sol", "reasoning_effort": "high", "developer_instructions": None}, changed
    assert "approvalPolicy" not in changed and "sandboxPolicy" not in changed

    def send(text):
        box.fill(text)
        page.locator("#sdk-send").click()

    send("/default")
    expect(mode).to_have_value("default")
    expect(box).to_have_value("")
    page.locator("#sdk-controls").click()
    dialog = page.locator("#sdk-controls-dialog")
    expect(dialog).to_be_visible()
    expect(dialog.locator('#sdk-shell-command, #sdk-shell-run')).to_have_count(0)
    expect(dialog).not_to_contain_text('直接执行命令')
    if fake.fail_read:
        expect(dialog.locator("#sdk-goal-current")).to_contain_text("读取目标失败")
        expect(dialog.locator("#sdk-goal-save")).to_be_disabled()
        fake.fail_read = False
        dialog.locator("#sdk-goal-refresh").click()
    expect(dialog.locator("#sdk-goal-current")).to_have_text("尚未设置目标")
    dialog.locator("#sdk-control-approval").select_option("never")
    dialog.locator("#sdk-control-sandbox").select_option("workspaceWrite")
    dialog.locator("#sdk-permission-save").click()
    expect(dialog.locator("#sdk-controls-result")).to_contain_text("权限设置已应用")
    expect(dialog.locator('#sdk-control-approval option').first).to_contain_text("当前：不请求审批")
    assert fake.settings["sandboxPolicy"] == {"type": "workspaceWrite"}

    objective = "完成 SDK 目标与权限的浏览器验收，并保留当前共享会话 / realistic-long-project-name"
    dialog.locator("#sdk-goal-objective").fill(objective)
    dialog.locator("#sdk-goal-budget").fill("2400")
    dialog.locator("#sdk-goal-save").click()
    expect(dialog.locator("#sdk-goal-current")).to_contain_text(objective)
    assert fake.goal["tokenBudget"] == 2400 and fake.goal["status"] == "active"
    # External shared-TUI progress updates the summary without clobbering edits.
    dialog.locator("#sdk-goal-objective").fill("未保存的目标修改")
    fake.external_goal_usage()
    expect(dialog.locator("#sdk-goal-current")).to_contain_text("123 / 2400 tokens")
    expect(dialog.locator("#sdk-goal-objective")).to_have_value("未保存的目标修改")
    if screenshots:
        page.screenshot(path=str(screenshots / (label + "-settings.png")))
    dialog.locator(".modal-close").click()

    send("/goal pause")
    expect(box).to_have_value("")
    assert fake.goal["objective"] == objective and fake.goal["status"] == "paused"
    send("/goal resume")
    expect(box).to_have_value("")
    assert fake.goal["status"] == "active"
    for empty in ["!", "/shell"]:
        send(empty)
        expect(page.locator(".sdk-status")).to_contain_text("用法")
        expect(box).to_have_value(empty)
        expect(page.locator("#sdk-controls-dialog")).to_have_count(0)
    send("!printf 'synthetic output' | cat")
    expect(box).to_have_value("")
    expect(page.locator(".sdk-live")).to_contain_text("synthetic shell output")
    assert fake.methods("thread/shellCommand")[-1]["params"]["command"] == "printf 'synthetic output' | cat"
    send("/shell synthetic-error")
    expect(page.locator(".sdk-status")).to_contain_text("Synthetic shell failed")
    expect(box).to_have_value("/shell synthetic-error")
    send("/unknown-control")
    expect(page.locator(".sdk-status")).to_contain_text("不支持的命令")
    expect(box).to_have_value("/unknown-control")
    assert len(fake.methods("turn/start")) == starts, "a control became a model message"
    if screenshots:
        page.screenshot(path=str(screenshots / (label + "-shell-error.png")))
    send("/permissions")
    expect(dialog).to_be_visible()
    expect(box).to_have_value("")
    expect(dialog.locator("#sdk-goal-objective")).to_have_value(objective)
    dialog.locator("#sdk-goal-clear").click()
    expect(dialog.locator("#sdk-goal-current")).to_have_text("尚未设置目标")
    dialog.locator(".modal-close").click()
    page.reload(wait_until="domcontentloaded")
    expect(page.locator("#sdk-input")).to_be_visible()
    expect(page.locator("#sdk-mode")).to_have_value("default")
    expect(page.locator("#sdk-input")).to_have_value("")
    # A reset reconnects to this same thread and reads its current controls.
    resumed = len(fake.methods("thread/resume"))
    generation = page.evaluate("CodexSdkView.generation")
    fake.disconnect()
    page.wait_for_function("previous => CodexSdkView.generation > previous && !!CodexSdkView.summary", arg=generation)
    assert len(fake.methods("thread/resume")) > resumed
    expect(page.locator("#sdk-mode")).to_have_value("default")
    page.locator("#sdk-controls").click()
    expect(dialog.locator('#sdk-control-sandbox option').first).to_contain_text("工作区写入")
    expect(dialog.locator("#sdk-goal-current")).to_have_text("尚未设置目标")
    dialog.locator(".modal-close").click()
    assert not errors, errors
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="sd-sdk-controls-") as temporary:
        root = Path(temporary)
        corpus = build(root)
        app_socket = root / "app.sock"
        with ControlsServer(app_socket, corpus.paths["sdk"], THREAD) as fake, server(corpus, args.binary, app_socket) as base:
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch(headless=True)
                try:
                    for width, theme in ((1280, "light"), (390, "dark"), (1280, "dark"), (390, "light")):
                        exercise(browser, base, corpus, fake, width, theme, args.screenshots)
                finally:
                    browser.close()
    print("PASS SDK controls: native mode/policy settings, explicit goals, shell output, errors, reload and reconnect in desktop/mobile light/dark")


if __name__ == "__main__":
    main()
