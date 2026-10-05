#!/usr/bin/env python3
"""Complex-family transfer: CLI advice and persisted dynamic-tool dependencies."""

import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, Corpus, isolated_server
from session_clone_browser import prepare
from session_files_browser import fixture, uid
from session_transfer_browser import ident
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-transfer-environment-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        root=Path(tmp);source=prepare(root/'source')
        roots,claude,side,agent=fixture(root/'files');roots['codex']=str(source.root/'codex')
        metadata=source.root/'state/session-metadata.json';doc=json.loads(metadata.read_text())
        doc['sessions'][uid('claude',claude[2])]={'nest_parent':{'source':'codex','sid':ident(2)}}
        doc['sessions'][uid('grok',root/'files/grok/project'/ident(10))]={'nest_parent':{'source':'claude','sid':ident(2)}}
        metadata.write_text(json.dumps(doc))
        with sqlite3.connect(source.root/'codex/state_5.sqlite') as db:
            db.execute('ALTER TABLE thread_dynamic_tools ADD COLUMN namespace TEXT')
            db.execute('INSERT INTO thread_dynamic_tools VALUES (?,?,?,?,?,?)',(ident(2),0,'db_only','PRIVATE_DESCRIPTION','{"private":"schema"}',None))
            db.execute('INSERT INTO thread_dynamic_tools VALUES (?,?,?,?,?,?)',(ident(2),1,'call','PRIVATE_DESCRIPTION','{}','database'))
        path=source.paths['a-agent'];lines=path.read_text().splitlines();meta=json.loads(lines[0])
        meta['payload']['dynamic_tools']=[{'name':'lookup','namespace':'legacy','description':'PRIVATE_DESCRIPTION','inputSchema':{}},
            {'type':'namespace','name':'remote','description':'PRIVATE_DESCRIPTION','tools':[{'type':'function','name':'search','description':'PRIVATE_DESCRIPTION','inputSchema':{}}]},
            {'type':'function','name':'db_only','description':'PRIVATE_DESCRIPTION','inputSchema':{}}]
        lines[0]=json.dumps(meta)
        message=json.loads(lines[1]);message['payload']['content'][0]['text']+=' {"dynamic_tools":[{"name":"ordinary_text"}]}'
        lines[1]=json.dumps(message);path.write_text('\n'.join(lines)+'\n')
        nodes=[]
        for letter in ('a','b','c'):
            corpus=source if letter=='a' else Corpus(root/letter)
            if letter!='a':
                for directory in ('state','proc'):(corpus.root/directory).mkdir(parents=True)
            node=SimpleNamespace(name=letter,nid=letter*32,port=free_port(),token=TOKEN);nodes.append(node)
            (corpus.root/'ids').mkdir();(corpus.root/'ids/node-id').write_text(node.nid+'\n');(corpus.root/'trash').mkdir()
            env=node_env(corpus.root,node.port,'127.0.0.0/8');env.update({f'SESSIONDOCK_{k.upper()}_ROOT':v for k,v in roots.items()})
            env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
            stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',trash_dir=corpus.root/'trash',extra_env=env))
        hubroot=root/'hub';hubroot.mkdir();hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,nodes);hub.start();stack.callback(hub.stop)
        browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
        context=browser.new_context();stack.callback(context.close);page=context.new_page()
        good=[{'id':kind,'source':kind,'installed':True,'version':'2.5.0'} for kind in ('codex','claude','grok')]
        bad=[{'id':'codex','source':'codex','installed':True,'version':'2.4.0'},
             {'id':'claude','source':'claude','installed':False},
             {'id':'grok','source':'grok','installed':True}]
        delayed=[];mode={'delay':False,'unknown':False}
        def clients(route):
            if mode['unknown']:route.fulfill(status=503,json={'error':'fixture unavailable'});return
            if nodes[1].nid in route.request.url:
                if mode['delay']:delayed.append(route);return
                route.fulfill(json={'clients':bad});return
            route.fulfill(json={'clients':good})
        page.route('**/api/clients',clients)
        base=f'http://127.0.0.1:{hub.port}'
        page.goto(base,wait_until='networkidle')
        selected=scoped(nodes[0].nid,source.uid('a'))
        def open_dialog(known_tools=True):
            page.locator(f'#side .item[data-uid="{selected}"]').click()
            with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as result:
                page.locator('#a-clone-group').click()
            assert result.value.ok,result.value.text()
            if known_tools:assert result.value.json()['dynamic_tools']==['database.call','db_only','legacy.lookup','remote.search']
            else:assert 'dynamic_tools' not in result.value.json()
            assert 'PRIVATE_DESCRIPTION' not in result.value.text() and '"private"' not in result.value.text()
            dialog=page.locator('#clone-group-dialog');expect(dialog.locator('.clone-members tbody tr')).to_have_count(14)
            return dialog
        dialog=open_dialog();note=dialog.locator('.transfer-environment')
        expect(note).to_have_text('4 个动态工具执行器未核验')
        expect(note).to_have_attribute('title','database.call、db_only、legacy.lookup、remote.search')
        dialog.locator('#transfer-target').select_option(nodes[1].nid)
        expect(note).to_contain_text('目标 Codex 存在较旧版本')
        expect(note).to_contain_text('目标未配置可用的 Claude CLI')
        expect(note).to_contain_text('未核验 Grok 版本差异')
        expect(dialog.locator('.clone-confirm')).to_be_enabled()
        dialog.locator('.clone-cancel').click()
        mode['delay']=True;dialog=open_dialog();note=dialog.locator('.transfer-environment')
        dialog.locator('#transfer-target').select_option(nodes[1].nid)
        expect(note).to_have_text('正在核对目标 CLI…');expect(dialog.locator('.clone-confirm')).to_be_disabled()
        page.wait_for_timeout(100);assert delayed
        dialog.locator('#transfer-target').select_option(nodes[2].nid)
        expect(note).to_have_text('4 个动态工具执行器未核验')
        for request in delayed:request.fulfill(json={'clients':bad})
        page.wait_for_timeout(100)
        expect(note).to_have_text('4 个动态工具执行器未核验')
        dialog.locator('.clone-cancel').click()
        def old_plan(route):
            response=route.fetch();data=response.json();data.pop('dynamic_tools')
            route.fulfill(response=response,json=data)
        page.route('**/api/session/clone/plan',old_plan,times=1)
        mode['unknown']=True;dialog=open_dialog(False);note=dialog.locator('.transfer-environment')
        expect(note).to_contain_text('未核验目标 CLI');expect(dialog.locator('.clone-confirm')).to_be_enabled()
        expect(note).to_contain_text('未核验动态工具依赖')
        with page.expect_response(lambda r:r.url.endswith('/api/session/clone')) as copied:
            dialog.locator('.clone-confirm').click()
        assert copied.value.ok,copied.value.text()
        page.wait_for_function('(uid)=>S.sel===uid',arg=copied.value.json()['target_uid'])
        expect(page.locator('#msgs')).to_contain_text('Branch A current')
        print('PASS Chromium shows CLI absence/older/unknown versions, ignores stale target replies, lists native dynamic tools without definitions, and copies the fourteen-session family',flush=True)


if __name__=='__main__':main()
