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
from transfer_ui import select_target
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import urlencode, urlsplit, parse_qs
from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, Corpus, isolated_server
from frontend_paths import frontend_dir
from session_clone_browser import prepare
from session_files_browser import fixture, uid, claude_row, encoded
from session_transfer_browser import ident
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN


def node_call(node,path,value=None,raw=None):
    connection=http.client.HTTPConnection('127.0.0.1',node.port,timeout=45)
    data=raw if raw is not None else json.dumps(value).encode()
    connection.request('POST',path,body=data,headers={'Content-Type':'application/x-tar' if raw is not None else 'application/json',
        'X-SessionDock-Protocol':'1','X-SessionDock-Node-Token':TOKEN})
    response=connection.getresponse();result=(response.status,response.read());connection.close();return result


def reopen_transfer(page, hub, operation):
    page.reload(wait_until='networkidle')
    page.wait_for_function("!document.querySelector('#transfer-tasks').hidden",timeout=15000)
    button=page.locator('#transfer-tasks:visible, #a-global-transfer-tasks:visible')
    if not button.count():
        menu=page.locator('#header-more-btn:visible, #a-more:visible').last
        menu.click()
        button=page.locator('#transfer-tasks:visible, #a-global-transfer-tasks:visible')
    button.first.click()
    row=page.locator(f'#transfer-tasks-dialog tr[data-operation="{operation}"]')
    with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/progress')) as restored:
        row.get_by_role('button',name='继续处理').click()
    assert restored.value.ok,restored.value.text()
    data=restored.value.json()
    assert data['request']['operation_id']==operation and data['plan']['operation_id']==operation
    dialog=page.locator('#clone-group-dialog')
    expect(dialog.locator('#transfer-target')).to_be_disabled()
    expect(dialog.locator('#transfer-target')).to_have_js_property('value', data['request']['target_node'])
    expect(dialog.locator('.transfer-progress')).to_be_visible()
    expect(dialog.locator('.transfer-progress')).to_have_attribute('data-phase',data['phase'])
    return dialog


def rejected_bundles(source,target,operation):
    status,raw=node_call(source,'/api/session/transfer/manifest',{'operation_id':operation});assert status==200,raw
    manifest=json.loads(raw)
    wrong=copy.deepcopy(manifest);wrong['roots']['codex']+='/wrong'
    status,raw=node_call(target,'/api/session/transfer/check',wrong);assert status==409 and json.loads(raw)['code']=='move_root_mismatch',raw
    wrong=copy.deepcopy(manifest);previous=wrong['environment'][0]['cwd']
    wrong['environment'][0]['cwd']+='/missing'
    for group in (wrong['operation']['plan']['group'],wrong['operation'].get('full_group')):
        for member in (group or {}).get('members',[]):
            if member['cwd']==previous:member['cwd']=previous+'/missing'
    status,raw=node_call(target,'/api/session/transfer/check',wrong);assert status==409 and json.loads(raw)['code']=='move_cwd_missing',raw
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
    def __init__(self,host,root,source,roots,node,binary,native_codex=False):
        repo=Path(__file__).resolve().parents[1]
        self.process=subprocess.Popen(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10',host,
            'python3 -u '+shlex.quote(str(repo/'tests/session_transfer_peer.py'))],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
        schemas={}
        for path in (() if native_codex else (source.root/'codex').glob('*.sqlite')):
            with sqlite3.connect(path) as db:
                schemas[str(path)]=';\n'.join(row[0] for row in db.execute("SELECT sql FROM sqlite_master WHERE type IN ('table','index') AND sql IS NOT NULL AND name NOT GLOB 'sqlite_*' AND name NOT GLOB '_sessiondock*'"))+';'
        config={'root':str(root),'roots':roots,'cwds':[{'path':str(source.root/('workspace' if source.paths else 'cwd')), 'mode':(source.root/('workspace' if source.paths else 'cwd')).stat().st_mode & 0o777}],
            'schemas':schemas,'native_codex':native_codex,'node_id':node.nid,'token':TOKEN,'binary':str(binary.resolve()),'web':str(frontend_dir())}
        self.process.stdin.write(json.dumps(config)+'\n');self.process.stdin.flush()
        ready=json.loads(self.process.stdout.readline())
        self.native_baseline=ready.get('native_baseline')
        self.tunnel=subprocess.Popen(['ssh','-o','BatchMode=yes','-o','ExitOnForwardFailure=yes','-N',
            '-L',f'127.0.0.1:{node.port}:127.0.0.1:{ready["node_port"]}',host])
        time.sleep(.3)
        assert self.tunnel.poll() is None,'loopback SSH tunnel failed'
    def call(self,operation,**kwargs):
        self.process.stdin.write(json.dumps({'operation':operation,**kwargs})+'\n');self.process.stdin.flush()
        result=json.loads(self.process.stdout.readline());assert result['ok'],result;return result
    def read(self,path):return base64.b64decode(self.call('read',path=str(path))['bytes'])
    def append(self,path,raw):self.call('append',path=str(path),bytes=base64.b64encode(raw).decode())
    def write(self,path,raw):self.call('write',path=str(path),bytes=base64.b64encode(raw).decode())
    def close(self):
        try:self.call('finish')
        finally:
            self.process.stdin.close();self.process.wait(timeout=20)
            self.tunnel.terminate();self.tunnel.wait(timeout=10)


def seed_move_receipts(source, provider, selected, binary, conversation):
    """Finished private receipts: native binding, draft alias, old rollout and retry."""
    root=source.root;cwd=root/('workspace' if provider=='codex' else 'cwd')
    for name in ('host','lifecycle'):(root/name).mkdir(mode=0o700)
    initialized=subprocess.run([str(binary.resolve()),'--initialize-lifecycle',str(root/'lifecycle')],capture_output=True,timeout=15)
    assert initialized.returncode==0,initialized.stderr.decode()
    path=root/'lifecycle/lifecycle-ledger.json';doc=json.loads(path.read_text())
    sid=ident(10 if provider=='grok' else 2)
    foreign='claude' if provider=='codex' else 'codex'
    ids=[f'{1000+i:032x}' for i in range(6)]
    for i,rid in enumerate(ids):
        kind=foreign if i==4 else provider
        binding={'spec':{'source':provider,'sid':sid,'uid':selected},'state':'confirmed',
            'method':'operator','evidence':None,'bound_at':1} if i in (0,3) else None
        launch={'kind':'resume','sid':sid,'uid':kind+':eeeeeeeeeeeeeeee'} if i in (2,4) else {'kind':'new_pending' if kind=='codex' else 'fixed'}
        doc['records'][rid]={'record_id':rid,'request_id':'move-fixture-'+rid,
            'spec':{'source':kind,'adapter_id':'synthetic-move','cwd':str(cwd),'launch':launch},
            'launch_id':rid,'instance_id':rid,'host_name':'sessiondock-'+rid,'revision':i+1,
            'state':'exited','failure':None,'cancel_requested':False,'binding':binding,'session_id':None,
            'created_at':int(time.time())-10,'finished_at':int(time.time())-1,'discarded':i==3}
    doc['revision']=6;path.write_text(json.dumps(doc))
    drafts=json.loads(conversation.read_text())
    drafts['aliases']={selected:'launch:'+ids[1]}
    drafts['drafts']['launch:'+ids[1]]=drafts['drafts'].pop(selected)
    unrelated=(source.uid('unrelated') if provider=='codex' else
        uid('grok',root/'grok/project'/ident(10)) if provider=='claude' else
        uid('claude',root/'claude/projects/project'/(ident(1)+'.jsonl')))
    drafts['aliases'][unrelated]='launch:'+ids[2]
    drafts['drafts']['launch:'+ids[2]]={'revision':1,'value':{'text':'shared with unrelated native session'}}
    for i in (0,3,4,5):drafts['drafts']['launch:'+ids[i]]={'revision':1,'value':{'text':'receipt draft '+str(i)}}
    conversation.write_text(json.dumps(drafts))
    launcher=root/'launcher.json'
    launcher.write_text(json.dumps({'host_binary':str(binary.resolve().with_name('ptyhost')),'host_dir':str(root/'host'),
        'adapters':[{'id':'synthetic-move','source':provider,'executable':'/bin/sh','args':['-c','exit 0'],'env':{'PATH':'/usr/bin:/bin'}}]}))
    return ids,{'host_dir':root/'host','lifecycle_dir':root/'lifecycle','launcher_config':launcher,'file_write_roots':(root,)}


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--peer',help='Opt-in SSH peer with shared checkout; all target data stays in private /tmp directories')
    parser.add_argument('--preserve',action='store_true',help='Exercise identity-preserving cross-node copies')
    parser.add_argument('--move',action='store_true',help='Move groups, checking source retirement or shared-storage rejection')
    parser.add_argument('--dependencies',action='store_true',help='Check native external media/output/link contents on an SSH peer')
    args=parser.parse_args()
    if args.dependencies and not args.peer:parser.error('--dependencies requires --peer')
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
                    (corpus.root/'trash').mkdir()
                external={}
                if args.dependencies:
                    from session_dependency_fixtures import add_external_dependencies
                    external=add_external_dependencies(source,provider)
                originals={str(p):p.read_bytes() for kind in ('claude','codex','grok') for p in (source.root/kind).rglob('*') if p.is_file() and '.sqlite' not in p.name}
                original_links={p:os.readlink(p) for p in originals if Path(p).is_symlink()}
                receipt_options={};receipt_ids=[]
                if args.move:
                    ledger=source.root/'state/conversations/conversation-ledger.json'
                    ledger.parent.mkdir()
                    ledger.write_text(json.dumps({'drafts':{selected:{'revision':1,'value':{'text':'source unsent draft'}},'codex:ffffffffffffffff':{'revision':1,'value':{'text':'unrelated draft'}}}}))
                    if args.peer:receipt_ids,receipt_options=seed_move_receipts(source,provider,selected,args.binary,ledger)
                hubroot=base_root/'hub';hubroot.mkdir();hub=None;completed=None;continued=None
                peer=Peer(args.peer,base_root,source,roots,b,args.binary) if args.peer else None
                read_target=peer.read if peer else lambda p:p.read_bytes()
                if external:
                    peer.call('seed_cwd',path=str(base_root/'external'),entries=[
                        {'relative':p.name,'kind':'file','mode':0o600,'bytes':base64.b64encode(raw).decode()} for p,raw in external.items()])
                for restart in (False,True):
                    with ExitStack() as stack:
                        for node,corpus in ((a,source),(b,destination)):
                            if peer and node is b:
                                peer.call('start');stack.callback(peer.call,'stop');continue
                            env=node_env(corpus.root,node.port,'127.0.0.0/8')
                            env.update({f'SESSIONDOCK_{k.upper()}_ROOT':v for k,v in roots.items()})
                            env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
                            stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',trash_dir=corpus.root/'trash',extra_env=env,**(receipt_options if node is a else {})))
                        if hub is None:hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[a,b])
                        hub.start();stack.callback(hub.stop)
                        context=browser.new_context(service_workers='block');stack.callback(context.close)
                        page=context.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                        source_uid=scoped(a.nid,selected)
                        if restart:
                            if args.move:
                                locked,raw=node_call(a,'/api/session/clone/plan',{'uid':selected})
                                assert locked==409 and json.loads(raw)['code']=='move_recovery_required',raw
                            if args.preserve and not args.move:
                                recovered=json.loads(read_target(reused_path))
                                assert recovered['phase']=='failed',recovered['phase']
                                assert read_target(continued[0])==continued[1]
                                print('PASS '+provider+' restart compensation preserves reused files and continued history',flush=True)
                            result=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/transfer/clone',data={'uid':source_uid,'target_node':b.nid,'operation_id':completed['operation_id']})
                            assert result.ok,result.text();assert result.json()['target_uid']==completed['target_uid']
                            assert read_target(continued[0])==continued[1]
                            if args.preserve and peer and not args.move:
                                status,raw=node_call(a,'/api/session/clone/plan',{'uid':selected,'new_ids':False})
                                assert status==200,raw
                                conflict=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/transfer/clone',data={'uid':source_uid,'target_node':b.nid,'operation_id':json.loads(raw)['operation_id']})
                                assert conflict.status==409 and conflict.json()['code']=='move_conflict',conflict.text()
                                assert read_target(continued[0])==continued[1]
                                assert all(Path(p).read_bytes()==raw for p,raw in originals.items())
                                print('PASS '+provider+' changed destination rejects another preserved-ID copy without overwriting either side',flush=True)
                            print('PASS '+provider+' Hub/node restart retry preserves continued target and fixed identity',flush=True)
                            if args.move:
                                target_local=provider+':'+completed['target_uid'].split('~',1)[1]
                                status,raw=node_call(b,'/api/session/clone/plan',{'uid':target_local,'new_ids':False,'mode':'move'});assert status==200,raw
                                back=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/transfer/clone',data={'uid':completed['target_uid'],'target_node':a.nid,'operation_id':json.loads(raw)['operation_id']})
                                assert back.ok,back.text();assert back.json()['target_uid']==scoped(a.nid,target_local)
                                returned_uid=back.json()['target_uid']
                                restored=context.request.get(f'http://127.0.0.1:{hub.port}/api/session/conversation',params={'uid':returned_uid})
                                assert restored.ok,restored.text()
                                saved=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/conversation',data={'uid':returned_uid,'revision':restored.json()['draft']['revision'],'value':{'text':'new draft after moving back'}})
                                assert saved.ok,saved.text()
                                status,raw=node_call(a,'/api/session/transfer/retire',{'operation_id':completed['operation_id']})
                                assert status==200,raw
                                assert any(row['value'].get('text')=='new draft after moving back' for row in json.loads(ledger.read_text())['drafts'].values())
                                status,raw=node_call(a,'/api/session/clone/plan',{'uid':target_local});assert status==200,raw
                                peer.call('stop');peer.call('start')
                                status,raw=node_call(b,'/api/session/clone/plan',{'uid':target_local})
                                assert status==409 and json.loads(raw)['code']=='move_recovery_required',raw
                                print('PASS '+provider+' moving back reclaims the original node; older receipts cannot unlock the new source after restart',flush=True)
                            continue
                        page.locator(f'#side .item[data-uid="{source_uid}"]').click()
                        # Seed the final plan for the later interrupted-handoff fixtures.
                        # Mode controls themselves must not perform planning requests.
                        if args.move or args.preserve:
                            def final_options(route):
                                body=route.request.post_data_json
                                body.update(mode='move' if args.move else 'clone',new_ids=not args.preserve)
                                route.continue_(post_data=json.dumps(body))
                            page.route('**/api/session/clone/plan',final_options,times=1)
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as planned:
                            page.locator('#a-clone-group').click()
                        # Corrupt clone bundles reach content validation. Local move
                        # fixtures share storage and are rejected earlier; their
                        # real UI refusal and intact source are asserted below.
                        if provider=='codex' and not peer and not args.move:
                            rejected_bundles(a,b,planned.value.json()['operation_id'])
                        dialog=page.locator('#clone-group-dialog')
                        expect(dialog.locator('.clone-members tbody tr')).to_have_count(count,timeout=20000)
                        select_target(dialog, b.nid)
                        expect(dialog.locator('#transfer-new-ids')).to_be_checked()
                        if args.move:
                            dialog.locator('.transfer-segments label').nth(1).click()
                            if not args.preserve:dialog.locator('#transfer-new-ids').check()
                        elif args.preserve:
                            dialog.locator('#transfer-new-ids').uncheck()
                        move_plan=planned
                        expect(dialog.locator('.clone-confirm')).to_be_enabled()
                        if args.move and peer:
                            for interrupted in (False,True):
                                abandoned=move_plan.value.json()['operation_id']
                                status,raw=node_call(a,'/api/session/transfer/export',{'operation_id':abandoned});assert status==200,raw[:200]
                                status,raw=node_call(b,'/api/session/transfer/receive',raw=raw);assert status==200,raw
                                status,raw=node_call(b,'/api/session/clone',{'uid':selected,'operation_id':abandoned});assert status==200,raw
                                source_record=json.loads((source.root/'state/transfers'/abandoned/'operation.json').read_text())
                                source_file=Path((source_record['plan']['files'] or source_record['file_plan']['files'])[0]['source'])
                                before=source_file.read_bytes()
                                change=b'{"type":"sessiondock_fixture_continuation"}\n' if source_file.suffix=='.jsonl' else b'\n'
                                source_file.write_bytes(before+change)
                                if interrupted:
                                    target_record=json.loads(read_target(destination.root/'state/transfers'/abandoned/'operation.json'))
                                    published=(target_record['staged']['files'] or target_record['file_publications'])[0]
                                    target_file=source.root/'codex'/published['relative'] if provider=='codex' else Path(published['target'])
                                    target_before=read_target(target_file)
                                    peer.write(target_file,target_before+change)
                                    with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as cancelled:
                                        dialog.locator('.clone-confirm').click()
                                    assert cancelled.value.status==409 and cancelled.value.json()['code']=='move_recovery_required',cancelled.value.text()
                                    assert json.loads((source.root/'state/transfers'/abandoned/'operation.json').read_text())['phase']=='aborting'
                                    status,raw=node_call(a,'/api/session/transfer/switch',{'operation_id':abandoned});assert status==409,raw
                                    status,raw=node_call(a,'/api/session/clone/plan',{'uid':selected});assert status==409,raw
                                    peer.call('stop');peer.call('start')
                                    assert read_target(target_file)==target_before+change
                                    hub.stop()
                                    if provider=='claude':
                                        journal=hubroot/'transfers'/f'{abandoned}.json'
                                        old=json.loads(journal.read_text())
                                        for key in ('preview','bytes_sent','bytes_total','error'):old.pop(key,None)
                                        journal.write_text(json.dumps(old))
                                    hub.start()
                                    if provider=='codex':page.set_viewport_size({'width':390,'height':844})
                                    dialog=reopen_transfer(page,hub,abandoned)
                                    if provider=='codex':page.set_viewport_size({'width':1280,'height':720})
                                    expect(dialog.locator('.transfer-progress')).to_contain_text('撤回待完成')
                                    peer.write(target_file,target_before)
                                    source_file.write_bytes(before)
                                    with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as move_plan:
                                        dialog.locator('.transfer-abort').click()
                                else:
                                    def restore_before_replan(route):
                                        source_file.write_bytes(before)
                                        route.continue_()
                                    page.route('**/api/session/clone/plan',restore_before_replan,times=1)
                                    with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as move_plan:
                                        with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as cancelled:
                                            dialog.locator('.clone-confirm').click()
                                    assert cancelled.value.status==409 and cancelled.value.json()['code']=='move_cancelled',cancelled.value.text()
                                assert move_plan.value.ok,move_plan.value.text()
                                for node in (a,b):
                                    status,raw=node_call(node,'/api/session/transfer/status',{'operation_id':abandoned})
                                    assert status==200 and json.loads(raw)['phase']=='aborted',raw
                                status,raw=node_call(b,'/api/session/clone',{'uid':selected,'operation_id':abandoned})
                                assert status==409,raw
                                assert all(Path(p).read_bytes()==raw for p,raw in originals.items())
                                expect(dialog.locator('.clone-confirm')).to_be_enabled()
                            print('PASS '+provider+' stale handoff compensates; interrupted withdrawal preserves changed target, survives restart and resumes from Chromium',flush=True)
                        if external:
                            # Each failure is driven by the actual confirmation button.
                            # Equal-length changes prove byte comparison, not just size.
                            for path,raw in external.items():
                                peer.call('unlink',path=str(path))
                                with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as missing:
                                    dialog.locator('.clone-confirm').click()
                                assert missing.value.status==409 and missing.value.json()['code']=='move_io',missing.value.text()
                                assert str(path) in missing.value.text(),missing.value.text()
                                peer.write(path,bytes([raw[0]^1])+raw[1:])
                                with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as mismatch:
                                    dialog.locator('.clone-confirm').click()
                                assert mismatch.value.status==409 and mismatch.value.json()['code']=='move_cwd_mismatch',mismatch.value.text()
                                peer.write(path,raw)
                            assert all(p.read_bytes()==raw for p,raw in external.items())
                            print('PASS '+provider+' Chromium rejects changed external image/output/link bytes; ignores ordinary path examples',flush=True)
                        cleanup_obstruction=None
                        if args.move and args.preserve and peer and provider in ('codex','claude','grok'):
                            operation=move_plan.value.json()['operation_id']
                            for route in (f'/api/nodes/{a.nid}/api/session/transfer/switch','/api/session/transfer/switch'):
                                forbidden=context.request.post(f'http://127.0.0.1:{hub.port}'+route,data={'uid':source_uid,'operation_id':operation})
                                assert forbidden.status==404 and forbidden.json()['code']=='private_transfer_route',forbidden.text()
                            status,raw=node_call(a,'/api/session/transfer/export',{'operation_id':operation});assert status==200,raw[:200]
                            status,raw=node_call(b,'/api/session/transfer/receive',raw=raw);assert status==200,raw
                            status,raw=node_call(b,'/api/session/clone',{'uid':selected,'operation_id':operation})
                            assert status==200 and json.loads(raw)['phase']=='ready',raw
                            status,raw=node_call(b,'/api/session/clone/plan',{'uid':selected})
                            assert status==409 and json.loads(raw)['code']=='move_recovery_required',raw
                            peer.call('stop');peer.call('start')
                            status,raw=node_call(b,'/api/session/transfer/status',{'operation_id':operation})
                            assert status==200 and json.loads(raw)['phase']=='ready',raw
                            status,raw=node_call(a,'/api/session/transfer/switch',{'operation_id':operation});assert status==200,raw
                            cleanup_obstruction=source.root/'trash'/f'move-{operation}-{provider}'/'files/1'
                            cleanup_obstruction.mkdir(parents=True)
                            print('PASS target remains fenced and ready across restart before source handoff',flush=True)
                        with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as response:
                            dialog.locator('.clone-confirm').click()
                        reply=response.value
                        if cleanup_obstruction:
                            assert reply.status==409 and reply.json()['code']=='move_cleanup_pending',reply.text()
                            expect(dialog.locator('.transfer-error')).to_contain_text('服务端正在重试源端清理')
                            expect(dialog.locator('#transfer-target')).to_be_disabled()
                            with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/cancel')) as too_late:
                                dialog.locator('.transfer-abort').click()
                            assert too_late.value.status==409 and too_late.value.json()['code']=='move_cleanup_pending',too_late.value.text()
                            status,raw=node_call(b,'/api/session/transfer/status',{'operation_id':operation})
                            assert status==200 and json.loads(raw)['phase']=='ready',raw
                            print('PASS failed source retirement leaves the verified target fenced; cancellation continues committed cleanup',flush=True)
                            dialog=reopen_transfer(page,hub,operation)
                            expect(dialog.locator('.transfer-progress')).to_contain_text('源端清理待重试')
                            partial=json.loads((source.root/'state/transfers'/operation/'operation.json').read_text())
                            assert partial['phase']=='retiring'
                            manifest=json.loads((cleanup_obstruction.parent.parent/'manifest.json').read_text())
                            assert manifest['files'][0]['in_trash'] and not Path(manifest['files'][0]['origin']).exists()
                            cleanup_obstruction.rmdir()
                            # After partial retirement, a new independent CLI
                            # session can attach to a captured (now absent) parent.
                            # The sidebar relation alone must fence further deletion.
                            late_path=source.root/'codex/sessions'/('rollout-'+ident(998)+'.jsonl')
                            late_path.parent.mkdir(parents=True,exist_ok=True)
                            late_path.write_bytes(encoded({'type':'session_meta','payload':{'id':ident(998),'cwd':str(source.root/'cwd'),'timestamp':'2026-09-11T12:00:00Z'}})+
                                                  encoded({'type':'response_item','payload':{'type':'message','role':'user','content':'Late attached child'}}))
                            metadata_path=source.root/'state/session-metadata.json'
                            baseline=json.loads(metadata_path.read_text()) if metadata_path.exists() else {"schema_version":1,"revision":1,"sessions":{}}
                            attached=json.loads(json.dumps(baseline))
                            attached['revision']+=1
                            captured=next(m for m in partial['full_group']['members'] if not m['agent'])
                            late_uid=uid('codex',late_path)
                            attached['sessions'][late_uid]={'nest_parent':{'source':captured['source'],'sid':captured['sid']},'nest_initialized':True}
                            metadata_path.write_text(json.dumps(attached))
                            with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as referenced:
                                dialog.locator('.clone-confirm').click()
                            assert referenced.value.status==409 and referenced.value.json()['code']=='move_cleanup_pending',referenced.value.text()
                            assert late_path.exists() and (cleanup_obstruction.parent/'0').exists()
                            assert json.loads((source.root/'state/transfers'/operation/'operation.json').read_text())['phase']=='retiring'
                            late_path.unlink();baseline['revision']=attached['revision']+1
                            metadata_path.write_text(json.dumps(baseline))
                            print('PASS Chromium cleanup retry preserves a partially retired family with a newly attached child',flush=True)
                            if provider=='grok':
                                late=source.root/'grok/project'/ident(999);late.mkdir(parents=True)
                                (late/'summary.json').write_text(json.dumps({'info':{'id':ident(999),'cwd':str(source.root/'cwd')},'generated_title':'Late Grok ref'}))
                                agent_id=ident(13)
                                (late/'chat_history.jsonl').write_text(json.dumps({'type':'assistant','tool_calls':[{'id':'late-grok','name':'send_subagent_message','arguments':'{}'}]})+'\n')
                                (late/'updates.jsonl').write_text(json.dumps({'params':{'update':{'toolCallId':'late-grok','rawInput':{'subagent_id':agent_id}}}})+'\n')
                                with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as referenced:
                                    dialog.locator('.clone-confirm').click()
                                assert referenced.value.status==409 and referenced.value.json()['code']=='move_cleanup_pending',referenced.value.text()
                                assert (cleanup_obstruction.parent/'0').exists()
                                # Keep no target in input: the persisted native reply alone
                                # still references this agent after partial retirement.
                                (late/'updates.jsonl').write_text(json.dumps({'params':{'update':{'toolCallId':'late-grok',
                                    'rawOutput':json.dumps({'subagent_id':agent_id})}}})+'\n')
                                with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as referenced:
                                    dialog.locator('.clone-confirm').click()
                                assert referenced.value.status==409 and referenced.value.json()['code']=='move_cleanup_pending',referenced.value.text()
                                call=late/'chat_history.jsonl';call.write_text(call.read_text().replace('send_subagent_message','Bash'))
                                with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as response:
                                    dialog.locator('.clone-confirm').click()
                                reply=response.value;assert reply.ok,reply.text()
                                for path in late.iterdir():path.unlink()
                                late.rmdir()
                                print('PASS Grok late chat/update agent reference protects partially retired group; ordinary tool input does not block cleanup',flush=True)
                            elif provider=='claude':
                                agent_id=next(m['sid'] for m in partial['full_group']['members'] if m['agent'])
                                late=source.root/'claude/projects/project'/(ident(999)+'.jsonl')
                                for short in (False,True):
                                    call={'type':'tool_use','id':'late-send','name':'SendMessage','input':{'to':agent_id[:7] if short else agent_id,'message':'late'}}
                                    rows=[claude_row(ident(999),'assistant',ident(991),None,[call],cwd=str(source.root/'cwd'))]
                                    if short:
                                        result={'success':True,'resumedAgentId':agent_id}
                                        rows.append(claude_row(ident(999),'user',ident(992),ident(991),[{'type':'tool_result','tool_use_id':'late-send','content':json.dumps(result)}],toolUseResult=result,cwd=str(source.root/'cwd')))
                                    late.write_bytes(b''.join(encoded(row) for row in rows))
                                    with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as referenced:
                                        dialog.locator('.clone-confirm').click()
                                    assert referenced.value.status==409 and referenced.value.json()['code']=='move_cleanup_pending',referenced.value.text()
                                    assert late.exists() and (cleanup_obstruction.parent/'0').exists()
                                # Same shaped output from a different tool is ordinary data.
                                call['name']='Bash'
                                late.write_bytes(b''.join(encoded(row) for row in rows))
                                with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as response:
                                    dialog.locator('.clone-confirm').click()
                                reply=response.value;assert reply.ok,reply.text()
                                late.unlink()
                                print('PASS Claude partial cleanup retains late SendMessage dependencies, including resolved short IDs; ignores unrelated tool output',flush=True)
                            else:
                                late=source.root/'codex/sessions'/f'rollout-2026-10-01T00-00-00-{ident(999)}.jsonl'
                                late.write_text(json.dumps({'type':'session_meta','payload':{'id':ident(999),'forked_from_id':partial['full_group']['members'][0]['sid'],'cwd':str(source.root/'workspace'),'timestamp':'2026-10-01T00:00:00Z'}})+'\n'+json.dumps({'type':'event_msg','payload':{'type':'user_message','message':'Late fork'}})+'\n')
                                with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as referenced:
                                    dialog.locator('.clone-confirm').click()
                                assert referenced.value.status==409 and referenced.value.json()['code']=='move_cleanup_pending',referenced.value.text()
                                assert late.exists() and (cleanup_obstruction.parent/'0').exists()
                                late.unlink()
                                # Neither generation has a parent edge. Only the call in
                                # the archived generation identifies the result as an agent.
                                generations=[]
                                for index,record in enumerate((
                                    {'type':'function_call','name':'spawn_agent','call_id':'late-call','arguments':'{}'},
                                    {'type':'function_call_output','call_id':'late-call','output':json.dumps({'agent_id':partial['full_group']['members'][0]['sid']})},
                                )):
                                    folder=source.root/'codex'/('archived_sessions' if index==0 else 'sessions')
                                    folder.mkdir(exist_ok=True)
                                    path=folder/f'rollout-2026-10-02T00-00-0{index}-{ident(998)}.jsonl'
                                    path.write_text(json.dumps({'type':'session_meta','payload':{'id':ident(998),'rollout_id':ident(990+index),'cwd':str(source.root/'workspace'),'timestamp':f'2026-10-02T00:00:0{index}Z'}})+'\n'+json.dumps({'type':'response_item','payload':record})+'\n')
                                    generations.append(path)
                                with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as referenced:
                                    dialog.locator('.clone-confirm').click()
                                assert referenced.value.status==409 and referenced.value.json()['code']=='move_cleanup_pending',referenced.value.text()
                                assert all(path.exists() for path in generations) and (cleanup_obstruction.parent/'0').exists()
                                # An identical call ID in an unrelated thread is not an edge.
                                first=generations[0]
                                first.write_text(first.read_text().replace(ident(998),ident(997)))
                                page.close();hub.stop();hub.start()
                                deadline=time.monotonic()+20
                                journal=hubroot/'transfers'/f'{operation}.json'
                                while json.loads(journal.read_text())['phase']!='complete':
                                    assert time.monotonic()<deadline,journal.read_text()
                                    time.sleep(.1)
                                # Read the idempotent completed result only after the
                                # background reconciler finished without a browser.
                                reply=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/transfer/clone',data={
                                    'uid':source_uid,'target_node':b.nid,'operation_id':operation})
                                assert reply.ok,reply.text()
                                inventory=context.request.get(f'http://127.0.0.1:{hub.port}/api/sessions')
                                assert inventory.ok,inventory.text()
                                target_row=next(row for row in inventory.json()['sessions']
                                                if row['uid']==reply.json()['target_uid'])
                                share={'sid':target_row['source']+':'+target_row['sid'],'node':b.nid}
                                page=context.new_page();page.goto(f'http://127.0.0.1:{hub.port}/?'+urlencode(share),wait_until='networkidle')
                                expect(page.locator('#msgs')).to_contain_text({'codex':'Branch A current','claude':'Branch A final','grok':'Grok answer 10'}[provider])
                                assert parse_qs(urlsplit(page.url).query)=={key:[value] for key,value in share.items()},page.url
                                print('PASS committed move finishes source retirement before target activation after page and Hub restart',flush=True)
                                for path in generations:path.unlink()
                                print('PASS Chromium protects cross-generation agent results and scopes call IDs to their thread',flush=True)
                                print('PASS Chromium retains a new outside fork dependency during partial cleanup and retries without republishing target or releasing source fence',flush=True)
                        if args.move and not peer:
                            assert reply.status==409 and reply.json()['code']=='move_shared_storage',reply.text()
                            expect(dialog.locator('.transfer-error')).to_contain_text('共享会话存储')
                            assert all(Path(p).read_bytes()==raw for p,raw in originals.items())
                            status,raw=node_call(a,'/api/session/clone/plan',{'uid':selected})
                            assert status==200,raw
                            print('PASS '+provider+' Chromium rejects moving shared storage without removing source files',flush=True)
                            break
                        assert reply.ok,reply.text();completed=reply.json()
                        task_request={'uid':source_uid,'target_node':b.nid,'operation_id':completed['operation_id']}
                        progress=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/transfer/progress',data=task_request)
                        assert progress.ok and progress.json()['phase']=='complete',progress.text()
                        if not (args.move and args.preserve and peer and provider in ('codex','claude','grok')):
                            assert progress.json()['bytes_sent']==progress.json()['bytes_total']>0,progress.text()
                        pending=context.request.get(f'http://127.0.0.1:{hub.port}/api/session/transfers')
                        assert pending.ok and all(t['request']['operation_id']!=completed['operation_id'] for t in pending.json()['operations']),pending.text()
                        assert completed['phase']=='complete' and b.nid in completed['target_uid']
                        page.wait_for_function('(uid)=>S.sel===uid',arg=completed['target_uid'],timeout=30000)
                        expect(page.locator('#msgs')).to_contain_text({'codex':'Branch A current','claude':'Branch A final','grok':'Grok answer 10'}[provider])
                        source_op=json.loads((source.root/'state/transfers'/completed['operation_id']/'operation.json').read_text())
                        target_op=json.loads(read_target(destination.root/'state/transfers'/completed['operation_id']/'operation.json'))
                        assert source_op['phase']==('retired' if args.move else 'exported') and target_op['phase']=='complete' and target_op['incoming_digest']
                        if args.move:
                            # Historical ledger IDs retain their original source node.
                            old_links=[{'sid':source_uid,'node':a.nid}]
                            old_links.extend({'sid':m['source']+':'+m['sid'],'node':a.nid}
                                for m in (source_op.get('file_plan') or source_op['plan'])['group']['members'])
                            resolved=context.request.post(f'http://127.0.0.1:{hub.port}/api/sessions/resolve',data={'links':old_links})
                            assert resolved.ok,resolved.text()
                            assert all(r['status']=='found' and r['session']['node']==b.nid and not r['session']['copied']
                                for r in resolved.json()['results']),resolved.text()
                            page.goto(f'http://127.0.0.1:{hub.port}/?sid={source_uid}&node={a.nid}',wait_until='networkidle')
                            page.wait_for_function('uid=>S.sel===uid',arg=completed['target_uid'])
                            expect(page.locator('#msgs')).to_contain_text({'codex':'Branch A current','claude':'Branch A final','grok':'Grok answer 10'}[provider])
                            expect(page.locator('#session-stop-notice')).to_contain_text('已移动')
                            print('PASS '+provider+' retained original link opens moved session in Chromium',flush=True)
                        if external:
                            environment=json.loads(read_target(destination.root/'state/transfers'/completed['operation_id']/'incoming-environment.json'))
                            dependencies={path:value for snapshot in environment for path,value in snapshot['dependencies'].items()}
                            assert set(map(str,external))<=set(dependencies),dependencies
                            assert not any('/missing/' in path for path in dependencies)
                            assert not any('sessiondock_attachments' in Path(path).parts for path in dependencies)
                            source_manifest=json.loads((source.root/'state/transfers'/completed['operation_id']/'export-manifest.json').read_text())
                            assert not any('sessiondock_attachments' in json.dumps(snapshot) for snapshot in source_manifest['environment'])
                            assert not any('sessiondock_attachments' in str(file['relative']) for file in source_manifest['files'])
                            assert not (source.root/'cwd/sessiondock_attachments').exists()
                            assert all(read_target(path)==raw and path.read_bytes()==raw for path,raw in external.items())
                            print('PASS '+provider+' missing Composer upload references do not enter the dependency checks or archive',flush=True)
                        if args.move:
                            moved={f['source'] for f in source_op['plan']['files']}
                            moved.update(f['source'] for f in (source_op['file_plan'] or {}).get('files',[]))
                            assert all(not os.path.lexists(p) if p in moved else Path(p).read_bytes()==raw for p,raw in originals.items())
                            recovered={}
                            for manifest_path in (source.root/'trash').glob('move-*/manifest.json'):
                                manifest=json.loads(manifest_path.read_text());assert manifest['state']=='trashed'
                                blocked,raw=node_call(a,'/api/trash/restore',{'id':manifest['entry_id']})
                                assert blocked==409 and json.loads(raw)['code']=='move_session_locked',raw
                                for file in manifest['files']:
                                    held=manifest_path.parent/'files'/file['name']
                                    recovered[file['origin']]=os.readlink(held) if file['origin'] in original_links else held.read_bytes()
                            assert set(recovered)==moved and all(recovered[p]==(original_links[p] if p in original_links else originals[p]) for p in moved)
                            drafts=json.loads(ledger.read_text())['drafts']
                            assert selected not in drafts and drafts['codex:ffffffffffffffff']['value']['text']=='unrelated draft'
                            if receipt_ids:
                                receipts=json.loads((source.root/'lifecycle/lifecycle-ledger.json').read_text())['records']
                                assert all(receipts[rid]['discarded'] for rid in receipt_ids[:4]),receipts
                                assert all(not receipts[rid]['discarded'] for rid in receipt_ids[4:]),receipts
                                assert all('launch:'+receipt_ids[i] not in drafts for i in (0,1,3)),drafts
                                assert drafts['launch:'+receipt_ids[2]]['value']['text']=='shared with unrelated native session',drafts
                                assert all('launch:'+rid in drafts for rid in receipt_ids[4:]),drafts
                                page.reload(wait_until='networkidle')
                                pending=context.request.get(f'http://127.0.0.1:{hub.port}/api/term/list',params={'node':a.nid})
                                assert pending.ok,pending.text()
                                assert not any(str(row.get('record_id','')).endswith(rid) for row in pending.json()['pending'] for rid in receipt_ids[:4]),pending.text()
                                page.wait_for_function('uid=>S.sel===uid',arg=completed['target_uid'])
                                print('PASS '+provider+' move clears bound/aliased/old-rollout and already-discarded receipts; preserves unrelated and other-provider identities',flush=True)
                            for database in source_op['native']['databases']:
                                with sqlite3.connect(database['path']) as db:
                                    for table in database['tables']:
                                        for row in table['rows']:
                                            clause=' AND '.join('"'+key.replace('"','""')+'" IS ?' for key in table['keys'])
                                            remaining=db.execute('SELECT count(*) FROM "'+table['name']+'" WHERE '+clause,[row[key] for key in table['keys']]).fetchone()[0]
                                            assert remaining==int(table['name'] in ('projects','project_roots','thread_sections')),(table['name'],row)
                            locked,raw=node_call(a,'/api/session/clone/plan',{'uid':selected})
                            assert locked==409 and json.loads(raw)['code']=='move_recovery_required',raw
                            if not page.locator('#trash').is_visible():page.locator('#header-more-btn').click()
                            page.locator('#trash').click()
                            expect(page.locator('#trash-dialog')).to_be_visible()
                            expect(page.locator('#trash-list .trash-item')).to_have_count(1)
                            expect(page.locator('#trash-list button[data-act="restore"]')).to_be_disabled()
                            expect(page.locator('#trash-list .trash-origin')).to_contain_text('会话已迁出')
                            page.locator('#trash-done').click()
                            print('PASS '+provider+' move retires the whole source group into verified trash and keeps source fenced',flush=True)
                        else:assert all(Path(p).read_bytes()==raw for p,raw in originals.items())
                        if args.preserve and not args.move:
                            assert not completed['new_ids']
                            assert completed['target_uid']==scoped(b.nid,selected)
                            assert all(old==new for old,new in target_op['plan']['identities']['threads'].items())
                            for item in target_op.get('staged',{}).get('files',[]):
                                assert read_target(Path(roots['codex'])/item['relative'])==originals[item['source']]
                            for item in target_op['file_publications']:
                                assert read_target(Path(item['target']))==originals[item['source']]
                            # A new operation can reuse the exact existing files
                            # and native rows; its receipt must not claim them.
                            status,raw=node_call(a,'/api/session/clone/plan',{'uid':selected,'new_ids':False});assert status==200,raw
                            repeated=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/transfer/clone',data={'uid':source_uid,'target_node':b.nid,'operation_id':json.loads(raw)['operation_id']})
                            assert repeated.ok,repeated.text()
                            reused=json.loads(read_target(destination.root/'state/transfers'/repeated.json()['operation_id']/'operation.json'))
                            assert len(reused['reused_files'])==len(target_op['staged']['files'])+len(target_op['file_publications'])
                            assert target_op['rewritten']==source_op['native']
                            for db in reused['rewritten']['databases']:
                                if peer:receipt=peer.call('receipt',path=db['path'],operation_id=reused['id'])['receipt']
                                else:
                                    with sqlite3.connect(db['path']) as connection:
                                        receipt=json.loads(connection.execute('SELECT receipt FROM _sessiondock_clone_journal WHERE operation_id=?',(reused['id'],)).fetchone()[0])
                                assert all(not table['rows'] for table in receipt['inserted']['tables'])
                            # Simulate interruption after a reuse-only publication.
                            # Recovery must never delete reused files or native rows.
                            reused['phase']='verifying'
                            reused_path=destination.root/'state/transfers'/reused['id']/'operation.json'
                            raw=json.dumps(reused).encode()
                            if peer:peer.write(reused_path,raw)
                            else:reused_path.write_bytes(raw)
                            print('PASS '+provider+' preserves native identities and exact history bytes; new operation reuses existing destination',flush=True)
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
                        continuation=b'{"type":"sessiondock_fixture_continuation"}\n'
                        if peer:peer.append(changed,continuation)
                        else:changed.write_bytes(changed.read_bytes()+continuation)
                        continued=(changed,read_target(changed))
                        print('PASS '+provider+' Chromium selects another node, streams whole family, publishes and opens history/agent',flush=True)
                if peer:peer.close()
        finally:
            if 'peer' in locals() and peer and peer.process.poll() is None:peer.close()
            browser.close()


if __name__=='__main__':main()
