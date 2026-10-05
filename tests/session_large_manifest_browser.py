#!/usr/bin/env python3
"""Chromium transfers a group whose native database makes the manifest exceed 64 MiB."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import sqlite3
import tempfile
import time
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, Corpus, isolated_server
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN
from session_clone_browser import prepare
from session_transfer_browser import ident
from session_bundle_browser import Peer


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--peer',help='Optional SSH peer with shared checkout; move between private local filesystems')
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-large-manifest-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root=Path(tmp);source=prepare(root/'source');destination=Corpus(root/'destination')
        roots={kind:str(source.root/kind) for kind in ('codex','claude','grok')}
        payload=json.dumps({'type':'message','content':'native projection payload '+('x'*(34*1024*1024))})
        database=source.root/'codex/thread_history_1.sqlite'
        with sqlite3.connect(database) as db:
            db.execute('INSERT INTO thread_items VALUES (?,?,?,?)',(ident(2),'large-turn','large-item',payload))
        a=SimpleNamespace(name='source',nid='a'*32,port=free_port(),token=TOKEN)
        b=SimpleNamespace(name='destination',nid='b'*32,port=free_port(),token=TOKEN)
        for node,corpus in ((a,source),(b,destination)):
            for name in ('state','proc','ids','trash'):(corpus.root/name).mkdir(parents=True,exist_ok=True)
            (corpus.root/'ids/node-id').write_text(node.nid+'\n')
        peer=Peer(args.peer,root,source,roots,b,args.binary) if args.peer else None
        if peer:stack.callback(peer.close)
        for node,corpus in ((a,source),(b,destination)):
            if peer and node is b:
                peer.call('start');continue
            env=node_env(corpus.root,node.port,'127.0.0.0/8')
            env.update({f'SESSIONDOCK_{k.upper()}_ROOT':v for k,v in roots.items()})
            env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
            stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',trash_dir=corpus.root/'trash',extra_env=env))
        hubroot=root/'hub';hubroot.mkdir();hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[a,b]);hub.start();stack.callback(hub.stop)
        browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
        page=browser.new_page();page.set_default_timeout(30000)
        errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
        page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
        selected=scoped(a.nid,source.uid('a'))
        page.locator(f'#side .item[data-uid="{selected}"]').click()
        page.locator('#a-clone-group').click();dialog=page.locator('#clone-group-dialog')
        expect(dialog.locator('.clone-confirm')).to_be_enabled()
        dialog.locator('#transfer-target').select_option(b.nid)
        if peer:
            dialog.locator('.transfer-segments label').nth(1).click()
            dialog.locator('#transfer-new-ids').check()
        expect(dialog.locator('.clone-confirm')).to_be_enabled()
        started=time.monotonic()
        with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone') and r.request.method=='POST',timeout=120000) as done:
            dialog.locator('.clone-confirm').click()
        assert done.value.ok,done.value.text()
        result=done.value.json();assert result['phase']=='complete',result
        page.wait_for_function('uid=>S.sel===uid',arg=result['target_uid'])
        expect(page.locator('#msgs')).to_contain_text('Branch A current')
        operation=result['operation_id']
        receipt=json.loads((source.root/'state/transfers'/operation/'operation.json').read_text())
        # Native before/after images alone exceed the ordinary Hub JSON cap.
        native_bytes=sum(len(json.dumps(receipt[k],separators=(',',':'))) for k in ('native','rewritten'))
        assert native_bytes>64*1024*1024,native_bytes
        native_id=result['link_map']['codex:'+ident(2)].split(':',1)[1]
        target_db=database
        if peer:
            # Fetch only this private test database for native payload verification.
            target_db=root/'target-history.sqlite';target_db.write_bytes(peer.read(database))
            assert receipt['phase']=='retired'
            assert not source.paths['a'].exists()
            assert any((source.root/'trash').glob('move-*/manifest.json'))
        else:
            assert source.paths['a'].exists()
        with sqlite3.connect(target_db) as db:
            assert db.execute('SELECT count(*) FROM thread_items WHERE thread_id=? AND item_json=?',(native_id,payload)).fetchone()==(1,)
        assert not errors,errors
        print(f'PASS Chromium {"move" if peer else "copy"}: native before/after {native_bytes/1024/1024:.1f} MiB, target history and exact native payload intact, {time.monotonic()-started:.2f}s',flush=True)


if __name__=='__main__':main()
