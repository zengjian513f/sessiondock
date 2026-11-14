#!/usr/bin/env python3
"""Read and fill the shared history gap in every provider's SDK/chat entry."""
import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_mode_fixtures import provider


def exercise(browser, base, source, label, screenshots):
    for width, theme in ((1440, 'light'), (390, 'dark')):
        context = browser.new_context(viewport={'width': width, 'height': 900},
                                      color_scheme=theme, service_workers='block')
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        rows = [{'role': 'user' if i % 2 == 0 else 'assistant', 'phase': 'final',
                 'turn_id': f'gap-turn-{i // 2}',
                 'text': f'历史记录 {i:04d}：检查较早与较近的消息。'} for i in range(810)]

        def messages(route):
            query = parse_qs(urlsplit(route.request.url).query)
            if '/page?' in route.request.url:
                cursor = query['cursor'][0]
                start, stop = int(cursor[:16], 16), int(cursor[16:], 16)
                limit = int(query.get('limit', ['200'])[0])
                if query.get('direction') == ['backward']:
                    begin, end = max(start, stop - limit), stop
                    next_start, next_stop = start, begin
                else:
                    begin, end = start, min(stop, start + limit)
                    next_start, next_stop = end, stop
                remaining = next_stop - next_start
                route.fulfill(json={'messages': rows[begin:end], 'page': {
                    'cursor': query['cursor'][0], 'start': begin, 'end': end, 'stop': stop,
                    'remaining': remaining,
                    'next': f'{next_start:016x}{next_stop:016x}' if remaining else None}})
            else:
                response = route.fetch()
                data = response.json()
                if 'start' in query:
                    data.update(messages=[], reset=False, partial=None)
                else:
                    data.update(messages=rows[:4] + rows[-200:], reset=True,
                                message_total=len(rows), partial={
                                    'head': 4, 'tail': 200, 'omitted': 606,
                                    'batch': 200, 'cursor': f'{4:016x}{610:016x}'})
                route.fulfill(response=response, json=data)

        context.route('**/api/messages/**', messages)
        context.route('**/api/watch?*', lambda r: r.fulfill(
            content_type='text/event-stream', body=': synthetic history\n\n'))
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible():
            page.locator(f'#chips button[data-source="{source}"]').click()
        row.click()
        modes = ('sdk',) if source in ('kiro', 'codebuddy', 'droid') else ('sdk', 'chat')
        for mode in modes:
            # Reopen a fresh bounded window through the actual page entry.
            page.reload(wait_until='domcontentloaded')
            page.wait_for_function('T.listLoaded')
            page.locator('#a-' + mode).click()
            box = page.locator('.sdk-history' if mode == 'sdk' else '#msgs')
            gap = box.locator('.history-gap')
            expect(gap.locator('.history-gap-load')).to_have_text('载入全部606条')
            gap.scroll_into_view_if_needed()
            name = f'{label}-{source}-{mode}-{width}-{theme}'
            controls = gap.locator('.history-gap-button')
            expect(controls).to_have_text(['↓ 载入200条', '载入全部606条', '↑ 载入200条'])
            rects = [controls.nth(i).bounding_box() for i in range(3)]
            assert all(rects[i]['y'] + rects[i]['height'] <= rects[i + 1]['y']
                       for i in range(2)), f'{name}: controls must stack top to bottom'
            assert max(r['x'] + r['width'] / 2 for r in rects) - min(
                r['x'] + r['width'] / 2 for r in rects) < 1, f'{name}: controls must share a center'
            assert controls.evaluate_all('nodes => nodes.every(n => {const s=getComputedStyle(n); '
                'return s.borderTopWidth === "0px" && s.borderRadius === "0px" && s.boxShadow === "none"})'), name
            if screenshots:
                page.screenshot(path=str(screenshots / f'{name}.png'), full_page=True)
                gap.screenshot(path=str(screenshots / f'{name}-gap.png'))
            assert gap.evaluate('(g) => g.scrollWidth <= g.clientWidth'), name
            earlier = gap.get_by_role('button', name='↓ 载入200条', exact=True)
            expect(earlier).to_have_attribute('title', '从中间未载入消息中，读取时间最早，紧接上方内容的最多 200 条')
            earlier.click()
            expect(gap.locator('.history-gap-load')).to_have_text('载入全部406条')
            expect(box).to_contain_text('历史记录 0004')
            expect(box).not_to_contain_text('历史记录 0409')
            gap.get_by_role('button', name='↑ 载入200条', exact=True).click()
            expect(gap.locator('.history-gap-load')).to_have_text('载入全部206条')
            expect(box).to_contain_text('历史记录 0609')
            expect(box).not_to_contain_text('历史记录 0409')
            gap.get_by_role('button', name='载入全部206条', exact=True).click()
            expect(gap).to_have_count(0)
            expect(box).to_contain_text('历史记录 0409')
            print(f'PASS {name}: gap scope, chronological labels, both ends, remaining count, completion', flush=True)
        assert not errors, errors
        context.unroute_all(behavior='wait')
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-gap-labels-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in ('codex', 'claude', 'grok', 'agy', 'opencode', 'kiro', 'codebuddy', 'droid'):
                root = Path(tmp) / source
                root.mkdir()
                with provider(source, root, args.binary) as (base, peer, extra):
                    exercise(browser, base, source, 'node', args.screenshots)
                    hub_root = root / 'hub'
                    hub_root.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                              [SimpleNamespace(name='GapNode', token=TOKEN,
                               port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', source, 'hub', args.screenshots)
                    finally:
                        hub.stop()
        finally:
            browser.close()


if __name__ == '__main__':
    main()
