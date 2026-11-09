#!/usr/bin/env python3
"""Portable Droid writer matching through browser open, using private snapshots.

Exercises the shared matcher on Linux; macOS ps/lsof and Windows CIM collection
still require native-platform acceptance. No real processes are signalled.
"""
import argparse
from contextlib import ExitStack
from datetime import datetime,timezone
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from playwright.sync_api import expect,sync_playwright
from droid_sdk_fixtures import DroidFixture
from history_fixtures import BINARY
from private_hosts import private_hosts
from node_auth_fixtures import node_env,free_port,TOKEN
from hub_fixtures import Hub


def check(root,hub_mode,shots):
    f=DroidFixture(root);pid=42420;started=1794200000
    log=f.home/'.factory/logs/droid-log-single.log';log.parent.mkdir(parents=True)
    snapshot=f.root/'proc/droid-processes.json'
    other='11111111-2222-4333-8444-555555555555'
    def log_entry(sid,offset):
        stamp=datetime.fromtimestamp(started+offset,timezone.utc).isoformat()
        return '['+stamp+'] INFO: session | Context: '+json.dumps({'process.pid':pid,'sessionId':sid,'tags':{'clientMode':'tui','sessionId':sid}})+'\n'
    cases=[
        ('windows-unknown',None,'droid.exe','',False),
        ('resume-selected',None,'droid.exe --resume '+f.sid,'',False),
        ('short-resume',None,'droid -r '+f.sid,'',False),
        ('different-sid',None,'droid.exe --resume '+other,'',True),
        ('mac-same-cwd',str(f.cwd),'droid','',False),
        ('mac-other-cwd',str(f.root/'elsewhere'),'droid','',True),
        ('stale-pid',None,'droid',log_entry(other,-1),False),
        ('log-selected',None,'droid --resume '+other,log_entry(f.sid,1),False),
        ('log-latest',None,'droid --resume '+f.sid,log_entry(f.sid,1)+log_entry(other,2)+log_entry(f.sid,-1),True),
    ]
    snapshot.write_text('[]')
    with ExitStack() as stack:
        stack.enter_context(private_hosts(f.root/'host'))
        port=free_port();extra=node_env(f.root,port,'127.0.0.0/8') if hub_mode else None
        base=stack.enter_context(f.server(BINARY,extra))
        if hub_mode:
            hroot=f.root/'hub';hroot.mkdir();h=Hub(BINARY.with_name('sessiondock-hub'),hroot,[SimpleNamespace(name='DroidWriters',token=TOKEN,port=port)])
            h.start();stack.callback(h.stop);base=f'http://127.0.0.1:{h.port}'
        with sync_playwright() as p:
            browser=p.chromium.launch(headless=True);page=browser.new_page(viewport={'width':1280,'height':900});errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
            for name,cwd,command,logs,allowed in cases:
                snapshot.write_text(json.dumps([{'pid':pid,'ppid':1,'command':command,'started':started,'cwd':cwd}]))
                log.write_text(logs)
                page.goto(base,wait_until='networkidle')
                card=page.locator(f'#side .item[data-uid$="{f.uid.split(":",1)[1]}"]');expect(card).to_be_visible();uid=card.get_attribute('data-uid');card.click()
                page.locator('#a-sdk').click()
                run=page.locator('#sdk-composer .composer-run-action');expect(run).to_be_visible()
                before=len(f.frames('spawn'))
                with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/open')) as opened:run.click()
                receipt=opened.value
                assert receipt.ok==allowed,(name,receipt.status,receipt.text())
                if allowed:
                    page.wait_for_function('NativeSdkView.connected')
                    stopped=page.request.post(base+'/api/native-sdk/stop',data={'uid':uid});assert stopped.ok,stopped.text()
                else:
                    assert 'droid_sdk_session_busy' in receipt.text(),receipt.text()
                    expect(page.locator('.native-sdk-pane')).to_contain_text('Droid')
                    assert len(f.frames('spawn'))==before,'rejected open spawned writer'
                if name in ('windows-unknown','log-latest'):page.screenshot(path=str(shots/(name+'.png')),full_page=True)
            assert not errors,errors;browser.close()
    print('PASS portable Droid writer matcher', 'hub' if hub_mode else 'node',flush=True)


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--screenshots',type=Path,default=Path('/tmp/sessiondock-droid-writers-browser'));args=parser.parse_args()
    for hub in (False,True):
        shots=args.screenshots/('hub' if hub else 'node');shots.mkdir(parents=True,exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='sessiondock-droid-writers-') as root:check(root,hub,shots)

if __name__=='__main__':main()
