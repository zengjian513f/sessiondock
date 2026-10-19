#!/usr/bin/env python3
"""Middle-message fork and exact thread/PTY stop on one shared daemon."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from codex_sdk_actions_browser import build, environment, THREAD, TURN2
from codex_sdk_message_actions_browser import MessageServer, FORK
from codex_sdk_browser import server
from history_fixtures import BINARY, codex_row, codex_message, encoded
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from private_hosts import private_hosts
from popups import on_popup
from spawned_by_fixtures import proc_pid

OTHER = '01a1adc0-0000-7000-8000-000000000098'
LATER = '分支点之后的第三轮 Later question'


class StopServer(MessageServer):
    def __init__(self, socket, source, retained, other, case="normal"):
        super().__init__(socket, source, retained)
        self.paths[OTHER] = other
        self.loaded = set() if case == 'normal' else {THREAD, OTHER}
        self.bad_history = set() if case == 'normal' else {THREAD}
        self.unloaded = {THREAD} if case == 'unloaded' else set()
        self.active = {THREAD} if case == 'unreadable' else set()
        self.fail_interrupt = False

    async def _handle(self, writer, message):
        method, params = message.get('method'), message.get('params') or {}
        if method == 'thread/loaded/list':
            await self.send(writer, {'id': message['id'], 'result': {'data': sorted(self.loaded)}})
            return
        if method == 'thread/resume':
            if params['threadId'] in self.bad_history:
                await self.send(writer, {'id': message['id'], 'error': {'code': -32602,
                    'message': f"invalid paginated history lineage for {params['threadId']}: missing source rollout"}})
                return
            self.loaded.add(params['threadId'])
        if method == 'turn/interrupt':
            assert params['turnId'] == '', params
            if params['threadId'] in self.unloaded or self.fail_interrupt:
                await self.send(writer, {'id': message['id'], 'error': {'code': -32602,
                    'message': 'thread not loaded' if params['threadId'] in self.unloaded else 'synthetic interrupt failure'}})
            else:
                self.active.discard(params['threadId'])
                await self.send(writer, {'id': message['id'], 'result': {}})
            return
        if method == 'thread/unsubscribe':
            # Keep loaded/list deliberately stale: a native daemon may retain
            # an unsubscribed thread during its unload grace period.
            await self.send(writer, {'id': message['id'], 'result': {'status': 'notLoaded' if params['threadId'] in self.unloaded else 'unsubscribed'}})
            return
        await super()._handle(writer, message)
        if method == 'thread/fork':
            self.loaded.add(FORK)
            meta = json.loads(self.fork_path.read_text())
            meta['payload']['cwd'] = json.loads(self.source_path.read_text().splitlines()[0])['payload']['cwd']
            self.fork_path.write_bytes(encoded(meta))


def exercise(browser, base, corpus, fake, proc, screenshots, label):
    context = browser.new_context(viewport={'width': 1280 if 'wide' in label else 390, 'height': 900},
                                  color_scheme='light' if 'light' in label else 'dark', service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    on_popup(page, lambda dialog: dialog.accept())
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='networkidle')
    page.locator('#side .item').filter(has_text='第一轮 retained question').first.click()
    page.wait_for_function('CodexSdkView.connected && !!CodexSdkView.historyData')
    source = page.evaluate('S.sel')
    bubble = page.locator('.sdk-history .msg[data-role="user"]').filter(has_text='第二轮 remove this question')
    bubble.get_by_role('button', name='消息操作', exact=True).click()
    bubble.get_by_role('button', name='分支', exact=True).click()
    expect(page.locator('#sdk-input')).to_have_value('第二轮 remove this question')
    branch = page.evaluate('S.sel')
    expect(page.locator('.sdk-history')).not_to_contain_text(LATER)
    expect(page.locator('.sdk-history')).not_to_contain_text('remove this answer')
    expect(page.locator('#side .item')).to_have_count(2)
    # One shared process holds every native file. It is not a CLI instance
    # owned by the deepest fork and must never receive process termination.
    for fd, path in [(4, fake.fork_path), (5, fake.paths[FORK]), (6, fake.paths[OTHER])]:
        (proc / '400' / 'fd' / str(fd)).symlink_to(path)
    if 'narrow' in label: page.get_by_role('button', name='返回会话列表', exact=True).click()
    page.locator(f'#side .item[data-uid="{source}"]').click()
    expect(page.locator('.sdk-history')).to_contain_text(LATER)
    # Joining a real private PTY adds only this source's guarded client.
    with page.expect_response(lambda response: '/api/term/takeover' in response.url) as takeover:
        page.locator('#a-term').click()
    result = takeover.value.json()
    assert takeover.value.status == 200 and not result.get('needs_confirm'), result
    try:
        page.wait_for_function('uid => T.list.some(row => row.uid === uid && row.instance_id)', arg=source, timeout=7000)
    except Exception as error:
        raise AssertionError({'takeover': result, 'list': page.evaluate('T.list'), 'source': source}) from error
    page.locator('#a-sdk').click()
    page.wait_for_function('CodexSdkView.connected')
    parent_terminal = page.evaluate('T.list.find(row => row.uid === S.sel).name')
    # BUG-20261019-111304-7ce045: an independent middle-message fork must
    # not jump back to its visible parent's PTY and reveal the parent's tail.
    if 'narrow' in label: page.get_by_role('button', name='返回会话列表', exact=True).click()
    page.locator(f'#side .item[data-uid="{branch}"]').click()
    page.wait_for_function('CodexSdkView.connected && !!CodexSdkView.historyData')
    expect(page.locator('.sdk-history')).not_to_contain_text(LATER)
    if screenshots: page.screenshot(path=str(screenshots / f'{label}-branch-before-terminal.png'))
    page.locator('#a-term').click()
    page.wait_for_function('!ConsoleUI.busy.size')
    assert page.evaluate('S.sel') == branch, 'terminal switched the independent fork to its parent'
    try:
        page.wait_for_function('T.ws?.readyState === WebSocket.OPEN', timeout=10000)
    except Exception as error:
        raise AssertionError(page.evaluate('({selected:S.sel,uid:T.uid,name:T.name,list:T.list,errors:[...ConsoleUI.errors]})')) from error
    assert page.evaluate('T.name') != parent_terminal
    page.wait_for_function('uid => T.list.some(row => row.uid === uid && row.instance_id)', arg=branch)
    assert page.evaluate('T.uid') == branch
    assert page.evaluate('name => T.list.some(row => row.name === name)', parent_terminal)
    page.locator('#a-sdk').click()
    page.wait_for_function('CodexSdkView.connected && !!CodexSdkView.historyData')
    expect(page.locator('.sdk-history')).not_to_contain_text(LATER)
    expect(page.locator('.sdk-history')).not_to_contain_text('remove this answer')
    expect(page.locator('#sdk-input')).to_have_value('第二轮 remove this question')
    assert page.evaluate('S.sel') == branch
    if screenshots: page.screenshot(path=str(screenshots / f'{label}-branch-after-terminal.png'))
    # Opening the parent explicitly still shows its complete, separate history.
    if 'narrow' in label: page.get_by_role('button', name='返回会话列表', exact=True).click()
    page.locator(f'#side .item[data-uid="{source}"]').click()
    page.wait_for_function('CodexSdkView.connected && !!CodexSdkView.historyData')
    expect(page.locator('.sdk-history')).to_contain_text(LATER)
    action = page.locator('#a-session-toggle')
    if not action.is_visible(): page.locator('#a-more').click()
    with page.expect_response(lambda response: response.url.endswith('/api/session/stop')) as stopped:
        action.click()
    result = stopped.value.json()
    assert stopped.value.status == 200 and result['stopped'], result
    assert fake.methods('thread/unsubscribe') and fake.methods('thread/unsubscribe')[-1]['params'] == {'threadId': THREAD}, {
        'stop': result, 'rpc': fake.received[-12:],
        'view': page.evaluate('({uid:CodexSdkView.uid,connected:CodexSdkView.connected,stopped:CodexSdkView.stopped,sel:S.sel})')}
    expect(page.locator('.sdk-pane .sdk-status')).to_contain_text('会话已停止')
    assert page.evaluate('S.sel') == source
    page.wait_for_function('uid => !T.list.some(row => row.uid === uid && row.instance_id && !row.stale)', arg=source)
    assert fake.methods('thread/unsubscribe')[-1]['params'] == {'threadId': THREAD}
    live = page.request.get(base + '/api/live?force=1').json()
    assert source not in live['uids'] and branch in live['sdk_uids'], live
    assert not errors, errors
    if screenshots: page.screenshot(path=str(screenshots / f'{label}-parent-stopped.png'))
    # The branch stays usable, then stopping it unsubscribes only its thread.
    if 'narrow' in label: page.get_by_role('button', name='返回会话列表', exact=True).click()
    page.locator(f'#side .item[data-uid="{branch}"]').click()
    page.wait_for_function('CodexSdkView.connected')
    action = page.locator('#a-session-toggle')
    if not action.is_visible(): page.locator('#a-more').click()
    with page.expect_response(lambda response: response.url.endswith('/api/session/stop')) as stopped:
        action.click()
    result = stopped.value.json()
    assert stopped.value.status == 200 and result['stopped'], result
    expect(page.locator('.sdk-pane .sdk-status')).to_contain_text('会话已停止')
    assert fake.methods('thread/unsubscribe')[-1]['params'] == {'threadId': FORK}
    live = page.request.get(base + '/api/live?force=1').json()
    assert branch not in live['uids'] and source not in live['uids'], live
    expect(page.locator('#side .item')).to_have_count(2)
    if screenshots: page.screenshot(path=str(screenshots / f'{label}-branch-stopped.png'))
    context.close()
    print('PASS ' + label + ': middle fork excludes later messages; exact parent PTY/SDK stop; independent branch stop; daemon retained', flush=True)


def unreadable_history(browser, base, fake, screenshots, label, case):
    context = browser.new_context(viewport={'width': 390 if 'narrow' in label else 1280, 'height': 900},
                                  color_scheme='dark' if 'dark' in label else 'light', service_workers='block')
    context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
    page = context.new_page()
    popups = []
    on_popup(page, lambda d: (popups.append(d.message), d.accept()))
    page.goto(base, wait_until='networkidle')
    source, other = page.evaluate("ids => ids.map(id => S.sessions.find(row => row.sid === id).uid)", [THREAD, OTHER])
    with page.expect_response(lambda r: r.url.endswith('/api/codex-sdk/open')) as opened:
        page.locator(f'#side .item[data-uid="{source}"]').click()
    assert opened.value.status != 200 and 'missing source rollout' in opened.value.text()
    page.wait_for_function('!CodexSdkView.connected')
    expect(page.locator('.sdk-history')).to_contain_text('retained answer')
    resumes = len(fake.methods('thread/resume'))
    def stop():
        # Exercise the same sidebar menu as the reported failure.
        if 'narrow' in label and page.locator('.mobile-back').first.is_visible():
            page.locator('.mobile-back').first.click()
        page.locator(f'#side .item[data-uid="{source}"]').click(button='right')
        with page.expect_response(lambda r: r.url.endswith('/api/session/stop')) as reply:
            page.locator('#item-menu [data-act="stop"]').click()
        return reply.value
    if case == 'unreadable':
        fake.fail_interrupt = True
        failed = stop()
        assert failed.status != 200 and 'synthetic interrupt failure' in failed.text(), failed.text()
        expect(page.locator('dialog.app-popup[open]')).to_have_count(0)
        live = page.request.get(base + '/api/live?force=1').json()
        assert source in live['sdk_uids'], live
        assert any('synthetic interrupt failure' in message for message in popups), popups
        fake.fail_interrupt = False
    stopped = stop()
    assert stopped.status == 200 and stopped.json()['stopped'], stopped.text()
    assert len(fake.methods('thread/resume')) == resumes, fake.received
    assert fake.methods('turn/interrupt')[-1]['params'] == {'threadId': THREAD, 'turnId': ''}
    assert fake.methods('thread/unsubscribe')[-1]['params'] == {'threadId': THREAD}
    assert THREAD not in fake.active
    live = page.request.get(base + '/api/live?force=1').json()
    assert source not in live['uids'] and other in live['sdk_uids'], live
    if 'narrow' in label:
        # Returning to the mobile list closes the SDK view. Its sidebar action
        # must offer start after a stop, without reopening unreadable history.
        page.locator(f'#side .item[data-uid="{source}"]').click(button='right')
        expect(page.locator('#item-menu [data-act="stop"]')).to_have_text('启动会话')
    else:
        expect(page.locator('.sdk-pane .sdk-status')).to_contain_text('会话已停止')
        expect(page.locator('.sdk-pane #sdk-send')).to_be_disabled()
    assert not any(m.get('method') in ('turn/interrupt','thread/unsubscribe') and
                   m.get('params',{}).get('threadId') == OTHER for m in fake.received)
    if screenshots: page.screenshot(path=str(screenshots / f'{label}-stopped.png'))
    if 'narrow' in label: page.keyboard.press('Escape')
    # A failed explicit reopen cannot revive the daemon's stale loaded entry.
    with page.expect_response(lambda r: r.url.endswith('/api/codex-sdk/open')) as reopened:
        page.locator(f'#side .item[data-uid="{source}"]').click()
    assert reopened.value.status != 200, reopened.value.text()
    live = page.request.get(base + '/api/live?force=1').json()
    assert source not in live['uids'] and other in live['sdk_uids'], live
    context.close()
    print(f'PASS {label}: unreadable history cannot block stop; no resume, exact interrupt/unsubscribe, unrelated thread retained; {case}', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    parser.add_argument('--case', action='append', choices=('normal','unreadable','unloaded'))
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-stop-') as temporary, sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        try:
            for mode, size, theme, case in [(m,s,t,c) for m,s,t in [('node','wide','light'),('hub','wide','light'),('hub','narrow','dark')] for c in (args.case or ('normal','unreadable','unloaded'))]:
                label = f'{mode}-{size}-{theme}-{case}'
                root = Path(temporary) / label
                root.mkdir()
                corpus, _ = build(root)
                path = corpus.paths['sdk']
                rows = [json.loads(line) for line in path.read_text().splitlines()]
                (root / 'work').mkdir()
                rows[0]['payload']['cwd'] = str(root / 'work')
                turn = None
                for row in rows:
                    if row['type'] == 'turn_context': turn = row['payload']['turn_id']
                    elif row['type'] == 'response_item': row['payload']['turn_id'] = turn
                rows += [codex_row('turn_context', {'turn_id': 'third-turn'}), codex_message('user', LATER)]
                for ordinal, row in enumerate(rows): row['ordinal'] = ordinal
                path.write_bytes(b''.join(encoded(row) for row in rows))
                stop = next(i for i, row in enumerate(rows) if row.get('type') == 'turn_context' and row['payload']['turn_id'] == TURN2)
                retained = b''.join(encoded(row) for row in rows[:stop])
                # An unrelated thread on the same daemon need not be listed.
                other = root / 'other.jsonl'
                other.write_bytes(encoded(codex_row('session_meta', {'id': OTHER})))
                if case != 'normal':
                    other = corpus.put('other', 'codex', [codex_row('session_meta', {'id': OTHER}),
                        codex_message('user', 'Unrelated loaded thread')], [])
                env = environment(root, args.binary)
                config = root / 'launcher.json'
                settings = json.loads(config.read_text())
                settings['profiles'][0]['env'] = {'PATH': os.environ['PATH'],
                    'SESSIONDOCK_TEST_ARGV_LOG': str(root / 'argv.jsonl')}
                config.write_text(json.dumps(settings))
                proc = root / 'proc'
                (proc / 'stat').write_text('btime 1700000000\n')
                proc_pid(proc, 400, 'codex', ['codex', 'app-server'], 1, fds={3: path})
                port = free_port()
                env.update(node_env(root, port, '127.0.0.0/8'))
                with private_hosts(root), StopServer(root / 'app.sock', path, retained, other, case) as fake, server(corpus, args.binary, root / 'app.sock', env) as base:
                    if mode == 'hub':
                        hub_root = root / 'hub'
                        hub_root.mkdir()
                        hub = Hub(args.binary.with_name('sessiondock-hub').resolve(), hub_root,
                                  [SimpleNamespace(name='StopNode', token=TOKEN, port=port)])
                        hub.start()
                        try:
                            if case == 'normal': exercise(browser, f'http://127.0.0.1:{hub.port}', corpus, fake, proc, args.screenshots, label)
                            else: unreadable_history(browser, f'http://127.0.0.1:{hub.port}', fake, args.screenshots, label, case)
                        finally: hub.stop()
                    elif case == 'normal': exercise(browser, base, corpus, fake, proc, args.screenshots, label)
                    else: unreadable_history(browser, base, fake, args.screenshots, label, case)
        finally: browser.close()


if __name__ == '__main__': main()
