#!/usr/bin/env python3
"""Configured SCP routing, relay fallback and cancellation through Chromium.
Synthetic native sessions and a deterministic fake scp; no production SSH/data.
"""
import argparse
from contextlib import ExitStack
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import tempfile
import threading
import time
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, isolated_server
from session_files_browser import fixture, uid
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN
from transfer_ui import select_target


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-transport space [x]-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root = Path(tmp)
        roots, claude, _, _ = fixture(root/'source')
        source, target = Corpus(root/'source'), Corpus(root/'target')
        for corpus in (source, target):
            for folder in ('state', 'proc', 'ids', 'trash'):
                (corpus.root/folder).mkdir(parents=True, exist_ok=True)
        roots['codex'] = str(source.root/'codex')
        originals = {p: p.read_bytes() for p in (source.root/'claude').rglob('*') if p.is_file()}
        shim = root/'bin'; shim.mkdir()
        control, calls = root/'control.json', root/'scp-calls.jsonl'
        control.write_text(json.dumps({'mode': 'ok'}))
        (shim/'scp').write_text('''#!/usr/bin/python3
import json, os, pathlib, re, shutil, sys, time
root=pathlib.Path(os.environ['SD_TRANSPORT_FIXTURE'])
mode=json.loads((root/'control.json').read_text())['mode']
source=re.sub(r'\\\\(.)', r'\\1', sys.argv[-2].split(':',1)[1]); target=pathlib.Path(sys.argv[-1])
with (root/'scp-calls.jsonl').open('a') as out: out.write(json.dumps({'args':sys.argv[1:],'mode':mode,'pid':os.getpid()})+'\\n')
assert '-s' in sys.argv and 'StrictHostKeyChecking=yes' in sys.argv
assert target.parent.name.startswith('incoming-') and target.name=='archive.tar'
assert pathlib.Path(source).name.startswith('export-')
if mode=='fail': target.write_bytes(b'partial'); sys.exit(1)
if mode=='block':
    target.write_bytes(b'partial')
    while True: time.sleep(.1)
time.sleep(.6)
shutil.copyfile(source,target)
if mode=='corrupt':
    with target.open('r+b') as out: out.write(b'BAD TAR')
''')
        (shim/'scp').chmod(0o700)
        old_path = os.environ['PATH']; old_fixture = os.environ.get('SD_TRANSPORT_FIXTURE')
        os.environ['PATH'] = str(shim)+os.pathsep+old_path
        os.environ['SD_TRANSPORT_FIXTURE'] = str(root)
        stack.callback(lambda: os.environ.__setitem__('PATH', old_path))
        stack.callback(lambda: os.environ.pop('SD_TRANSPORT_FIXTURE', None) if old_fixture is None else os.environ.__setitem__('SD_TRANSPORT_FIXTURE', old_fixture))
        target_port = free_port()
        behavior = {'old': False, 'lose': False, 'relay': 0}
        class Proxy(BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_GET(self): self.forward()
            def do_POST(self): self.forward()
            def forward(self):
                body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                if self.path.endswith('/transfer/receive'): behavior['relay'] += 1
                connection = http.client.HTTPConnection('127.0.0.1', target_port, timeout=90)
                try:
                    connection.request(self.command, self.path, body=body, headers={k:v for k,v in self.headers.items() if k.lower() not in ('host','connection')})
                    response = connection.getresponse(); data = response.read()
                    if self.path.endswith('/transfer/check') and behavior['old']:
                        value=json.loads(data);value.pop('scp_transfer',None);data=json.dumps(value).encode()
                    if self.path.endswith('/scp/receive') and behavior['lose']:
                        behavior['lose']=False; self.close_connection=True; return
                    self.send_response(response.status)
                    for key,value in response.getheaders():
                        if key.lower() not in ('transfer-encoding','content-length','connection'): self.send_header(key,value)
                    self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError): pass
                finally: connection.close()
        proxy=ThreadingHTTPServer(('127.0.0.1',0),Proxy);proxy.daemon_threads=True
        threading.Thread(target=proxy.serve_forever,daemon=True).start()
        stack.callback(proxy.server_close);stack.callback(proxy.shutdown)
        a=SimpleNamespace(name='source',nid='a'*32,port=free_port(),token=TOKEN)
        b=SimpleNamespace(name='target',nid='b'*32,port=proxy.server_port,token=TOKEN)
        for node,corpus,port in ((a,source,a.port),(b,target,target_port)):
            (corpus.root/'ids/node-id').write_text(node.nid+'\n')
            env=node_env(corpus.root,port,'127.0.0.0/8')
            env.update({f'SESSIONDOCK_{key.upper()}_ROOT':value for key,value in roots.items()})
            env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
            stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',trash_dir=corpus.root/'trash',extra_env=env))
        hubroot=root/'hub';hubroot.mkdir()
        hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[a,b])
        policy=hubroot/'transport.json'
        endpoint={'address':'127.0.0.1','user':'fixture','port':2222}
        networks=[{'name':'slow','transport':'scp','priority':1,'nodes':{a.nid:endpoint,b.nid:endpoint}},
                  {'name':'fast','transport':'scp','priority':100,'nodes':{a.nid:endpoint,b.nid:endpoint}},
                  {'name':'later-fast','transport':'scp','priority':100,'nodes':{a.nid:endpoint,b.nid:endpoint}}]
        def configure(networks):
            hub.stop(); policy.write_text(json.dumps({'default':'hub','networks':networks}))
            hub.env['SESSIONDOCK_HUB_TRANSFER_CONFIG']=str(policy);hub.start()
        configure(networks);stack.callback(hub.stop)
        browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
        context=browser.new_context(service_workers='block');stack.callback(context.close)
        page=context.new_page()
        selected=scoped(a.nid,uid('claude',claude[2]))
        def dialog():
            page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
            page.locator(f'#side .item[data-uid="{selected}"]').click()
            page.locator('#a-clone-group').click()
            result=page.locator('#clone-group-dialog')
            expect(result.locator('.clone-confirm')).to_be_enabled(timeout=15000)
            select_target(result,b.nid)
            return result
        def journal(operation): return json.loads((hubroot/'transfers'/f'{operation}.json').read_text())
        def scp_calls(): return [json.loads(line) for line in calls.read_text().splitlines()] if calls.exists() else []
        def run(mode, expected, fallback=None):
            control.write_text(json.dumps({'mode':mode}));before=len(scp_calls());relays=behavior['relay']
            panel=dialog()
            with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=30000) as response:
                panel.locator('.clone-confirm').click()
            result=response.value
            assert result.ok,result.text()
            value=result.json();saved=journal(value['operation_id'])
            assert saved['transport']['method']==expected,saved['transport']
            if fallback: assert fallback in saved['transport']['fallback'],saved['transport']
            else: assert saved['transport']['fallback'] is None,saved['transport']
            if expected=='scp':
                assert saved['transport']['network']=='fast',saved['transport']
                assert behavior['relay']==relays
                assert len(scp_calls())==before+1
            else: assert behavior['relay']==relays+1
            expect(page.locator('#msgs')).to_contain_text('Branch A final',timeout=15000)
            assert all(p.read_bytes()==raw for p,raw in originals.items())
            assert not list((target.root/'state/transfers').glob('incoming-*'))
            assert not list((source.root/'state/transfers'/value['operation_id']).glob('export-*.tar'))
            return value
        copied=run('ok','scp')
        print('PASS priority selects SCP; destination history opens; no relay bytes; originals preserved',flush=True)
        before=len(scp_calls());hub.stop();hub.start()
        result=page.request.post(f'http://127.0.0.1:{hub.port}/api/session/transfer/clone',data={'uid':selected,'target_node':b.nid,'operation_id':copied['operation_id']})
        assert result.ok and result.json()['target_uid']==copied['target_uid'],result.text()
        assert len(scp_calls())==before
        print('PASS completed transfer retry after Hub restart does not copy or publish twice',flush=True)
        behavior['lose']=True;run('ok','scp')
        print('PASS lost SCP response reconciles durable receipt without relay or duplicate copy',flush=True)
        run('fail','hub','SCP 连接或传输失败')
        print('PASS partial SCP failure is cleaned before Hub relay fallback',flush=True)
        before=len(scp_calls());behavior['old']=True;run('ok','hub','节点版本不支持');behavior['old']=False
        assert len(scp_calls())==before
        print('PASS old node capability falls back without starting SCP',flush=True)
        configure([{'name':'source-only','transport':'scp','nodes':{a.nid:endpoint}}]);before=len(scp_calls());run('ok','hub')
        assert len(scp_calls())==before
        print('PASS no common network uses unchanged Hub relay',flush=True)
        configure(networks)
        control.write_text(json.dumps({'mode':'corrupt'}));relays=behavior['relay'];panel=dialog()
        with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=30000) as response:
            panel.locator('.clone-confirm').click()
        assert not response.value.ok,response.value.text()
        assert behavior['relay']==relays
        assert all(p.read_bytes()==raw for p,raw in originals.items())
        print('PASS corrupt SCP archive fails validation without relay fallback or source changes',flush=True)
        for action in ('cancel','hub-restart'):
            control.write_text(json.dumps({'mode':'block'}));relays=behavior['relay'];before=len(scp_calls());panel=dialog()
            panel.locator('.clone-confirm').click()
            expect(panel.locator('.transfer-progress-detail')).to_contain_text('SCP 直传',timeout=15000)
            deadline=time.monotonic()+10
            while len(scp_calls())==before:
                assert time.monotonic()<deadline;page.wait_for_timeout(50)
            pid=scp_calls()[-1]['pid']
            if args.screenshots and action=='cancel':
                args.screenshots.mkdir(parents=True,exist_ok=True)
                for theme in ('light','dark'):
                    page.evaluate('(theme) => {localStorage.setItem("sessiondock.theme", theme); document.documentElement.dataset.theme = theme;}',theme)
                    for width in (1280,390):
                        page.set_viewport_size({'width':width,'height':900})
                        page.screenshot(path=str(args.screenshots/f'scp-{theme}-{width}.png'))
                page.set_viewport_size({'width':1280,'height':900})
            if action=='cancel':
                with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/cancel'),timeout=15000) as cancelled:
                    panel.locator('.transfer-abort').click()
                assert cancelled.value.ok,cancelled.value.text()
            else:
                hub.stop();hub.start()
            deadline=time.monotonic()+15
            while True:
                try: os.kill(pid,0)
                except ProcessLookupError: break
                assert time.monotonic()<deadline,'SCP process survived cancellation/recovery';page.wait_for_timeout(100)
            assert behavior['relay']==relays
            assert not list((target.root/'state/transfers').glob('incoming-*'))
            assert all(p.read_bytes()==raw for p,raw in originals.items())
            print('PASS '+action+' stops SCP, reclaims partial data and never falls back',flush=True)
        for suffix in ('prepare','receive','settle','release'):
            response=page.request.post(f'http://127.0.0.1:{hub.port}/api/session/transfer/scp/{suffix}',data={})
            assert response.status==404 and response.json()['code']=='private_transfer_route',(suffix,response.status,response.text())
        print('PASS browser cannot invoke private SCP routes',flush=True)


if __name__=='__main__': main()
