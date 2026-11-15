#!/usr/bin/env python3
"""Real collector + node/Hub: cycle money, denominator, incremental reads and UI."""
import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_fixtures import Corpus, isolated_server, BINARY, REPO
from hub_fixtures import Hub, free_port
from node_auth_fixtures import node_env, TOKEN


def iso(seconds):
    return datetime.fromtimestamp(seconds, timezone.utc).isoformat()


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding='utf-8')


def append(path, *records):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('a', encoding='utf-8') as stream:
        for row in records:
            stream.write(json.dumps(row)+'\n')


def codex(ident, when, model='gpt-6.1-sol'):
    return [{'type':'turn_context','timestamp':iso(when),'payload':{'model':model}},
            {'type':'token_usage_record','timestamp':iso(when),'payload':{'response_id':ident,
             'usage':{'input_tokens':1000000,'cached_input_tokens':600000,'output_tokens':1000000,'reasoning_output_tokens':900000,'cache_write_input_tokens':0}}}]


def quotas(root, now, used=25):
    for source in ('codex','claude','agy','droid','kiro'):
        windows=[{'id':source+':primary','group':'Gemini Models' if source=='agy' else source,
                  'window':300,'remaining_percent':100-used,'resets_at':iso(now+7200)}]
        if source=='codex':windows.append({'id':'codex:secondary','group':'codex','window':10080,'remaining_percent':50,'resets_at':iso(now+86400)})
        if source=='kiro':windows=[{'id':'monthly','group':'Pro','window':'monthly','remaining_percent':75,'used':25,'limit':100,'unit':'Credits','resets_at':iso(now+86400)}]
        dump(root/'quotas'/f'{source}.json',{'source':source,'captured_at':iso(now),'buckets':windows})


def quota_point(when, end, used):
    return {'type':'event_msg','timestamp':iso(when),'payload':{'type':'token_count','rate_limits':{
        'limit_id':'codex','primary':{'used_percent':used,'window_minutes':300,'resets_at':end}}}}


def batch(path, sid, when, model, totals):
    # Synthetic requests preserve the reference totals without crossing 272K.
    append(path,{'type':'session_meta','timestamp':iso(when),'payload':{'id':sid}},
           {'type':'turn_context','timestamp':iso(when),'payload':{'model':model}})
    count=(totals[0]+199999)//200000
    previous=[0,0,0]
    for i in range(count):
        cumulative=[n*(i+1)//count for n in totals]
        u=[n-p for n,p in zip(cumulative,previous)];previous=cumulative
        usage={'input_tokens':u[0],'cached_input_tokens':u[1],'output_tokens':u[2]}
        # The first local response precedes the 5% observation, but its matched
        # token_count arrives afterwards; the checkpoint must use that time.
        early=sid=='local' and model=='gpt-6-astra' and i==0
        append(path,{'type':'token_usage_record','timestamp':iso(when-41 if early else when+i/count),'payload':{
            'thread_id':sid,'response_id':f'{sid}-{model}-{i}','usage':usage}})
        if early:
            append(path,{'type':'event_msg','timestamp':iso(when),'payload':{'type':'token_count',
                'info':{'last_token_usage':usage}}})



def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=Path,default=BINARY);parser.add_argument('--output',type=Path,default=REPO/'target/agent-usage/screenshots');parser.add_argument('--baseline',action='store_true');args=parser.parse_args()
    args.output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-usage-') as tmp, ExitStack() as stack:
        root=Path(tmp);state=root/'state';state.mkdir();now=time.time();roots={}
        for source in ('codex','claude','grok','agy','kiro','droid','codebuddy'):
            roots[source]=str(root/source);(root/source).mkdir()
        cp=root/'codex/2026/11/15/rollout-usage.jsonl'
        trash=root/'trash';retired=trash/'move-out-codex/files/0'
        batch(retired,'retired',now-130,'gpt-6-astra',(118315973,116853120,246837))
        append(retired,quota_point(now-140,now+7200,0),quota_point(now-120,now+7200,5))
        dump(trash/'move-out-codex/manifest.json',{'files':[{'name':'0','origin':str(root/'removed.jsonl'),'in_trash':True}]})
        dump(state/'accounting-transfers/out/operation.json',{'id':'out','moving':True,'phase':'retired','created_ms':int((now-110)*1000),
            'plan':{'group':{'members':[{'source':'codex','sid':'retired'}]}}})
        batch(cp,'local',now-80,'gpt-6-astra',(369767995-118315973,363940352-116853120,885982-246837))
        batch(cp,'local',now-70,'gpt-6.1-sol',(60128014,58930944,116547))
        append(cp,quota_point(now-60,now+7200,15),quota_point(now-50,now+7200,14))
        # Imported calls belong to another machine before the incoming move.
        foreign=root/'codex/foreign.jsonl'
        batch(foreign,'imported',now-100,'gpt-6-astra',(100000,0,100000))
        dump(state/'accounting-transfers/in/operation.json',{'id':'in','moving':True,'phase':'complete','created_ms':int((now-20)*1000),
            'plan':{'group':{'members':[{'source':'codex','sid':'imported'}]}}})
        # Native compaction metering is outside the reference token_count basis.
        append(cp,*codex('compaction',now-40),{'type':'compacted','timestamp':iso(now-39),'payload':{'compaction_response_id':'compaction'}})
        append(root/'claude/project/session.jsonl',{'type':'assistant','sessionId':'claude-usage','timestamp':iso(now-60),
            'message':{'id':'claude-r1','model':'claude-sonnet-5-5','usage':{'input_tokens':1000000,'output_tokens':100000,'cache_read_input_tokens':1000000,
            'cache_creation_input_tokens':1000000,'cache_creation':{'ephemeral_5m_input_tokens':500000,'ephemeral_1h_input_tokens':500000}},'content':[{'type':'text','text':'PRIVATE-BODY-MUST-NOT-ENTER-LEDGER'}]}})
        append(root/'agy/brain/one/.system_generated/logs/transcript_full.jsonl',{'type':'PLANNER_RESPONSE','status':'DONE','step_index':1,'created_at':iso(now-60),'model':'gemini-4-carbon-high','input_tokens':1000000,'cache_read_tokens':2000000,'output_tokens':100000})
        dump(root/'grok/project/sid/usage.json',{'sessionId':'sid','turns':[{'turnNumber':1,'endedAt':iso(now-60),'modelUsage':{'grok-4.6':{'inputTokens':100000,'cachedReadTokens':50000,'outputTokens':10000,'costUsdTicks':1850000000}}}]})
        dump(root/'droid/project/sid.settings.json',{'model':'claude-sonnet-5-5','tokenUsage':{'inputTokens':1000000,'outputTokens':100000}})
        row={'type':'message','timestamp':iso(now-60),'providerData':{'messageId':'cb-r1','stepSeq':1,'model':'gpt-6.1-sol','rawUsage':{'prompt_tokens':1000000,'completion_tokens':1000000,'prompt_tokens_details':{'cached_tokens':600000}}}}
        append(root/'codebuddy/project/sid.jsonl',row,{**row,'type':'function_call'},
               {**row,'providerData':{**row['providerData'],'stepSeq':2}})
        db=root/'opencode.db';roots['opencode']=str(db);con=sqlite3.connect(db)
        con.execute('create table session_message(id text,session_id text,type text,time_created int,time_updated int,data text)')
        con.execute('insert into session_message values(?,?,?,?,?,?)',('oc1','sid','assistant',int((now-60)*1000),1,json.dumps({'model':{'id':'claude-sonnet-5-5'},'time':{'completed':int(now*1000)},'tokens':{'input':1000000,'output':100000,'cache':{'read':0,'write':0}}})));con.commit();con.close()
        quotas(state,now,used=15)
        directory=state/'agent-usage';directory.mkdir()
        dump(directory/'settings.json',{'enabled':False,'interval_seconds':1,'sources':{}})
        dump(directory/'config.json',{'directory':str(directory),'roots':roots,'quota_dir':str(state/'quotas'),'prices':str(REPO/'tools/agent_usage_prices.json'),'transfer_dir':str(state/'accounting-transfers'),'trash_dir':str(trash)})
        command=[sys.executable,str(REPO/'tools/agent_usage.py'),'collect','--config',str(directory/'config.json')]
        subprocess.run(command,check=True,capture_output=True)
        first=json.loads((directory/'snapshot.json').read_text());assert first['metrics']['bytes_read']>0
        subprocess.run(command,check=True,capture_output=True)
        second=json.loads((directory/'snapshot.json').read_text());assert second['metrics']['bytes_read']==0,second['metrics']
        assert 'PRIVATE-BODY' not in (directory/'ledger-v2.sqlite').read_bytes().decode(errors='ignore')
        with sync_playwright() as pw, ExitStack() as browser_stack:
            options={'headless':True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):options['executable_path']=os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser=pw.chromium.launch(**options);browser_stack.callback(browser.close)
            port=free_port();env=node_env(root,port,'127.0.0.0/8');env['SESSIONDOCK_USAGE_PYTHON']=sys.executable
            base,_=stack.enter_context(isolated_server(Corpus(root),args.binary.resolve(),state_dir=state,trash_dir=trash,extra_env=env))
            (state/'accounting-transfers').rename(state/'transfers')
            hr=root/'hub';hr.mkdir();hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hr,[SimpleNamespace(port=port,token=TOKEN,name='Usage Node')]);hub.start();stack.callback(hub.stop)
            context=browser.new_context(viewport={'width':1280,'height':950});page=context.new_page();errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
            context.route('**/api/clients',lambda route:route.fulfill(json={'clients':[{'source':s,'installed':True} for s in roots]}))
            if args.baseline:
                page.goto(base+'/quotas.html');expect(page.locator('#quota-agents .quota-agent')).not_to_have_count(0)
                page.screenshot(path=str(args.output/'before-wide-light.png'),full_page=True);return
            page.goto(base+'/quotas.html')
            current=page.locator('[data-source="codex"] tbody .quota-api').first
            expect(page.locator('#quota-agents table')).to_have_count(5)
            expect(page.locator('#usage-cycles')).to_have_count(0)
            expect(current).to_contain_text('$475.97');expect(current).to_contain_text('$3,282.54')
            assert '14.5%' in current.get_attribute('title')
            assert '首次显示 15%' in current.get_attribute('title')
            expect(page.locator('[data-source="claude"] tbody .quota-api')).to_contain_text('$6.35')
            expect(page.locator('[data-source="agy"] tbody .quota-api')).to_contain_text('$3.20')
            expect(page.locator('[data-source="kiro"] tbody .quota-api')).to_have_text('N/A')
            report=page.evaluate("fetch('api/usage').then(r=>r.json())")
            values={r['source']:r for r in report['rows']}
            assert abs(values['codebuddy']['api_usd']-33.44)<1e-8
            assert abs(values['opencode']['api_usd']-3)<1e-8
            assert abs(values['grok']['api_usd']-.185)<1e-8
            for url,mode in [(base,'node'),(f'http://127.0.0.1:{hub.port}','hub')]:
                for width in (1280,390):
                    for theme in ('light','dark'):
                        page.set_viewport_size({'width':width,'height':950});page.goto(url+'/quotas.html')
                        page.evaluate('(theme)=>document.documentElement.dataset.theme=theme',theme)
                        expect(current).to_contain_text('$475.97')
                        assert page.evaluate('document.documentElement.scrollWidth<=innerWidth'),(mode,width,theme)
                        current.focus();current.evaluate('(n)=>n.blur()')
                        if width==390:
                            page.locator('.quota-scroll').evaluate_all("nodes=>nodes.forEach(n=>{const cell=n.querySelector('.quota-api'),first=n.querySelector('tbody th');n.scrollLeft+=cell.getBoundingClientRect().left-n.getBoundingClientRect().left-first.getBoundingClientRect().width})")
                        page.screenshot(path=str(args.output/f'{mode}-{width}-{theme}.png'),full_page=True)
            page.set_viewport_size({'width':1280,'height':950});page.goto(base)
            page.locator('#settings').click();page.get_by_role('tab',name='机器',exact=True).click();page.get_by_role('link',name='Agent 用量 →').click()
            expect(page.locator('#quota-dialog')).to_be_visible();expect(current).to_contain_text('$475.97')
            expect(page.locator('#usage-cycles')).to_have_count(0)
            page.screenshot(path=str(args.output/'settings-dialog.png'),full_page=True)
            page.locator('#quota-dialog .modal-close').click();expect(page.locator('#quota-dialog')).to_have_count(0)
            page.goto(base+'/quotas.html')
            def wait_snapshot(predicate):
                deadline=time.monotonic()+45
                while time.monotonic()<deadline:
                    report=page.evaluate("fetch('api/usage',{cache:'no-store'}).then(r=>r.json())")
                    if not report.get('running') and predicate(report):return report
                    page.wait_for_timeout(200)
                raise AssertionError(report)
            def refresh():
                before=page.evaluate("fetch('api/usage').then(r=>r.json()).then(r=>r.captured_at)")
                page.locator('#quota-refresh').click()
                wait_snapshot(lambda r:r.get('captured_at')!=before)
                page.reload();expect(current).to_be_visible()
            refresh();expect(current).to_contain_text('$475.97')
            # Same request copies and unchanged files do not add charges.
            shutil.copyfile(cp,cp.with_name('rollout-copy.jsonl'));refresh()
            expect(current).to_contain_text('$475.97');refresh()
            assert json.loads((directory/'snapshot.json').read_text())['metrics']['bytes_read']==0
            # First 5% matches the reference and uses the same observation time.
            q=json.loads((state/'quotas/codex.json').read_text())
            q['captured_at']=iso(now-120);q['buckets'][0]['remaining_percent']=95;dump(state/'quotas/codex.json',q)
            refresh();expect(current).to_contain_text('$143.82');expect(current).to_contain_text('$3,196.08')
            # Long-context multiplier applies to the entire request, not excess tokens.
            quotas(state,now,used=15)
            append(cp,*codex('long',now-30));refresh();expect(current).to_contain_text('$492.69')
            # Half an appended line is retried. Unknown pricing hides the estimate.
            record=codex('unknown',now-25,'unpriced-model')[-1]
            prefix=codex('unknown',now-25,'unpriced-model')[0];append(cp,prefix)
            raw=json.dumps(record)
            with cp.open('a') as f:f.write(raw[:40])
            refresh();expect(current).to_contain_text('$492.69')
            with cp.open('a') as f:f.write(raw[40:]+'\n')
            refresh();expect(current.locator('small')).to_have_count(0)
            assert '缺少型号、价格或完整计量' in current.get_attribute('title')
            # Correct a price without re-reading native logs.
            prices=json.loads((directory/'prices.json').read_text())
            prices['models']['unpriced-model']=prices['models']['gpt-6.1-sol'];dump(directory/'prices.json',prices)
            refresh();expect(current).to_contain_text('$509.41')
            # Stop at first 100%; a later Credits-funded call is excluded.
            append(cp,quota_point(now-15,now+7200,100),*codex('paid',now-10))
            quotas(state,now,used=100);refresh();expect(current).to_contain_text('$509.41')
            expect(current.locator('small')).to_have_text('整轮 ≈ $509.41')
            assert '后续 Credits' in current.get_attribute('title')
            quotas(state,now,used=0);refresh();expect(current.locator('small')).to_have_count(0)
            quotas(state,now-1800,used=30);refresh();expect(current.locator('small')).to_have_count(0)
            # Missing retired history is explicit and must not yield a full estimate.
            quotas(state,now,used=100);retired.unlink();refresh()
            assert '迁出历史缺失' in current.get_attribute('title');expect(current.locator('small')).to_have_count(0)
            page.screenshot(path=str(args.output/'partial.png'),full_page=True)
            # The background timer updates without clicking refresh.
            result=page.evaluate("fetch('api/usage/settings',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({enabled:true,interval_seconds:1,sources:{}})}).then(r=>r.ok)")
            assert result
            append(root/'claude/project/session.jsonl',{'type':'assistant','sessionId':'claude-usage','timestamp':iso(now-5),
                'message':{'id':'timer','model':'claude-sonnet-5-5','usage':{'input_tokens':1000000,'output_tokens':0}}})
            wait_snapshot(lambda r:any(v['source']=='claude' and v['api_usd'] is not None and abs(v['api_usd']-8.35)<.001 for v in r['rows']))
            page.reload();expect(page.locator('[data-source="claude"] tbody .quota-api')).to_contain_text('$8.35')
            assert not errors,errors
            print('PASS: original node/Hub tables, eight adapters, reference checkpoints, ownership, compaction, long context, exhaustion, duplicate/tail, partial/stale/zero, periodic refresh, wide/narrow light/dark')


if __name__=='__main__':main()
