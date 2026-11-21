#!/usr/bin/env python3
"""Type-based bubbles and disclosure interaction during SDK streaming."""
import argparse
import asyncio
import json
from pathlib import Path
import re
import tempfile
from sdk_ui_fixtures import start_sdk
from playwright.sync_api import sync_playwright, expect
from codex_sdk_browser import build, server, THREAD
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, codex_message

TEXT = '\n\n'.join(f'段落 {i}：原生补全需要保留路径、参数与光标位置。' for i in range(24)) + '\n\n完整结尾 END'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-bubbles-') as tmp, sync_playwright() as p:
        root = Path(tmp)
        corpus = build(root)
        with corpus.paths['sdk'].open('a') as f:
            f.write(json.dumps(codex_message('user', TEXT)) + '\n')
            f.write(json.dumps(codex_message('assistant', TEXT)) + '\n')
        state = root / 'state'
        state.mkdir()
        with FakeAppServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake, server(
                corpus, args.binary, root/'app.sock', {'SESSIONDOCK_STATE_DIR': str(state)}) as base:
            browser = p.chromium.launch(headless=True)
            for theme in ('light', 'dark'):
                context = browser.new_context(viewport={'width': 1440, 'height': 900}, color_scheme=theme)
                page = context.new_page()
                def screenshot(name):
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / f'{theme}-{name}.png'))
                page.goto(base, wait_until='networkidle')
                page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
                for width in (3406, 390):
                    page.set_viewport_size({'width': width, 'height': 900})
                    if not page.locator('#sdk-input').is_visible():
                        page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
                    answer = page.locator('.sdk-history .msg[data-role="assistant"]').last
                    expect(answer.locator('.mb.clip')).to_have_count(0)
                    assert answer.locator('.mb').evaluate('(b) => b.scrollHeight <= b.clientHeight + 1')
                    bubble = page.locator('.sdk-history .msg[data-role="user"]').last
                    expect(bubble).to_contain_text('完整结尾 END')
                    screenshot(f'{width}-before')
                    expand = bubble.get_by_text('展开全文', exact=False)
                    expect(expand).to_be_visible()
                    expand.click()
                    assert bubble.locator('.mb').evaluate('(b) => b.scrollHeight <= b.clientHeight + 1')
                    bubble.get_by_text('收起', exact=True).click()
                    expect(expand).to_be_visible()
                    screenshot(f'{width}-collapsed')
                    expand.click()
                    bubble.get_by_text('收起', exact=True).scroll_into_view_if_needed()
                    screenshot(f'{width}-expanded')
                    bubble.get_by_text('收起', exact=True).click()
                width = 1440 if theme == 'light' else 390
                page.set_viewport_size({'width': width, 'height': 900})
                start_sdk(page)
                page.locator('#sdk-input').fill('stream a long reply')
                page.locator('#sdk-send').click()
                expect(page.locator('.sdk-live')).to_contain_text('stream a long reply')
                turn_id = fake.turn
                def notify(method, raw):
                    asyncio.run_coroutine_threadsafe(fake.notify(method, {'threadId': THREAD, 'turnId': turn_id, **raw}), fake.loop).result(timeout=5)
                status = page.locator('.sdk-pane .sdk-status [role=status]')
                menu = page.locator('#sdk-status-menu')
                metric = page.locator('.sdk-turn-metric')
                notify('item/started', {'item': {'id': 'raw-thought', 'type': 'reasoning', 'summary': []}})
                notify('item/reasoning/textDelta', {'itemId': 'raw-thought', 'delta': '不可见推理'})
                expect(status).to_have_text('思考中')
                expect(page.locator('[data-sdk-item="raw-thought"]')).to_have_count(0)
                status.click()
                expect(menu).to_be_visible()
                expect(menu).to_contain_text('暂无可见摘要')
                expect(menu).to_contain_text('本轮已用时')
                page.keyboard.press('Escape')
                notify('item/completed', {'item': {'id': 'raw-thought', 'type': 'reasoning', 'summary': []}})
                expect(status).to_have_text('处理中')

                tool_item = {'id': 'progress-tool', 'type': 'dynamicToolCall', 'tool': 'test_lookup', 'status': 'inProgress'}
                notify('item/started', {'item': tool_item})
                expect(status).to_have_text('执行工具')
                status.click()
                expect(menu).to_contain_text('正在运行 1 项工具 · test_lookup')
                page.keyboard.press('Escape')
                notify('thread/status/changed', {'status': {'type': 'active', 'activeFlags': ['waitingOnApproval']}})
                expect(status).to_have_text('待批准')
                expect(metric).to_be_hidden()
                notify('thread/status/changed', {'status': {'type': 'active', 'activeFlags': []}})
                expect(status).to_have_text('执行工具')
                notify('item/completed', {'item': {**tool_item, 'status': 'completed'}})
                expect(status).to_have_text('处理中')

                notify('item/started', {'item': {'id': 'progress-reply', 'type': 'agentMessage', 'phase': 'final_answer', 'text': ''}})
                notify('item/agentMessage/delta', {'itemId': 'progress-reply', 'delta': '你好🌱'})
                expect(status).to_have_text('生成回复')
                expect(metric).to_have_text('· 3 字符')
                if theme == 'light':
                    expect(metric).to_be_visible()
                else:
                    expect(metric).to_be_hidden()
                status.click()
                expect(menu).to_contain_text('本轮已收到 3 字符回复')
                expect(menu).to_contain_text(re.compile(r'本轮已用时 .*秒'))
                screenshot('reply-progress')
                page.reload(wait_until='domcontentloaded')
                expect(status).to_have_text('生成回复')
                expect(metric).to_have_text('· 3 字符')
                notify('item/completed', {'item': {'id': 'progress-reply', 'type': 'agentMessage', 'phase': 'final_answer', 'text': '你好🌱'}})
                expect(status).to_have_text('处理中')
                expect(metric).to_be_hidden()

                notify('item/started', {'item': {'id': 'long-reply', 'type': 'agentMessage', 'text': TEXT}})
                live = page.locator('[data-sdk-item="long-reply"]')
                expect(live.locator('.mb')).to_contain_text('完整结尾 END')
                expect(live.locator('.mb.clip')).to_have_count(0)
                # Beyond the old character threshold, all assistant prose still renders.
                notify('item/agentMessage/delta', {'itemId': 'long-reply', 'delta': TEXT * 8 + '\n最终完整结尾'})
                expect(live.locator('.mb')).to_contain_text('最终完整结尾')
                assert live.locator('.mb').evaluate('(b) => b.scrollHeight <= b.clientHeight + 1')
                notify('item/completed', {'item': {'id': 'long-reply', 'type': 'agentMessage', 'text': TEXT * 9 + '\n最终完整结尾'}})
                notify('item/started', {'item': {'id': 'long-thought', 'type': 'reasoning', 'summary': [TEXT]}})
                thought = page.locator('[data-sdk-item="long-thought"]')
                more = thought.get_by_role('button', name='展开思考摘要')
                expect(more).to_be_visible()
                expect(thought.locator('.mb')).to_be_hidden()
                expect(status).to_have_text('思考中')
                status.click()
                expect(menu).to_contain_text(f'{len(TEXT):,} 字符摘要')
                page.keyboard.press('Escape')
                screenshot('thinking-preview')
                # An arriving delta between pointer down/up must not replace the button.
                more.scroll_into_view_if_needed()
                bounds = more.bounding_box()
                page.mouse.move(bounds['x'] + 10, bounds['y'] + 10)
                page.mouse.down()
                notify('item/reasoning/summaryTextDelta', {'itemId': 'long-thought', 'summaryIndex': 0, 'delta': '\n指针按下时新增'})
                expect(thought.locator('.mb')).to_contain_text('指针按下时新增')
                page.mouse.up()
                expect(thought.get_by_role('button', name='收起思考摘要')).to_be_visible()
                notify('item/reasoning/summaryTextDelta', {'itemId': 'long-thought', 'summaryIndex': 0, 'delta': '\n展开后新增'})
                expect(thought.locator('.mb')).to_contain_text('展开后新增')
                expect(thought.locator('.mb.clip')).to_have_count(0)
                thought.get_by_role('button', name='收起思考摘要').click()
                notify('item/reasoning/summaryTextDelta', {'itemId': 'long-thought', 'summaryIndex': 0, 'delta': '\n收起后新增'})
                expect(thought.locator('.mb')).to_contain_text('收起后新增')
                expect(thought.locator('.mb')).to_be_hidden()
                more.click()
                thought.get_by_role('button', name='收起思考摘要').scroll_into_view_if_needed()
                screenshot('streaming-expanded')
                output = 'long-output ' * 20 + '\n' + '\n'.join(f'工具输出行 {i}' for i in range(20))
                command = {'id': 'long-tool', 'type': 'commandExecution', 'command': 'synthetic-command',
                           'status': 'inProgress', 'aggregatedOutput': output}
                notify('item/started', {'item': command})
                tool = page.locator('[data-sdk-item="long-tool"]')
                tool.get_by_text('展开全文', exact=False).click()
                tool.get_by_role('button', name='展开原始参数', exact=True).click()
                tool.locator('.tool-wrap').last.click()
                notify('item/commandExecution/outputDelta', {'itemId': 'long-tool', 'delta': '\n工具新增尾行'})
                expect(tool.locator('.tool-out')).to_contain_text('工具新增尾行')
                expect(tool.locator('.tool-args')).to_be_visible()
                expect(tool.locator('.tool-wrap').last).to_have_attribute('aria-pressed', 'false')
                tool.get_by_text('收起', exact=True).click()
                notify('item/commandExecution/outputDelta', {'itemId': 'long-tool', 'delta': '\n工具仍收起'})
                expect(tool.get_by_text('展开全文', exact=False)).to_be_visible()
                expect(tool.locator('.tool-out')).to_contain_text('工具仍收起')
                expect(tool.locator('.tool-out')).to_have_class(__import__('re').compile('collapsed-output'))
                tool.get_by_text('展开全文', exact=False).click()
                notify('item/completed', {'item': {**command, 'status': 'completed',
                       'aggregatedOutput': output + '\n工具完成尾行', 'exitCode': 0}})
                expect(tool.locator('.tool-out')).to_contain_text('工具完成尾行')
                tool.get_by_text('收起', exact=True).scroll_into_view_if_needed()
                screenshot('tool-expanded')
                page.locator('#sdk-stop').click()
                expect(page.locator('#sdk-stop')).to_be_hidden()
                page.locator('#a-chat').click()
                page.locator('#a-sdk').click()
                expect(page.locator('.sdk-history').get_by_text('展开全文', exact=False).last).to_be_visible()
                context.close()
                print(f'PASS {theme}: persisted/live expansion, resize, view reentry', flush=True)
            browser.close()


if __name__ == '__main__':
    main()
