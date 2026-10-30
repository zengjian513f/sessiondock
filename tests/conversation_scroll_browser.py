#!/usr/bin/env python3
"""SDK and CLI conversation history share bottom-follow behavior across sources."""
import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from sdk_mode_fixtures import provider
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN


def exercise(browser, base, source, screenshots, label, baseline=False):
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        context = browser.new_context(viewport={'width': width, 'height': 900},
                                      color_scheme=theme, service_workers='block')
        page = context.new_page()
        page.set_default_timeout(15000)
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        history = []
        for index in range(24):
            history.extend([
                {'role': 'user', 'text': f'问题 {index}：请解释会话历史如何展示。', 'turn_id': f'turn-{index}'},
                {'role': 'assistant', 'phase': 'final', 'turn_id': f'turn-{index}',
                 'text': f'答复 {index}：打开会话时显示最新消息，向上翻阅时保留阅读位置。\n\n'
                         '这是一段用于检查长历史、换行与滚动的合成内容。' * 3}])
        def messages(route):
            response = route.fetch()
            data = response.json()
            start = int(parse_qs(urlsplit(route.request.url).query).get('start', ['0'])[0])
            data.update(messages=history[start:], reset=not start, start=start, end=len(history),
                        message_total=len(history), partial=None, activity_changed=True,
                        activity={'state': 'working'})
            route.fulfill(response=response, json=data)
        context.route('**/api/messages/**', messages)
        def inactive(route):
            response = route.fetch()
            data = response.json()
            data.update(uids=[], sdk_uids=[], working_uids=[], tmux_uids=[],
                        sessions=[], sdk_sessions=[], pending=[])
            route.fulfill(response=response, json=data)
        context.route('**/api/term/list*', inactive)
        context.route('**/api/live*', inactive)
        context.route('**/api/watch?*', lambda r: r.fulfill(
            content_type='text/event-stream', body=': static synthetic history\n\n'))
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible():
            page.locator(f'#chips button[data-source="{source}"]').click()
        row.click()
        page.locator('#a-sdk').click()
        page.wait_for_function('SdkView.pane && !SdkView.historyReady && SdkView.historyData')
        for mode in (('sdk',) if source in ('kiro', 'codebuddy') else ('sdk', 'chat')):
            page.locator('#a-' + mode).click()
            box = page.locator('.sdk-msgs' if mode == 'sdk' else '#msgs')
            expect(box).to_contain_text('答复 23')
            name = f'{label}-{source}-{mode}-{width}-{theme}'
            page.screenshot(path=str(screenshots / f'{name}.png'), full_page=True)
            if baseline:
                print(name, box.evaluate('(b)=>({top:b.scrollTop,height:b.scrollHeight,client:b.clientHeight})'), flush=True)
                continue
            def bottom():
                page.wait_for_function('selector => {const b=document.querySelector(selector); return b.scrollHeight>b.clientHeight && b.scrollHeight-b.scrollTop-b.clientHeight<3}', arg='.sdk-msgs' if mode == 'sdk' else '#msgs')
            bottom()
            # Keep dragging the last line while an unchanged poll arrives.
            line = box.locator('.msg[data-role="assistant"] .mb p').last
            points = line.evaluate("""p => {
                const node = p.firstChild, range = document.createRange();
                range.setStart(node, 0); range.setEnd(node, 6);
                const r = range.getClientRects()[0];
                return {x:r.x + 1, y:r.y + r.height/2, end:r.right - 1};
            }""")
            page.mouse.move(points['x'], points['y']); page.mouse.down()
            page.mouse.move(points['end'], points['y'], steps=8)
            selected = page.evaluate('getSelection().toString()')
            assert selected, name
            page.evaluate('window.dragAnchor = getSelection().anchorNode')
            page.evaluate('async () => {await syncSession(S.sel, S.agent)}')
            assert page.evaluate('dragAnchor.isConnected'), f'{name}: poll removed selection anchor'
            assert page.evaluate('getSelection().toString()') == selected, name
            page.mouse.move(points['end'] + 15, points['y'], steps=3)
            page.mouse.up()
            assert len(page.evaluate('getSelection().toString()')) < 40, name
            page.screenshot(path=str(screenshots / f'{name}-selection.png'), full_page=True)
            page.mouse.click(points['x'], points['y'])
            # Actual wheel navigation, followed by the same background sync used by watch.
            box.hover(); page.mouse.wheel(0, -650)
            page.wait_for_function('selector => {const b=document.querySelector(selector); return b.scrollHeight-b.scrollTop-b.clientHeight>300}', arg='.sdk-msgs' if mode == 'sdk' else '#msgs')
            before = box.evaluate('(b)=>b.scrollTop')
            history.append({'role': 'assistant', 'phase': 'final', 'text': f'后台追加 {name}'})
            page.evaluate('async () => {await syncSession(S.sel, S.agent)}')
            expect(box).to_contain_text(f'后台追加 {name}')
            assert abs(box.evaluate('(b)=>b.scrollTop') - before) < 3, name
            page.screenshot(path=str(screenshots / f'{name}-reading.png'), full_page=True)
            page.mouse.wheel(0, 100000)
            bottom()
            history.append({'role': 'assistant', 'phase': 'final', 'text': f'跟随追加 {name}'})
            page.evaluate('async () => {await syncSession(S.sel, S.agent)}')
            expect(box).to_contain_text(f'跟随追加 {name}')
            bottom()
            page.set_viewport_size({'width': max(320, width - 70), 'height': 740})
            bottom()
            page.set_viewport_size({'width': width, 'height': 900})
            page.reload(wait_until='domcontentloaded')
            page.wait_for_function('T.listLoaded')
            page.locator('#a-' + mode).click()
            expect(box).to_contain_text(f'跟随追加 {name}')
            bottom()
            page.screenshot(path=str(screenshots / f'{name}-final.png'), full_page=True)
            print(f'PASS {name}: open, reading/update, follow/update, resize, reload', flush=True)
        assert not errors, errors
        context.unroute_all(behavior="wait")
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='kiro,codex,claude,grok,agy,opencode')
    parser.add_argument('--screenshots', type=Path, required=True)
    parser.add_argument('--baseline', action='store_true')
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-scroll-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(tmp) / source; root.mkdir()
                with provider(source, root, args.binary) as (base, peer, extra):
                    exercise(browser, base, source, args.screenshots, 'node', args.baseline)
                    if args.baseline: continue
                    hub_root = root / 'hub'; hub_root.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                              [SimpleNamespace(name='ScrollNode', token=TOKEN,
                               port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', source, args.screenshots, 'hub')
                    finally: hub.stop()
        finally: browser.close()


if __name__ == '__main__':
    main()
