#!/usr/bin/env python3
"""Codex SDK two-tab HTTP/1 actions and restoration of the history watch.

Synthetic provider regression for a failure reproduced against the real daemon.
Native providers' equivalent path is covered by native_sdk_queue_browser.
"""
import argparse
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from codex_sdk_queue_browser import QueueServer
from history_fixtures import BINARY
from native_sdk_queue_fixtures import NativeQueueFixture


def replaced_history(browser, base, uid, screenshots, source):
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    page = context.new_page()
    held = []
    def hold_first(route):
        if not held:
            held.append(route)
        else:
            route.continue_()
    try:
        page.goto(base, wait_until='domcontentloaded')
        page.route('**/api/messages/*', hold_first)
        row = page.locator(f'#side .item[data-uid="{uid}"]')
        row.click()
        expect(page.locator('#sdk-input')).to_be_visible()
        assert len(held) == 1
        row.click()
        page.wait_for_function('SdkView.historyData?.messages.length > 0')
        expect(page.locator('.sdk-history')).not_to_contain_text('读取历史失败')
        assert page.evaluate('SdkView.historyFailed') is False
        if screenshots:
            page.locator('#right').screenshot(path=str(screenshots / f'{source}-history-retry.png'))
    finally:
        for route in held:
            try:
                route.abort()
            except Exception:
                pass  # The superseded browser request may already be gone.
        page.unroute_all(behavior='ignoreErrors')
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-sdk-multitab-') as tmp:
        root = Path(tmp)
        corpus = build(root)
        env = environment(root, args.binary)
        with QueueServer(root / 'app.sock', corpus.paths['sdk'], THREAD) as fake, \
                server(corpus, args.binary, root / 'app.sock', env) as base, \
                sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
            errors = []
            def open_page():
                page = context.new_page()
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(base, wait_until='domcontentloaded')
                page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
                expect(page.locator('#sdk-input')).to_be_enabled()
                page.wait_for_function('CodexSdkView.connected && !_es')
                return page
            try:
                first = open_page()
                first.locator('#sdk-input').fill('held real-world queue regression')
                first.locator('#sdk-send').click()
                expect(first.locator('#sdk-send')).to_have_text('排队')
                first.locator('#sdk-input').fill('cancel from another tab')
                first.locator('#sdk-send').click()
                expect(first.locator('.sdk-queued')).to_have_count(1)
                second = open_page()
                expect(second.locator('.sdk-queued')).to_have_count(1)
                with second.expect_response(lambda response: response.url.endswith('/api/codex-sdk/queue'), timeout=10000) as cancelled:
                    second.locator('.sdk-queued [data-queue-action=cancel]').click()
                assert cancelled.value.ok, cancelled.value.text()
                expect(first.locator('.sdk-queued')).to_have_count(0)
                expect(second.locator('.sdk-queued')).to_have_count(0)
                assert len(fake.methods('turn/start')) == 1
                if args.screenshots:
                    second.locator('#right').screenshot(path=str(args.screenshots / 'two-tab-cancel.png'))
                second.close()
                first.locator('#a-chat').click()
                first.wait_for_function('uid => _es?.readyState===1 && _esUid===uid', arg=corpus.uid('sdk'))
                first.locator('#a-sdk').click()
                first.wait_for_function('CodexSdkView.connected && !_es')
                with first.expect_response(lambda response: response.url.endswith('/api/codex-sdk/interrupt')) as stopped:
                    first.locator('#sdk-stop').click()
                assert stopped.value.ok, stopped.value.text()
                first.wait_for_function('!CodexSdkView.turn')
                assert not errors, errors
            finally:
                context.close()
            replaced_history(browser, base, corpus.uid('sdk'), args.screenshots, 'codex')
            browser.close()
        with sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            native = NativeQueueFixture(root / 'native', 'grok', args.binary, BINARY.parent / 'ptyhost')
            with native.server() as base:
                probe = browser.new_page()
                probe.goto(base, wait_until='domcontentloaded')
                probe.wait_for_function('sid => S.sessions.some(row => row.sid===sid)', arg=native.sid)
                uid = probe.evaluate('sid => S.sessions.find(row => row.sid===sid).uid', native.sid)
                probe.close()
                replaced_history(browser, base, uid, args.screenshots, 'grok')
            browser.close()
    print('PASS SDK two-tab cancellation over HTTP/1, running turn, watch restore/stop and replaced Codex/Grok history reads')


if __name__ == '__main__':
    main()
