#!/usr/bin/env python3
"""Double Esc before the first prompt keeps a same-thread console usable.

Only a private synthetic TUI writes native-shaped rollouts; no model CLI runs.
The reset has no history_base and ordinal zero, as Codex 0.163.0 writes it.
"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, REPO, Corpus, codex_row, codex_message, isolated_server
from host_identity import request
from hub_fixtures import Hub, free_port, scoped
from private_hosts import private_hosts
from terminal_browser import stop


FAKE = r'''
import json, os, pathlib, sys, tty
root, sid = pathlib.Path(sys.argv[1]), sys.argv[2]
generation, escapes, menu, text = 0, 0, False, ''
path = root / ('rollout-2026-09-11T08-00-00-' + sid + '.jsonl')
def output(value):
    os.write(1, (value + '\r\n').encode())
tty.setraw(0)
output('RS_READY: 双 Esc 回退到第一条之前')
while True:
    part = os.read(0, 1)
    if not part:
        break
    if part == b'\x1b':
        escapes += 1
        if escapes == 2:
            menu = True
            output('Revert to first prompt: Enter to confirm')
        continue
    if part in (b'\r', b'\n'):
        if menu:
            generation += 1
            rollout = '00000000-0000-7000-8000-' + f'{generation + 1:012d}'
            stamp = f'2026-09-11T{8 + generation:02d}:00:00Z'
            path = root / ('rollout-' + stamp[:-1].replace(':', '-') + '-' + sid + '_' + rollout + '.jsonl')
            meta = {'id': sid, 'session_id': sid, 'cwd': str(root),
                    'timestamp': stamp, 'history_mode': 'paginated'}
            if generation == 2:
                meta['history_base'] = None
            rows = [{'type': 'session_meta', 'ordinal': 0, 'timestamp': stamp, 'payload': meta},
                    {'type': 'event_msg', 'ordinal': 1, 'timestamp': stamp,
                     'payload': {'type': 'thread_settings_applied'}}]
            path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
            output('Conversation reverted to this point.')
            output('RS_REVERT_' + str(generation))
        elif text:
            with path.open('a') as stream:
                stream.write(json.dumps({'type': 'response_item', 'ordinal': 2, 'payload': {
                    'type': 'message', 'role': 'user', 'content': [{'type': 'input_text', 'text': text}]}}) + '\n')
            output('RS_INPUT_OK ' + text)
        text, escapes, menu = '', 0, False
    else:
        text += part.decode('ascii')
'''

GRID = r"""text => {
  const buffer = T.term?.buffer.active, lines = [];
  for (let i = 0; i < (buffer?.length || 0); i++) {
    const line = buffer.getLine(i), value = line?.translateToString() || '';
    if (line?.isWrapped && lines.length) lines[lines.length - 1] += value;
    else lines.push(value);
  }
  return lines.join('\n').includes(text);
}"""


def check(browser, binary, output, hub_mode, width, theme):
    label = f"{'hub' if hub_mode else 'node'}-{width}-{theme}"
    with tempfile.TemporaryDirectory(prefix="sessiondock-empty-rewind-") as temporary, private_hosts(Path(temporary)):
        root = Path(temporary)
        for name in ('host', 'work', 'claude', 'codex', 'grok', 'state', 'hub'):
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        sid = '00000000-0000-7000-8000-000000000001'
        original = corpus.put('original', 'codex', [codex_row('session_meta', {
            'id': sid, 'session_id': sid, 'cwd': str(root / 'work'),
            'timestamp': '2026-09-11T08:00:00Z', 'history_mode': 'paginated'}),
            codex_message('user', '原始第一条：回退后不应继承 Original first prompt', 1)], [])
        native = original.with_name(f'rollout-2026-09-11T08-00-00-{sid}.jsonl')
        original.rename(native)
        corpus.paths['original'] = native
        uid = corpus.uid('original')
        guard_uid = uid
        script = root / 'fake.py'
        script.write_text(FAKE)
        instance, name = uuid.uuid4().hex, 'synthetic-empty-rewind'
        process = subprocess.Popen([str(REPO / 'target/debug/ptyhost'), '--dir', str(root / 'host'),
            'run', '--name', name, '--cwd', str(root / 'work'), '--cols', '100', '--rows', '30',
            '--meta', json.dumps({'source': 'codex', 'sid': sid, 'uid': uid, 'instance_id': instance}),
            '--', sys.executable, str(script), str(native.parent), sid],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        hub = None
        context = None
        try:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                assert process.poll() is None
                try:
                    record = json.loads((root / 'host' / (name + '.json')).read_text())
                    if 'RS_READY' in request(record, {'op': 'capture', 'styled': False}).get('text', ''):
                        break
                except (OSError, json.JSONDecodeError):
                    pass
                time.sleep(.03)
            else:
                raise AssertionError('synthetic TUI did not start')
            node = SimpleNamespace(nid='a' * 32, name='RewindNode', port=free_port(), token='f' * 64)
            extra = {}
            if hub_mode:
                for key, value in [('node-id', node.nid), ('node-token', node.token)]:
                    (root / key).touch(mode=0o600)
                    (root / key).write_text(value)
                extra = {'SESSIONDOCK_NODE_BIND': f'127.0.0.1:{node.port}',
                         'SESSIONDOCK_NODE_ID_FILE': str(root / 'node-id'),
                         'SESSIONDOCK_NODE_TOKEN_FILE': str(root / 'node-token'),
                         'SESSIONDOCK_NODE_PEERS': '127.0.0.0/8'}
            view_uid = lambda value: scoped(node.nid, value) if hub_mode else value
            with isolated_server(corpus, binary, host_dir=root / 'host', state_dir=root / 'state', extra_env=extra) as (base, _):
                if hub_mode:
                    hub = Hub(REPO / 'target/debug/sessiondock-hub', root / 'hub', [node])
                    hub.start()
                    base = f'http://127.0.0.1:{hub.port}'
                context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme,
                                              service_workers='block')
                context.route('**/*', lambda route: route.continue_()
                              if route.request.url.startswith(base + '/') else route.abort())
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(base, wait_until='networkidle')
                page.locator(f'#side .item[data-uid="{view_uid(uid)}"]').click()
                page.locator('#a-term').click()
                page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                page.wait_for_function(GRID, arg='RS_READY')
                page.screenshot(path=str(output / (label + '-before.png')))
                first = page.evaluate('({name:T.name, instance:T.views.get(T.name).instanceId})')
                page.evaluate('window.rewindSocket = T.ws')
                for generation in (1, 2):
                    terminal = page.locator('#termpane .xterm-helper-textarea')
                    terminal.press('Escape')
                    terminal.press('Escape')
                    page.wait_for_function(GRID, arg='Enter to confirm')
                    terminal.press('Enter')
                    page.wait_for_function(GRID, arg=f'RS_REVERT_{generation}')
                    # Refresh the real catalogs, without injecting any UI state.
                    page.evaluate('async () => { await loadSessions(true); await loadTermList(); }')
                    # Hub health probes run every ten seconds; a catalog
                    # refresh may overlap one without changing the live PTY.
                    try:
                        page.wait_for_function('old => S.sel !== old && T.uid === S.sel',
                                               arg=view_uid(uid), timeout=15000)
                    except Exception:
                        print('rebind diagnostics', page.evaluate("""() => ({selected:S.sel, uid:T.uid,
                          list:T.list, error:T.listError, nodes:HUB_MODE ? [...Nodes.errors] : []})"""), flush=True)
                        raise
                    uid = page.evaluate('S.sel')
                    if hub_mode:
                        uid = uid.replace('codex:' + node.nid + '~', 'codex:')
                    expect(page.locator('#a-term')).to_have_attribute('aria-pressed', 'true')
                    assert page.evaluate('T.ws === rewindSocket')
                    assert page.evaluate('({name:T.name, instance:T.views.get(T.name).instanceId})') == first
                    page.locator('#a-chat').click()
                    expect(page.locator('#msgs')).not_to_contain_text('Original first prompt')
                    expect(page.locator('#msgs .msg')).to_have_count(0)
                    page.locator('#a-term').click()
                    expect(page.locator('#termpane')).to_be_visible()
                    terminal.press_sequentially(f'after-rewind-{generation}')
                    terminal.press('Enter')
                    page.wait_for_function(GRID, arg=f'RS_INPUT_OK after-rewind-{generation}')
                # A fresh page must claim the original instance under the new
                # view UID, with no new CLI or immutable identity rewrite.
                page.reload(wait_until='networkidle')
                if width < 700:
                    page.locator('.mobile-back').first.click()
                page.locator(f'#side .item[data-uid="{view_uid(uid)}"]').click()
                page.locator('#a-term').click()
                page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
                terminal = page.locator('#termpane .xterm-helper-textarea')
                terminal.press_sequentially('after-reload')
                terminal.press('Enter')
                page.wait_for_function(GRID, arg='RS_INPUT_OK after-reload')
                assert page.evaluate('({name:T.name, instance:T.views.get(T.name).instanceId})') == first
                assert process.poll() is None
                listed = context.request.get(base + '/api/term/list?force=1').json()
                bound = next(row for row in listed['sessions'] if row['instance_id'] == instance)
                assert bound['uid'] == view_uid(guard_uid), bound
                assert bound['current_uid'] == view_uid(uid), bound
                assert not errors, errors
                page.screenshot(path=str(output / (label + '-after.png')))
                print(f'PASS {label}: double Esc, empty history, two resets, same socket/guard, reload/input', flush=True)
        except Exception:
            if context and context.pages:
                context.pages[0].screenshot(path=str(output / (label + '-failure.png')))
            raise
        finally:
            if context:
                context.close()
            if hub:
                hub.stop()
            stop(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path(tempfile.gettempdir()) / 'sessiondock-empty-rewind-screens')
    parser.add_argument('--case', choices=('first', 'all'), default='all')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = playwright.chromium.launch(**options)
        try:
            cases = [(False, 1280, 'light')] if args.case == 'first' else [
                (hub, width, theme) for hub in (False, True) for width in (1280, 390) for theme in ('light', 'dark')]
            for case in cases:
                check(browser, args.binary, args.output, *case)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
