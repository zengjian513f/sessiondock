#!/usr/bin/env python3
"""Opt-in two-host Chromium transfer of one fourteen-session cross-CLI family."""
# run_validation: skip

import argparse
from transfer_ui import select_target
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import urlencode
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, isolated_server
from session_clone_browser import prepare, native_rows
from session_files_browser import fixture, uid
from session_bundle_browser import Peer
from session_transfer_browser import ident
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--peer',required=True,help='SSH peer; private fixtures only')
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-mixed-bundle-') as tmp, sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            for moving in (False,True):
                for fresh in (False,True):
                    root=Path(tmp)/f'{moving}-{fresh}';root.mkdir()
                    source=prepare(root/'source')
                    roots,claude,side,agent=fixture(root/'files')
                    roots['codex']=str(source.root/'codex')
                    metadata=source.root/'state/session-metadata.json'
                    before=json.loads(metadata.read_text())
                    selected=uid('claude',claude[2]);grok_uid=uid('grok',root/'files/grok/project'/ident(10))
                    before['sessions'][selected]={'nest_parent':{'source':'codex','sid':ident(2)},'starred':True}
                    before['sessions'][grok_uid]={'nest_parent':{'source':'claude','sid':ident(2)},'starred':True}
                    before['sessions'][source.uid('b')]={'nest_initialized':True}
                    metadata.write_text(json.dumps(before))
                    original={p:p.read_bytes() for home in (source.root/'codex',root/'files/claude',root/'files/grok')
                              for p in home.rglob('*') if p.is_file() and '.sqlite' not in p.name}
                    original_db=native_rows(source.root/'codex')
                    a=SimpleNamespace(name='source',nid='a'*32,port=free_port(),token=TOKEN)
                    b=SimpleNamespace(name='destination',nid='b'*32,port=free_port(),token=TOKEN)
                    (source.root/'ids').mkdir();(source.root/'ids/node-id').write_text(a.nid+'\n')
                    (source.root/'trash').mkdir();hubroot=root/'hub';hubroot.mkdir()
                    with ExitStack() as stack:
                        peer=Peer(args.peer,root,source,roots,b,args.binary);stack.callback(peer.close)
                        peer.call('seed_cwd',path=str(root/'files/cwd'),entries=[
                            {'relative':'','kind':'directory','mode':(root/'files/cwd').stat().st_mode & 0o777}])
                        peer.call('start')
                        env=node_env(source.root,a.port,'127.0.0.0/8')
                        env.update({f'SESSIONDOCK_{k.upper()}_ROOT':v for k,v in roots.items()})
                        env['SESSIONDOCK_PROC_ROOT']=str(source.root/'proc')
                        stack.enter_context(isolated_server(source,args.binary,state_dir=source.root/'state',
                            trash_dir=source.root/'trash',extra_env=env))
                        hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[a,b]);hub.start();stack.callback(hub.stop)
                        context=browser.new_context();stack.callback(context.close)
                        page=context.new_page();base=f'http://127.0.0.1:{hub.port}'
                        page.goto(base+'/?'+urlencode({'sid':scoped(a.nid,selected),'node':a.nid}),wait_until='networkidle')
                        expect(page.locator('#msgs')).to_contain_text('Branch A final')
                        loaded=json.loads(metadata.read_text())
                        assert loaded['sessions']==before['sessions']
                        before=loaded  # Include normal startup catalog migration before transfer.
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as planned:
                            page.locator('#a-clone-group').click()
                        assert planned.value.ok,planned.value.text()
                        dialog=page.locator('#clone-group-dialog')
                        expect(dialog.locator('.clone-members tbody tr')).to_have_count(14)
                        select_target(dialog, b.nid)
                        if moving:
                            dialog.locator('.transfer-segments label').nth(1).click()
                        if dialog.locator('#transfer-new-ids').is_checked()!=fresh:
                            dialog.locator('#transfer-new-ids').set_checked(fresh)
                        expect(dialog.locator('.clone-confirm')).to_be_enabled()
                        expect(dialog.locator('.clone-members tbody tr')).to_have_count(14)
                        with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as transferred:
                            dialog.locator('.clone-confirm').click()
                        assert transferred.value.ok,transferred.value.text()
                        result=transferred.value.json()
                        page.wait_for_function('(uid)=>S.sel===uid',arg=result['target_uid'])
                        expect(page.locator('#msgs')).to_contain_text('Branch A final')
                        operation=json.loads((source.root/'state/transfers'/result['operation_id']/'operation.json').read_text())
                        target=json.loads(peer.read(root/'destination/state/transfers'/result['operation_id']/'operation.json'))
                        assert operation['phase']==('retired' if moving else 'exported')
                        assert target['phase']=='complete'
                        members=operation['full_group']['members']
                        assert len({(m['source'],m['sid']) for m in members})==14
                        assert len([m for m in members if m['source']=='codex' and m['sid']==ident(2)])==2
                        codex=operation['plan']['identities']['threads'];ids=operation['file_plan']['sessions']
                        state=json.loads(peer.read(root/'destination/state/session-metadata.json'))['sessions']
                        response=context.request.get(base+'/api/sessions');assert response.ok,response.text()
                        rows=response.json()['sessions']
                        def row_for(source,sid):
                            return next(r for r in rows if r['source']==source and r['sid']==sid and b.nid in r['uid'] and not r.get('continued_in'))
                        for provider,sid,text in [('codex',codex[ident(2)],'Branch A current'),
                                                  ('claude',ids['claude:'+ident(2)],'Branch A final'),
                                                  ('grok',ids['grok:'+ident(10)],'Grok answer 10')]:
                            row=row_for(provider,sid)
                            page.goto(base+'/?'+urlencode({'sid':row['uid'],'node':b.nid}),wait_until='networkidle')
                            expect(page.locator('#msgs')).to_contain_text(text)
                            local=provider+':'+row['uid'].split('~',1)[1]
                            if provider=='claude':assert state[local]['nest_parent']=={'source':'codex','sid':codex[ident(2)]}
                            if provider=='grok':assert state[local]['nest_parent']=={'source':'claude','sid':ids['claude:'+ident(2)]}
                        detached=row_for('codex',codex[ident(4)])
                        detached_uid='codex:'+detached['uid'].split('~',1)[1]
                        assert state[detached_uid]['nest_initialized'] is True and 'nest_parent' not in state[detached_uid]
                        assert all((old!=new)==fresh for old,new in codex.items())
                        assert all((old.split(':',1)[1]!=new)==fresh for old,new in ids.items())
                        if moving:
                            moved={Path(f['source']) for f in operation['plan']['files']}
                            moved.update(Path(f['source']) for f in operation['file_plan']['files'])
                            assert all(not os.path.lexists(p) if p in moved else p.read_bytes()==raw for p,raw in original.items())
                        else:
                            assert all(p.read_bytes()==raw for p,raw in original.items())
                            assert json.loads(metadata.read_text())==before,(before,json.loads(metadata.read_text()))
                            assert {key:value for key,value in native_rows(source.root/'codex').items() if not key[1].startswith('_sessiondock_')}==original_db
                        print(f'PASS mixed fourteen-session Chromium {"move" if moving else "copy"}, {"new" if fresh else "preserved"} identities: cross-provider links, three target histories and source integrity',flush=True)
        finally:browser.close()


if __name__=='__main__':main()
