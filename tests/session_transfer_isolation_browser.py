#!/usr/bin/env python3
"""A stalled transfer must not serialize other groups or its own cancellation."""
import argparse
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
from history_parity import BINARY, Corpus, isolated_server
from session_files_browser import fixture, uid
from session_transfer_browser import ident
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN
from session_bundle_browser import node_call


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-transfer-isolation-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root=Path(tmp);roots,claude,_,_=fixture(root/'source');source=Corpus(root/'source');destination=Corpus(root/'destination')
        for corpus in (source,destination):
            for name in ('state','proc','ids','trash'): (corpus.root/name).mkdir(parents=True)
        roots['codex']=str(source.root/'codex')
        # Neither unreadable/dangling project content nor a huge sparse build
        # output is part of a session migration.
        cwd=source.root/'cwd';(cwd/'dangling-project').symlink_to(root/'missing')
        with (cwd/'build-output').open('wb') as out:out.truncate(8*1024**3)
        huge_atime=(cwd/'build-output').stat().st_atime_ns
        real_port=free_port();blocked=threading.Event();release=threading.Event();held={'id':None}
        class Proxy(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_GET(self):self.forward()
            def do_POST(self):self.forward()
            def forward(self):
                body=self.rfile.read(int(self.headers.get('Content-Length','0')))
                if self.path.endswith('/transfer/manifest') and (held['id'] is None or json.loads(body).get('operation_id')==held['id']):
                    blocked.set();release.wait(30)
                remote=http.client.HTTPConnection('127.0.0.1',real_port,timeout=35)
                try:
                    remote.request(self.command,self.path,body=body,headers={k:v for k,v in self.headers.items() if k.lower() not in ('host','connection')})
                    response=remote.getresponse();data=response.read();self.send_response(response.status)
                    for key,value in response.getheaders():
                        if key.lower() not in ('transfer-encoding','content-length','connection'):self.send_header(key,value)
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
            dialog.locator('#transfer-target').select_option(b.nid)
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
        second=context.new_page();other=open_dialog(second,uid('grok',source.root/'grok/project'/ident(10)),False)
        with second.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=15000) as copied:
            other.locator('.clone-confirm').click()
        assert copied.value.ok,copied.value.text()
        assert not release.is_set()
        expect(second.locator('#msgs')).to_contain_text('Grok answer 10')
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
