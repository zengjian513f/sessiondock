#!/usr/bin/env python3
"""Recover an existing SDK turn after detached work and a bridge reconnect.

Chromium, private native records and a fake daemon; no production writes or CLI.
"""
import argparse
import asyncio
import json
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD, build, server
from codex_sdk_controls_browser import ControlsServer
from history_fixtures import BINARY, codex_message, codex_row

TURN = 'existing-long-running-turn'
PROMPT = '继续完成整体任务。'


class RecoveryServer(ControlsServer):
    def __init__(self, socket_path, rollout):
        super().__init__(socket_path, rollout, THREAD)
        self.turn = TURN
        self.recovered = [{'id': 'existing-user', 'type': 'userMessage',
                           'content': [{'type': 'text', 'text': PROMPT}]}]
        for index in range(48):
            self.recovered.append({'id': f'progress-{index}', 'type': 'agentMessage',
                'text': f'后台进度 {index:02d} / persisted while away，继续核对实际结果。',
                'phase': 'commentary'})
        with self.rollout.open('a') as native:
            for item in self.recovered:
                row = codex_message('user' if item['type'] == 'userMessage' else 'assistant',
                                    PROMPT if item['type'] == 'userMessage' else item['text'])
                row['payload']['internal_chat_message_metadata_passthrough'] = {'turn_id': TURN}
                native.write(json.dumps(row, ensure_ascii=False) + '\n')

    async def _handle(self, writer, message):
        method, params = message.get('method'), message.get('params', {})
        if method == 'thread/resume':
            result = {'thread': {'id': THREAD, 'status': {'type': 'active'}, 'turns': []},
                      'model': 'fake-sol', 'reasoningEffort': 'high'}
        elif method == 'thread/turns/list':
            assert params['limit'] == 1 and params['sortDirection'] == 'desc', params
            # Native summary view omits assistant and tool bodies.
            items = list(self.recovered) if params['itemsView'] == 'full' else self.recovered[:1]
            if params['itemsView'] == 'full' and not getattr(self, 'published_during_recovery', False):
                self.published_during_recovery = True
                newer = {**self.recovered[-1], 'text': self.recovered[-1]['text'] + ' 恢复读取期间的新内容。'}
                self.recovered[-1] = newer
                await self.notify('item/completed', {'turnId': TURN, 'item': newer})
            result = {'data': [{'id': TURN, 'status': 'inProgress', 'items': items}], 'nextCursor': None}
        else:
            return await super()._handle(writer, message)
        await self.send(writer, {'id': message['id'], 'result': result})

    def progress(self, text):
        async def publish():
            item = {'id': f'progress-{len(self.recovered)}', 'type': 'agentMessage',
                    'text': text, 'phase': 'commentary'}
            self.recovered.append(item)
            await self.notify('item/completed', {'turnId': TURN, 'item': item})
        asyncio.run_coroutine_threadsafe(publish(), self.loop).result(timeout=5)


def exercise(browser, binary, root, width, theme, screenshots):
    corpus = build(root)
    # Identical prompts in earlier turns must not choose the history boundary.
    with corpus.paths['sdk'].open('a') as native:
        native.write(json.dumps(codex_message('user', PROMPT)) + '\n')
        native.write(json.dumps(codex_message('assistant', '上一次同文提示的答复应保留。')) + '\n')
    with RecoveryServer(root/'app.sock', corpus.paths['sdk']) as fake, server(corpus, binary, root/'app.sock') as base:
        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.goto(base, wait_until='networkidle')
        page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
        expect(page.locator('#sdk-send')).to_have_text('排队')
        expect(page.locator('.sdk-history')).to_contain_text('上一次同文提示的答复应保留。')
        page.screenshot(path=str(screenshots/f'{width}-{theme}-recovered.png'))
        live = page.locator('.sdk-live')
        expect(live.locator('.msg[data-role="assistant"]')).to_have_count(48)
        expect(live.locator('.msg[data-role="assistant"]').last).to_contain_text('恢复读取期间的新内容')
        assert live.locator('.msg[data-role="assistant"]').evaluate_all(
            'nodes => nodes.map(n => n.dataset.sdkItem)') == [f'progress-{index}' for index in range(48)]
        assert page.locator('.sdk-msgs').inner_text().count('后台进度 00 /') == 1
        assert page.locator('.sdk-msgs').inner_text().count('后台进度 47 /') == 1
        status = page.locator('.sdk-pane .sdk-status')
        expect(status).to_have_text('处理中')
        assert status.evaluate('e => e.scrollWidth <= e.clientWidth + 1')
        expect(status).to_be_enabled()
        assert status.evaluate('e => getComputedStyle(e).cursor') == 'pointer'
        bounds = status.bounding_box()
        page.mouse.click(bounds['x'] + bounds['width']/2, bounds['y'] + bounds['height']/2)
        expect(page.locator('#sdk-status-menu')).to_be_visible()
        page.keyboard.press('Escape')
        page.locator('#a-chat').click()
        fake.progress('离开 SDK 页面期间产生的进度，回到页面应完整显示。')
        page.locator('#a-sdk').click()
        expect(live).to_contain_text('离开 SDK 页面期间产生的进度')
        expect(live.locator('.msg[data-role="assistant"]')).to_have_count(49)
        page.locator('#sdk-input').fill('保留未发送草稿 / draft')
        resumes = len(fake.methods('thread/resume'))
        fake.disconnect()
        fake.wait_for(lambda: len(fake.methods('thread/resume')) > resumes)
        expect(live).to_contain_text('离开 SDK 页面期间产生的进度', timeout=20000)
        page.wait_for_function('CodexSdkView.connected && CodexSdkView.es?.readyState === 1')
        expect(page.locator('#sdk-input')).to_have_value('保留未发送草稿 / draft')
        expect(live.locator('.msg[data-role="assistant"]')).to_have_count(49)
        # Full native turn identity still bounds history if compaction omitted
        # the user item from the recovered turn.
        fake.recovered = fake.recovered[1:]
        fake.disconnect()
        page.reload(wait_until='networkidle')
        expect(live.locator('.msg[data-role="assistant"]')).to_have_count(49)
        assert page.locator('.sdk-msgs').inner_text().count('后台进度 00 /') == 1
        assert not fake.methods('turn/start') and not fake.methods('turn/interrupt')
        assert all(m['params']['itemsView'] == 'full' for m in fake.methods('thread/turns/list'))
        page.screenshot(path=str(screenshots/f'{width}-{theme}-reconnected.png'))
        assert not errors, errors
        context.close()
    print(f'PASS recovery {width}/{theme}: cold attach, detached progress, reconnect, compacted prompt, passive status', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for width, theme in [(1440, 'light'), (390, 'light'), (1440, 'dark'), (390, 'dark')]:
                with tempfile.TemporaryDirectory(prefix='sd-sdk-history-') as tmp:
                    exercise(browser, args.binary, Path(tmp), width, theme, args.screenshots)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
