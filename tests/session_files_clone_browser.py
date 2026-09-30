#!/usr/bin/env python3
"""Claude/Grok complex family copies through actual node/Hub Chromium controls."""
import argparse
import json
import os
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
import tempfile
from playwright.sync_api import sync_playwright, expect
from history_parity import Corpus, BINARY, isolated_server, get_json
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN
from session_files_browser import fixture, uid
from session_transfer_browser import ident


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-files-publish-') as tmp, sync_playwright() as pw:
        root=Path(tmp);roots,claude,side,agent=fixture(root/'node');corpus=Corpus(root/'node')
        (corpus.root/'state').mkdir();(corpus.root/'proc').mkdir()
        original={str(p):p.read_bytes() for folder in ('claude','grok') for p in (corpus.root/folder).rglob('*') if p.is_file()}
        node=SimpleNamespace(name='source',nid='c'*32,port=free_port(),token=TOKEN)
        (corpus.root/'ids').mkdir();(corpus.root/'ids/node-id').write_text(node.nid+'\n')
        hubroot=root/'hub';hubroot.mkdir();hub=None
        browser=pw.chromium.launch(headless=True,**({'executable_path':os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']} if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE') else {}))
        operations=[]
        try:
            for restart in (False,True):
                with ExitStack() as stack:
                    env=node_env(corpus.root,node.port,'127.0.0.0/8');env.update(SESSIONDOCK_CLAUDE_ROOT=roots['claude'],SESSIONDOCK_PROC_ROOT=str(corpus.root/'proc'))
                    base,opener=stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',extra_env=env))
                    if hub is None:hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node])
                    hub.start();stack.callback(hub.stop)
                    context=browser.new_context(service_workers='block');stack.callback(context.close)
                    page=context.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                    if not restart:
                        for source,selected in [('claude',uid('claude',claude[2])),('grok',uid('grok',corpus.root/'grok/project'/ident(10)))]:
                            selected=scoped(node.nid,selected)
                            page.locator(f'#side .item[data-uid="{selected}"]').click(button='right')
                            page.locator('#item-menu [data-act="clone"]').click()
                            dialog=page.locator('#clone-group-dialog')
                            expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=20000)
                            expect(dialog.locator('.clone-members tbody tr')).to_have_count(4)
                            with page.expect_response(lambda r:r.url.endswith('/api/session/clone') and r.request.method=='POST') as reply:
                                dialog.locator('.clone-confirm').click()
                            response=reply.value;assert response.ok,response.text();result=response.json()
                            page.wait_for_function('(id)=>S.sel===id',arg=result['target_uid'])
                            expect(page.locator('#msgs')).to_contain_text('Branch A final' if source=='claude' else 'Grok answer 10')
                            op=json.loads((corpus.root/'state/transfers'/result['operation_id']/'operation.json').read_text())
                            assert op['phase']=='complete'
                            ids=op['file_plan']['sessions']
                            if source=='claude':
                                page.locator('#a-view-switch').click()
                                page.locator(f'#session-view-menu button[data-agent="{ids["claude:"+agent]}"]').click()
                                expect(page.locator('#msgs')).to_contain_text('Agent answer')
                                assert (corpus.root/'claude/file-history'/ids['claude:'+ident(2)]/'abcdef@v1').is_file()
                            else:
                                rows=get_json(opener,base,'/api/sessions')['sessions']
                                child=next(row for row in rows if row['sid']==ids['grok:'+ident(13)])
                                page.locator(f'#side .item[data-uid="{scoped(node.nid,child["uid"])}"]').click()
                                expect(page.locator('#msgs')).to_contain_text('Grok answer 13')
                            assert all(Path(p).read_bytes()==raw for p,raw in original.items())
                            operations.append((selected,result,op))
                            print('PASS real Hub/Chromium '+source+' whole-family copy, opened new history/agent, source intact',flush=True)
                        # Fail after earlier files have been published: rollback
                        # must remove only this transaction's owned files.
                        selected=operations[0][0]
                        page.locator(f'#side .item[data-uid="{selected}"]').click()
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as planned:
                            page.locator('#a-clone-group').click()
                        plan=planned.value.json()
                        journal=corpus.root/'state/transfers'/plan['operation_id']/'operation.json'
                        pending=json.loads(journal.read_text())
                        last=Path(pending['file_publications'][-1]['staging'])
                        last.write_bytes(last.read_bytes()+b'changed')
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone') and r.request.method=='POST') as failed:
                            page.locator('#clone-group-dialog .clone-confirm').click()
                        assert not failed.value.ok
                        assert json.loads(journal.read_text())['phase']=='failed'
                        assert all(not Path(f['target']).exists() for f in pending['file_publications'])
                        assert all(Path(p).read_bytes()==raw for p,raw in original.items())
                        print('PASS failed file-family publication rolls back owned files and preserves source',flush=True)
                    else:
                        for selected,result,op in operations:
                            target=Path(op['file_publications'][0]['target']);before=target.read_bytes()
                            # A retry after restart cannot republish or overwrite a continued clone.
                            target.write_bytes(before+b'\n')
                            reply=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/clone',data={'uid':selected,'operation_id':op['id']})
                            assert reply.ok,reply.text();assert reply.json()['target_uid']==result['target_uid']
                            assert target.read_bytes()==before+b'\n'
                        assert all(Path(p).read_bytes()==raw for p,raw in original.items())
                        print('PASS file-family publication retries survive node/Hub restart without replacing continued files',flush=True)
        finally:browser.close()


if __name__=='__main__':main()
