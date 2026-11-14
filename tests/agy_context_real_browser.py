#!/usr/bin/env python3
"""Operator-only real Agy context via Chromium, private HOME and synthetic gateway."""
from contextlib import ExitStack
import argparse
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
from playwright.sync_api import sync_playwright,expect
from agy_real_browser import installed_agy
from agy_test_gateway import gateway, MODELS
from agy_sdk_browser import select_sdk,send,watch
MODEL = MODELS[0]
from history_fixtures import BINARY,REPO,Corpus,isolated_server
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--agy',type=Path,default=installed_agy())
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    root=Path(tempfile.mkdtemp(prefix='sessiondock-agy-context-real-'))
    print('ROOT',root,flush=True)
    for name in ('home','work','state','host','ledger','mirror','claude','codex','grok','config','data','cache','tmp','proc'):
        (root/name).mkdir(mode=0o700)
    native=root/'home/.gemini/antigravity-cli'
    onboarding=native/'cache/onboarding.json';onboarding.parent.mkdir(parents=True)
    onboarding.write_text(json.dumps({'consumerOnboardingComplete':True,'enterpriseOnboardingComplete':True,'onboardingComplete':True}))
    with ExitStack() as cleanup, gateway(root) as (url,records,errors):
        cleanup.callback(stop_private_sdkhosts,root)
        env={'HOME':str(root/'home'),'PATH':'/usr/bin:/bin','LANG':'C.UTF-8','TERM':'dumb',
            'AGY_CLI_DISABLE_AUTO_UPDATE':'1','AGY_LLM_GATEWAY_URL':url+'/v1','AGY_LLM_GATEWAY_MODELS':','.join(MODELS),
            'AGY_LLM_GATEWAY_API_KEY':'synthetic-local-fixture','AGY_LLM_GATEWAY_WIRE_PROTOCOL':'openai',
            'HTTP_PROXY':url,'HTTPS_PROXY':url,'ALL_PROXY':url,'NO_PROXY':'127.0.0.1,localhost,::1'}
        for key,folder in [('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_CACHE_HOME','cache'),('XDG_STATE_HOME','state'),('TMPDIR','tmp')]:env[key]=str(root/folder)
        argv=[str(args.agy),'--model',MODEL,'--input-format','stream-json','--output-format','stream-json']
        seed=subprocess.run(argv,input='',capture_output=True,text=True,cwd=root/'work',env=env,timeout=20)
        assert seed.returncode==0,seed.stderr
        init=json.loads(seed.stdout.splitlines()[0]);sid=init['conversation_id']
        assert init['init']['model']==MODEL
        launcher=root/'launcher.json'
        launcher.write_text(json.dumps({'schema':2,'host_binary':str(REPO/'target/debug/ptyhost'),
            'host_dir':str(root/'host'),'adapters':[],'profiles':[{'id':'agy-context-real','source':'agy',
            'executable':str(args.agy),'args':['--model',MODEL],'env':env}]}))
        initialize('--initialize-lifecycle',root/'ledger',args.binary)
        extra={'SESSIONDOCK_NATIVE_SDKS':'auto','SESSIONDOCK_AGY_HOME':str(native),'SESSIONDOCK_AGY_ROOT':str(root/'mirror')}
        with isolated_server(Corpus(root),args.binary,host_dir=root/'host',lifecycle_dir=root/'ledger',
                state_dir=root/'state',launcher_config=launcher,extra_env=extra) as (base,_),sync_playwright() as pw:
            browser=pw.chromium.launch(headless=True)
            page=browser.new_page(viewport={'width':1280,'height':900});page.set_default_timeout(20000)
            try:
                page.goto(base,wait_until='domcontentloaded')
                page.wait_for_function('sid=>S.sessions.some(r=>r.sid===sid)',arg=sid)
                uid=page.evaluate('sid=>S.sessions.find(r=>r.sid===sid).uid',sid)
                select_sdk(page,uid,connected=False)
                page.locator('#sdk-composer .composer-run-action').click()
                page.wait_for_function('NativeSdkView.connected')
                watch(page,uid)
                send(page,'Private context measurement fixture')
                page.wait_for_function('NativeSdkView.raw.context_usage?.max_tokens > 0',timeout=30000)
                measurement=page.evaluate('NativeSdkView.raw.context_usage')
                assert measurement['used_tokens']>0 and measurement['percent']>0,measurement
                footer=page.locator('.native-sdk-pane .sdk-gauges')
                expect(footer).to_contain_text('上下文')
                hosts=list((root/'state/sdkhost').glob('*/status.json'))
                pid=json.loads(hosts[0].read_text())['pid']
                page.wait_for_function('!NativeSdkView.running')
                before=len(records)
                page.locator('#sdk-input').fill('/context');page.locator('#sdk-send').click()
                page.wait_for_function('!NativeSdkView.loadingUsage')
                assert json.loads(hosts[0].read_text())['pid']==pid
                assert len(records)==before,'context query must not run a model'
                page.reload(wait_until='domcontentloaded')
                expect(page.locator('.native-sdk-pane .sdk-gauges')).to_contain_text('上下文',timeout=20000)
                print('NATIVE_CONTEXT',measurement,flush=True)
                assert not errors,errors
                assert records and all(r['request']['model']==MODELS[0 if r['kind']=='main' else 1] for r in records)
                print('PASS real Agy context: native estimate, footer, slash refresh without model turn, same PID/SID and reload',flush=True)
            finally:
                browser.close()

if __name__=='__main__':main()
