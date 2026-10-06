#!/usr/bin/env python3
"""Cancel a failed local publication, then clone after restart without changing sources."""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile
import sqlite3
import time
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, isolated_server
from session_clone_browser import prepare_confirmed, prepare
from session_files_browser import fixture, uid
from session_bundle_browser import reopen_transfer
from session_transfer_browser import ident
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-local-recovery-') as tmp, sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True,**({'executable_path':os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']} if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE') else {}))
        try:
            for provider in ('codex','claude','grok'):
                root=Path(tmp)/provider;root.mkdir()
                if provider=='codex':
                    corpus=prepare(root/'node');selected=corpus.uid('a');count=6
                    roots={k:str(corpus.root/k) for k in ('codex','claude','grok')}
                    expected='Branch A current'
                else:
                    roots,main,side,agent=fixture(root/'node');corpus=Corpus(root/'node')
                    (corpus.root/'state').mkdir();(corpus.root/'proc').mkdir()
                    roots['codex']=str(corpus.root/'codex')
                    selected=uid('claude',main[2]) if provider=='claude' else uid('grok',corpus.root/'grok/project'/ident(10))
                    count=4;expected='Branch A final' if provider=='claude' else 'Grok answer 10'
                originals={p:p.read_bytes() for name in ('codex','claude','grok') for p in (corpus.root/name).rglob('*') if p.is_file() and '.sqlite' not in p.name}
                node=SimpleNamespace(name='source',nid='c'*32,port=free_port(),token=TOKEN)
                (corpus.root/'ids').mkdir();(corpus.root/'ids/node-id').write_text(node.nid+'\n')
                (corpus.root/'trash').mkdir()
                hubroot=root/'hub';hubroot.mkdir();hub=None
                selected=scoped(node.nid,selected)
                for restart in (False,True):
                    with ExitStack() as stack:
                        env=node_env(corpus.root,node.port,'127.0.0.0/8')
                        env.update({f'SESSIONDOCK_{k.upper()}_ROOT':v for k,v in roots.items()})
                        env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
                        stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',trash_dir=corpus.root/'trash',extra_env=env))
                        if hub is None:hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node])
                        hub.start();stack.callback(hub.stop)
                        context=browser.new_context(service_workers='block');stack.callback(context.close)
                        page=context.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                        if not restart:
                            page.locator(f'#side .item[data-uid="{selected}"]').click()
                            with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as response:
                                page.locator('#a-clone-group').click()
                            plan=response.value.json();operation=plan['operation_id']
                            dialog=page.locator('#clone-group-dialog')
                            expect(dialog.locator('.clone-members tbody tr')).to_have_count(count)
                            journal=corpus.root/'state/transfers'/operation/'operation.json'
                            prepare_confirmed(page,node,operation)
                            saved=json.loads(journal.read_text())
                            if provider=='codex':
                                last=journal.parent/'staging'/saved['staged']['files'][-1]['relative']
                            else:last=Path(saved['file_publications'][-1]['staging'])
                            original=last.read_bytes();last.write_bytes(original+b'changed')
                            with page.expect_response(lambda r:r.url.endswith('/api/session/clone'),timeout=60000) as failed:
                                dialog.locator('.clone-confirm').click()
                            assert not failed.value.ok,failed.value.text()
                            assert json.loads(journal.read_text())['phase']=='aborted'
                            assert all(p.read_bytes()==raw for p,raw in originals.items())
                            aborted=operation
                            print('PASS '+provider+' failed local publication is cancelled and preserves source',flush=True)
                        else:
                            page.locator(f'#side .item[data-uid="{selected}"]').click()
                            page.wait_for_function('(uid)=>S.sel===uid',arg=selected)
                            with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as response:
                                page.locator('#a-clone-group').click()
                            assert response.value.ok,response.value.text()
                            plan=response.value.json();operation=plan['operation_id']
                            assert operation!=aborted
                            journal=corpus.root/'state/transfers'/operation/'operation.json'
                            saved=json.loads(journal.read_text())
                            dialog=page.locator('#clone-group-dialog')
                            expect(dialog.locator('.clone-members tbody tr')).to_have_count(count)
                            with page.expect_response(lambda r:r.url.endswith('/api/session/clone'),timeout=60000) as result:
                                dialog.locator('.clone-confirm').click()
                            assert result.value.ok,result.value.text()
                            completed=result.value.json()
                            assert completed['operation_id']==operation and completed['target_uid']==plan['target_uid']
                            assert completed['phase']=='complete'
                            page.wait_for_function('(uid)=>S.sel===uid',arg=completed['target_uid'])
                            expect(page.locator('#msgs')).to_contain_text(expected)
                            final=json.loads(journal.read_text())
                            assert final['plan']['identities']==saved['plan']['identities']
                            assert final.get('file_plan')==saved.get('file_plan')
                            pending=context.request.get(f'http://127.0.0.1:{hub.port}/api/session/transfers')
                            assert pending.ok and not any(t['request']['operation_id']==operation for t in pending.json()['operations'])
                            assert all(p.read_bytes()==raw for p,raw in originals.items())
                            print('PASS '+provider+' Hub/node restart and Chromium fresh copy preserve planned identity, history and source',flush=True)
                            if provider=='codex':
                                # Hold the target database's writer lock after planning,
                                # then crash the Hub while native publication is waiting.
                                page.locator(f'#side .item[data-uid="{selected}"]').click()
                                with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as prepared:
                                    page.locator('#a-clone-group').click()
                                interrupted_plan=prepared.value.json();interrupted=interrupted_plan['operation_id']
                                interrupted_path=corpus.root/'state/transfers'/interrupted/'operation.json'
                                prepare_confirmed(page,node,interrupted)
                                with sqlite3.connect(corpus.root/'codex/state_5.sqlite') as locked:
                                    locked.execute('BEGIN IMMEDIATE')
                                    with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/progress')) as progress:
                                        page.locator('#clone-group-dialog .clone-confirm').click()
                                    assert progress.value.ok and progress.value.json()['phase']=='publishing',progress.value.text()
                                    expect(page.locator('#clone-group-dialog .transfer-progress')).to_contain_text('发布历史')
                                    hub.process.kill();hub.process.wait(timeout=5)
                                for _ in range(100):
                                    native=json.loads(interrupted_path.read_text())
                                    if native['phase']=='complete':break
                                    time.sleep(.05)
                                assert native['phase']=='complete',native.get('error')
                                unresolved=json.loads((hubroot/'transfers'/(interrupted+'.json')).read_text())
                                assert unresolved['phase']=='publishing'
                                hub.start()
                                page.reload(wait_until='networkidle')
                                acknowledged=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/clone',
                                    data={'uid':selected,'operation_id':interrupted})
                                assert acknowledged.ok,acknowledged.text()
                                assert acknowledged.json()['target_uid']==interrupted_plan['target_uid']
                                target_uid=acknowledged.json()['target_uid']
                                page.locator(f'#side .item[data-uid="{target_uid}"]').click()
                                expect(page.locator('#msgs')).to_contain_text(expected)
                                assert all(p.read_bytes()==raw for p,raw in originals.items())
                                print('PASS local publication outlives Hub crash; Chromium opens the completed clone with the same identity',flush=True)
        finally:browser.close()


if __name__=='__main__':main()
