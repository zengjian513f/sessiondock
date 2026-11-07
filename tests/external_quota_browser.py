"""External Codex and conversation views share the node's cached collector."""
import argparse
import asyncio
import threading
from contextlib import ExitStack
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from urllib.parse import urlencode

from playwright.sync_api import sync_playwright, expect
from codex_sdk_browser import build, server, THREAD
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, codex_row, codex_message, batch35_agent_meta
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env

CLI = r'''
import json,sys,os
from pathlib import Path
root=Path(os.environ['QUOTA_FIXTURE'])
if '--version' in sys.argv: print('fixture');sys.exit()
with (root/'calls').open('a') as f:f.write('query\n')
if (root/'fail').exists():sys.exit(1)
for line in sys.stdin:
 q=json.loads(line)
 if q.get('method')=='initialize':print(json.dumps({'id':q['id'],'result':{}}),flush=True)
 if q.get('method')=='account/rateLimits/read':
  print(json.dumps({'id':q['id'],'result':{'rateLimits':{'primary':{'usedPercent':float((root/'used').read_text()) if (root/'used').exists() else 40,'windowDurationMins':10080,'resetsAt':1800000000},'credits':{'balance':'30816.7'}}}}),flush=True)
'''


class DelayedQuotaServer(FakeAppServer):
    quota_gate = None

    async def _handle(self, writer, message):
        gate = self.quota_gate
        if message.get('method') == 'account/rateLimits/read' and gate:
            bucket = self._bucket()
            while not gate.is_set():
                await asyncio.sleep(0.01)
            await self.send(writer, {'id': message['id'], 'result': {
                'rateLimits': bucket, 'rateLimitsByLimitId': {'codex': bucket}}})
            return
        await super()._handle(writer, message)


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--screenshots', type=Path, required=True)
    args=parser.parse_args();args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-external-quota-') as temp, ExitStack() as stack:
        root=Path(temp);corpus=build(root)
        corpus.put('external','codex',[codex_row('session_meta',{'id':'external','cwd':'/synthetic/usage'}),
            codex_message('user','外部 CLI 用量：同一机器共享额度，保留独立会话上下文'),
            codex_row('turn_context',{'model':'external-model','effort':'high'}),
            codex_row('event_msg',{'type':'token_count','info':{
                'last_token_usage':{'total_tokens':800},'model_context_window':4000,
                'total_token_usage':{'total_tokens':90000,'input_tokens':80000,'output_tokens':10000}},
                'rate_limits':{'primary':{'used_percent':11,'window_minutes':10080},
                    'credits':{'balance':'37358.1','has_credits':True,'unlimited':False}}})],[])
        child='0199a100-0000-7000-8000-000000000002'
        corpus.put(child,'codex',[batch35_agent_meta(child,THREAD),
            codex_message('user','Child usage is independent'),
            codex_row('event_msg',{'type':'token_count','info':{
                'last_token_usage':{'total_tokens':2000},'model_context_window':4000}})],[],parent='sdk')
        state=root/'state';(state/'quotas').mkdir(parents=True)
        quota=state/'quotas/codex.json'
        (root/'host').mkdir(mode=0o700)
        (root/'home').mkdir(mode=0o700)
        script=root/'cli.py';script.write_text(CLI)
        config=root/'launcher.json';config.write_text(json.dumps({'schema':2,'host_binary':str(BINARY.with_name('ptyhost')),'host_dir':str(root/'host'),'profiles':[{'id':'codex-fixture','source':'codex','executable':sys.executable,'args':[str(script)],'env':{'QUOTA_FIXTURE':str(root),'HOME':str(root/'home')}}]}))
        subprocess.run([str(BINARY),'--initialize-lifecycle',str(root/'ledger')],check=True,capture_output=True)
        port=free_port();env=node_env(root,port,'127.0.0.0/8')
        env.update(SESSIONDOCK_PTYHOST_DIR=str(root/'host'),SESSIONDOCK_LIFECYCLE_DIR=str(root/'ledger'),SESSIONDOCK_STATE_DIR=str(state),SESSIONDOCK_LAUNCHER_CONFIG=str(config))
        fake=stack.enter_context(DelayedQuotaServer(root/'app.sock',corpus.paths['sdk'],THREAD))
        base=stack.enter_context(server(corpus,BINARY,root/'app.sock',env))
        hubroot=root/'hub';hubroot.mkdir()
        hub=Hub(BINARY.with_name('sessiondock-hub'),hubroot,[SimpleNamespace(name='Quota node',token=TOKEN,port=port)])
        hub.start();stack.callback(hub.stop)
        with sync_playwright() as pw:
            browser=pw.chromium.launch()
            for mode,url in [('node',base),('hub',f'http://127.0.0.1:{hub.port}')]:
                for width,theme in [(1280,'light'),(390,'dark')]:
                    if mode=='node' and width==1280: quota.write_text(json.dumps({'source':'codex','captured_at':'2020-01-01T00:00:00Z','buckets':[{'id':'codex:primary','window':10080,'remaining_percent':89}]}))
                    context=browser.new_context(viewport={'width':width,'height':900},color_scheme=theme)
                    page=context.new_page();errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
                    page.goto(url);page.wait_for_function('T.listLoaded && S.sessions.length >= 2')
                    uid=page.evaluate("S.sessions.find(r=>r.sid==='external').uid")
                    page.route(re.compile(r'/api/live(?:\?|$)'),lambda r:r.fulfill(json={'known':True,'uids':[uid],'working_uids':[uid],'sdk_uids':[],'tmux_uids':[]}))
                    page.reload();page.wait_for_function('T.listLoaded')
                    card=page.locator(f'#side .item[data-uid="{uid}"]')
                    if width<720 and not card.is_visible():page.locator('#detail .mobile-back').click()
                    card.click()
                    expect(page.locator('.sdk-pane')).to_be_visible()
                    assert page.evaluate('CodexSdkView.readOnly')
                    # No selected-thread open or resume is needed for account reads.
                    assert not [q for q in fake.methods('thread/resume') if q.get('params',{}).get('threadId')=='external']
                    expect(page.locator('[data-gauge="7d"] b')).to_have_text('40%',timeout=30000)
                    expect(page.locator('.sdk-pane .sdk-credits')).to_contain_text('30,817')
                    expect(page.locator('[data-gauge="context"] b')).to_have_text('20%')
                    expect(page.locator('[data-gauge="context"]')).to_have_attribute('title',re.compile('最近落盘测量'))
                    credit=page.locator('.sdk-pane .sdk-credits')
                    assert credit.bounding_box()['x']+credit.bounding_box()['width']<=width
                    for selector in ['#sdk-model','#sdk-effort','#sdk-tier','#sdk-mode','#sdk-control-approval','#sdk-control-sandbox']:
                        if not page.locator(selector).is_visible():page.locator('#sdk-more').click()
                        expect(page.locator(selector)).to_be_visible()
                        expect(page.locator(selector)).to_be_disabled()
                    page.screenshot(path=str(args.screenshots/f'{mode}-{width}-disabled-controls.png'))
                    page.keyboard.press('Escape')
                    page.screenshot(path=str(args.screenshots/f'{mode}-{width}-sdk.png'))
                    tab=context.new_page();tab.goto(url+'/quotas.html')
                    expect(tab.locator('#quota-refresh')).to_be_enabled()
                    (root/'used').write_text('30.8')
                    tab.locator('#quota-refresh').click()
                    expect(tab.locator('[data-source="codex"] .quota-remaining b')).to_have_text('30.8%',timeout=30000)
                    expect(page.locator('[data-gauge="7d"] b')).to_have_text('30.8%')
                    page.locator('#a-chat').click()
                    expect(page.locator('#conversation-status .sdk-gauges')).to_contain_text('30.8%')
                    (root/'used').write_text('40')
                    tab.locator('#quota-refresh').click()
                    expect(page.locator('#conversation-status .sdk-gauges')).to_contain_text('40%',timeout=30000)
                    tab.close()
                    page.locator('#a-sdk').click()
                    captured=json.loads(quota.read_text())['captured_at']
                    count=(root/'calls').read_text().count('query')
                    page.locator('#a-chat').click()
                    expect(page.locator('#conversation-status .sdk-gauges')).to_contain_text('40%')
                    expect(page.locator('#conversation-status .sdk-gauges')).to_contain_text('30,817')
                    page.screenshot(path=str(args.screenshots/f'{mode}-{width}-conversation.png'))
                    page.locator('#a-sdk').click()
                    expect(page.locator('[data-gauge="7d"] b')).to_have_text('40%')
                    assert (root/'calls').read_text().count('query')==count
                    assert json.loads(quota.read_text())['captured_at']==captured
                    assert not errors,errors
                    if mode=='node' and width==1280:
                        original=corpus.paths['external'].read_bytes()
                        # Compaction invalidates the last request's context until
                        # the external process writes a new measurement.
                        with corpus.paths['external'].open('a') as stream:
                            stream.write(json.dumps(codex_row('compacted',{'message':'compacted'}))+'\n')
                        with page.expect_response('**/api/codex-sdk/recorded-usage?*'):page.reload()
                        expect(page.locator('[data-gauge="context"]')).to_be_hidden()
                        # Native recorded credits remain visible if the account
                        # service has no snapshot or cannot refresh.
                        corpus.paths['external'].write_bytes(original)
                        page.route(re.compile(r'/api/quotas(?:/refresh)?(?:\?|$)'),lambda r:r.fulfill(status=503,json={'error':'offline'}))
                        page.reload()
                        expect(page.locator('.sdk-pane .sdk-credits')).to_contain_text('37,358')
                        expect(page.locator('[data-gauge="context"] b')).to_have_text('20%')
                        page.screenshot(path=str(args.screenshots/'native-fallback.png'))
                        for balance in [None,'0']:
                            packet=codex_row('event_msg',{'type':'token_count','info':None,
                                'rate_limits':{'credits':{'balance':balance,'has_credits':False}}})
                            with corpus.paths['external'].open('a') as stream:stream.write(json.dumps(packet)+'\n')
                            with page.expect_response('**/api/codex-sdk/recorded-usage?*'):page.reload()
                            if balance is None:expect(page.locator('.sdk-pane .sdk-credits')).to_be_hidden()
                            else:expect(page.locator('.sdk-pane .sdk-credits')).to_have_text('🪙 0')
                        corpus.paths['external'].write_bytes(original)
                    page.goto(url+'/?'+urlencode({'sid':'codex:'+THREAD+'/agent:'+child}))
                    expect(page.locator('.sdk-history')).to_contain_text('Child usage is independent')
                    expect(page.locator('[data-gauge="context"] b')).to_have_text('50%')
                    assert page.evaluate('CodexSdkView.readOnly')
                    assert not [q for q in fake.methods('thread/resume') if q.get('params',{}).get('threadId')==child]
                    page.screenshot(path=str(args.screenshots/f'{mode}-{width}-child.png'))
                    context.close()
                    print('PASS',mode,width,theme,flush=True)
            # Full native account reads publish the same normalized snapshot.
            context=browser.new_context();page=context.new_page();page.goto(base)
            uid=corpus.uid('sdk')
            page.wait_for_function('T.listLoaded')
            page.locator(f'#side .item[data-uid="{uid}"]').click()
            page.locator('#sdk-composer .composer-run-action').click()
            expect(page.locator('#sdk-model')).to_have_value('fake-sol')
            expect(page.locator('[data-gauge="5h"] b')).to_have_text('40%')
            page.screenshot(path=str(args.screenshots/'managed-sdk.png'))
            tab=context.new_page();tab.goto(base+'/quotas.html')
            expect(tab.locator('#quota-refresh')).to_be_enabled()
            updated=page.request.get(base+'/api/quotas').json()
            def managed_refresh(route):
                updated['agents'][0]['captured_at']=datetime.now(timezone.utc).isoformat()
                route.fulfill(json=updated)
            tab.route('**/api/quotas/refresh',managed_refresh)
            updated['agents'][0]['buckets'][0]['remaining_percent']=69.2
            tab.locator('#quota-refresh').click()
            expect(page.locator('[data-gauge="5h"] b')).to_have_text('30.8%')
            updated['agents'][0]['buckets'][0]['remaining_percent']=60
            tab.locator('#quota-refresh').click()
            expect(page.locator('[data-gauge="5h"] b')).to_have_text('40%')
            tab.close()
            page.locator('#a-chat').click()
            expect(page.locator('#conversation-status [data-balance="reset-credits"]')).to_have_text('重置卡 2 张')
            expect(page.locator('#conversation-status .sdk-gauges')).to_contain_text('40%')
            page.screenshot(path=str(args.screenshots/'managed-conversation.png'))
            saved=page.request.get(base+'/api/quotas').json()['agents'][0]
            assert saved['buckets'][0]['remaining_percent']==60,saved
            # A failed explicit collection keeps the SDK's successful snapshot;
            # browser re-entry shares the cached failure without retry storms.
            (root/'fail').touch()
            failed=page.request.post(base+'/api/quotas/refresh?source=codex').json()['agents'][0]
            assert failed['error'] and failed['captured_at']==saved['captured_at'],failed
            page.reload();page.wait_for_function('T.listLoaded')
            page.locator(f'#side .item[data-uid="{corpus.uid("external")}"]').click()
            expect(page.locator('[data-gauge="5h"] b')).to_have_text('40%')
            expect(page.locator('[data-gauge="7d"] b')).to_have_text('25%')
            page.screenshot(path=str(args.screenshots/'failed-refresh.png'))
            # Two SDK pages share one in-flight native read. A separate fresh
            # collector may finish first; the delayed SDK value must not replace
            # its persisted snapshot or any already-open surface.
            (root/'fail').unlink()
            page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
            expect(page.locator('#sdk-input')).to_be_visible()
            page.wait_for_function('CodexSdkView.connected && !CodexSdkView.limitsRead')
            other_context=browser.new_context()
            other=other_context.new_page();other.goto(base)
            other.wait_for_function('T.listLoaded')
            other.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
            other.wait_for_function('CodexSdkView.connected && !CodexSdkView.limitsRead')
            quota_page=context.new_page();quota_page.goto(base+'/quotas.html')
            expect(quota_page.locator('#quota-refresh')).to_be_enabled()
            before=len(fake.methods('account/rateLimits/read'))
            fake.quota_gate=threading.Event()
            page.locator("#a-chat").click()
            page.locator("#a-sdk").click()
            page.wait_for_timeout(200)
            other.locator("#a-chat").click()
            other.locator("#a-sdk").click()
            other.wait_for_timeout(200)
            (root/'used').write_text('57')
            with quota_page.expect_response('**/api/quotas/refresh'):
                quota_page.locator('#quota-refresh').click()
            expect(quota_page.locator('#quota-refresh')).to_be_enabled()
            expect(quota_page.locator('[data-source="codex"] .quota-remaining b')).to_have_count(1)
            expect(quota_page.locator('[data-source="codex"] .quota-remaining b')).to_have_text('57%')
            expect(page.locator('[data-gauge="7d"] b')).to_have_text('57%')
            saved=page.request.get(base+'/api/quotas').json()['agents'][0]
            fake.quota_gate.set()
            page.wait_for_function('!CodexSdkView.limitsRead')
            other.wait_for_function('!CodexSdkView.limitsRead')
            assert len(fake.methods('account/rateLimits/read')) == before+1, 'native query must be shared'
            persisted=page.request.get(base+'/api/quotas').json()['agents'][0]
            assert persisted['captured_at']==saved['captured_at'], (saved,persisted)
            assert persisted['buckets']==saved['buckets'], (saved,persisted)
            expect(page.locator('[data-gauge="7d"] b')).to_have_text('57%')
            expect(other.locator('[data-gauge="7d"] b')).to_have_text('57%')
            page.screenshot(path=str(args.screenshots/'delayed-sdk-read.png'))
            # A later failed collection must not suppress a successful SDK read.
            other_context.close()
            fake.quota_gate=threading.Event()
            page.locator('#a-chat').click();page.locator('#a-sdk').click()
            page.wait_for_timeout(200)
            (root/'fail').touch()
            with quota_page.expect_response('**/api/quotas/refresh'):
                quota_page.locator('#quota-refresh').click()
            expect(quota_page.locator('#quota-refresh')).to_be_enabled()
            fake.quota_gate.set()
            page.wait_for_function('!CodexSdkView.limitsRead')
            expect(page.locator('[data-gauge="5h"] b')).to_have_text('40%')
            recovered=page.request.get(base+'/api/quotas').json()['agents'][0]
            assert recovered['error'] is None and recovered['buckets'][0]['remaining_percent']==60, recovered
            print('PASS shared native read, late success, late failure and persisted quota ordering', flush=True)
            context.close()
            browser.close()

if __name__=='__main__':main()
