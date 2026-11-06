"""Quota collection publishes to already-open SDK views, dialogs and tabs."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import sync_playwright, expect
from codex_sdk_browser import server
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from kiro_sdk_fixtures import KiroFixture
from node_auth_fixtures import TOKEN, node_env
from send_browser import initialize
from sdkhost_fixtures import stop_private_sdkhosts
from sdk_ui_fixtures import start_sdk
from sdk_usage_fixtures import refresh_usage


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-quota-sync-') as temp, ExitStack() as stack:
        root = Path(temp)
        for name in ('claude', 'codex', 'grok', 'state', 'trash'):
            (root / name).mkdir()
        fixture = KiroFixture(root, host_binary=BINARY.with_name('ptyhost'))
        initialize('--initialize-lifecycle', root / 'ledger', BINARY)
        stack.callback(stop_private_sdkhosts, root)
        port = free_port()
        env = {**fixture.env, **node_env(root, port, '127.0.0.0/8'),
               'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_STATE_DIR': str(root / 'state'),
               'SESSIONDOCK_LIFECYCLE_DIR': str(root / 'ledger'),
               'SESSIONDOCK_PTYHOST_DIR': str(fixture.host_dir),
               'SESSIONDOCK_TRASH_DIR': str(root / 'trash')}
        base = stack.enter_context(server(SimpleNamespace(root=root), BINARY, 'off', env))
        hubroot = root / 'hub'; hubroot.mkdir()
        hub = Hub(BINARY.with_name('sessiondock-hub'), hubroot,
                  [SimpleNamespace(name='Quota node', token=TOKEN, port=port)])
        hub.start(); stack.callback(hub.stop)
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            for mode, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
                used = root / 'kiro-credits-used'
                used.write_text('2970')
                # Seed only synthetic account state, never production sessions.
                (root / 'state/quotas').mkdir(exist_ok=True)
                (root / 'state/quotas/kiro.json').write_text(json.dumps({
                    'source': 'kiro', 'captured_at': '2020-01-01T00:00:00Z', 'buckets': []}))
                context = browser.new_context(viewport={'width': 1280, 'height': 900})
                page = context.new_page(); errors = []
                page.on('pageerror', lambda e: errors.append(str(e)))
                page.goto(url)
                page.wait_for_function('async sid => { await loadSessions(true); return S.sessions.some(r=>r.sid===sid); }', arg=fixture.sid)
                uid = page.evaluate('sid=>S.sessions.find(r=>r.sid===sid).uid', fixture.sid)
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                gauge = page.locator('.sdk-pane .sdk-gauges .sdk-gauge').filter(has_text='月度').locator('b')
                # Explicit quota page refresh also supports a stopped SDK.
                page.locator('#sdk-input').fill('draft survives quota refresh')
                page.locator('#settings').click()
                page.get_by_role('tab', name='机器', exact=True).click()
                page.locator('#quota-open').click()
                table = page.locator('#quota-dialog [data-source="kiro"]')
                page.locator('#quota-refresh').click()
                expect(table.locator('.quota-remaining b')).to_have_text('29.7%', timeout=30000)
                expect(gauge).to_have_text('29.7%')
                page.screenshot(path=str(args.screenshots / f'{mode}-before.png'))
                used.write_text('3080')
                page.locator('#quota-refresh').click()
                expect(table.locator('.quota-remaining b')).to_have_text('30.8%', timeout=30000)
                expect(gauge).to_have_text('30.8%')
                for width, theme in [(1280, 'light'), (390, 'dark')]:
                    page.set_viewport_size({'width': width, 'height': 900})
                    page.emulate_media(color_scheme=theme)
                    page.screenshot(path=str(args.screenshots / f'{mode}-{width}-dialog.png'))
                page.keyboard.press('Escape'); page.keyboard.press('Escape')
                expect(page.locator('#sdk-input')).to_have_value('draft survives quota refresh')
                tab = context.new_page(); tab.on('pageerror', lambda e: errors.append(str(e)))
                tab.goto(url + '/quotas.html')
                expect(tab.locator('#quota-refresh')).to_be_enabled()
                used.write_text('3190')
                tab.locator('#quota-refresh').click()
                expect(tab.locator('[data-source="kiro"] .quota-remaining b')).to_have_text('31.9%', timeout=30000)
                expect(gauge).to_have_text('31.9%')
                # Late stale responses cannot roll either surface back.
                stale = {'agents': [{'source': 'kiro', 'captured_at': '2020-01-01T00:00:00Z',
                                    'buckets': [{'window': 'monthly', 'remaining_percent': 99}]}]}
                tab.route('**/api/quotas/refresh', lambda r: r.fulfill(json=stale))
                tab.locator('#quota-refresh').click()
                expect(tab.locator('#quota-refresh')).to_be_enabled()
                expect(tab.locator('[data-source="kiro"] .quota-remaining b')).to_have_text('31.9%')
                expect(gauge).to_have_text('31.9%')
                tab.unroute('**/api/quotas/refresh')
                tab.route('**/api/quotas/refresh', lambda r: r.fulfill(status=503, json={'error': 'offline'}))
                tab.locator('#quota-refresh').click()
                expect(tab.locator('#quota-status')).to_contain_text('读取失败')
                expect(gauge).to_have_text('31.9%')
                tab.unroute('**/api/quotas/refresh')
                page.set_viewport_size({'width': 1280, 'height': 900})
                start_sdk(page)
                used.write_text('3300')
                refresh_usage(page)
                expect(gauge).to_have_text('33%', timeout=30000)
                expect(tab.locator('[data-source="kiro"] .quota-remaining b')).to_have_text('33%')
                for width, theme in [(1280, 'light'), (390, 'dark')]:
                    page.set_viewport_size({'width': width, 'height': 900})
                    page.emulate_media(color_scheme=theme)
                    if width < 720 and not page.locator('#sdk-input').is_visible():
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(gauge).to_have_text('33%')
                    page.screenshot(path=str(args.screenshots / f'{mode}-{width}-sdk.png'))
                assert not fixture.received('session/prompt'), 'Quota synchronization started inference'
                assert not errors, errors
                context.close()
                print('PASS quota synchronization', mode, flush=True)
            browser.close()


if __name__ == '__main__':
    main()
