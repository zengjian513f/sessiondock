#!/usr/bin/env python3
"""Five SDKs share editor, queued Markdown, status menus and history expansion."""
import argparse
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from sdk_start_stop_fixtures import provider
from sdk_selection_fixtures import snapshots


def exercise(browser, base, source, events, width, theme, screenshots):
    context = browser.new_context(viewport={'width': width, 'height': 900},
        color_scheme=theme, service_workers='block')
    page = context.new_page()
    errors, held = [], []
    page.on('pageerror', lambda error: errors.append(str(error)))
    api = 'codex-sdk' if source == 'codex' else 'native-sdk'
    raw = {'items': [], 'requests': [], 'running': False, 'input_queue': [],
        'summary': {'settings': {'model': 'shared-model', 'effort': 'low'}, 'queue': [],
                    'status': {'type': 'idle'}},
        'model': 'shared-model', 'effort': 'low', 'capabilities': {'settings': True}}
    events.packet = {'type': 'snapshot', 'seq': 1, 'raw': raw}
    def publish():
        events.packet.setdefault('seq', 0)
        events.publish()
    context.route(f'**/api/{api}/open', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
    context.route(f'**/api/{api}/events?*', lambda r: r.continue_(url=events.url))
    context.route(f'**/api/{api}/models*', lambda r: r.fulfill(json={'ok': True, 'raw': []}))
    context.route(f'**/api/{api}/limits*', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
    context.route(f'**/api/{api}/usage', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
    context.route(f'**/api/{api}/turn', lambda r: held.append(r))
    history = [
        {'role': 'user', 'text': '请审阅共享界面', 'turn_id': 'past'},
        {'role': 'thinking', 'text': '需要保留用户展开的分析过程。\n' * 10, 'turn_id': 'past'},
        {'role': 'assistant', 'text': '历史结论', 'turn_id': 'past'},
        {'role': 'user', 'text': '接着检查发送与状态', 'turn_id': 'second'},
        {'role': 'assistant', 'text': '第二轮结论', 'turn_id': 'second'},
    ]
    def messages(route):
        response = route.fetch()
        data = response.json()
        data.update(messages=history, reset=True, start=0, end=100 + len(history),
                    message_total=len(history), partial=None, activity={'state': 'idle'})
        route.fulfill(response=response, json=data)
    context.route('**/api/messages/**', messages)
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    row = page.locator(f'#side .item[data-uid^="{source}:"]').first
    if not row.is_visible(): page.locator(f'#chips button[data-source="{source}"]').click()
    row.click()
    page.locator('#a-sdk').click()
    process = page.locator('.sdk-history .turn-process[data-turn-id="past"]')
    expect(process).to_be_visible()
    power = page.locator('#a-session-toggle')
    if not power.is_visible(): page.locator('#a-more').click()
    power.click()
    editor = page.locator('#sdk-input')
    expect(editor).to_be_enabled()
    try:
        expect(page.locator('.sdk-pane .sdk-status')).to_have_text('就绪')
    except AssertionError:
        print('SDK startup:', errors, page.evaluate('({readOnly:SdkView.readOnly, connected:SdkView.connected, stopped:SdkView.stopped, source:SdkView.es?.url})'), flush=True)
        raise
    process.locator('.fold-toggle').first.click()
    expect(process.locator('.turn-process-body')).to_be_visible()

    # A real history read after a synthetic native completion must retain the
    # user's expanded process; do not call the renderer or alter its DOM.
    if source == 'codex':
        events.packet = {'method': 'turn/started', 'raw': {'turn': {'id': 'live', 'items': []}}}
    else:
        raw['running'] = True
    publish()
    try:
        expect(page.locator('#sdk-stop')).to_be_visible()
    except AssertionError:
        print('SDK turn:', errors, page.evaluate('({readOnly:SdkView.readOnly, connected:SdkView.connected, stopped:SdkView.stopped, turn:SdkView.turn, source:SdkView.es?.url})'), flush=True)
        raise
    expect(page.locator('#sdk-stop')).to_have_attribute('title', '中断当前回合')
    history.append({'role': 'assistant', 'text': '历史刷新后的结论', 'turn_id': 'second'})
    if source == 'codex':
        events.packet = {'method': 'turn/completed', 'raw': {'turn': {'id': 'live', 'status': 'completed'}}}
    else:
        raw['running'] = False
    publish()
    expect(page.locator('.sdk-history')).to_contain_text('历史刷新后的结论')
    expect(process.locator('.turn-process-body')).to_be_visible()

    # Protocol adapters keep their native shapes, but preserve the same three
    # tool controls when streaming output replaces a rendered tool.
    output = 'long-output ' * 20 + '\n' + '\n'.join(f'工具输出行 {i}' for i in range(20))
    def tool_update(tail):
        if source == 'codex':
            events.packet = {'method': 'item/started', 'raw': {'item': {
                'id': 'shared-tool', 'type': 'commandExecution', 'command': 'synthetic-command',
                'status': 'inProgress', 'aggregatedOutput': output + tail}}}
        else:
            raw['items'] = [{'id': 'shared-tool', 'role': 'tool', 'name': 'exec_command',
                'summary': '$ synthetic-command',
                'input': {'command': 'synthetic-command'}, 'result': output + tail,
                'status': 'in_progress'}]
        publish()
    tool_update('')
    tool = page.locator('[data-sdk-item="shared-tool"]')
    tool.get_by_text('展开全文', exact=False).click()
    tool.get_by_role('button', name='展开原始参数', exact=True).click()
    tool.get_by_role('button', name='自动换行', exact=True).click()
    tool_update('\n工具新增尾行')
    expect(tool.locator('.tool-out')).to_contain_text('工具新增尾行')
    expect(tool.locator('.tool-args')).to_be_visible()
    expect(tool.get_by_role('button', name='保持原行', exact=True)).to_have_attribute('aria-pressed', 'true')
    tool.get_by_text('收起', exact=True).click()
    tool_update('\n折叠后仍保留选择')
    expect(tool.get_by_text('展开全文', exact=False)).to_be_visible()
    expect(tool.locator('.tool-out')).not_to_contain_text('折叠后仍保留选择')
    raw['items'] = []
    events.packet = {'type': 'snapshot', 'raw': raw}
    publish()
    expect(tool).to_have_count(0)

    editor.fill('**检查** `shared` 的用户输入')
    editor.press('Shift+Enter')
    editor.press('End')
    editor.press('Enter') if width < 500 else editor.type('第二行')
    assert not held
    # Mouse SEND keeps editor focus; writing the next draft during admission
    # must survive the previous submission's response.
    editor.focus()
    with page.expect_request(f'**/api/{api}/turn'): page.locator('#sdk-send').click()
    expect(editor).to_be_focused()
    page.wait_for_function("document.querySelector('#sdk-send').getAttribute('aria-busy') === 'true'")
    assert held
    submitted = held[0].request.post_data_json['text']
    editor.fill('发送期间写下的下一条草稿')
    row = {'id': 'queued', 'state': 'queued', 'text': submitted}
    if source == 'codex':
        raw['summary']['queue'] = [row]
    else:
        raw['input_queue'] = [dict(row, id='bridge:queued')]
    held.pop().fulfill(json={'ok': True, 'accepted': True, 'raw': {'queued': True}})
    events.packet = {'type': 'snapshot', 'raw': raw}
    publish()
    queued = page.locator('.sdk-queue .msg[data-role="user"]')
    expect(queued).to_have_count(1)
    expect(queued.locator('.mb b')).to_have_text('检查')
    expect(queued.locator('.mb code')).to_have_text('shared')
    expect(editor).to_have_value('发送期间写下的下一条草稿')
    expect(queued.get_by_role('button', name='撤销', exact=True)).to_be_visible()

    error = '原生请求失败：保留完整原因，不能改变其他来源的权限与投递语义。'
    raw['error'] = error
    raw['summary']['last_error'] = {'message': error}
    publish()
    status, menu = page.locator('.sdk-pane .sdk-status'), page.locator('#sdk-status-menu')
    expect(status).to_be_enabled()
    status.press('Enter')
    expect(menu).to_be_visible()
    expect(menu).to_contain_text(error)
    if screenshots: page.screenshot(path=str(screenshots/f'{source}-{width}-{theme}.png'), full_page=True)
    page.set_viewport_size({'width': width + 20, 'height': 900})
    expect(menu).not_to_be_visible()
    raw.pop('error')
    raw['summary'].pop('last_error')
    raw['summary']['queue'] = []
    raw['input_queue'] = []
    publish()
    expect(status).to_have_text('就绪')
    expect(status).to_be_disabled()
    assert not errors, errors
    context.close()
    print(f'PASS {source} {width}/{theme}: history/tool expansion, editor focus/draft, Markdown queue, status/resize', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-shared-ui-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in ('codex', 'agy', 'grok', 'claude', 'opencode'):
                root = Path(tmp)/source; root.mkdir()
                with provider(source, root, args.binary) as (base, _, _), snapshots() as events:
                    for width, theme in ((1440, 'light'), (390, 'dark')):
                        exercise(browser, base, source, events, width, theme, args.screenshots)
        finally: browser.close()


if __name__ == '__main__': main()
