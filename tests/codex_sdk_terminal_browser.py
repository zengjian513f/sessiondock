#!/usr/bin/env python3
"""A terminal joins an SDK thread's daemon without replacing its running process.

Real private PTY host and browser interactions, scripted app-server, fake CLI
and synthetic /proc. Independent CLI sessions still require confirmation.
"""
import argparse
import json
import os
import re
import shlex
from pathlib import Path
import subprocess
import sys
import tempfile

from playwright.sync_api import expect, sync_playwright
from sdk_ui_fixtures import start_sdk
from codex_sdk_browser import THREAD, build
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, REPO, codex_row, codex_message, isolated_server
from new_session_model_browser import argv_lines
from private_hosts import private_hosts
from spawned_by_fixtures import proc_pid


class ProviderPeer(FakeAppServer):
    async def _handle(self, writer, message):
        method = message.get('method')
        if method in ('config/read', 'thread/read', 'thread/loaded/list'):
            result = {'config':{'model_provider':'openai'}} if method=='config/read' else ({'thread':{'id':THREAD,'modelProvider':'custom-provider'}} if method=='thread/read' else {'data':[]})
            await self.send(writer, {'id':message['id'], 'result':result})
            return
        await super()._handle(writer,message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-terminal-') as tmp:
        root = Path(tmp)
        for name in ['work', 'state', 'ledger', 'host', 'proc']:
            (root / name).mkdir(mode=0o700)
        corpus = build(root)
        for name, sid, title in [('sdk', THREAD, 'Shared SDK thread'),
                                 ('independent', 'independent-cli-thread', 'Independent CLI thread')]:
            corpus.put(name, 'codex', [codex_row('session_meta', {'id':sid, 'cwd':str(root/'work')}),
                                      codex_message('user', title)], [])
        proc = root / 'proc'
        (proc/'stat').write_text('btime 1700000000\n')
        proc_pid(proc, 400, 'codex', ['codex', 'app-server'], 1, fds={3:corpus.paths['sdk']})
        proc_pid(proc, 500, 'codex', ['codex', 'resume', 'independent-cli-thread'], 1,
                 fds={3:corpus.paths['independent']})
        log = root / 'argv.jsonl'
        socket = root/'sd-providers'/('custom-provider'.encode().hex()+'.sock')
        socket.parent.mkdir()
        shared = root/'app-server-control/app-server-control.sock'
        shared.parent.mkdir()
        config = root / 'launcher.json'
        config.write_text(json.dumps({'schema':2, 'host_binary':str(REPO/'target/debug/ptyhost'),
            'host_dir':str(root/'host'), 'profiles':[{'id':'codex-cli', 'source':'codex',
                'executable':sys.executable, 'args':[str(REPO/'tests/fake_model_cli.py')],
                'env':{'PATH':os.environ['PATH'], 'SESSIONDOCK_TEST_ARGV_LOG':str(log)}}]}))
        config.chmod(0o600)
        subprocess.run([str(args.binary.resolve()), '--initialize-lifecycle', str(root/'ledger')],
                       check=True, capture_output=True)
        server_wrapper = root/'sessiondock'
        server_wrapper.write_text('#!/bin/sh\nexport CODEX_HOME='+shlex.quote(str(root))+'\nexec '+shlex.quote(str(args.binary.resolve()))+' "$@"\n')
        server_wrapper.chmod(0o700)
        with private_hosts(root), ProviderPeer(shared, corpus.paths['sdk'], THREAD) as default_peer, ProviderPeer(socket, corpus.paths['sdk'], THREAD) as fake:
            with isolated_server(corpus, server_wrapper, state_dir=root/'state', host_dir=root/'host',
                    lifecycle_dir=root/'ledger', launcher_config=config,
                    extra_env={'SESSIONDOCK_PROC_ROOT':str(proc), 'SESSIONDOCK_CODEX_APP_SERVER':''}) as (base, _):
                with sync_playwright() as pw:
                    browser = pw.chromium.launch(headless=True)
                    try:
                        page = browser.new_page(viewport={'width':1280,'height':900})
                        errors = []
                        page.on('pageerror', lambda error: errors.append(error.stack))
                        page.goto(base, wait_until='domcontentloaded')
                        page.wait_for_function('T.listLoaded')
                        uid = corpus.uid('sdk')
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                        start_sdk(page)
                        expect(page.locator('#sdk-model')).to_have_value('fake-sol')
                        expect(page.locator('#sdk-effort')).to_have_value('high')
                        expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_class(re.compile(r'.*\blive\b.*'))
                        page.locator('#sdk-input').fill('approve before terminal attach')
                        page.locator('#sdk-send').click()
                        expect(page.locator('.sdk-request')).to_contain_text('需要批准执行命令')
                        assert not [argv for argv in argv_lines(log) if 'resume' in argv], 'SDK progress must not require launching a PTY'
                        assert not list((root / 'host').glob('*.sock')), 'SDK progress must not require a PTY host'
                        if args.screenshots:
                            page.screenshot(path=str(args.screenshots/'sdk-before-terminal.png'))
                        with page.expect_response(lambda r: '/api/term/takeover' in r.url) as response:
                            page.locator('#a-term').click()
                        receipt = response.value.json()
                        assert receipt.get('ok') and not receipt.get('needs_confirm'), receipt
                        expect(page.locator('dialog.app-popup[open]')).to_have_count(0)
                        expect(page.locator('#termpane')).to_be_visible()
                        page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
                        page.wait_for_function("Array.from({length:T.term.buffer.active.length}, (_,i)=>"
                            "T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('fake cli ready')")
                        launches = [argv for argv in argv_lines(log) if 'resume' in argv]
                        assert len(launches) == 1, launches
                        argv = launches[0]
                        assert argv[argv.index('resume')+1] == THREAD, argv
                        assert argv[argv.index('--remote')+1] == 'unix://'+str(socket), argv
                        if args.screenshots:
                            page.screenshot(path=str(args.screenshots/'shared-terminal.png'))
                        page.locator('#a-sdk').click()
                        expect(page.locator('.sdk-request')).to_contain_text('需要批准执行命令')
                        page.locator('.sdk-request').get_by_role('button', name='批准', exact=True).click()
                        expect(page.locator('.sdk-history')).to_contain_text('All done after approval')
                        assert not default_peer.methods('thread/resume'), 'terminal routed to default provider'
                        assert len(fake.methods('turn/start')) == 1
                        assert len(fake.methods('thread/resume')) == 1, fake.methods('thread/resume')
                        # The fleet snapshot misses the node, while CHECK/SSE
                        # still observes this exact managed CLI as running.
                        page.locator('#a-chat').click()
                        def omit_managed_live(route):
                            response = route.fetch()
                            data = response.json()
                            for key in ['uids', 'tmux_uids', 'sdk_uids']:
                                data[key] = [value for value in data.get(key, []) if value != uid]
                            route.fulfill(response=response, json=data)
                        page.route('**/api/live*', omit_managed_live)
                        page.reload(wait_until='domcontentloaded')
                        expect(page.locator('#msgs')).to_contain_text('All done after approval')
                        expect(page.locator('#dlive')).to_have_class(re.compile(r'\btmux\b'), timeout=15000)
                        expect(page.locator('#dlive')).to_have_class(re.compile(r'\bvisible\b'))
                        expect(page.locator(f'#side .item[data-uid="{uid}"] .item-status')).to_have_class(re.compile(r'\btmux\b'))
                        assert not page.evaluate('uid => S.live.has(uid)', uid)
                        if args.screenshots:
                            page.screenshot(path=str(args.screenshots/'managed-live-fallback.png'))
                        page.unroute('**/api/live*', omit_managed_live)
                        independent = corpus.uid('independent')
                        page.locator(f'#side .item[data-uid="{independent}"]').click()
                        with page.expect_response(lambda r: '/api/term/takeover' in r.url) as response:
                            page.locator('#a-term').click()
                        result = response.value.json()
                        assert result.get('needs_confirm') and result['pids'] == [500], result
                        popup = page.locator('dialog.app-popup[open]')
                        expect(popup).to_contain_text('接管会先停止外部运行')
                        popup.get_by_role('button', name='取消', exact=True).click()
                        assert len([argv for argv in argv_lines(log) if 'resume' in argv]) == 1
                        assert not errors, errors
                    finally:
                        browser.close()
    print('PASS SDK→PTY joins shared daemon without replacement; SDK still sends; independent CLI confirms')


if __name__ == '__main__':
    main()
