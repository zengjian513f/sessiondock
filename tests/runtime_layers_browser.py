#!/usr/bin/env python3
"""Browser-created private CLI survives service restart; owned runtime retention and faults.

No real CLI/provider, production sessions or API-only acceptance. File mutations
are fault/age fixture setup; assertions follow real product navigation and input.
"""
# run_validation: serial
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import statistics
import socket
import threading
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, isolated_server, get_json, build_corpus
from hub_send_browser import prepare
from private_hosts import private_hosts, running_identity
from send_browser import create_claude, wait_history, xterm_includes
from browser_evidence_fixtures import run_with_evidence


def until(page, check, seconds=12):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        value=check()
        if value:return value
        page.wait_for_timeout(100)
    raise AssertionError('runtime layer condition timed out')


def rows(directory):
    result=[]
    for path in directory.glob('runtime-*.jsonl'):
        for line in path.read_bytes().splitlines():
            try:result.append(json.loads(line))
            except (ValueError,UnicodeError):pass
    return result


def seed(directory):
    directory.mkdir(parents=True,exist_ok=True)
    now=int(time.time()*1000);old=now-86400000-10000
    records=lambda values:b''.join(json.dumps({'timestamp_ms':t,'event':e}).encode()+b'\n' for t,e in values)
    (directory/'runtime-900001-1.jsonl').write_bytes(records([(old,'old'),(now,'recent')])+b'\xffpartial')
    (directory/'runtime-900002-1.jsonl').write_bytes(b'\xffexpired partial')
    (directory/'runtime-900003-1.jsonl').write_bytes(records([(old,'expired')]))
    (directory/'runtime-900006-1.jsonl').write_bytes(b'\xff'*65536+records([(old,'oldfragment')]))
    (directory/'runtime-900004-1.jsonl').write_bytes(records([(now-86400000+25000,'boundary')]))
    for name in ['runtime-900002-1.jsonl','.runtime-clean-900009']:
        p=directory/name
        if not p.exists():p.write_bytes(b'old temporary')
        os.utime(p,((old/1000),)*2)
    for name in ['session.jsonl','usage.json','bugreport.json','runtime-900005-1.jsonl.gz']:
        (directory/name).write_bytes(b'protected fixture')
        os.utime(directory/name,((old/1000),)*2)


def retained(directory, boundary=False):
    data=(directory/'runtime-900001-1.jsonl').read_bytes()
    assert b'"old"' not in data and b'"recent"' in data and b'\xffpartial' in data
    assert not (directory/'runtime-900002-1.jsonl').exists()
    assert not (directory/'runtime-900003-1.jsonl').exists()
    long_partial=(directory/'runtime-900006-1.jsonl').read_bytes()
    assert long_partial.startswith(b'\xff'*65536) and b'oldfragment' in long_partial
    assert not (directory/'.runtime-clean-900009').exists()
    assert (directory/'runtime-900004-1.jsonl').exists() != boundary
    for name in ['session.jsonl','usage.json','bugreport.json','runtime-900005-1.jsonl.gz']:
        assert (directory/name).read_bytes()==b'protected fixture'


@contextmanager
def collector(binary, root):
    proc=root/'proc';(proc/'sys/kernel/random').mkdir(parents=True,exist_ok=True)
    (proc/'sys/kernel/random/boot_id').write_text('synthetic-runtime-boot')
    (proc/'stat').write_text('btime 1700000000\n')
    (root/'node-id').write_text('a'*32)
    fake=root/'fake-collector'
    fake.write_text('#!'+sys.executable+'\nimport sys,time\nprint("READY",flush=True)\nfor i in range(1000): print("PRIVATE_COLLECTOR_TEXT_"+str(i),file=sys.stderr,flush=True)\ntime.sleep(2)\n')
    fake.chmod(0o700)
    environment=dict(os.environ,RESOURCE_AGENT_BPFTRACE=str(fake))
    with (root/'collector-stderr').open('wb') as stderr:
        child=subprocess.Popen([str(binary.resolve().with_name('resource-agent')),'--node-id-file',str(root/'node-id'),
            '--uid',str(os.getuid()),'--state',str(root/'agent-state.json'),'--socket',str(root/'agent.sock'),
            '--proc-root',str(proc),'--events','auto'],stdout=subprocess.DEVNULL,stderr=stderr,env=environment)
        try:yield child
        finally:
            child.terminate();child.wait(timeout=10)


@contextmanager
def collector_link(root):
    listener=socket.socket(socket.AF_UNIX);listener.bind(str(root/'agent-link.sock'));listener.listen();listener.settimeout(.1)
    stop=threading.Event();delay=threading.Event();workers=[]
    def serve(client):
        try:
            with client, socket.socket(socket.AF_UNIX) as upstream:
                client.settimeout(3);upstream.settimeout(3);upstream.connect(str(root/'agent.sock'))
                if delay.is_set():stop.wait(2.5);return
                request=client.makefile('rb').readline();upstream.sendall(request)
                response=upstream.makefile('rb').readline();client.sendall(response)
        except OSError:pass
    def accept():
        while not stop.is_set():
            try:client,_=listener.accept()
            except socket.timeout:continue
            except OSError:return
            thread=threading.Thread(target=serve,args=(client,),daemon=True);thread.start();workers.append(thread)
    thread=threading.Thread(target=accept,daemon=True);thread.start()
    try:yield delay
    finally:
        stop.set();listener.close();thread.join(timeout=1)
        for worker in workers:worker.join(timeout=3)


def layers(browser,args):
    with tempfile.TemporaryDirectory(prefix='sd-runtime-layers-') as tmp:
        root=Path(tmp);config=prepare(root)
        hostlog=root/'host/runtime-logs';nodelog=root/'state/runtime-logs';agentlog=root/'agent-state.runtime-logs'
        for directory in (hostlog,nodelog,agentlog):seed(directory)
        evidence={}; timings=[]
        environment={'SESSIONDOCK_NODE_ID_FILE':str(root/'node-id'),'SESSIONDOCK_RESOURCE_AGENT_SOCKET':str(root/'agent-link.sock'),'SESSIONDOCK_PROC_ROOT':str(root/'proc')}
        def server():return isolated_server(Corpus(root),args.binary,state_dir=root/'state',host_dir=root/'host',
            lifecycle_dir=root/'ledger',launcher_config=config,extra_env=environment)
        with private_hosts(root),collector(args.binary,root) as agent,collector_link(root) as delayed:
            with server() as (base,opener):
                page=browser.new_page(viewport={'width':1280,'height':900});page.goto(base,wait_until='networkidle')
                until(page,lambda:(root/'agent.sock').exists())
                receipt=create_claude(page,base,root/'work')
                page.locator('#a-chat').click();page.locator('#cinput').fill('runtime synthetic seed');page.locator('#csend').click()
                wait_history(page,'runtime synthetic seed');uid=page.evaluate('S.sel')
                record=json.loads((root/'host'/f'{receipt["name"]}.json').read_text());hostpid=record['host_pid'];identity=running_identity(hostpid)
                until(page,lambda:any(r.get('event')=='pty.started' for r in rows(hostlog)))
                started=next(r for r in rows(hostlog) if r.get('event')=='pty.started')
                assert started['identity']['name']==receipt['name'] and started['identity']['instance_id']
                # Replace only the owned diagnostic directory. Closing/reopening the real
                # console generates host lifecycle logs while its sink is unavailable.
                host_saved=root/'host-log-saved';hostlog.rename(host_saved);hostlog.write_text('injected sink failure')
                page.reload(wait_until='networkidle')
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                if not page.locator('#termpane').is_visible():page.locator('#a-term').click()
                page.wait_for_function('T.ws?.readyState === WebSocket.OPEN || !!document.querySelector("dialog.app-popup[open]")')
                popup=page.locator('dialog.app-popup[open]')
                if popup.is_visible():
                    expect(popup).to_contain_text('终端')
                    popup.get_by_role('button',name='取消',exact=True).click()
                    assert running_identity(hostpid)==identity
                    page.locator('#a-term').click()
                    page.locator('dialog.app-popup[open]').get_by_role('button',name='确定',exact=True).click()
                page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
                page.locator('#termpane .xterm-helper-textarea').press_sequentially('sink-fault-input')
                page.locator('#termpane .xterm-helper-textarea').press('Enter');xterm_includes(page,'> sink-fault-input')
                assert running_identity(hostpid)==identity
                page.wait_for_timeout(6500)  # Keep the fault through a real aggregation flush.
                hostlog.unlink();host_saved.rename(hostlog)
                page.locator('#a-resources').click() if page.locator('#a-resources').count() else page.get_by_role('button',name='资源',exact=True).click()
                expect(page.locator('.session-resources')).to_be_visible()
                expect(page.locator('.sr-node')).to_contain_text('在线')
                expect(page.locator('.sr-node')).to_contain_text('不支持探测')
                for directory in (hostlog,nodelog,agentlog):retained(directory)
                # Delay the real Unix-socket exchange, not the browser response.
                delayed.set()
                page.get_by_role('button',name='刷新资源',exact=True).click()
                until(page,lambda:any(r.get('event')=='resource.request' and r.get('duration_ms',0)>=2000 for r in rows(agentlog)),seconds=15)
                # Resource refresh can serve a cached snapshot: do not invent a slow HTTP receipt.
                expect(page.locator('.session-resources')).to_be_visible()
                delayed.clear();page.get_by_role('button',name='刷新资源',exact=True).click()
                expect(page.locator('.sr-node')).to_contain_text('在线',timeout=15000)
                # The real sampler cannot replace a directory as its state file.
                state=root/'agent-state.json'
                if state.exists():state.unlink()
                state.mkdir()
                agent_saved=root/'agent-log-saved';agentlog.rename(agent_saved);agentlog.write_text('injected sink failure')
                for i in range(12):
                    tick=time.perf_counter()
                    with page.expect_response(lambda r:urlsplit(r.url).path=='/api/session/resources') as response:
                        page.get_by_role('button',name='刷新资源',exact=True).click()
                    assert response.value.status==200
                    result=response.value.json()
                    assert any(node['node_id']=='a'*32 for node in result['nodes'])
                    expect(page.locator('.session-resources')).to_be_visible();timings.append((time.perf_counter()-tick)*1000)
                    page.wait_for_timeout(500)
                    if i==6:
                        agentlog.unlink();agent_saved.rename(agentlog)
                until(page,lambda:any(r.get('event')=='resource.attribution_save_failed' for r in rows(agentlog)))
                # Rotation then sink failure: each subsequent write must reopen, and failures must not block the page.
                active=next(p for p in nodelog.glob('runtime-*.jsonl') if '90000' not in p.name)
                active.rename(nodelog/'external-rotation.jsonl')
                page.screenshot(path=str(args.output/'resources.png'))
                # Exact malformed cleanup failure: another pathname blocks the cleaner's private temporary.
                # Node pid from the original startup segment after rotation.
                node_pid=json.loads((nodelog/'external-rotation.jsonl').read_text().splitlines()[0])['pid']
                blocker=nodelog/f'.runtime-clean-{node_pid}';blocker.mkdir()
                # Wait across actual low-frequency idle cleanup, while using the resource page normally.
                for i in range(7):
                    page.wait_for_timeout(9000)
                    page.get_by_role('button',name='刷新资源',exact=True).click()
                    print('runtime idle retention progress',i+1,'/7',flush=True)
                assert not (hostlog/'runtime-900004-1.jsonl').exists()
                assert not (agentlog/'runtime-900004-1.jsonl').exists()
                assert (nodelog/'runtime-900004-1.jsonl').exists()
                health=get_json(opener,base,'/api/health')['runtime_log'];assert health['cleanup_errors']>0,health
                blocker.rmdir();state.rmdir()
                for directory in (hostlog,agentlog):
                    summaries=[r for r in rows(directory) if r.get('event')=='log.delivery']
                    assert summaries and summaries[-1]['write_errors']>0,(directory,summaries)
                assert any(r.get('event')=='log.delivery' for r in rows(nodelog)), 'rotation stranded the writer' 
                evidence['before_restart']={'host_pid':hostpid,'runtime_health':health,'resource_errors':rows(agentlog)}
                page.close()
            assert running_identity(hostpid)==identity
            with server() as (base,opener):
                page=browser.new_page();page.goto(base,wait_until='networkidle');page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.locator('#a-term').click();page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
                page.locator('#termpane .xterm-helper-textarea').press_sequentially('after-runtime-restart')
                page.locator('#termpane .xterm-helper-textarea').press('Enter');xterm_includes(page,'> after-runtime-restart')
                assert running_identity(hostpid)==identity
                until(page,lambda:not (nodelog/'runtime-900004-1.jsonl').exists())
                for directory in (hostlog,nodelog,agentlog):retained(directory,boundary=True)
                page.screenshot(path=str(args.output/'reconnected.png'));page.close()
            evidence['host_survived_restart']=True
        # Private host cleanup gracefully exits it; startup/exit log and final accounting survive.
        allrows=rows(hostlog)+rows(nodelog)+rows(agentlog)
        assert any(r.get('event')=='pty.stopped' for r in allrows)
        assert any(r.get('event')=='resource.stopped' for r in allrows)
        repeats=[r for r in allrows if r.get('event')=='resource.attribution_save_failed' and r.get('repeat_count')]
        assert repeats,repeats
        assert 'PRIVATE_COLLECTOR_TEXT_' not in json.dumps(allrows)
        assert any(r.get('event')=='resource.collector_failed' for r in allrows)
        assert any(r.get('event')=='resource.collector_stderr' and r.get('lost',0)>0 for r in allrows)
        assert 'runtime synthetic seed' not in json.dumps(allrows) and 'after-runtime-restart' not in json.dumps(allrows) and 'sink-fault-input' not in json.dumps(allrows)
        generated=[r for r in allrows if 'pid' in r]
        (args.output/'process-events.json').write_text(json.dumps(generated,indent=2))
        evidence.update(p50_ms=statistics.median(timings),p95_ms=sorted(timings)[-1],events=len(generated),
            bytes=sum(p.stat().st_size for d in (hostlog,nodelog,agentlog) for p in d.glob('runtime-*.jsonl') if not p.name.startswith('runtime-9000')),
            repeated_failures=sum(r['repeat_count'] for r in repeats),retention_idle_restart=True)
        (args.output/'layers.json').write_text(json.dumps(evidence,indent=2))
        print('PASS runtime layers, independent host survives service restart, resource failures, idle/restart retention',flush=True)


def requests(browser,args):
    evidence={}
    for mode in ['off','default','all','failed','recovered']:
        with tempfile.TemporaryDirectory(prefix='sd-runtime-requests-') as tmp:
            root=Path(tmp);corpus=build_corpus(root);logs=root/'runtime';logs.mkdir()
            if mode in ['failed','recovered']:
                logs.rmdir();logs.write_text('injected sink fault')
            env={'SESSIONDOCK_RUNTIME_LOG_DIR':str(logs)}
            if mode!='default':env['SESSIONDOCK_LOG_REQUESTS']='off' if mode=='off' else 'all'
            times=[]
            with isolated_server(corpus,args.binary,extra_env=env) as (base,opener):
                page=browser.new_page();page.goto(base)
                for i in range(40):
                    sid='claude-branch' if i%2 else 'codex-fork';tick=time.perf_counter()
                    page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                    expect(page.locator('#msgs')).to_contain_text('Claude selected answer' if i%2 else 'Codex fork answer')
                    times.append((time.perf_counter()-tick)*1000)
                health=get_json(opener,base,'/api/health')['runtime_log']
                if mode in ['failed','recovered']:assert health['write_errors']>0,health
                if mode=='recovered':
                    logs.unlink();logs.mkdir()
                    for i in range(7):
                        page.wait_for_timeout(9000)
                        page.locator(f'#side .item[data-uid="{corpus.uid("codex-fork")}"]').click()
                        print('runtime sink recovery progress',i+1,'/7',flush=True)
                    delivered=[r for r in rows(logs) if r.get('event')=='log.delivery']
                    assert delivered and delivered[-1]['write_errors']>=health['write_errors']
                page.close();started=time.monotonic()
            assert time.monotonic()-started<4
            data=rows(logs) if logs.is_dir() else []
            if mode=='all':
                assert any(r.get('repeat_count',0)>1 and r.get('page') and r.get('trace') for r in data),data
                assert len(data)<40
            if mode=='default':assert not any(r.get('event')=='http.request' and r.get('status')==200 for r in data)
            evidence[mode]={'samples':len(times),'p50_ms':statistics.median(times),'p95_ms':sorted(times)[37],
                'events':len(data),'bytes':sum(p.stat().st_size for p in logs.glob('*.jsonl')) if logs.is_dir() else 0,'health':health,
                'shutdown_s':time.monotonic()-started}
            (args.output/f'{mode}.json').write_text(json.dumps(data,indent=2))
            print('PASS runtime requests',mode,evidence[mode],flush=True)
    (args.output/'requests.json').write_text(json.dumps(evidence,indent=2))


def volume(browser,args):
    with tempfile.TemporaryDirectory(prefix='sd-runtime-volume-') as tmp:
        root=Path(tmp);corpus=build_corpus(root);directory=root/'runtime';directory.mkdir()
        segment=directory/'runtime-900020-1.jsonl';stamp=int(time.time()*1000)
        old=json.dumps({'timestamp_ms':stamp-86410000,'event':'synthetic.background','level':'warn','count':100,'duration_ms':123}).encode()+b'\n'
        recent=json.dumps({'timestamp_ms':stamp,'event':'synthetic.recent','level':'info','count':100,'duration_ms':123}).encode()+b'\n'
        with segment.open('wb') as out:
            for i in range(100000):out.write(old);out.write(recent)
        input_bytes=segment.stat().st_size;began=time.monotonic();times=[]
        with isolated_server(corpus,args.binary,extra_env={'SESSIONDOCK_RUNTIME_LOG_DIR':str(directory)}) as (base,opener):
            page=browser.new_page();page.goto(base)
            for i in range(30):
                tick=time.perf_counter();sid='claude-branch' if i%2 else 'codex-fork'
                page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                expect(page.locator('#msgs')).to_contain_text('Claude selected answer' if i%2 else 'Codex fork answer')
                times.append((time.perf_counter()-tick)*1000)
            until(page,lambda:segment.stat().st_size==len(recent)*100000,seconds=30)
            assert segment.read_bytes()==recent*100000
            health=get_json(opener,base,'/api/health')['runtime_log'];assert health['cleanup_scanned_bytes']>=input_bytes
            result={'input_records':200000,'input_bytes':input_bytes,'retained_records':100000,
                'elapsed_with_navigation_s':time.monotonic()-began,'health':health,
                'p50_ms':statistics.median(times),'p95_ms':sorted(times)[28]}
            page.close()
        (args.output/'volume.json').write_text(json.dumps(result,indent=2));print('PASS runtime volume',result,flush=True)


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--output',type=Path,default=Path('target/runtime-layers'));parser.add_argument('--only',choices=['layers','requests','volume'])
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    with sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            if args.only in (None,'layers'):layers(browser,args)
            if args.only in (None,'requests'):requests(browser,args)
            if args.only in (None,'volume'):volume(browser,args)
        except BaseException:
            for context in browser.contexts:
                for index,page in enumerate(context.pages):
                    try:page.screenshot(path=str(args.output/f'failure-{index}.png'))
                    except Exception:pass
            raise
        finally:browser.close()

if __name__=='__main__':run_with_evidence(main,'target/runtime-layers')
