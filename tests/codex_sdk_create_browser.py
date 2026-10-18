#!/usr/bin/env python3
"""SDK default and lazy creation through a real node and Hub, with a fake daemon.

Click Create, retry a lost response, reopen/reload the unpersisted thread,
send the first message, approve its command, and follow the native history.
Desktop/mobile and light/dark. No PTY, native CLI, account or production data.
"""
from __future__ import annotations

import argparse
import re
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build, server
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from new_session_model_browser import open_dialog
from node_auth_fixtures import TOKEN, node_env


def exercise(browser, base, fake, root, width, theme, screenshots, label):
    print(f"{label}: create/retry/reload/first SEND", flush=True)
    context = browser.new_context(viewport={"width": width, "height": 844 if width < 500 else 900},
                                  color_scheme=theme, service_workers="block")
    context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
    page = context.new_page()
    page.set_default_timeout(10000)
    errors, requests = [], []
    page.on("pageerror", lambda error: errors.append(error.stack))
    page.on("request", lambda request: requests.append(request.url))
    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")
    open_dialog(page)
    page.locator('input[name="new-source"][value="codex"]').check()
    page.locator("#new-model").click()
    page.locator('[data-model-option]').filter(has_text="Fake Luna").click()
    expect(page.locator("#new-effort")).to_have_value("medium")
    work = root / (label + '-工作目录-with-a-realistic-long-project-name')
    work.mkdir()
    page.locator("#new-cwd").fill(str(work))
    before = len(fake.methods("thread/start"))
    # The actual node creates the thread, but the browser loses its receipt.
    def lose_response(route):
        route.fetch()
        route.abort("failed")
    page.route("**/api/codex-sdk/create", lose_response, times=1)
    page.locator("#new-session-go").click()
    expect(page.locator("#new-session-error")).not_to_be_empty()
    assert len(fake.methods("thread/start")) == before + 1
    assert not fake.rollout.exists(), "Create must leave the native root untouched"
    page.locator("#new-session-go").click()
    expect(page.locator("#new-session-dialog")).to_be_hidden()
    expect(page.locator("#sdk-input")).to_be_visible()
    expect(page.locator("#a-sdk")).to_have_attribute("aria-pressed", "true")
    expect(page.locator("#dlive")).to_have_class(re.compile(r"\bsdk\b"))
    expect(page.locator('#sdk-model')).to_have_value('fake-luna')
    expect(page.locator('#sdk-effort')).to_have_value('medium')
    assert len(fake.methods("thread/start")) == before + 1, "retry created a second thread"
    start = fake.methods("thread/start")[-1]["params"]
    assert start["cwd"] == str(work) and start["model"] == "fake-luna"
    assert start["config"]["model_reasoning_effort"] == "medium"
    uid = page.evaluate("S.sel")
    assert uid.startswith("codex:") and not page.evaluate("T.pending.length")
    if width > 500:
        page.locator('#livecount').click()
        expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_be_visible()
        page.locator('#allcount').click()
    assert not any('/api/term/create' in url for url in requests)
    expect(page.locator(".sdk-status")).not_to_contain_text("失败")
    if screenshots:
        page.screenshot(path=str(screenshots / f"{label}-empty.png"))

    # A reload rejoins the same live thread, with no invented history file.
    page.reload(wait_until="domcontentloaded")
    expect(page.locator("#sdk-input")).to_be_visible()
    assert page.evaluate("S.sel") == uid
    expect(page.locator('#sdk-model')).to_have_value('fake-luna')
    expect(page.locator('#sdk-effort')).to_have_value('medium')
    assert not fake.rollout.exists()
    expect(page.locator(".sdk-status")).not_to_contain_text("失败")
    text = "approve SDK 首条消息 creates the native rollout"
    page.locator("#sdk-input").fill(text)
    page.locator("#sdk-send").click()
    expect(page.locator(".sdk-request")).to_contain_text("需要批准执行命令")
    assert fake.rollout.is_file(), "first turn must let Codex persist its own rollout"
    page.locator(".sdk-request").get_by_role("button", name="批准", exact=True).click()
    expect(page.locator(".sdk-history")).to_contain_text("All done after approval", timeout=15000)
    expect(page.locator('.sdk-pane .msg[data-role="user"]').filter(has_text=text)).to_have_count(1)
    page.wait_for_function("uid => S.sessions.some(row => row.uid === uid)", arg=uid)
    expect(page.locator('#detail > .dhead h2')).to_contain_text(text)
    assert page.evaluate("S.sel") == uid, "native history changed the selected identity"
    # Existing conversation view remains available during the retirement transition.
    page.locator("#a-chat").click()
    expect(page.locator('#msgs')).to_contain_text('All done after approval')
    page.locator("#a-sdk").click()
    expect(page.locator("#sdk-input")).to_be_visible()
    expect(page.locator('.sdk-history')).to_contain_text('All done after approval')
    if screenshots:
        page.screenshot(path=str(screenshots / f"{label}-persisted.png"))
    page.reload(wait_until="domcontentloaded")
    expect(page.locator("#a-sdk")).to_have_attribute("aria-pressed", "true")
    expect(page.locator(".sdk-history")).to_contain_text("All done after approval")
    assert not errors, errors
    context.close()


def discard_empty(browser, base, fake, root, label):
    print(f"{label}: missing directory confirmation and discard before first SEND", flush=True)
    context = browser.new_context(service_workers="block")
    page = context.new_page()
    page.set_default_timeout(10000)
    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("T.listLoaded")
    open_dialog(page)
    page.locator('input[name="new-source"][value="codex"]').check()
    work = root / label / 'new-directory'
    page.locator('#new-cwd').fill(str(work))
    before = len(fake.methods('thread/start'))
    page.locator('#new-session-go').click()
    popup = page.locator('dialog.app-popup[open]')
    expect(popup).to_contain_text('启动目录不存在')
    assert len(fake.methods('thread/start')) == before and not work.exists()
    popup.get_by_role('button', name='确定', exact=True).click()
    expect(page.locator('#sdk-input')).to_be_visible()
    assert work.is_dir() and len(fake.methods('thread/start')) == before + 1
    uid = page.evaluate('S.sel')
    path = fake.rollout
    row = page.locator(f'#side .item[data-uid="{uid}"]')
    expect(row).to_be_visible()
    row.scroll_into_view_if_needed()
    page.wait_for_timeout(150)  # Let the sidebar's scroll-to-close handler settle.
    row.click(button='right')
    page.locator('#item-menu [data-act="delete"]').click()
    expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_count(0)
    assert not path.exists()
    page.reload(wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_count(0)
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--hub-binary", type=Path)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    hub_binary = args.hub_binary or args.binary.with_name("sessiondock-hub")
    with tempfile.TemporaryDirectory(prefix="sd-sdk-create-") as temporary:
        root = Path(temporary)
        corpus = build(root)
        port = free_port()
        environment = node_env(root, port, "127.0.0.0/8")
        with FakeAppServer(root / "app.sock", corpus.paths["sdk"], THREAD) as fake:
            with server(corpus, args.binary, root / "app.sock", environment) as base:
                hub_root = root / "hub"
                hub_root.mkdir()
                node = SimpleNamespace(name="SdkNode", token=TOKEN, port=port)
                hub = Hub(hub_binary.resolve(), hub_root, [node])
                print("isolated node registered; starting Hub", flush=True)
                hub.start()
                try:
                    with sync_playwright() as playwright:
                        browser = playwright.chromium.launch(headless=True)
                        try:
                            for mode, url in (("node", base), ("hub", f"http://127.0.0.1:{hub.port}")):
                                for width, theme in ((1280, "light"), (390, "dark"), (1280, "dark"), (390, "light")):
                                    label = f"{mode}-{width}-{theme}"
                                    exercise(browser, url, fake, root, width, theme, args.screenshots, label)
                                discard_empty(browser, url, fake, root, mode + '-discard')
                        finally:
                            browser.close()
                finally:
                    hub.stop()
    print("PASS SDK default: node/Hub create, lost-response retry, no rollout before first input, "
          "reload, first SEND, approval, stable native identity, desktop/mobile light/dark")


if __name__ == "__main__":
    main()
