#!/usr/bin/env python3
"""Codex subagent SDK views: owned thread routing through node/Hub Chromium.

Synthetic native histories and a fake daemon only. Exercises root/child/nested
selection, live message and tool output, approval/question replies, SEND,
steering, interrupt, reload and reconnect on desktop/mobile in both themes.
"""
from __future__ import annotations

import argparse
import asyncio
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import urlencode

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build, server
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, batch35_agent_meta, codex_message, encoded
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env

CHILD = '0199a100-0000-7000-8000-000000000002'
NESTED = '0199a100-0000-7000-8000-000000000003'
SIBLING = '0199a100-0000-7000-8000-000000000004'


def add_children(corpus):
    for child, parent in ((CHILD, THREAD), (NESTED, CHILD), (SIBLING, THREAD)):
        corpus.put(child, 'codex', [batch35_agent_meta(child, parent),
            codex_message('user', '检查 SDK 子代理历史 ' + child),
            codex_message('assistant', 'Child history sentinel ' + child)], [], parent='sdk')


class SubagentServer(FakeAppServer):
    def __init__(self, socket_path, corpus):
        super().__init__(socket_path, corpus.paths['sdk'], THREAD)
        self.paths = {THREAD: corpus.paths['sdk'], **{child: corpus.paths[child] for child in (CHILD, NESTED, SIBLING)}}
        self.active = {}
        self.pending_replies = {}

    async def emit(self, thread, method, **params):
        await self.notify(method, {'threadId': thread, **params})

    async def item(self, thread, turn, item):
        await self.emit(thread, 'item/completed', turnId=turn, item=item)

    async def begin(self, thread, text):
        turn = 'child-turn-' + str(len(self.methods('turn/start')))
        self.active[thread] = turn
        with self.paths[thread].open('ab') as f:
            f.write(encoded(codex_message('user', text)))
        await self.emit(thread, 'turn/started', turn={'id': turn, 'status': 'inProgress', 'items': []})
        await self.item(thread, turn, {'type': 'userMessage', 'id': turn + '-user', 'content': [{'type': 'text', 'text': text}]})
        await self.item(thread, turn, {'type': 'agentMessage', 'id': turn + '-assistant', 'text': '', 'phase': 'commentary'})
        await self.emit(thread, 'item/agentMessage/delta', turnId=turn, itemId=turn + '-assistant', delta='Child live stream ' + thread)
        command = {'id': turn + '-cmd', 'type': 'commandExecution', 'command': 'echo synthetic-child', 'status': 'inProgress'}
        await self.emit(thread, 'item/started', turnId=turn, item=command)
        await self.emit(thread, 'item/commandExecution/outputDelta', turnId=turn, itemId=command['id'], delta='Child tool output ' + thread)
        if text.startswith(('approve', 'ask')):
            key = 'request-' + turn
            question = text.startswith('ask')
            method = 'item/tool/requestUserInput' if question else 'item/commandExecution/requestApproval'
            params = {'threadId': thread, 'turnId': turn, 'itemId': command['id']}
            params.update({'questions': [{'id': 'color', 'header': 'Color', 'question': 'Child question?', 'options': [{'label': 'Blue', 'description': 'Synthetic blue'}]}]} if question else
                          {'command': 'echo synthetic-child', 'cwd': '/synthetic/sdk', 'reason': 'Child approval', 'availableDecisions': ['accept', 'decline']})
            self.pending_replies[key] = thread
            for client in list(self.clients):
                await self.send(client, {'method': method, 'id': key, 'params': params})
        return turn

    async def finish(self, thread, text, status='completed'):
        turn = self.active.pop(thread)
        with self.paths[thread].open('ab') as f:
            f.write(encoded(codex_message('assistant', text)))
        await self.item(thread, turn, {'type': 'agentMessage', 'id': turn + '-final', 'text': text, 'phase': 'final_answer'})
        await self.emit(thread, 'turn/completed', turn={'id': turn, 'status': status, 'items': []})

    async def _handle(self, writer, message):
        method, mid, params = message.get('method'), message.get('id'), message.get('params') or {}
        async def reply(value):
            await self.send(writer, {'id': mid, 'result': value})
        if method is None and mid in self.pending_replies:
            thread = self.pending_replies.pop(mid)
            assert 'error' not in message, message
            await self.emit(thread, 'serverRequest/resolved', requestId=mid)
            await self.finish(thread, 'Child completed response ' + thread)
        elif method == 'thread/resume':
            thread = params['threadId']
            assert thread in self.paths, params
            await reply({'thread': {'id': thread, 'status': {'type': 'active' if thread in self.active else 'idle'}, 'turns': []},
                         'model': 'fake-sol', 'reasoningEffort': 'high'})
        elif method == 'turn/start':
            thread = params['threadId']
            assert thread in self.paths, params
            turn = await self.begin(thread, params['input'][0]['text'])
            await reply({'turn': {'id': turn, 'status': 'inProgress', 'items': []}})
        elif method == 'turn/steer':
            thread = params['threadId']
            assert params['expectedTurnId'] == self.active[thread], params
            await self.item(thread, self.active[thread], {'type': 'userMessage', 'id': self.active[thread] + '-steer', 'content': params['input']})
            await reply({'turnId': self.active[thread]})
        elif method == 'turn/interrupt':
            thread = params['threadId']
            assert params['turnId'] == self.active[thread], params
            await self.finish(thread, 'Child interrupted ' + thread, 'interrupted')
            await reply({})
        else:
            await super()._handle(writer, message)

    def unrelated(self):
        async def send():
            for thread in (THREAD, SIBLING):
                await self.item(thread, 'unrelated', {'type': 'agentMessage', 'id': 'unrelated-' + thread, 'text': 'Unrelated live sentinel ' + thread})
        asyncio.run_coroutine_threadsafe(send(), self.loop).result(timeout=5)

    def disconnect(self):
        async def close():
            for client in list(self.clients):
                client.close()
                await client.wait_closed()
        asyncio.run_coroutine_threadsafe(close(), self.loop).result(timeout=5)


def exercise(browser, base, fake, width, theme, screenshots, label):
    print(label + ': child history, live tools, replies, steer/stop, nested switching, reload/reconnect', flush=True)
    context = browser.new_context(viewport={'width': width, 'height': 844 if width < 500 else 900}, color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(10000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    def visit(agent=None):
        sid = 'codex:' + THREAD + ('/agent:' + agent if agent else '')
        page.goto(base + '/?' + urlencode({'sid': sid}), wait_until='domcontentloaded')
        expect(page.locator('#a-sdk')).to_have_attribute('aria-pressed', 'true')
        pane = page.locator('.sdk-pane')
        expect(pane.locator('.sdk-history')).to_contain_text('Child history sentinel ' + agent if agent else 'earlier synthetic answer')
        expect(pane.locator("#sdk-model")).to_have_value("fake-sol")
        assert page.evaluate('CodexSdkView.agent') == agent
        return pane
    root_before = fake.paths[THREAD].read_bytes()
    pane = visit(CHILD)
    expect(pane.locator('.sdk-history')).not_to_contain_text('earlier synthetic answer')
    resumes_before = len(fake.methods('thread/resume'))
    missing = page.request.post(base + '/api/codex-sdk/open', data={'uid': page.evaluate('S.sel'), 'agent': '0199a100-0000-7000-8000-ffffffffffff'})
    assert missing.status == 404, missing.text()
    assert len(fake.methods('thread/resume')) == resumes_before, 'unknown child must not fall back to the owner'
    box = pane.locator('#sdk-input')
    box.fill('approve child SDK command ' + label)
    pane.locator('#sdk-send').click()
    expect(pane.locator('.sdk-live')).to_contain_text('Child live stream ' + CHILD)
    expect(pane.locator('.sdk-live')).to_contain_text('Child tool output ' + CHILD)
    expect(pane.locator('.sdk-request')).to_contain_text('Child approval')
    fake.unrelated()
    expect(pane).not_to_contain_text('Unrelated live sentinel')
    # A reload restores exactly the child's stream and outstanding request.
    page.reload(wait_until='domcontentloaded')
    expect(pane.locator('.sdk-request')).to_contain_text('Child approval')
    expect(pane.locator('.sdk-live')).to_contain_text('Child live stream ' + CHILD)
    if screenshots:
        page.screenshot(path=str(screenshots / (label + '-approval.png')))
    pane.locator('.sdk-request').get_by_role('button', name='批准', exact=True).click()
    expect(pane.locator('.sdk-history')).to_contain_text('Child completed response ' + CHILD)
    expect(pane.locator('.sdk-request')).to_have_count(0)
    box = pane.locator('#sdk-input')
    box.fill('ask child SDK question ' + label)
    pane.locator('#sdk-send').click()
    expect(pane.locator('.sdk-request')).to_contain_text('Child question?')
    pane.locator('.sdk-request').get_by_label('Blue', exact=True).check()
    pane.locator('.sdk-request').get_by_role('button', name='提交', exact=True).click()
    expect(pane.locator('#sdk-stop')).to_be_hidden()
    box.fill('hold child SDK work ' + label)
    pane.locator('#sdk-send').click()
    expect(pane.locator('#sdk-stop')).to_be_visible()
    box.fill('steer selected child ' + label)
    pane.locator('#sdk-send').click()
    expect(pane.locator('.sdk-queued')).to_contain_text('steer selected child ' + label)
    pane.locator('.sdk-queued [data-queue-action="steer"]').click()
    expect(pane.locator('.sdk-live')).to_contain_text('steer selected child ' + label)
    assert fake.methods('turn/steer')[-1]['params']['threadId'] == CHILD
    pane.locator('#sdk-stop').click()
    expect(pane.locator('#sdk-stop')).to_be_hidden()
    assert fake.methods('turn/interrupt')[-1]['params']['threadId'] == CHILD
    assert all(m['params']['threadId'] == CHILD for m in fake.methods('turn/start')), fake.methods('turn/start')
    assert fake.paths[THREAD].read_bytes() == root_before, 'child operations must not change the root'

    # View preferences are independent for root and child, with a real menu switch.
    page.locator('#a-chat').click()
    expect(page.locator('.sdk-pane')).to_have_count(0)
    page.locator('#a-view-switch').click()
    page.locator('#session-view-menu [data-agent=""]').click()
    expect(page.locator('.sdk-pane .sdk-history')).to_contain_text('earlier synthetic answer')
    page.locator('#a-view-switch').click()
    page.locator(f'#session-view-menu [data-agent="{CHILD}"]').click()
    expect(page.locator('.sdk-pane')).to_have_count(0)
    page.locator('#a-sdk').click()
    expect(page.locator('.sdk-history')).to_contain_text('Child history sentinel ' + CHILD)

    pane = visit(NESTED)
    expect(pane.locator('.sdk-history')).not_to_contain_text('Child history sentinel ' + CHILD)
    resumes = sum(m['params']['threadId'] == NESTED for m in fake.methods('thread/resume'))
    generation = page.evaluate('CodexSdkView.generation')
    fake.disconnect()
    page.wait_for_function('previous => CodexSdkView.generation > previous && !!CodexSdkView.summary', arg=generation)
    assert sum(m['params']['threadId'] == NESTED for m in fake.methods('thread/resume')) > resumes
    expect(pane.locator('#sdk-model')).to_have_value('fake-sol')
    page.wait_for_function('CodexSdkView.summary?.resume?.thread?.id === ' + repr(NESTED))
    if screenshots:
        page.screenshot(path=str(screenshots / (label + '-nested.png')))
    visit(SIBLING)
    expect(page.locator('.sdk-history')).not_to_contain_text('Child completed response ' + CHILD)
    visit()
    expect(page.locator('.sdk-history')).not_to_contain_text('Child history sentinel')
    assert not errors, errors
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-agent-') as tmp:
        root = Path(tmp)
        corpus = build(root)
        add_children(corpus)
        port = free_port()
        with SubagentServer(root/'app.sock', corpus) as fake, server(corpus, args.binary, root/'app.sock', node_env(root, port, '127.0.0.0/8')) as base:
            hub_root = root/'hub'
            hub_root.mkdir()
            hub = Hub((args.hub_binary or args.binary.with_name('sessiondock-hub')).resolve(), hub_root, [SimpleNamespace(name='SdkNode', token=TOKEN, port=port)])
            hub.start()
            try:
                with sync_playwright() as p:
                    browser = p.chromium.launch(headless=True)
                    try:
                        for mode, url in (('node', base), ('hub', f'http://127.0.0.1:{hub.port}')):
                            for width, theme in ((1280, 'light'), (390, 'dark'), (1280, 'dark'), (390, 'light')):
                                exercise(browser, url, fake, width, theme, args.screenshots, f'{mode}-{width}-{theme}')
                    finally:
                        browser.close()
            finally:
                hub.stop()
    print('PASS SDK subagent views: node/Hub, selected native child thread, history/live/replies/send/steer/stop, root/sibling/nested isolation, reload/reconnect, desktop/mobile light/dark')


if __name__ == '__main__':
    main()
