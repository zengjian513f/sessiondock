#!/usr/bin/env python3
"""Dedicated SDK attribution from actual private Claude/Grok/Agy host bindings."""
import argparse
from contextlib import ExitStack
import os
from pathlib import Path
import subprocess
import shutil
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from process_links_browser import wait_for
from sdk_resources_fixtures import peer, serve, active_hosts
from sdkhost_fixtures import stop_private_sdkhosts
from spawned_by_fixtures import START, proc_pid


def bindings(page, sid, pid, present=True):
    page.wait_for_function('''async ({sid,pid,present}) => {
      const r=await (await fetch('/api/process-links')).json();
      const owned=r.bindings.filter(b=>b.session.sid===sid).map(b=>b.process.pid);
      return present ? owned.includes(pid) && owned.includes(300) && !owned.includes(100)
                     : !owned.includes(pid);
    }''', arg=dict(sid=sid,pid=pid,present=present), timeout=30000,polling=500)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--with-agent',action='store_true')
    parser.add_argument('--sources',default='claude,grok,agy')
    parser.add_argument('--screenshots',type=Path)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='native-resources-') as tmp, sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root=Path(tmp)/source;root.mkdir()
                fixture=peer(source,root,args.binary)
                try:
                    with ExitStack() as cleanup:
                        if args.with_agent:
                            log=cleanup.enter_context(tempfile.TemporaryFile())
                            agent=subprocess.Popen([str(args.binary.resolve().with_name('resource-agent')),
                                '--uid',str(os.getuid()),'--node-id-file',str(root/'node-id'),
                                '--socket',str(root/'collector.sock'),'--state',str(root/'agent.json'),
                                '--proc-root',str(fixture.proc),'--events','off'],stdout=log,stderr=log)
                            cleanup.callback(lambda:(agent.terminate(),agent.wait(timeout=10)))
                        for restarted in (False,True):
                            with serve(source,fixture,args.binary,args.with_agent) as base:
                                page=browser.new_page(viewport={'width':1280,'height':960})
                                page.goto(base,wait_until='domcontentloaded')
                                page.wait_for_function('T.listLoaded')
                                page.wait_for_function('''async sid=>{await loadSessions(true);
                                    return S.sessions.some(s=>s.sid===sid || s.session_id===sid)}''',arg=fixture.sid)
                                uid=page.evaluate('sid=>S.sessions.find(s=>s.sid===sid || s.session_id===sid).uid',fixture.sid)
                                if not restarted:
                                    # External CLI ownership and its plain worker use
                                    # native argv/FD evidence, without SDK checkpoints.
                                    argv=[source,'--resume',fixture.sid]
                                    START[700]=10000
                                    fds=None
                                    if source=='agy':
                                        argv=[source,'--conversation',fixture.sid]
                                        fds={3:str(fixture.native/'conversations'/f'{fixture.sid}.db')}
                                    proc_pid(fixture.proc,700,source,argv,1,fds=fds)
                                    proc_pid(fixture.proc,300,'python',['python','cli-worker.py'],700)
                                    bindings(page,fixture.sid,700)
                                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                                    page.locator('#a-resources').click()
                                    expect(page.locator('.session-resources')).to_be_visible()
                                    shutil.rmtree(fixture.proc/'700');shutil.rmtree(fixture.proc/'300')
                                    bindings(page,fixture.sid,700,False)
                                    print(f'PASS {source} external CLI and worker',flush=True)
                                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                                    page.locator('#a-sdk').click()
                                    page.locator('#sdk-input').click()
                                    page.locator('#sdk-input').fill('Resource ownership check')
                                    with page.expect_response(lambda r:'/api/native-sdk/turn' in r.url and r.request.method=='POST') as sent:
                                        page.locator('#sdk-send').click()
                                    assert sent.value.ok,sent.value.text()
                                    hosts=wait_for(lambda:active_hosts(root))
                                    assert len(hosts)==1,hosts
                                    pid=hosts[0]; START[pid]=20000
                                    proc_pid(fixture.proc,100,'sdkhost',['sdkhost'],1)
                                    proc_pid(fixture.proc,pid,source,[source,'sdk-peer'],100)
                                    proc_pid(fixture.proc,300,'python',['python','worker.py'],pid)
                                    for ident in (pid,300):
                                        stat=fixture.proc/str(ident)/'stat'
                                        stat.write_text(stat.read_text().rstrip()+' 0 0\n')
                                        (stat.parent/'smaps_rollup').write_text('Pss: 1024 kB\n')
                                # No SDK open on recovery: only ordinary resource reads.
                                bindings(page,fixture.sid,pid)
                                assert active_hosts(root)==[pid]
                                page.locator(f'#side .item[data-uid="{uid}"]').click()
                                page.locator('#a-resources').click()
                                page.get_by_role('button',name='仅当前会话',exact=True).click()
                                if args.with_agent:
                                    page.wait_for_function('''async uid=>{
                                      const r=await(await fetch('/api/session/resources?uid='+encodeURIComponent(uid)+'&scope=direct')).json();
                                      return r.totals.metrics.memory_pss_bytes.value===2097152;
                                    }''',arg=uid,timeout=30000,polling=500)
                                    page.get_by_role('button',name='刷新资源').click()
                                    expect(page.locator('.sr-node .sr-status')).to_have_text('在线')
                                    expect(page.locator('.sr-totals')).to_contain_text('2 MiB')
                                if args.screenshots:
                                    args.screenshots.mkdir(parents=True,exist_ok=True)
                                    page.screenshot(path=str(args.screenshots/f'{source}-{args.with_agent}-{restarted}.png'),full_page=True)
                                if restarted:
                                    # Exit the private host without opening or stopping
                                    # through a bridge; resource readers see its exit.
                                    stop_private_sdkhosts(root)
                                    stat=fixture.proc/str(pid)/'stat'
                                    stat.write_text(stat.read_text().replace(' 20000 ',' 30000 '))
                                    bindings(page,fixture.sid,pid,False)
                                page.close()
                                print(f'PASS {source} resources collector={args.with_agent}, restarted={restarted}',flush=True)
                finally:
                    stop_private_sdkhosts(root)
        finally:
            browser.close()


if __name__=='__main__':
    main()
