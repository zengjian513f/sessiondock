#!/usr/bin/env python3
"""Chromium cloning discovers relationships on demand, caches them and sees new edges."""

import argparse
from contextlib import ExitStack
import copy
import json
from pathlib import Path
import select
import tempfile
import time
import uuid
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, isolated_server
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN
from session_files_browser import fixture, uid, claude_row, encoded
from session_transfer_browser import ident
from session_transfer_cache_fixtures import Opens


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--unrelated',type=int,default=128,help='Unrelated native histories for index scaling')
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-transfer-cache-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root=Path(tmp);roots,claude,side,agent=fixture(root/'node');corpus=Corpus(root/'node')
        with claude[2].open('ab') as stream:
            stream.write(encoded({'type':'mode','sessionId':ident(2),'fixture_padding':'x'*(2400*1024)}))
        unrelated=[]
        for number in range(1000,1000+args.unrelated):
            path=claude[2].parent/(ident(number)+'.jsonl')
            path.write_bytes(encoded(claude_row(ident(number),'user',ident(number*10),None,'Unrelated '+str(number),cwd=str(corpus.root/'cwd')))+
                encoded(claude_row(ident(number),'assistant',ident(number*10+1),ident(number*10),'x'*(128*1024),cwd=str(corpus.root/'cwd'))))
            unrelated.append(path)
        for folder in ('state','proc','ids'):(corpus.root/folder).mkdir()
        # Normal list discovery reads headers, but has no reason to open an
        # unselected session's compacted history. Observe it before node startup.
        checkpoint_agent=corpus.root/'grok/project'/ident(14);checkpoint_agent.mkdir()
        (checkpoint_agent/'summary.json').write_bytes(encoded({'info':{'id':ident(14),'cwd':str(corpus.root/'cwd')},
            'generated_title':'Checkpoint-only agent','agent_id':ident(14)}))
        (checkpoint_agent/'chat_history.jsonl').write_bytes(encoded({'type':'user','content':'Checkpoint agent question'})+
            encoded({'type':'assistant','content':'Checkpoint agent answer'}))
        checkpoint=corpus.root/'grok/project'/ident(10)/'compaction_checkpoints'/(ident(890)+'.json')
        checkpoint.write_bytes(encoded({'compacted_history':[
            {'type':'assistant','tool_calls':[{'id':'checkpoint-resume','name':'send_subagent_message',
                'arguments':json.dumps({'subagent_id':ident(14),'message':'Review'})}]}]}))
        idle_watch=Opens([checkpoint]);stack.callback(idle_watch.close)
        cache=corpus.root/'state/transfers/relationships-v1.json'
        node=SimpleNamespace(name='source',nid='c'*32,port=free_port(),token=TOKEN)
        (corpus.root/'ids/node-id').write_text(node.nid+'\n')
        env=node_env(corpus.root,node.port,'127.0.0.0/8');env.update(SESSIONDOCK_CLAUDE_ROOT=roots['claude'],SESSIONDOCK_PROC_ROOT=str(corpus.root/'proc'),SESSIONDOCK_RUNTIME_LOG_DIR='-')
        server_log=corpus.root/'relationship-server.log'
        servers=stack.enter_context(ExitStack())
        def start():
            started=time.monotonic()
            servers.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',extra_env=env,log_path=server_log))
            elapsed=time.monotonic()-started
            print(f'Node startup to HTTP health: {elapsed:.3f}s',flush=True)
            return elapsed
        start()
        # A bounded observation window catches eager background discovery without
        # waiting for the former 60-second timer. No list/preview request yet.
        assert not select.select([idle_watch.fd],[],[],0.5)[0],'startup scanned compacted history'
        assert not cache.exists(),'startup built relationship summaries'
        hubroot=root/'hub';hubroot.mkdir();hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node]);hub.start();stack.callback(hub.stop)
        browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
        page=browser.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
        selected=scoped(node.nid,uid('claude',claude[2]))
        page.locator('#q').fill(selected)
        page.locator(f'#side .item[data-uid="{selected}"]').click()
        page.wait_for_function('(uid)=>S.sel===uid',arg=selected)
        assert not idle_watch.take(),'ordinary browsing scanned unrelated compacted history'
        assert not cache.exists(),'ordinary browsing built relationship summaries'
        watch=Opens(unrelated,shared=idle_watch);stack.callback(watch.close)
        def preview():
            started=time.monotonic()
            with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan'),timeout=30000) as planned:
                page.locator('#a-clone-group').click()
            assert planned.value.ok,planned.value.text()
            dialog=page.locator('#clone-group-dialog')
            try:expect(dialog.locator('.clone-confirm')).to_be_enabled()
            except AssertionError:
                print('Preview phase/count/error:',{k:planned.value.json().get(k) for k in ('phase','session_count','error')},flush=True)
                print('Preview dialog:',dialog.inner_text(),flush=True)
                raise
            return dialog,planned.value.json(),time.monotonic()-started
        dialog,plan,cold=preview();assert plan['session_count']==4,plan
        operation_dir=corpus.root/'state/transfers'/plan['operation_id']
        assert {p.name for p in operation_dir.iterdir()}=={'operation.json'},'preview staged native files'
        assert watch.take()=={p.name for p in unrelated},'cold preview skipped candidate histories'
        assert idle_watch.take()=={checkpoint.name},'cold preview skipped compacted relationships'
        saved=json.loads(cache.read_text())
        assert {str(p) for p in unrelated}.issubset({row[0] for row in saved})
        dialog.locator('.clone-cancel').click()
        dialog,plan,warm=preview();assert not watch.take(),'warm preview reread unrelated histories'
        deadline=time.monotonic()+3
        while True:
            records=[json.loads(line) for line in server_log.read_text().splitlines() if line.startswith('{')]
            if any(row.get('event')=='transfer.relationship_index.query' and row.get('updated')==0 for row in records):break
            assert time.monotonic()<deadline,'warm preview rebuilt the relationship graph'
            page.wait_for_timeout(50)
        print('PASS Chromium warm preview queries the reverse index with zero history updates',flush=True)
        shots=Path('target/transfer-source-index');shots.mkdir(parents=True,exist_ok=True)
        for width in (1280,390):
            page.set_viewport_size({'width':width,'height':900})
            for theme in ('light','dark'):
                page.emulate_media(color_scheme=theme)
                expect(page.locator('html')).to_have_attribute('data-theme',theme)
                dialog.screenshot(path=str(shots/f'preview-{width}-{theme}.png'))
        page.set_viewport_size({'width':1280,'height':900})
        progress_watch=Opens([corpus.root/'state/transfers'/plan['operation_id']/'operation.json'],shared=idle_watch)
        stack.callback(progress_watch.close)
        for _ in range(2):
            response=page.wait_for_event('response',predicate=lambda r:r.url.endswith('/api/session/clone/progress'))
            assert response.ok,response.text()
            assert response.json()['operation_id']==plan['operation_id']
        assert not progress_watch.take(),'foreground progress reopened the operation journal'
        print('PASS Chromium progress polls read cached state without opening the native journal',flush=True)
        started=time.monotonic()
        with page.expect_response(lambda r:r.url.endswith('/api/session/clone') and r.request.method=='POST',timeout=30000) as copied:
            dialog.locator('.clone-confirm').click()
        assert copied.value.ok,copied.value.text()
        expect(page.locator('#msgs')).to_contain_text('Branch A final')
        assert not watch.take(),'execution reread unrelated histories'
        print(f'PASS Chromium cold preview {cold:.3f}s, warm preview {warm:.3f}s, copy {time.monotonic()-started:.3f}s; unchanged unrelated histories not opened',flush=True)
        # Historical operation payloads are much larger than their lock/ownership
        # metadata. They must not be parsed again for every new small transfer.
        old=json.loads((operation_dir/'operation.json').read_text())
        old.update(phase='exported',incoming_digest=None,ownership_sequence=50)
        old['native']={'databases':[{'path':str(corpus.root/'unused.sqlite'),'tables':[
            {'name':'thread_items','columns':['thread_id','payload'],'keys':['thread_id'],
             'schema':'synthetic archived metadata','rows':[
                 {'thread_id':ident(990),'payload':json.dumps({'text':'Synthetic archived row '+str(i),
                     'items':[{'kind':'tool_result','text':'x'*128}]*4})}
                 for i in range(10000)]}]}]}
        journals=[]
        for _ in range(32):
            old['id']=str(uuid.uuid4())
            directory=corpus.root/'state/transfers'/old['id'];directory.mkdir()
            journal=directory/'operation.json';journal.write_text(json.dumps(old));journals.append(journal)
        # Let the existing abandoned-preview worker observe new terminal entries
        # once; it is independent of the on-demand lock/ownership summary cache.
        page.wait_for_timeout(11000)
        page.locator('#q').fill(selected)
        page.locator(f'#side .item[data-uid="{selected}"]').click()
        dialog,_,journal_cold=preview();dialog.locator('.clone-cancel').click()
        journal_watch=Opens(journals,shared=idle_watch);stack.callback(journal_watch.close)
        dialog,_,journal_warm=preview()
        journal_bytes=sum(p.stat().st_size for p in journals)
        print(f'Historical journals {journal_bytes/2**20:.1f} MiB: cold preview {journal_cold:.3f}s, warm preview {journal_warm:.3f}s',flush=True)
        assert not journal_watch.take(),'warm preview reopened unchanged historical operation journals'
        with page.expect_response(lambda r:r.url.endswith('/api/session/clone') and r.request.method=='POST',timeout=30000) as copied:
            dialog.locator('.clone-confirm').click()
        assert copied.value.ok,copied.value.text()
        expect(page.locator('#msgs')).to_contain_text('Branch A final')
        assert not journal_watch.take(),'ownership sequence reopened unchanged historical journals'
        current=json.loads((corpus.root/'state/transfers'/copied.value.json()['operation_id']/'operation.json').read_text())
        assert current['ownership_sequence']>50
        # Atomic replacement and deletion must invalidate lock summaries. The
        # original native sessions remain unchanged throughout these fixtures.
        changed=copy.deepcopy(old);changed.update(id=journals[0].parent.name,phase='retired',moving=True)
        replacement=journals[0].with_suffix('.tmp');replacement.write_text(json.dumps(changed));replacement.replace(journals[0])
        page.locator('#q').fill(selected)
        page.locator(f'#side .item[data-uid="{selected}"]').click()
        page.locator('#a-clone-group').click()
        expect(page.locator('#clone-group-dialog .transfer-error')).to_contain_text('会话组有尚未恢复的复制操作')
        page.locator('#clone-group-dialog .clone-cancel').click()
        assert not journal_watch.take(),'replacement reopened unchanged old inodes'
        journals[0].unlink()
        dialog,_,_=preview();dialog.locator('.clone-cancel').click()
        assert not journal_watch.take(),'deletion reopened unrelated journals'
        print('PASS demand-read journal summaries reuse old payloads, preserve ownership order and invalidate replacement/deletion',flush=True)
        # Add a reverse edge from an already cached, unrelated transcript.
        with unrelated[0].open('ab') as stream:
            stream.write(encoded({'type':'fork-context-ref','sessionId':ident(1000),'parentSessionId':ident(2)}))
        page.locator('#q').fill(selected)
        page.locator(f'#side .item[data-uid="{selected}"]').click()
        page.wait_for_function('(uid)=>S.sel===uid',arg=selected)
        watch.take()
        dialog,plan,_=preview();assert plan['session_count']==5,plan
        assert watch.take()=={unrelated[0].name},'must reread only changed unrelated history'
        dialog.locator('.clone-cancel').click()
        # The old process-only cache lost all relationships on every deployment.
        journal_stamps={p:p.stat().st_mtime_ns for p in journals if p.exists()}
        servers.close();startup=start()
        assert all(p.stat().st_mtime_ns==stamp for p,stamp in journal_stamps.items()),'startup rewrote archived journals'
        page.reload(wait_until='networkidle')
        page.wait_for_function('(id)=>Nodes.list.find(n=>n.id===id)?.online===true',arg=node.nid,timeout=20000)
        page.locator('#q').fill(selected)
        page.locator(f'#side .item[data-uid="{selected}"]').click()
        page.wait_for_function('(uid)=>S.sel===uid',arg=selected)
        watch.take() # List discovery reads native headers on process startup.
        idle_watch.take()
        stamp=cache.stat().st_mtime_ns
        dialog,plan,restarted=preview();assert plan['session_count']==5,plan
        # The transfer index is deliberately lazy now: its first preview reads
        # bounded headers. Persisted relationships must avoid checkpoint bodies.
        watch.take()
        assert not idle_watch.take(),'preview after restart reparsed cached checkpoint relationships'
        assert cache.stat().st_mtime_ns==stamp,'unchanged preview rewrote persistent index'
        dialog.locator('.clone-cancel').click()
        print(f'PASS Chromium restart health {startup:.3f}s, preview {restarted:.3f}s; archived journals preserved and relationships reused',flush=True)
        # Removing it must also invalidate the relationship cache.
        raw=unrelated[0].read_bytes();unrelated[0].write_bytes(raw[:raw.rfind(b'\n',0,len(raw)-1)+1])
        watch.take()
        dialog,plan,_=preview();assert plan['session_count']==4,plan
        assert watch.take()=={unrelated[0].name}
        print('PASS Chromium detects added and removed reverse fork edges without rescanning unchanged histories',flush=True)
        dialog.locator('.clone-cancel').click()
        # A newly created reverse ref and its deletion invalidate the graph.
        added=claude[2].parent/(ident(900000)+'.jsonl')
        added.write_bytes(encoded(claude_row(ident(900000),'user',ident(9000000),None,'New outside fork',cwd=str(corpus.root/'cwd')))+
            encoded({'type':'fork-context-ref','sessionId':ident(900000),'parentSessionId':ident(2)}))
        dialog,plan,_=preview();assert plan['session_count']==5,plan
        dialog.locator('.clone-cancel').click()
        added.unlink()
        dialog,plan,_=preview();assert plan['session_count']==4,plan
        dialog.locator('.clone-cancel').click()
        print('PASS Chromium reverse index includes newly created references and drops deleted histories',flush=True)
        grok_selected=scoped(node.nid,uid('grok',corpus.root/'grok/project'/ident(10)))
        page.goto(f'http://127.0.0.1:{hub.port}/?sid={grok_selected}&node={node.nid}',wait_until='networkidle')
        page.wait_for_function('(uid)=>S.sel===uid',arg=grok_selected)
        dialog,plan,_=preview();assert plan['session_count']==5,plan
        dialog.locator('.clone-cancel').click()
        print('PASS Chromium idle startup leaves compacted history unread; on-demand preview includes checkpoint-only agent',flush=True)
        # The ordinary native row signature need not change when only a Grok
        # checkpoint changes. Its additional dependency must invalidate the graph.
        original_checkpoint=checkpoint.read_bytes()
        checkpoint.write_bytes(encoded({'compacted_history':[]}))
        dialog,plan,_=preview();assert plan['session_count']==4,plan
        dialog.locator('.clone-cancel').click()
        checkpoint.write_bytes(original_checkpoint)
        dialog,plan,_=preview();assert plan['session_count']==5,plan
        dialog.locator('.clone-cancel').click()
        print('PASS Chromium checkpoint-only changes invalidate reverse links with unchanged list rows',flush=True)




if __name__=='__main__':main()
