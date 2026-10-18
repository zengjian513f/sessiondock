#!/usr/bin/env python3
"""Quota page: real nodes/Hub, fake CLIs, Claude stdin wrapper, snapshot recovery."""
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from hub_fixtures import Hub, free_port
from node_auth_fixtures import node_env, TOKEN

FAKE = r'''
import sys,json,os,time
from pathlib import Path
source=sys.argv[1]
if '--version' in sys.argv:
 print('synthetic quota CLI 1.0');sys.exit()
if 'models' in sys.argv:
 print('fixture\tFixture');sys.exit()
root=Path(os.environ['QUOTA_FIXTURE'])
if (root/'fail').exists(): sys.exit(1)
if source=='agy':
 assert sys.argv[2:]==['-p','/usage','--output-format','json'],sys.argv
 print(json.dumps({'status':'SUCCESS','num_turns':0,'command':{'name':'usage','data':{'groups':[
 {'name':'Gemini Models','buckets':[{'id':'gemini-weekly','window':'weekly','remaining_fraction':.85,'reset_time':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime(time.time()+604800))},{'id':'gemini-5h','window':'5h','remaining_fraction':0,'reset_time':'2020-10-17T03:06:57Z'}]},
 {'name':'Claude and GPT models','buckets':[{'id':'3p-5h','window':'5h','remaining_fraction':1}]}]}}}));sys.exit()
assert sys.argv[2:]==['app-server','--stdio'],sys.argv
for line in sys.stdin:
 q=json.loads(line)
 if q['method']=='initialize': print(json.dumps({'id':1,'result':{}}),flush=True)
 elif q['method']=='initialized': pass
 elif q['method']=='account/rateLimits/read':
  with (root/'calls').open('a') as f:f.write('query\n')
  while (root/'hold').exists(): time.sleep(.02)
  used=float((root/'used').read_text()) if (root/'used').exists() else 25
  print(json.dumps({'id':2,'result':{'rateLimitResetCredits':{'availableCount':3,'credits':[{'status':'available','expiresAt':int(time.time())+7200}]},'rateLimitsByLimitId':{'codex':{'credits':{'balance':'12345.89','hasCredits':True,'unlimited':False},'primary':{'usedPercent':used,'windowDurationMins':300,'resetsAt':int(time.time())+7200},'secondary':{'usedPercent':60,'windowDurationMins':10080,'resetsAt':int(time.time())+172800}},'extra':{'limitName':'Long model bucket 模型专用额度组','primary':{'windowDurationMins':60}}}}}),flush=True)
 else: raise AssertionError(q)
'''

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--binary',default=str(BINARY));ap.add_argument('--output',default='/tmp/sessiondock-quotas-browser');args=ap.parse_args()
    binary=Path(args.binary).resolve();out=Path(args.output);out.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-quotas-') as tmp, ExitStack() as stack:
        root=Path(tmp)
        for name in ('state','host','ledger','home','claude','codex','grok','proc'): (root/name).mkdir(mode=0o700)
        script=root/'fake.py';script.write_text(FAKE)
        profiles=[{'id':source+'-fixture','source':source,'executable':sys.executable,'args':[str(script),source],'env':{'QUOTA_FIXTURE':str(root),'HOME':str(root/'home')}} for source in ('codex','claude','agy')]
        config=root/'launcher.json';config.write_text(json.dumps({'schema':2,'host_binary':str(binary.parent/'ptyhost'),'host_dir':str(root/'host'),'profiles':profiles}));config.chmod(0o600)
        subprocess.run([str(binary),'--initialize-lifecycle',str(root/'ledger')],env={'PATH':'/usr/bin:/bin'},check=True,capture_output=True)
        settings=root/'home/settings.json';settings.write_text(json.dumps({'statusLine':{'type':'command','command':'cat','padding':2},'unrelated':True}))
        subprocess.run([str(binary),'quota-install',str(root/'state'),str(settings)],check=True)
        installed=settings.read_text();subprocess.run([str(binary),'quota-install',str(root/'state'),str(settings)],check=True);assert settings.read_text()==installed
        assert json.loads(installed)['unrelated'] and json.loads(installed)['statusLine']['padding']==2
        payload=json.dumps({'rate_limits':{'five_hour':{'used_percentage':12.5,'resets_at':int(time.time())+7200},'seven_day':{'used_percentage':100}},'private_fixture':'must not be cached'})
        result=subprocess.run([str(binary),'quota-statusline',str(root/'state')],input=payload,text=True,capture_output=True,check=True)
        assert result.stdout==payload
        assert 'private_fixture' not in (root/'state/quotas/claude.json').read_text()
        # A callback without limits must not turn an existing snapshot into zero usage.
        subprocess.run([str(binary),'quota-statusline',str(root/'state')],input='{}',text=True,capture_output=True,check=True)
        # Yesterday's snapshot must be replaced merely by opening the page.
        (root/'state/quotas/codex.json').write_text(json.dumps({'source':'codex','captured_at':'2020-10-17T03:02:16Z','buckets':[{'id':'codex:primary','group':'codex','window':10080,'remaining_percent':9}]}))
        (root/'hold').touch()
        port=free_port();extra=node_env(root,port,'127.0.0.0/8');extra['SESSIONDOCK_PROC_ROOT']=str(root/'proc')
        base,_=stack.enter_context(isolated_server(Corpus(root),binary,state_dir=root/'state',host_dir=root/'host',lifecycle_dir=root/'ledger',launcher_config=config,extra_env=extra))
        hubroot=root/'hub';hubroot.mkdir()
        node=SimpleNamespace(port=port,token=TOKEN,name='Quota Node 长机器名')
        other=root/'other';other.mkdir();(other/'proc').mkdir()
        otherport=free_port()
        stack.enter_context(isolated_server(Corpus(other),binary,extra_env={**node_env(other,otherport,'127.0.0.0/8'),'SESSIONDOCK_PROC_ROOT':str(other/'proc')}))
        othernode=SimpleNamespace(port=otherport,token=TOKEN,name='Empty Node')
        hub=Hub(binary.parent/'sessiondock-hub',hubroot,[node,othernode]);hub.start();stack.callback(hub.stop)
        with sync_playwright() as pw:
            options={'headless':True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):options['executable_path']=os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser=pw.chromium.launch(**options)
            context=browser.new_context(viewport={'width':1280,'height':950},service_workers='block');page=context.new_page();errors=[];page.on('pageerror',lambda e:errors.append(str(e)));page.clock.install()
            hubbase=f'http://127.0.0.1:{hub.port}'
            page.goto(hubbase);page.locator('#settings').click();page.get_by_role('tab',name='机器',exact=True).click()
            link=page.get_by_role('link',name='Agent 额度 →')
            assert link.evaluate("n => !!(document.querySelector('#client-matrix').compareDocumentPosition(n) & Node.DOCUMENT_POSITION_FOLLOWING)")
            page.screenshot(path=str(out/'settings-entry.png'))
            link.click()
            assert page.url.rstrip('/')==hubbase
            machine=page.locator('#quota-dialog')
            expect(machine).to_be_visible()
            expect(machine.get_by_role('table')).to_have_count(3)
            for name in ('Codex','Claude Code','Agy'):
                table=machine.get_by_role('table',name=name,exact=True)
                headers={'Codex':['机器','窗口','已使用','重置倒计时','Credits','重置卡','采集'],
                         'Claude Code':['机器','窗口','已使用','重置倒计时','采集'],
                         'Agy':['机器','额度组','窗口','已使用','重置倒计时','采集']}
                expect(table.get_by_role('columnheader')).to_have_text(headers[name])
                expect(table.get_by_role('rowheader',name='Empty Node',exact=True)).to_have_count(0)
            expect(machine.locator('section[data-source=claude]')).to_contain_text('12.5%')
            expect(machine.locator('section[data-source=codex]')).to_contain_text('91%')
            expect(page.locator('#quota-refresh')).to_be_disabled()
            page.screenshot(path=str(out/'cached-loading.png'))
            (root/'hold').unlink()
            expect(machine.locator('section[data-source=codex]')).to_contain_text('25%',timeout=30000)
            expect(machine.locator('section[data-source=codex]')).to_contain_text('N/A')
            expect(machine.locator('td.quota-credits')).to_have_text('12,345')
            assert machine.locator('td.quota-credits').evaluate("n => getComputedStyle(n).textAlign")=='right'
            expect(machine.locator('section[data-source=codex] [role=progressbar]').first).to_have_attribute('aria-label','Quota Node 长机器名 5 小时 已使用额度')
            assert machine.locator('section[data-source=codex] [role=progressbar]').first.get_attribute('aria-valuenow')=='25'
            expect(machine.locator('section[data-source=agy] .quota-remaining b')).to_have_text(['15%','100%','0%'])
            expect(machine.locator('section[data-source=codex]')).to_contain_text('3 张')
            expect(machine.locator('section[data-source=agy]')).to_contain_text('15%')
            expect(machine.locator('section[data-source=agy]')).to_contain_text('已到期')
            expect(machine.locator('section[data-source=claude]')).to_contain_text('100%')
            expect(page.locator('#quota-refresh')).to_be_enabled()
            assert (root/'calls').read_text().count('query')==1
            deadline=machine.locator('section[data-source=codex] [data-deadline]').first
            before=deadline.inner_text();page.clock.fast_forward(65000)
            assert deadline.inner_text()!=before
            assert (root/'calls').read_text().count('query')==1, 'countdown must not query CLIs'
            # Explicit refresh still queries, then reopening queries again.
            (root/'used').write_text('26')
            page.get_by_role('button',name='刷新').click()
            expect(machine.locator('section[data-source=codex]')).to_contain_text('26%')
            expect(page.locator('#quota-refresh')).to_be_enabled()
            assert (root/'calls').read_text().count('query')==2
            (root/'used').write_text('25')
            for theme in ('light','dark'):
                page.evaluate("theme => localStorage.setItem('sessiondock.hub./.theme', JSON.stringify(theme))", theme)
                for width in (1280,390):
                    page.set_viewport_size({'width':width,'height':950});page.reload()
                    if width==390: page.locator('#header-more-btn').click()
                    page.locator('#settings').click();page.get_by_role('tab',name='机器',exact=True).click();page.locator('#quota-open').click();expect(machine.locator('section[data-source=codex]')).to_contain_text('25%')
                    expect(page.locator('#quota-refresh')).to_be_enabled()
                    for bar in machine.get_by_role('progressbar').all():
                        geometry=bar.evaluate("n => {const s=getComputedStyle(n),b=n.nextElementSibling,r=n.getBoundingClientRect(),t=b.getBoundingClientRect();return [s.width,s.height,s.borderRadius,Math.abs((r.y+r.height/2)-(t.y+t.height/2))<1,t.x>=r.right]}")
                        assert geometry==['40px','6px','3px',True,True],geometry
                    high=machine.locator('section[data-source=claude] .sdk-gauge.low')
                    expect(high).to_have_count(1)
                    assert high.locator('u').evaluate('n => getComputedStyle(n).backgroundColor')=='rgb(229, 72, 77)'
                    assert high.locator('b').evaluate('n => getComputedStyle(n).color')=='rgb(229, 72, 77)'
                    assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                    page.screenshot(path=str(out/f'{theme}-{width}.png'),full_page=True)
                    if width==390:
                        scroll=page.get_by_role('region',name='Codex 额度表，可横向滚动')
                        scroll.hover();page.mouse.wheel(1000,0)
                        expect(machine.get_by_role('table',name='Codex',exact=True).get_by_role('columnheader',name='采集')).to_be_in_viewport()
                        assert scroll.evaluate('n => n.scrollLeft > 0')
                        page.screenshot(path=str(out/f'{theme}-390-scrolled.png'),full_page=True)
            assert (root/'calls').read_text().count('query')==6, 'each opening must query once'
            (root/'fail').touch();page.get_by_role('button',name='刷新').click();expect(machine.locator('section[data-source=codex]')).to_contain_text('查询失败',timeout=30000);expect(machine.locator('section[data-source=codex]')).to_contain_text('25%');expect(machine.locator('section[data-source=agy]')).to_contain_text('15%')
            page.screenshot(path=str(out/'query-failure.png'),full_page=True)
            # A node transport failure leaves its displayed snapshot in place.
            page.route('**/api/quotas/refresh',lambda route:route.fulfill(status=503,body='offline') if (root/'ids/node-id').read_text().strip() in route.request.url else route.continue_())
            page.get_by_role('button',name='刷新').click();expect(page.locator('#quota-status')).to_contain_text('1 台读取失败');expect(machine.locator('section[data-source=codex]')).to_contain_text('25%')
            machine.get_by_role('button',name='关闭',exact=True).click()
            expect(machine).to_have_count(0)
            expect(page.locator('#settings-dialog')).to_be_visible()
            page.locator('#quota-open').click()
            expect(machine.locator('section[data-source=claude]')).to_contain_text('12.5%')
            expect(page.locator('#quota-refresh')).to_be_enabled()
            page.keyboard.press('Escape')
            expect(machine).to_have_count(0)
            # Explicitly uninstalled clients stay hidden even with stale quota snapshots.
            page.route('**/api/clients',lambda route:route.fulfill(json={'clients':[{'source':'codex','installed':True},{'source':'claude','installed':False},{'source':'agy','installed':False}]}))
            page.locator('#quota-open').click()
            expect(machine.get_by_role('table')).to_have_count(1)
            expect(machine.get_by_role('table',name='Codex',exact=True)).to_be_visible()
            page.screenshot(path=str(out/'installed-only.png'))
            expect(page.locator('#quota-refresh')).to_be_enabled()
            page.keyboard.press('Escape')
            page.unroute('**/api/clients')
            page.unroute('**/api/quotas/refresh')
            page.route('**/api/quotas/refresh',lambda route:route.fulfill(json={'agents':[{'source':'codex','buckets':[{'id':'codex:primary','group':'codex','window':300,'remaining_percent':75}]}]}))
            page.route('**/api/quotas',lambda route:route.fulfill(json={'agents':[{'source':'codex','buckets':[{'id':'codex:primary','group':'codex','window':300,'remaining_percent':75}]}]}))
            page.locator('#quota-open').click()
            expect(machine.locator('td.quota-credits')).to_have_text(['N/A'])
            expect(machine.locator('.quota-cards')).to_have_text(['N/A'])
            expect(page.locator('#quota-refresh')).to_be_enabled()
            page.keyboard.press('Escape');page.unroute('**/api/quotas');page.unroute('**/api/quotas/refresh')
            (root/'fail').unlink();(root/'used').write_text('31')
            page.goto(base+'/quotas.html');machine=page;expect(machine.locator('section[data-source=claude]')).to_contain_text('12.5%')
            expect(machine.locator('section[data-source=codex]')).to_contain_text('31%')
            expect(page.locator('#quota-refresh')).to_be_enabled()
            page.reload()
            expect(machine.locator('section[data-source=codex]')).to_contain_text('31%')
            expect(page.locator('#quota-refresh')).to_be_enabled()
            page.get_by_role('link',name='← 会话列表').click();expect(page.locator('#header-more-btn')).to_be_visible()
            assert not errors,errors
            context.close();browser.close()
    print('PASS quotas: Hub/local navigation, native structured queries, Claude wrapper, snapshots, failures, responsive themes')

if __name__=='__main__':main()
