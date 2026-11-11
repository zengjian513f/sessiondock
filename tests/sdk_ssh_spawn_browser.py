#!/usr/bin/env python3
"""SSH SDK callers capture a remote parent before the dispatcher exits."""
import argparse
from contextlib import ExitStack
import http.client
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from types import SimpleNamespace
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD
from history_fixtures import BINARY, REPO, Corpus, codex_message, codex_row, isolated_server
from hub_fixtures import Hub, scoped
from new_session_model_browser import open_dialog
from node_auth_fixtures import TOKEN, free_port, node_env
from sdk_start_stop_fixtures import provider
from spawned_by_fixtures import proc_pid


def proc_root(root, name):
    proc = root/'request-proc'
    (proc/'net').mkdir(parents=True)
    (proc/'sys/kernel/random').mkdir(parents=True)
    (proc/'sys/kernel/random/boot_id').write_text('sdk-ssh-'+name)
    (proc/'stat').write_text('btime 1700000000\n')
    (proc/'net/tcp').write_text('header\n')
    (root/'ids').mkdir()
    (root/'ids/node-id').write_text(name*32)
    return proc


def exercise(browser, binary, hub_binary, root, source, with_agent, agent_binary, screenshots):
    a, b = root/'a', root/'b'
    a.mkdir(); b.mkdir()
    pa, pb = proc_root(a, 'a'), proc_root(b, 'b')
    (a/'state').mkdir(mode=0o700)
    for source_root in ('claude','codex','grok'):
        (a/source_root).mkdir()
    corpus = Corpus(a)
    corpus.put('parent', 'codex', [codex_row('session_meta', {
        'id': THREAD, 'cwd': str(a), 'timestamp': '2026-09-11T10:00:00Z'}),
        codex_message('user', 'Remote SDK dispatcher parent')], [])
    proc_pid(pa, 100, 'codex', ['codex'], 1, fds={3: str(corpus.paths['parent'])})
    nodes = [SimpleNamespace(name=name, nid=name*32, port=free_port(), token=TOKEN) for name in ('a','b')]
    extras = []
    with ExitStack() as stack:
        for home, proc, node in zip((a,b),(pa,pb),nodes):
            env = {**node_env(home,node.port,'127.0.0.0/8'), 'SESSIONDOCK_PROC_ROOT':str(proc)}
            if with_agent:
                process = subprocess.Popen([str(agent_binary), '--uid',str(os.getuid()),
                    '--node-id-file',str(home/'ids/node-id'), '--socket',str(home/'agent.sock'),
                    '--state',str(home/'agent.json'), '--proc-root',str(proc), '--events','off'],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                def stop(process=process):
                    process.terminate(); process.wait(timeout=10)
                stack.callback(stop)
                env['SESSIONDOCK_RESOURCE_AGENT_SOCKET'] = str(home/'agent.sock')
            else:
                env['SESSIONDOCK_RESOURCE_AGENT_MODE'] = 'off'
            extras.append(env)
        stack.enter_context(isolated_server(corpus,binary,state_dir=a/'state',extra_env=extras[0]))
        base, peer, _ = stack.enter_context(provider(source,b,binary,extra_env=extras[1]))
        # Destination has the same native SID: node identity must disambiguate it.
        if source != 'codex':
            Corpus(b).put('local', 'codex', [codex_row('session_meta', {
                'id': THREAD, 'cwd': str(b), 'timestamp': '2026-09-11T10:00:00Z'}),
                codex_message('user','Unrelated destination session')], [])
        home = root/'hub'; home.mkdir()
        hub = Hub(hub_binary,home,nodes); hub.start(); stack.callback(hub.stop)
        context = browser.new_context(viewport={'width':1280,'height':900})
        stack.callback(context.close)
        page = context.new_page(); page.set_default_timeout(20000)
        page.goto(base,wait_until='domcontentloaded'); page.wait_for_function('T.listLoaded')
        page.locator('#nest-toggle').click()
        endpoint = 'codex-sdk' if source == 'codex' else 'native-sdk'
        port = urlsplit(base).port
        requests = []

        def forward(body):
            connection = http.client.HTTPConnection('127.0.0.1',port,timeout=30)
            connection.connect()
            local = connection.sock.getsockname()[1]
            (pb/'net/tcp').write_text(f'header\n 0: 0100007F:{local:04X} 0100007F:{port:04X} 01 0 0 0 0 0 901\n')
            connection.request('POST',f'/api/{endpoint}/create',json.dumps(body),{'Content-Type':'application/json'})
            response = connection.getresponse(); status, raw = response.status,response.read()
            connection.close()
            return status,raw

        def intercept(route):
            # Cold connection: no process-link report can know this caller yet.
            proc_pid(pa,200,'ssh',['ssh','remote','python'],100,fds={4:'socket:[900]'})
            (pa/'net/tcp').write_text('header\n 0: 0100000A:C350 0200000A:C366 01 0 0 0 0 0 900\n')
            proc_pid(pb,300,'python',['python','dispatch.py'],1,
                env=[('SSH_CONNECTION','10.0.0.1 50000 10.0.0.2 50022')],fds={4:'socket:[901]'})
            if requests:
                # Same PID, new incarnation; old verified links must not apply.
                # The new SSH client is multiplexed and has no unique origin.
                (pa/'200/cmdline').write_bytes(b'ssh\0-M\0remote\0')
                for proc, pid, old, new in ((pa,200,20000,40000),(pb,300,30000,60000)):
                    stat=proc/str(pid)/'stat'
                    stat.write_text(stat.read_text().replace(f' {old}',f' {new}'))
            requests.append(route.request.post_data_json)
            status, raw = forward(requests[-1])
            route.fulfill(status=status,content_type='application/json',body=raw)

        page.route(f'**/api/{endpoint}/create',intercept)
        open_dialog(page)
        page.locator(f'input[name=new-source][value={source}]').check()
        page.locator('#new-cwd').fill(str(getattr(peer,'work',b)))
        with page.expect_response(lambda r:r.url.endswith(f'/api/{endpoint}/create')) as created:
            page.locator('#new-session-go').click()
        assert created.value.ok,created.value.text()
        receipt = created.value.json(); uid = receipt['uid']
        parent = {'node_id':nodes[0].nid,'source':'codex','sid':THREAD}
        assert receipt['session'].get('nest_parent') == parent,receipt
        expect(page.locator('#new-session-dialog')).to_be_hidden()
        expect(page.locator('#sdk-input')).to_be_visible()
        page.wait_for_function('async uid=>{await loadSessions(true);return sidebarSessions().some(r=>r.uid===uid)}',arg=uid)
        assert page.evaluate('uid=>sidebarSessions().find(r=>r.uid===uid).nest_parent',uid)==parent
        # The dispatcher exits; attachment must survive both exit and retry.
        shutil.rmtree(pa/'200'); shutil.rmtree(pb/'300')
        (pa/'net/tcp').write_text('header\n')
        assert forward(requests[-1])[0] == 200
        page.goto(f'http://127.0.0.1:{hub.port}',wait_until='domcontentloaded')
        child_uid, parent_uid = scoped(nodes[1].nid,uid),scoped(nodes[0].nid,corpus.uid('parent'))
        page.wait_for_function('T.listLoaded')
        page.locator('#nest-toggle').click()
        page.wait_for_function('uid=>sidebarSessions().some(r=>r.uid===uid)',arg=child_uid)
        parent_row=page.locator(f'#side .item[data-uid="{parent_uid}"]')
        child_row=page.locator(f'#side .item[data-uid="{child_uid}"]')
        if not child_row.is_visible(): parent_row.locator('.nest-caret').click()
        expect(child_row).to_be_visible(); expect(child_row).to_have_attribute('data-depth','1')
        child_row.click(); expect(page.locator('#sdk-input')).to_be_visible()
        if screenshots:
            screenshots.mkdir(parents=True,exist_ok=True)
            for width,theme in [(1280,'light'),(390,'dark')]:
                page.set_viewport_size({'width':width,'height':900}); page.emulate_media(color_scheme=theme)
                if width==390 and page.locator('#detail .mobile-back').is_visible():
                    page.locator('#detail .mobile-back').click()
                page.screenshot(path=str(screenshots/f'{source}-{with_agent}-{width}.png'),full_page=True)
            page.set_viewport_size({'width':1280,'height':900})
        child_row.click(button='right')
        with page.expect_response(lambda r:r.url.endswith('/api/session/nest')) as detached:
            page.locator('#item-menu [data-act="detach"]').click()
        assert detached.value.ok,detached.value.text()
        assert forward(requests[-1])[0]==200
        page.reload(wait_until='domcontentloaded')
        page.wait_for_function('uid=>sidebarSessions().some(r=>r.uid===uid)',arg=child_uid)
        assert not page.evaluate('uid=>sidebarSessions().find(r=>r.uid===uid).nest_parent',child_uid)
        hub.stop(); hub.start()
        page.reload(wait_until='domcontentloaded')
        page.wait_for_function('uid=>sidebarSessions().some(r=>r.uid===uid)',arg=child_uid)
        assert not page.evaluate('uid=>sidebarSessions().find(r=>r.uid===uid).nest_parent',child_uid)
        page.goto(base,wait_until='domcontentloaded'); page.wait_for_function('T.listLoaded')
        open_dialog(page)
        page.locator(f'input[name=new-source][value={source}]').check()
        page.locator('#new-cwd').fill(str(getattr(peer,'work',b)))
        with page.expect_response(lambda r:r.url.endswith(f'/api/{endpoint}/create')) as independent:
            page.locator('#new-session-go').click()
        assert independent.value.ok,independent.value.text()
        other=independent.value.json()
        assert other['uid']!=uid and not other['session'].get('nest_parent'),other
        expect(page.locator('#new-session-dialog')).to_be_hidden()
        expect(page.locator('#sdk-input')).to_be_visible()
    print(f'PASS {source}, collector={with_agent}: cold SSH SDK create, scoped parent receipt/list/Hub, exit/retry, detach and ambiguous reused PID',flush=True)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--hub-binary',type=Path)
    parser.add_argument('--with-agent',action='store_true')
    parser.add_argument('--agent-binary',type=Path,default=REPO/'target/release/resource-agent')
    parser.add_argument('--sources',default='codex,kiro')
    parser.add_argument('--screenshots',type=Path)
    args=parser.parse_args(); binary=args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix='sd-sdk-ssh-') as tmp,sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root=Path(tmp)/source; root.mkdir()
                exercise(browser,binary,args.hub_binary or binary.with_name('sessiondock-hub'),root,source,args.with_agent,args.agent_binary.resolve(),args.screenshots)
        finally:browser.close()


if __name__=='__main__':main()
