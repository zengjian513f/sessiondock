#!/usr/bin/env python3
"""Real Chromium links survive copy chains, deletion and restarts; private histories."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import shutil
import tempfile
import time
import uuid
from types import SimpleNamespace
from urllib.parse import urlencode
from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, Corpus, isolated_server
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN
from session_files_browser import fixture, uid
from session_transfer_browser import ident


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-link-lineage-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root=Path(tmp);roots,main,side,agent=fixture(root/'node');corpus=Corpus(root/'node')
        node=SimpleNamespace(name='origin',nid='a'*32,port=free_port(),token=TOKEN)
        for name in ('state','proc','ids'):(corpus.root/name).mkdir()
        (corpus.root/'ids/node-id').write_text(node.nid+'\n')
        env=node_env(corpus.root,node.port,'127.0.0.0/8')
        env.update(SESSIONDOCK_CLAUDE_ROOT=roots['claude'],SESSIONDOCK_PROC_ROOT=str(corpus.root/'proc'))
        servers=stack.enter_context(ExitStack())
        def start():servers.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',extra_env=env))
        start()
        peer=SimpleNamespace(name='destination',nid='b'*32,port=free_port(),token=TOKEN)
        other=Corpus(root/'other');other.root.mkdir()
        for name in ('state','proc','ids'):(other.root/name).mkdir()
        (other.root/'ids/node-id').write_text(peer.nid+'\n')
        peer_env=node_env(other.root,peer.port,'127.0.0.0/8')
        peer_env.update({f'SESSIONDOCK_{k.upper()}_ROOT':str(corpus.root/k) for k in ('claude','codex','grok')})
        peer_env.update(SESSIONDOCK_CLAUDE_ROOT=roots['claude'],SESSIONDOCK_PROC_ROOT=str(other.root/'proc'))
        stack.enter_context(isolated_server(other,args.binary,state_dir=other.root/'state',extra_env=peer_env))
        hubroot=root/'hub';hubroot.mkdir();hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node,peer]);hub.start();stack.callback(hub.stop)
        browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
        page=browser.new_page();errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
        base=f'http://127.0.0.1:{hub.port}'
        original='claude:'+ident(2);original_uid=uid('claude',main[2])
        def open_link(spec,expected,agent_view=False,copied=False):
            page.goto(base+'/?'+urlencode({'sid':spec,'node':node.nid}),wait_until='networkidle')
            expect(page.locator('#msgs')).to_contain_text('Agent answer' if agent_view else 'Branch A final')
            page.wait_for_function('uid=>S.sel===uid',arg=expected)
            if copied:expect(page.locator('#session-stop-notice')).to_contain_text('副本')
        def clone(source,target=None):
            page.goto(base+'/?'+urlencode({'sid':source,'node':node.nid}),wait_until='networkidle')
            expect(page.locator('#msgs')).to_contain_text('Branch A final')
            page.locator('#a-clone-group').click()
            dialog=page.locator('#clone-group-dialog');expect(dialog.locator('.clone-confirm')).to_be_enabled()
            if target:
                dialog.locator('#transfer-target').select_option(target)
                expect(dialog.locator('.clone-confirm')).to_be_enabled()
            endpoint='/api/session/transfer/clone' if target else '/api/session/clone'
            with page.expect_response(lambda r:r.url.endswith(endpoint) and r.request.method=='POST') as reply:
                dialog.locator('.clone-confirm').click()
            assert reply.value.ok,reply.value.text()
            result=reply.value.json();page.wait_for_function('uid=>S.sel===uid',arg=result['target_uid'])
            assert result['link_map'].get(source),result
            return result
        def resolve(spec):
            start=time.monotonic();status,body=hub.json('POST','/api/sessions/resolve',{'links':[{'sid':spec,'node':node.nid}]})
            assert status==200,body
            print(f'resolve {body["results"][0]["status"]}: {(time.monotonic()-start)*1000:.1f} ms',flush=True)
            return body['results'][0]
        def remove(ids):
            # Model deletion by a native CLI, outside SessionDock's own trash UI.
            for sid in ids:
                if not sid.startswith('claude:'):continue
                name=sid.split(':',1)[1]
                path=main[2].parent/(name+'.jsonl')
                path.unlink(missing_ok=True)
                folder=main[2].parent/name
                if folder.exists():shutil.rmtree(folder)
        b=clone(original);bsid=b['link_map'][original]
        c=clone(bsid);csid=c['link_map'][bsid]
        d=clone(original);dsid=d['link_map'][original]
        assert resolve(original)['session']['sid']==original
        open_link(original,scoped(node.nid,original_uid))
        remove(['claude:'+ident(i) for i in (1,2,3)])
        assert resolve(original)['session']['sid']==bsid
        open_link(original,b['target_uid'],copied=True)
        remove(b['link_map'].values())
        assert resolve(original)['session']['sid']==dsid,'nearest surviving direct copy beats deeper descendant'
        remove(d['link_map'].values())
        assert resolve(original)['session']['sid']==csid
        open_link(original,c['target_uid'],copied=True)
        open_link(original_uid,c['target_uid'],copied=True)
        open_link(original+'/agent:'+agent,c['target_uid'],agent_view=True,copied=True)
        print('PASS original-first; nearest then oldest; copies-of-copies; old UID and agent links through actual page',flush=True)
        hub.stop();servers.close()
        # Simulate receipts written by an older release before identity maps.
        for receipt in (hubroot/'transfers').glob('*.json'):
            value=json.loads(receipt.read_text());value['result'].pop('link_map',None);value.pop('created_ms',None)
            receipt.write_text(json.dumps(value))
        start();hub.start()
        open_link(original,c['target_uid'],copied=True)
        assert resolve(original)['session']['sid']==csid
        # The existing journal is the only durable source; no new database.
        assert len(list((corpus.root/'state/transfers').glob('*/operation.json')))==3
        assert all(json.loads(p.read_text())['result'].get('link_map') for p in (hubroot/'transfers').glob('*.json'))
        print('PASS lineage survives restart and lazily backfills legacy receipts',flush=True)
        e=clone(csid,peer.nid)
        servers.close()
        result=resolve(original)
        assert result['status']=='unavailable' and any(s['node']==peer.nid for s in result['alternatives']),result
        page.goto(base+'/?'+urlencode({'sid':original,'node':node.nid}),wait_until='networkidle')
        expect(page.locator('#session-stop-notice')).to_contain_text('尚不能确认')
        page.locator('#session-stop-notice button').filter(has_text='打开副本').first.click()
        page.wait_for_function('uid=>S.sel===uid',arg=e['target_uid'])
        expect(page.locator('#msgs')).to_contain_text('Branch A final')
        print('PASS offline origin offers accessible copy without silently redirecting',flush=True)
        start()
        remove(e['link_map'].values())
        remove(c['link_map'].values())
        hub.stop()
        receipt=json.loads(next((hubroot/'transfers').glob('*.json')).read_text())
        # A return edge terminates; aborted copies never make missing links valid.
        for phase,target in [('complete',original),('aborted','grok:'+ident(10))]:
            operation=str(uuid.uuid4())
            receipt['request'].update(operation_id=operation,uid=scoped(node.nid,original_uid),target_node=node.nid)
            receipt.update(phase=phase,created_ms=int(time.time()*1000))
            receipt['result']['link_map']={original:target}
            (hubroot/'transfers'/(operation+'.json')).write_text(json.dumps(receipt))
        hub.start()
        assert resolve(original)['status']=='missing'
        page.goto(base+'/?'+urlencode({'sid':original,'node':node.nid}),wait_until='networkidle')
        expect(page.locator('#session-stop-notice')).to_contain_text('后继均不存在')
        servers.close()
        page.reload(wait_until='networkidle')
        expect(page.locator('#session-stop-notice')).to_contain_text('尚不能确认')
        assert not errors,errors
        print('PASS cycles terminate, aborted edges ignored, deleted chain and offline origin distinguished',flush=True)


if __name__=='__main__':main()
