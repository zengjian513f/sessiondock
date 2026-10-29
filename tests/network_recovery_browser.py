#!/usr/bin/env python3
"""Weak network UI: stalled headers/bodies, half-open SSE and lost SEND replies.

Every fault follows real clicks/typing against private fixtures. The 12/20/30 s
production deadlines are exercised without replacing fetch or shortening timers.
"""
import argparse
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright, expect
from composer_editor_browser import fixture
from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from codex_sdk_queue_browser import QueueServer
from frontend_framework_browser import launch_chromium
from header_fold_browser import corpus, SID
from history_fixtures import BINARY, Corpus, claude_row, codex_message, encoded, isolated_server
from network_fault_fixtures import StalledResponses
from private_hosts import private_hosts
from popups import on_popup
from send_browser import create_claude, wait_history
from session_start_fixtures import choose_mode
from sdk_ui_fixtures import start_sdk


def repeated_reconnect(browser, binary, screenshots=None):
    with tempfile.TemporaryDirectory(prefix='sessiondock-network-reconnect-') as tmp, StalledResponses() as faults:
        root = Path(tmp)
        data = build(root)
        with QueueServer(root / 'app.sock', data.paths['sdk'], THREAD) as fake, \
                server(data, binary, root / 'app.sock', environment(root, binary)) as base:
            for sdk, width, theme in ((True, 472, 'light'), (False, 1280, 'dark')):
                retained = 'earlier synthetic answer' if sdk else 'Recovered True history sentinel'
                context = browser.new_context(viewport={'width': width, 'height': 850}, color_scheme=theme)
                page = context.new_page()
                page.goto(base, wait_until='domcontentloaded')
                page.locator(f'#side .item[data-uid="{data.uid("sdk")}"]').click()
                start_sdk(page)
                print(f'Opened synthetic SDK at {width}px', flush=True)
                expect(page.locator('.sdk-history')).to_contain_text(retained)
                if not sdk:
                    page.locator('#a-chat').click()
                else:
                    page.locator('#sdk-input').fill('keep this unsent draft through reconnect')
                selector = '.sdk-history' if sdk else '#msgs'
                blocked, reads, connections = [True], [], []

                def stall_read(route):
                    reads.append(route.request.url)
                    if blocked[0]:
                        faults.redirect(route, 'headers')
                    else:
                        route.continue_()

                def track_events(route):
                    connections.append(route.request.url)
                    route.continue_()

                page.route('**/api/messages/*?*', stall_read)
                page.route('**/api/events', track_events)
                # Real periodic history reads reach their production deadline.
                expect(page.locator('#network-status')).to_contain_text('会话同步暂时中断', timeout=35000)
                expect(page.locator(selector)).to_contain_text(retained)
                if screenshots:
                    page.screenshot(path=str(screenshots / f'{"sdk" if sdk else "chat"}-stalled.png'), full_page=True)
                before = len(connections)
                for _ in range(6):
                    page.get_by_role('button', name='立即重连', exact=True).click()
                page.wait_for_timeout(300)
                assert len(connections) == before + 1, ('reconnect restarted an in-flight stream', len(connections) - before)
                blocked[0] = False
                with data.paths['sdk'].open('ab') as stream:
                    stream.write(encoded(codex_message('assistant', f'Recovered {sdk} history sentinel')))
                faults.release()
                expect(page.locator('#network-status')).to_have_count(0, timeout=15000)
                expect(page.locator(selector)).to_contain_text(f'Recovered {sdk} history sentinel', timeout=15000)
                if sdk:
                    expect(page.locator('#sdk-input')).to_have_value('keep this unsent draft through reconnect')
                    assert not fake.methods('turn/start'), 'network recovery must never SEND'
                # A completed attempt does not suppress later explicit recovery.
                context.set_offline(True)
                expect(page.locator('#network-status')).to_contain_text('网络已断开')
                before = len(connections)
                context.set_offline(False)
                expect(page.locator('#network-status')).to_have_count(0, timeout=15000)
                page.wait_for_function('uiEventsReady')
                assert len(connections) > before
                if screenshots:
                    page.screenshot(path=str(screenshots / f'{"sdk" if sdk else "chat"}-recovered.png'), full_page=True)
                page.unroute_all(behavior='ignoreErrors')
                context.close()
                print(f'PASS repeated reconnect coalesced, history/draft retained and later recovery ({"SDK/mobile" if sdk else "chat/desktop"})', flush=True)


def history(browser, binary):
    with tempfile.TemporaryDirectory(prefix='sessiondock-network-history-') as tmp, StalledResponses() as faults:
        data = corpus(Path(tmp))
        with isolated_server(data, binary) as (base, _):
            context = browser.new_context(viewport={'width':390, 'height':850})
            page = context.new_page()
            page.goto(base, wait_until='networkidle')
            reads = []
            def stalled_read(route):
                reads.append(route.request.url)
                if len(reads) == 1: faults.redirect(route, 'headers')
                else: route.continue_()
            page.route('**/api/messages/*?*', stalled_read)
            page.locator(f'#side .item[data-uid="{data.uid(SID)}"]').click()
            expect(page.locator('#detail')).to_contain_text('读取超时', timeout=15000)
            expect(page.get_by_role('button', name='重试读取', exact=True)).to_be_visible()
            expect(page.locator('#msgs')).to_contain_text('Fold sweep question', timeout=10000)
            assert len(reads) >= 2
            faults.release(); page.unroute('**/api/messages/*?*')
            context.set_offline(True)
            expect(page.locator('#network-status')).to_contain_text('网络已断开')
            expect(page.locator('#msgs')).to_contain_text('Fold sweep question')
            context.set_offline(False)
            expect(page.locator('#network-status')).to_have_count(0, timeout=15000)
            print('PASS first-read timeout/retry and mobile offline notice with history retained', flush=True)
            context.close()

            context = browser.new_context(viewport={'width':1280, 'height':900})
            page = context.new_page()
            connections = []
            def stall_events(route):
                connections.append(route.request.url)
                if len(connections) == 1: faults.redirect(route, 'events')
                else: route.continue_()
            page.route('**/api/events', stall_events)
            page.goto(base, wait_until='networkidle')
            page.wait_for_function('uiEventsReady && !uiEventApplying')
            sid = 'network-recovery-new-row'
            data.put(sid, 'claude', [claude_row(sid, 'user', 'net-u', None, 'New row after stalled events')], [])
            expect(page.locator('#network-status')).to_contain_text('实时同步连接中断', timeout=35000)
            expect(page.locator(f'#side .item[data-uid="{data.uid(sid)}"]')).to_be_visible(timeout=12000)
            expect(page.locator('#network-status')).to_have_count(0)
            assert len(connections) == 2, connections
            print('PASS half-open SSE detected, visible reconnect, new list baseline without reload', flush=True)
            context.close()


def composer(browser, binary):
    with tempfile.TemporaryDirectory(prefix='sessiondock-network-composer-') as tmp, private_hosts(Path(tmp)), StalledResponses() as faults:
        root = Path(tmp)
        launcher = fixture(root, binary)
        with isolated_server(Corpus(root), binary, host_dir=root/'host', lifecycle_dir=root/'ledger',
                launcher_config=launcher, state_dir=root/'state', file_roots=(root/'work',),
                file_write_roots=(root/'work',)) as (base, _):
            context = browser.new_context(viewport={'width':1280, 'height':900})
            page = context.new_page()
            errors, sends = [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.on('request', lambda request: sends.append(request.post_data_json)
                if request.url.endswith('/api/session/conversation/send') else None)
            on_popup(page, lambda dialog: dialog.accept())
            page.goto(base, wait_until='networkidle')
            choose_mode(page, 'claude', 'cli')
            page.locator('#settings-dialog .modal-close').click()
            create_claude(page, base, root/'work', open_terminal=False)
            page.wait_for_function('composerUid && !composerDraft().loading && composerDraft().inputStatus?.state === "ready"')
            blocked = {'mode':'body'}
            saves = []
            def save(route):
                saves.append(route.request.post_data_json)
                if blocked['mode'] == 'body': faults.redirect(route, 'body')
                elif blocked['mode'] == 'fail': route.fulfill(status=503, json={'error':'synthetic save outage'})
                else: route.continue_()
            page.route('**/api/session/conversation', save)
            page.locator('#cinput').fill('draft body must time out')
            expect(page.locator('.draft-save-error')).to_contain_text('超时', timeout=15000)
            blocked['mode'] = ''; faults.release()
            page.wait_for_function('!composerSaving.has(composerDraft()) && !composerDraft().storageError && composerDraft().savedVersion === composerDraft().editVersion', timeout=20000)
            expect(page.locator('#cinput')).to_have_value('draft body must time out')
            assert len(saves) >= 2
            print('PASS response body timeout, automatic draft recovery and unchanged text', flush=True)

            blocked['mode'] = 'fail'
            page.locator('#csend').click()
            page.wait_for_function('!composerSending && !!composerDraft().storageError')
            count = len(saves); blocked['mode'] = ''
            page.wait_for_function('!composerSaving.has(composerDraft()) && !composerDraft().storageError', timeout=10000)
            assert len(saves) > count and len(sends) == 0
            page.unroute('**/api/session/conversation')
            print('PASS failed submission preparation resumes draft saves, never auto-SEND', flush=True)

            page.route('**/api/session/conversation/send', lambda route: faults.redirect(route, 'body'))
            page.locator('#csend').click()
            expect(page.locator('#cinput')).to_have_value('', timeout=26000)
            page.wait_for_function('!composerSending')
            assert len(sends) == 1
            wait_history(page, 'draft body must time out')
            faults.release(); page.unroute('**/api/session/conversation/send')
            assert not errors, errors
            print('PASS stalled SEND receipt is queried automatically, exactly one write', flush=True)
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as pw:
        browser = launch_chromium(pw)
        try:
            repeated_reconnect(browser, args.binary, args.screenshots)
            history(browser, args.binary)
            composer(browser, args.binary)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
