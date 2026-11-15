#!/usr/bin/env python3
"""Agent tabs and reading position under periodic, delayed and cross-tab updates."""
import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, isolated_server
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env

NAMES={'codex':'Codex','kiro':'Kiro','agy':'Agy','claude':'Claude Code','droid':'Droid'}


def main():
    ap=argparse.ArgumentParser();ap.add_argument('--binary',type=Path,default=BINARY)
    ap.add_argument('--output',type=Path,default=Path('/tmp/sessiondock-quota-tabs'));args=ap.parse_args()
    args.output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-quota-tabs-') as tmp, ExitStack() as stack:
        root=Path(tmp);port=free_port();binary=args.binary.resolve()
        base,_=stack.enter_context(isolated_server(Corpus(root),binary,extra_env=node_env(root,port,'127.0.0.0/8')))
        hr=root/'hub';hr.mkdir();hub=Hub(binary.with_name('sessiondock-hub'),hr,[SimpleNamespace(port=port,token=TOKEN,name='Quota Node')])
        hub.start();stack.callback(hub.stop)
        now=time.time();iso=lambda t:datetime.fromtimestamp(t,timezone.utc).isoformat()
        def buckets(source):
            groups=['Gemini Models','Claude and GPT models'] if source=='agy' else ['Standard','Core'] if source=='droid' else [source]
            windows=['5h','weekly','monthly'] if source=='droid' else ['monthly'] if source=='kiro' else ['5h','weekly']
            return [{'id':f'{group}:{window}','group':group,'window':window,'remaining_percent':67,
                     'resets_at':iso(now+7200+(86400 if window=='weekly' else 0))}
                    for group in groups for window in windows]
        with sync_playwright() as pw:
            options={'headless':True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):options['executable_path']=os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser=pw.chromium.launch(**options)
            for is_hub,url in [(False,base),(True,f'http://127.0.0.1:{hub.port}')]:
                for modal in (False,True):
                    for width,theme in [(1280,'light'),(1280,'dark'),(390,'light'),(390,'dark')]:
                        state={'revision':0,'reads':0,'empty':False,'fail':False,'pending':[],'hold':is_hub}
                        context=browser.new_context(viewport={'width':width,'height':800},service_workers='block');page=context.new_page()
                        errors=[];page.on('pageerror',lambda e:errors.append(str(e)));page.clock.install()
                        fleet=[{'id':f'node-{i}','name':name,'online':True,'enabled':True} for i,name in enumerate(
                            ['Lyra','Cygnus','Pavo','Orion · 长机器名称','Vela','Cetus','远端节点'] if is_hub else ['Local Node'])]
                        def quota():
                            return {'agents':[{'source':s,'captured_at':iso(now+state['revision']),
                                'buckets':buckets(s),'error':None} for s in NAMES]}
                        def usage():
                            state['reads']+=1
                            return {'captured_at':iso(now+state['revision']),'price_version':'fixture','rows':[
                                {'source':s,**b,'start':iso(now-3600),'end':b['resets_at'],'quota_at':iso(now+state['revision']),
                                 'api_usd':120+state['revision'],'equivalent_usd':360+state['revision'],
                                 'used_percent':33,'reason':None,'models':[]} for s in NAMES for b in buckets(s)]}
                        def route_api(route):
                            path=urlsplit(route.request.url).path
                            if path.endswith('/api/nodes'):
                                route.fulfill(json={'mode':'hub' if is_hub else 'node','nodes':fleet});return
                            if path.endswith('/api/clients'):
                                route.fulfill(json={'clients':[] if state['empty'] else [{'source':s,'installed':True} for s in NAMES]});return
                            if '/api/quotas' in path:
                                if state['hold'] and '/node-6/' in path:
                                    state['pending'].append(route);return
                                route.fulfill(status=503 if state['fail'] else 200,json={} if state['fail'] else quota());return
                            if '/api/usage' in path:
                                route.fulfill(json=usage());return
                            route.continue_()
                        context.route('**/api/**',route_api)
                        if modal:
                            page.goto(url)
                            if width==390:page.locator('#header-more-btn').click()
                            page.locator('#settings').click();page.get_by_role('tab',name='机器',exact=True).click()
                            page.locator('#quota-open').click();surface=page.locator('#quota-dialog')
                        else:
                            page.goto(url+'/quotas.html');surface=page.locator('main')
                        page.evaluate('theme=>document.documentElement.dataset.theme=theme',theme)
                        tabs=surface.get_by_role('tablist',name='Agent 类型')
                        expect(tabs.get_by_role('tab')).to_have_text(list(NAMES.values()))
                        expect(surface.get_by_role('table')).to_have_count(1)
                        stem=f'{"hub" if is_hub else "node"}-{"dialog" if modal else "page"}-{width}-{theme}'
                        # Every source is reachable without scrolling past another table.
                        for source,name in NAMES.items():
                            tabs.get_by_role('tab',name=name,exact=True).click()
                            expect(surface.get_by_role('tabpanel')).to_have_count(1)
                            expect(surface.get_by_role('table',name=name,exact=True)).to_be_visible()
                            expect(surface.locator(f'#quota-panel-{source} .quota-api').first).to_contain_text('$120.00')
                            if is_hub and modal:page.screenshot(path=str(args.output/f'{stem}-{source}.png'),full_page=True)
                        page.screenshot(path=str(args.output/f'{stem}.png'),full_page=True)
                        # The keyboard follows visible tabs; hidden panels are not focusable.
                        tab=tabs.get_by_role('tab',name='Droid',exact=True);tab.focus();tab.press('Home')
                        expect(tabs.get_by_role('tab',name='Codex',exact=True)).to_be_focused()
                        page.keyboard.press('End');expect(tab).to_be_focused();expect(tab).to_have_attribute('aria-selected','true')
                        page.set_viewport_size({'width':width,'height':640 if is_hub else 300})
                        viewport=surface.locator('.quota-body');scroller=surface.locator('#quota-panel-droid .quota-scroll')
                        viewport.hover();page.mouse.wheel(0,2000)
                        page.wait_for_function("document.querySelector('.quota-body').scrollTop>20")
                        if width==390:
                            scroller.hover();page.mouse.wheel(600,0)
                            page.wait_for_function("document.querySelector('#quota-panel-droid .quota-scroll').scrollLeft>0")
                        reading_node=fleet[-2 if is_hub else 0]['id']
                        cell=surface.locator(f'#quota-panel-droid tbody[data-node="{reading_node}"] .quota-api').last
                        cell.focus();cell.evaluate('node=>window.readingCell=node')
                        before=viewport.evaluate('node=>node.scrollTop');left=scroller.evaluate('node=>node.scrollLeft')
                        page.evaluate("window.readingScroller=document.querySelector('#quota-panel-droid .quota-scroll')")
                        # The late final node expands one placeholder, without replacing readers.
                        state['hold']=False
                        for pending in state['pending']:pending.fulfill(json=quota())
                        expect(page.locator('#quota-refresh')).to_be_enabled(timeout=10000)
                        expect(cell).to_be_focused()
                        assert page.evaluate('readingCell.isConnected && readingScroller.isConnected')
                        assert viewport.evaluate('node=>node.scrollTop')>20
                        # Periodic usage polling used to clear every table and jump to the top.
                        before=viewport.evaluate('node=>node.scrollTop');left=scroller.evaluate('node=>node.scrollLeft')
                        state['revision']=1;reads=state['reads'];page.clock.run_for(61000)
                        expect(cell).to_contain_text('$121.00')
                        assert state['reads']>reads
                        assert abs(viewport.evaluate('node=>node.scrollTop')-before)<2
                        assert abs(scroller.evaluate('node=>node.scrollLeft')-left)<2
                        expect(cell).to_be_focused();expect(tab).to_have_attribute('aria-selected','true')
                        # SDK quota broadcasts update the same connected cell and deadline.
                        state['revision']=2;updated=quota()['agents'][-1]
                        updated['buckets'][-1]['remaining_percent']=42
                        updated['buckets'][-1]['resets_at']=iso(now+43200)
                        page.evaluate("data=>{const channel=new BroadcastChannel('sessiondock.quotas.v2./');channel.postMessage(data);channel.close()}",
                                      {'key':f'{fleet[-1]["id"] if is_hub else ""}:droid','quota':updated})
                        expect(surface.locator('#quota-panel-droid .quota-remaining b').last).to_have_text('58%')
                        assert page.evaluate('readingCell.isConnected && document.activeElement===readingCell')
                        # A new cycle removes its old estimate; the bottom may shrink
                        # by one line, so retain the nearest valid reading position.
                        top,maximum=viewport.evaluate('node=>[node.scrollTop,node.scrollHeight-node.clientHeight]')
                        assert abs(top-min(before,maximum))<2,(top,before,maximum)
                        before=top
                        page.clock.run_for(1000)
                        expect(surface.locator('#quota-panel-droid [data-deadline]').last).to_have_attribute('data-deadline',iso(now+43200))
                        # Each Agent retains its own vertical/horizontal reading position.
                        tabs.get_by_role('tab',name='Agy',exact=True).click();expect(surface.get_by_role('table',name='Agy',exact=True)).to_be_visible()
                        tab.click();assert abs(viewport.evaluate('node=>node.scrollTop')-before)<2
                        assert abs(scroller.evaluate('node=>node.scrollLeft')-left)<2
                        state['revision']=3;page.locator('#quota-refresh').click();expect(page.locator('#quota-refresh')).to_be_enabled()
                        expect(tab).to_have_attribute('aria-selected','true');assert viewport.evaluate('node=>node.scrollTop')>20
                        page.screenshot(path=str(args.output/f'{stem}-scrolled.png'),full_page=True)
                        state['fail']=True;page.locator('#quota-refresh').click();expect(page.locator('#quota-refresh')).to_be_enabled()
                        expect(surface.get_by_role('table',name='Droid',exact=True)).to_be_visible()
                        expect(surface.locator('#quota-panel-droid .quota-error').first).to_have_text('节点暂不可达')
                        assert viewport.evaluate('node=>node.scrollTop')>20
                        state['empty']=True;state['fail']=False;page.locator('#quota-refresh').click()
                        expect(tabs).to_be_hidden();expect(surface.get_by_role('table')).to_have_count(0)
                        if modal:
                            surface.get_by_role('button',name='关闭',exact=True).click();expect(page.locator('#settings-dialog')).to_be_visible()
                        assert not errors,errors
                        context.close()
            browser.close()
    print('PASS quota tabs: node/Hub, page/dialog, sources, keyboard, periodic/delayed/broadcast refresh, focus and scroll retention')


if __name__=='__main__':main()
