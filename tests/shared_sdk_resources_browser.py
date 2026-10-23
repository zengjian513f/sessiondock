#!/usr/bin/env python3
"""Codex/OpenCode shared services: exact workers, CLI identity and no double charge."""
import argparse
from contextlib import ExitStack
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server
from opencode_sdk_fixtures import FakeOpenCode, SID
from spawned_by_fixtures import START, proc_pid


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--with-agent',action='store_true')
    parser.add_argument('--screenshots',type=Path)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='shared-sdk-resources-') as tmp, ExitStack() as cleanup:
        root=Path(tmp); corpus=Corpus(root)
        (root/'state').mkdir()
        for sid in ('launcher','thread-a','thread-b'):
            corpus.put(sid,'codex',[codex_row('session_meta',dict(id=sid,cwd='/synthetic/shared')),
                codex_message('user','Shared resource '+sid)],[])
        native=FakeOpenCode(root)
        second='ses_shared_resource_second';native.create(second,str(native.cwd))
        proc=root/'proc';(proc/'sys/kernel/random').mkdir(parents=True)
        (proc/'sys/kernel/random/boot_id').write_text('private-shared-boot')
        (proc/'stat').write_text('btime 1700000000\n');(root/'node-id').write_text('a'*32)
        START.update({301:30100,900:90000})
        proc_pid(proc,900,'codex',['codex'],1,fds={3:str(corpus.paths['launcher'])})
        proc_pid(proc,100,'codex',['codex','app-server'],900,env=[('CODEX_SESSION_ID','launcher')],
                 fds={3:str(corpus.paths['thread-a']),4:str(corpus.paths['thread-b'])})
        proc_pid(proc,200,'python',['python','a.py'],100,env=[('CODEX_THREAD_ID','thread-a'),('CODEX_SESSION_ID','launcher')])
        proc_pid(proc,300,'code-host',['codex-code-mode-host'],100,env=[('CODEX_THREAD_ID','thread-b')])
        proc_pid(proc,301,'python',['python','b.py'],300)
        proc_pid(proc,400,'python',['python','unknown.py'],100,env=[('CODEX_SESSION_ID','launcher')])
        proc_pid(proc,500,'opencode',['opencode','serve','--service'],900,env=[('CODEX_SESSION_ID','launcher')])
        proc_pid(proc,600,'sh',['sh','tool.sh'],500,env=[('OPENCODE_SESSION_ID',SID),('CODEX_SESSION_ID','launcher')])
        proc_pid(proc,601,'python',['python','opencode.py'],600)
        proc_pid(proc,700,'opencode',['opencode','--session',second],1,env=[('OPENCODE_SESSION_ID',SID)])
        proc_pid(proc,701,'python',['python','cli.py'],700,env=[('OPENCODE_SESSION_ID',SID)])
        for entry in proc.iterdir():
            if entry.name.isdigit():
                stat=entry/'stat';stat.write_text(stat.read_text().rstrip()+' 0 0\n')
                (entry/'smaps_rollup').write_text('Pss: 1024 kB\n')
        if args.with_agent:
            log=cleanup.enter_context(tempfile.TemporaryFile())
            agent=subprocess.Popen([str(args.binary.resolve().with_name('resource-agent')),
                '--uid',str(os.getuid()),'--node-id-file',str(root/'node-id'),
                '--socket',str(root/'collector.sock'),'--state',str(root/'agent.json'),
                '--proc-root',str(proc),'--events','off'],stdout=log,stderr=log)
            cleanup.callback(lambda:(agent.terminate(),agent.wait(timeout=10)))
        env={'SESSIONDOCK_PROC_ROOT':str(proc),'SESSIONDOCK_NODE_ID_FILE':str(root/'node-id'),
             'SESSIONDOCK_RESOURCE_AGENT_MODE':'auto' if args.with_agent else 'off',
             'SESSIONDOCK_RESOURCE_AGENT_SOCKET':str(root/'collector.sock'),
             'SESSIONDOCK_OPENCODE_DB':str(native.db),'SESSIONDOCK_OPENCODE_ROOT':str(root/'mirror')}
        expected={'launcher':[900],'thread-a':[200],'thread-b':[300,301],SID:[600,601],second:[700,701]}
        with sync_playwright() as pw:
            browser=pw.chromium.launch(headless=True)
            try:
                for restarted in (False,True):
                    with isolated_server(corpus,args.binary,state_dir=root/'state',extra_env=env) as (base,_):
                        page=browser.new_page(viewport={'width':1280,'height':960})
                        page.goto(base,wait_until='domcontentloaded')
                        page.wait_for_function('S.sessions?.length===5')
                        page.wait_for_function('''async expected=>{
                          const r=await(await fetch('/api/process-links')).json();
                          const got={}; for(const b of r.bindings)(got[b.session.sid]??=[]).push(b.process.pid);
                          return Object.keys(got).length===Object.keys(expected).length && Object.entries(expected).every(([sid,pids])=>
                            JSON.stringify((got[sid]||[]).sort((a,b)=>a-b))===JSON.stringify(pids));
                        }''',arg=expected,timeout=30000,polling=500)
                        for sid,pids in expected.items():
                            uid=page.evaluate('sid=>S.sessions.find(s=>s.sid===sid).uid',sid)
                            page.locator(f'#side .item[data-uid="{uid}"]').click()
                            page.locator('#a-resources').click()
                            page.get_by_role('button',name='仅当前会话',exact=True).click()
                            if args.with_agent:
                                page.wait_for_function('''async({uid,n})=>{
                                  const r=await(await fetch('/api/session/resources?uid='+encodeURIComponent(uid)+'&scope=direct')).json();
                                  return r.totals.metrics.process_count.value===n && r.totals.metrics.memory_pss_bytes.value===n*1048576;
                                }''',arg=dict(uid=uid,n=len(pids)),timeout=30000,polling=500)
                                page.get_by_role('button',name='刷新资源').click()
                                expect(page.locator('.sr-totals')).to_contain_text(f'{len(pids)} MiB')
                            if args.screenshots and sid in ('thread-a',SID):
                                args.screenshots.mkdir(parents=True,exist_ok=True)
                                page.screenshot(path=str(args.screenshots/f'shared-{sid}-{args.with_agent}-{restarted}.png'),full_page=True)
                        if restarted:
                            # No evidence on a reused worker: never inherit the old
                            # session or the daemon launcher's identity.
                            (proc/'200/environ').write_bytes(b'CODEX_SESSION_ID=launcher\0')
                            stat=proc/'200/stat';stat.write_text(stat.read_text().replace(' 20000 ',' 99999 '))
                            page.wait_for_function('''async()=>!(await(await fetch('/api/process-links')).json()).bindings.some(b=>b.process.pid===200)''',timeout=30000,polling=500)
                        page.close()
                        print(f'PASS shared Codex/OpenCode exact workers, CLI override, no daemon/launcher charge; collector={args.with_agent} restart={restarted}',flush=True)
            finally:
                browser.close()


if __name__=='__main__':
    main()
