#!/usr/bin/env python3
"""Real Hub/two-node Chromium cross-node clone path with shared native storage.
Each provider uses synthetic branches and agents; runtime state is node-private.
"""
import argparse
import base64
import copy
import http.client
import io
import tarfile
import shlex
import sqlite3
import subprocess
import time
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, Corpus, isolated_server
from session_clone_browser import prepare
from session_files_browser import fixture, uid
from session_transfer_browser import ident
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN


def node_call(node,path,value=None,raw=None):
    connection=http.client.HTTPConnection('127.0.0.1',node.port,timeout=45)
    data=raw if raw is not None else json.dumps(value).encode()
    connection.request('POST',path,body=data,headers={'Content-Type':'application/x-tar' if raw is not None else 'application/json',
        'X-SessionDock-Protocol':'1','X-SessionDock-Node-Token':TOKEN})
    response=connection.getresponse();result=(response.status,response.read());connection.close();return result


def rejected_bundles(source,target,operation):
    status,raw=node_call(source,'/api/session/transfer/manifest',{'operation_id':operation});assert status==200,raw
    manifest=json.loads(raw)
    wrong=copy.deepcopy(manifest);wrong['roots']['codex']+='/wrong'
    status,raw=node_call(target,'/api/session/transfer/check',wrong);assert status==409 and json.loads(raw)['code']=='move_root_mismatch',raw
    wrong=copy.deepcopy(manifest);wrong['environment'][0]['entries']['']['executable']^=1
    status,raw=node_call(target,'/api/session/transfer/check',wrong);assert status==409 and json.loads(raw)['code']=='move_cwd_mismatch',raw
    status,archive=node_call(source,'/api/session/transfer/export',{'operation_id':operation});assert status==200,archive[:200]
    try:
        with tarfile.open(fileobj=io.BytesIO(archive)) as reader:
            members=[(entry.name,reader.extractfile(entry).read()) for entry in reader]
        for bad_path in (False,True):
            output=io.BytesIO()
            with tarfile.open(fileobj=output,mode='w') as writer:
                for index,(name,data) in enumerate(members):
                    if index==1:
                        if bad_path:name='../escaped'
                        else:data=bytes([data[0]^1])+data[1:]
                    header=tarfile.TarInfo(name);header.size=len(data);writer.addfile(header,io.BytesIO(data))
            status,raw=node_call(target,'/api/session/transfer/receive',raw=output.getvalue())
            assert status==409 and json.loads(raw)['code']=='move_format',raw
        with tarfile.open(fileobj=io.BytesIO(archive)) as reader:
            first_file=reader.getmembers()[1]
            short=archive[:first_file.offset_data+first_file.size//2]
        status,raw=node_call(target,'/api/session/transfer/receive',raw=short);assert status!=200,raw
        status,raw=node_call(target,'/api/session/transfer/status',{'operation_id':operation});assert status==404,raw
        print('PASS target rejects root/content mismatch, corrupt or truncated tar and traversal before publication',flush=True)
    finally:
        status,raw=node_call(source,'/api/session/transfer/release',{'operation_id':operation,'completed':False});assert status==200,raw


class Peer:
    def __init__(self,host,root,source,roots,node,binary):
        repo=Path(__file__).resolve().parents[1]
        self.process=subprocess.Popen(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10',host,
            'python3 -u '+shlex.quote(str(repo/'tests/session_transfer_peer.py'))],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
        schemas={}
        for path in (source.root/'codex').glob('*.sqlite'):
            with sqlite3.connect(path) as db:
                schemas[str(path)]=';\n'.join(row[0] for row in db.execute("SELECT sql FROM sqlite_master WHERE type IN ('table','index') AND sql IS NOT NULL") if not row[0].startswith('CREATE TABLE _sessiondock'))+';'
        config={'root':str(root),'roots':roots,'cwds':[{'path':str(source.root/('workspace' if source.paths else 'cwd')), 'mode':(source.root/('workspace' if source.paths else 'cwd')).stat().st_mode & 0o777}],
            'schemas':schemas,'node_id':node.nid,'token':TOKEN,'binary':str(binary.resolve()),'web':str(repo/'legacy-web')}
        self.process.stdin.write(json.dumps(config)+'\n');self.process.stdin.flush()
        ready=json.loads(self.process.stdout.readline())
        self.tunnel=subprocess.Popen(['ssh','-o','BatchMode=yes','-o','ExitOnForwardFailure=yes','-N',
            '-L',f'127.0.0.1:{node.port}:127.0.0.1:{ready["node_port"]}',host])
        time.sleep(.3)
        assert self.tunnel.poll() is None,'loopback SSH tunnel failed'
    def call(self,operation,**kwargs):
        self.process.stdin.write(json.dumps({'operation':operation,**kwargs})+'\n');self.process.stdin.flush()
        result=json.loads(self.process.stdout.readline());assert result['ok'],result;return result
    def read(self,path):return base64.b64decode(self.call('read',path=str(path))['bytes'])
    def append(self,path,raw):self.call('append',path=str(path),bytes=base64.b64encode(raw).decode())
    def close(self):
        try:self.call('finish')
        finally:
            self.process.stdin.close();self.process.wait(timeout=20)
            self.tunnel.terminate();self.tunnel.wait(timeout=10)


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--peer',help='Opt-in SSH peer with shared checkout; all target data stays in private /tmp directories')
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-bundle-browser-') as temporary, sync_playwright() as pw:
        root=Path(temporary)
        browser=pw.chromium.launch(headless=True,**({'executable_path':os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']} if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE') else {}))
        try:
            for provider in ('codex','claude','grok'):
                base_root=root/provider;base_root.mkdir()
                if provider=='codex':
                    source=prepare(base_root/'source');selected=source.uid('a');count=6
                    roots={kind:str(source.root/kind) for kind in ('claude','codex','grok')}
                else:
                    native,main,side,agent=fixture(base_root/'source');source=Corpus(base_root/'source')
                    (source.root/'state').mkdir();(source.root/'proc').mkdir()
                    roots={**native,'codex':str(source.root/'codex')}
                    selected=uid('claude',main[2]) if provider=='claude' else uid('grok',source.root/'grok/project'/ident(10));count=4
                destination=Corpus(base_root/'destination');destination.root.mkdir()
                for name in ('claude','codex','grok','state','proc'):(destination.root/name).mkdir()
                a=SimpleNamespace(name='source',nid='a'*32,port=free_port(),token=TOKEN)
                b=SimpleNamespace(name='destination',nid='b'*32,port=free_port(),token=TOKEN)
                for node,corpus in ((a,source),(b,destination)):
                    (corpus.root/'ids').mkdir();(corpus.root/'ids/node-id').write_text(node.nid+'\n')
                originals={str(p):p.read_bytes() for native in roots.values() for p in Path(native).rglob('*') if p.is_file() and '.sqlite' not in p.name}
                hubroot=base_root/'hub';hubroot.mkdir();hub=None;completed=None;continued=None
                peer=Peer(args.peer,base_root,source,roots,b,args.binary) if args.peer else None
                read_target=peer.read if peer else lambda p:p.read_bytes()
                for restart in (False,True):
                    with ExitStack() as stack:
                        for node,corpus in ((a,source),(b,destination)):
                            if peer and node is b:
                                peer.call('start');stack.callback(peer.call,'stop');continue
                            env=node_env(corpus.root,node.port,'127.0.0.0/8')
                            env.update({f'SESSIONDOCK_{k.upper()}_ROOT':v for k,v in roots.items()})
                            env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
                            stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',extra_env=env))
                        if hub is None:hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[a,b])
                        hub.start();stack.callback(hub.stop)
                        context=browser.new_context(service_workers='block');stack.callback(context.close)
                        page=context.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                        source_uid=scoped(a.nid,selected)
                        if restart:
                            result=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/transfer/clone',data={'uid':source_uid,'target_node':b.nid,'operation_id':completed['operation_id']})
                            assert result.ok,result.text();assert result.json()['target_uid']==completed['target_uid']
                            assert read_target(continued[0])==continued[1]
                            print('PASS '+provider+' Hub/node restart retry preserves continued target and fixed identity',flush=True)
                            continue
                        page.locator(f'#side .item[data-uid="{source_uid}"]').click()
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as planned:
                            page.locator('#a-clone-group').click()
                        if provider=='codex' and not peer:rejected_bundles(a,b,planned.value.json()['operation_id'])
                        dialog=page.locator('#clone-group-dialog')
                        expect(dialog.locator('.clone-members tbody tr')).to_have_count(count,timeout=20000)
                        dialog.locator('#transfer-target').select_option(b.nid)
                        expect(dialog.locator('#transfer-new-ids')).to_be_checked()
                        expect(dialog.locator('.clone-confirm')).to_be_enabled()
                        with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as response:
                            dialog.locator('.clone-confirm').click()
                        reply=response.value;assert reply.ok,reply.text();completed=reply.json()
                        assert completed['phase']=='complete' and b.nid in completed['target_uid']
                        page.wait_for_function('(uid)=>S.sel===uid',arg=completed['target_uid'],timeout=30000)
                        expect(page.locator('#msgs')).to_contain_text({'codex':'Branch A current','claude':'Branch A final','grok':'Grok answer 10'}[provider])
                        source_op=json.loads((source.root/'state/transfers'/completed['operation_id']/'operation.json').read_text())
                        target_op=json.loads(read_target(destination.root/'state/transfers'/completed['operation_id']/'operation.json'))
                        assert source_op['phase']=='exported' and target_op['phase']=='complete' and target_op['incoming_digest']
                        assert all(Path(p).read_bytes()==raw for p,raw in originals.items())
                        if provider=='codex':
                            agent=target_op['plan']['identities']['threads'][ident(7)]
                            page.locator('#a-view-switch').click();page.locator(f'#session-view-menu button[data-agent="{agent}"]').click()
                            expect(page.locator('#msgs')).to_contain_text('A agent answer')
                            changed=Path(roots['codex'])/target_op['staged']['files'][0]['relative']
                        elif provider=='claude':
                            new_agent=target_op['file_plan']['sessions']['claude:'+agent]
                            page.locator('#a-view-switch').click();page.locator(f'#session-view-menu button[data-agent="{new_agent}"]').click()
                            expect(page.locator('#msgs')).to_contain_text('Agent answer')
                            changed=next(Path(f['target']) for f in target_op['file_publications'] if f['target'].endswith('.jsonl'))
                        else:
                            child=target_op['file_plan']['sessions']['grok:'+ident(13)]
                            child_path=Path(roots['grok'])/'project'/child
                            page.locator(f'#side .item[data-uid="{scoped(b.nid,uid("grok",child_path))}"]').click()
                            expect(page.locator('#msgs')).to_contain_text('Grok answer 13')
                            changed=child_path/'chat_history.jsonl'
                        if peer:peer.append(changed,b'\n')
                        else:changed.write_bytes(changed.read_bytes()+b'\n')
                        continued=(changed,read_target(changed))
                        print('PASS '+provider+' Chromium selects another node, streams whole family, publishes and opens clone/agent; source intact',flush=True)
                if peer:peer.close()
        finally:
            if 'peer' in locals() and peer and peer.process.poll() is None:peer.close()
            browser.close()


if __name__=='__main__':main()
