#!/usr/bin/env python3
"""Kiro first headless/ACP launches: cross-node ownership and sidebar parents."""
import argparse
from contextlib import ExitStack
from datetime import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import server
from history_fixtures import BINARY, Corpus, codex_message, codex_row
from hub_fixtures import Hub, free_port
from kiro_sdk_fixtures import KiroFixture
from node_auth_fixtures import TOKEN, node_env
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize
from spawned_by_fixtures import START, proc_pid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--with-agent', action='store_true')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-kiro-spawn-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root = Path(tmp)
        nodes = []
        for name in ('a', 'b'):
            path = root / name
            for directory in ('claude', 'codex', 'grok', 'state'):
                (path / directory).mkdir(parents=True)
            nodes.append(SimpleNamespace(root=path, name=name, port=free_port(), token=TOKEN))
        parent = Corpus(nodes[0].root)
        parent.put('owner', 'codex', [codex_row('session_meta', {
            'id':'owner', 'cwd':'/synthetic/review', 'timestamp':'2026-10-17T00:00:00Z'}),
            codex_message('user', 'Review the execution design with Kiro')], [])
        fixture = KiroFixture(nodes[1].root, binary.with_name('ptyhost'))
        stack.callback(stop_private_sdkhosts, nodes[1].root)
        initialize('--initialize-lifecycle', nodes[1].root / 'ledger', binary)
        proc_a = nodes[0].root / 'proc'; proc_a.mkdir()
        boot = int(datetime.fromisoformat('2026-10-17T23:55:00+00:00').timestamp())
        for node, proc in zip(nodes, (proc_a, fixture.proc)):
            (proc / 'stat').write_text(f'btime {boot}\n')
            (proc / 'sys/kernel/random').mkdir(parents=True)
            (proc / 'sys/kernel/random/boot_id').write_text('kiro-spawn-' + node.name)
            (proc / 'net').mkdir()
            (proc / 'net/tcp').write_text('header\n')
            (node.root / 'node-id').write_text(node.name * 32)
        START.update({100:100, 200:200, 300:300, 700:29000, 701:29100,
                      710:29000, 720:29000, 730:42000, 740:29000, 750:29000})
        proc_pid(proc_a, 100, 'codex', ['codex'], 1, fds={3:str(parent.paths['owner'])})
        proc_pid(proc_a, 200, 'ssh', ['ssh', 'remote', 'bash'], 100, fds={4:'socket:[900]'})
        (proc_a / 'net/tcp').write_text('header\n 0: 0100000A:C350 0200000A:C366 01 0 0 0 0 0 900\n')
        proc_pid(fixture.proc, 300, 'bash', ['bash'], 1,
                 env=[('SSH_CONNECTION','10.0.0.1 50000 10.0.0.2 50022')])

        cases = [(fixture.sid, 700, 'Headless review with internal ACP'),
                 ('sess-direct-log', 710, 'Headless review with direct log'),
                 ('sess-independent-acp', 720, 'Independent ACP review'),
                 ('sess-resumed', 730, 'Resumed existing review'),
                 ('sess-stale-log', 740, 'Old log is not current ownership'),
                 ('sess-ambiguous', 750, 'Shared ACP has no exclusive owner')]
        for sid, pid, title in cases:
            path = fixture.sessions / 'bucket_v3' / sid
            path.mkdir(exist_ok=True)
            fixture.write_v3_session(path, sid, str(fixture.work), title)
            (path / 'messages.jsonl').write_text('')
            native = fixture.home / '.kiro/sessions/bucket' / sid
            native.mkdir(parents=True)
            (native/'session.json').write_text(json.dumps({'dataModelVersion':1,'id':sid,
                'title':title,'workspacePaths':[str(fixture.work)],'modelId':'synthetic-model',
                'createdAt':'2026-10-18T00:00:00Z','lastModifiedAt':'2026-10-18T00:05:00Z'}))
            (native/'messages.jsonl').write_text(json.dumps({'id':'u1','timestamp':'2026-10-18T00:00:00Z',
                'payload':{'type':'user','content':'Review evidence for '+title}})+'\n')
            acp = pid in (720, 740, 750)
            argv = ['node','/private/kiro/acp-server.js'] if acp else ['kiro-cli','chat','--v3','--no-interactive','--output-format','stream-json']
            if pid == 730:
                argv += ['--resume-id', sid]
            proc_pid(fixture.proc, pid, 'node' if acp else 'kiro-cli', argv, 300)
            log_pid = pid
            if pid == 700:
                log_pid = 701
                proc_pid(fixture.proc, log_pid, 'node', ['node','/private/kiro/acp-server.js'], pid)
            log = nodes[1].root / f'.kiro/logs/{log_pid}/kiro.log'
            log.parent.mkdir(parents=True)
            log.write_text(json.dumps({'timestamp':'2026-10-17T23:00:00Z' if pid == 740 else '2026-10-18T00:03:00Z',
                'rootConversationId':sid, 'rootTurnId':'turn-1', 'message':'native turn context'})+'\n')
            if pid == 750:
                with log.open('a') as stream:
                    stream.write(json.dumps({'timestamp':'2026-10-18T00:03:01Z',
                        'rootConversationId':fixture.empty_sid, 'rootTurnId':'other-turn'})+'\n')
            (fixture.proc / str(log_pid) / 'fd/21').symlink_to(log)

        environments = []
        for node, proc in zip(nodes, (proc_a, fixture.proc)):
            env = {**node_env(node.root,node.port,'127.0.0.0/8'),
                   'SESSIONDOCK_PROC_ROOT':str(proc), 'SESSIONDOCK_STATE_DIR':str(node.root/'state'),
                   'SESSIONDOCK_NODE_ID_FILE':str(node.root/'node-id'),
                   'SESSIONDOCK_RESOURCE_AGENT_SOCKET':str(node.root/'agent.sock')}
            if node.name == 'b':
                env.update(fixture.env)
                env.update(SESSIONDOCK_NATIVE_SDKS='auto', SESSIONDOCK_LIFECYCLE_DIR=str(node.root/'ledger'),
                           SESSIONDOCK_PTYHOST_DIR=str(fixture.host_dir))
            environments.append(env)
            if args.with_agent:
                log = stack.enter_context(tempfile.TemporaryFile())
                agent = subprocess.Popen([str(binary.with_name('resource-agent')),
                    '--uid',str(os.getuid()),'--node-id-file',str(node.root/'node-id'),
                    '--socket',str(node.root/'agent.sock'),'--state',str(node.root/'agent.json'),
                    '--proc-root',str(proc),'--events','off'],stdout=log,stderr=log)
                stack.callback(lambda p=agent: (p.terminate(),p.wait(timeout=10)))
        browser = pw.chromium.launch(headless=True); stack.callback(browser.close)
        hub_root = root/'hub'; hub_root.mkdir()
        hub = None
        for restarted in (False, True):
            with ExitStack() as running:
                bases = [running.enter_context(server(node,binary,'off',env)) for node,env in zip(nodes,environments)]
                if hub is None:
                    hub = Hub(binary.with_name('sessiondock-hub'),hub_root,nodes)
                hub.start(); running.callback(hub.stop)
                page = browser.new_page(viewport={'width':1440,'height':900}); running.callback(page.close)
                page.set_default_timeout(30000)
                page.goto(f'http://127.0.0.1:{hub.port}',wait_until='domcontentloaded')
                page.wait_for_function('T.listLoaded')
                page.wait_for_function('''async sid=>{await loadSessions(true);return S.sessions.some(r=>r.sid===sid)}''',arg=fixture.sid)
                page.locator('#nest-toggle').click()
                if args.screenshots and not restarted:
                    page.screenshot(path=str(args.screenshots/f'kiro-spawn-{args.with_agent}-initial.png'),full_page=True)
                page.wait_for_function('''async sids => {await loadSessions(true);return sids.every(sid => {
                    const row=S.sessions.find(r=>r.sid===sid);return row?.nest_parent?.sid==='owner'
                      && row.nest_parent.node_id==='a'.repeat(32)})}''',arg=[r[0] for r in cases[:(2 if restarted else 3)]])
                for sid,pid,title in cases:
                    row = page.evaluate('sid=>S.sessions.find(r=>r.sid===sid)',sid)
                    if pid >= 730:
                        assert not row.get('nest_parent'), row
                    if restarted and pid == 720:
                        assert not row.get('nest_parent'), 'Explicit detach did not survive restart'
                    page.locator(f'#side .item[data-uid="{row["uid"]}"]').click()
                    expect(page.locator(f'#side .item[data-uid="{row["uid"]}"]')).to_have_attribute(
                        'data-depth', '1' if pid in ((700,710) if restarted else (700,710,720)) else '0')
                    expect(page.locator('#msgs')).to_contain_text('Review evidence for '+title)
                owner = page.evaluate('S.sessions.find(r=>r.sid==="owner").uid')
                page.locator(f'#side .item[data-uid="{owner}"]').click()
                expect(page.locator('#msgs')).to_contain_text('Review the execution design with Kiro')
                local = browser.new_page(); running.callback(local.close)
                local.goto(bases[1],wait_until='domcontentloaded')
                local.wait_for_function('T.listLoaded')
                if not restarted:
                    local.wait_for_function('''async () => {
                      const r=await (await fetch('/api/process-links')).json();
                      return [700,710,720,730].every(pid=>r.bindings.some(b=>b.process.pid===pid && b.session.source==='kiro'))
                    }''')
                    report = local.evaluate('async ()=>(await fetch("/api/process-links")).json()')
                    assert not any(b['session']['source']=='kiro' and b['process']['pid'] in (740,750) for b in report['bindings'])
                    # Observation of standalone ACP must remain outside the CLI control list.
                    acp_uid = local.evaluate('S.sessions.find(r=>r.sid==="sess-independent-acp").uid')
                    local.locator(f'#side .item[data-uid="{acp_uid}"]').click()
                    expect(local.locator('#sdk-composer .sdk-status')).to_contain_text('外部只读')
                    expect(local.locator('#sdk-input')).to_be_disabled()
                    assert not fixture.received('session/load')
                    for width,theme in ((1440,'light'),(390,'dark')):
                        page.set_viewport_size({'width':width,'height':900});page.emulate_media(color_scheme=theme)
                        if args.screenshots:
                            page.screenshot(path=str(args.screenshots/f'kiro-spawn-{args.with_agent}-{width}-{theme}.png'),full_page=True)
                    page.set_viewport_size({'width':1440,'height':900})
                    detached = page.evaluate('S.sessions.find(r=>r.sid==="sess-independent-acp").uid')
                    page.locator(f'#side .item[data-uid="{detached}"]').click(button='right')
                    page.locator('#item-menu [data-act="detach"]').click()
                    page.wait_for_function('uid=>!S.sessions.find(r=>r.uid===uid).nest_parent',arg=detached)
                    local.evaluate('async ()=>(await fetch("/api/process-links")).json()')
                    page.evaluate('loadSessions(true)')
                    assert not page.evaluate('uid=>S.sessions.find(r=>r.uid===uid).nest_parent',detached)
                    # Exit before restart: the saved relationship must survive without a live process.
                    for pid in (700,701,710,720,730,740,750):
                        shutil.rmtree(fixture.proc/str(pid))
                print(f'PASS Kiro cross-node first headless/ACP ownership; collector={args.with_agent}; restarted={restarted}',flush=True)


if __name__ == '__main__':
    main()
