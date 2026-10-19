#!/usr/bin/env python3
"""Native collab events, MCP JSON replies and sparse account updates in Chromium.

Private fake app-server, temporary histories and loopback service only.
"""
from __future__ import annotations

import argparse
import asyncio
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build, server
from codex_sdk_controls_browser import ControlsServer
from history_fixtures import BINARY

CHILD = '019a1da0-2244-7333-8444-556677889900'
PROMPT = '检查子代理返回值 / inspect protocol event from /synthetic/sdk/long-project-name'


class ProtocolServer(ControlsServer):
    single_window = False

    def _bucket(self):
        bucket = super()._bucket()
        bucket.update(limitName='Codex account', normalModelSlug='fake-sol')
        if self.single_window:
            bucket['secondary'] = None
        return bucket

    async def _script(self, turn_id, text, content=None):
        await self.notify('thread/status/changed', {'status': {'type': 'active', 'activeFlags': []}})
        await self.notify('turn/started', {'turn': {'id': turn_id, 'status': 'inProgress', 'items': []}})
        await self._user_item(turn_id, text, content=content)
        await self.notify('item/started', {'turnId': turn_id, 'item': self.collab('inProgress')})

        async def answered(result):
            await self.notify('serverRequest/resolved', {'requestId': self.request_seq - 1})
            await self._agent(turn_id, turn_id + '-answer', 'MCP accepted supplied JSON', 'commentary')

        await self._request('mcpServer/elicitation/request', {
            'turnId': turn_id, 'serverName': 'Synthetic MCP', 'message': '请填写审批说明 / Enter required name',
            'mode': 'form', 'requestedSchema': {'type': 'object', 'properties': {'name': {'type': 'string'}},
                                              'required': ['name']}}, answered)

    def collab(self, status):
        return {'id': 'native-collab', 'type': 'collabAgentToolCall', 'tool': 'spawnAgent',
                'status': status, 'senderThreadId': THREAD, 'receiverThreadIds': [CHILD],
                'prompt': PROMPT, 'model': 'fake-luna', 'reasoningEffort': 'low',
                'agentsStates': {CHILD: {'status': 'running' if status == 'inProgress' else 'completed',
                                        'message': None if status == 'inProgress' else '子代理完成 / reviewed native event'}}}

    def emit(self, method, raw):
        asyncio.run_coroutine_threadsafe(self.notify(method, raw), self.loop).result(timeout=5)


def exercise(browser, base, corpus, fake, width, theme, screenshots):
    print(f'{width}-{theme}: collab tool / invalid MCP JSON / sparse limits', flush=True)
    fake.single_window = False
    context = browser.new_context(viewport={'width': width, 'height': 1000}, color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(10000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='domcontentloaded')
    page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
    expect(page.locator('#sdk-input')).to_be_visible()
    expect(page.locator('.sdk-credits')).to_have_text('🪙 1,235')
    page.locator('#sdk-input').fill(PROMPT)
    page.locator('#sdk-send').click()
    tool = page.locator('[data-sdk-item="native-collab"]')
    expect(tool).to_be_visible()
    expect(tool).to_contain_text('spawnAgent')
    expect(tool.locator('.tool-status')).to_have_text('运行中…')
    expect(tool).not_to_contain_text('collabAgentToolCall')
    tool.get_by_role('button', name='展开原始参数', exact=True).click()
    expect(tool.locator('.tool-args')).to_contain_text(CHILD)
    fake.emit('item/completed', {'turnId': fake.turn, 'item': fake.collab('completed')})
    expect(tool.locator('.tool-status')).to_contain_text('✓ 完成')
    expect(tool.locator('.tool-args')).to_be_visible()
    expect(tool.locator('.tool-out')).to_contain_text('子代理完成 / reviewed native event')
    expect(tool.locator('.tool-out')).to_contain_text(CHILD)
    expect(tool.locator('.tool-out.err')).to_have_count(0)
    tool.get_by_role('button', name='收起原始参数', exact=True).click()

    card = page.locator('.sdk-request').filter(has_text='Synthetic MCP')
    editor = card.locator('textarea')
    invalid = '{invalid JSON 中文输入仍应保留'
    editor.fill(invalid)
    responses = len(fake.responses())
    card.get_by_role('button', name='提交', exact=True).click()
    expect(card.get_by_role('alert')).to_contain_text('JSON 格式无效')
    expect(editor).to_have_value(invalid)
    expect(editor).to_have_attribute('aria-invalid', 'true')
    expect(card.get_by_role('button', name='提交', exact=True)).to_be_enabled()
    assert len(fake.responses()) == responses, 'invalid JSON must not produce a JSON-RPC reply'
    if screenshots:
        card.get_by_role('button', name='提交', exact=True).scroll_into_view_if_needed()
        page.screenshot(path=str(screenshots / f'{width}-{theme}-invalid-json.png'))
    editor.fill('')
    card.get_by_role('button', name='提交', exact=True).click()
    expect(card.get_by_role('alert')).to_contain_text('JSON 格式无效')
    expect(editor).to_have_value('')
    assert len(fake.responses()) == responses, 'empty input must not be accepted as an empty object'
    editor.fill('{"name":"已审阅 / human supplied content"}')
    card.get_by_role('button', name='提交', exact=True).click()
    fake.wait_for(lambda: len(fake.responses()) > responses)
    assert fake.responses()[-1]['result'] == {'action': 'accept', 'content': {'name': '已审阅 / human supplied content'}}
    expect(card).to_have_count(0)
    expect(page.locator('.sdk-live')).to_contain_text('MCP accepted supplied JSON')

    # Native rolling updates may contain null metadata and windows. Preserve
    # the complete snapshot while accepting false, zero and fresh window data.
    fake.emit('account/rateLimits/updated', {'rateLimits': {
        'limitId': 'codex', 'limitName': None, 'planType': None, 'normalModelSlug': None,
        'credits': None, 'primary': None,
        'secondary': {'usedPercent': 26, 'windowDurationMins': None, 'resetsAt': None}}})
    expect(page.locator('[data-gauge="7d"] b')).to_have_text('26%')
    expect(page.locator('[data-gauge="5h"] b')).to_have_text('40%')
    expect(page.locator('.sdk-credits')).to_have_text('🪙 1,235')
    assert page.evaluate('CodexSdkView.limits.limitName') == 'Codex account'
    assert page.evaluate('CodexSdkView.limits.planType') == 'pro'
    assert page.evaluate('CodexSdkView.limits.normalModelSlug') == 'fake-sol'
    fake.emit('account/rateLimits/updated', {'rateLimits': {'limitId': 'another-bucket',
                                                         'primary': {'usedPercent': 99}}})
    expect(page.locator('[data-gauge="5h"] b')).to_have_text('40%')
    fake.emit('account/rateLimits/updated', {'rateLimits': {'limitId': 'codex',
        'credits': {'hasCredits': True, 'unlimited': False, 'balance': '0'},
        'primary': {'usedPercent': 0}, 'secondary': None}})
    expect(page.locator('[data-gauge="5h"] b')).to_have_text('0%')
    expect(page.locator('[data-gauge="7d"] b')).to_have_text('26%')
    expect(page.locator('.sdk-credits')).to_have_text('🪙 0')
    usage = page.locator('.sdk-gauges')
    if not usage.is_visible():
        toggle = page.locator('#sdk-usage-toggle')
        (toggle if toggle.is_visible() else page.locator('#sdk-more')).click()
    expect(usage).to_be_visible()
    expect(page.locator('[data-gauge="5h"]')).to_be_visible()
    expect(page.locator('[data-gauge="7d"]')).to_be_visible()
    if screenshots:
        page.screenshot(path=str(screenshots / f'{width}-{theme}-sparse-limits.png'))
    page.keyboard.press('Escape')

    failed = {**fake.collab('failed'), 'id': 'failed-collab', 'tool': 'wait',
              'agentsStates': {CHILD: {'status': 'errored', 'message': '子代理执行失败'}}}
    fake.emit('item/completed', {'turnId': fake.turn, 'item': failed})
    expect(page.locator('[data-sdk-item="failed-collab"] .tool-out.err')).to_contain_text('子代理执行失败')
    fake.emit('item/completed', {'turnId': fake.turn, 'item': {**failed, 'id': 'interrupted-collab', 'status': 'interrupted'}})
    expect(page.locator('[data-sdk-item="interrupted-collab"] .tool-out.err')).to_contain_text('errored')
    fake.single_window = True
    page.locator('#sdk-stop').click()
    expect(page.locator('#sdk-stop')).to_be_hidden()
    # An authoritative complete read can remove the absent secondary window.
    expect(page.locator('[data-gauge="7d"]')).to_be_hidden()
    expect(page.locator('.sdk-credits')).to_have_text('🪙 1,235')
    assert not errors, errors
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-protocol-') as tmp, sync_playwright() as playwright:
        root = Path(tmp)
        corpus = build(root)
        with ProtocolServer(root / 'app.sock', corpus.paths['sdk'], THREAD) as fake, server(corpus, args.binary, root / 'app.sock') as base:
            browser = playwright.chromium.launch(headless=True)
            try:
                for width, theme in ((1440, 'light'), (390, 'dark'), (1440, 'dark'), (390, 'light')):
                    exercise(browser, base, corpus, fake, width, theme, args.screenshots)
            finally:
                browser.close()
    print('PASS SDK protocol: native collab state/results, MCP JSON validation and sparse limits in desktop/mobile light/dark')


if __name__ == '__main__':
    main()
