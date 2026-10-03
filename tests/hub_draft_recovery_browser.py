#!/usr/bin/env python3
"""Draft discovery isolates slow/offline peers instead of retrying every node."""
from browser_runtime import js
import argparse
from collections import Counter
from contextlib import ExitStack
from pathlib import Path
import tempfile
import time
from urllib.parse import urlsplit, parse_qs
from playwright.sync_api import sync_playwright
from history_parity import BINARY
from hub_http_suite import FakeNode, Hub


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-draft-recovery-') as tmp, ExitStack() as stack:
        nodes=[]
        for key in 'abcdef':
            node=FakeNode(key*32,'Node'+key.upper());nodes.append(node);stack.callback(node.stop)
        local_uid=nodes[0].state()['row']['uid']
        nodes[0].set(drafts={local_uid:{'revision':1,'value':{'text':'Restored draft','attachments':[],'quotes':[]}}})
        hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),Path(tmp),nodes)
        nodes[3].set(offline=True)
        hub.start();stack.callback(hub.stop)
        for _ in range(30):
            hub.json('GET','/api/sessions?force=1')
            _, health = hub.json('GET','/api/nodes')
            if next(n for n in health['nodes'] if n['id']==nodes[3].nid)['online'] is False:
                break
            time.sleep(0.1)
        else:
            raise AssertionError('Hub did not record initially offline node')
        with sync_playwright() as pw, ExitStack() as browser_stack:
            browser=pw.chromium.launch(headless=True);browser_stack.callback(browser.close)
            page=browser.new_page(viewport={'width':1400,'height':900})
            requests=Counter();held=[];failing={nodes[4].nid,nodes[5].nid}
            errors=[];page.on('pageerror',lambda error:errors.append(str(error)))
            def drafts(route):
                node=parse_qs(urlsplit(route.request.url).query)['node'][0]
                requests[node]+=1
                if node==nodes[4].nid and requests[node]==1:
                    held.append(route);return
                if node in failing:route.fulfill(status=503,json={'error':'synthetic offline peer'})
                else:route.continue_()
            page.route('**/api/session/conversation/drafts?*',drafts)
            base=f'http://127.0.0.1:{hub.port}'
            page.goto(base,wait_until='domcontentloaded')
            uid=local_uid.replace(':',':'+nodes[0].nid+'~',1)
            page.wait_for_function(js('uid=>composerDrafts.get(uid)?.text==="Restored draft"', 'uid=>runtime.composer.composerDrafts.get(uid)?.text==="Restored draft"'),arg=uid)
            # A real click can open the good node while another discovery hangs.
            page.locator(f'#side .item[data-uid="{uid}"] .t').click()
            page.wait_for_function(js('uid=>S.sel===uid && cache.has(uid)', 'uid=>runtime.core.state.selection.sel===uid && runtime.core.cache.cache.has(uid)'),arg=uid)
            assert page.evaluate(js('composerServerRecoveries.get("'+nodes[4].nid+'").request!==null', 'runtime.composer.composerServerRecoveries.get("' + nodes[4].nid + '").request!==null'))
            # Let several real live/terminal-list polling cycles pass. The held
            # request times out at 12 seconds; neither peer retries the group.
            page.wait_for_function(js('composerServerRecoveries.get("'+nodes[4].nid+'").failures===1', 'runtime.composer.composerServerRecoveries.get("' + nodes[4].nid + '").failures===1'),timeout=16000)
            page.wait_for_timeout(3200)
            assert requests==Counter({n.nid:1 for n in nodes if n is not nodes[3]}),requests
            assert page.evaluate(js('uid=>composerDrafts.get(uid).text', 'uid=>runtime.composer.composerDrafts.get(uid).text'),uid)=='Restored draft'
            page.evaluate(js('''()=>{window.__recoveryRenders=0;const render=renderSide;
              renderSide=function(...args){__recoveryRenders++;return render(...args)}}''', """()=>{window.__recoveryRenders=0;const render=runtime.sidebarView.renderSide;
              runtime.sidebarView.renderSide=function(...args){__recoveryRenders++;return render(...args)}}"""))
            # Only the failed, due node retries; concurrent calls share its GET.
            page.evaluate(js('node=>{composerServerRecoveries.get(node).retryAt=0}', 'node=>{runtime.composer.composerServerRecoveries.get(node).retryAt=0}'),nodes[5].nid)
            page.evaluate(js('Promise.all([recoverServerComposerDrafts(),recoverServerComposerDrafts()])', 'Promise.all([runtime.composer.recoverServerComposerDrafts(),runtime.composer.recoverServerComposerDrafts()])'))
            assert requests[nodes[5].nid]==2 and requests[nodes[4].nid]==1,requests
            assert all(requests[n.nid]==1 for n in nodes[:3]),requests
            assert page.evaluate('__recoveryRenders')==0
            assert page.evaluate(js('node=>composerServerRecoveries.get(node).retryAt-Date.now()>55000', 'node=>runtime.composer.composerServerRecoveries.get(node).retryAt-Date.now()>55000'),nodes[5].nid)
            # A known-offline node is not contacted even when its retry is due.
            page.evaluate(js('''node=>{Nodes.list.find(n=>n.id===node).online=false;
              composerServerRecoveries.get(node).retryAt=0}''', """node=>{runtime.core.state.nodes.list.find(n=>n.id===node).online=false;
              runtime.composer.composerServerRecoveries.get(node).retryAt=0}"""),nodes[5].nid)
            page.evaluate(js('recoverServerComposerDrafts()', 'runtime.composer.recoverServerComposerDrafts()'))
            assert requests[nodes[5].nid]==2
            # Recovery is immediate on the next observation of online:true;
            # there is no need to reload the page or wait out the backoff.
            failing.remove(nodes[5].nid)
            page.evaluate(js('''node=>{composerServerRecoveries.get(node).retryAt=Date.now()+300000;
              Nodes.list.find(n=>n.id===node).online=true}''', """node=>{runtime.composer.composerServerRecoveries.get(node).retryAt=Date.now()+300000;
              runtime.core.state.nodes.list.find(n=>n.id===node).online=true}"""),nodes[5].nid)
            page.evaluate(js('recoverServerComposerDrafts()', 'runtime.composer.recoverServerComposerDrafts()'))
            assert requests[nodes[5].nid]==3
            assert page.evaluate(js('node=>composerServerRecoveries.get(node).done', 'node=>runtime.composer.composerServerRecoveries.get(node).done'),nodes[5].nid)
            page.evaluate(js('recoverServerComposerDrafts()', 'runtime.composer.recoverServerComposerDrafts()'))
            assert requests[nodes[5].nid]==3
            # A newly discovered node is not hidden by an all-nodes done flag.
            extra=FakeNode('1'*32,'NodeNew');stack.callback(extra.stop)
            token=Path(tmp)/'NodeNew.token';token.write_text(extra.token+'\n');token.chmod(0o600)
            hub.command('register','--name',extra.name,'--url',f'http://127.0.0.1:{extra.port}',
                        '--token-file',str(token))
            # Registration updates the registry file; reload the isolated Hub.
            hub.stop();hub.start()
            page.evaluate(js('loadNodes()', 'runtime.core.nodes.loadNodes()'));page.evaluate(js('recoverServerComposerDrafts()', 'runtime.composer.recoverServerComposerDrafts()'))
            assert requests[extra.nid]==1,requests
            assert all(requests[n.nid]==1 for n in nodes[:3]),requests
            assert requests[nodes[3].nid]==0,requests
            for route in held:
                route.abort()
            assert not errors,errors
            print('PASS hub draft recovery: known-offline peer zero requests, healthy peers queried once, failure isolation, timeout, backoff, offline/recovery and new-node discovery')


if __name__=='__main__':main()
