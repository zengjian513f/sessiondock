#!/usr/bin/env python3
"""Codex background terminals through Chromium and an isolated fake daemon."""

import argparse
import asyncio
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build, server
from codex_sdk_subagent_browser import CHILD, SubagentServer, add_children
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env


LIST = 'thread/backgroundTerminals/list'
LATE = 'same-item-in-two-threads'
FOREGROUND = 'newly-listed-item'


def command(item, process, label):
    return {'itemId': item, 'processId': process, 'command': label, 'cwd': '/synthetic/sdk'}


class BackgroundServer(SubagentServer):
    def __init__(self, socket_path, corpus):
        super().__init__(socket_path, corpus)
        self.loaded.update((THREAD, CHILD))
        self.supports_background = True
        self.fail_pages = False
        self.terminals = {
            THREAD: [command(LATE, '1', 'running root command')],
            CHILD: [command(LATE, '1', 'different child command')],
        }

    async def _handle(self, writer, message):
        if message.get('method') != LIST:
            await super()._handle(writer, message)
            return
        assert writer in self.query_clients, 'background reads must not use the durable input channel'
        assert any(m['params']['capabilities']['experimentalApi'] for m in self.methods('initialize')
                   if m['params']['clientInfo']['name'] == 'sessiondock-query')
        request_id = message['id']
        params = message['params']
        thread = params['threadId']
        offset = int(params.get('cursor') or 0)
        if not self.supports_background:
            await self.send(writer, {'id': request_id, 'error': {'code': -32601, 'message': 'unknown method'}})
        elif thread not in self.loaded:
            await self.send(writer, {'id': request_id, 'error': {'code': -32600, 'message': 'thread not found: ' + thread}})
        elif self.fail_pages and offset:
            await self.send(writer, {'id': request_id, 'error': {'code': -32603, 'message': 'temporary fake query failure'}})
        else:
            rows = self.terminals.get(thread, [])
            await self.send(writer, {'id': request_id, 'result': {
                'data': rows[offset:offset + 1], 'nextCursor': str(offset + 1) if offset + 1 < len(rows) else None,
            }})

    def native(self, coroutine):
        asyncio.run_coroutine_threadsafe(coroutine, self.loop).result(timeout=5)

    async def delta(self, thread, item, text):
        await self.emit(thread, 'item/commandExecution/outputDelta', turnId='foreground-turn', itemId=item, delta=text)

    async def start_and_finish_foreground(self):
        task = command(FOREGROUND, '2', 'new command keeps running')
        turn = {'id': 'foreground-turn', 'status': 'inProgress', 'items': []}
        await self.emit(THREAD, 'turn/started', turn=turn)
        await self.emit(THREAD, 'item/started', turnId=turn['id'], item={
            'id': task['itemId'], 'type': 'commandExecution', 'command': task['command'], 'status': 'inProgress',
        })
        await self.delta(THREAD, task['itemId'], 'output before the first list\n')
        self.terminals[THREAD].append(task)
        await self.emit(THREAD, 'turn/completed', turn={**turn, 'status': 'completed'})
        await self.emit(THREAD, 'thread/status/changed', status={'type': 'idle'})


def rows(page, tasks):
    page.wait_for_function('''tasks => {
        const rows = CodexSdkView.es?.testBackground;
        return Array.isArray(rows) && JSON.stringify(rows.map(row => row.task)) === JSON.stringify(tasks);
    }''', arg=tasks)
    result = page.evaluate('CodexSdkView.es.testBackground')
    assert all(row['output_available'] and 'output' not in row for row in result), result
    return result


def start_sdk(page):
    page.wait_for_function("""() => {
      return SdkView.pane && !SdkView.historyReady && !SdkView.discovering;
    }""")
    if page.evaluate('SdkView.readOnly'):
        page.locator('.sdk-pane .composer-run-action').click()
    page.wait_for_function('SdkView.connected')


def exercise(browser, base, fake, width, theme, screenshots, label):
    context = browser.new_context(service_workers='block', viewport={'width': width, 'height': 850},
                                  color_scheme=theme)
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    # Inspect the actual SDK view's SSE without opening another native subscription.
    context.add_init_script('''window.EventSource = class extends EventSource {
        constructor(...args) {
            super(...args);
            this.addEventListener('sdk', ({data}) => {
                const event = JSON.parse(data);
                if (event.type === 'snapshot' || event.method === 'bridge/backgroundCommands')
                    this.testBackground = event.raw.background_commands;
                if (event.method === 'item/commandExecution/outputDelta')
                    this.testDelta = event.raw.delta;
            });
        }
    };''')
    page = context.new_page()
    page.set_default_timeout(15000)
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=THREAD)
    uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', THREAD)
    footer = page.locator('#sdk-composer .sdk-status-row')

    def read(route, agent='', **params):
        reply = page.request.post(base + '/api/native-sdk/' + route, data={'uid': uid, 'agent': agent, **params})
        assert reply.ok, reply.text()
        return reply.json()['raw']

    # A shared daemon can answer for an already loaded thread before a browser
    # activates SDK mode. The independent read grants no output subscription.
    ordinary = read('background_tasks')['background_commands']
    assert ordinary == [{'task': LATE, 'command': 'running root command'}], ordinary
    assert not fake.methods('thread/resume'), fake.methods('thread/resume')
    assert read('background_output', task=LATE) is None

    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.locator('#a-chat').click()
    chat = page.locator('#conversation-status')
    chat.get_by_role('button', name='后台任务 1', exact=True).click()
    expect(page.locator('.background-command-list:popover-open, .task-output-dialog[open]')).to_contain_text('running root command')
    page.keyboard.press('Escape')
    page.locator('#a-sdk').click()
    start_sdk(page)
    assert rows(page, [LATE])[0]['command'] == 'running root command'
    initial = read('background_output', task=LATE)
    assert initial['output'] == '' and initial['clipped'], initial
    assert read('background_output', task='forged-item') is None
    fake.native(fake.emit(THREAD, 'item/commandExecution/terminalInteraction', turnId='foreground-turn',
                          itemId=LATE, processId='1', stdin='never included in output'))
    fake.native(fake.delta(THREAD, LATE, 'first observed root output\n'))
    page.wait_for_function('CodexSdkView.es?.testDelta === "first observed root output\\n"')
    assert read('background_output', task=LATE)['output'] == 'first observed root output\n'
    footer.get_by_role('button', name='后台任务 1', exact=True).click()
    dialog = page.get_by_role('dialog', name='后台输出')
    expect(dialog.locator('.task-output-text')).to_have_text('first observed root output\n')
    expect(dialog.get_by_role('link', name='下载完整日志')).to_have_count(0)
    if screenshots:
        page.screenshot(path=str(screenshots / f'{label}-output.png'))
    page.keyboard.press('Escape')

    fake.native(fake.start_and_finish_foreground())
    expect(page.locator('#sdk-send')).to_have_text('发送')
    rows(page, [LATE, FOREGROUND])
    footer.get_by_role('button', name='后台任务 2', exact=True).click()
    menu = footer.get_by_role('dialog', name='后台任务')
    expect(menu.get_by_role('button', name='running root command', exact=False)).to_be_visible()
    expect(menu.get_by_role('button', name='new command keeps running', exact=False)).to_be_visible()
    if screenshots:
        page.screenshot(path=str(screenshots / f'{label}-list.png'))
    page.keyboard.press('Escape')
    first = read('background_output', task=FOREGROUND)
    assert first == {'output': 'output before the first list\n'}, first
    assert any(m['params'].get('cursor') == '1' for m in fake.methods(LIST)), fake.methods(LIST)
    fake.fail_pages = True
    before = len([m for m in fake.methods(LIST) if m['params'].get('cursor') == '1'])
    fake.wait_for(lambda: len([m for m in fake.methods(LIST) if m['params'].get('cursor') == '1']) > before)
    assert read('background_tasks')['background_commands'] == rows(page, [LATE, FOREGROUND])
    assert read('background_output', task=FOREGROUND) == first
    fake.fail_pages = False

    big = '前' * 23000 + ' tail\n'
    fake.native(fake.delta(THREAD, LATE, big))
    page.wait_for_function('delta => CodexSdkView.es?.testDelta === delta', arg=big)
    tail = read('background_output', task=LATE)
    encoded = big.encode()[-65536:].decode(errors='ignore')
    assert tail['output'] == encoded and tail['clipped'], (len(tail['output'].encode()), tail.get('clipped'))

    page.locator('#a-view-switch').click()
    page.locator(f'#session-view-menu [data-agent="{CHILD}"]').click()
    if page.locator('#a-sdk').get_attribute('aria-pressed') != 'true':
        page.locator('#a-sdk').click()
    start_sdk(page)
    assert rows(page, [LATE])[0]['command'] == 'different child command'
    assert read('background_output', CHILD, task=FOREGROUND) is None
    fake.native(fake.delta(THREAD, LATE, 'root continues independently\n'))
    fake.native(fake.delta(CHILD, LATE, 'only child output\n'))
    page.wait_for_function('CodexSdkView.es?.testDelta === "only child output\\n"')
    assert read('background_output', CHILD, task=LATE)['output'] == 'only child output\n'

    page.locator('#a-view-switch').click()
    page.locator('#session-view-menu [data-agent=""]').click()
    start_sdk(page)
    rows(page, [LATE, FOREGROUND])
    page.reload(wait_until='domcontentloaded')
    start_sdk(page)
    rows(page, [LATE, FOREGROUND])
    assert read('background_output', task=LATE)['output'].endswith('root continues independently\n')
    assert 'only child output' not in read('background_output', task=LATE)['output']

    page.evaluate('window.previousSdkStream = CodexSdkView.es')
    fake.disconnect()
    page.wait_for_function('CodexSdkView.es !== previousSdkStream && CodexSdkView.connected')
    rows(page, [LATE, FOREGROUND])
    recovered = read('background_output', task=LATE)
    assert recovered['output'] == '' and recovered['clipped'], recovered
    fake.native(fake.delta(THREAD, LATE, 'after reconnect\n'))
    page.wait_for_function('CodexSdkView.es?.testDelta === "after reconnect\\n"')
    assert read('background_output', task=LATE)['output'] == 'after reconnect\n'
    fake.terminals[THREAD] = []
    rows(page, [])
    expect(footer.get_by_role('button', name='后台任务', exact=False)).to_have_count(0)
    assert read('background_output', task=LATE) is None

    # A new channel negotiates method support independently. The old daemon
    # path returns no tasks and keeps the ordinary SDK connection usable.
    fake.supports_background = False
    page.evaluate('window.previousSdkStream = CodexSdkView.es')
    fake.disconnect()
    page.wait_for_function('CodexSdkView.es !== previousSdkStream && CodexSdkView.connected')
    rows(page, [])
    expect(page.locator('#sdk-model')).to_have_value('fake-sol')
    assert read('background_tasks')['background_commands'] == []
    for method in ('turn/start', 'thread/shellCommand', 'command/exec/write',
                   'thread/backgroundTerminals/terminate', 'thread/backgroundTerminals/clean'):
        assert not fake.methods(method), fake.methods(method)
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        try:
            for hub_mode, width, theme in [(False, 1280, 'light'), (True, 390, 'dark')]:
                with tempfile.TemporaryDirectory(prefix='sd-codex-terminals-') as tmp:
                    root = Path(tmp)
                    corpus = build(root)
                    add_children(corpus)
                    port = free_port()
                    env = node_env(root, port, '127.0.0.0/8') if hub_mode else None
                    with BackgroundServer(root / 'app.sock', corpus) as fake, server(
                        corpus, args.binary, root / 'app.sock', env
                    ) as base:
                        hub = None
                        if hub_mode:
                            hub_root = root / 'hub'
                            hub_root.mkdir()
                            hub = Hub(args.binary.with_name('sessiondock-hub'), hub_root, [
                                SimpleNamespace(name='CodexNode', port=port, token=TOKEN),
                            ])
                            hub.start()
                            base = f'http://127.0.0.1:{hub.port}'
                        try:
                            exercise(browser, base, fake, width, theme, args.screenshots,
                                     'hub-390-dark' if hub_mode else 'node-1280-light')
                        finally:
                            if hub:
                                hub.stop()
        finally:
            browser.close()
    print('PASS Codex native background terminals: scoped list, bounded output, foreground, reconnect, old daemon')


if __name__ == '__main__':
    main()
