#!/usr/bin/env python3
"""Droid identity-preserving move and shared snapshot merge via Chromium/SSH peer."""
# run_validation: skip
import argparse
import base64
from contextlib import ExitStack
import hashlib
import json
import shlex
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from droid_sdk_fixtures import native_records, native_uid, project_key
from history_fixtures import BINARY, Corpus, isolated_server
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN
from session_bundle_browser import Peer
from transfer_ui import select_target


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + '\n').encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--peer', required=True)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-droid-move-browser'))
    args = parser.parse_args(); args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-droid-move-') as temporary, sync_playwright() as pw, ExitStack() as stack:
        root=Path(temporary); source=Corpus(root/'source')
        for name in ('claude','codex','grok','workspace','state','proc','ids','trash'):
            (source.root/name).mkdir(parents=True,exist_ok=True)
        factory=source.root/'factory'; sessions=factory/'sessions'; sessions.mkdir(parents=True)
        cwd=source.root/'workspace'
        ids=['10000000-0000-4000-8000-'+str(i).zfill(12) for i in range(1,4)]
        raw_content=b'original file contents\n'; content_hash=hashlib.sha256(raw_content).hexdigest()
        content_path=factory/'snapshots/content'/content_hash[:2]/content_hash[2:]
        def seed(sid, title, timestamp):
            path=sessions/project_key(cwd)/(sid+'.jsonl')
            rows=native_records(sid,cwd,[(title,'Droid move answer')]);rows[0]['title']=title
            # Preserve deliberate CRLF, spacing and escape spelling during move.
            transcript=b''.join((json.dumps(row,ensure_ascii=True,separators=(', ', ': '))+'\r\n').encode() for row in rows)
            settings=b'{ "modelId" : "fake-droid", "unknownFutureSetting": { "keep":"\\u4f60" } }\r\n'
            manifest={'sessionId':sid,'version':1,'createdAt':timestamp,'lastAccessedAt':timestamp,
                'boundaries':[{'messageId':'seed-user-0','messageIndex':0,'timestamp':timestamp,
                    'files':[{'filePath':str(cwd/'file.txt'),'contentHash':content_hash,'size':len(raw_content),'capturedAt':timestamp,'mode':33188}],
                    'deletions':[],'creations':[],'totalSize':len(raw_content)}],
                'oldestAvailableBoundaryIndex':0,'evictedBoundaryCount':0}
            entry={'sessionId':sid,'boundaryIndex':0,'timestamp':timestamp,'sizeBytes':len(raw_content),'contentHashes':[content_hash]}
            return {path:transcript,path.with_suffix('.settings.json'):settings,
                factory/'snapshots/manifests'/(sid+'.snapshots.json'):encoded(manifest)},entry
        first,first_entry=seed(ids[0],'Droid selected move',100)
        other,other_entry=seed(ids[1],'Droid source survivor',200)
        target_other,target_entry=seed(ids[2],'Droid target survivor',300)
        source_files={**first,**other,content_path:raw_content}
        index=factory/'snapshots/.index.json';metadata=factory/'snapshots/content/.metadata.json'
        source_files[index]=encoded({'version':1,'lastCleanupAt':7,'totalSizeBytes':len(raw_content)*2,'boundaryQueue':[first_entry,other_entry]})
        source_files[metadata]=encoded({'version':1,'refCounts':{content_hash:2},'totalSizeBytes':len(raw_content),'lastGCAt':9})
        for path,data in source_files.items(): path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(data)
        source.paths['droid']=next(iter(first))
        nodes=[SimpleNamespace(name=name,nid=letter*32,port=free_port(),token=TOKEN) for name,letter in [('Source','a'),('Target','b')]]
        (source.root/'ids/node-id').write_text(nodes[0].nid+'\n')
        roots={kind:str(source.root/kind) for kind in ('claude','codex','grok')};roots['droid']=str(sessions)
        peer=Peer(args.peer,root,source,roots,nodes[1],args.binary);stack.callback(peer.close)
        baseline={**target_other,content_path:raw_content,
            index:b'{ "version":1, "lastCleanupAt":5, "totalSizeBytes":'+str(len(raw_content)).encode()+b', "boundaryQueue":'+json.dumps([target_entry]).encode()+b', "future": {"spelling":"\\u4f60"} }\r\n',
            metadata:b'{ "version":1, "refCounts":{ "'+content_hash.encode()+b'":1 }, "totalSizeBytes":'+str(len(raw_content)).encode()+b', "lastGCAt":6, "future": {"spelling":"\\u4f60"} }\r\n'}
        peer.call('seed_cwd',path=str(source.root),entries=[{'relative':str(path.relative_to(source.root)),'kind':'file','bytes':base64.b64encode(data).decode(),'mode':0o600} for path,data in baseline.items()])
        peer.call('start')
        env=node_env(source.root,nodes[0].port,'127.0.0.0/8');env.update(SESSIONDOCK_DROID_ROOT=str(sessions),SESSIONDOCK_PROC_ROOT=str(source.root/'proc'),SESSIONDOCK_CODEBUDDY_ROOT='',SESSIONDOCK_KIRO_ROOT='')
        source_binary=root/'droid-only-server'
        source_binary.write_text('#!/bin/sh\nexec env -u SESSIONDOCK_CODEX_ROOT -u SESSIONDOCK_CLAUDE_ROOT -u SESSIONDOCK_GROK_ROOT '+shlex.quote(str(args.binary.resolve()))+' \"$@\"\n')
        source_binary.chmod(0o700)
        base,_=stack.enter_context(isolated_server(source,source_binary,state_dir=source.root/'state',trash_dir=source.root/'trash',extra_env=env))
        hubroot=root/'hub';hubroot.mkdir();hub=Hub(args.binary.with_name('sessiondock-hub'),hubroot,nodes);hub.start();stack.callback(hub.stop)
        browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
        context=browser.new_context(viewport={'width':1280,'height':900},service_workers='block');stack.callback(context.close)
        page=context.new_page();page.set_default_timeout(20000);errors=[];page.on('pageerror',lambda error:errors.append(error.stack))
        page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
        original_uid=native_uid(source.paths['droid'])
        def choose(node):
            uid=scoped(node.nid,original_uid)
            if page.locator('#detail .mobile-back').is_visible():page.locator('#detail .mobile-back').click()
            page.locator(f'#side .item[data-uid="{uid}"]').click(button='right')
            page.locator('#item-menu [data-act="clone"]').click()
            dialog=page.locator('#clone-group-dialog');expect(dialog).to_be_visible()
            expect(dialog.locator(f'.transfer-tree input[data-uid="{uid}"]')).to_be_enabled()
            expect(dialog.locator('[name="transfer-mode"], #transfer-new-ids')).to_have_count(0)
            return dialog
        # Every responsive entry offers the selected source with the global fixed-ID policy.
        for width,theme in ((1280,'light'),(390,'dark')):
            page.set_viewport_size({'width':width,'height':900});page.emulate_media(color_scheme=theme)
            dialog=choose(nodes[0]);page.screenshot(path=str(args.screenshots/f'tree-{width}-{theme}.png'),full_page=True);dialog.locator('.clone-cancel').click()
        page.set_viewport_size({'width':1280,'height':900});page.emulate_media(color_scheme='light')
        def move(owner,destination):
            dialog=choose(owner);select_target(dialog,destination.nid)
            try:expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=30000)
            except Exception:
                print('PLAN',dialog.inner_text(),flush=True);raise
            with page.expect_response(lambda response:response.url.endswith('/api/session/transfer/clone'),timeout=90000) as completed:
                dialog.locator('.clone-confirm').click()
            assert completed.value.ok,completed.value.text()
            result=completed.value.json();assert result['mode']=='move' and result['new_ids'] is False,result
            expect(dialog).not_to_be_visible(timeout=30000)
            return result
        # Mission pointers require a separately verified persistent dependency closure.
        for in_settings in (False,True):
            path=source.paths['droid'].with_suffix('.settings.json') if in_settings else source.paths['droid']
            original=first[path]
            if in_settings:
                value=json.loads(original);value['tags']=[{'name':'mission','metadata':{'missionId':ids[0]}}]
                modified=encoded(value)
            else:
                head,tail=original.split(b'\r\n',1);value=json.loads(head);value['decompMissionId']=ids[0]
                modified=json.dumps(value).encode()+b'\r\n'+tail
            path.write_bytes(modified)
            page.reload(wait_until='networkidle')
            dialog=choose(nodes[0]);select_target(dialog,nodes[1].nid)
            expect(dialog).to_contain_text('mission',timeout=30000)
            expect(dialog.locator('.clone-confirm')).to_be_disabled()
            assert path.read_bytes()==modified
            assert peer.read(index)==baseline[index]
            dialog.locator('.clone-cancel').click()
            path.write_bytes(original)
        page.reload(wait_until='networkidle')

        # Existing shared content without native refcounts needs Droid's own recovery.
        peer.call('unlink',path=str(metadata))
        dialog=choose(nodes[0]);select_target(dialog,nodes[1].nid)
        expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=30000)
        with page.expect_response(lambda response:response.url.endswith('/api/session/transfer/clone'),timeout=90000) as incomplete:
            dialog.locator('.clone-confirm').click()
        assert not incomplete.value.ok and '共享元数据不完整' in incomplete.value.text(),incomplete.value.text()
        assert peer.read(index)==baseline[index] and source.paths['droid'].exists()
        missing=False
        try:peer.read(metadata)
        except AssertionError as error:
            assert 'No such file' in str(error),error
            missing=True
        assert missing,'migration silently rebuilt native refcounts'
        peer.write(metadata,baseline[metadata])
        dialog.locator('.clone-cancel').click()

        # Another session in the target's shared snapshot root is a real writer blocker.
        proc=root/'destination/proc'; pid='424242'
        proc_files={proc/'stat':b'btime 1700000000\n',
            proc/pid/'stat':(pid+' (droid) '+' '.join(['S','1',*(['0']*17),'100'])+'\n').encode(),
            proc/pid/'cmdline':('droid\0--resume\0'+ids[2]+'\0').encode(),
            proc/pid/'environ':b''}
        peer.call('seed_cwd',path=str(root),entries=[{'relative':str(path.relative_to(root)),'kind':'file','bytes':base64.b64encode(data).decode(),'mode':0o600} for path,data in proc_files.items()]+[
            {'relative':str((proc/pid/'cwd').relative_to(root)),'kind':'symlink','target':str(cwd),'mode':0o700}])
        dialog=choose(nodes[0]);select_target(dialog,nodes[1].nid)
        expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=30000)
        with page.expect_response(lambda response:response.url.endswith('/api/session/transfer/clone'),timeout=90000) as busy:
            dialog.locator('.clone-confirm').click()
        assert not busy.value.ok and '会话仍在运行' in busy.value.text(),busy.value.text()
        assert source.paths['droid'].exists()
        assert peer.read(metadata)==baseline[metadata]
        dialog.locator('.clone-cancel').click()
        peer.call('unlink',path=str(proc/pid/'stat'))
        peer.call('unlink',path=str(proc/pid/'cmdline'))

        # Fail after publishing the shared index: compensation must restore all original bytes.
        dialog=choose(nodes[0])
        with page.expect_response(lambda response:response.url.endswith('/api/session/clone/plan')) as planned:
            select_target(dialog,nodes[1].nid)
        expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=30000)
        operation=planned.value.json()['operation_id']
        marker=metadata.with_name(f'.sessiondock-{operation}-{metadata.name}.pending')
        peer.write(marker,b'private fixture publication collision')
        with page.expect_response(lambda response:response.url.endswith('/api/session/transfer/clone'),timeout=90000) as failed:
            dialog.locator('.clone-confirm').click()
        assert not failed.value.ok,failed.value.text()
        for path,data in first.items():assert path.read_bytes()==data
        assert peer.read(index)==baseline[index] and peer.read(metadata)==baseline[metadata]
        for path,data in target_other.items():assert peer.read(path)==data
        # Rollback cleans transaction-owned staging markers; allow a fresh confirmed attempt.
        dialog.locator('.clone-cancel').click()
        page.reload(wait_until='networkidle')
        result=move(nodes[0],nodes[1])
        for path,data in first.items():assert not path.exists();assert peer.read(path)==data
        assert content_path.read_bytes()==raw_content and peer.read(content_path)==raw_content
        for path,data in other.items():assert path.read_bytes()==data
        for path,data in target_other.items():assert peer.read(path)==data
        target_index=peer.read(index);target_metadata=peer.read(metadata)
        assert b'"future": {"spelling":"\\u4f60"}' in target_index and target_index.endswith(b' }\r\n')
        assert b'"future": {"spelling":"\\u4f60"}' in target_metadata and target_metadata.endswith(b' }\r\n')
        assert json.loads(target_metadata)['refCounts'][content_hash]==2
        assert json.loads(target_index)['boundaryQueue']==[first_entry,target_entry]
        assert index.read_bytes()==source_files[index] and metadata.read_bytes()==source_files[metadata]
        expect(page.locator('.dhead')).to_contain_text('Droid selected move')
        page.screenshot(path=str(args.screenshots/'moved.png'),full_page=True)
        move(nodes[1],nodes[0])
        for path,data in first.items():assert path.read_bytes()==data
        for path,data in other.items():assert path.read_bytes()==data
        for path,data in target_other.items():assert peer.read(path)==data
        assert metadata.read_bytes()==source_files[metadata]
        assert peer.read(content_path)==raw_content
        page.screenshot(path=str(args.screenshots/'returned.png'),full_page=True)
        assert not errors,errors
    print('PASS Droid move: fixed IDs, settings and rewind manifests, shared CAS merge, writer blocking, rollback, unrelated-byte preservation, source retention and return')


if __name__=='__main__':main()
