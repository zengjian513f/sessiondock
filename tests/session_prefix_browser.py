#!/usr/bin/env python3
"""Two private nodes: existing complex history prefixes, merge and recovery."""
# run_validation: skip
# Explicit SSH peer required; invoked directly as documented.
import argparse
import base64
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, Corpus, isolated_server
from session_clone_browser import prepare
from session_files_browser import fixture, uid
from session_transfer_browser import ident
from session_bundle_browser import Peer
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN


def seed(peer, source):
    entries=[]
    for kind in ('claude','codex','grok'):
        for p in (source.root/kind).rglob('*'):
            row={'relative':str(p.relative_to(source.root)), 'mode':p.stat().st_mode & 0o777}
            if p.is_symlink():row.update(kind='symlink',target=os.readlink(p))
            elif p.is_dir():row.update(kind='directory')
            else:row.update(kind='file',bytes=base64.b64encode(p.read_bytes()).decode())
            entries.append(row)
    peer.call('seed_cwd',path=str(source.root),entries=entries)


def transfer(page, hub, selected, target, count, move=False, error=None):
    page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
    page.locator(f'#side .item[data-uid="{selected}"]').click()
    page.locator('#a-clone-group').click()
    dialog=page.locator('#clone-group-dialog')
    expect(dialog.locator('.clone-members tbody tr')).to_have_count(count)
    dialog.locator('#transfer-target').select_option(target)
    if move:
        with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')):
            dialog.locator('.transfer-segments label').nth(1).click()
    else:
        with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')):
            dialog.locator('#transfer-new-ids').uncheck()
    expect(dialog.locator('#transfer-new-ids')).not_to_be_checked()
    with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=90000) as result:
        dialog.locator('.clone-confirm').click()
    if error:
        assert result.value.status==409 and result.value.json()["code"]==error,result.value.text()
        return result.value.json()
    assert result.value.ok,result.value.text()
    return result.value.json()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--peer',required=True)
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-prefix-browser-') as tmp, sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            for provider in ('codex','claude','grok'):
                root=Path(tmp)/provider;root.mkdir()
                if provider=='codex':
                    source=prepare(root/'source');selected=source.uid('a');count=6
                    primary=source.paths['a'];extra={'type':'event_msg','payload':{'type':'user_message','message':'PREFIX_EXTENDED'}}
                    roots={k:str(source.root/k) for k in ('claude','codex','grok')}
                else:
                    roots,main,side,agent=fixture(root/'source');source=Corpus(root/'source')
                    (source.root/'state').mkdir();(source.root/'proc').mkdir()
                    roots['codex']=str(source.root/'codex')
                    primary=main[2] if provider=='claude' else source.root/'grok/project'/ident(10)/'chat_history.jsonl'
                    selected=uid(provider,primary if provider=='claude' else primary.parent);count=4
                    extra=({'type':'user','sessionId':ident(2),'uuid':ident(888),'parentUuid':ident(154),
                            'cwd':str(source.root/'cwd'),'message':{'role':'user','content':'PREFIX_EXTENDED'}}
                           if provider=='claude' else {'type':'user','content':'PREFIX_EXTENDED'})
                for name in ('ids','trash'):(source.root/name).mkdir()
                a=SimpleNamespace(name='source',nid='a'*32,port=free_port(),token=TOKEN)
                b=SimpleNamespace(name='destination',nid='b'*32,port=free_port(),token=TOKEN)
                (source.root/'ids/node-id').write_text(a.nid+'\n')
                peer=Peer(args.peer,root,source,roots,b,args.binary)
                with ExitStack() as stack:
                    stack.callback(peer.close)
                    seed(peer,source)
                    source_metadata=source.root/'state/session-metadata.json'
                    source_doc=json.loads(source_metadata.read_text()) if source_metadata.exists() else {'schema_version':1,'revision':1,'sessions':{}}
                    source_doc['sessions'][selected]={'group':'source-group','fork_parent_visible':True}
                    source_metadata.write_text(json.dumps(source_doc))
                    target_metadata=root/'destination/state/session-metadata.json'
                    target_before={'starred':True,'starred_at':123,'group':'target-group','activity_revision':7}
                    unrelated='claude:ffffffffffffffff'
                    peer.write(target_metadata,json.dumps({'schema_version':1,'revision':1,'sessions':{
                        selected:target_before,unrelated:{'starred':True}}}).encode())
                    before=primary.read_bytes();primary.write_bytes(before+json.dumps(extra).encode()+b'\n')
                    after=primary.read_bytes()
                    old_files={str(primary):before}
                    old_rollout=None
                    if provider=='codex':
                        old_rollout=source.paths['a-old']
                        old_files[str(old_rollout)]=old_rollout.read_bytes()
                        old_rollout.write_bytes(old_files[str(old_rollout)]+json.dumps(extra).encode()+b'\n')
                        old_rollout_after=old_rollout.read_bytes()
                    summary=None
                    if provider=='grok':
                        summary=primary.parent/'summary.json';old_summary=summary.read_bytes()
                        doc=json.loads(old_summary);doc['generated_title']='Prefix extended';summary.write_text(json.dumps(doc));new_summary=summary.read_bytes();old_files[str(summary)]=old_summary
                    if provider=='codex':
                        database=source.root/'codex/state_5.sqlite'
                        with sqlite3.connect(database) as db:db.execute('UPDATE threads SET name=? WHERE id=?',('Native extended',ident(2)))
                    env=node_env(source.root,a.port,'127.0.0.0/8')
                    env.update({f'SESSIONDOCK_{k.upper()}_ROOT':v for k,v in roots.items()})
                    env['SESSIONDOCK_PROC_ROOT']=str(source.root/'proc')
                    stack.enter_context(isolated_server(source,args.binary,state_dir=source.root/'state',trash_dir=source.root/'trash',extra_env=env))
                    peer.call('start')
                    hubroot=root/'hub';hubroot.mkdir()
                    hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[a,b]);hub.start();stack.callback(hub.stop)
                    context=browser.new_context(service_workers='block');stack.callback(context.close)
                    page=context.new_page()
                    # Equal history has no evidence to choose one divergent preference set.
                    primary.write_bytes(before)
                    transfer(page,hub,scoped(a.nid,selected),b.nid,count,error='move_conflict')
                    assert json.loads(peer.read(target_metadata))['sessions'][selected]==target_before
                    primary.write_bytes(after)
                    # A target with independent continuation is never replaced.
                    divergent=before+json.dumps({'type':'user','content':'TARGET_ONLY'}).encode()+b'\n'
                    peer.write(primary,divergent)
                    transfer(page,hub,scoped(a.nid,selected),b.nid,count,error='move_conflict')
                    assert peer.read(primary)==divergent and primary.read_bytes()==after
                    peer.write(primary,before)
                    print('PASS '+provider+' divergent target rejected through Chromium without overwriting either side',flush=True)
                    if provider=='codex':
                        peer.call('stop')
                        changed=peer.call('thread_path',path=str(database),sid=ident(2),rollout=str(source.paths['parent']))
                        peer.call('start')
                        transfer(page,hub,scoped(a.nid,selected),b.nid,count,error='move_conflict')
                        assert peer.read(primary)==before
                        peer.call('stop')
                        peer.call('thread_path',path=str(database),sid=ident(2),rollout=changed['previous'])
                        peer.call('start')
                        print('PASS Codex same thread with different current rollout rejects prefix replacement',flush=True)
                    for move in (False,True):
                        result=transfer(page,hub,scoped(a.nid,selected),b.nid,count,move)
                        assert peer.read(primary)==after
                        if old_rollout:assert peer.read(old_rollout)==old_rollout_after
                        if summary:assert peer.read(summary)==new_summary
                        journal=root/'destination/state/transfers'/result['operation_id']/'operation.json'
                        operation=json.loads(peer.read(journal))
                        assert operation['replaced_files'],operation
                        assert operation['phase']=='complete'
                        assert operation['metadata_replaced'][selected]==target_before
                        published_metadata=json.loads(peer.read(target_metadata))['sessions'][selected]
                        assert published_metadata=={**source_doc['sessions'][selected],
                            'activity_revision':7,'clone_operation':result['operation_id']},published_metadata
                        if provider=='codex':
                            receipt=peer.call('receipt',path=str(database),operation_id=result['operation_id'])['receipt']
                            old_rows=[r for t in receipt['replaced']['tables'] if t['name']=='threads' for r in t['rows']]
                            assert any(r['id']==ident(2) and r['name']=='Native a' for r in old_rows),receipt
                        if move:
                            assert not primary.exists()
                            print('PASS '+provider+' Chromium whole-group move extends target prefixes and retires source',flush=True)
                            continue
                        # Recreate a receiver crash after verification but before durable
                        # completion. Startup must compensate file and native-row changes.
                        peer.call('stop')
                        for target in operation['replaced_files']:
                            target=Path(target)
                            marker=target.with_name('.sessiondock-'+result['operation_id']+'-'+target.name+'.pending')
                            peer.call('link',path=str(marker),source=str(target))
                            # Interrupt between hard-link creation and atomic rename.
                            peer.call('link',path=str(marker.with_suffix('.publish')),source=str(target))
                        operation['phase']='verifying';peer.write(journal,json.dumps(operation).encode())
                        # A target writer after publication must prevent compensation.
                        changed=after+json.dumps({'type':'user','content':'TARGET_LATER'}).encode()+b'\n'
                        peer.write(primary,changed);peer.call('start')
                        held=json.loads(peer.read(journal))
                        assert held['phase']=='rollback_required' and peer.read(primary)==changed,held
                        peer.call('stop');peer.write(primary,after)
                        # Now resolve the file mismatch but introduce a target preference change.
                        # It must be preserved, and block file/native compensation too.
                        changed_doc=json.loads(peer.read(target_metadata))
                        changed_doc['sessions'][selected]={**published_metadata,'starred':True,'starred_at':987}
                        changed_doc['sessions'][unrelated]={'starred':True,'group':'concurrent-unrelated'}
                        peer.write(target_metadata,json.dumps(changed_doc).encode());peer.call('start')
                        held=json.loads(peer.read(journal))
                        assert held['phase']=='rollback_required' and peer.read(primary)==after,held
                        assert json.loads(peer.read(target_metadata))['sessions']==changed_doc['sessions']
                        peer.call('stop')
                        changed_doc=json.loads(peer.read(target_metadata));changed_doc['sessions'][selected]=published_metadata
                        peer.write(target_metadata,json.dumps(changed_doc).encode());peer.call('start')
                        assert peer.read(primary)==before
                        restored=json.loads(peer.read(target_metadata))['sessions']
                        assert restored[selected]==target_before
                        assert restored[unrelated]=={'starred':True,'group':'concurrent-unrelated'}
                        if summary:assert peer.read(summary)==old_summary
                        recovered=json.loads(peer.read(journal));assert recovered['phase']=='failed',recovered
                        for target in operation['replaced_files']:
                            # Recovery retains the exact original for later inspection.
                            assert peer.read(operation['replaced_files'][target]['backup'])==old_files[target]
                            assert peer.read(target)==old_files[target]
                        if provider=='codex':
                            with tempfile.NamedTemporaryFile(suffix='.sqlite') as local:
                                Path(local.name).write_bytes(peer.read(database))
                                with sqlite3.connect(local.name) as db:
                                    assert db.execute('SELECT name FROM threads WHERE id=?',(ident(2),)).fetchone()[0]=='Native a'
                        assert primary.read_bytes()==after
                        print('PASS '+provider+' Chromium prefix copy and restart restore original bytes and native rows',flush=True)
        finally:browser.close()


if __name__=='__main__':main()
