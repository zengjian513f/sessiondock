"""Confirm out-of-order account notifications before publishing a quota decrease."""
import argparse
import asyncio
from contextlib import ExitStack
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD, build, server
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from sdk_ui_fixtures import start_sdk


class QuotaServer(FakeAppServer):
    def emit(self, used, reset=1791000000):
        asyncio.run_coroutine_threadsafe(self.notify('account/rateLimits/updated', {
            'threadId': None, 'rateLimits': {'limitId': 'codex', 'secondary': {
                'usedPercent': used, 'windowDurationMins': 10080, 'resetsAt': reset}}}),
            self.loop).result(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args(); args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-quota-order-') as temp, ExitStack() as stack:
        root = Path(temp); corpus = build(root)
        fake = stack.enter_context(QuotaServer(root / 'app.sock', corpus.paths['sdk'], THREAD))
        state = root / 'state'; state.mkdir()
        port = free_port()
        env = {**node_env(root, port, '127.0.0.0/8'), 'SESSIONDOCK_STATE_DIR': str(state)}
        base = stack.enter_context(server(corpus, BINARY, root / 'app.sock', env))
        hubroot = root / 'hub'; hubroot.mkdir()
        hub = Hub(BINARY.with_name('sessiondock-hub'), hubroot,
                  [SimpleNamespace(name='Quota node', token=TOKEN, port=port)])
        hub.start(); stack.callback(hub.stop)
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            for mode, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
                for width, theme in [(1440, 'light'), (390, 'dark')]:
                    fake.used['secondary'] = 32
                    context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
                    page = context.new_page(); errors = []
                    page.on('pageerror', lambda e: errors.append(str(e)))
                    page.goto(url); page.wait_for_function('T.listLoaded')
                    uid = page.evaluate('sid=>S.sessions.find(r=>r.sid===sid).uid', THREAD)
                    if width < 720 and not page.locator('#side').is_visible():
                        page.locator('#detail .mobile-back').click()
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    start_sdk(page)
                    gauge = page.locator('[data-gauge="7d"] b')
                    expect(gauge).to_have_text('32%')
                    page.locator('#sdk-input').fill('draft survives quota confirmation')
                    tab = context.new_page(); tab.goto(url)
                    tab.wait_for_function('T.listLoaded')
                    if width < 720 and not tab.locator('#side').is_visible():
                        tab.locator('#detail .mobile-back').click()
                    tab.locator(f'#side .item[data-uid="{uid}"]').click()
                    tab.locator('#a-chat').click()
                    table = tab.locator('#conversation-status .sdk-gauge b').filter(has_text='32%')
                    expect(table).to_have_count(1)
                    page.locator('[data-gauge="7d"]').scroll_into_view_if_needed()
                    page.screenshot(path=str(args.screenshots / f'{mode}-{width}-before.png'))
                    held = []
                    page.route('**/api/codex-sdk/limits?*', lambda route: held.append(route))
                    # Concurrent inference responses carry 32, then an older 31.
                    # Delay the confirming account query; neither surface may flicker.
                    fake.emit(31)
                    page.wait_for_timeout(300)
                    page.locator('[data-gauge="7d"]').scroll_into_view_if_needed()
                    page.screenshot(path=str(args.screenshots / f'{mode}-{width}-pending.png'))
                    expect(gauge).to_have_text('32%')
                    expect(table).to_have_count(1)
                    if width < 720:
                        tab.locator('#conversation-status .sdk-gauges').hover()
                        tab.mouse.wheel(500, 0)
                        tab.wait_for_timeout(150)
                    tab.screenshot(path=str(args.screenshots / f'{mode}-{width}-shared.png'))
                    assert len(held) == 1, 'a decrease must be confirmed by an account read'
                    fake.emit(31); page.wait_for_timeout(100)
                    assert len(held) == 1, 'duplicate decreases must share the in-flight read'
                    with page.expect_response('**/api/codex-sdk/limits?*'):
                        held.pop().continue_()
                    expect(gauge).to_have_text('32%')
                    page.wait_for_function('!CodexSdkView.limitsRead')
                    # Query failures retain the known value and retry without
                    # publishing the unconfirmed decrease to another tab.
                    fake.emit(31)
                    page.wait_for_timeout(200)
                    assert len(held) == 1
                    held.pop().fulfill(status=503, json={'error': 'synthetic outage'})
                    page.wait_for_function('CodexSdkView.limitsRetryTimer !== 0')
                    expect(gauge).to_have_text('32%')
                    expect(table).to_have_count(1)
                    page.wait_for_timeout(1200)
                    assert len(held) == 1
                    held.pop().continue_()
                    page.wait_for_function('!CodexSdkView.limitsRead')
                    # A genuinely lower authoritative value is accepted, not clamped.
                    fake.used['secondary'] = 30
                    fake.emit(31)
                    page.wait_for_timeout(200)
                    assert len(held) == 1
                    held.pop().continue_()
                    expect(gauge).to_have_text('30%')
                    expect(tab.locator('#conversation-status .sdk-gauge b').filter(has_text='30%')).to_have_count(1)
                    page.wait_for_function('!CodexSdkView.limitsRead')
                    # A new window is a reset, not a stale same-window measurement.
                    fake.emit(0, reset=1791604800)
                    expect(gauge).to_have_text('0%')
                    assert not held
                    expect(page.locator('#sdk-input')).to_have_value('draft survives quota confirmation')
                    page.locator('[data-gauge="7d"]').scroll_into_view_if_needed()
                    page.screenshot(path=str(args.screenshots / f'{mode}-{width}-reset.png'))
                    assert not errors, errors
                    context.close()
                    print('PASS Codex quota order', mode, width, theme, flush=True)
            browser.close()


if __name__ == '__main__':
    main()
