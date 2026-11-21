#!/usr/bin/env python3
"""Real collector + node/Hub: cycle money, denominator, incremental reads and UI."""
import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
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
    usage={'input_tokens':1000000,'cached_input_tokens':600000,'output_tokens':1000000,
           'reasoning_output_tokens':900000,'cache_write_input_tokens':0}
    return [{'type':'turn_context','timestamp':iso(when),'payload':{'model':model}},
            {'type':'token_usage_record','timestamp':iso(when),'payload':{'response_id':ident,'usage':usage}},
            {'type':'event_msg','timestamp':iso(when),'payload':{'type':'token_count','info':{'last_token_usage':usage}}}]


def quotas(root, now, used=25):
    for source in ('codex','claude','agy','droid','kiro'):
        windows=[{'id':source+':primary','group':'Gemini Models' if source=='agy' else 'Standard' if source=='droid' else source,
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
        append(path,{'type':'event_msg','timestamp':iso(when+i/count),'payload':{'type':'token_count',
            'info':{'last_token_usage':usage}}})



def price_upgrade(browser, binary, output):
    """An existing private price file must gain new models from the release."""
    with tempfile.TemporaryDirectory(prefix='sessiondock-price-upgrade-') as tmp, ExitStack() as stack:
        root=Path(tmp);state=root/'state';directory=state/'agent-usage';now=time.time()
        native=root/'codex';native.mkdir();path=native/'usage.jsonl'
        append(path,{'type':'session_meta','payload':{'id':'price-upgrade'}},
               quota_point(now-30,now+7200,0))
        for model in ('gpt-6.2-sol','gpt-6.2-astra'):
            for total in (100000,300000):
                records=codex(f'{model}-{total}',now-20,model)
                usage=records[1]['payload']['usage']
                usage.update(input_tokens=total,cached_input_tokens=total*3//5,
                             output_tokens=100000,reasoning_output_tokens=0)
                append(path,*records)
        append(path,quota_point(now-10,now+7200,25))
        quotas(state,now)
        prices=json.loads((REPO/'tools/agent_usage_prices.json').read_text())
        for model in ('gpt-6.2-sol','gpt-6.2-astra'):prices['models'].pop(model)
        prices['version']='old-private-catalog'
        prices['models']['gpt-6.1-sol']['input']=7  # Preserve administrator overrides.
        dump(directory/'prices.json',prices)
        dump(directory/'settings.json',{'enabled':False,'interval_seconds':300,'sources':{}})
        dump(directory/'config.json',{'directory':str(directory),'roots':{'codex':str(native)},
             'quota_dir':str(state/'quotas'),'prices':str(directory/'prices.json')})
        subprocess.run([sys.executable,str(REPO/'tools/agent_usage.py'),'collect','--config',
                        str(directory/'config.json')],check=True,capture_output=True)
        old=json.loads((directory/'snapshot.json').read_text())
        assert old['rows'][0]['unpriced']==4 and old['rows'][0]['equivalent_usd'] is None
        port=free_port();env=node_env(root,port,'127.0.0.0/8')
        env.update(SESSIONDOCK_USAGE_PYTHON=sys.executable,SESSIONDOCK_CODEX_ROOT=str(native))
        base,_=stack.enter_context(isolated_server(Corpus(root),binary,state_dir=state,extra_env=env))
        hr=root/'hub';hr.mkdir()
        hub=Hub(binary.with_name('sessiondock-hub'),hr,[SimpleNamespace(port=port,token=TOKEN,name='Price Node')])
        hub.start();stack.callback(hub.stop)
        context=browser.new_context(viewport={'width':1280,'height':950});stack.callback(context.close)
        context.route('**/api/clients',lambda route:route.fulfill(json={'clients':[{'source':'codex','installed':True}]}))
        page=context.new_page();page.goto(base+'/quotas.html')
        cell=page.locator('[data-source="codex"] tbody .quota-api').first
        expect(cell).to_contain_text('N/A')
        page.locator('#quota-refresh').click()
        deadline=time.monotonic()+45
        while time.monotonic()<deadline:
            result=page.evaluate("fetch('api/usage').then(r=>r.json())")
            if not result.get('running') and result.get('captured_at')!=old['captured_at']:break
            page.wait_for_timeout(200)
        row=result['rows'][0]
        assert row['unpriced']==0 and not row['partial'],row
        assert abs(row['api_usd']-15.51)<1e-8,row
        assert row['equivalent_usd'] is not None and abs(row['equivalent_usd']-15.51/.245)<1e-8,row
        assert result['metrics']['bytes_read']==0,result['metrics']
        assert json.loads((directory/'prices.json').read_text())==prices
        for url,mode in ((base,'node'),(f'http://127.0.0.1:{hub.port}','hub')):
            for width,theme in ((1280,'light'),(390,'dark')):
                page.set_viewport_size({'width':width,'height':950});page.goto(url+'/quotas.html')
                page.evaluate('(theme)=>document.documentElement.dataset.theme=theme',theme)
                expect(cell).to_have_text('$15.51整轮 ≈ $63.31')
                assert 'gpt-6.2-sol' in cell.get_attribute('title')
                assert 'gpt-6.2-astra' in cell.get_attribute('title')
                if width==390:
                    page.locator('.quota-scroll').evaluate_all("nodes=>nodes.forEach(n=>n.scrollLeft=100)")
                page.screenshot(path=str(output/f'price-upgrade-{mode}-{width}.png'),full_page=True)
        page.set_viewport_size({'width':1280,'height':950});page.goto(base)
        page.locator('#settings').click();page.get_by_role('tab',name='机器',exact=True).click()
        page.get_by_role('link',name='Agent 用量 →').click()
        expect(cell).to_have_text('$15.51整轮 ≈ $63.31')
        page.screenshot(path=str(output/'price-upgrade-settings.png'),full_page=True)
        page.locator('#quota-dialog .modal-close').click();page.goto(base+'/quotas.html')
        def refresh():
            before=page.evaluate("fetch('api/usage').then(r=>r.json()).then(r=>r.captured_at)")
            page.locator('#quota-refresh').click()
            deadline=time.monotonic()+45
            while time.monotonic()<deadline:
                report=page.evaluate("fetch('api/usage').then(r=>r.json())")
                if not report.get('running') and report.get('captured_at')!=before:
                    page.reload();return report
                page.wait_for_timeout(200)
            raise AssertionError(report)
        # A rewind repeats a retained cumulative checkpoint, then switches to
        # per-request records. It must not make the whole machine incomplete.
        total={'input_tokens':800000,'cached_input_tokens':480000,
               'cache_write_input_tokens':0,'output_tokens':400000}
        checkpoint=lambda at: {'type':'event_msg','timestamp':iso(at),'payload':{
            'type':'token_count','info':{'total_token_usage':total}}}
        resumed=native/'resumed.jsonl'
        append(resumed,{'type':'session_meta','payload':{'id':'price-upgrade'}},
               checkpoint(now-12),*codex('continued',now-11,'gpt-6.2-sol'))
        result=refresh();assert result['rows'][0]['partial']
        expect(cell).not_to_contain_text('整轮')
        append(path,checkpoint(now-15))
        archive=root/'archived_sessions';archive.mkdir();path.rename(archive/'original.jsonl')
        result=refresh();row=result['rows'][0]
        assert not row['partial'] and row['reason'] is None,row
        expect(cell).to_have_text('$32.23整轮 ≈ $131.55')
        # Old indexes lack checkpoint evidence. Upgrade unchanged native and
        # archived files once; later refreshes remain incremental.
        with sqlite3.connect(directory/'ledger-v2.sqlite') as index:
            index.execute("update parser_versions set version=1 where source='codex'")
            index.execute('delete from counter_checkpoints')
            for path_name,raw in index.execute("select path,state from files where source='codex'").fetchall():
                state_value=json.loads(raw);state_value.pop('counter_baseline',None)
                index.execute("update files set state=? where source='codex' and path=?",(json.dumps(state_value),path_name))
        result=refresh();assert result['metrics']['bytes_read']>0
        expect(cell).to_have_text('$32.23整轮 ≈ $131.55')
        result=refresh();assert result['metrics']['bytes_read']==0
        # Same counts under another SID are not evidence of complete history.
        append(native/'unrelated.jsonl',{'type':'session_meta','payload':{'id':'unrelated'}},checkpoint(now-9))
        result=refresh();assert result['rows'][0]['partial']
        assert '历史记录不完整' in cell.get_attribute('title')
        expect(cell).not_to_contain_text('整轮')


def price_sync_case(browser, binary, output):
    with tempfile.TemporaryDirectory(prefix='sessiondock-price-sync-') as tmp, ExitStack() as stack:
        root=Path(tmp);state=root/'state';directory=state/'agent-usage';now=time.time()
        remote={'calls':0,'mode':'ok','revision':1}
        class OfficialPrices(BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_GET(self):
                remote['calls']+=1
                if remote['mode']=='offline':
                    self.close_connection=True;return
                rows=(
                    '| gpt-6.1-sol | $4 | $20 | $0.2 | $5 | $10 | $8 | $30 | $0.4 | $10 | $20 |\n'
                    '| gpt-sync-new | $2 | $10 | $0.1 | $2.5 | $5 | $4 | $15 | $0.2 | $5 | $10 |'
                    if remote['revision']==1 else
                    '| gpt-6.1-sol | $6 | $30 | $0.3 | $7.5 | $15 | $12 | $45 | $0.6 | $15 | $30 |\n'
                    '| gpt-sync-new | $3 | $15 | $0.15 | $3.75 | $7.5 | $6 | $22.5 | $0.3 | $7.5 | $15 |')
                header=('| Model | Short context input | Short context output | Short context cached input | Short context 30m write | Short context 12h write | Long context input | Long context output | Long context cached input | Long context 30m write | Long context 12h write |\n'
                        '| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n')
                body=('### Standard pricing data\n'+header+rows+'\n'
                      '| Model | Input | Output | Cached input |\n| --- | --- | --- | --- |\n'
                      '| gpt-sync-mini | $1 | $5 | - |\n'
                      '### Batch pricing data\n| Model | Input | Output | Cached input |\n'
                      '| gpt-sync-new | $999 | $999 | $999 |\n'
                      '### Fast pricing data\n'+header+
                      '| gpt-6.1-sol | $4 | $20 | $0.2 | $5 | $10 | $8 | $30 | $0.4 | $10 | $20 |\n'
                      '| gpt-5.4 (<272K context length) | $5 | $30 | $0.5 | - | - | - | - | - | - | - |\n'
                      '### Ultrafast pricing data\n'+header+
                      '| gpt-6.1-sol | $12 | $60 | $0.6 | $15 | $30 | $24 | $90 | $1.2 | $30 | $60 |\n'
                      'Short context: ≤272K input tokens. Long context: >272K input tokens.\n'
                      'Ultrafast costs 3 times the Standard rate through February 28, 2027, and 6 times the Standard rate starting March 1, 2027 (Pacific time).\n'
                      '### Ultrafast promotion pricing data\n'+header+
                      '| gpt-sync-new | $6 | $30 | $0.3 | $7.5 | $15 | $12 | $45 | $0.6 | $15 | $30 |\n'
                      '### Ultrafast list pricing data\n'+header+
                      '| gpt-sync-new | $12 | $60 | $0.6 | $15 | $30 | $24 | $90 | $1.2 | $30 | $60 |\n'
                      'Specialized models\n\nStandard\n\n### Grouped Pricing Table data\n'
                      '| Category | Model | Input | Output | Cached input | 30m write | 12h write |\n'
                      '| --- | --- | --- | --- | --- | --- | --- |\n'
                      '| Codex | gpt-5.3-codex | $1.75 | $14 | $0.175 | - | - |\n'
                      'Fast\n\n### Grouped Pricing Table data\n'
                      '| Category | Model | Input | Output | Cached input |\n'
                      '| --- | --- | --- | --- | --- |\n'
                      '| Codex | gpt-5.3-codex | $3.50 | $28 | $0.35 |\n')
                if remote['mode']=='malformed':body=body.replace('Short context output','Changed output column')
                raw=body.encode();self.send_response(200);self.send_header('Content-Type','text/markdown')
                self.send_header('Content-Length',str(len(raw)));self.end_headers();self.wfile.write(raw)
        httpd=ThreadingHTTPServer(('127.0.0.1',0),OfficialPrices)
        thread=threading.Thread(target=httpd.serve_forever,daemon=True);thread.start()
        stack.callback(httpd.server_close);stack.callback(httpd.shutdown)
        native=root/'codex';native.mkdir();path=native/'usage.jsonl'
        append(path,{'type':'session_meta','payload':{'id':'sync'}},quota_point(now-40,now+7200,0))
        for model,total in (('gpt-6.1-sol',100000),('gpt-sync-new',100000),('gpt-sync-new',300000)):
            records=codex(f'{model}-{total}',now-20,model)
            records[1]['payload']['usage'].update(input_tokens=total,cached_input_tokens=total*3//5,
                                                 output_tokens=100000,reasoning_output_tokens=0)
            append(path,*records)
        append(path,quota_point(now-10,now+7200,25));quotas(state,now)
        prior=json.loads((REPO/'tools/agent_usage_prices.json').read_text())
        prior['version']='prior-release';prior['models']['gpt-6.1-sol']['output']=8
        dump(directory/'default-prices.json',prior)
        custom=json.loads(json.dumps(prior))
        custom['models']['gpt-6.1-sol']['input']=7
        dump(directory/'prices.json',custom)
        dump(directory/'settings.json',{'enabled':False,'interval_seconds':1,'sources':{}})
        dump(directory/'config.json',{'directory':str(directory),'roots':{'codex':str(native)},
             'quota_dir':str(state/'quotas'),'prices':str(directory/'prices.json')})
        subprocess.run([sys.executable,str(REPO/'tools/agent_usage.py'),'collect','--config',str(directory/'config.json')],check=True,capture_output=True)
        port=free_port();env=node_env(root,port,'127.0.0.0/8')
        env.update(SESSIONDOCK_USAGE_PYTHON=sys.executable,SESSIONDOCK_USAGE_PRICE_SYNC='on',
                   SESSIONDOCK_USAGE_PRICE_URL=f'http://127.0.0.1:{httpd.server_port}/pricing.md')
        dump(directory/'online-prices.json', {'url':env['SESSIONDOCK_USAGE_PRICE_URL'],
             'checked_at':now, 'updated_at':iso(now), 'models':{}})
        base,_=stack.enter_context(isolated_server(Corpus(root),binary,state_dir=state,extra_env=env))
        hr=root/'hub';hr.mkdir();hub=Hub(binary.with_name('sessiondock-hub'),hr,[SimpleNamespace(port=port,token=TOKEN,name='Price Sync')])
        hub.start();stack.callback(hub.stop)
        context=browser.new_context(viewport={'width':1280,'height':950});stack.callback(context.close)
        context.route('**/api/clients',lambda r:r.fulfill(json={'clients':[{'source':'codex','installed':True}]}))
        page=context.new_page();page.goto(base+'/quotas.html')
        cell=page.locator('[data-source="codex"] tbody .quota-api').first
        expect(cell).not_to_contain_text('整轮');assert remote['calls']==0
        def report():return page.evaluate("fetch('api/usage').then(r=>r.json())")
        def wait_for(predicate):
            deadline=time.monotonic()+45
            while time.monotonic()<deadline:
                result=report()
                if not result.get('running') and predicate(result):return result
                page.wait_for_timeout(200)
            raise AssertionError(result)
        def expire():
            cache=json.loads((directory/'online-prices.json').read_text());cache['checked_at']=0
            dump(directory/'online-prices.json',cache)
        def refresh(due=False):
            if due:expire()
            before=report()['captured_at'];page.locator('#quota-refresh').click()
            result=wait_for(lambda r:r['captured_at']!=before);page.reload();return result
        result=refresh();expect(cell).to_contain_text('$5.39');expect(cell).to_contain_text('整轮')
        assert result['rows'][0]['unpriced']==0 and result['metrics']['bytes_read']==0
        assert remote['calls']==1 and result['price_sync']['updated_at']
        assert json.loads((directory/'prices.json').read_text())==custom
        assert json.loads((directory/'price-base.json').read_text())==prior
        refresh();assert remote['calls']==1  # Normal refreshes use the daily cache.
        remote['revision']=2;result=refresh(True);expect(cell).to_contain_text('$7.95')
        successful=result['price_sync']['updated_at'];assert remote['calls']==2
        for mode in ('offline','malformed'):
            remote['mode']=mode;result=refresh(True)
            expect(cell).to_contain_text('$7.95');expect(cell).to_contain_text('整轮')
            assert result['price_sync']['updated_at']==successful
            assert '同步失败' in cell.get_attribute('title')
            calls=remote['calls'];refresh();assert remote['calls']==calls  # Failure backoff.
        # Cold outage falls back to embedded prices and keeps genuinely unknown models partial.
        (directory/'online-prices.json').unlink();remote['mode']='offline'
        result=refresh();expect(cell).to_contain_text('$1.29');expect(cell).not_to_contain_text('整轮')
        assert result['price_sync']['updated_at'] is None and result['rows'][0]['unpriced']==2
        remote['mode']='ok';result=refresh(True);expect(cell).to_contain_text('$7.95')
        # An administrator can explicitly pin even a rate equal to an old default.
        dump(directory/'price-overrides.json',{'models':{'gpt-sync-new':{'input':2}}})
        refresh();expect(cell).to_contain_text('$7.91')
        (directory/'price-overrides.json').unlink()
        managed=json.loads(json.dumps(custom));managed['models']['gpt-6.1-sol']['input']=2
        managed['models']['gpt-6.1-sol'].pop('long')
        dump(directory/'prices.json',managed)
        result=refresh();expect(cell).to_contain_text('$7.91')
        assert '本地覆盖' not in result['price_version']
        dump(directory/'prices.json',custom)
        remote['revision']=1;expire();before=report()['price_sync']['updated_at']
        page.evaluate("fetch('api/usage/settings',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({enabled:true,interval_seconds:1,sources:{}})}).then(r=>r.json())")
        result=wait_for(lambda r:r.get('price_sync',{}).get('updated_at')!=before)
        assert result['price_sync']['error'] is None and result['metrics']['bytes_read']==0
        page.reload();expect(cell).to_contain_text('$5.39')
        for url,mode in ((base,'node'),(f'http://127.0.0.1:{hub.port}','hub')):
            for width,theme in ((1280,'light'),(390,'dark')):
                page.set_viewport_size({'width':width,'height':950});page.goto(url+'/quotas.html')
                page.evaluate('(theme)=>document.documentElement.dataset.theme=theme',theme)
                expect(cell).to_contain_text('$5.39');assert 'OpenAI 价目上次同步' in cell.get_attribute('title')
                if width==390:page.locator('.quota-scroll').evaluate_all('nodes=>nodes.forEach(n=>n.scrollLeft=100)')
                page.screenshot(path=str(output/f'price-sync-{mode}-{width}.png'),full_page=True)
        page.set_viewport_size({'width':1280,'height':950});page.goto(base)
        page.locator('#settings').click();page.get_by_role('tab',name='机器',exact=True).click()
        page.get_by_role('link',name='Agent 用量 →').click()
        expect(cell).to_contain_text('$5.39');assert 'OpenAI 价目上次同步' in cell.get_attribute('title')
        page.screenshot(path=str(output/'price-sync-settings.png'),full_page=True)

        page.goto(base+'/quotas.html')
        page.evaluate("fetch('api/usage/settings',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({enabled:false,interval_seconds:1,sources:{}})}).then(r=>r.json())")
        wait_for(lambda r: not r['settings']['enabled'])
        def settings(tier, at):
            return {'type':'event_msg','timestamp':iso(at),'payload':{
                'type':'thread_settings_applied','thread_settings':{'service_tier':tier}}}
        def request(ident, at, model, total, tier=None):
            records = codex(ident,at,model)
            records[1]['payload']['usage'].update(input_tokens=total,cached_input_tokens=total*3//5,output_tokens=100000)
            if tier:
                records[1]['payload']['service_tier'] = tier
            return records

        fast = request('tier-fast',now-18,'gpt-6.1-sol',100000)
        fast[1]['timestamp'] = fast[2]['timestamp'] = iso(now-17)
        append(path,settings('priority',now-19),fast[0],settings('ultrafast',now-17.5),*fast[1:],
               *request('tier-ultra',now-16,'gpt-sync-new',300000),
               *request('tier-standard',now-15,'gpt-6.1-sol',100000,'default'))
        result = refresh()
        pricing = json.loads((directory/'online-prices.json').read_text())
        assert pricing['tiers']['fast']['gpt-5.3-codex']['output'] == 28
        promotion, regular = pricing['tiers']['ultrafast']['gpt-sync-new']
        assert promotion['until'] == regular['from'] == '2027-03-01T08:00:00Z'
        expected = '$15.91' if iso(now-16) < '2027-03-01T08:00:00+00:00' else '$21.95'
        expect(cell).to_contain_text(expected)
        assert 'gpt-6.1-sol · Fast：$2.17' in cell.get_attribute('title')
        assert 'gpt-sync-new · Ultrafast' in cell.get_attribute('title')
        assert '2 次调用未记录实际档位' in cell.get_attribute('title')
        assert result['rows'][0]['unpriced'] == 0
        remote['mode'] = 'offline'
        result = refresh(True)
        expect(cell).to_contain_text(expected)
        assert result['price_sync']['error'] and result['rows'][0]['unpriced'] == 0
        page.screenshot(path=str(output/'price-tiers-node.png'),full_page=True)
        page.set_viewport_size({'width':390,'height':950})
        page.goto(f'http://127.0.0.1:{hub.port}/quotas.html')
        page.evaluate("document.documentElement.dataset.theme='dark'")
        expect(cell).to_contain_text(expected)
        assert 'Ultrafast' in cell.get_attribute('aria-label')
        page.locator('.quota-scroll').evaluate_all('nodes=>nodes.forEach(n=>n.scrollLeft=100)')
        page.screenshot(path=str(output/'price-tiers-hub.png'),full_page=True)

        page.goto(base+'/quotas.html')
        append(path,*request('tier-unsupported',now-14,'gpt-sync-mini',100000,'fast'))
        result = refresh()
        assert result['rows'][0]['unpriced'] == 1
        expect(cell).to_contain_text('记录不完整')
        expect(cell).not_to_contain_text('整轮')


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
        append(cp,*codex('unreported',now-45)[:-1])
        append(cp,*codex('compaction',now-40),{'type':'compacted','timestamp':iso(now-39),'payload':{'compaction_response_id':'compaction'}})
        append(root/'claude/project/session.jsonl',{'type':'assistant','sessionId':'claude-usage','timestamp':iso(now-60),
            'message':{'id':'claude-r1','model':'claude-sonnet-5-5','usage':{'input_tokens':1000000,'output_tokens':100000,'cache_read_input_tokens':1000000,
            'cache_creation_input_tokens':1000000,'cache_creation':{'ephemeral_5m_input_tokens':500000,'ephemeral_1h_input_tokens':500000}},'content':[{'type':'text','text':'PRIVATE-BODY-MUST-NOT-ENTER-LEDGER'}]}})
        agy_path=root/'agy/brain/one/.system_generated/logs/transcript_full.jsonl'
        notice=lambda model: {'type':'USER_INPUT','status':'DONE','content':'<USER_REQUEST>\nPRIVATE-BODY\n</USER_REQUEST>\n<USER_SETTINGS_CHANGE>\nThe user changed setting `Model Selection` from None to '+model+'. No need to comment on this change if the user does not ask.\n</USER_SETTINGS_CHANGE>'}
        agy_call={'type':'PLANNER_RESPONSE','status':'DONE','created_at':iso(now-60),'input_tokens':1000000,'cache_read_tokens':2000000,'output_tokens':100000}
        append(agy_path,notice('Gemini 4 Carbon (High)'),{**agy_call,'step_index':1},
               notice('Gemini 4 Argon (Medium)'),{**agy_call,'step_index':2},
               notice('Gemini 4 Hydrogen (Low)'),
               {'type':'USER_INPUT','status':'DONE','content':'<USER_REQUEST>\n'+notice('Gemini 4 Argon (High)')['content']+'\n</USER_REQUEST>'},
               {**agy_call,'step_index':3})
        dump(root/'grok/project/sid/usage.json',{'sessionId':'sid','turns':[{'turnNumber':1,'endedAt':iso(now-60),'modelUsage':{'grok-4.6':{'inputTokens':100000,'cachedReadTokens':50000,'outputTokens':10000,'costUsdTicks':1850000000}}}]})
        dump(root/'droid/project/sid.settings.json',{'model':'claude-sonnet-5-5','tokenUsage':{'inputTokens':1000000,'outputTokens':100000}})
        logs=root/'logs';logs.mkdir();droid_log=logs/'droid-log-single.log'
        droid_row={'inputTokens':1000000,'totalInputTokens':3000000,'outputTokens':100000,'reasoningTokens':50000,
                   'cacheReadInputTokens':1000000,'cachedTokensWritten':1000000,'upstreamResponseId':'droid-r1',
                   'tags':{'modelId':'claude-fable-5.2','sessionId':'droid-sid','isByok':'false','private':'PRIVATE-BODY'}}
        log_line=lambda row,at:f'[{iso(at)}] INFO: [Agent] Streaming result | Context: '+json.dumps(row)+'\n'
        droid_log.write_text(f'[{iso(now-18000)}] INFO: startup\n'+log_line(droid_row,now-60)+
            log_line({**droid_row,'upstreamResponseId':'byok','tags':{**droid_row['tags'],'isByok':'true'}},now-60)+
            log_line({**droid_row,'upstreamResponseId':'old'},now-36000))
        (logs/'droid-log-single.log.1').write_text(log_line(droid_row,now-60))
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
        # Upgrade only the two changed parsers; keep the large Codex index intact.
        with sqlite3.connect(directory/'ledger-v2.sqlite') as index:
            index.execute("delete from parser_versions where source in ('agy','droid')")
            index.execute('insert into files values(?,?,?,?,?,?,?,?)',('droid',str(root/'droid/project/sid.settings.json'),0,0,0,0,0,json.dumps({'partial':True,'baseline_at':iso(now)})))
        subprocess.run(command,check=True,capture_output=True)
        upgraded=json.loads((directory/'snapshot.json').read_text())
        assert upgraded['metrics']['changed_files']==3,upgraded['metrics']
        assert not next(r for r in upgraded['rows'] if r['source']=='droid')['partial']
        subprocess.run(command,check=True,capture_output=True)
        second=json.loads((directory/'snapshot.json').read_text());assert second['metrics']['bytes_read']==0,second['metrics']
        assert 'PRIVATE-BODY' not in (directory/'ledger-v2.sqlite').read_bytes().decode(errors='ignore')
        with sync_playwright() as pw, ExitStack() as browser_stack:
            options={'headless':True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):options['executable_path']=os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser=pw.chromium.launch(**options);browser_stack.callback(browser.close)
            port=free_port();env=node_env(root,port,'127.0.0.0/8');env['SESSIONDOCK_USAGE_PYTHON']=sys.executable
            (root/'agy-native').mkdir()
            env.update(SESSIONDOCK_AGY_HOME=str(root/'agy'),SESSIONDOCK_AGY_ROOT=str(root/'agy-native'),SESSIONDOCK_DROID_ROOT=str(root/'droid'))
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
            expect(page.locator('[data-source="agy"] tbody .quota-api')).to_have_text('$8.80整轮 ≈ $58.67')
            droid_cell=page.locator('[data-source="droid"] tbody .quota-api')
            expect(droid_cell).to_have_text('$22.20–$28.20整轮 ≈ $148.00–$188.00')
            assert '5 分钟至 1 小时' in droid_cell.get_attribute('title')
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
            expect(droid_cell).to_contain_text('$22.20–$28.20')
            with droid_log.open('a') as stream:
                stream.write(log_line({**droid_row,'upstreamResponseId':'droid-r2','cacheWriteTtl':'1h'},now-50))
            refresh();expect(droid_cell).to_contain_text('$50.40–$56.40')
            # Older diagnostics cannot establish a complete monthly numerator.
            saved=json.loads((state/'quotas/droid.json').read_text());saved['buckets'][0].update(window='monthly',starts_at=iso(now+7200-30*86400))
            dump(state/'quotas/droid.json',saved);refresh()
            expect(droid_cell).to_contain_text('≥ ')
            assert '日志未覆盖周期起点' in droid_cell.get_attribute('title')
            expect(droid_cell).not_to_contain_text('整轮')
            quotas(state,now,used=15)
            # Factory logs Core recovery separately from request metering, including
            # when rotation puts the transition and the request in different files.
            droid_quota = state/'quotas/droid.json'
            weekly = [
                {'id':f'{group.lower()}:weekly','group':group,'window':'weekly',
                 'remaining_percent':90,'resets_at':iso(now+7200)}
                for group in ('Standard', 'Core')
            ]
            dump(droid_quota, {'source':'droid','captured_at':iso(now),'buckets':weekly})
            recovery = {'sessionId':'droid-sid','factory.process.instance_id':'droid-process',
                        'slot':'main','source':'detector_recovery','reason':'overage_reactive_402',
                        'previousModelId':'claude-fable-5.2','modelId':'kimi-k3'}
            transition = lambda row, at: f'[{iso(at)}] INFO: [Model-Router] Model transition | Context: '+json.dumps(row)+'\n'
            (logs/'droid-log-single.log.2').write_text(
                f'[{iso(now-7*86400)}] INFO: startup\n'+transition(recovery, now-90))
            kimi = {**droid_row,'upstreamResponseId':'kimi-core','factory.process.instance_id':'droid-process',
                    'inputTokens':100000,'outputTokens':10000,'cacheReadInputTokens':100000,'cachedTokensWritten':0,
                    'tags':{**droid_row['tags'],'modelId':'kimi-k3'}}
            with droid_log.open('a') as stream:
                stream.write(log_line(kimi, now-80))
                stream.write(log_line({**kimi,'upstreamResponseId':'kimi-extra','usagePool':'extra'}, now-70))
                stream.write(log_line({**kimi,'upstreamResponseId':'kimi-byok','tags':{**kimi['tags'],'isByok':'true'}}, now-65))
            refresh()
            droid_weekly = page.locator('[data-source="droid"] tbody tr')
            core = droid_weekly.filter(has_text='Core').locator('.quota-api')
            standard = droid_weekly.filter(has_text='Standard').locator('.quota-api')
            for url, mode in [(base,'node'),(f'http://127.0.0.1:{hub.port}','hub')]:
                page.goto(url+'/quotas.html')
                page.get_by_role('tab', name='Droid').click()
                expect(core).to_have_text('$0.48整轮 ≈ $4.80')
                expect(core).to_have_attribute('title', re.compile(r'kimi-k3：\$0\.48，1 次调用'))
                expect(standard).not_to_have_attribute('title', re.compile('kimi-k3|无法确认'))
                page.screenshot(path=str(args.output/f'{mode}-droid-weekly.png'),full_page=True)
            page.goto(base+'/quotas.html')
            with droid_log.open('a') as stream:
                stream.write(log_line({**kimi,'upstreamResponseId':'kimi-other-process',
                                       'factory.process.instance_id':'other-process'}, now-52))
                stream.write(transition({**recovery,'source':'manual','reason':None}, now-48))
                stream.write(log_line({**kimi,'upstreamResponseId':'kimi-manual'}, now-45))
            refresh()
            expect(core).to_have_text('≥ $0.48记录不完整')
            expect(core).to_have_attribute('title', re.compile('无法确认请求所属额度组'))
            quotas(state,now,used=15)
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
            record=codex('unknown',now-25,'unpriced-model')[1]
            prefix=codex('unknown',now-25,'unpriced-model')[0];append(cp,prefix)
            raw=json.dumps(record)
            with cp.open('a') as f:f.write(raw[:40])
            refresh();expect(current).to_contain_text('$492.69')
            with cp.open('a') as f:f.write(raw[40:]+'\n')
            append(cp,codex('unknown',now-25,'unpriced-model')[2])
            refresh();expect(current.locator('small:not(.quota-api-note)')).to_have_count(0)
            assert '缺少型号、价格或完整计量' in current.get_attribute('title')
            # Correct a price without re-reading native logs.
            prices=json.loads((directory/'prices.json').read_text())
            prices['models']['unpriced-model']=prices['models']['gpt-6.1-sol'];dump(directory/'prices.json',prices)
            refresh();expect(current).to_contain_text('$509.41')
            # Stop at first 100%; a later Credits-funded call is excluded.
            append(cp,quota_point(now-15,now+7200,100),*codex('paid',now-10))
            quotas(state,now,used=100);refresh();expect(current).to_contain_text('$509.41')
            expect(current.locator('small:not(.quota-api-note)')).to_have_text('整轮 ≈ $509.41')
            assert '后续 Credits' in current.get_attribute('title')
            # A different session with only modern request metering remains usable.
            append(root/'codex/native-only.jsonl',{'type':'session_meta','timestamp':iso(now-10),'payload':{'id':'native-only'}},
                   *codex('native-only',now-10)[:-1])
            refresh();expect(page.locator('[data-source="codex"] tbody .quota-api').nth(1)).to_contain_text('$542.85')
            quotas(state,now,used=0);refresh();expect(current.locator('small:not(.quota-api-note)')).to_have_count(0)
            quotas(state,now-1800,used=30);refresh();expect(current.locator('small:not(.quota-api-note)')).to_have_count(0)
            # Missing retired history is explicit and must not yield a full estimate.
            quotas(state,now,used=100);retired.unlink();refresh()
            assert '迁出历史缺失' in current.get_attribute('title');expect(current.locator('small:not(.quota-api-note)')).to_have_count(0)
            page.screenshot(path=str(args.output/'partial.png'),full_page=True)
            # The background timer updates without clicking refresh.
            result=page.evaluate("fetch('api/usage/settings',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({enabled:true,interval_seconds:1,sources:{}})}).then(r=>r.ok)")
            assert result
            append(root/'claude/project/session.jsonl',{'type':'assistant','sessionId':'claude-usage','timestamp':iso(now-5),
                'message':{'id':'timer','model':'claude-sonnet-5-5','usage':{'input_tokens':1000000,'output_tokens':0}}})
            wait_snapshot(lambda r:any(v['source']=='claude' and v['api_usd'] is not None and abs(v['api_usd']-8.35)<.001 for v in r['rows']))
            page.reload();expect(page.locator('[data-source="claude"] tbody .quota-api')).to_contain_text('$8.35')
            assert not errors,errors
            price_upgrade(browser,args.binary.resolve(),args.output)
            price_sync_case(browser,args.binary.resolve(),args.output)
            print('PASS: original node/Hub tables, eight adapters, reference checkpoints, ownership, compaction, long context, exhaustion, duplicate/tail, partial/stale/zero, periodic refresh, wide/narrow light/dark')


if __name__=='__main__':main()
