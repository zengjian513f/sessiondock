#!/usr/bin/env python3
"""Reject stale staging, then clone a cross-provider family after restart."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import urlencode, urlsplit, parse_qs
from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, isolated_server, get_json
from session_clone_browser import prepare_confirmed, prepare, native_rows
from session_files_browser import fixture, uid
from session_transfer_browser import ident
from hub_http_suite import Hub, free_port
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
        before['sessions'][corpus.uid('b')]={'nest_initialized':True}
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
                        prepare_confirmed(page,node,operation)
                        saved=json.loads(journal.read_text())
                        last=Path(saved['file_publications'][-1]['staging'])
                        last.write_bytes(last.read_bytes()+b'changed')
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone')) as failed:
                            dialog.locator('.clone-confirm').click()
                        assert failed.value.status==409 and failed.value.json()['code']=='move_plan_stale',failed.value.text()
                        # The UI abandons a stale plan before publication. A new
                        # plan must regenerate staging from unchanged sources.
                        expect(dialog.locator('.clone-confirm')).to_be_enabled()
                        assert json.loads(journal.read_text())['phase']=='aborted',journal.read_text()
                        aborted_operation=operation;aborted_journal=journal
                        assert {key:rows for key,rows in native_rows(corpus.root/'codex').items() if not key[1].startswith('_sessiondock_')}==source_db
                        assert json.loads(metadata.read_text())==before
                        assert all(p.read_bytes()==raw for p,raw in original.items())
                        print('PASS mixed fourteen-session plan rejects stale staging before publication and preserves all providers',flush=True)
                    else:
                        page.goto(f'http://127.0.0.1:{hub.port}/?'+urlencode({'sid':'claude:'+ident(2),'node':node.nid}),wait_until='networkidle')
                        expect(page.locator('#msgs')).to_contain_text('Branch A final')
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as replanned:
                            page.locator('#a-clone-group').click()
                        assert replanned.value.ok,replanned.value.text()
                        operation=replanned.value.json()['operation_id']
                        assert operation!=aborted_operation
                        assert json.loads(aborted_journal.read_text())['phase']=='aborted'
                        journal=corpus.root/'state/transfers'/operation/'operation.json'
                        saved=json.loads(journal.read_text())
                        dialog=page.locator('#clone-group-dialog')
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
                        detached=session('codex',codex[ident(4)])
                        assert state[detached['uid']]['nest_initialized'] is True and 'nest_parent' not in state[detached['uid']]
                        assert state[cloned_claude['uid']]['nest_parent']=={'source':'codex','sid':codex[ident(2)]}
                        assert state[cloned_grok['uid']]['nest_parent']=={'source':'claude','sid':ids['claude:'+ident(2)]}
                        for source,sid,text in [('claude',ids['claude:'+ident(2)],'Branch A final'),
                                                ('codex',codex[ident(2)],'Branch A current'),
                                                ('grok',ids['grok:'+ident(10)],'Grok answer 10')]:
                            page.goto(f'http://127.0.0.1:{hub.port}/?'+urlencode({'sid':source+':'+sid,'node':node.nid}),wait_until='networkidle')
                            expect(page.locator('#msgs')).to_contain_text(text)
                            query=parse_qs(urlsplit(page.url).query)
                            assert query['sid']==[source+':'+sid] and query['node']==[node.nid],page.url
                        assert all(p.read_bytes()==raw for p,raw in original.items())
                        for key,value in before['sessions'].items():assert state[key]==value
                        after_db=native_rows(corpus.root/'codex')
                        for key,records in source_db.items():
                            assert all(record in after_db[key] for record in records),key
                        print('PASS browser restart replans the mixed clone, opens all three native share links, remaps ownership and preserves originals',flush=True)
                        # A missing parent and a remote same-SID parent are known
                        # members that a single-source plan must never discard.
                        baseline=json.loads(metadata.read_text())
                        for parent in [{'source':'codex','sid':ident(999)},
                                       {'source':'codex','sid':codex[ident(2)],'node_id':'d'*32}]:
                            broken=json.loads(json.dumps(baseline))
                            broken['revision']=json.loads(metadata.read_text())['revision']+1
                            broken['sessions'][cloned_claude['uid']]['nest_parent']=parent
                            metadata.write_text(json.dumps(broken))
                            page.goto(f'http://127.0.0.1:{hub.port}/?'+urlencode({'sid':'claude:'+cloned_claude['sid'],'node':node.nid}),wait_until='networkidle')
                            with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as blocked:
                                page.locator('#a-clone-group').click()
                            assert blocked.value.status==409 and blocked.value.json()['code']=='move_group_incomplete',blocked.value.text()
                            expect(page.locator('#clone-group-dialog .transfer-error')).to_be_visible()
                            expect(page.locator('#clone-group-dialog .clone-confirm')).to_be_disabled()
                        baseline['revision']=json.loads(metadata.read_text())['revision']+1
                        metadata.write_text(json.dumps(baseline))
                        assert all(p.read_bytes()==raw for p,raw in original.items())
                        print('PASS Chromium whole-group plans refuse missing parents and remote same-SID parents',flush=True)
        finally:browser.close()


if __name__=='__main__':main()
