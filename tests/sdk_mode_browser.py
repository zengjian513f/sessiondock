#!/usr/bin/env python3
"""Five SDK sources reconcile previous views against synthetic server ownership.

Real private provider bridges, node/Hub HTTP and Chromium. Ownership responses
are controlled HTTP fixtures; the managed Claude console uses a real private PTY.
No real CLI, provider account, native home or production session is used.
"""
import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from claude_sdk_fixtures import managed_console
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_mode_fixtures import provider
from session_start_fixtures import choose_mode


def exercise(browser, base, source, peer, screenshots, label):
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        print(f'Mode compatibility: {label} {source} {width} {theme}', flush=True)
        context = browser.new_context(viewport={'width': width, 'height': 900},
                                      color_scheme=theme, service_workers='block')
        page = context.new_page()
        page.set_default_timeout(15000)
        errors, opens = [], []
        state = {'mode': 'sdk'}
        page.add_init_script("""(() => {
          const original = window.fetch;
          window.sdkOpenCount = 0;
          window.fetch = function(url, options) {
            if (String(url).endsWith('/open')
                && (SdkView.discovering || SdkView.selection?.mode === 'loading')) window.sdkOpenCount++;
            return original.apply(this, arguments);
          };
        })();""")
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.on('request', lambda request: opens.append(request.post_data_json)
                if request.method == 'POST' and request.url.split('?')[0].endswith('/open') else None)

        def live(route):
            response = route.fetch()
            data = response.json()
            # The source's inventory has exactly one seeded session. /api/live
            # includes it after SDK open, but no selected UID exists at startup.
            selected = state.get('uid')
            if selected:
                data['uids'] = list(set(data.get('uids', []) + [selected]))
                data['sdk_uids'] = [uid for uid in data.get('sdk_uids', []) if uid != selected]
                if state['mode'] == 'sdk':
                    data['sdk_uids'].append(selected)
                if state['mode'] == 'error' and 'force=1' in route.request.url:
                    route.fulfill(status=503, json={'error': 'synthetic ownership unavailable'})
                    return
            route.fulfill(response=response, json=data)

        page.route('**/api/live*', live)

        def runtime(route):
            # Session switches read ownership from the per-session runtime GET.
            response = route.fetch()
            data = response.json()
            if state.get('uid') and data.get('uid') == state['uid']:
                if state['mode'] == 'error':
                    route.fulfill(status=503, json={'error': 'synthetic ownership unavailable'})
                    return
                data.update(sdk=state['mode'] == 'sdk', external=state['mode'] == 'cli', running=True, known=True)
            route.fulfill(response=response, json=data)

        page.route('**/api/session/runtime*', runtime)
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible():
            page.locator(f'#chips button[data-source="{source}"]').click()
        uid = row.get_attribute('data-uid')
        state['uid'] = uid
        assert ('~' in uid) == (label == 'hub'), uid
        # Old persistent Claude choices must never overrule server ownership.
        page.evaluate("uid => localStorage.setItem('sessiondock.claudeSessionViews', JSON.stringify({[uid]:'terminal'}))", uid)

        def choose():
            if page.locator('#detail .mobile-back').is_visible():
                page.locator('#detail .mobile-back').click()
            page.locator(f'#side .item[data-uid="{uid}"]').click()

        def capture(name):
            page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-{theme}-{name}.png'))

        choose()
        expect(page.locator('#sdk-input')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_enabled()
        page.locator('#a-chat').click()
        choose()
        expect(page.locator('#sdk-input')).to_be_visible()  # prior chat unsupported by SDK-only owner
        page.wait_for_function("SdkView.selection?.mode !== 'loading' && !SdkView.discovering && SdkView.connected")
        expect(page.locator('#sdk-send')).to_be_enabled()
        capture('sdk')
        # Reopening a warm session must paint before ownership/network reads.
        held = []
        def hold_live(route):
            held.append(route)
        page.route('**/api/live?*force=1*', hold_live)
        page.route('**/api/session/runtime*', hold_live)
        count = page.evaluate('window.sdkOpenCount')
        choose()
        expect(page.locator('.sdk-history .msg').first).to_be_visible(timeout=1000)
        expect(page.locator('#sdk-send')).to_be_disabled()
        page.wait_for_function("SdkView.discovering && SdkView.selection?.mode === 'loading'")
        assert page.evaluate('window.sdkOpenCount') == count
        capture('cached-pending')
        page.unroute('**/api/live?*force=1*', hold_live)
        page.unroute('**/api/session/runtime*', hold_live)
        assert held, 'ownership discovery request was not intercepted'
        for route in held:
            (runtime if '/api/session/runtime' in route.request.url else live)(route)
        try:
            expect(page.locator('#sdk-send')).to_be_enabled()
        except Exception:
            print(page.evaluate('({selection:SdkView.selection,discovering:SdkView.discovering,readOnly:SdkView.readOnly,connected:SdkView.connected,notice:SdkView.notice?.textContent})'), flush=True)
            raise

        # External CLI uses the configured mode, independent of old SDK views.
        if page.locator('#detail .mobile-back').is_visible():
            page.locator('#detail .mobile-back').click()
        choose_mode(page, source, 'cli')
        page.locator('#settings-dialog .modal-close').click()
        choose()
        page.locator('#a-chat').click()
        state['mode'] = 'cli'
        count = len(opens)
        choose()
        expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
        choose()  # compatible last chat remains chat
        expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
        assert len(opens) == count, opens
        page.reload(wait_until='networkidle')
        expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
        assert len(opens) == count, opens
        capture('cli')
        # Re-query on a cached selection: do not reuse old runtime evidence.
        state['mode'] = 'sdk'
        choose()
        expect(page.locator('#sdk-input')).to_be_visible()
        page.wait_for_function("SdkView.selection?.mode === 'sdk' && !SdkView.discovering && SdkView.connected")
        state['mode'] = 'cli'
        count = len(opens)
        choose()
        expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
        assert len(opens) == count, opens
        state['mode'] = 'error'
        # A failed forced read must not resurrect stale browser SDK ownership.
        page.evaluate('uid => S.liveSdk.add(uid)', uid)
        choose()
        expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
        assert len(opens) == count, opens
        assert page.evaluate("uid => JSON.parse(localStorage.getItem('sessiondock.claudeSessionViews'))[uid]", uid) == 'terminal'
        assert not errors, errors
        page.unroute_all(behavior='wait')
        context.close()


def console_check(browser, base, peer, screenshots):
    with managed_console(peer):
        context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
        page = context.new_page()
        page.goto(base, wait_until='networkidle')
        row = page.locator('#side .item[data-uid^="claude:"]').first
        uid = row.get_attribute('data-uid')
        # This test exercises actual host discovery and attachment. The bridge
        # used by the HTTP ownership matrix is explicitly idle and discarded.
        response = page.request.post(base+'/api/native-sdk/discard', data={'uid': uid})
        assert response.ok, response.text()
        row.click()
        page.wait_for_function("SdkView.selection?.mode !== 'loading'")
        expect(page.locator('#a-term')).to_have_attribute('aria-pressed', 'true')
        page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
        page.locator('#a-chat').click()
        row.click()
        expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
        page.locator('#a-term').click()
        row.click()
        expect(page.locator('#a-term')).to_have_attribute('aria-pressed', 'true')
        page.screenshot(path=str(screenshots/'managed-cli-compatible-terminal.png'))
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('target/sdk-mode'))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-mode-') as temporary, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in ('claude', 'grok', 'agy', 'opencode', 'codex'):
                root = Path(temporary)/source; root.mkdir()
                with provider(source, root, args.binary) as (base, peer, extra):
                    exercise(browser, base, source, peer, args.screenshots, 'node')
                    hub_root = root/'mode-hub'; hub_root.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                              [SimpleNamespace(name='ModeNode', token=TOKEN,
                                               port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', source, peer, args.screenshots, 'hub')
                    finally:
                        hub.stop()
                    if source == 'claude':
                        console_check(browser, base, peer, args.screenshots)
        finally:
            browser.close()
    print('PASS SDK mode compatibility: five sources, node/Hub, current backend ownership, compatible prior views, reload, failure and actual managed CLI')


if __name__ == '__main__':
    main()
