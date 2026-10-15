#!/usr/bin/env python3
"""Browser-driven multi-host maintenance, scratch recovery and independent lifetime."""
# run_validation: serial
import argparse
import json
import os
from pathlib import Path
import statistics
import tempfile
import threading
import time
from playwright.sync_api import sync_playwright
from runtime_layers_browser import rows, until
from history_fixtures import BINARY, Corpus, isolated_server, get_json, build_corpus
from hub_send_browser import prepare
from private_hosts import private_hosts, running_identity
from send_browser import create_claude, xterm_includes, wait_history
from browser_evidence_fixtures import run_with_evidence


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--output',type=Path,default=Path('target/runtime-final-review'))
    parser.add_argument('--scratch-only',action='store_true')
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-runtime-final-') as temporary, sync_playwright() as pw:
        root=Path(temporary)
        if args.scratch_only:
            corpus=build_corpus(root);logs=root/'runtime'
            with isolated_server(corpus,args.binary,extra_env={'SESSIONDOCK_RUNTIME_LOG_DIR':str(logs)}) as (base,opener):
                browser=pw.chromium.launch(headless=True);page=browser.new_page();page.goto(base)
                page.locator(f'#side .item[data-uid="{corpus.uid("codex-fork")}"]').click()
                until(page,lambda:any(r.get('event')=='server.listening' for r in rows(logs)))
                pid=next(r['pid'] for r in rows(logs) if r.get('event')=='server.listening')
                poison=logs/f'.runtime-clean-{pid}';poison.write_text('preserved recent leftover')
                expired=logs/'runtime-999999-1.jsonl'
                expired.write_text(json.dumps({'timestamp_ms':int(time.time()*1000)-86410000})+'\n')
                until(page,lambda:not expired.exists(),seconds=75)
                assert poison.read_text()=='preserved recent leftover'
                page.locator(f'#side .item[data-uid="{corpus.uid("claude-branch")}"]').click()
                page.screenshot(path=str(args.output/'scratch.png'));browser.close()
            (args.output/'result.json').write_text(json.dumps({'scratch_recovery':True}));return
        config=prepare(root)
        logs=root/'host/runtime-logs';logs.mkdir(parents=True,exist_ok=True)
        browser=pw.chromium.launch(headless=True);identities={};uids=[];times=[];result={}
        def server():return isolated_server(Corpus(root),args.binary,state_dir=root/'state',host_dir=root/'host',lifecycle_dir=root/'ledger',launcher_config=config,extra_env={'SESSIONDOCK_LOG_REQUESTS':'all'})
        def interact(page,uid,word):
            tick=time.perf_counter();page.locator(f'#side .item[data-uid="{uid}"]').click()
            if not page.locator('#termpane').is_visible():page.locator('#a-term').click()
            page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
            page.locator('#termpane .xterm-helper-textarea:visible').press_sequentially(word)
            page.locator('#termpane .xterm-helper-textarea:visible').press('Enter');xterm_includes(page,'> '+word)
            times.append((time.perf_counter()-tick)*1000)
        try:
            with private_hosts(root):
                with server() as (base,opener):
                    page=browser.new_page(viewport={'width':1280,'height':900});page.goto(base,wait_until='networkidle')
                    for index in range(3):
                        if index:
                            page.close();page=browser.new_page(viewport={'width':1280,'height':900});page.goto(base,wait_until='networkidle')
                        receipt=create_claude(page,base,root/'work')
                        page.locator('#a-chat').click();page.locator('#cinput').fill(f'private host seed {index}');page.locator('#csend').click()
                        wait_history(page,f'private host seed {index}');uids.append(page.evaluate('S.sel'))
                        record=json.loads((root/'host'/f'{receipt["name"]}.json').read_text());pid=record['host_pid'];identities[pid]=running_identity(pid)
                    print('created three isolated browser hosts',flush=True)
                    stamp=int(time.time()*1000)
                    old=(json.dumps({'timestamp_ms':stamp-86410000,'event':'fixture.expired'})+'\n').encode()
                    recent=(json.dumps({'timestamp_ms':stamp,'event':'fixture.recent'})+'\n').encode()
                    # 24h-shaped private retained volume per active host; every owner must
                    # parse its own shard, not N copies of the complete shared directory.
                    segments=[]
                    for pid in identities:
                        path=logs/f'runtime-{pid}-1.jsonl';path.write_bytes((old+recent)*50000);segments.append(path)
                        (logs/f'.runtime-clean-{pid}').write_text('recent crash leftover')
                    protected=['session.jsonl','usage.json','bugreport.json','runtime-999999-1.jsonl.gz','.runtime-clean-evidence']
                    for name in protected:(logs/name).write_bytes(b'protected')
                    input_bytes=sum(p.stat().st_size for p in segments)
                    # The node owns this dead-process shard. Interpose a real rename
                    # failure after scratch creation; no runtime test hooks/mocks.
                    nodelogs=root/'state/runtime-logs'
                    nodepid=next(r['pid'] for r in rows(nodelogs) if r.get('event')=='server.listening')
                    poison=nodelogs/f'.runtime-clean-{nodepid}';poison.write_text('recent crash leftover')
                    fault=nodelogs/'runtime-999999-1.jsonl';fault.write_bytes((old+recent)*150000)
                    triggered=threading.Event();stop=threading.Event()
                    def deny_replace():
                        while not stop.wait(.002):
                            if list(nodelogs.glob(f'.runtime-clean-{nodepid}-*')):
                                nodelogs.chmod(0o500);triggered.set();return
                    watcher=threading.Thread(target=deny_replace,daemon=True);watcher.start()
                    try:
                        until(page,lambda:triggered.is_set(),seconds=80)
                        until(page,lambda:get_json(opener,base,'/api/health')['runtime_log']['cleanup_errors']>0,seconds=30)
                        # Removal was denied too; retain that failure and recover with
                        # a fresh unique scratch instead of poisoning the next sweep.
                        print('atomic replacement failure observed; recovering',flush=True)
                        result['fault_health']=get_json(opener,base,'/api/health')['runtime_log']
                        assert fault.read_bytes()==(old+recent)*150000
                    finally:
                        stop.set();watcher.join(timeout=2);nodelogs.chmod(0o700)
                    for uid in uids:interact(page,uid,'during-maintenance')
                    until(page,lambda:all(p.stat().st_size==len(recent)*50000 for p in segments),seconds=100)
                    until(page,lambda:fault.stat().st_size==len(recent)*150000,seconds=100)
                    for p in segments:assert p.read_bytes()==recent*50000
                    assert fault.read_bytes()==recent*150000
                    assert poison.read_text()=='recent crash leftover'
                    for name in protected:assert (logs/name).read_bytes()==b'protected'
                    for pid,identity in identities.items():assert running_identity(pid)==identity
                    result.update(hosts=3,input_bytes=input_bytes,retained_bytes=sum(p.stat().st_size for p in segments),node_health=get_json(opener,base,'/api/health')['runtime_log'])
                    page.screenshot(path=str(args.output/'multi-host.png'));page.close()
                for pid,identity in identities.items():assert running_identity(pid)==identity
                with server() as (base,opener):
                    page=browser.new_page();page.goto(base,wait_until='networkidle')
                    for uid in uids:interact(page,uid,'after-node-restart')
                    page.screenshot(path=str(args.output/'reconnected.png'));page.close()
                result['survived_restart']=True
            deliveries=[r for r in rows(logs) if r.get('event')=='log.delivery']
            per_host={str(pid):max((r.get('cleanup_scanned_bytes',0) for r in deliveries if r.get('pid')==pid),default=0) for pid in identities}
            assert all(input_bytes//3 <= count < input_bytes//3+100000 for count in per_host.values()),per_host
            result['host_scanned_bytes']=per_host
            # Node restart must still maintain orphan logs from terminated hosts.
            orphan=logs/f'runtime-{next(iter(identities))}-2.jsonl';orphan.write_bytes(old+recent)
            with server() as (base,opener):
                page=browser.new_page();page.goto(base,wait_until='networkidle')
                page.locator(f'#side .item[data-uid="{uids[0]}"]').click()
                until(page,lambda:orphan.read_bytes()==recent)
                result['orphan_node_health']=get_json(opener,base,'/api/health')['runtime_log'];page.close()
            result.update(status='PASS',p50_ms=statistics.median(times),max_ms=max(times),samples=len(times),orphan_cleaned=True)
            (args.output/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result),flush=True)
        except BaseException:
            for context in browser.contexts:
                for index,page in enumerate(context.pages):
                    try:page.screenshot(path=str(args.output/f'failure-{index}.png'))
                    except Exception:pass
            raise
        finally:browser.close()

if __name__=='__main__':run_with_evidence(main,'target/runtime-final-review')
