#!/usr/bin/env python3
"""Real Agy/CodeBuddy/Claude history actions via Chromium; private homes, synthetic gateway, no paid calls."""
from contextlib import ExitStack, contextmanager
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import uuid
from types import SimpleNamespace
from urllib.parse import unquote, urlsplit

from playwright.sync_api import expect, sync_playwright
from agy_real_browser import installed_agy
from agy_test_gateway import gateway, MODELS
from codebuddy_sdk_fixtures import CodebuddyFixture, native_records
from claude_sdk_fixtures import ClaudeFixture
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN, node_env, free_port
from sdk_ui_fixtures import start_sdk
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize

PROMPTS = ['保留第一轮 native-first', '回退第二轮 native-second', '删除第三轮 native-third']


@contextmanager
def native_server(source, root, args):
    with ExitStack() as stack:
        stack.callback(stop_private_sdkhosts, root)
        server_stack = stack.enter_context(ExitStack())
        port = free_port()
        extra = node_env(root, port, '127.0.0.1/32')
        if source == 'codebuddy':
            fixture = CodebuddyFixture(root)
            rows = native_records(fixture.sid, fixture.cwd, [(p, 'echo: '+p) for p in PROMPTS])
            for row in list(rows):
                if 'model' in row.get('providerData',{}):row['providerData']['model']='glm-5.3-flash'
                if row.get('role') == 'user':
                    rows.append({'type':'file-history-snapshot','messageId':row['id'],'parentId':row['parentId'],
                        'timestamp':row['timestamp'],'isSnapshotUpdate':False,
                        'snapshot':{'messageId':row['id'],'trackedFileBackups':{}},
                        'sessionId':fixture.sid,'cwd':str(fixture.cwd)})
            fixture.path.write_text(''.join(json.dumps(row)+'\n' for row in rows))
            config = json.loads(fixture.launcher.read_text())
            config['profiles'][0].update(executable=str(args.codebuddy),args=['--model','glm-5.3-flash','--effort','low'],env={
                'HOME':str(fixture.home),'CODEBUDDY_CONFIG_DIR':str(fixture.config),'PATH':'/usr/bin:/bin',
                'HTTP_PROXY':'http://127.0.0.1:1','HTTPS_PROXY':'http://127.0.0.1:1','ALL_PROXY':'http://127.0.0.1:1',
                'CODEBUDDY_API_KEY':'synthetic-key','CODEBUDDY_TELEMETRY_ENABLED':'false'})
            fixture.launcher.write_text(json.dumps(config))
            server_factory = lambda: fixture.server(args.binary, extra_env=extra, preserve_sdkhosts=True)
            base = server_stack.enter_context(server_factory())
            sid, cwd = fixture.sid, fixture.cwd
            records, errors = [], []
        elif source == 'claude':
            fixture = ClaudeFixture(root)
            rows, parent = [], None
            for index, prompt in enumerate(PROMPTS):
                for role in ('user','assistant'):
                    ident = str(uuid.uuid4())
                    rows.append({'type':role,'uuid':ident,'parentUuid':parent,'sessionId':fixture.sid,
                        'cwd':str(fixture.cwd),'isSidechain':False,'timestamp':f'2026-11-15T12:00:0{index}Z',
                        'message':{'role':role,'content':prompt if role=='user' else [{'type':'text','text':'echo: '+prompt}],
                            **({'id':str(uuid.uuid4()),'model':'claude-haiku-5-5','stop_reason':'end_turn','usage':{'input_tokens':1,'output_tokens':1}} if role=='assistant' else {})}})
                    parent = ident
            fixture.path.write_text(''.join(json.dumps(row)+'\n' for row in rows))
            config = json.loads(fixture.launcher.read_text())
            config['profiles'][0].update(executable=str(args.claude),args=['--model','claude-haiku-5-5','--effort','low'],env={
                'HOME':str(fixture.home),'CLAUDE_CONFIG_DIR':str(fixture.home/'.claude'),'PATH':'/usr/bin:/bin',
                'HTTP_PROXY':'http://127.0.0.1:1','HTTPS_PROXY':'http://127.0.0.1:1','ALL_PROXY':'http://127.0.0.1:1',
                'ANTHROPIC_API_KEY':'synthetic-key','CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC':'1'},env_remove=['ANTHROPIC_AUTH_TOKEN'])
            fixture.launcher.write_text(json.dumps(config))
            server_factory = lambda: fixture.server(args.binary, extra_env=extra, preserve_sdkhosts=True)
            base = server_stack.enter_context(server_factory())
            sid, cwd = fixture.sid, fixture.cwd
            records, errors = [], []
        else:
            for name in ('home','work','state','host','ledger','mirror','claude','codex','grok','config','data','cache','tmp','proc'):
                (root/name).mkdir(mode=0o700)
            native = root/'home/.gemini/antigravity-cli'
            onboarding = native/'cache/onboarding.json'; onboarding.parent.mkdir(parents=True)
            onboarding.write_text(json.dumps(dict(consumerOnboardingComplete=True, enterpriseOnboardingComplete=True, onboardingComplete=True)))
            url, records, errors = stack.enter_context(gateway(root))
            env = {'HOME':str(root/'home'),'PATH':'/usr/bin:/bin','LANG':'C.UTF-8','TERM':'dumb',
                'AGY_CLI_DISABLE_AUTO_UPDATE':'1','AGY_LLM_GATEWAY_URL':url+'/v1','AGY_LLM_GATEWAY_MODELS':','.join(MODELS),
                'AGY_LLM_GATEWAY_API_KEY':'synthetic-local-fixture','AGY_LLM_GATEWAY_WIRE_PROTOCOL':'openai',
                'HTTP_PROXY':url,'HTTPS_PROXY':url,'ALL_PROXY':url,'NO_PROXY':'127.0.0.1,localhost,::1'}
            for key,folder in [('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_CACHE_HOME','cache'),('XDG_STATE_HOME','state'),('TMPDIR','tmp')]: env[key]=str(root/folder)
            argv = [str(args.agy),'--model',MODELS[0],'--input-format','stream-json','--output-format','stream-json']
            # EOF queues all three turns; the native CLI completes them before exiting.
            seed = subprocess.run(argv,input=''.join(json.dumps({'event':'user','message':{'content':p}})+'\n' for p in PROMPTS),
                capture_output=True,text=True,cwd=root/'work',env=env,timeout=45)
            assert seed.returncode == 0, seed.stderr
            init = json.loads(seed.stdout.splitlines()[0]); sid = init['conversation_id']
            assert init['init']['model'] == MODELS[0]
            cwd = root/'work'
            launcher = root/'launcher.json'
            launcher.write_text(json.dumps({'schema':2,'host_binary':str(REPO/'target/debug/ptyhost'),'host_dir':str(root/'host'),
                'adapters':[],'profiles':[{'id':'agy-history-real','source':'agy','executable':str(args.agy),'args':['--model',MODELS[0]],'env':env}]}))
            initialize('--initialize-lifecycle',root/'ledger',args.binary)
            extra.update(SESSIONDOCK_NATIVE_SDKS='auto',SESSIONDOCK_AGY_HOME=str(native),SESSIONDOCK_AGY_ROOT=str(root/'mirror'))
            server_factory = lambda: isolated_server(Corpus(root),args.binary,host_dir=root/'host',lifecycle_dir=root/'ledger',
                state_dir=root/'state',launcher_config=launcher,extra_env=extra)
            base,_ = server_stack.enter_context(server_factory())
        protected = cwd/'preserve.txt'; protected.write_text('keep workspace edit')
        hubroot = root/'hub'; hubroot.mkdir()
        hub = Hub(args.binary.with_name('sessiondock-hub'),hubroot,[SimpleNamespace(name='HistoryNode',port=port,token=TOKEN)])
        hub.start(); stack.callback(hub.stop)
        peer=SimpleNamespace(base=base,hub=f'http://127.0.0.1:{hub.port}',sid=sid,records=records,errors=errors,protected=protected)
        def restart():
            server_stack.close()
            result=server_stack.enter_context(server_factory())
            peer.base=result[0] if isinstance(result,tuple) else result
        peer.restart=restart
        yield peer


def open_source(page, base, sid):
    page.goto(base,wait_until='domcontentloaded')
    page.wait_for_function('async sid => {await loadSessions(true);return S.sessions.some(s=>s.sid===sid)}',arg=sid)
    uid = page.evaluate('sid=>S.sessions.find(s=>s.sid===sid).uid',sid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    if not page.locator('.sdk-pane').count(): page.locator('#a-sdk').click()
    start_sdk(page)
    page.wait_for_function('SdkView.connected && !SdkView.running')
    return uid


def bubble(page, text):
    return page.locator('.sdk-msgs .msg[data-role="user"]').filter(has_text=text)


def perform(page, message, action, endpoint):
    print('ACTION',endpoint,flush=True)
    message.locator('.sdk-bubble-menu').get_by_role('button',name={'分支':'从此处之前分支','回退':'回退到此处之前'}.get(action,action),exact=True).click()
    modal = page.locator('dialog.app-popup[open]')
    with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/'+endpoint)) as response:
        modal.get_by_role('button',name='分支' if endpoint=='fork' else '回退',exact=True).click()
    assert response.value.ok, response.value.text()
    return response.value.json(), response.value.request.post_data_json


def exercise(browser, peer, base, label, source, args):
    context = browser.new_context(viewport={'width':1280,'height':900},service_workers='block')
    page = context.new_page(); page.set_default_timeout(25000)
    page_errors=[]
    page.on('pageerror',lambda error:page_errors.append(str(error)))
    page.on('response', lambda r: print('HTTP_ERROR',r.status,r.url,flush=True) if '/api/native-sdk/' in r.url and r.status>=400 and not r.url.endswith('/usage') else None)
    print('OPEN',source,label,flush=True)
    uid = open_source(page,base,peer.sid)
    expect(bubble(page,PROMPTS[2])).to_have_count(1)
    page.wait_for_function('model=>SdkView.raw.model===model',arg={'agy':MODELS[0],'codebuddy':'glm-5.3-flash','claude':'claude-haiku-5-5'}[source])
    selected = bubble(page,PROMPTS[1]); expect(selected).to_have_count(1)
    expect(selected.locator('.sdk-bubble-menu').get_by_role('button',name='从此处之前分支',exact=True)).to_be_visible()
    expect(selected.locator('.sdk-bubble-menu').get_by_role('button',name='回退到此处之前',exact=True)).to_be_visible()
    page.locator('#sdk-input').fill('取消时保留草稿')
    selected.locator('.sdk-bubble-menu').get_by_role('button',name='从此处之前分支',exact=True).click()
    page.locator('dialog.app-popup[open]').get_by_role('button',name='取消',exact=True).click()
    expect(page.locator('#sdk-input')).to_have_value('取消时保留草稿')
    page.locator('#sdk-input').fill('')
    for width,theme in [(1280,'light'),(1280,'dark'),(390,'light'),(390,'dark')]:
        page.set_viewport_size({'width':width,'height':900});page.emulate_media(color_scheme=theme)
        page.evaluate('()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))')
        if not page.locator('.sdk-pane').is_visible():page.locator(f'#side .item[data-uid="{uid}"]').click()
        page.wait_for_function('SdkView.connected && !SdkView.readOnly')
        expect(selected.locator('.sdk-bubble-menu').get_by_role('button',name='回退到此处之前',exact=True)).to_be_visible()
        if args.screenshots:page.screenshot(path=str(args.screenshots/f'{source}-{label}-{width}-{theme}.png'),full_page=True)
    page.set_viewport_size({'width':1280,'height':900})
    page.evaluate('()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))')
    if label=='hub':
        assistant=page.locator('.sdk-msgs .msg[data-role="assistant"]').filter(has_text='echo: '+PROMPTS[0])
        receipt,request=perform(page,assistant,'从此处创建分支','fork')
    else:
        receipt,request=perform(page,selected,'分支','fork')
    fork_uid = receipt['uid']; assert fork_uid!=uid
    page.wait_for_function('uid=>SdkView.uid===uid',arg=fork_uid)
    if source != 'claude':
        expect(bubble(page,PROMPTS[0])).to_have_count(1,timeout=20000)
        expect(bubble(page,PROMPTS[1])).to_have_count(0)
        expect(bubble(page,PROMPTS[2])).to_have_count(0)
    else:
        # Claude persists a deferred native fork only on its first real input.
        # No fabricated prompt is sent just to materialize a transcript.
        assert receipt['session']['sdk_fork']['before']==request['before']
        assert receipt['session']['sdk_fork']['resume_at']
    expect(page.locator('#sdk-input')).to_have_value(PROMPTS[1] if label=='node' else '')
    retry = page.request.post(base+'/api/native-sdk/fork',data=request)
    assert retry.ok and retry.json()['uid']==fork_uid,retry.text()
    page.reload(wait_until='domcontentloaded')
    if source != 'claude':
        expect(bubble(page,PROMPTS[0])).to_have_count(1,timeout=20000)
        expect(bubble(page,PROMPTS[1])).to_have_count(0)
    if source != 'claude':
        start_sdk(page)
        # Hold catalog/body publication after a successful native receipt. This
        # reproduces the real mirror/index race without timing-dependent sleeps.
        delayed = source == 'codebuddy' and label == 'node'
        unavailable_reads = []
        known = set(page.evaluate('S.sessions.map(row=>row.uid)'))
        def pending_catalog(route):
            response = route.fetch(); data = response.json()
            if isinstance(data.get('sessions'),list):
                data['sessions'] = [row for row in data['sessions'] if row['uid'] in known]
                data['sig'] = 'synthetic-pending-publication'
            route.fulfill(response=response,json=data)
        def pending_body(route):
            uid = unquote(urlsplit(route.request.url).path.rsplit('/',1)[-1])
            if uid in known:route.continue_()
            else:
                unavailable_reads.append(uid)
                route.fulfill(status=404,json={'error':'会话不存在'})
        if delayed:
            page.route('**/api/sessions?*',pending_catalog)
            page.route('**/api/messages/**',pending_body)
        page.locator('#sdk-input').fill('/fork');page.locator('#sdk-send').click()
        with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/fork')) as full:
            page.locator('dialog.app-popup[open]').get_by_role('button',name='分支',exact=True).click()
        assert full.value.ok,full.value.text()
        page.wait_for_function('uid=>SdkView.uid===uid && SdkView.connected',arg=full.value.json()['uid'])
        if delayed:
            expect(page.locator('#sdk-input')).to_have_value('')
            assert not unavailable_reads,unavailable_reads
            assert not page.evaluate('SdkView.historyFailed')
            page.unroute('**/api/sessions?*',pending_catalog)
            page.unroute('**/api/messages/**',pending_body)
            # Deliver the inventory update after the initial action completed.
            page.evaluate('loadSessions(true,true)')
        try:
            expect(bubble(page,PROMPTS[0])).to_have_count(1,timeout=20000)
        except Exception:
            print('FORK_STATE',page.evaluate('JSON.stringify({uid:SdkView.uid,connected:SdkView.connected,history:SdkView.historyData,pending:SdkView.pendingAtOpen,notice:SdkView.notice?.textContent,rows:S.sessions.filter(r=>r.uid===SdkView.uid),sdk:T.sdkSessions.filter(r=>r.uid===SdkView.uid)})'),flush=True)
            page.screenshot(path=str(REPO/'target/history-actions-failure.png'),full_page=True)
            raise
        expect(bubble(page,PROMPTS[1])).to_have_count(0)
        empty,_=perform(page,bubble(page,PROMPTS[0]),'分支','fork')
        page.wait_for_function('uid=>SdkView.uid===uid && SdkView.connected',arg=empty['uid'])
        expect(page.locator('.sdk-msgs .msg[data-role="user"]')).to_have_count(0)
        expect(page.locator('#sdk-input')).to_have_value(PROMPTS[0])
    print('OPEN',source,label,flush=True)
    uid = open_source(page,base,peer.sid)
    expect(bubble(page,PROMPTS[2])).to_have_count(1)
    observer=context.new_page();observer.set_default_timeout(25000)
    open_source(observer,base,peer.sid)
    expect(bubble(observer,PROMPTS[2])).to_have_count(1)
    receipt,_ = perform(page,bubble(page,PROMPTS[1]),'回退','revert')
    assert receipt.get('session_id',receipt.get('raw',{}).get('session_id'))==peer.sid,receipt
    expect(bubble(page,PROMPTS[0])).to_have_count(1,timeout=20000)
    expect(bubble(page,PROMPTS[1])).to_have_count(0)
    expect(bubble(page,PROMPTS[2])).to_have_count(0)
    expect(page.locator('#sdk-input')).to_have_value(PROMPTS[1])
    expect(bubble(observer,PROMPTS[0])).to_have_count(1,timeout=20000)
    expect(bubble(observer,PROMPTS[1])).to_have_count(0,timeout=20000)
    expect(bubble(observer,PROMPTS[2])).to_have_count(0)
    page.reload(wait_until='domcontentloaded')
    expect(bubble(page,PROMPTS[0])).to_have_count(1,timeout=20000)
    expect(bubble(page,PROMPTS[1])).to_have_count(0)
    start_sdk(page)
    if source == 'agy':
        from agy_sdk_browser import send
        send(page, '回退后的新输入 native-replacement')
        expect(page.locator('.sdk-msgs')).to_contain_text('echo: 回退后的新输入 native-replacement',timeout=30000)
        page.wait_for_function('!SdkView.running')
        expect(bubble(page,PROMPTS[1])).to_have_count(0)
        expect(bubble(page,PROMPTS[2])).to_have_count(0)
        main = [row for row in peer.records if row['kind']=='main'][-1]
        context_text = json.dumps(main['request'],ensure_ascii=False)
        assert 'native-first' in context_text and 'native-second' not in context_text and 'native-third' not in context_text,context_text
    # Slash command must include persisted history and send the native user identity.
    page.locator('#sdk-input').fill('/revert');page.locator('#sdk-send').click()
    modal = page.locator('#sdk-action-dialog[open]')
    expect(modal.locator('select option')).to_have_count(2 if source=='agy' else 1)
    with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/revert')) as response:
        modal.get_by_role('button',name='回退',exact=True).click()
    assert response.value.ok,response.value.text()
    expect(page.locator('.sdk-msgs .msg[data-role="user"]')).to_have_count(0)
    page.reload(wait_until='domcontentloaded')
    expect(page.locator('.sdk-msgs .msg[data-role="user"]')).to_have_count(0)
    last_revert=response.value.request.post_data_json
    peer.restart()
    base=peer.base if label=='node' else peer.hub
    open_source(page,base,peer.sid)
    expect(page.locator('.sdk-msgs .msg[data-role="user"]')).to_have_count(0)
    replay=page.request.post(base+'/api/native-sdk/revert',data=last_revert)
    assert replay.ok,replay.text()
    if source=='agy':
        send(page,'重连后的新输入 native-after-restart')
        expect(page.locator('.sdk-msgs')).to_contain_text('echo: 重连后的新输入 native-after-restart',timeout=30000)
        page.wait_for_function('!SdkView.running')
        context_text=json.dumps([row for row in peer.records if row['kind']=='main'][-1]['request'],ensure_ascii=False)
        assert all(token not in context_text for token in ['native-first','native-second','native-third','native-replacement'])
    assert peer.protected.read_text()=='keep workspace edit'
    assert not peer.errors,peer.errors
    assert not page_errors,page_errors
    context.close()
    print('PASS',source,label,'native fork, source preservation, retry, rewind, slash first-user, refresh, restart/retry, unchanged workspace',flush=True)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--agy',type=Path,default=installed_agy())
    parser.add_argument('--codebuddy',type=Path,default=Path.home()/'.local/lib/npm-global/bin/codebuddy')
    parser.add_argument('--claude',type=Path,default=Path.home()/'.local/bin/claude')
    parser.add_argument('--source',choices=['agy','codebuddy','claude','all'],default='all')
    parser.add_argument('--transport',choices=['node','hub','all'],default='all')
    parser.add_argument('--screenshots',type=Path)
    args=parser.parse_args();args.binary=args.binary.resolve()
    if args.screenshots:args.screenshots.mkdir(parents=True,exist_ok=True)
    with sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            for source in (['agy','codebuddy','claude'] if args.source=='all' else [args.source]):
                for transport in (['node','hub'] if args.transport=='all' else [args.transport]):
                    root=Path(tempfile.mkdtemp(prefix=f'sd-{source}-history-{transport}-'));print('ROOT',root,flush=True)
                    with native_server(source,root,args) as peer:
                        exercise(browser,peer,peer.base if transport=='node' else peer.hub,transport,source,args)
        finally:browser.close()

if __name__=='__main__':main()
