#!/usr/bin/env python3
"""Five SDKs and conversation footers expose only useful status details in Chromium."""
import argparse
import asyncio
import html
import json
from pathlib import Path
import re
import tempfile

from sdk_ui_fixtures import start_sdk
from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build, server
from codex_sdk_controls_browser import ControlsServer
from history_fixtures import BINARY
from native_sdk_footer_browser import native_events, provider


def check_status(page, status, menu, label, detail=None):
    expect(status).to_have_text(label)
    expect(status).to_be_visible()
    if detail is None:
        expect(status).to_be_disabled()
        for attribute in ('aria-haspopup', 'aria-controls', 'aria-expanded'):
            expect(status).not_to_have_attribute(attribute, re.compile('.+'))
        # Conversation polls replace the label; read its current rect atomically.
        box = status.evaluate('e => { const r = e.getBoundingClientRect(); return {x:r.x, y:r.y, width:r.width, height:r.height}; }')
        page.mouse.click(box['x'] + box['width'] / 2, box['y'] + box['height'] / 2)
        expect(menu).not_to_be_visible()
        assert status.evaluate('e => getComputedStyle(e).cursor') == 'default'
    else:
        expect(status).to_be_enabled()
        status.click()
        expect(menu).to_be_visible()
        expect(menu).to_contain_text(detail)
        bounds = menu.bounding_box()
        assert bounds['x'] >= 0 and bounds['x'] + bounds['width'] <= page.viewport_size['width'] + 1, bounds
        assert menu.evaluate('e => e.scrollWidth <= e.clientWidth + 1')
        # A short retry explanation must fit its text instead of occupying 300px.
        if detail == '（2）':
            assert bounds['width'] < 180, bounds
        page.keyboard.press('Escape')
        status.press('Enter')
        expect(menu).to_be_visible()


def select_source(page, base, source):
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    row = page.locator(f'#side .item[data-uid^="{source}:"]').first
    back = page.locator('#detail .mobile-back')
    if not row.is_visible() and back.is_visible():
        back.click()
    if not row.is_visible():
        page.locator(f'#chips button[data-source="{source}"]').click()
    row.click()
    return row.get_attribute('data-uid')


def native(browser, base, source, events, shots):
    for width, theme in ((1912, 'light'), (390, 'dark'), (320, 'light'), (1440, 'dark')):
        print(f'Checking {source} {width}/{theme}', flush=True)
        context = browser.new_context(viewport={'width': width, 'height': 948}, color_scheme=theme,
                                      service_workers='block', permissions=['local-network-access'])
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda e: errors.append(str(e)))
        context.route('**/api/native-sdk/open', lambda r: r.fulfill(json={'ok': True, 'capabilities': {}}))
        context.route('**/api/native-sdk/events?*', lambda r: r.continue_(url=events.url))
        states = [
            ({}, '就绪', None),
            ({'running': True}, '处理中', None),
            ({'requests': [{'key': 'approval', 'type': 'approval', 'title': 'Approve operation'}]}, re.compile('^(待批准|等待回答)$'), None),
            ({'requests': [{'key': 'question', 'type': 'question', 'title': 'Choose option'}]}, re.compile('^(待回答|等待回答)$'), None),
            ({'status': 'compacting'}, '压缩中', None),
            ({'retry': {}}, re.compile('^(正在重试|请求重试中)$'), None),
            ({'retry': {'attempt': 2}}, '正在重试', '（2）'),
            ({'input_queue': [{'id': 'queued', 'state': 'queued'}]}, '等待发送', None),
            ({'input_queue': [{'id': 'paused', 'state': 'failed'}]}, '队列暂停', '处理失败或待核对记录'),
            ({'error': '模型暂不可用。Capacity unavailable for the selected model. ' * 3}, '错误', 'Capacity unavailable'),
        ]
        for seq, (raw, label, detail) in enumerate(states, 1):
            events.packet = {'type': 'snapshot', 'seq': seq, 'raw': {'running': False, **raw}}
            uid = select_source(page, base, source)
            if not page.locator('#sdk-input').is_visible():
                page.locator('#a-sdk').click()
            start_sdk(page)
            status = page.locator('.native-sdk-pane .sdk-status')
            check_status(page, status, page.locator('#sdk-status-menu'), label, detail)
            if label in ('处理中', '错误') or detail == '（2）':
                page.screenshot(path=str(shots / f'{source}-sdk-{width}-{theme}-{seq}.png'))
            page.keyboard.press('Escape')
        # The ordinary conversation view uses the same disclosure rule for every source.
        current = {'value': 'working'}
        def document(route):
            response = route.fetch()
            body = response.text()
            match = re.search(r'(<meta name="sessiondock-capabilities" content=")(.*?)(">)', body)
            config = json.loads(html.unescape(match[2]))
            config.update(conversation_send=True, terminal=True)
            body = body[:match.start(2)] + html.escape(json.dumps(config), quote=True) + body[match.end(2):]
            route.fulfill(response=response, body=body)
        context.route(re.compile(re.escape(base) + r'/(?:\?.*)?$'), document)
        context.route('**/api/term/list*', lambda r: r.fulfill(json={'enabled': True,
            'sources': {source: True}, 'pending': [], 'sessions': [{'name': 'status-fixture',
            'uid': uid, 'sid': 'status-fixture', 'source': source, 'instance_id': '1' * 32}]}))
        context.route('**/api/session/conversation?*', lambda r: r.fulfill(json={'draft': {'revision': 0, 'value': None}}))
        context.route('**/api/session/conversation/check', lambda r: r.fulfill(json={'ok': True,
            'input': {'state': 'ready', 'code': '', 'message': ''}, 'prompt': None}))
        context.route('**/api/live*', lambda r: r.fulfill(json={'known': True,
            'uids': [] if current['value'] == 'stopped' else [uid], 'working_uids': [], 'tmux_uids': []}))
        def history(route):
            response = route.fetch()
            data = response.json()
            value = current['value']
            data['activity'] = {'state': 'working' if value == 'compacting' else value,
                'error': 'Capacity unavailable 详细错误信息' if value == 'failed' else None}
            data['cli'] = {'instance': {'phase': 'compacting', 'running': True, 'busy': True}} if value == 'compacting' else None
            route.fulfill(response=response, json=data)
        context.route('**/api/messages/**', history)
        for value, label in [('working', '处理中'), ('waiting', '待回答'), ('aborted', '已中断'),
                             ('failed', '执行失败'), ('idle', '就绪'), ('stopped', '未运行')]:
            current['value'] = value
            print(f'  {source} conversation {value}', flush=True)
            select_source(page, base, source)
            page.locator('#a-chat').click()
            footer = page.locator('#conversation-status')
            expect(footer).to_be_visible()
            check_status(page, footer.locator('.sdk-status'), page.locator('#conversation-status-menu'),
                         label, 'Capacity unavailable' if value == 'failed' else None)
            if value in ('working', 'failed'):
                page.screenshot(path=str(shots / f'{source}-conversation-{width}-{theme}-{value}.png'))
            page.keyboard.press('Escape')
        assert not errors, errors
        context.close()
    print(f'PASS {source}: SDK and conversation states, mouse/keyboard, widths/themes', flush=True)


def codex(browser, base, corpus, fake, shots):
    def notify(method, params):
        asyncio.run_coroutine_threadsafe(fake.notify(method, params), fake.loop).result(timeout=5)
    for width, theme in ((1912, 'light'), (390, 'dark'), (320, 'light'), (1440, 'dark')):
        context = browser.new_context(viewport={'width': width, 'height': 948}, color_scheme=theme,
                                      service_workers='block')
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda e: errors.append(str(e)))
        select_source(page, base, 'codex')
        if not page.locator('#sdk-input').is_visible():
            page.locator('#a-sdk').click()
        start_sdk(page)
        expect(page.locator('#sdk-model')).to_have_value('fake-sol')
        status, menu = page.locator('.sdk-pane .sdk-status'), page.locator('#sdk-status-menu')
        notify('turn/completed', {'turn': {'id': 'status', 'status': 'completed', 'items': []}})
        notify('thread/status/changed', {'status': {'type': 'idle'}})
        check_status(page, status, menu, '就绪')
        notify('turn/started', {'turn': {'id': 'status', 'status': 'inProgress', 'items': []}})
        for flags, label in [([], '处理中'), (['waitingOnApproval'], '待批准'), (['waitingOnUserInput'], '待回答')]:
            notify('thread/status/changed', {'status': {'type': 'active', 'activeFlags': flags}})
            check_status(page, status, menu, label)
        notify('thread/status/changed', {'status': {'type': 'active', 'activeFlags': []}})
        notify('item/started', {'turnId': 'status', 'item': {'id': 'compact', 'type': 'contextCompaction'}})
        check_status(page, status, menu, '压缩中')
        notify('item/completed', {'turnId': 'status', 'item': {'id': 'compact', 'type': 'contextCompaction'}})
        notify('turn/completed', {'turn': {'id': 'status', 'status': 'interrupted', 'items': []}})
        notify('thread/status/changed', {'status': {'type': 'idle'}})
        check_status(page, status, menu, '已中断')
        page.locator('#sdk-input').fill('/unsupported-status-fixture')
        page.locator('#sdk-send').click()
        check_status(page, status, menu, '不支持的命令', '草稿已保留')
        page.screenshot(path=str(shots / f'codex-error-{width}-{theme}.png'))
        # Open error details must disappear on a live transition, without a reload.
        notify('turn/started', {'turn': {'id': 'next-status', 'status': 'inProgress', 'items': []}})
        notify('thread/status/changed', {'status': {'type': 'active', 'activeFlags': []}})
        check_status(page, status, menu, '处理中')
        page.screenshot(path=str(shots / f'codex-working-{width}-{theme}.png'))
        assert not errors, errors
        context.close()
    print('PASS codex: states, error details, live dismissal, widths/themes', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-status-disclosure-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        browser = pw.chromium.launch(headless=True)
        try:
            codex_root = root / 'codex'
            codex_root.mkdir()
            corpus = build(codex_root)
            with ControlsServer(codex_root / 'app.sock', corpus.paths['sdk'], THREAD) as fake, \
                    server(corpus, args.binary, codex_root / 'app.sock') as base:
                codex(browser, base, corpus, fake, args.screenshots)
            for source in ('claude', 'grok', 'opencode', 'agy'):
                source_root = root / source
                source_root.mkdir()
                with provider(source, source_root, args.binary) as base, native_events() as events:
                    native(browser, base, source, events, args.screenshots)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
