#!/usr/bin/env python3
"""A stalled transfer must not serialize other groups or its own cancellation."""
import argparse
from transfer_ui import select_target
from contextlib import ExitStack
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import io
import tarfile
from pathlib import Path
import tempfile
import threading
import time
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, isolated_server
from session_files_browser import fixture, uid
from session_transfer_browser import ident
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN
from session_bundle_browser import node_call
from session_transfer_cache_fixtures import Opens


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--screenshots',type=Path)
    args=parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-transfer-isolation-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root=Path(tmp);roots,claude,_,_=fixture(root/'source');source=Corpus(root/'source');destination=Corpus(root/'destination')
        for corpus in (source,destination):
            for name in ('state','proc','ids','trash'): (corpus.root/name).mkdir(parents=True)
        roots['codex']=str(source.root/'codex')
        unrelated=source.root/'grok/project'/ident(900)
        (unrelated/'compaction_checkpoints').mkdir(parents=True)
        (unrelated/'summary.json').write_text(json.dumps({'info':{'id':ident(900),'cwd':str(source.root/'cwd')},'generated_title':'Unrelated target history'}))
        (unrelated/'chat_history.jsonl').write_text('{"type":"user","content":"Unrelated"}\n')
        checkpoint=unrelated/'compaction_checkpoints/unrelated.json'
        checkpoint.write_text(json.dumps({'compacted_history':[{'type':'user','content':'unrelated '*10000}]}))
        # Neither unreadable/dangling project content nor a huge sparse build
        # output is part of a session migration.
        cwd=source.root/'cwd';(cwd/'dangling-project').symlink_to(root/'missing')
        with (cwd/'build-output').open('wb') as out:out.truncate(8*1024**3)
        huge_atime=(cwd/'build-output').stat().st_atime_ns
        real_port=free_port();blocked=threading.Event();release=threading.Event();held={'id':None,'phase':'manifest'}
        wire={'framing':None,'relayed':0}
        class Proxy(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_GET(self):self.forward()
            def do_POST(self):self.forward()
            def forward(self):
                body=self.rfile.read(int(self.headers.get('Content-Length','0')))
                operation=json.loads(body).get('operation_id') if body and self.headers.get('Content-Type','').startswith('application/json') else None
                pause = self.path.endswith('/transfer/manifest') if held['phase']=='manifest' else self.path.endswith('/transfer/reserve') and (destination.root/'state/transfers'/str(operation)/'operation.json').exists()
                if pause and (held['id'] is None or operation==held['id']):
                    blocked.set();release.wait(120)
                remote=http.client.HTTPConnection('127.0.0.1',real_port,timeout=35)
                try:
                    remote.request(self.command,self.path,body=body,headers={k:v for k,v in self.headers.items() if k.lower() not in ('host','connection')})
                    response=remote.getresponse();data=response.read();self.send_response(response.status)
                    for key,value in response.getheaders():
                        if key.lower() not in ('transfer-encoding','content-length','connection'):self.send_header(key,value)
                    framing=wire['framing'] if self.path.endswith('/transfer/manifest') and response.status==200 and operation!=held['id'] else None
                    if framing:
                        # A native projection's numeric byte arrays are cheap on
                        # the wire but expensive as serde_json::Value. Unknown
                        # fields also exercise forwarding without Hub rewriting.
                        prefix=data.rstrip()[:-1]+b',"wire_padding":['
                        block=b'0,'*32768;blocks=1152;suffix=b'0]}'
                        size=len(prefix)+len(block)*blocks+len(suffix)
                        self.send_header('Transfer-Encoding','chunked') if framing=='chunked' else self.send_header('Content-Length',str(size))
                        self.end_headers()
                        def send(part):
                            if framing=='chunked':self.wfile.write(('%x\r\n'%len(part)).encode())
                            self.wfile.write(part)
                            if framing=='chunked':self.wfile.write(b'\r\n')
                        send(prefix)
                        for _ in range(blocks):send(block)
                        send(suffix)
                        if framing=='chunked':self.wfile.write(b'0\r\n\r\n')
                        wire['relayed']+=1
                    else:
                        self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
                except (BrokenPipeError,ConnectionResetError,http.client.RemoteDisconnected):pass
                finally:remote.close()
        proxy=ThreadingHTTPServer(('127.0.0.1',0),Proxy);proxy.daemon_threads=True
        threading.Thread(target=proxy.serve_forever,daemon=True).start()
        stack.callback(proxy.server_close);stack.callback(proxy.shutdown);stack.callback(release.set)
        a=SimpleNamespace(name='source',nid='a'*32,port=proxy.server_port,token=TOKEN)
        b=SimpleNamespace(name='target',nid='b'*32,port=free_port(),token=TOKEN)
        for node,corpus,port in ((a,source,real_port),(b,destination,b.port)):
            (corpus.root/'ids/node-id').write_text(node.nid+'\n')
            env=node_env(corpus.root,port,'127.0.0.0/8');env.update({f'SESSIONDOCK_{key.upper()}_ROOT':value for key,value in roots.items()})
            env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
            stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',trash_dir=corpus.root/'trash',extra_env=env))
        hubroot=root/'hub';hubroot.mkdir();hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[a,b]);hub.start();stack.callback(hub.stop)
        browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
        context=browser.new_context(service_workers='block');stack.callback(context.close)
        def open_dialog(page,selected,moving):
            page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
            page.locator(f'#side .item[data-uid="{scoped(a.nid,selected)}"]').click()
            page.locator('#a-clone-group').click()
            dialog=page.locator('#clone-group-dialog');expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=15000)
            select_target(dialog, b.nid)
            if moving:
                requests=[]
                page.on('request',lambda request:requests.append(request.url))
                for choice in ('move','clone','move'):
                    dialog.locator(f'input[name="transfer-mode"][value="{choice}"]').check()
                    expect(dialog.locator('.clone-confirm')).to_have_text('移动整组' if choice=='move' else '复制整组')
                    expect(dialog.locator('.clone-confirm')).to_be_enabled()
                page.wait_for_timeout(250)
                assert not any('/clone/plan' in url for url in requests),requests
                print('PASS mode switches stay enabled, display the action, and make no planning requests',flush=True)
            expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=15000)
            return dialog
        compatibility=context.new_page();check=open_dialog(compatibility,uid('grok',source.root/'grok/project'/ident(10)),False)
        check.locator('#transfer-new-ids').uncheck()
        def old_plan(route):
            response=route.fetch();data=response.json();data.pop('new_ids',None)
            route.fulfill(response=response,json=data)
        compatibility.route('**/api/session/clone/plan',old_plan,times=1)
        check.locator('.clone-confirm').click()
        expect(check.locator('.transfer-error')).to_contain_text('源机器版本尚不支持保留 UID')
        check.locator('#transfer-new-ids').check()
        expect(check.locator('.clone-confirm')).to_be_enabled()
        check.locator('.clone-cancel').click()
        compatibility.close()
        print('PASS unsupported final options fail before execution; changing options allows retry',flush=True)
        first=context.new_page();dialog=open_dialog(first,uid('claude',claude[2]),True)
        with first.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as planned:
            dialog.locator('.clone-confirm').click()
        held['id']=planned.value.json()['operation_id']
        assert blocked.wait(5),'first transfer never reached preparation'
        expect(dialog.locator('.transfer-progress')).to_have_attribute('data-phase','preparing',timeout=10000)
        unrelated_watch=Opens([checkpoint]);stack.callback(unrelated_watch.close)
        for framing in ('length','chunked'):
            wire['framing']=framing
            second=context.new_page();other=open_dialog(second,uid('grok',source.root/'grok/project'/ident(10)),False)
            with second.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=30000) as copied:
                other.locator('.clone-confirm').click()
            assert copied.value.ok,copied.value.text()
            assert not release.is_set()
            expect(second.locator('#msgs')).to_contain_text('Grok answer 10')
            memory=Path(f'/proc/{hub.process.pid}/status').read_text()
            high_kib=int(next(line.split()[1] for line in memory.splitlines() if line.startswith('VmHWM:')))
            assert high_kib<200*1024,(framing,high_kib)
            print(f'PASS Chromium copy with 72 MiB numeric manifest, {framing} framing; Hub peak {high_kib} KiB',flush=True)
            second.close()
        assert wire['relayed']==2,wire
        wire['framing']=None
        assert not unrelated_watch.take(),'target publication scanned unrelated checkpoint history'
        target_cache=json.loads((destination.root/'state/transfers/relationships-v1.json').read_text())
        assert str(unrelated/'chat_history.jsonl') not in {row[0] for row in target_cache},'target rebuilt whole-store relationships'
        print('PASS cold target verifies only delivered histories without scanning unrelated checkpoints',flush=True)
        assert (cwd/'build-output').stat().st_atime_ns==huge_atime
        print('PASS independent group finishes while another transfer is stalled; huge/unrelated cwd content is never read',flush=True)
        started=time.monotonic()
        with first.expect_response(lambda r:r.url.endswith('/api/session/transfer/cancel'),timeout=10000) as cancelled:
            dialog.locator('.transfer-abort').click()
        assert cancelled.value.ok,cancelled.value.text()
        assert time.monotonic()-started<10
        operation=json.loads((source.root/'state/transfers'/held['id']/'operation.json').read_text())
        assert operation['phase']=='aborted'
        assert all(path.is_file() for path in claude.values())
        print('PASS in-flight Chromium cancellation bypasses execution lock and restores source before stalled request is released',flush=True)
        release.set()
        first.close()
        for action in ('cancel-copy','close-copy','background-copy','crash-copy','restart-hub'):
            # Hold the source's final recheck after a complete bundle was received,
            # before publishing anything. Both nodes must reclaim their payloads.
            blocked.clear();release.clear();held.update(id=None,phase='received')
            page=context.new_page();dialog=open_dialog(page,uid('claude',claude[2]),False)
            # The UI already owns its own preview; capture the request it submits.
            submitted=[]
            page.on('request',lambda request:submitted.append(request.post_data_json) if request.url.endswith('/api/session/transfer/clone') else None)
            dialog.locator('.clone-confirm').click()
            assert blocked.wait(10),action
            assert submitted,action
            operation=submitted[-1]['operation_id'];held['id']=operation
            target_dir=destination.root/'state/transfers'/operation
            assert (target_dir/'operation.json').exists()
            assert any(path.name!='operation.json' for path in target_dir.iterdir())
            started=time.monotonic()
            if action in ('cancel-copy','close-copy'):
                with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/cancel'),timeout=10000) as cancelled:
                    dialog.locator('.transfer-abort' if action=='cancel-copy' else '.transfer-close').click()
                assert cancelled.value.ok,cancelled.value.text()
                assert time.monotonic()-started<10
                if action=='close-copy':expect(dialog).to_have_count(0)
            elif action in ('background-copy','crash-copy'):
                # Model suspended background polling without weakening the old
                # 30-second deadline: server work must own its own lifetime.
                if action=='crash-copy':page.close()
                else:page.route('**/api/session/transfer/progress',lambda route:route.abort())
                time.sleep(36)
                journal=hubroot/'transfers'/f'{operation}.json'
                assert json.loads(journal.read_text())['phase'] not in ('aborted','aborting'),action
                release.set()
                deadline=time.monotonic()+20
                while json.loads(journal.read_text())['phase']!='complete':
                    assert time.monotonic()<deadline,(action,json.loads(journal.read_text())['phase'])
                    time.sleep(.1)
                if action=='background-copy':
                    page.unroute('**/api/session/transfer/progress')
                    expect(dialog).to_have_count(0,timeout=15000)
                    result=json.loads(journal.read_text())['result']
                    expect(page.locator('#side .item.sel')).to_have_attribute('data-uid',result['target_uid'])
                    page.close()
                assert all(path.is_file() for path in claude.values())
                print('PASS '+action+' completes after 36 seconds without browser progress polling',flush=True)
                continue
            else:
                if action=='restart-hub':hub.stop();hub.start()
                else:page.close()
                deadline=time.monotonic()+45
                while json.loads((hubroot/'transfers'/f'{operation}.json').read_text())['phase']!='aborted':
                    assert time.monotonic()<deadline,action
                    if action=='restart-hub':page.wait_for_timeout(100)
                    else:time.sleep(.1)
                if action=='restart-hub':
                    expect(dialog.locator('.transfer-error')).to_contain_text('本次操作已撤回，未完成迁移',timeout=15000)
                    expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=15000)
                    if args.screenshots:
                        for width,theme in ((1440,'dark'),(390,'light')):
                            page.set_viewport_size({'width':width,'height':900})
                            page.evaluate("theme => {document.documentElement.dataset.theme=theme;}",theme)
                            page.screenshot(path=str(args.screenshots/f'aborted-{width}-{theme}.png'),full_page=True)
            for directory in (source.root/'state/transfers'/operation,target_dir):
                assert json.loads((directory/'operation.json').read_text())['phase']=='aborted',action
                assert {path.name for path in directory.iterdir()}=={'operation.json','startup-index-v1.json'},(action,list(directory.iterdir()))
            assert all(path.is_file() for path in claude.values())
            assert not list((source.root/'claude').rglob('.sessiondock-*.pending'))
            print('PASS '+action+' reclaims both staged copies and preserves source; elapsed '+str(round(time.monotonic()-started,2))+'s',flush=True)
            release.set()
            if not page.is_closed():page.close()
        direct=SimpleNamespace(port=real_port)
        status,raw=node_call(direct,'/api/session/clone/plan',{'uid':uid('grok',source.root/'grok/project'/ident(10))})
        assert status==200,raw
        operation=json.loads(raw)['operation_id']
        status,archive=node_call(direct,'/api/session/transfer/export',{'operation_id':operation})
        assert status==200,archive[:200]
        with tarfile.open(fileobj=io.BytesIO(archive)) as package:
            first=package.next()
            prefix=first.offset_data+((first.size+511)//512)*512
        receiving=http.client.HTTPConnection('127.0.0.1',b.port,timeout=5)
        try:
            receiving.putrequest('POST','/api/session/transfer/receive')
            receiving.putheader('Content-Length',str(len(archive)))
            receiving.putheader('X-SessionDock-Protocol','1')
            receiving.putheader('X-SessionDock-Node-Token',TOKEN)
            receiving.endheaders(archive[:prefix])
            deadline=time.monotonic()+5
            while not list((destination.root/'state/transfers').glob('incoming-*/receiving')):
                assert time.monotonic()<deadline,'receiver did not start reading file payloads'
                time.sleep(.02)
            status,raw=node_call(b,'/api/session/transfer/interrupt',{'operation_id':operation})
            assert status==200,raw
            response=receiving.getresponse()
            assert response.status>=400,response.read()
            response.read()
            assert not list((destination.root/'state/transfers').glob('incoming-*'))
            print('PASS node interrupt stops a real receive worker waiting for payload bytes and removes temporary data',flush=True)
        finally:receiving.close()


if __name__=='__main__':main()
