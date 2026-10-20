#!/usr/bin/env python3
"""Session switches retire stale details before asynchronous mode selection.

Real private provider bridges, node/Hub HTTP and Chromium. Ownership responses
are controlled HTTP fixtures; native histories and SDK peers are synthetic.
No real CLI, provider account, native home or production session is used.
"""
import argparse
import re
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_message, codex_row
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_mode_fixtures import provider


def exercise(browser, base, source, screenshots, label):
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        print(f'Session switching: {label} {source} {width} {theme}', flush=True)
        context = browser.new_context(viewport={'width': width, 'height': 900},
                                      color_scheme=theme, service_workers='block')
        page = context.new_page()
        page.set_default_timeout(15000)
        errors, opens = [], []
        state = {}
        held = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.on('request', lambda request: opens.append(request.post_data_json)
                if request.method == 'POST' and request.url.split('?')[0].endswith('/open') else None)

        def live(route):
            response = route.fetch()
            data = response.json()
            # Advertise only the selected fixture as SDK-owned, so viewing it
            # reconnects to a synthetic bridge without a real CLI/account.
            selected = state.get('uid')
            if selected:
                data['uids'] = list(set(data.get('uids', []) + [selected]))
                data['sdk_uids'] = [uid for uid in data.get('sdk_uids', []) if uid != selected]
                data['sdk_uids'].append(selected)
            route.fulfill(response=response, json=data)

        def delayed_live(route):
            if state.get('hold') and 'force=1' in route.request.url:
                held.append(route)
            else:
                live(route)
        page.route('**/api/live*', delayed_live)
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded')
        rows = page.locator(f'#side .item[data-uid^="{source}:"]').filter(
            has_not_text='NAVIGATION OLD SESSION')
        row = rows.filter(has_text='Synthetic Kiro V3 Session').first if source == 'kiro' else rows.first
        if not row.is_visible():
            page.locator(f'#chips button[data-source="{source}"]').click()
        uid = row.get_attribute('data-uid')
        title = page.evaluate('uid => S.sessions.find(row => row.uid === uid).title', uid)
        state['uid'] = uid
        assert ('~' in uid) == (label == 'hub'), uid

        def choose():
            if page.locator('#detail .mobile-back').is_visible():
                page.locator('#detail .mobile-back').click()
            page.locator(f'#side .item[data-uid="{uid}"]').click()
            if not state.get('hold'):
                page.wait_for_function("SdkView.selection?.mode !== 'loading'")
                expect(page.locator('#detail .spin')).to_have_count(0)

        def capture(name):
            page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-{theme}-{name}.png'))

        # Both a cold selection and a cached reopen must retire the previous
        # header/messages before waiting for current SDK/CLI ownership.
        for cached in (False, True):
            if page.locator('#detail .mobile-back').is_visible():
                page.locator('#detail .mobile-back').click()
            old = page.locator('#side .item').filter(has_text='NAVIGATION OLD SESSION')
            old.click()
            expect(page.locator('#msgs')).to_contain_text('NAVIGATION OLD SESSION')
            old_uid = old.get_attribute('data-uid')
            opening_count = len(opens)
            state['hold'] = True
            choose()
            page.wait_for_function("SdkView.selection?.mode === 'loading'")
            assert held, 'selection must actually be waiting for the ownership response'
            capture('cached-selection-wait' if cached else 'cold-selection-wait')
            expect(page.locator('#detail')).not_to_contain_text('NAVIGATION OLD SESSION')
            expect(page.locator('#detail .spin')).to_have_text('正在读取会话…')
            assert page.evaluate('S.sel') == uid
            assert page.evaluate('uid => cache.has(viewKey(uid, null))', uid) == cached
            expect(page.locator('#detail .dhead h2')).to_have_text(title)
            assert len(opens) == opening_count, 'ownership lookup must not open the SDK'
            state['hold'] = False
            for route in held:
                live(route)
            held.clear()
            expect(page.locator('#sdk-input')).to_be_visible()
            expect(page.locator('#sdk-send')).to_be_enabled()
            assert page.evaluate('S.sel') != old_uid
            capture('cached-selection-ready' if cached else 'cold-selection-ready')

        # Leave a pending selection and select another session. Its late
        # ownership response must not reopen the abandoned view or mobile page.
        state['hold'] = True
        choose()
        page.wait_for_function("SdkView.selection?.mode === 'loading'")
        assert held
        if width < 720:
            page.locator('#detail .mobile-back').click()
            expect(page.locator('body')).not_to_have_class(re.compile(r'.*mobile-detail.*'))
        state['hold'] = False
        old.click()
        expect(page.locator('#msgs')).to_contain_text('NAVIGATION OLD SESSION')
        for route in held:
            live(route)
        held.clear()
        expect(page.locator('#detail .dhead h2')).to_have_text('NAVIGATION OLD SESSION')
        assert page.evaluate('S.sel') == old_uid
        capture('superseded-selection')
        assert not errors, errors
        page.unroute_all(behavior='wait')
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('target/session-switch'))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-session-switch-') as temporary, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in ('claude', 'grok', 'agy', 'opencode', 'codex', 'kiro'):
                root = Path(temporary)/source; root.mkdir()
                with provider(source, root, args.binary) as (base, peer, extra):
                    Corpus(root).put('navigation-old', 'codex', [
                        codex_row('session_meta', {'id': 'navigation-old', 'cwd': '/synthetic/navigation'}),
                        codex_message('user', 'NAVIGATION OLD SESSION'),
                        codex_message('assistant', 'Previous session content must leave with its title.')], [])
                    exercise(browser, base, source, args.screenshots, 'node')
                    # The current Hub does not advertise Kiro SDK capability,
                    # so it has no asynchronous SDK ownership selection path.
                    if source == 'kiro':
                        continue
                    hub_root = root/'mode-hub'; hub_root.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                              [SimpleNamespace(name='ModeNode', token=TOKEN,
                                               port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', source, args.screenshots, 'hub')
                    finally:
                        hub.stop()
        finally:
            browser.close()
    print('PASS session switching: six node sources and five Hub SDK sources, desktop/light and mobile/dark, cold/cached selections and superseded ownership responses')


if __name__ == '__main__':
    main()
