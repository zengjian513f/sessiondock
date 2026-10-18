#!/usr/bin/env python3
"""Codex SDK mode: the third session view driven through a scripted app-server.

A standard-library fake (`codex_sdk_fixtures.FakeAppServer`) listens on a
private Unix socket and speaks the app-server WebSocket JSON-RPC transport;
SessionDock connects to it through `SESSIONDOCK_CODEX_APP_SERVER`. Desktop
Chromium opens a synthetic Codex session, switches to the SDK view, and checks
history bubbles, model/effort overrides, streamed replies, a command approval
that survives a page reload, a user-input question, steering and interrupt.
Other sources do not offer it; Codex subagents use the same view. No real CLI or daemon is involved.
"""
from __future__ import annotations

import argparse
import asyncio
from contextlib import contextmanager
import json
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
                                codex_row("response_item", {"type": "reasoning", "summary": [
                                    {"type": "summary_text", "text": "Earlier process to fold and expand"}]}),
                                codex_message("assistant", "earlier synthetic answer"),
                                codex_row("event_msg", {"type": "token_count", "info": {
                                    "last_token_usage": {"total_tokens": 800},
                                    "total_token_usage": {"total_tokens": 90000},
                                    "model_context_window": 4000}}),
                                codex_row("event_msg", {"type": "token_count", "info": None})], [])
    return corpus


@contextmanager
def server(corpus, binary, app_socket, extra_env=None):
    environment = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    environment.update(SESSIONDOCK_BIND=f"127.0.0.1:{port}", SESSIONDOCK_WEB_DIR=str(frontend_dir()),
                       SESSIONDOCK_CODEX_APP_SERVER=str(app_socket))
    proc_root = corpus.root / 'sdk-proc'
    proc_root.mkdir(exist_ok=True)
    (proc_root / 'stat').write_text('btime 1700000000\n')
    environment['SESSIONDOCK_PROC_ROOT'] = str(proc_root)
    for source in ("claude", "codex", "grok"):
        environment["SESSIONDOCK_" + source.upper() + "_ROOT"] = str(corpus.root / source)
    environment.update(extra_env or {})
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


def opening_check(browser, corpus, base, expect, screenshots):
    """History must not wait for open or SSE, or be fetched twice on entry."""
    context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
    page = context.new_page()
    held = []
    reads = []
    page.route("**/api/codex-sdk/open", lambda route: held.append(route))
    page.on("request", lambda request: reads.append(request.url) if "/api/messages/" in request.url else None)
    page.goto(base, wait_until="networkidle")
    page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
    history = page.locator('.sdk-history')
    expect(history).to_contain_text("earlier synthetic answer")
    expect(page.locator('.sdk-status')).to_have_text("连接中")
    assert len(reads) == 1, reads
    assert held, "SDK connection was not started"
    if screenshots:
        screenshots.mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(screenshots / "opening-pending.png"))
    held.pop().fulfill(status=503, content_type="application/json",
                       body=json.dumps({"ok": False, "error": "Synthetic unavailable daemon"}))
    expect(page.locator('.sdk-status')).to_contain_text("Synthetic unavailable daemon")
    expect(history).to_contain_text("earlier synthetic answer")
    page.unroute("**/api/codex-sdk/open")
    snapshots = []
    page.route("**/api/codex-sdk/events?*", lambda route: snapshots.append(route))
    # A failed open can be retried through the already-selected SDK button.
    page.locator('#a-sdk').click()
    expect(history).to_contain_text("earlier synthetic answer")
    expect(page.locator('.sdk-gauge[data-gauge="5h"] b')).to_have_text("40%")
    assert snapshots, "SDK snapshot was not requested"
    assert len(reads) == 1, reads
    snapshots.pop().continue_()
    expect(page.locator('.sdk-gauge[data-gauge="context"] b')).to_have_text("20%")
    expect(history.locator('.msg[data-role="assistant"]')).to_have_count(1)
    assert len(reads) == 1, reads
    # Re-entering a cached session must include records written while away.
    page.unroute("**/api/codex-sdk/events?*")
    before = corpus.paths["sdk"].read_text()
    try:
        with corpus.paths["sdk"].open("a") as native:
            native.write(json.dumps(codex_message("assistant", "cached reentry sentinel")) + "\n")
        page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
        expect(history).to_contain_text("cached reentry sentinel")
    finally:
        context.close()
        corpus.paths["sdk"].write_text(before)


def loading_check(browser, corpus, base, expect, screenshots):
    """Cold history cannot delay SDK selection, connection or draft editing."""
    for width, theme in ((1280, 'light'), (1280, 'dark'), (390, 'light'), (390, 'dark')):
        context = browser.new_context(viewport={"width": width, "height": 900}, service_workers="block")
        context.add_init_script("localStorage.setItem('sessiondock.theme', JSON.stringify(" + json.dumps(theme) + "));")
        page = context.new_page()
        pending, reads, errors = [], [], []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.route('**/api/messages/**', lambda route: pending.append(route))
        page.on('request', lambda request: reads.append(request.url) if '/api/messages/' in request.url else None)
        page.goto(base, wait_until='networkidle')
        expect(page.locator('html')).to_have_attribute('data-theme', theme)
        page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
        expect(page.locator('.sdk-pane')).to_be_visible()
        expect(page.locator('#a-sdk')).to_have_attribute('aria-pressed', 'true')
        expect(page.locator('#detail h2')).not_to_have_text('控制台')
        expect(page.locator('#termpane')).to_be_hidden()
        expect(page.locator('.sdk-gauge[data-gauge="5h"] b')).to_have_text('40%')
        assert len(pending) == len(reads) == 1, reads
        page.locator('#sdk-input').fill('Draft while history is loading')
        if screenshots:
            screenshots.mkdir(parents=True, exist_ok=True)
            page.screenshot(path=str(screenshots / f'loading-{width}-{theme}.png'))
        pending.pop().continue_()
        expect(page.locator('.sdk-history')).to_contain_text('earlier synthetic answer')
        expect(page.locator('#sdk-input')).to_have_value('Draft while history is loading')
        assert len(reads) == 1, reads
        if width == 1280 and theme == 'light':
            page.reload(wait_until='domcontentloaded')
            expect(page.locator('.sdk-history .spin')).to_be_visible()
            page.locator('#sdk-input').fill('Draft survives a failed history read')
            assert len(pending) == 1
            pending.pop().fulfill(status=503, content_type='application/json',
                body=json.dumps({'error': 'Synthetic history temporarily unavailable'}))
            expect(page.locator('.sdk-history .empty')).to_contain_text('Synthetic history temporarily unavailable')
            page.unroute('**/api/messages/**')
            expect(page.locator('.sdk-history')).to_contain_text('earlier synthetic answer')
            expect(page.locator('#sdk-input')).to_have_value('Draft survives a failed history read')
            page.route('**/api/messages/**', lambda route: pending.append(route))
        # Explicitly leaving SDK while the initial request is pending must stick.
        page.reload(wait_until='domcontentloaded')
        expect(page.locator('.sdk-pane')).to_be_visible()
        page.locator('#a-chat').click()
        expect(page.locator('.sdk-pane')).to_have_count(0)
        assert len(pending) == 1
        pending.pop().continue_()
        expect(page.locator('#msgs')).to_contain_text('earlier synthetic answer')
        expect(page.locator('.sdk-pane')).to_have_count(0)
        assert not errors, errors
        context.close()


def browser_check(corpus, base, fake, screenshots=None):
    from playwright.sync_api import expect, sync_playwright
    with sync_playwright() as playwright:
        launch = {"headless": True}
        if os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE"):
            launch["executable_path"] = os.environ["PLAYWRIGHT_CHROMIUM_EXECUTABLE"]
        browser = playwright.chromium.launch(**launch)
        try:
            loading_check(browser, corpus, base, expect, screenshots)
            opening_check(browser, corpus, base, expect, screenshots)
            context = browser.new_context(viewport={"width": 1280, "height": 900}, service_workers="block")
            context.route("**/*", lambda route: route.continue_() if route.request.url.startswith(base + "/") else route.abort())
            page = context.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            limits_failures = [503, "network"]
            limits_reads = []

            def read_limits(route):
                failure = limits_failures.pop(0) if limits_failures else None
                limits_reads.append(failure)
                if failure == "network":
                    route.abort("failed")
                elif failure:
                    route.fulfill(status=failure, content_type="application/json",
                                  body=json.dumps({"ok": False, "error": "Temporary limits failure"}))
                else:
                    route.continue_()

            page.route("**/api/codex-sdk/limits?*", read_limits)
            page.goto(base, wait_until="networkidle")

            # Codex roots and children offer the view; other sources do not.
            assert page.evaluate("CodexSdkView.available('claude:0123456789abcdef', null)") is False
            assert page.evaluate("CodexSdkView.available('codex:0123456789abcdef', 'helper')") is True

            pane = open_sdk(page, corpus, expect)
            # Resume must show persisted context before any turn is submitted.
            expect(pane.locator('.sdk-gauge[data-gauge="context"] b')).to_have_text("20%")
            # Failed HTTP and network reads recover without a turn, notification, or reload.
            expect(pane.locator('.sdk-gauge[data-gauge="5h"]')).to_be_hidden()
            expect(pane.locator('.sdk-credits')).to_be_hidden()
            expect(pane.locator('.sdk-gauge[data-gauge="5h"] b')).to_have_text("40%", timeout=10000)
            expect(pane.locator('.sdk-credits')).to_have_text("Credits 1,235")
            assert limits_reads[:3] == [503, "network", None], limits_reads
            assert not fake.methods("turn/start"), "reading usage must not start a model turn"
            page.reload(wait_until="networkidle")
            expect(pane.locator('.sdk-gauge[data-gauge="context"] b')).to_have_text("20%")
            def toggle_processes():
                if not page.locator('#a-turns').is_visible():
                    page.locator('#a-more').click()
                page.locator('#a-turns').click()

            process = pane.locator('.sdk-history > .turn-process')
            expect(process).to_have_count(1)
            expect(process).to_have_class('msg turn-process folded')
            toggle_processes()
            expect(process.locator('.turn-process-body')).to_be_visible()
            expect(process).to_contain_text('Earlier process to fold and expand')
            page.reload(wait_until='networkidle')
            expect(process.locator('.turn-process-body')).to_be_visible()
            process.locator('.fold-toggle').first.click()
            expect(process.locator('.turn-process-body')).to_be_hidden()
            process.locator('.fold-toggle').first.click()
            expect(process.locator('.turn-process-body')).to_be_visible()
            toggle_processes()
            expect(process.locator('.turn-process-body')).to_be_hidden()
            if screenshots:
                screenshots.mkdir(parents=True, exist_ok=True)
                for width, theme in [(1280, "light"), (390, "dark"), (1280, "dark"), (390, "light")]:
                    page.set_viewport_size({"width": width, "height": 900})
                    page.emulate_media(color_scheme=theme)
                    page.reload(wait_until='networkidle')
                    expect(pane).to_be_visible()
                    usage_toggle = pane.locator('#sdk-usage-toggle')
                    if usage_toggle.is_visible():
                        usage_toggle.click()
                    expect(pane.locator('.sdk-gauge[data-gauge="context"]')).to_be_visible()
                    expect(pane.locator('.sdk-gauge[data-gauge="5h"] b')).to_have_text("40%")
                    expect(pane.locator('.sdk-credits')).to_have_text("Credits 1,235")
                    if usage_toggle.is_visible():
                        page.screenshot(path=str(screenshots / f"usage-{width}-{theme}.png"))
                        page.keyboard.press('Escape')
                    toggle_processes()
                    expect(process.locator('.turn-process-body')).to_be_visible()
                    page.screenshot(path=str(screenshots / f"process-expanded-{width}-{theme}.png"))
                    toggle_processes()
                    expect(process.locator('.turn-process-body')).to_be_hidden()
                    page.screenshot(path=str(screenshots / f"context-{width}-{theme}.png"))
                page.set_viewport_size({"width": 1280, "height": 900})
            history = pane.locator(".sdk-history")
            # Persisted history uses the same bubbles as the message view.
            expect(history.locator('.msg[data-role="user"]')).to_contain_text("earlier synthetic question")
            expect(history.locator('.msg[data-role="assistant"]')).to_contain_text("earlier synthetic answer")
            fake.wait_for(lambda: fake.methods("thread/resume"))

            # SDK commands dispatch locally and never become a model turn.
            command_box = pane.locator('#sdk-input')
            command_box.fill('/help')
            pane.locator('#sdk-send').click()
            expect(pane.locator('.sdk-status')).to_contain_text('可用命令')
            expect(command_box).to_have_value('')
            command_box.fill('/model')
            pane.locator('#sdk-send').click()
            expect(pane.locator('#sdk-model')).to_be_focused()
            command_box.fill('/effort')
            pane.locator('#sdk-send').click()
            expect(pane.locator('#sdk-effort')).to_be_focused()
            command_box.fill('/status')
            pane.locator('#sdk-send').click()
            expect(pane.locator('.sdk-status')).to_contain_text('fake-sol')
            command_box.fill('/unknown-sdk-command')
            pane.locator('#sdk-send').click()
            expect(pane.locator('.sdk-status')).to_contain_text('不支持的命令')
            expect(command_box).to_have_value('/unknown-sdk-command')
            assert not fake.methods('turn/start')

            # Model and effort pickers come from model/list and ride on turn/start.
            model = pane.locator("#sdk-model")
            expect(model.locator("option")).to_have_count(2)
            assert all(value for value in model.locator("option").evaluate_all("nodes => nodes.map(node => node.value)"))
            expect(model).to_have_value("fake-sol")
            effort = pane.locator("#sdk-effort")
            expect(effort).to_have_value("high")  # Current thread, not model/list's low default.
            fake.set_settings("fake-private-model", "xhigh")
            expect(model).to_have_value("fake-private-model")
            expect(effort).to_have_value("xhigh")
            fake.set_settings("fake-sol", "high")
            expect(model).to_have_value("fake-sol")
            model.select_option("fake-luna")
            expect(effort.locator('option[value="medium"]')).to_have_count(1)
            effort.select_option("medium")
            fake.set_settings("fake-sol", "low")
            expect(model).to_have_value("fake-luna")  # Preserve an unsent choice across events.
            expect(effort).to_have_value("medium")

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
            expect(pane.locator("#sdk-send")).to_have_text("排队")

            # Closing the SDK surface detaches only the browser subscriber.
            # Daemon work and its server-side snapshot continue without a page.
            page.locator('#a-chat').click()
            expect(page.locator('#session-content-view')).to_have_count(0)
            asyncio.run_coroutine_threadsafe(fake._agent(fake.turn, 'away-progress',
                'Progress while the view is closed', 'commentary'), fake.loop).result(timeout=5)
            page.locator('#a-sdk').click()
            expect(pane).to_contain_text('Progress while the view is closed')
            assert len(fake.methods('thread/resume')) == 1
            assert not fake.methods('thread/unsubscribe')
            assert not fake.methods('turn/interrupt')

            # A permanently closed EventSource will not retry by itself.
            # Keep the real SSE/snapshot path; inject only the transport failure.
            box.fill('draft survives a disconnected stream')
            reconnect_failures = [503]
            def reconnect_open(route):
                if reconnect_failures:
                    route.fulfill(status=reconnect_failures.pop(), content_type='application/json',
                                  body=json.dumps({'ok': False, 'error': 'temporary service restart'}))
                else:
                    route.continue_()
            page.route('**/api/codex-sdk/open', reconnect_open)
            page.evaluate('''() => {
                window.closedSdkStream = CodexSdkView.es;
                closedSdkStream.close();
                closedSdkStream.dispatchEvent(new Event('error'));
            }''')
            expect(pane.locator('.sdk-status')).to_have_text('重连中')
            page.wait_for_function('CodexSdkView.es !== closedSdkStream && CodexSdkView.es?.readyState === 1')
            expect(pane).to_contain_text('Progress while the view is closed')
            expect(box).to_have_value('draft survives a disconnected stream')
            assert not reconnect_failures
            page.unroute('**/api/codex-sdk/open', reconnect_open)
            box.fill('')

            # The open approval survives a reload: it comes back in the snapshot.
            page.reload(wait_until="networkidle")
            pane = open_sdk(page, corpus, expect)
            expect(pane.locator('#sdk-model')).to_have_value('fake-luna')
            expect(pane.locator('#sdk-effort')).to_have_value('medium')
            live = pane.locator(".sdk-live")
            card = live.locator(".sdk-request")
            expect(card).to_contain_text("需要批准执行命令")
            expect(live.locator('.msg[data-role="assistant"]').filter(has_text='Working on it')).to_have_count(1)
            assert fake.methods("thread/resume").__len__() == 1, "a reload must reuse the loaded thread"
            limits_failures.append(503)
            with page.expect_response(lambda response: "/api/codex-sdk/limits?" in response.url and response.status == 503):
                card.get_by_role("button", name="批准", exact=True).click()
            # Refresh failure retains the last good values while retrying in the background.
            expect(pane.locator('.sdk-gauge[data-gauge="5h"] b')).to_have_text("40%")
            expect(pane.locator('.sdk-credits')).to_have_text("Credits 1,235")
            reply = fake.wait_for(lambda: [m for m in fake.responses() if m.get("result", {}).get("decision")])[0]
            assert reply["id"] == 0 and reply["result"] == {"decision": "accept"}, reply
            expect(live.locator(".sdk-request")).to_have_count(0)
            expect(pane.locator(".tool-entry").filter(has_text="echo synthetic")).to_have_count(1)
            expect(pane).to_contain_text("All done after approval")
            expect(pane.locator("#sdk-stop")).to_be_hidden()
            # Context joins the limit bars: 1000 of a 4000-token window used (25%).
            context_bar = pane.locator('.sdk-gauge[data-gauge="context"]')
            expect(context_bar.locator("b")).to_have_text("25%")
            expect(context_bar.locator(".sdk-bar")).to_have_attribute("aria-valuenow", "25")
            with page.expect_response(lambda response: "/api/codex-sdk/limits?" in response.url and response.status == 200):
                expect(pane.locator('.sdk-credits')).to_have_text("Credits 1,235")
            # After completion the rollout holds the turn; it is shown exactly once.
            expect(history.locator('.msg[data-role="assistant"]').filter(has_text="All done after approval")).to_have_count(1)
            expect(pane.locator('.msg[data-role="assistant"]').filter(has_text="All done after approval")).to_have_count(1)
            expect(pane.locator('.msg[data-role="user"]').filter(has_text="approve the synthetic command")).to_have_count(1)

            # Account limits as bars (used %, 100% = full), credits, and redeemable reset cards.
            five, week = pane.locator('.sdk-gauge[data-gauge="5h"]'), pane.locator('.sdk-gauge[data-gauge="7d"]')
            expect(five.locator("b")).to_have_text("40%")
            expect(five.locator(".sdk-bar")).to_have_attribute("aria-valuenow", "40")
            expect(week.locator("b")).to_have_text("25%")
            expect(pane.locator(".sdk-credits")).to_have_text("Credits 1,235")
            pane.locator('#sdk-more').click()
            reset = pane.locator("#sdk-reset-card")
            expect(reset.locator("option")).to_have_count(2)
            expect(reset.locator("option").nth(1)).to_contain_text("Reset B")
            reset.select_option("card-b")
            pane.locator("#sdk-reset-use").click()
            page.locator("dialog[open] button", has_text="兑换").click()
            consumed = fake.wait_for(lambda: fake.methods("account/rateLimitResetCredit/consume"))[0]["params"]
            assert consumed["creditId"] == "card-b" and len(consumed["idempotencyKey"]) >= 16, consumed
            expect(five.locator("b")).to_have_text("0%")
            expect(week.locator("b")).to_have_text("0%")
            expect(reset.locator("option")).to_have_count(1)
            expect(reset.locator("option")).to_contain_text("Reset A")
            expect(pane.locator(".sdk-status")).to_contain_text("已兑换")

            # A question card, steering into the running turn, then interrupt.
            box = pane.locator("#sdk-input")
            box.fill("ask me something")
            pane.locator("#sdk-send").click()
            started = fake.wait_for(lambda: fake.methods('turn/start') if len(fake.methods('turn/start')) == 2 else None)[-1]['params']
            assert 'model' not in started and 'effort' not in started, started
            question = pane.locator(".sdk-request")
            expect(question).to_contain_text("Which color?")
            question.get_by_label("Blue").check()
            question.get_by_role("button", name="提交").click()
            answer = fake.wait_for(lambda: [m for m in fake.responses() if "answers" in m.get("result", {})])[0]
            assert answer["result"] == {"answers": {"color": {"answers": ["Blue"]}}}, answer
            expect(pane).to_contain_text("You picked Blue")
            box.fill("steer toward tests")
            box.press("Enter")
            expect(pane.locator(".sdk-queued")).to_contain_text("steer toward tests")
            pane.locator('.sdk-queued [data-queue-action="steer"]').click()
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
            # Closing a view with a failed read cancels its scheduled retry.
            limits_failures.append(503)
            with page.expect_response(lambda response: "/api/codex-sdk/limits?" in response.url and response.status == 503):
                page.locator('#a-sdk').click()
            page.locator('#a-chat').click()
            read_count = len(limits_reads)
            page.wait_for_timeout(1500)
            assert len(limits_reads) == read_count, "closed SDK view kept retrying usage reads"
            assert not errors, errors
            context.close()
        finally:
            browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-sdk-") as temporary:
        root = Path(temporary)
        corpus = build(root)
        app_socket = root / "app-server.sock"
        with FakeAppServer(app_socket, corpus.paths["sdk"], THREAD) as fake:
            with server(corpus, args.binary, app_socket) as base:
                browser_check(corpus, base, fake, args.screenshots)
    print("PASS Codex SDK view: history bubbles, model/effort override, streaming, approval across reload, "
          "user-input answer, steer, interrupt, non-Codex excluded")


if __name__ == "__main__":
    main()
