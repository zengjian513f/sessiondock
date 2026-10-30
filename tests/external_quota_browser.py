"""External Codex and conversation views share the node's cached collector."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from types import SimpleNamespace

from playwright.sync_api import sync_playwright, expect
from codex_sdk_browser import build, server, THREAD
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, codex_row, codex_message
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
  print(json.dumps({'id':q['id'],'result':{'rateLimits':{'primary':{'usedPercent':40,'windowDurationMins':10080,'resetsAt':1800000000},'credits':{'balance':'30816.7'}}}}),flush=True)
'''


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--screenshots', type=Path, required=True)
    args=parser.parse_args();args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-external-quota-') as temp, ExitStack() as stack:
        root=Path(temp);corpus=build(root)
        corpus.put('external','codex',[codex_row('session_meta',{'id':'external','cwd':'/synthetic/usage'}),
            codex_message('user','外部 CLI 用量：同一机器共享额度，保留独立会话上下文')],[])
        state=root/'state';(state/'quotas').mkdir(parents=True)
        quota=state/'quotas/codex.json'
        (root/'host').mkdir(mode=0o700)
        (root/'home').mkdir(mode=0o700)
        script=root/'cli.py';script.write_text(CLI)
        config=root/'launcher.json';config.write_text(json.dumps({'schema':2,'host_binary':str(BINARY.with_name('ptyhost')),'host_dir':str(root/'host'),'profiles':[{'id':'codex-fixture','source':'codex','executable':sys.executable,'args':[str(script)],'env':{'QUOTA_FIXTURE':str(root),'HOME':str(root/'home')}}]}))
        subprocess.run([str(BINARY),'--initialize-lifecycle',str(root/'ledger')],check=True,capture_output=True)
        port=free_port();env=node_env(root,port,'127.0.0.0/8')
        env.update(SESSIONDOCK_PTYHOST_DIR=str(root/'host'),SESSIONDOCK_LIFECYCLE_DIR=str(root/'ledger'),SESSIONDOCK_STATE_DIR=str(state),SESSIONDOCK_LAUNCHER_CONFIG=str(config))
        fake=stack.enter_context(FakeAppServer(root/'app.sock',corpus.paths['sdk'],THREAD))
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
                    page.screenshot(path=str(args.screenshots/f'{mode}-{width}-sdk.png'))
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
            context.close()
            browser.close()

if __name__=='__main__':main()
