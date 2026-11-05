#!/usr/bin/env python3
"""Seven-source message dates/raw copy through SDK and conversation on node/Hub."""
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
from sdk_selection_fixtures import snapshots
from sdk_ui_fixtures import start_sdk

STAMP = '2026-11-05T07:57:49Z'
TEXT = ('检查 [报告](https://example.test/report?x=1&y=2#details) 与 '
        '[源码](/synthetic/project/main.rs:42)。\n\n'
        '```sh\nprintf "保留代码与链接"\n```\n\n'
        + '\n\n'.join(f'第 {i} 项：保留完整原文、换行及 Markdown 链接地址。' for i in range(18)))
ANSWER = '已检查 [最终报告](https://example.test/result?q=中文#result)，请按原始地址打开。'


def exercise(browser, base, source, events, screenshots, mode):
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme,
            timezone_id='Asia/Shanghai', permissions=['clipboard-read', 'clipboard-write'], service_workers='block')
        page = context.new_page(); errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        history = [dict(role='user', text=TEXT, ts=STAMP, turn_id='header-turn', id='header-user',
                        native_item_id='header-user', native_id='header-user', message_id='header-user', prompt_index=0, native={'step_index': 0}),
                   dict(role='assistant', text=ANSWER, ts=STAMP, phase='final', turn_id='header-turn',
                        id='header-answer', native_item_id='header-answer', native_id='header-answer'),
                   dict(role='user', text='没有原始时间的消息，不能伪造发送时间。', ts='invalid')]
        if source == 'codex':
            # Older real rollouts lack item IDs: retain dates through an exact,
            # unambiguous match within the native turn, never across turns.
            for row in history:
                for key in ('id', 'native_id', 'native_item_id', 'message_id', 'turn_id'): row.pop(key, None)
        def messages(route):
            response = route.fetch(); data = response.json()
            current = parse_qs(urlsplit(route.request.url).query).get("start", [""])[0] == str(len(history))
            data.update(messages=[] if current else history, reset=not current, start=len(history) if current else 0, end=len(history),
                        message_total=len(history), partial=None, activity={'state': 'idle'})
            route.fulfill(response=response, json=data)
        context.route('**/api/messages/**', messages)
        context.route('**/api/watch?*', lambda r: r.fulfill(content_type='text/event-stream', body=': fixed history\n\n'))
        api = 'codex-sdk' if source == 'codex' else 'native-sdk'
        raw = {'items': [], 'requests': [], 'running': False, 'input_queue': [], 'capabilities': {},
               'summary': {'settings': {}, 'queue': [], 'status': {'type': 'idle'}}}
        events.packet = {'type': 'snapshot', 'seq': 0, 'raw': raw}
        context.route(f'**/api/{api}/open', lambda r: r.fulfill(json={'ok': True, 'raw': {}, 'capabilities': {}}))
        context.route(f'**/api/{api}/events?*', lambda r: r.continue_(url=events.url))
        for endpoint in ('models', 'limits', 'usage', 'agents'):
            context.route(f'**/api/{api}/{endpoint}*', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible(): page.locator(f'#chips button[data-source="{source}"]').click()
        row.click()
        page.locator('#a-sdk').click()
        expect(page.locator('.sdk-history')).to_contain_text(ANSWER.split('[')[0])
        name = f'{mode}-{source}-{width}-{theme}'
        def check(scope, entry):
            user = scope.locator('.msg[data-role="user"]').filter(has_text='检查 报告').first
            user.get_by_role('button', name='复制消息原文').hover()
            header = user.locator('.message-header'); bubble = user.locator('.message-bubble')
            assert not bubble.locator('.message-header').count()
            assert header.bounding_box()['y'] + header.bounding_box()['height'] <= bubble.bounding_box()['y']
            expect(header.get_by_role('button').first.locator('svg')).to_be_visible()
            expect(user.locator('time')).to_have_text('2026-11-05 15:57:49')
            expect(user.locator('time')).to_have_attribute('datetime', '2026-11-05T07:57:49.000Z')
            user.get_by_role('button', name='复制消息原文').click()
            assert page.evaluate('navigator.clipboard.readText()') == TEXT
            user.get_by_role('button', name='展开全文', exact=False).click()
            user.get_by_role('button', name='复制消息原文').hover()
            user.get_by_role('button', name='复制消息原文').press('Enter')
            assert page.evaluate('navigator.clipboard.readText()') == TEXT
            user.get_by_role('button', name='收起', exact=True).click()
            answer = scope.locator('.msg[data-role="assistant"]').filter(has_text='已检查').first
            answer.get_by_role('button', name='复制消息原文').click()
            assert page.evaluate('navigator.clipboard.readText()') == ANSWER
            missing = scope.locator('.msg[data-role="user"]').filter(has_text='没有原始时间')
            expect(missing.locator('time')).to_be_hidden()
            expect(missing).not_to_contain_text('时间未提供')
            expect(missing.locator('time')).not_to_have_attribute('datetime', '')
            if screenshots: page.screenshot(path=str(screenshots / f'{name}-{entry}.png'), full_page=True)
        check(page.locator('.sdk-history'), 'sdk')
        chat = page.locator('#a-chat')
        if source not in ('kiro', 'codebuddy'):
            chat.click()
            check(page.locator('#detail'), 'conversation')
            page.locator('#a-sdk').click()
        start_sdk(page)
        # The actual regression: dated disk history is replaced a few seconds
        # later by the native replay, whose items contain no timestamp.
        user = {key: value for key, value in history[0].items() if key != 'ts'}
        answer = {key: value for key, value in history[1].items() if key != 'ts'}
        if source == 'codex':
            user = {'id': 'header-user', 'type': 'userMessage', 'content': [{'type': 'text', 'text': TEXT}]}
            answer = {'id': 'header-answer', 'type': 'agentMessage', 'text': ANSWER}
        raw.update(items=[user, answer], running=True, turn={'id': 'header-turn', 'status': 'inProgress'})
        events.publish()
        replay = page.locator('.sdk-live .msg').filter(has_text='已检查').first
        expect(replay.locator('time')).to_have_attribute('datetime', '2026-11-05T07:57:49.000Z')
        # Cross the regular history polling interval, then replay another bare snapshot.
        page.wait_for_timeout(3500)
        events.publish()
        expect(replay.locator('time')).to_have_attribute('datetime', '2026-11-05T07:57:49.000Z')
        if screenshots: page.screenshot(path=str(screenshots / f'{name}-replayed.png'), full_page=True)
        live_text = '实时回复 [原始链接](https://example.test/live?a=1#stream)'
        item = {'id': 'header-live', 'text': live_text,
                **({'type': 'agentMessage'} if source == 'codex' else {'role': 'assistant'})}
        raw.update(items=[item], running=True, turn={'id': 'header-live-turn', 'status': 'inProgress'})
        events.publish()
        live = page.locator('.sdk-live .msg[data-role="assistant"]').filter(has_text='实时回复')
        expect(live.locator('time')).to_be_hidden()
        item['received_at'] = '2026-11-05T07:57:52Z'
        events.publish()
        expect(live.locator('time')).to_have_text('2026-11-05 15:57:52')
        expect(live.locator('time')).to_have_attribute('title', '后台接收时间 · 本地时间 · 2026-11-05T07:57:52Z')
        # Real SDK items can arrive before their timestamped native record.
        # The history poll alone must fill in the date without another SDK event.
        history.append(dict(role='assistant', text=live_text, ts=STAMP, turn_id='header-live-turn',
                            id='header-live', native_item_id='header-live', native_id='header-live'))
        expect(live.locator('time')).to_have_text('2026-11-05 15:57:49', timeout=15000)
        expect(live.locator('time')).to_have_attribute('title', '原始时间 · 本地时间 · 2026-11-05T07:57:49Z')
        live.get_by_role('button', name='复制消息原文').click()
        assert page.evaluate('navigator.clipboard.readText()') == live_text
        item['text'] += '\n\n后续增量必须完整复制。'
        events.publish()
        expect(live).to_contain_text('后续增量')
        expect(live.locator('time')).to_have_attribute('datetime', '2026-11-05T07:57:49.000Z')
        live.get_by_role('button', name='复制消息原文').click()
        assert page.evaluate('navigator.clipboard.readText()') == item['text']
        # Clipboard API rejection falls back to the browser's native copy command.
        page.evaluate("() => { navigator.clipboard.writeText = async () => { throw new Error('denied'); }; }")
        live.get_by_role('button', name='复制消息原文').click()
        assert page.evaluate('navigator.clipboard.readText()') == item['text']
        page.evaluate("window.originalCopyCommand = document.execCommand.bind(document); document.execCommand = () => false")
        live.get_by_role('button', name='复制消息原文').click()
        expect(live.get_by_role('button', name='复制消息原文')).to_have_attribute('data-copy-state', 'failed')
        page.evaluate('() => { document.execCommand = window.originalCopyCommand; }')
        live.get_by_role('button', name='复制消息原文').click()
        expect(live.get_by_role('button', name='复制消息原文')).to_have_attribute('data-copy-state', 'copied')
        if screenshots: page.screenshot(path=str(screenshots / f'{name}-live.png'), full_page=True)
        if source == 'codex' and mode == 'node' and width == 390:
            settings = page.locator('#a-global-settings')
            if not settings.is_visible(): page.locator('#a-more').click()
            settings.click()
            page.get_by_role('tab', name='外观', exact=True).click()
            scale = page.locator('#setting-scale')
            scale.focus(); scale.press('End')
            expect(scale).to_have_value('150')
            page.locator('#settings-dialog').get_by_role('button', name='关闭', exact=True).click()
            user = page.locator('.sdk-history .msg[data-role="user"]').first
            user.get_by_role('button', name='复制消息原文').click()
            assert page.evaluate('navigator.clipboard.readText()') == TEXT
            expect(page.locator('#scale-indicator')).to_have_css('opacity', '0')
            user.get_by_role('button', name='复制消息原文').hover()
            if screenshots: page.screenshot(path=str(screenshots / f'{name}-zoom150.png'), full_page=True)

        assert not errors, errors
        context.unroute_all(behavior='wait'); context.close()
        print('PASS', name, 'history/conversation/live, dates, full raw copy, clipboard fallback/retry', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='codex,claude,grok,agy,opencode,kiro,codebuddy')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-message-headers-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(tmp) / source; root.mkdir()
                with provider(source, root, args.binary) as (base, _, extra), snapshots() as events:
                    exercise(browser, base, source, events, args.screenshots, 'node')
                    hub_root = root / 'hub'; hub_root.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                        [SimpleNamespace(name='HeadersNode', token=TOKEN,
                            port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try: exercise(browser, f'http://127.0.0.1:{hub.port}', source, events, args.screenshots, 'hub')
                    finally: hub.stop()
        finally: browser.close()


if __name__ == '__main__': main()
