#!/usr/bin/env python3
"""SDK native-history windows and explicit middle pages through Chromium.

Private synthetic JSONL, a fake daemon and real read-model/page/SSE routes.
"""
import argparse
from contextlib import contextmanager
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD, build, server
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, codex_message
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env


@contextmanager
def runtime(corpus, binary, root, mode):
    port = free_port()
    env = {**node_env(root, port, '127.0.0.0/8'), 'SESSIONDOCK_HISTORY_PAGE_EVENTS': '80'}
    with server(corpus, binary, root/'app.sock', env) as base:
        if mode == 'node':
            yield base
        else:
            hub_root = root/'hub'
            hub_root.mkdir()
            hub = Hub(binary.with_name('sessiondock-hub').resolve(), hub_root,
                      [SimpleNamespace(name='HistoryNode', token=TOKEN, port=port)])
            hub.start()
            try:
                yield f'http://127.0.0.1:{hub.port}'
            finally:
                hub.stop()


def exercise(browser, binary, mode, width, theme, screenshots):
    with tempfile.TemporaryDirectory(prefix='sd-sdk-pages-') as tmp:
        root = Path(tmp)
        corpus = build(root)
        path = corpus.paths['sdk']
        with path.open('a') as native:
            for index in range(750):
                for role in ('user', 'assistant'):
                    row = codex_message(role, f'{role} history row {index:04d} / 中间历史按需读取')
                    row['payload']['turn_id'] = f'past-turn-{index}'
                    native.write(json.dumps(row, ensure_ascii=False) + '\n')
        with FakeAppServer(root/'app.sock', path, THREAD) as fake, runtime(corpus, binary, root, mode) as base:
            context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
            page = context.new_page()
            errors, reads, held = [], [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.on('request', lambda request: reads.append(request.url) if '/api/messages/' in request.url else None)
            page.goto(base, wait_until='networkidle')
            page.locator('#side .item').filter(has_text='earlier synthetic question').first.click()
            history = page.locator('.sdk-history')
            gap = history.locator('.history-gap-load')
            expect(gap).to_be_visible()
            expect(history).to_contain_text('assistant history row 0749')
            expect(history).not_to_contain_text('history row 0300')
            page.wait_for_function('CodexSdkView.connected && !_es')
            assert len(reads) == 1 and parse_qs(urlsplit(reads[0]).query).get('window') == ['1'], reads
            assert page.evaluate('cache.get(viewKey(S.sel,S.agent)).msgs.some(m=>m.text.includes("history row 0300"))') is False
            if screenshots:
                screenshots.mkdir(parents=True, exist_ok=True)
                gap.scroll_into_view_if_needed()
                page.screenshot(path=str(screenshots/f'{mode}-{width}-{theme}-gap.png'))
            page.locator('#sdk-input').fill('草稿保持 / keep draft while paging')
            page.evaluate('HISTORY_PAGE_CHAIN = false')
            def state():
                return page.evaluate('''() => {
                  const e=cache.get(viewKey(S.sel,S.agent));
                  return {end:e.end, head:e.version.head, anchor:e.anchor, omitted:e.partial?.omitted || 0};
                }''')
            before = state()
            # The native live stream advances while a historical page is held.
            page.route('**/api/messages/*/page?*', lambda route: held.append(route))
            gap.click()
            assert len(held) == 1
            with path.open('a') as native:
                native.write(json.dumps(codex_message('assistant', 'SDK native live append while page is pending'))+'\n')
            expect(history).to_contain_text('SDK native live append while page is pending')
            live = state()
            assert live['end'] > before['end'] and live['omitted'] == before['omitted']
            held.pop().continue_()
            page.unroute('**/api/messages/*/page?*')
            page.wait_for_function('n => cache.get(viewKey(S.sel,S.agent)).partial?.omitted === n', arg=before['omitted']-80)
            after = state()
            assert {k: after[k] for k in ('end','head','anchor')} == {k: live[k] for k in ('end','head','anchor')}
            expect(page.locator('#sdk-input')).to_have_value('草稿保持 / keep draft while paging')
            expect(history).to_contain_text('SDK native live append while page is pending')
            # Failure stays in this SDK gap and explicit retry loads one page.
            page.route('**/api/messages/*/page?*', lambda route: held.append(route))
            gap.click()
            assert held
            with path.open('a') as native:
                native.write(json.dumps(codex_message('assistant', 'SDK native live append before page failure'))+'\n')
            expect(history).to_contain_text('SDK native live append before page failure')
            after = state()
            live = after
            held.pop().fulfill(status=409, json={'error':'Synthetic historical page conflict'})
            page.unroute('**/api/messages/*/page?*')
            expect(history.locator('.history-page-error')).to_contain_text('Synthetic historical page conflict')
            assert state() == after
            history.get_by_role('button', name='重试加载这一页').click()
            page.wait_for_function('n => cache.get(viewKey(S.sel,S.agent)).partial?.omitted === n', arg=after['omitted']-80)
            page.route('**/api/messages/*/page?*', lambda route: route.fulfill(
                status=410, json={'error':'Synthetic expired historical grant'}), times=1)
            gap.click()
            expect(history.locator('.history-page-error')).to_contain_text('Synthetic expired historical grant')
            history.get_by_role('button', name='重新载入当前历史').click()
            expect(history.locator('.history-page-error')).to_have_count(0)
            assert state()['end'] == live['end']
            # A page from a detached SDK generation cannot modify reentered history.
            page.route('**/api/messages/*/page?*', lambda route: held.append(route))
            gap.click()
            assert held
            pending_state = state()
            page.locator('#a-chat').click()
            page.locator('#a-sdk').click()
            expect(history.locator('.history-gap-load')).to_be_visible()
            held.pop().continue_()
            page.unroute('**/api/messages/*/page?*')
            page.wait_for_function('historyPageRequests.size === 0')
            assert state() == pending_state
            # Full SDK refresh after a turn still preserves the native gap.
            page.locator('#sdk-input').fill('hold synthetic SDK work')
            page.locator('#sdk-send').click()
            expect(page.locator('#sdk-stop')).to_be_visible()
            page.locator('#sdk-stop').click()
            expect(page.locator('#sdk-stop')).to_be_hidden()
            expect(history.locator('.history-gap-load')).to_be_visible()
            # A native rewrite makes the fallback cursor invalid. Reselecting
            # through the sidebar must reset to another window, not a full read.
            page.evaluate('_es?.close()')
            path.write_text(path.read_text().replace('earlier synthetic question', 'rewritten synthetic question', 1))
            if width < 500:
                page.locator('#detail .mobile-back').click()
            page.locator('#side .item').first.click()
            expect(history).to_contain_text('rewritten synthetic question')
            expect(history.locator('.history-gap-load')).to_be_visible()
            expect(history).not_to_contain_text('assistant history row 0300')
            expect(history).to_contain_text('SDK native live append before page failure')
            if screenshots:
                screenshots.mkdir(parents=True, exist_ok=True)
                page.screenshot(path=str(screenshots/f'{mode}-{width}-{theme}-window.png'))
            # One explicit click can chain finite pages; no unwindowed read.
            page.evaluate('HISTORY_PAGE_CHAIN = true')
            history.locator('.history-gap-load').click()
            expect(history.locator('.history-gap')).to_have_count(0, timeout=30000)
            expect(history).to_contain_text('assistant history row 0300')
            assert all('/page?' in url or parse_qs(urlsplit(url).query).get('window') == ['1'] for url in reads), reads
            assert not errors, errors
            context.close()
        print(f'PASS SDK pages {mode}/{width}/{theme}: window-only entry/reset, explicit pages, native live race, retry/reload, detach guard, completion, chain', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for mode, width, theme in [('node',1440,'light'),('node',390,'dark'),('hub',1440,'dark'),('hub',390,'light')]:
                exercise(browser, args.binary, mode, width, theme, args.screenshots)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
