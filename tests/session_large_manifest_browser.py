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
    parser.add_argument('--local',action='store_true',help='Verify same-node copying instead of cross-node transfer')
    parser.add_argument('--progress',action='store_true',help='Add large related histories and verify live work counters in the dialog')
    parser.add_argument('--peer',help='Optional SSH peer with shared checkout; move between private local filesystems')
    args=parser.parse_args()
    if args.local and args.peer:parser.error('--local and --peer are mutually exclusive')
    with tempfile.TemporaryDirectory(prefix='sessiondock-large-manifest-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root=Path(tmp);source=prepare(root/'source');destination=Corpus(root/'destination')
        if args.progress:
            row=json.dumps({'type':'event_msg','payload':{'type':'token_count','info':{'progress_fixture':'x'*16384}}})+'\n'
            for key in ('a','b','grandchild'):
                with source.paths[key].open('a') as stream:
                    for _ in range(2048):stream.write(row)
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
        telemetry=[]
        def observe(response):
            if response.url.endswith('/progress') and response.ok:
                data=response.json()
                if data.get('work'):telemetry.append((time.monotonic(),data))
        page.on('response',observe)
        page.add_init_script("""document.addEventListener('DOMContentLoaded',()=>{
          window.progressPaints=[];
          let previous='';
          new MutationObserver(records=>{
            const removed=records.flatMap(r=>[...r.removedNodes]).find(n=>n.id==='clone-group-dialog');
            const box=document.querySelector('.transfer-progress:not([hidden])') || removed?.querySelector('.transfer-progress:not([hidden])');
            if(!box)return;
            const bar=box.querySelector('[role=progressbar]');
            const detail=box.querySelector('.transfer-progress-detail')?.textContent;
            const key=box.dataset.phase+'|'+detail+'|'+bar.getAttribute('aria-valuenow');
            if(key===previous)return;previous=key;
            window.progressPaints.push({at:performance.now(),phase:box.dataset.phase,detail,
              percent:bar.getAttribute('aria-valuenow'),text:bar.getAttribute('aria-valuetext'),
              width:bar.querySelector('i').style.width});
          }).observe(document.body,{subtree:true,childList:true,attributes:true,characterData:true});
        });""")
        errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
        page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
        selected=scoped(a.nid,source.uid('a'))
        page.locator(f'#side .item[data-uid="{selected}"]').click()
        page.locator('#a-clone-group').click();dialog=page.locator('#clone-group-dialog')
        expect(dialog.locator('.clone-confirm')).to_be_enabled()
        dialog.locator('#transfer-target').select_option(a.nid if args.local else b.nid)
        if peer:
            dialog.locator('.transfer-segments label').nth(1).click()
            dialog.locator('#transfer-new-ids').check()
        expect(dialog.locator('.clone-confirm')).to_be_enabled()
        started=time.monotonic()
        endpoint='/api/session/clone' if args.local else '/api/session/transfer/clone'
        with page.expect_response(lambda r:r.url.endswith(endpoint) and r.request.method=='POST',timeout=120000) as done:
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
        if args.progress:
            paints=page.evaluate('window.progressPaints')
            report=Path('target/session-transfer-progress-report.json');report.parent.mkdir(exist_ok=True)
            report.write_text(json.dumps(paints,ensure_ascii=False,indent=2))
            quantified=[p for p in paints if p['detail'] and p['percent'] is not None]
            assert len(quantified)>5,paints
            for paint in quantified:
                assert 0<=float(paint['percent'])<=100,paint
                assert paint['width']==paint['percent']+'%',paint
                assert paint['detail'] in paint['text'] and '总进度' in paint['text'],paint
            assert any('复制相关历史' in p['detail'] for p in paints),paints
            assert any('→' in p['detail'] and p['percent'] is not None for p in paints),paints
            assert any('保存迁移记录' in p['detail'] for p in paints),paints
            if not args.local:assert any(p['phase']=='rechecking' and p['detail'] for p in paints),paints
            amounts=[float(p['percent']) for p in paints]
            assert amounts==sorted(amounts),('overall progress went backwards',paints)
            assert all(float(p['percent'])<100 for p in paints if p['phase']!='complete'),paints
            assert paints[-1]['phase']=='complete' and amounts[-1]==100,paints
            print('PASS continuous overall progress through files and phases; 100% only on completion',flush=True)
            # Actual bytes advance repeatedly inside one step, not just phase changes.
            counters={}
            for at,data in telemetry:
                for step in data['work']:
                    key=(data['phase'],step['label'])
                    counters.setdefault(key,set()).add(step['done'])
            changing={str(k):len(v) for k,v in counters.items() if len(v)>=2}
            assert changing,counters
            running=[p for p in paints if p['phase'] not in ('planning','planned','complete') and p['detail']]
            gaps=[b['at']-a['at'] for a,b in zip(running,running[1:])]
            gaps.sort()
            print('PROGRESS '+json.dumps({'paints':len(paints),'changing_steps':changing,
                'gap_p95_ms':round(gaps[int((len(gaps)-1)*.95)],1) if gaps else None,
                'gap_max_ms':round(max(gaps),1) if gaps else None},ensure_ascii=False),flush=True)
        print(f'PASS Chromium {"move" if peer else "copy"}: native before/after {native_bytes/1024/1024:.1f} MiB, target history and exact native payload intact, {time.monotonic()-started:.2f}s',flush=True)


if __name__=='__main__':main()
