#!/usr/bin/env python3
"""Codex SDK mode: the third session view driven through a scripted app-server.

A standard-library fake (`codex_sdk_fixtures.FakeAppServer`) listens on a
private Unix socket and speaks the app-server WebSocket JSON-RPC transport;
SessionDock connects to it through `SESSIONDOCK_CODEX_APP_SERVER`. Desktop
Chromium opens a synthetic Codex session, switches to the SDK view, and checks
history bubbles, model/effort overrides, streamed replies, a command approval
that survives a page reload, a user-input question, steering and interrupt.
Other sources and subagent views do not offer it. No real CLI or daemon is involved.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from urllib.error import URLError
from urllib.request import ProxyHandler, build_opener

from codex_sdk_fixtures import FakeAppServer
from frontend_paths import frontend_dir
from history_fixtures import BINARY, REPO, Corpus, codex_message, codex_row, get_json
from http_fixtures import NoRedirects

THREAD = "01a1sdk0-0000-7000-8000-000000000001"


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    meta = {"id": THREAD, "cwd": "/synthetic/sdk", "timestamp": "2026-09-11T10:00:00Z"}
    corpus.put("sdk", "codex", [codex_row("session_meta", meta),
                                codex_message("user", "earlier synthetic question"),
                                codex_message("assistant", "earlier synthetic answer")], [])
    return corpus


@contextmanager
def server(corpus, binary, app_socket):
    environment = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    environment.update(SESSIONDOCK_BIND=f"127.0.0.1:{port}", SESSIONDOCK_WEB_DIR=str(frontend_dir()),
                       SESSIONDOCK_CODEX_APP_SERVER=str(app_socket))
    for source in ("claude", "codex", "grok"):
        environment["SESSIONDOCK_" + source.upper() + "_ROOT"] = str(corpus.root / source)
    base = f"http://127.0.0.1:{port}"
    opener = build_opener(ProxyHandler({}), NoRedirects())
    with tempfile.TemporaryFile(mode="w+b") as log:
        process = subprocess.Popen([str(binary.resolve(strict=True))], cwd=REPO, env=environment, stdout=log, stderr=log)
        try:
            for _ in range(150):
                assert process.poll() is None, "isolated server exited early"
                try:
                    get_json(opener, base, "/api/health")
                    break
                except (OSError, URLError):
                    time.sleep(0.05)
            else:
                raise AssertionError("isolated server failed to become ready")
            yield base
        except BaseException:
            log.flush()
            log.seek(0)
            output = log.read().decode("utf-8", "replace")
            if output:
                print(output, file=sys.stderr, end="")
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()  # Only this test's exact child process.
                    process.wait(timeout=5)


def open_sdk(page, corpus, expect):
    page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
    expect(page.locator("#msgs")).to_contain_text("earlier synthetic answer")
    sdk = page.locator("#a-sdk")
    expect(sdk).to_be_visible()
    sdk.click()
    expect(sdk).to_have_attribute("aria-pressed", "true")
    pane = page.locator("#session-content-view.sdk-pane")
    expect(pane).to_be_visible()
    expect(page.locator("#composer")).to_be_hidden()
    return pane


def browser_check(corpus, base, fake):
    from playwright.sync_api import expect, sync_playwright
    with sync_playwright() as playwright:
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**launch)
        try:
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
            page = context.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(base, wait_until="networkidle")

            # Only top-level Codex sessions offer the view.
            assert page.evaluate("CodexSdkView.available('claude:0123456789abcdef', null)") is False
            assert page.evaluate("CodexSdkView.available('codex:0123456789abcdef', 'helper')") is False

            pane = open_sdk(page, corpus, expect)
            history = pane.locator(".sdk-history")
            # Persisted history uses the same bubbles as the message view.
            expect(history.locator('.msg[data-role="user"]')).to_contain_text("earlier synthetic question")
            expect(history.locator('.msg[data-role="assistant"]')).to_contain_text("earlier synthetic answer")
            fake.wait_for(lambda: fake.methods("thread/resume"))

            # Model and effort pickers come from model/list and ride on turn/start.
            model = pane.locator("#sdk-model")
            expect(model.locator("option")).to_have_count(3)
            model.select_option("fake-luna")
            effort = pane.locator("#sdk-effort")
            expect(effort.locator('option[value="medium"]')).to_have_count(1)
            effort.select_option("medium")

            box = pane.locator("#sdk-input")
            box.fill("approve the synthetic command")
            box.press("Enter")
            started = fake.wait_for(lambda: fake.methods("turn/start"))[0]["params"]
            assert started["threadId"] == THREAD and started["model"] == "fake-luna" and started["effort"] == "medium", started
            expect(box).to_have_value("")
            live = pane.locator(".sdk-live")
            expect(live.locator('.msg[data-role="user"]')).to_contain_text("approve the synthetic command")
            expect(live.locator('.msg[data-role="assistant"]')).to_contain_text("Working on it")
            card = live.locator(".sdk-request")
            expect(card).to_contain_text("需要批准执行命令")
            expect(card).to_contain_text("echo synthetic")
            expect(card).to_contain_text("Synthetic approval reason")
            expect(pane.locator("#sdk-stop")).to_be_visible()
            expect(pane.locator("#sdk-send")).to_have_text("追加")

            # The open approval survives a reload: it comes back in the snapshot.
            page.reload(wait_until="networkidle")
            pane = open_sdk(page, corpus, expect)
            live = pane.locator(".sdk-live")
            card = live.locator(".sdk-request")
            expect(card).to_contain_text("需要批准执行命令")
            expect(live.locator('.msg[data-role="assistant"]')).to_contain_text("Working on it")
            assert fake.methods("thread/resume").__len__() == 1, "a reload must reuse the loaded thread"
            card.get_by_role("button", name="批准", exact=True).click()
            reply = fake.wait_for(lambda: [m for m in fake.responses() if m.get("result", {}).get("decision")])[0]
            assert reply["id"] == 0 and reply["result"] == {"decision": "accept"}, reply
            expect(live.locator(".sdk-request")).to_have_count(0)
            expect(pane.locator(".tool-entry").filter(has_text="echo synthetic")).to_have_count(1)
            expect(pane).to_contain_text("All done after approval")
            expect(pane.locator("#sdk-stop")).to_be_hidden()
            # Context joins the limit bars: 1000 of a 4000-token window used.
            context_bar = pane.locator('.sdk-gauge[data-gauge="context"]')
            expect(context_bar.locator("b")).to_have_text("75%")
            expect(context_bar.locator(".sdk-bar")).to_have_attribute("aria-valuenow", "75")
            # After completion the rollout holds the turn; it is shown exactly once.
            expect(history.locator('.msg[data-role="assistant"]').filter(has_text="All done after approval")).to_have_count(1)
            expect(pane.locator('.msg[data-role="assistant"]').filter(has_text="All done after approval")).to_have_count(1)
            expect(pane.locator('.msg[data-role="user"]').filter(has_text="approve the synthetic command")).to_have_count(1)

            # Account limits as bars (remaining), credits, and redeemable reset cards.
            five, week = pane.locator('.sdk-gauge[data-gauge="5h"]'), pane.locator('.sdk-gauge[data-gauge="7d"]')
            expect(five.locator("b")).to_have_text("60%")
            expect(five.locator(".sdk-bar")).to_have_attribute("aria-valuenow", "60")
            expect(week.locator("b")).to_have_text("75%")
            expect(pane.locator(".sdk-credits")).to_have_text("Credits 1,235")
            reset = pane.locator("#sdk-reset-card")
            expect(reset.locator("option")).to_have_count(2)
            expect(reset.locator("option").nth(1)).to_contain_text("Reset B")
            reset.select_option("card-b")
            pane.locator("#sdk-reset-use").click()
            page.locator("dialog[open] button", has_text="兑换").click()
            consumed = fake.wait_for(lambda: fake.methods("account/rateLimitResetCredit/consume"))[0]["params"]
            assert consumed["creditId"] == "card-b" and len(consumed["idempotencyKey"]) >= 16, consumed
            expect(five.locator("b")).to_have_text("100%")
            expect(week.locator("b")).to_have_text("100%")
            expect(reset.locator("option")).to_have_count(1)
            expect(reset.locator("option")).to_contain_text("Reset A")
            expect(pane.locator(".sdk-status")).to_contain_text("已兑换")

            # A question card, steering into the running turn, then interrupt.
            box = pane.locator("#sdk-input")
            box.fill("ask me something")
            pane.locator("#sdk-send").click()
            question = pane.locator(".sdk-request")
            expect(question).to_contain_text("Which color?")
            question.get_by_label("Blue").check()
            question.get_by_role("button", name="提交").click()
            answer = fake.wait_for(lambda: [m for m in fake.responses() if "answers" in m.get("result", {})])[0]
            assert answer["result"] == {"answers": {"color": {"answers": ["Blue"]}}}, answer
            expect(pane).to_contain_text("You picked Blue")
            box.fill("steer toward tests")
            box.press("Enter")
            steer = fake.wait_for(lambda: fake.methods("turn/steer"))[0]["params"]
            assert steer["expectedTurnId"] == "turn-2" and steer["input"][0]["text"] == "steer toward tests", steer
            expect(pane.locator(".sdk-live")).to_contain_text("steer toward tests")
            pane.locator("#sdk-stop").click()
            stop = fake.wait_for(lambda: fake.methods("turn/interrupt"))[0]["params"]
            assert stop == {"threadId": THREAD, "turnId": "turn-2"}, stop
            expect(pane.locator("#sdk-stop")).to_be_hidden()
            expect(pane.locator("#sdk-send")).to_have_text("发送")

            # Back to the ordinary conversation view.
            page.locator("#a-chat").click()
            expect(page.locator("#session-content-view")).to_have_count(0)
            expect(page.locator("#msgs")).to_contain_text("All done after approval")
            assert not errors, errors
            context.close()
        finally:
            browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-sdk-") as temporary:
        root = Path(temporary)
        corpus = build(root)
        app_socket = root / "app-server.sock"
        with FakeAppServer(app_socket, corpus.paths["sdk"], THREAD) as fake:
            with server(corpus, args.binary, app_socket) as base:
                browser_check(corpus, base, fake)
    print("PASS Codex SDK view: history bubbles, model/effort override, streaming, approval across reload, "
          "user-input answer, steer, interrupt, non-Codex excluded")


if __name__ == "__main__":
    main()
