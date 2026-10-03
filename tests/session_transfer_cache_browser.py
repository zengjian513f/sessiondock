#!/usr/bin/env python3
"""Chromium cloning discovers relationships on demand, caches them and sees new edges."""
from browser_runtime import js
import argparse
from contextlib import ExitStack
import ctypes
import json
import os
from pathlib import Path
import select
import struct
import tempfile
import time
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, Corpus, isolated_server
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN
from session_files_browser import fixture, uid, claude_row, encoded
from session_transfer_browser import ident


class Opens:
    def __init__(self, paths):
        libc=ctypes.CDLL(None,use_errno=True)
        self.fd=libc.inotify_init1(os.O_NONBLOCK|os.O_CLOEXEC)
        assert self.fd>=0
        self.names={}
        for path in paths:
            watch=libc.inotify_add_watch(self.fd,os.fsencode(path),0x20)
            assert watch>=0
            self.names[watch]=path.name
    def take(self):
        result=set()
        while True:
            try: data=os.read(self.fd,65536)
            except BlockingIOError: return result
            offset=0
            while offset<len(data):
                watch,mask,cookie,length=struct.unpack_from('iIII',data,offset)
                assert not mask&0x4000,'inotify queue overflow'
                if mask&0x20: result.add(self.names[watch])
                offset+=16+length
    def close(self): os.close(self.fd)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-transfer-cache-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root=Path(tmp);roots,claude,side,agent=fixture(root/'node');corpus=Corpus(root/'node')
        with claude[2].open('ab') as stream:
            stream.write(encoded({'type':'mode','sessionId':ident(2),'fixture_padding':'x'*(2400*1024)}))
        unrelated=[]
        for number in range(1000,1128):
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
        env=node_env(corpus.root,node.port,'127.0.0.0/8');env.update(SESSIONDOCK_CLAUDE_ROOT=roots['claude'],SESSIONDOCK_PROC_ROOT=str(corpus.root/'proc'))
        servers=stack.enter_context(ExitStack())
        def start():
            servers.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',extra_env=env))
        start()
        # A bounded observation window catches eager background discovery without
        # waiting for the former 60-second timer. No list/preview request yet.
        assert not select.select([idle_watch.fd],[],[],0.5)[0],'startup scanned compacted history'
        assert not cache.exists(),'startup built relationship summaries'
        hubroot=root/'hub';hubroot.mkdir();hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node]);hub.start();stack.callback(hub.stop)
        browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
        page=browser.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
        selected=scoped(node.nid,uid('claude',claude[2]))
        page.locator(f'#side .item[data-uid="{selected}"]').click()
        page.wait_for_function(js('(uid)=>S.sel===uid', '(uid)=>runtime.core.state.selection.sel===uid'),arg=selected)
        assert not idle_watch.take(),'ordinary browsing scanned unrelated compacted history'
        assert not cache.exists(),'ordinary browsing built relationship summaries'
        watch=Opens(unrelated);stack.callback(watch.close)
        def preview():
            started=time.monotonic()
            with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan'),timeout=30000) as planned:
                page.locator('#a-clone-group').click()
            assert planned.value.ok,planned.value.text()
            dialog=page.locator('#clone-group-dialog')
            expect(dialog.locator('.clone-confirm')).to_be_enabled()
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
        started=time.monotonic()
        with page.expect_response(lambda r:r.url.endswith('/api/session/clone') and r.request.method=='POST',timeout=30000) as copied:
            dialog.locator('.clone-confirm').click()
        assert copied.value.ok,copied.value.text()
        expect(page.locator('#msgs')).to_contain_text('Branch A final')
        assert not watch.take(),'execution reread unrelated histories'
        print(f'PASS Chromium cold preview {cold:.3f}s, warm preview {warm:.3f}s, copy {time.monotonic()-started:.3f}s; unchanged unrelated histories not opened',flush=True)
        # Add a reverse edge from an already cached, unrelated transcript.
        with unrelated[0].open('ab') as stream:
            stream.write(encoded({'type':'fork-context-ref','sessionId':ident(1000),'parentSessionId':ident(2)}))
        page.locator(f'#side .item[data-uid="{selected}"]').click()
        page.wait_for_function(js('(uid)=>S.sel===uid', '(uid)=>runtime.core.state.selection.sel===uid'),arg=selected)
        watch.take()
        dialog,plan,_=preview();assert plan['session_count']==5,plan
        assert watch.take()=={unrelated[0].name},'must reread only changed unrelated history'
        dialog.locator('.clone-cancel').click()
        # The old process-only cache lost all relationships on every deployment.
        servers.close();start()
        page.reload(wait_until='networkidle')
        page.locator(f'#side .item[data-uid="{selected}"]').click()
        page.wait_for_function(js('(uid)=>S.sel===uid', '(uid)=>runtime.core.state.selection.sel===uid'),arg=selected)
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
        print(f'PASS Chromium restart preview {restarted:.3f}s reuses persisted relationships',flush=True)
        # Removing it must also invalidate the relationship cache.
        raw=unrelated[0].read_bytes();unrelated[0].write_bytes(raw[:raw.rfind(b'\n',0,len(raw)-1)+1])
        watch.take()
        dialog,plan,_=preview();assert plan['session_count']==4,plan
        assert watch.take()=={unrelated[0].name}
        print('PASS Chromium detects added and removed reverse fork edges without rescanning unchanged histories',flush=True)
        dialog.locator('.clone-cancel').click()
        grok_selected=scoped(node.nid,uid('grok',corpus.root/'grok/project'/ident(10)))
        page.locator(f'#side .item[data-uid="{grok_selected}"]').click()
        page.wait_for_function(js('(uid)=>S.sel===uid', '(uid)=>runtime.core.state.selection.sel===uid'),arg=grok_selected)
        dialog,plan,_=preview();assert plan['session_count']==5,plan
        dialog.locator('.clone-cancel').click()
        print('PASS Chromium idle startup leaves compacted history unread; on-demand preview includes checkpoint-only agent',flush=True)


if __name__=='__main__':main()
