"""Confirm out-of-order account notifications before publishing a quota decrease."""
import argparse
import asyncio
from datetime import datetime, timezone
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
    reset = 1791000000
    extra = False

    def _bucket(self):
        bucket = super()._bucket()
        bucket['planType'] = 'promax'
        bucket['secondary']['resetsAt'] = self.reset
        return bucket

    async def _handle(self, writer, message):
        if message.get('method') == 'account/rateLimits/read':
            bucket = self._bucket()
            limits = {'codex': bucket}
            if self.extra:
                limits['special'] = {'limitName': 'Special model', 'primary': {
                    'usedPercent': 9, 'windowDurationMins': 300}}
            await self.send(writer, {'id': message['id'], 'result': {
                'rateLimits': bucket, 'rateLimitsByLimitId': limits,
                'rateLimitResetCredits': {'availableCount': len(self.cards), 'credits': list(self.cards)}}})
            return
        await super()._handle(writer, message)

    def emit(self, used, reset=1791000000):
        asyncio.run_coroutine_threadsafe(self.notify('account/rateLimits/updated', {
            'threadId': None, 'rateLimits': {'limitId': 'codex', 'planType': 'pro', 'secondary': {
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
                    fake.reset = 1791000000
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
                    # BUG-20261107-104739-ad88f6: old-plan inference keeps
                    # reporting 3%, while the authoritative current plan is 1%.
                    fake.used['secondary'] = 1
                    fake.emit(1)
                    page.wait_for_timeout(200)
                    held.pop().continue_()
                    expect(gauge).to_have_text('1%')
                    page.wait_for_function('!CodexSdkView.limitsRead')
                    # A pre-upgrade tab must not poison the corrected account channel.
                    tab.evaluate('''uid => {
                        const channel=new BroadcastChannel(`sessiondock.quotas.${new URL('./',location.href).pathname}`);
                        channel.postMessage({key:UsageUI.accountKey(uid),quota:{source:'codex',
                            captured_at:new Date().toISOString(),buckets:[{id:'codex:secondary',
                            window:10080,remaining_percent:97}]}});
                        channel.close();
                    }''', uid)
                    page.wait_for_timeout(100)
                    expect(gauge).to_have_text('1%')
                    fake.emit(3)
                    page.wait_for_timeout(200)
                    assert len(held) == 1, 'increases from an old plan also need confirmation'
                    expect(gauge).to_have_text('1%')
                    expect(tab.locator('#conversation-status .sdk-gauge b').filter(has_text='1%')).to_have_count(1)
                    page.locator('[data-gauge="7d"]').scroll_into_view_if_needed()
                    page.screenshot(path=str(args.screenshots / f'{mode}-{width}-plan-pending.png'))
                    held.pop().continue_()
                    page.wait_for_function('!CodexSdkView.limitsRead')
                    expect(gauge).to_have_text('1%')
                    # A delayed full HTTP response must keep its server timestamp.
                    # Another tab has already published a newer account snapshot.
                    fake.emit(3)
                    page.wait_for_timeout(200)
                    old = held.pop()
                    response = old.fetch()
                    data = response.json()
                    assert data['captured_at']
                    newer = page.request.get(url + '/api/quotas' if mode == 'node' else
                        url + '/api/nodes/' + uid.split(':')[1].split('~')[0] + '/api/quotas').json()['agents'][0]
                    newer['captured_at'] = datetime.now(timezone.utc).isoformat()
                    for bucket in newer['buckets']:
                        if bucket['id'] == 'codex:secondary': bucket['remaining_percent'] = 98
                    # Use a real quota-page refresh to publish the newer response.
                    quota_tab = context.new_page(); quota_tab.goto(url + '/quotas.html')
                    expect(quota_tab.locator('#quota-refresh')).to_be_enabled()
                    quota_tab.route('**/api/quotas/refresh', lambda route: route.fulfill(json={'agents': [newer]}))
                    quota_tab.locator('#quota-refresh').click()
                    expect(gauge).to_have_text('2%')
                    old.fulfill(response=response)
                    page.wait_for_function('!CodexSdkView.limitsRead')
                    expect(gauge).to_have_text('2%')
                    expect(tab.locator('#conversation-status .sdk-gauge b').filter(has_text='2%')).to_have_count(1)
                    quota_tab.close()
                    # A genuine reset still works, through the same account read.
                    fake.used['secondary'] = 0
                    fake.reset = 1791604800
                    fake.emit(0, reset=fake.reset)
                    page.wait_for_timeout(200)
                    assert len(held) == 1
                    expect(gauge).to_have_text('2%')
                    held.pop().continue_()
                    expect(gauge).to_have_text('0%')
                    page.wait_for_function('!CodexSdkView.limitsRead')
                    fake.extra = True
                    fake.emit(0)
                    page.wait_for_timeout(200); held.pop().continue_()
                    expect(tab.locator('#conversation-status .sdk-gauge b').filter(has_text='9%')).to_have_count(1)
                    page.wait_for_function('!CodexSdkView.limitsRead')
                    fake.extra = False
                    fake.emit(0)
                    page.wait_for_timeout(200); held.pop().continue_()
                    expect(tab.locator('#conversation-status .sdk-gauge b').filter(has_text='9%')).to_have_count(0)
                    expect(page.locator('#sdk-input')).to_have_value('draft survives quota confirmation')
                    page.locator('[data-gauge="7d"]').scroll_into_view_if_needed()
                    page.screenshot(path=str(args.screenshots / f'{mode}-{width}-reset.png'))
                    assert not errors, errors
                    context.close()
                    print('PASS Codex quota order', mode, width, theme, flush=True)
            browser.close()


if __name__ == '__main__':
    main()
