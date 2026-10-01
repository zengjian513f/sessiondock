#!/usr/bin/env python3
"""Clone one cross-provider connected family; compensate and retry after restart."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import urlencode
from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, isolated_server, get_json
from session_clone_browser import prepare, native_rows
from session_files_browser import fixture, uid
from session_bundle_browser import reopen_transfer
from session_transfer_browser import ident
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-mixed-clone-') as tmp, sync_playwright() as pw:
        root=Path(tmp);corpus=prepare(root/'codex-node')
        roots,claude,side,agent=fixture(root/'file-node')
        roots['codex']=str(corpus.root/'codex')
        metadata=corpus.root/'state/session-metadata.json'
        before=json.loads(metadata.read_text())
        claude_uid=uid('claude',claude[2]);grok_uid=uid('grok',root/'file-node/grok/project'/ident(10))
        before['sessions'][claude_uid]={'nest_parent':{'source':'codex','sid':ident(2)},'starred':True}
        before['sessions'][grok_uid]={'nest_parent':{'source':'claude','sid':ident(2)},'starred':True}
        metadata.write_text(json.dumps(before))
        original={p:p.read_bytes() for home in (corpus.root/'codex',root/'file-node/claude',root/'file-node/grok')
                  for p in home.rglob('*') if p.is_file() and '.sqlite' not in p.name}
        source_db=native_rows(corpus.root/'codex')
        node=SimpleNamespace(name='source',nid='c'*32,port=free_port(),token=TOKEN)
        (corpus.root/'ids').mkdir();(corpus.root/'ids/node-id').write_text(node.nid+'\n')
        (corpus.root/'trash').mkdir();hubroot=root/'hub';hubroot.mkdir()
        hub=None
        browser=pw.chromium.launch(headless=True)
        try:
            for restart in (False,True):
                with ExitStack() as stack:
                    env=node_env(corpus.root,node.port,'127.0.0.0/8')
                    env.update({f'SESSIONDOCK_{k.upper()}_ROOT':v for k,v in roots.items()})
                    env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
                    base,opener=stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',
                        trash_dir=corpus.root/'trash',extra_env=env))
                    if hub is None:hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node])
                    hub.start();stack.callback(hub.stop)
                    context=browser.new_context();stack.callback(context.close)
                    page=context.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                    if not restart:
                        loaded=json.loads(metadata.read_text())
                        assert loaded['sessions']==before['sessions']
                        before=loaded  # Include startup catalog migration before testing rollback.
                        page.goto(f'http://127.0.0.1:{hub.port}/?'+urlencode({'sid':'claude:'+ident(2),'node':node.nid}),wait_until='networkidle')
                        expect(page.locator('#msgs')).to_contain_text('Branch A final')
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as planned:
                            page.locator('#a-clone-group').click()
                        assert planned.value.ok,planned.value.text()
                        plan=planned.value.json();operation=plan['operation_id']
                        dialog=page.locator('#clone-group-dialog')
                        expect(dialog.locator('.clone-members tbody tr')).to_have_count(14)
                        assert {s['source'] for s in plan['sessions']}=={'codex','claude','grok'}
                        journal=corpus.root/'state/transfers'/operation/'operation.json'
                        saved=json.loads(journal.read_text())
                        last=Path(saved['file_publications'][-1]['staging']);contents=last.read_bytes()
                        last.write_bytes(contents+b'changed')
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone')) as failed:
                            dialog.locator('.clone-confirm').click()
                        assert not failed.value.ok,failed.value.text()
                        assert json.loads(journal.read_text())['phase']=='failed'
                        assert {key:rows for key,rows in native_rows(corpus.root/'codex').items() if not key[1].startswith('_sessiondock_')}==source_db
                        assert json.loads(metadata.read_text())==before
                        assert all(p.read_bytes()==raw for p,raw in original.items())
                        last.write_bytes(contents)
                        print('PASS mixed Codex/Claude/Grok fourteen-session plan; late publication failure compensates native rows and all providers',flush=True)
                    else:
                        dialog=reopen_transfer(page,hub,operation)
                        expect(dialog.locator('.clone-members tbody tr')).to_have_count(14)
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone')) as copied:
                            dialog.locator('.clone-confirm').click()
                        assert copied.value.ok,copied.value.text()
                        final=json.loads(journal.read_text());assert final['phase']=='complete'
                        assert final['plan']['identities']==saved['plan']['identities']
                        assert final['file_plan']==saved['file_plan']
                        ids=final['file_plan']['sessions'];codex=final['plan']['identities']['threads']
                        state=json.loads(metadata.read_text())['sessions']
                        rows=get_json(opener,base,'/api/sessions')['sessions']
                        def session(source,sid):return next(r for r in rows if r['source']==source and r['sid']==sid and not r.get('continued_in'))
                        cloned_claude=session('claude',ids['claude:'+ident(2)])
                        cloned_grok=session('grok',ids['grok:'+ident(10)])
                        assert state[cloned_claude['uid']]['nest_parent']=={'source':'codex','sid':codex[ident(2)]}
                        assert state[cloned_grok['uid']]['nest_parent']=={'source':'claude','sid':ids['claude:'+ident(2)]}
                        for source,sid,text in [('claude',ids['claude:'+ident(2)],'Branch A final'),
                                                ('codex',codex[ident(2)],'Branch A current'),
                                                ('grok',ids['grok:'+ident(10)],'Grok answer 10')]:
                            row=session(source,sid)
                            page.goto(f'http://127.0.0.1:{hub.port}/?'+urlencode({'sid':scoped(node.nid,row['uid']),'node':node.nid}),wait_until='networkidle')
                            expect(page.locator('#msgs')).to_contain_text(text)
                        assert all(p.read_bytes()==raw for p,raw in original.items())
                        for key,value in before['sessions'].items():assert state[key]==value
                        after_db=native_rows(corpus.root/'codex')
                        for key,records in source_db.items():
                            assert all(record in after_db[key] for record in records),key
                        print('PASS browser restart retries the same mixed clone, opens all three histories, remaps cross-provider ownership and preserves originals',flush=True)
        finally:browser.close()


if __name__=='__main__':main()
