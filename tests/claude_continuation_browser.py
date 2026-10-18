#!/usr/bin/env python3
"""Claude continued-in follows the same PTY through a compact hidden-child catalog.

Private native-shaped records and a synthetic TUI; no model CLI or production data.
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
from history_fixtures import BINARY, REPO, Corpus, claude_row, encoded, get_json, isolated_server
from host_identity import request
from hub_fixtures import Hub, free_port, scoped
from private_hosts import private_hosts
from terminal_browser import stop
from send_browser import initialize
from codex_empty_rewind_browser import GRID

OLD = '10000000-0000-4000-8000-000000000001'
NEW = '10000000-0000-4000-8000-000000000002'
FAKE = r'''import json, os, pathlib, sys, tty
old, new, old_sid, new_sid = map(str, sys.argv[1:])
text = ''
def output(value):
    os.write(1, (value + '\r\n').encode())
tty.setraw(0)
output('RS_READY')
while True:
    char = os.read(0, 1)
    if not char:
        break
    if char in (b'\r', b'\n'):
        if text == 'continue':
            row = {'type':'continued-in', 'sessionId':old_sid,
                   'continuedInSessionId':new_sid, 'timestamp':'2026-09-11T10:01:00Z'}
            with open(old, 'a') as stream:
                stream.write(json.dumps(row) + '\n')
            output('RS_CONTINUED: New continuation answer 最新会话内容')
        elif text:
            row = {'type':'user', 'uuid':text, 'parentUuid':'new-answer', 'sessionId':new_sid,
                   'cwd':'/synthetic/history', 'timestamp':'2026-09-11T10:02:00Z',
                   'message':{'role':'user','content':text}}
            with open(new, 'a') as stream:
                stream.write(json.dumps(row) + '\n')
            output('RS_INPUT_OK ' + text)
        text = ''
    else:
        text += char.decode('ascii')
'''


def check(browser, binary, output, hub_mode, width, theme):
    label = f"{'hub' if hub_mode else 'node'}-{width}-{theme}"
    with tempfile.TemporaryDirectory(prefix='sessiondock-claude-continuation-') as temporary, private_hosts(Path(temporary)):
        root = Path(temporary)
        corpus = Corpus(root)
        for source in ('claude', 'codex', 'grok'):
            (root / source).mkdir()
        old = corpus.put(OLD, 'claude', [claude_row(OLD, 'user', 'old-input', None,
            'Old question 续接前的长标题 /synthetic/history — conversation'),
            claude_row(OLD, 'assistant', 'old-answer', 'old-input', 'Old answer 旧会话内容')], [])
        new = corpus.put(NEW, 'claude', [claude_row(NEW, 'user', 'new-input', None,
            'New continuation question 续接后的问题'),
            claude_row(NEW, 'assistant', 'new-answer', 'new-input', 'New continuation answer 最新会话内容')], [])
        old_uid, new_uid = corpus.uid(OLD), corpus.uid(NEW)
        state = root / 'state'
        state.mkdir(mode=0o700)
        saved = state / 'session-metadata.json'
        # An automatic link can be committed before continued-in is discovered.
        metadata = {'schema_version':1, 'revision':1, 'sessions': {
            new_uid:{'nest_parent':{'source':'claude','sid':OLD}, 'nest_initialized':True}}}
        saved.write_text(json.dumps(metadata))
        saved.chmod(0o600)
        script = root / 'fake.py'
        script.write_text(FAKE)
        (root/'work').mkdir()
        (root/'proc').mkdir()
        (root/'proc'/'stat').write_text('btime 1700000000\n')
        (root/'ledger').mkdir(mode=0o700)
        initialize('--initialize-lifecycle', root/'ledger', binary)
        launcher = root/'launcher.json'
        launcher.touch(mode=0o600)
        launcher.write_text(json.dumps({'schema':2, 'host_binary':str(REPO/'target/debug/ptyhost'),
            'host_dir':str(root/'host'), 'adapters':[], 'profiles':[]}))
        instance, name = uuid.uuid4().hex, 'synthetic-claude-continuation'
        process = subprocess.Popen([str(REPO / 'target/debug/ptyhost'), '--dir', str(root / 'host'),
            'run', '--name', name, '--cwd', str(root), '--cols', '100', '--rows', '30',
            '--meta', json.dumps({'source':'claude','sid':OLD,'uid':old_uid,'instance_id':instance}),
            '--', sys.executable, str(script), str(old), str(new), OLD, NEW],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        hub, context = None, None
        try:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                assert process.poll() is None
                try:
                    record = json.loads((root / 'host' / (name + '.json')).read_text())
                    if 'RS_READY' in request(record, {'op':'capture','styled':False}).get('text',''):
                        break
                except (OSError, json.JSONDecodeError):
                    pass
                time.sleep(.03)
            else:
                raise AssertionError('synthetic TUI did not start')
            node = SimpleNamespace(nid='a'*32, name='ContinuationNode', port=free_port(), token='f'*64)
            extra = {'SESSIONDOCK_PROC_ROOT':str(root/'proc')}
            if hub_mode:
                for key, value in [('node-id', node.nid), ('node-token', node.token)]:
                    (root / key).touch(mode=0o600)
                    (root / key).write_text(value)
                extra.update({'SESSIONDOCK_NODE_BIND':f'127.0.0.1:{node.port}',
                    'SESSIONDOCK_NODE_ID_FILE':str(root/'node-id'),
                    'SESSIONDOCK_NODE_TOKEN_FILE':str(root/'node-token'), 'SESSIONDOCK_NODE_PEERS':'127.0.0.0/8'})
            view_uid = lambda uid: scoped(node.nid, uid) if hub_mode else uid
            with isolated_server(corpus, binary, host_dir=root/'host', state_dir=state,
                                 lifecycle_dir=root/'ledger', launcher_config=launcher,
                                 file_roots=(root/'work',), file_write_roots=(root/'work',),
                                 extra_env=extra) as (base, opener):
                if hub_mode:
                    (root/'hub').mkdir()
                    hub = Hub(REPO/'target/debug/sessiondock-hub', root/'hub', [node])
                    hub.start()
                    base = f'http://127.0.0.1:{hub.port}'
                context = browser.new_context(viewport={'width':width,'height':900}, color_scheme=theme,
                                              service_workers='block')
                context.add_init_script("localStorage.setItem('sessiondock.childMode', JSON.stringify('hidden'))")
                context.route('**/*', lambda route: route.continue_()
                              if route.request.url.startswith(base+'/') else route.abort())
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(base, wait_until='networkidle')
                page.locator(f'#side .item[data-uid="{view_uid(old_uid)}"]').click()
                expect(page.locator('#msgs')).to_contain_text('Old answer')
                page.locator('#cinput').fill('保留未发送草稿 continuation draft')
                page.screenshot(path=str(output / (label+'-before.png')))
                page.locator('#a-term').click()
                page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
                page.wait_for_function(GRID, arg='RS_READY')
                initial = page.evaluate('({name:T.name, instance:T.views.get(T.name).instanceId})')
                page.evaluate('window.continuationSocket = T.ws')
                terminal = page.locator('#termpane .xterm-helper-textarea')
                terminal.press_sequentially('continue')
                terminal.press('Enter')
                page.wait_for_function(GRID, arg='RS_CONTINUED')
                # Exercise signature polling against the real compact catalog.
                context.request.get(base+'/api/sessions?force=1')
                page.evaluate('async () => { await pollSessions(); await loadTermList(); }')
                page.wait_for_function('uid => S.sel === uid && T.uid === uid', arg=view_uid(new_uid), timeout=10000)
                assert page.evaluate('T.ws === continuationSocket')
                assert page.evaluate('({name:T.name, instance:T.views.get(T.name).instanceId})') == initial
                expect(page.locator('#a-term')).to_have_attribute('aria-pressed','true')
                page.locator('#a-chat').click()
                expect(page.locator('#msgs')).to_contain_text('New continuation answer')
                expect(page.locator('#cinput')).to_have_value('保留未发送草稿 continuation draft')
                page.screenshot(path=str(output / (label+'-followed.png')))
                page.locator('#a-term').click()
                terminal.press_sequentially('after-continuation')
                terminal.press('Enter')
                page.wait_for_function(GRID, arg='RS_INPUT_OK after-continuation')
                page.locator('#a-chat').click()
                expect(page.locator('#msgs')).to_contain_text('after-continuation')
                # Existing old-SID bookmarks resolve the continuation on reload.
                query = '?sid=claude:' + OLD + ('&node='+node.nid if hub_mode else '')
                page.goto(base+'/'+query, wait_until='networkidle')
                expect(page.locator('#msgs')).to_contain_text('New continuation answer')
                page.wait_for_function('uid => S.sel === uid', arg=view_uid(new_uid))
                page.reload(wait_until='networkidle')
                expect(page.locator('#msgs')).to_contain_text('after-continuation')
                page.locator('#a-term').click()
                page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
                assert page.evaluate('({name:T.name, instance:T.views.get(T.name).instanceId})') == initial
                page.locator('#termpane .xterm-helper-textarea').press_sequentially('after-reload')
                page.locator('#termpane .xterm-helper-textarea').press('Enter')
                page.wait_for_function(GRID, arg='RS_INPUT_OK after-reload')
                assert json.loads(saved.read_text())['sessions'][new_uid] == metadata['sessions'][new_uid]
                assert not errors, errors
                print('PASS Claude continuation',label,'live history, same socket/guard, input, old link, reload, saved relationship preserved',flush=True)
                context.close()
                context = None
                if hub:
                    hub.stop()
                    hub = None
        finally:
            if context:
                context.close()
            if hub:
                hub.stop()
            stop(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--output',type=Path,default=Path(tempfile.gettempdir())/'sessiondock-claude-continuation-screens')
    parser.add_argument('--case',choices=('first','all'),default='all')
    args = parser.parse_args()
    args.output.mkdir(parents=True,exist_ok=True)
    with sync_playwright() as playwright:
        options = {'headless':True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = playwright.chromium.launch(**options)
        try:
            cases = [(False,1280,'light')] if args.case == 'first' else [
                (hub,width,theme) for hub in (False,True) for width in (1280,390) for theme in ('light','dark')]
            for case in cases:
                check(browser,args.binary,args.output,*case)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
