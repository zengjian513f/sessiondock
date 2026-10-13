#!/usr/bin/env python3
"""Chromium research links; --labdesk-dir also exercises the real ledger API against private SQLite."""
import argparse
from contextlib import ExitStack
import importlib.util
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
from urllib.parse import parse_qs, urlencode, urlsplit

from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN
from nest_tree_browser import open_item_menu

SID = '15badaae-951a-4fa2-b3fc-d6a7790c95ff'


def synthetic():
    steps = [{'experiment_id':'E1', 'step_id':f'S{i:03}', 'name':f'步骤 {i} <img src=x onerror=alert(1)> · 分析长标题和跨机器样本',
              'status':'已完成', 'position':i} for i in range(1,8)]
    exps = [dict(experiment_id=f'E{i}', project='m1.05pq', name=f'稳定性验证 {i} · Long experiment title / 中英文样本覆盖和参数敏感性分析',
                 status='研究中', available=True, direct=i<8, steps=steps if i==1 else []) for i in range(1,9)]
    exps[-1]['steps'] = [dict(experiment_id='E8', step_id='S001', name='漏登记实验仍能反查的步骤', status='计划', position=1)]
    def report(rid, available=True):
        return dict(report_id=rid, title=f'研报 {rid} · 中英文长标题 Research report on stability', available=available,
                    project='m1.05pq', experiments=[{'experiment_id':e['experiment_id'],'project':e['project']} for e in exps])
    return dict(status='ok', experiments=exps, reports=[report('R1'),report('R404',False)], related_reports=[report('R2')])


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--screenshots',type=Path)
    parser.add_argument('--labdesk-dir',type=Path)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-research-') as temp, sync_playwright() as pw, ExitStack() as stack:
        root=Path(temp); corpus=Corpus(root/'node')
        for sid in (SID,'empty','unregistered'):
            corpus.put(sid,'codex',[codex_row('session_meta',{'id':sid,'cwd':'/synthetic/research','timestamp':'2026-10-13T01:00:00Z'}),
                                   codex_message('user','会话研究关联 / Research participation '+sid)],[])
        node=SimpleNamespace(name='Friendly display name', nid='a'*32, port=free_port(), token=TOKEN)
        (corpus.root/'ids').mkdir(); (corpus.root/'ids/node-id').write_text(node.nid+'\n')
        (corpus.root/'proc').mkdir()
        env=node_env(corpus.root,node.port,'127.0.0.0/8'); env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
        local=stack.enter_context(isolated_server(corpus,args.binary,extra_env=env))[0]
        hubroot=root/'hub';hubroot.mkdir();hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node]);hub.start();stack.callback(hub.stop)
        base=f'http://127.0.0.1:{hub.port}'
        client=None
        if args.labdesk_dir:
            sys.path.insert(0,str(args.labdesk_dir))
            spec=importlib.util.spec_from_file_location('research_fixture',args.labdesk_dir/'tests/session_research_fixtures.py')
            fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
            client=stack.enter_context(fixture.research_client('codex:'+SID,node.nid))
        browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
        context=browser.new_context(viewport={'width':1280,'height':1000});stack.callback(context.close)
        page=context.new_page();errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
        mode={'value':'normal'}; requests=[]; held=[]
        def route_research(route):
            query=parse_qs(urlsplit(route.request.url).query);requests.append(query)
            assert query['node']==[node.nid],query
            if mode['value']=='hold':
                held.append(route);return
            if mode['value']=='failure':
                route.fulfill(status=503,json={'error':'synthetic outage'});return
            if mode['value']=='authentication':
                route.fulfill(status=401,json={'error':'authentication_required'});return
            if mode['value']=='malformed':
                route.fulfill(json={});return
            params={k:v[0] for k,v in query.items()}
            if mode['value']=='mismatch':params['node']='b'*32
            if mode['value']=='unmapped':params['node']='c'*32
            if client:
                response=client.get('/api/session-research',params=params)
                route.fulfill(status=response.status_code,body=response.content,content_type='application/json');return
            status={'mismatch':'host_mismatch','unmapped':'node_unmapped'}.get(mode['value'])
            if not status and query['session_uid']!=['codex:'+SID]:status='ok' if query['session_uid']==['codex:empty'] else 'unregistered'
            route.fulfill(json=dict(status=status,experiments=[],reports=[],related_reports=[]) if status else synthetic())
        context.route('**/labdesk/api/session-research?*',route_research)
        context.route('**/labdesk/experiments?*',lambda r:r.fulfill(content_type='text/html',body='<h1>实验步骤目标页</h1>'))
        context.route('**/labdesk/reports?*',lambda r:r.fulfill(content_type='text/html',body='<h1>研报目标页</h1>'))
        def shot(name):
            if args.screenshots:
                args.screenshots.mkdir(parents=True,exist_ok=True);page.screenshot(path=str(args.screenshots/(name+'.png')),full_page=True)
        def open_info(sid=SID, url=base):
            uid=corpus.uid(sid) if url==local else scoped(node.nid,corpus.uid(sid))
            page.goto(url+'/session-info.html?'+urlencode({'uid':uid}))
            expect(page.locator('#info-summary')).to_be_visible()
        page.goto(base);page.wait_for_function('S.sessions.length === 3')
        open_item_menu(page,scoped(node.nid,corpus.uid(SID)));page.locator('#item-menu [data-act="info"]').click()
        expect(page.locator('#research-count')).to_have_text('实验 8 · 步骤 8 · 研报 2')
        assert requests[-1]['session_uid']==['codex:'+SID]
        expect(page.locator('#research-content img')).to_have_count(0)
        expect(page.locator('#research-content')).not_to_contain_text('Private step body')
        expect(page.locator('#research-content a').filter(has_text='R404')).to_have_count(0)
        expect(page.locator('#research-content')).to_contain_text('研报记录不可用')
        expect(page.locator('a').filter(has_text='S008')).to_have_count(0)
        first=page.locator('.si-research-experiment').first
        first.locator(':scope > summary').click();expect(first.locator('.si-research-steps')).to_be_hidden()
        first.locator(':scope > summary').click();expect(first.locator('.si-research-steps')).to_be_visible()
        first.locator('.si-research-overflow > summary').click()
        expect(first.locator('a').filter(has_text='S007')).to_be_visible()
        page.locator('#research-content > .si-research-overflow > summary').click()
        expect(page.locator('#research-content')).to_contain_text('通过步骤关联')
        expect(page.locator('a').filter(has_text='漏登记实验仍能反查')).to_be_visible()
        page.locator('.si-research-related > summary').click()
        expect(page.locator('.si-research-related')).to_contain_text('不计入上方研报数量')
        for title,params,path in [('E1 ·',{'eid':['E1'],'project':['m1.05pq']},'/labdesk/experiments'),
                                  ('S001 · 步骤',{'eid':['E1'],'project':['m1.05pq'],'step':['S001']},'/labdesk/experiments'),
                                  ('R1 ·',{'rid':['R1'],'project':['m1.05pq']},'/labdesk/reports')]:
            with page.expect_popup() as pop:page.locator('#research-content a').filter(has_text=title).first.click()
            popup=pop.value;popup.wait_for_load_state();assert urlsplit(popup.url).path==path;assert parse_qs(urlsplit(popup.url).query)==params;popup.close()
        shot('expanded')
        open_info()
        for width,theme in [(1280,'light'),(1280,'dark'),(390,'light'),(390,'dark')]:
            page.set_viewport_size({'width':width,'height':1000});page.evaluate('theme=>document.documentElement.dataset.theme=theme',theme)
            expect(page.locator('#research-count')).to_have_text('实验 8 · 步骤 8 · 研报 2')
            assert page.evaluate('document.documentElement.scrollWidth <= innerWidth'),(width,theme)
            shot(f'research-{width}-{theme}')
        mode['value']='failure';page.locator('#info-refresh').click()
        expect(page.locator('#research-status')).to_contain_text('以下保留上次结果');shot('refresh-error')
        mode['value']='normal';page.locator('#research-retry').click();expect(page.locator('#research-retry')).to_be_hidden()
        expect(page.locator('#research-status')).to_have_text('')
        for state,text in [('mismatch','机器与当前会话不同'),('unmapped','尚未配置'),('authentication','认证不可用'),('malformed','无效结果')]:
            mode['value']=state;open_info();expect(page.locator('#research-status')).to_contain_text(text);expect(page.locator('#research-content a')).to_have_count(0);shot(state)
        mode['value']='hold';open_info();expect(page.locator('#research-status')).to_have_text('正在读取研究关联…')
        expect(page.locator('#info-predecessors')).to_be_visible();shot('loading')
        mode['value']='normal';route_research(held.pop());expect(page.locator('#research-count')).to_have_text('实验 8 · 步骤 8 · 研报 2')
        mode['value']='failure';open_info();expect(page.locator('#research-status')).to_contain_text('暂时无法加载')
        expect(page.locator('#info-predecessors')).to_be_visible();shot('first-load-error')
        mode['value']='normal';page.locator('#research-retry').click();expect(page.locator('#research-count')).to_have_text('实验 8 · 步骤 8 · 研报 2')
        open_info('empty');expect(page.locator('#research-status')).to_contain_text('尚无已登记');shot('empty')
        open_info('unregistered');expect(page.locator('#research-status')).to_contain_text('尚未登记');shot('unregistered')
        open_info(url=local);expect(page.locator('#research-count')).to_have_text('实验 8 · 步骤 8 · 研报 2')
        assert not errors,errors
        print('PASS research browser: node + Hub, native identity, ledger query, missing experiment link, exact steps, direct/contextual reports, links, folds, wide/mobile themes, errors/retry and empty states',flush=True)


if __name__=='__main__':main()
