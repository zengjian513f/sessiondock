#!/usr/bin/env python3
"""Slow claim transport through the real console UI, with a private free shell.

Hold the request before forwarding or hold its response body beyond the old
five-second deadline. Exercise both renderers and preserve explicit-only retry
when the full end-to-end deadline expires. No production session or real CLI.
"""
import json
import os
from pathlib import Path
import tempfile
import time
import uuid

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
from host_identity import host
from terminal_exit_browser import XTERM_TEXT


def scenario(root, browser, renderer, phase):
    for name in ['host', 'work', 'claude', 'codex', 'grok']:
        (root / name).mkdir(mode=0o700)
    corpus = Corpus(root)
    sid = 'synthetic-native-sid'
    corpus.put(sid, 'codex', [
        codex_row('session_meta', {'id': sid, 'cwd': str(root / 'work')}),
        codex_message('user', 'Synthetic delayed terminal claim'),
    ], [])
    uid = corpus.uid(sid)
    selected_uid = uid
    if renderer == 'grid':
        child = 'synthetic-fork-sid'
        corpus.put(child, 'codex', [
            codex_row('session_meta', {'id': child, 'cwd': str(root / 'work'),
                                      'forked_from_id': sid}),
            codex_message('user', 'Synthetic delayed terminal claim in fork'),
        ], [])
        selected_uid = corpus.uid(child)
    native = corpus.paths[sid].read_bytes()
    instance = 'synthetic-' + uuid.uuid4().hex
    with host(root, instance, uid=uid) as (process, _), isolated_server(
            corpus, BINARY, host_dir=root / 'host') as (base, _):
        context = browser.new_context(viewport={'width': 390 if renderer == 'grid' else 1280,
                                               'height': 900}, service_workers='block')
        try:
            context.add_init_script("localStorage.setItem('sessiondock.consoleRenderer', JSON.stringify(%s))"
                                    % json.dumps(renderer))
            context.route('**/*', lambda route: route.continue_()
                          if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page()
            held, claims, errors, dialogs = [], [], [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.on('dialog', lambda dialog: (dialogs.append(dialog.message), dialog.dismiss()))

            def hold(route):
                claims.append(route.request.post_data_json)
                held.append(route)

            page.route('**/api/term/claim', hold)
            page.goto(base, wait_until='networkidle')
            page.locator(f'#side .item[data-uid="{selected_uid}"]').click()
            expect(page.locator('#msgs')).to_contain_text('Synthetic delayed terminal claim')
            button = page.locator('#a-term')
            expect(button).to_have_attribute('data-unavailable', 'false')
            button.click()
            page.wait_for_function('ConsoleUI.busy.has(S.sel)')
            # Pump Playwright until the actual network request is intercepted.
            deadline = time.monotonic() + 5
            while not held and time.monotonic() < deadline:
                page.wait_for_timeout(20)
            assert len(held) == 1
            response = held[0].fetch() if phase == 'body' else None
            # This real wait proves the production timer does not abort at 5 s.
            page.wait_for_timeout(6000)
            assert page.evaluate('ConsoleUI.busy.has(S.sel)'), 'claim abandoned at the old deadline'
            button.click()
            assert len(claims) == 1 and not dialogs
            assert not claims[0].get('force')
            assert claims[0]['uid'] == uid and claims[0]['instance_id'] == instance
            if phase == 'timeout':
                page.wait_for_function('!ConsoleUI.busy.has(S.sel)', timeout=20000)
                expect(page.locator('#termpane')).to_be_hidden()
                expect(page.locator('#console-toast')).to_contain_text('服务端可能已取得控制权')
                assert page.evaluate('uid => ConsoleUI.errors.has(uid)', selected_uid)
                page.wait_for_timeout(700)
                assert len(claims) == 1, 'ambiguous claim was automatically retried'
                held[0].abort()
                page.unroute('**/api/term/claim', hold)
                # Recovery requires a fresh user click, with no forced takeover.
                page.on('request', lambda request: claims.append(request.post_data_json)
                        if request.url.endswith('/api/term/claim') else None)
                button.click()
            elif response:
                held[0].fulfill(response=response)
            else:
                held[0].continue_()
            page.wait_for_function('T.ws?.readyState === WebSocket.OPEN', timeout=10000)
            expect(page.locator('#termpane')).to_be_visible()
            page.wait_for_function('(' + XTERM_TEXT + ")().includes('RS_SHELL_READY')")
            keyboard = page.locator('#termpane .xterm-helper-textarea')
            keyboard.press_sequentially('ping')
            keyboard.press('Enter')
            page.wait_for_function('(' + XTERM_TEXT + ")().includes('RS_PING_OK')")
            assert len(claims) == (2 if phase == 'timeout' else 1)
            assert all(not claim.get('force') for claim in claims)
            assert not page.evaluate('uid => ConsoleUI.errors.has(uid)', selected_uid)
            # A transport disconnect followed by repeated failed lease requests
            # must keep retrying without a click, reload, dialog or forced claim.
            retries = []
            def fail_reclaims(route):
                retries.append(route.request.post_data_json)
                if len(retries) <= 2:
                    route.abort('failed')
                else:
                    route.continue_()
            page.route('**/api/term/claim', fail_reclaims)
            page.evaluate('T.ws.close()')
            page.wait_for_function('T.ws?.readyState === WebSocket.OPEN', timeout=15000)
            assert len(retries) >= 3, retries
            assert all(not item.get('force') for item in retries)
            keyboard.press_sequentially('ping')
            keyboard.press('Enter')
            page.wait_for_function('(' + XTERM_TEXT + ")().includes('RS_PING_OK')")
            page.unroute('**/api/term/claim', fail_reclaims)

            assert process.poll() is None and corpus.paths[sid].read_bytes() == native
            if renderer == 'xterm':
                # A new owner is an authority decision, not a network failure.
                conflicts = []
                def held_elsewhere(route):
                    conflicts.append(route.request.post_data_json)
                    route.fulfill(status=409, content_type='application/json', body=json.dumps({
                        'conflict': True, 'owner': {'label': 'other page'}, 'same_address': True}))
                page.route('**/api/term/claim', held_elsewhere)
                page.evaluate('T.ws.close()')
                expect(page.locator('#termpane')).to_be_hidden(timeout=10000)
                page.wait_for_timeout(1500)
                assert len(conflicts) == 1 and not conflicts[0].get('force'), conflicts
            assert not errors and not dialogs, (errors, dialogs)
            print(f'PASS terminal claim {renderer}/{phase}: real click, delayed transport, shell input/output, network recovery, no automatic force', flush=True)
        finally:
            context.close()


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-claim-') as temporary, sync_playwright() as pw:
        launch = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**launch)
        try:
            for renderer, phase in [('xterm', 'headers'), ('grid', 'body'), ('grid', 'timeout')]:
                root = Path(temporary) / (renderer + '-' + phase)
                root.mkdir()
                scenario(root, browser, renderer, phase)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
