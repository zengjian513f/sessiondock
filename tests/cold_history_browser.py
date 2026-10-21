#!/usr/bin/env python3
"""Cold history remains readable while runtime ownership requests are pending."""
import argparse
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_mode_fixtures import provider


def exercise(browser, base, source, output, label, baseline):
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        context = browser.new_context(viewport={'width': width, 'height': 900},
                                      color_scheme=theme, service_workers='block')
        page = context.new_page()
        errors, opens, reads, held = [], [], [], []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.on('request', lambda request: opens.append(request.url)
                if request.method == 'POST' and request.url.split('?')[0].endswith('/open') else None)
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible():
            page.locator(f'#chips button[data-source="{source}"]').click()
        uid = row.get_attribute('data-uid')
        assert not page.evaluate('uid => cache.has(viewKey(uid, null))', uid)
        page.evaluate("""() => {
          window.coldReads = [];
          const original = fetchMessages;
          fetchMessages = function(uid, options) {
            coldReads.push({uid, start: options?.start, windowed: options?.windowed});
            return original.apply(this, arguments);
          };
        }""")

        def history(route):
            reads.append(route.request.url)
            response = route.fetch()
            data = response.json()
            # Preserve real server metadata/checkpoints, with the report's 205
            # message window and lengthy folded process content.
            messages = []
            for turn in range(5):
                tid = f'cold-{turn}'
                messages.append({'id': tid, 'turn_id': tid, 'role': 'user',
                                 'text': f'第 {turn + 1} 轮：检查读取链路与历史窗口。'})
                for index in range(39):
                    messages.append({'id': f'{tid}-{index}', 'turn_id': tid,
                                     'role': 'thinking', 'text': '核对历史记录与读取顺序。\n' * 100})
                messages.append({'id': f'{tid}-final', 'turn_id': tid, 'role': 'assistant',
                                 'phase': 'final', 'text': f'第 {turn + 1} 轮结论：历史可以独立读取。'})
            if data.get('reset'):
                data.update(messages=messages, message_total=len(messages), partial=None)
            route.fulfill(response=response, json=data)

        page.route('**/api/messages/**', history)
        page.route('**/api/live?*force=1*', lambda route: held.append(route))
        page.route('**/api/term/list?*force=1*', lambda route: held.append(route))
        profiler = context.new_cdp_session(page)
        profiler.send('Profiler.enable')
        profiler.send('Profiler.start')
        started = time.monotonic()
        row.click()
        page.wait_for_function("SdkView.selection?.mode === 'loading'")
        name = f'{label}-{source}-{width}-{theme}'
        if baseline:
            page.wait_for_timeout(1500)  # Bounded observation of the blocked old path.
            assert not reads, reads
            expect(page.locator('#detail .spin')).to_be_visible()
        else:
            expect(page.locator('.sdk-history')).to_contain_text('第 5 轮结论', timeout=2000)
            expect(page.locator('#sdk-send')).to_be_disabled()
            assert page.evaluate('SdkView.discovering && SdkView.selection.mode === "loading"')
            # An empty native catalog has checkpoint zero: its ordinary tail
            # poll also omits start from the URL. Distinguish that poll from a
            # second cold read by the actual read options.
            calls = page.evaluate('coldReads')
            assert len([call for call in calls if call.get('start') is None]) == 1, calls
            assert all('window=1' in url and '/page?' not in url for url in reads), reads
            page.locator('#sdk-input').fill('等待确认期间保留草稿')
            page.locator('#sdk-input').press('Enter')
        elapsed = round((time.monotonic() - started) * 1000)
        assert held and not opens, (len(held), opens)
        page.screenshot(path=str(output / f'{name}-pending.png'), full_page=True)
        profile = profiler.send('Profiler.stop')['profile']
        (output / f'{name}.cpuprofile').write_text(json.dumps(profile))
        print(f'{name}: history_requests={len(reads)}, pending_observation_ms={elapsed}', flush=True)
        # Failed discovery retains readable history and falls back to the
        # existing mode rules. It must never start a native writer.
        for route in held:
            route.fulfill(status=503, json={'error': 'synthetic ownership unavailable'})
        page.unroute('**/api/live?*force=1*')
        page.unroute('**/api/term/list?*force=1*')
        page.wait_for_function("SdkView.selection?.mode !== 'loading' && !SdkView.discovering")
        expect(page.locator('#detail')).to_contain_text('第 5 轮结论')
        assert not opens, opens
        assert not errors, errors
        page.screenshot(path=str(output / f'{name}-settled.png'), full_page=True)
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('target/cold-history'))
    parser.add_argument('--sources', default='codex,claude,grok,agy,opencode,kiro')
    parser.add_argument('--baseline', action='store_true')
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-cold-history-') as temporary, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(temporary) / source
                root.mkdir()
                with provider(source, root, args.binary) as (base, peer, extra):
                    exercise(browser, base, source, args.screenshots, 'node', args.baseline)
                    hub_root = root / 'hub'; hub_root.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                              [SimpleNamespace(name='HistoryNode', token=TOKEN,
                                               port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', source,
                                 args.screenshots, 'hub', args.baseline)
                    finally:
                        hub.stop()
        finally:
            browser.close()
    print('PASS cold history: pending ownership, windowed read, disabled sends, failure recovery, node/Hub')


if __name__ == '__main__':
    main()
