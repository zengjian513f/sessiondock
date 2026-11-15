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


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=Path,default=BINARY);parser.add_argument('--output',type=Path,default=REPO/'target/agent-usage/screenshots');parser.add_argument('--baseline',action='store_true');args=parser.parse_args()
    args.output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-usage-') as tmp, ExitStack() as stack:
        root=Path(tmp);state=root/'state';state.mkdir();now=time.time();roots={}
        for source in ('codex','claude','grok','agy','kiro','droid','codebuddy'):
            roots[source]=str(root/source);(root/source).mkdir()
        cp=root/'codex/2026/11/15/rollout-usage.jsonl'
        append(cp,{'type':'session_meta','timestamp':iso(now-100),'payload':{'id':'usage','cwd':'/synthetic'}},
               {'type':'event_msg','timestamp':iso(now-40*86400),'payload':{'type':'token_count','info':{'total_token_usage':{'input_tokens':5000000,'total_tokens':5000000}}}},
               *codex('r1',now-60))
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
        quotas(state,now)
        directory=state/'agent-usage';directory.mkdir()
        dump(directory/'settings.json',{'enabled':False,'interval_seconds':1,'sources':{}})
        dump(directory/'config.json',{'directory':str(directory),'roots':roots,'quota_dir':str(state/'quotas'),'prices':str(REPO/'tools/agent_usage_prices.json')})
        command=[sys.executable,str(REPO/'tools/agent_usage.py'),'collect','--config',str(directory/'config.json')]
        subprocess.run(command,check=True,capture_output=True)
        first=json.loads((directory/'snapshot.json').read_text());assert first['metrics']['bytes_read']>0
        subprocess.run(command,check=True,capture_output=True)
        second=json.loads((directory/'snapshot.json').read_text());assert second['metrics']['bytes_read']==0,second['metrics']
        assert 'PRIVATE-BODY' not in (directory/'ledger.sqlite').read_bytes().decode(errors='ignore')
        with sync_playwright() as pw, ExitStack() as browser_stack:
            options={'headless':True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):options['executable_path']=os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser=pw.chromium.launch(**options);browser_stack.callback(browser.close)
            port=free_port();env=node_env(root,port,'127.0.0.0/8');env['SESSIONDOCK_USAGE_PYTHON']=sys.executable
            base,_=stack.enter_context(isolated_server(Corpus(root),args.binary.resolve(),state_dir=state,extra_env=env))
            hr=root/'hub';hr.mkdir();hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hr,[SimpleNamespace(port=port,token=TOKEN,name='Usage Node')]);hub.start();stack.callback(hub.stop)
            context=browser.new_context(viewport={'width':1280,'height':950});page=context.new_page();errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
            context.route('**/api/clients',lambda route:route.fulfill(json={'clients':[{'source':s,'installed':True} for s in roots]}))
            if args.baseline:
                page.goto(base+'/quotas.html');expect(page.locator('#quota-agents .quota-agent')).not_to_have_count(0)
                page.screenshot(path=str(args.output/'before-wide-light.png'),full_page=True);return
            page.goto(base+'/quotas.html')
            cards=page.locator('.usage-cycle');expect(cards).to_have_count(9)
            current=page.locator('.usage-cycle[data-source="codex"][data-window="codex:primary"]')
            expect(current).to_contain_text('$10.86');expect(current).to_contain_text('$43.44')
            expect(page.locator('.usage-cycle[data-source="claude"]')).to_contain_text('$25.40')
            expect(page.locator('.usage-cycle[data-source="agy"]')).to_contain_text('$12.80')
            expect(page.locator('.usage-cycle[data-source="codebuddy"]')).to_contain_text('$21.72')
            expect(page.locator('.usage-cycle[data-source="opencode"]')).to_contain_text('$3.00')
            expect(page.locator('.usage-cycle[data-source="kiro"]')).to_contain_text('官方 Credits')
            current.locator('summary').click();expect(current).to_contain_text('输入 400,000');expect(current).to_contain_text('输出 1,000,000')
            current.locator('summary').click()
            for url,mode in [(base,'node'),(f'http://127.0.0.1:{hub.port}','hub')]:
                for width in (1280,390):
                    for theme in ('light','dark'):
                        page.set_viewport_size({'width':width,'height':950});page.goto(url+'/quotas.html')
                        page.evaluate('(theme)=>document.documentElement.dataset.theme=theme',theme)
                        expect(page.locator('.usage-cycle').first).to_be_visible()
                        assert page.evaluate('document.documentElement.scrollWidth<=innerWidth'),(mode,width,theme)
                        page.screenshot(path=str(args.output/f'{mode}-{width}-{theme}.png'),full_page=True)
                        page.get_by_role('button',name='统计设置',exact=False).click()
                        expect(page.get_by_role('form',name='统计设置')).to_be_visible();page.get_by_role('button',name='取消',exact=True).click()
            page.set_viewport_size({'width':1280,'height':950});page.goto(base)
            page.locator('#settings').click();page.get_by_role('tab',name='机器',exact=True).click();page.get_by_role('link',name='Agent 用量 →').click()
            expect(page.locator('#quota-dialog')).to_be_visible();expect(page.locator('#usage-cycles')).to_be_visible()
            page.screenshot(path=str(args.output/'settings-dialog.png'),full_page=True)
            page.locator('#quota-dialog .modal-close').click();expect(page.locator('#quota-dialog')).to_have_count(0)
            page.goto(base+'/quotas.html');page.locator('#usage-refresh').click();expect(page.locator('#usage-refresh')).to_be_enabled(timeout=30000)
            # The running service supplies only its configured roots; unrelated
            # standalone adapter samples above are no longer in this node report.
            current=page.locator('.usage-cycle[data-source="codex"][data-window="codex:primary"]')
            expect(current).to_contain_text('$43.44')
            # Append, then copy the same request into another file: one charge.
            append(cp,*codex('r2',now-30));shutil.copyfile(cp,cp.with_name('rollout-copy.jsonl'))
            page.locator('#usage-refresh').click();expect(page.locator('#usage-refresh')).to_be_enabled(timeout=30000)
            expect(current).to_contain_text('$21.72');expect(current).to_contain_text('$86.88')
            page.locator('#usage-refresh').click();expect(page.locator('#usage-refresh')).to_be_enabled(timeout=30000)
            expect(current).to_contain_text('$21.72')
            assert json.loads((directory/'snapshot.json').read_text())['metrics']['bytes_read']==0
            # A partial tail is retried, never charged twice.
            partial=codex('r3',now-20)[-1];raw=json.dumps(partial)
            with cp.open('a') as f:f.write(raw[:40])
            page.locator('#usage-refresh').click();expect(page.locator('#usage-refresh')).to_be_enabled(timeout=30000);expect(current).to_contain_text('$21.72')
            with cp.open('a') as f:f.write(raw[40:]+'\n')
            page.locator('#usage-refresh').click();expect(page.locator('#usage-refresh')).to_be_enabled(timeout=30000);expect(current).to_contain_text('$32.58')
            quotas(state,time.time(),used=0)
            page.locator('#usage-refresh').click();expect(page.locator('#usage-refresh')).to_be_enabled(timeout=30000);expect(current).to_contain_text('已用 0%');expect(current.locator('dd').nth(2)).to_have_text('—')
            quotas(state,time.time()-1800,used=30)
            page.locator('#usage-refresh').click();expect(page.locator('#usage-refresh')).to_be_enabled(timeout=30000);expect(current.locator('dd').nth(2)).to_have_text('—')
            # Monthly boundaries are explicit and apply only to this reset.
            quotas(state,time.time(),used=25)
            monthly=json.loads((state/'quotas/codex.json').read_text())
            monthly['buckets'][0]['window']='monthly';dump(state/'quotas/codex.json',monthly)
            page.locator('#usage-refresh').click();expect(page.locator('#usage-refresh')).to_be_enabled(timeout=30000)
            expect(current).to_contain_text('缺少周期边界')
            page.get_by_role('button',name='统计设置',exact=False).click()
            page.get_by_label('Codex codex 本周期起点',exact=True).fill(datetime.fromtimestamp(now-86400).strftime('%Y-%m-%dT%H:%M'))
            page.get_by_role('button',name='保存',exact=True).click()
            expect(page.get_by_role('form',name='统计设置')).to_have_count(0)
            expect(current).to_contain_text('$130.32',timeout=30000)
            monthly['buckets'][0]['resets_at']=iso(now+86400);dump(state/'quotas/codex.json',monthly)
            page.locator('#usage-refresh').click();expect(page.locator('#usage-refresh')).to_be_enabled(timeout=30000)
            expect(current).to_contain_text('缺少周期边界')
            # Separate quota pools cannot share an unclassified numerator.
            quotas(state,time.time(),used=25)
            pools=json.loads((state/'quotas/codex.json').read_text());pools['buckets'][1]['group']='special-model';dump(state/'quotas/codex.json',pools)
            page.locator('#usage-refresh').click();expect(page.locator('#usage-refresh')).to_be_enabled(timeout=30000)
            expect(current).to_contain_text('无法确认请求所属额度组');expect(current.locator('dd').nth(2)).to_have_text('—')
            quotas(state,time.time(),used=25);append(cp,*codex('r4',now-10,'unpriced-model'))
            page.locator('#usage-refresh').click();expect(page.locator('#usage-refresh')).to_be_enabled(timeout=30000);expect(current).to_contain_text('缺少型号、价格或完整计量')
            page.screenshot(path=str(args.output/'unpriced.png'),full_page=True)
            # Actual settings interaction enables the service timer. A later
            # request is ingested without pressing update.
            page.get_by_role('button',name='统计设置',exact=False).click()
            page.get_by_label('周期性更新',exact=True).check();page.get_by_label('更新间隔（秒）',exact=True).fill('1');page.get_by_role('button',name='保存',exact=True).click()
            expect(page.get_by_role('form',name='统计设置')).to_have_count(0)
            append(cp,*codex('timer-r5',now-5))
            # Refresh the visible page to read snapshots; these GETs do not run collectors.
            deadline=time.monotonic()+45
            while time.monotonic()<deadline:
                page.reload();expect(current).to_be_visible()
                if '$43.44' in current.inner_text():break
                page.wait_for_timeout(1000)
            expect(current).to_contain_text('$43.44')
            assert not errors,errors
            print('PASS: node/Hub UI, 8 adapters, formula, duplicate/partial tail, zero/stale/missing price, monthly boundaries/reset, quota pools, settings, real timer, wide/narrow light/dark')


if __name__=='__main__':main()
