#!/usr/bin/env python3
"""SDK modes, policies and native shell controls through Chromium.

Private fake app-server only: no shell, model, CLI, account or production data.
Exercises desktop/mobile, light/dark, actual controls and slash commands, live
settings notifications, removed goal controls, failures preserving drafts, reload and daemon reconnect.
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

    def external_settings(self):
        async def update():
            self.settings['approvalPolicy'] = 'on-request'
            await self.notify('thread/settings/updated', {'threadSettings': self.settings.copy()})
        asyncio.run_coroutine_threadsafe(update(), self.loop).result(timeout=5)

    def disconnect(self):
        async def close():
            for writer in list(self.clients):
                writer.close()
                await writer.wait_closed()
        asyncio.run_coroutine_threadsafe(close(), self.loop).result(timeout=5)


def exercise(browser, base, corpus, fake, width, theme, screenshots):
    label = f"{width}-{theme}"
    print(label + ": mode / policies / shell / failures / reconnect", flush=True)
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
    if not mode.is_visible(): page.locator('#sdk-more').click()
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
    if not page.locator('#sdk-control-approval').is_visible(): page.locator('#sdk-more').click()
    dialog = page.locator("#sdk-permissions")
    expect(dialog).to_be_visible()
    expect(dialog.locator('#sdk-shell-command, #sdk-shell-run')).to_have_count(0)
    expect(dialog).not_to_contain_text('直接执行命令')
    expect(page.locator('#sdk-controls, [id^="sdk-goal-"]')).to_have_count(0)
    expect(dialog).not_to_contain_text('目标')
    dialog.locator("#sdk-control-approval").select_option("never")
    dialog.locator("#sdk-control-sandbox").select_option("workspaceWrite")
    dialog.locator("#sdk-permission-save").click()
    expect(dialog.locator("#sdk-controls-result")).to_contain_text("权限设置已应用")
    expect(dialog.locator('#sdk-control-approval')).to_have_value("never")
    assert fake.settings["sandboxPolicy"] == {"type": "workspaceWrite"}

    # Shared-client settings refresh the current policy without replacing edits.
    dialog.locator('#sdk-control-sandbox').select_option('readOnly')
    fake.external_settings()
    expect(dialog.locator('#sdk-control-approval')).to_have_value('on-request')
    expect(dialog.locator('#sdk-control-sandbox')).to_have_value('readOnly')
    if screenshots:
        page.screenshot(path=str(screenshots / (label + "-settings.png")))
    page.keyboard.press('Escape')

    send('/goal status')
    expect(page.locator('.sdk-status')).to_contain_text('不支持的命令')
    expect(box).to_have_value('/goal status')
    assert not [row for row in fake.received if row.get('method', '').startswith('thread/goal/')]
    send('/help')
    expect(page.locator('.sdk-status')).not_to_contain_text('/goal')
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
    expect(dialog.locator('#sdk-control-approval')).to_have_value('on-request')
    page.keyboard.press('Escape')
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
    if not page.locator('#sdk-control-approval').is_visible(): page.locator('#sdk-more').click()
    expect(dialog.locator('#sdk-control-sandbox')).to_have_value("workspaceWrite")
    page.keyboard.press('Escape')
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
    print("PASS SDK controls: native mode/policy settings, flat action menu, removed goal controls, shell output, errors, reload and reconnect in desktop/mobile light/dark")


if __name__ == "__main__":
    main()
