#!/usr/bin/env python3
"""Whole-tree deletion through Chromium and Hub, with private native fixtures."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
import shutil
import time
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, isolated_server
from session_clone_browser import prepare
from transfer_ui import seed_names
from session_transfer_browser import ident
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN
from popups import on_popup
from spawned_by_suite import proc_pid
from session_files_browser import fixture, uid as file_uid


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=Path,default=BINARY);args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-tree-browser-') as tmp, tempfile.TemporaryDirectory(prefix='sessiondock-tree-trash-',dir='/dev/shm') as recycle, sync_playwright() as pw:
        root=Path(tmp);corpus=prepare(root/'node');trash=Path(recycle)
        roots,claude,_,_=fixture(root/'files')
        metadata=corpus.root/'state/session-metadata.json'
        document=json.loads(metadata.read_text())
        document['sessions'][file_uid('claude',claude[2])]={'nest_parent':{'source':'codex','sid':ident(2)},'starred':True}
        document['sessions'][file_uid('grok',root/'files/grok/project'/ident(10))]={'nest_parent':{'source':'claude','sid':ident(2)},'starred':True}
        metadata.write_text(json.dumps(document))
        extra_before={str(p):p.read_bytes() for name in ('claude','grok') for p in (root/'files'/name).rglob('*') if p.is_file()}
        title,_,_=seed_names(corpus.root/'codex',ident(2))
        before={str(p):p.read_bytes() for p in corpus.paths.values()}
        names=(corpus.root/'codex/session_index.jsonl').read_bytes()
        meta=(corpus.root/'state/session-metadata.json').read_bytes()
        node=SimpleNamespace(name='test',nid='a'*32,port=free_port(),token=TOKEN)
        (corpus.root/'ids').mkdir();(corpus.root/'ids/node-id').write_text(node.nid+'\n')
        (root/'hub').mkdir()
        browser=pw.chromium.launch(headless=True,args=['--no-sandbox'])
        receipts=[];hub=None
        try:
            for restart in (False,True):
                with ExitStack() as stack:
                    env=node_env(corpus.root,node.port,'127.0.0.0/8')
                    env.update({f'SESSIONDOCK_{k.upper()}_ROOT':v for k,v in roots.items()})
                    env['SESSIONDOCK_TRASH_DIR']=str(trash)
                    env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
                    env['SESSIONDOCK_CODEX_INDEX']=str(corpus.root/'codex/session_index.jsonl')
                    stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',extra_env=env))
                    if hub is None:hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),root/'hub',[node])
                    hub.start();stack.callback(hub.stop)
                    context=browser.new_context(service_workers='block',viewport={'width':390 if restart else 1280,'height':900},has_touch=True);stack.callback(context.close)
                    page=context.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                    on_popup(page,lambda d:d.accept())
                    uid=scoped(node.nid,corpus.uid('a'))
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    def open_tree(context_menu=False):
                        if context_menu:
                            page.locator(f'#side .item[data-uid="{uid}"]').click(button="right")
                            page.locator('#item-menu [data-act="delete-tree"]').click()
                        button=page.locator('#a-delete-tree')
                        if not context_menu:
                            if not button.is_visible():page.locator('#a-more').click()
                            button.click()
                        dialog=page.locator('#delete-tree-dialog')
                        expect(dialog.locator('.tree-confirm')).to_be_enabled(timeout=20000)
                        expect(dialog.locator('.clone-status')).to_contain_text('14 个会话')
                        expect(dialog.locator('tbody')).to_contain_text(title)
                        return dialog
                    if restart:
                        # A completed ID remains complete after restart and restoration.
                        old=receipts[-1]
                        result=page.request.post(f'http://127.0.0.1:{hub.port}/api/session/tree/delete',data=old)
                        assert result.ok,result.text()
                        assert result.json()['phase']=='complete',result.text()
                        assert all(Path(p).read_bytes()==b for p,b in before.items())
                        print('PASS durable retry after restore/restart never deletes restored files',flush=True)
                    dialog=open_tree(context_menu=not restart)
                    dialog.locator('.tree-close').click()
                    assert all(Path(p).read_bytes()==b for p,b in before.items())
                    print('PASS preview/cancel leaves all histories and custom titles unchanged',flush=True)
                    if not restart:
                        # Compare the copy inventory without staging native files.
                        if not page.locator('#a-clone-group').is_visible():page.locator('#a-more').click()
                        with page.expect_response('**/api/session/clone/plan') as response:page.locator('#a-clone-group').click()
                        copied=response.value.json()
                        page.locator('#clone-group-dialog .clone-cancel').click()
                        expect(page.locator('#clone-group-dialog')).to_have_count(0)
                        with page.expect_response('**/api/session/tree/plan') as response:dialog=open_tree()
                        preview=response.value.json()
                        assert {m['uid'] for m in preview['sessions']}=={m['uid'] for m in copied['sessions']}
                        # Add a related physical candidate after preview: no partial deletion.
                        added=corpus.paths['a'].with_name('rollout-new-related.jsonl')
                        added.write_bytes(corpus.paths['a'].read_bytes().replace(ident(2).encode(),ident(90).encode()).replace(ident(3).encode(),ident(91).encode()))
                        dialog.locator('.tree-confirm').click()
                        expect(dialog.locator('.transfer-error')).to_contain_text('成员已变化',timeout=20000)
                        assert all(Path(p).read_bytes()==b for p,b in before.items())
                        dialog.locator('.tree-close').click();added.unlink()
                        print('PASS copy scope parity and changed-membership refusal before any deletion',flush=True)
                    dialog=open_tree()
                    if not restart:
                        proc=corpus.root/'proc';(proc/'stat').write_text('btime 1700000000\n')
                        proc_pid(proc,100,'codex',['codex'],1,fds={3:str(corpus.paths['a-agent'])})
                        dialog.locator('.tree-confirm').click()
                        expect(dialog.locator('.transfer-error')).to_contain_text('仍在运行',timeout=20000)
                        assert all(Path(p).read_bytes()==b for p,b in before.items())
                        assert all(Path(p).read_bytes()==b for p,b in extra_before.items())
                        shutil.rmtree(proc/'100');dialog.locator('.tree-close').click()
                        print('PASS running hidden agent refuses the entire mixed-provider tree',flush=True)
                        dialog=open_tree()
                    meta=metadata.read_bytes()
                    values=[]
                    page.expose_function('treeProgress',lambda value:values.append(value))
                    page.evaluate('''() => {const bar=document.querySelector('#delete-tree-dialog [role=progressbar]'); new MutationObserver(()=>window.treeProgress(Number(bar.getAttribute('aria-valuenow')))).observe(bar,{attributes:true,attributeFilter:['aria-valuenow']});}''')
                    if restart:
                        def lose_reply(route):
                            route.fetch();route.abort('failed')
                        page.route('**/api/session/tree/delete',lose_reply,times=1)
                    started=time.monotonic()
                    with page.expect_request('**/api/session/tree/delete') as outgoing:dialog.locator('.tree-confirm').click()
                    receipts.append(outgoing.value.post_data_json)
                    expect(dialog.locator('.transfer-progress-label')).to_have_text('整棵会话树已移入回收站',timeout=20000)
                    elapsed=time.monotonic()-started
                    print(f'INFO confirmed tree delete to completed UI: {elapsed:.3f}s',flush=True)
                    assert values and values==sorted(values) and values[-1]==100,values
                    for key,p in corpus.paths.items():
                        assert p.exists()==(key=='unrelated'),(key,p)
                    assert all(not Path(p).exists() for p in extra_before),[p for p in extra_before if Path(p).exists()]
                    assert (corpus.root/'codex/session_index.jsonl').read_bytes()==names
                    assert (corpus.root/'state/session-metadata.json').read_bytes()==meta
                    entries=[p for p in trash.iterdir() if p.is_dir()]
                    assert len(entries)==1,entries
                    manifest=json.loads((entries[0]/'manifest.json').read_text())
                    assert len(manifest['sessions'])==15,manifest
                    repeat=page.request.post(f'http://127.0.0.1:{hub.port}/api/session/tree/delete',data=receipts[-1]);assert repeat.ok,repeat.text()
                    assert repeat.json()['phase']=='complete'
                    dialog.locator('.tree-close').click()
                    # Open trash through the page and restore the whole tree in one click.
                    if restart:page.locator('#header-more-btn').click()
                    page.locator('#trash').click()
                    item=page.locator('.trash-item');expect(item).to_have_count(1)
                    expect(item).to_contain_text('会话树 · 14 个会话')
                    item.get_by_role('button',name='恢复整棵树',exact=True).click()
                    expect(item).to_have_count(0,timeout=20000)
                    assert all(Path(p).read_bytes()==b for p,b in before.items())
                    assert (corpus.root/'codex/session_index.jsonl').read_bytes()==names
                    assert (corpus.root/'state/session-metadata.json').read_bytes()==meta
                    assert all(Path(p).read_bytes()==b for p,b in extra_before.items())
                    print(f'PASS {"mobile" if restart else "desktop"} tree delete: hidden ancestor, sibling, physical generations, agents, unrelated retained, monotonic progress, cross-device byte-exact restore, lost response recovery',flush=True)
        finally:browser.close()
    return 0

if __name__=='__main__':raise SystemExit(main())
