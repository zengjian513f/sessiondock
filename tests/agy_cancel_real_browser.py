#!/usr/bin/env python3
"""Operator-only real Agy cancellation via Chromium; private HOME and loopback model.

No credentials, paid models, everyday settings, or production sessions. The
synthetic model starts only a bounded private heartbeat worker and a self
subagent, then holds both model requests while the browser stops the parent.
"""
from contextlib import contextmanager, ExitStack
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import argparse
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import threading
import time

from playwright.sync_api import expect, sync_playwright
from agy_real_browser import installed_agy
from agy_sdk_browser import select_sdk, send, watch, idle
from agy_tools_real_browser import MODEL, WIRE_MODEL
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize


@contextmanager
def gateway(root):
    state = {'sid': None, 'stage': 0, 'errors': [], 'models': [], 'child_sid': None}
    for key in ('parent_waiting', 'child_waiting', 'release_parent', 'release_child'):
        state[key] = threading.Event()
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_): pass
        def do_CONNECT(self): self.send_error(403)
        def do_GET(self): self.send_error(403)
        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            state['models'].append(request.get('model'))
            if request.get('model') != WIRE_MODEL:
                state['errors'].append('unexpected wire model'); self.send_error(403); return
            messages = request.get('messages', [])
            system = str([m for m in messages if m['role'] == 'system'])
            match = re.search(r'Conversation ID: ([a-f0-9-]+)', system)
            sid = match[1] if match else None
            tools = {t['function']['name']:t['function'] for t in request.get('tools', [])}
            (root/'tool-schemas.json').write_text(json.dumps(tools, indent=2)) if tools else None
            delta, finish = {'role':'assistant','content':'PRIVATE_TITLE'}, 'stop'
            if sid and sid != state['sid']:
                state['child_sid'] = sid
                state['child_waiting'].set()
                state['release_child'].wait(90)
                delta['content'] = 'CHILD_SURVIVED_PARENT_CANCEL'
            elif sid:
                stage = state['stage']; state['stage'] += 1
                if stage == 0:
                    name, args = 'run_command', {'CommandLine':'python3 '+shlex.quote(str(root/'work/worker.py')),
                        'WaitMsBeforeAsync':500,'toolSummary':'Private bounded heartbeat worker'}
                elif stage == 1:
                    name, args = 'invoke_subagent', {'TypeName':'self', 'Role':'Private cancellation fixture', 'Prompt':'PRIVATE_CHILD_WAIT', 'Model':'inherit'}
                else:
                    if stage == 2:
                        state['parent_waiting'].set(); state['release_parent'].wait(90)
                    delta['content'] = 'PARENT_CONTINUED_SAME_RUNTIME'
                    name = None
                if stage < 2:
                    assert name in tools, (name, list(tools))
                    print('TOOL',name,flush=True)
                    delta = {'role':'assistant','tool_calls':[{'index':0,'id':f'private_{stage}',
                        'type':'function','function':{'name':name,'arguments':json.dumps(args)}}]}
                    finish='tool_calls'
            try:
                self.send_response(200); self.send_header('Content-Type','text/event-stream');self.end_headers()
                for part,reason in ((delta,None),({},finish)):
                    chunk={'id':'private-fixture','object':'chat.completion.chunk','model':WIRE_MODEL,
                        'choices':[{'index':0,'delta':part,'finish_reason':reason}]}
                    self.wfile.write(('data: '+json.dumps(chunk)+'\n\n').encode())
                self.wfile.write(b'data: [DONE]\n\n')
            except (BrokenPipeError,ConnectionResetError): pass
    server=ThreadingHTTPServer(('127.0.0.1',0),Handler);server.daemon_threads=True
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
    try: yield f'http://127.0.0.1:{server.server_port}',state
    finally:
        state['release_parent'].set();state['release_child'].set()
        server.shutdown();server.server_close();thread.join(timeout=5)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--agy',type=Path,default=installed_agy())
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    root=Path(tempfile.mkdtemp(prefix='sessiondock-agy-cancel-real-'))
    print('ROOT',root,flush=True)
    for name in ('home','work','state','host','ledger','mirror','claude','codex','grok','config','data','cache','tmp','proc'):
        (root/name).mkdir(mode=0o700)
    native=root/'home/.gemini/antigravity-cli'
    onboarding=native/'cache/onboarding.json';onboarding.parent.mkdir(parents=True)
    onboarding.write_text(json.dumps({'consumerOnboardingComplete':True,'enterpriseOnboardingComplete':True,'onboardingComplete':True}))
    settings=root/'home/.antigravity/settings.json';settings.parent.mkdir()
    settings.write_text(json.dumps({'models':{'list':[{'modelId':MODEL,'displayName':'Private fixture','toolFormatterType':'TOOL_FORMATTER_TYPE_NONE'}]}}))
    (root/'work/worker.py').write_text("import os,time\nfrom pathlib import Path\np=Path(__file__).parent\n(p/'worker.pid').write_text(str(os.getpid()))\nend=time.monotonic()+150\nwhile time.monotonic()<end and not (p/'release-worker').exists():\n (p/'heartbeat').write_text(str(time.monotonic()))\n time.sleep(.05)\n")
    with ExitStack() as cleanup, gateway(root) as (url,state):
        cleanup.callback(stop_private_sdkhosts,root)
        cleanup.callback(lambda:(root/'work/release-worker').touch())
        env={'HOME':str(root/'home'),'PATH':'/usr/bin:/bin','LANG':'C.UTF-8','TERM':'dumb',
            'AGY_CLI_DISABLE_AUTO_UPDATE':'1','AGY_LLM_GATEWAY_URL':url+'/v1','AGY_LLM_GATEWAY_MODELS':MODEL,
            'AGY_LLM_GATEWAY_API_KEY':'synthetic-local-fixture','AGY_LLM_GATEWAY_WIRE_PROTOCOL':'openai',
            'HTTP_PROXY':url,'HTTPS_PROXY':url,'ALL_PROXY':url,'NO_PROXY':'127.0.0.1,localhost,::1'}
        for key,folder in [('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_CACHE_HOME','cache'),('XDG_STATE_HOME','state'),('TMPDIR','tmp')]:env[key]=str(root/folder)
        argv=[str(args.agy),'--model',MODEL,'--input-format','stream-json','--output-format','stream-json']
        seed=subprocess.run(argv,input='',capture_output=True,text=True,cwd=root/'work',env=env,timeout=20)
        assert seed.returncode==0,seed.stderr
        init=json.loads(seed.stdout.splitlines()[0]);sid=init['conversation_id'];state['sid']=sid
        assert init['init']['model']==MODEL
        launcher=root/'launcher.json'
        launcher.write_text(json.dumps({'schema':2,'host_binary':str(REPO/'target/debug/ptyhost'),
            'host_dir':str(root/'host'),'adapters':[],'profiles':[{'id':'agy-cancel-real','source':'agy',
            'executable':str(args.agy),'args':['--model',MODEL,'--dangerously-skip-permissions'],'env':env}]}))
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
                select_sdk(page,uid);watch(page,uid)
                send(page,'Start the private cancellation fixture')
                assert state['parent_waiting'].wait(20),'parent request not active'
                assert state['child_waiting'].wait(20),'subagent request not active'
                heartbeat=float((root/'work/heartbeat').read_text())
                hosts=list((root/'state/sdkhost').glob('*/status.json'))
                pid=json.loads(hosts[0].read_text())['pid']
                print('BEFORE CANCEL',pid,state['child_sid'],flush=True)
                with page.expect_response(lambda r: '/api/native-sdk/interrupt' in r.url) as stopped:
                    page.locator('#sdk-stop').click()
                assert stopped.value.ok, stopped.value.text()
                state['release_parent'].set()
                # Headless result waits for background work. Assert preservation
                # before explicitly allowing our bounded private tasks to finish.
                page.wait_for_timeout(5500)
                os.kill(pid,0)
                assert float((root/'work/heartbeat').read_text())>heartbeat
                assert not state['release_child'].is_set()
                state['release_child'].set()
                (root/'work/release-worker').touch()
                idle(page)
                assert page.evaluate('NativeSdkView.raw.last_turn.status')=='interrupted'
                os.kill(pid,0)
                state['release_parent'].set();state['release_child'].set()
                child=native/'brain'/state['child_sid']/'.system_generated/logs/transcript_full.jsonl'
                deadline=time.monotonic()+20
                while not child.exists() or 'CHILD_SURVIVED_PARENT_CANCEL' not in child.read_text():
                    assert time.monotonic()<deadline,'child did not finish after parent cancel'
                    page.wait_for_timeout(100)
                send(page,'Continue after cancellation')
                expect(page.locator('.sdk-live')).to_contain_text('PARENT_CONTINUED_SAME_RUNTIME')
                idle(page)
                assert json.loads(hosts[0].read_text())['pid']==pid
                assert not state['errors'] and state['models'] and set(state['models'])=={WIRE_MODEL}
                print('PASS real Agy cancel: accepted RPC, same PID/SID, worker and child survive beyond 5s; after tasks finish, next turn succeeds',flush=True)
            finally:browser.close()

if __name__=='__main__':main()
