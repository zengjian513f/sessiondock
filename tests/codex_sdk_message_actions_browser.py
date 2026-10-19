#!/usr/bin/env python3
"""User-bubble revert/fork menus and restored drafts through node/Hub Chromium."""
from __future__ import annotations
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from codex_sdk_actions_browser import ActionsServer, THREAD, TURN2, build, environment
from codex_sdk_browser import server
from codex_sdk_compose_browser import saved
from history_fixtures import BINARY, codex_row, encoded
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env

TEXT = ('第二轮 remove this question · 请检查真实长度的项目路径 /workspace/projects/sessiondock/long-project-name\n保留中文、换行及 `quoted code`，重新编辑后发送。'
        + '\n\n'.join(f'\n第 {i} 段：检查多行绿气泡的消息操作按钮在展开和收起后仍然居中。' for i in range(16)))
FORK = '01a1adc0-0000-7000-8000-000000000099'
ROLLOUT = '01a1adc0-0000-7000-8000-000000000100'


def aligned(bubble):
    toggle = bubble.get_by_role('button', name='消息操作', exact=True)
    toggle.scroll_into_view_if_needed()
    button_box, bubble_box = toggle.bounding_box(), bubble.bounding_box()
    assert abs(button_box['y'] + button_box['height']/2 - bubble_box['y'] - bubble_box['height']/2) <= 1, (button_box, bubble_box)
    assert button_box['x'] >= 0 and button_box['x'] + button_box['width'] <= bubble_box['x'], (button_box, bubble_box)
    assert toggle.evaluate('(n) => { const r = n.getBoundingClientRect(); return n.contains(document.elementFromPoint(r.x + r.width/2, r.y + r.height/2)); }')


class MessageServer(ActionsServer):
    async def _handle(self, writer, message):
        method, params = message.get('method'), message.get('params') or {}
        if method == 'thread/fork':
            assert params == {'threadId': THREAD, 'excludeTurns': True, 'lastTurnId': TURN2}
            rows = [json.loads(line) for line in self.source_path.read_text().splitlines()]
            self.fork_path = self.source_path.parent / f'rollout-2026-10-17T12-00-00-{FORK}.jsonl'
            meta = codex_row('session_meta', {'id': FORK, 'forked_from_id': THREAD,
                'cwd': '/synthetic/sdk-actions/项目-long-workspace', 'timestamp': '2026-10-17T12:00:00Z',
                'history_mode': 'paginated', 'history_base': {'thread_id': THREAD,
                    'end_byte_offset': self.source_path.stat().st_size, 'end_ordinal_exclusive': len(rows)}}, len(rows))
            self.fork_path.write_bytes(encoded(meta))
            self.paths[FORK] = self.fork_path
            await self.send(writer, {'id': message['id'], 'result': {'thread': {
                'id': FORK, 'path': str(self.fork_path), 'cwd': meta['payload']['cwd'],
                'createdAt': 1792238400, 'status': {'type': 'idle'}, 'turns': []},
                'model': 'fake-sol', 'reasoningEffort': 'high'}})
            return
        if method == 'thread/revert' and params.get('threadId') == FORK:
            assert params['beforeTurnId'] == TURN2
            rows = [json.loads(line) for line in self.source_path.read_text().splitlines()]
            stop = next(i for i, row in enumerate(rows) if row.get('type') == 'turn_context' and row['payload']['turn_id'] == TURN2)
            # Codex 0.163 reverts inside inherited history by rotating this
            # same thread to a shorter prefix of the same physical ancestor.
            meta = json.loads(self.fork_path.read_text())
            meta['ordinal'] = stop
            meta['payload']['timestamp'] = '2026-10-17T12:00:00.100Z'
            meta['payload']['history_base'].update(end_byte_offset=len(b''.join(encoded(row) for row in rows[:stop])),
                                                  end_ordinal_exclusive=stop)
            path = self.fork_path.with_name(f'rollout-2026-10-17T12-00-00-{FORK}_{ROLLOUT}.jsonl')
            path.write_bytes(encoded(meta))
            self.paths[FORK] = path
            await self.notify('thread/reverted', {'threadId': FORK})
            await self.send(writer, {'id': message['id'], 'result': {'thread': {
                'id': FORK, 'path': str(path), 'cwd': meta['payload']['cwd'],
                'createdAt': 1792238400, 'status': {'type': 'idle'}, 'turns': []}}})
            return
        await super()._handle(writer, message)


def exercise(browser, base, fake, width, theme, screenshots, label):
    context = browser.new_context(viewport={'width': width, 'height': 844}, color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page(); errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='networkidle')
    page.locator('#side .item').filter(has_text='第一轮 retained question').first.click()
    page.wait_for_function('CodexSdkView.connected && !!CodexSdkView.historyData')
    source = page.evaluate('S.sel')
    def bubble():
        return page.locator('.sdk-history .msg[data-role="user"]').filter(has_text='第二轮 remove this question')
    def menu():
        aligned(bubble())
        bubble().get_by_role('button', name='消息操作', exact=True).click()
        popup = bubble().locator('.sdk-bubble-menu')
        expect(popup).to_be_visible()
        button_box = bubble().locator('.sdk-bubble-toggle').bounding_box()
        bubble_box = bubble().bounding_box()
        assert button_box['x'] >= 0 and button_box['x'] + button_box['width'] <= bubble_box['x'], (button_box, bubble_box)
        bounds = popup.bounding_box()
        assert bounds['x'] >= 0 and bounds['x'] + bounds['width'] <= width + 1
        assert bounds['y'] >= 0 and bounds['y'] + bounds['height'] <= 845
        return popup
    aligned(page.locator('.sdk-history .msg[data-role="user"]').first)
    aligned(bubble())
    if screenshots: page.screenshot(path=str(screenshots/f'{label}-collapsed.png'))
    bubble().get_by_role('button', name='展开全文', exact=False).click()
    aligned(bubble())
    if screenshots: page.screenshot(path=str(screenshots/f'{label}-expanded.png'))
    bubble().get_by_role('button', name='收起', exact=True).click()
    aligned(bubble())
    popup = menu()
    if screenshots: page.screenshot(path=str(screenshots/f'{label}-menu.png'))
    original = fake.source_path.read_bytes()
    with page.expect_response(lambda response: response.url.endswith('/api/codex-sdk/fork')) as fork_response:
        popup.get_by_role('button', name='分支', exact=True).click()
    fork_result = fork_response.value.json()
    assert fork_result['raw']['thread']['path'] == str(fake.paths[FORK]), fork_result
    page.wait_for_function('uid => S.sel !== uid', arg=source)
    branch = page.evaluate('S.sel')
    assert branch.endswith(fork_result['uid'].split(':', 1)[1]), (branch, fork_result)
    assert len(fake.methods('thread/fork')) == 1
    assert fake.source_path.read_bytes() == original
    assert fake.fork_path.is_file() and fake.paths[FORK].is_file()
    expect(page.locator('#side .item')).to_have_count(2)
    expect(page.locator('#sdk-input')).to_have_value(TEXT)
    expect(page.locator('.sdk-history')).to_contain_text('retained answer')
    expect(page.locator('.sdk-history')).not_to_contain_text('remove this answer')
    if screenshots: page.screenshot(path=str(screenshots/f'{label}-forked.png'))
    assert fake.methods('thread/fork')[-1]['params']['lastTurnId'] == TURN2
    assert fake.methods('thread/revert')[-1]['params'] == {'threadId': FORK, 'beforeTurnId': TURN2}
    saved(page, branch, TEXT)
    page.reload(wait_until='networkidle')
    expect(page.locator('#side .item')).to_have_count(2)
    expect(page.locator('#sdk-input')).to_have_value(TEXT)
    if width < 500: page.get_by_role('button', name='返回会话列表', exact=True).click()
    page.locator(f'#side .item[data-uid="{source}"]').click()
    expect(page.locator('.sdk-history')).to_contain_text('remove this answer')
    page.locator('#sdk-input').fill('原本尚未发送的草稿')
    popup = menu(); popup.get_by_role('button', name='回退', exact=True).click()
    page.locator('dialog[open]').get_by_role('button', name='取消', exact=True).click()
    expect(page.locator('#sdk-input')).to_have_value('原本尚未发送的草稿')
    expect(bubble()).to_be_visible()
    popup = menu(); popup.get_by_role('button', name='回退', exact=True).click()
    page.locator('dialog[open]').get_by_role('button', name='回退', exact=True).click()
    expect(page.locator('#sdk-input')).to_have_value('原本尚未发送的草稿\n\n' + TEXT)
    expect(page.locator('.sdk-history')).not_to_contain_text('remove this answer')
    assert page.evaluate('S.sel') == source
    assert not fake.methods('turn/start'), 'restoring a message sent it without user action'
    if screenshots: page.screenshot(path=str(screenshots/f'{label}-restored.png'))
    saved(page, source, '原本尚未发送的草稿\n\n' + TEXT)
    page.reload(wait_until='networkidle')
    expect(page.locator('#sdk-input')).to_have_value('原本尚未发送的草稿\n\n' + TEXT)
    # A user message started by another client uses the same menu on a live bubble.
    async def live_message():
        await fake.notify('turn/started', {'threadId': THREAD, 'turn': {'id': 'alignment-live', 'status': 'inProgress', 'items': []}})
        await fake.notify('item/completed', {'threadId': THREAD, 'turnId': 'alignment-live', 'item': {
            'id': 'alignment-user', 'type': 'userMessage', 'content': [{'type': 'text', 'text': '实时绿气泡 Live message'}]}})
    asyncio.run_coroutine_threadsafe(live_message(), fake.loop).result(timeout=5)
    live = page.locator('.sdk-live .msg[data-role="user"]')
    expect(live).to_contain_text('实时绿气泡')
    aligned(live)
    live.get_by_role('button', name='消息操作', exact=True).click()
    expect(live.locator('.sdk-bubble-menu')).to_be_visible()
    if screenshots: page.screenshot(path=str(screenshots/f'{label}-live.png'))
    assert branch != source and not errors, errors
    context.close()
    print('PASS ' + label + ': bubble menu, branch before selected turn, source preservation, revert/cancel, restored drafts/reload', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-bubbles-') as tmp, sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        try:
            for mode, width, theme in [('node', 1280, 'light'), ('node', 390, 'dark'), ('hub', 1280, 'dark'), ('hub', 390, 'light'), ('node', 3406, 'dark')]:
                label = f'{mode}-{width}-{theme}'; root = Path(tmp)/label; root.mkdir()
                corpus, retained = build(root)
                path = corpus.paths['sdk']
                rows = [json.loads(line) for line in path.read_text().splitlines()]
                turn = None
                for ordinal, row in enumerate(rows):
                    row['ordinal'] = ordinal
                    if row['type'] == 'turn_context': turn = row['payload']['turn_id']
                    elif row['type'] == 'response_item': row['payload']['turn_id'] = turn
                for row in rows:
                    if row.get('payload', {}).get('role') == 'user' and turn:
                        for part in row['payload']['content']:
                            if part.get('text') == '第二轮 remove this question': part['text'] = TEXT
                path.write_bytes(b''.join(encoded(row) for row in rows))
                stop = next(i for i, row in enumerate(rows) if row.get('type') == 'turn_context' and row['payload']['turn_id'] == TURN2)
                retained = b''.join(encoded(row) for row in rows[:stop])
                port = free_port(); env = {**environment(root, args.binary), **node_env(root, port, '127.0.0.0/8')}
                with MessageServer(root/'app.sock', path, retained) as fake, server(corpus, args.binary, root/'app.sock', env) as base:
                    if mode == 'hub':
                        hub_root = root/'hub'; hub_root.mkdir()
                        hub = Hub(args.binary.with_name('sessiondock-hub').resolve(), hub_root, [SimpleNamespace(name='MessageNode', token=TOKEN, port=port)])
                        hub.start()
                        try: exercise(browser, f'http://127.0.0.1:{hub.port}', fake, width, theme, args.screenshots, label)
                        finally: hub.stop()
                    else: exercise(browser, base, fake, width, theme, args.screenshots, label)
        finally: browser.close()


if __name__ == '__main__': main()
