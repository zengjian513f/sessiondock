#!/usr/bin/env python3
"""Chromium acceptance for top-level handoffs, adjacency and a real two-node Hub."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import urlencode

from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN
from nest_tree_browser import open_item_menu


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-relations-') as temp, sync_playwright() as pw, ExitStack() as stack:
        root = Path(temp)
        corpora, nodes, environments = [], [], []
        for name, sessions in [('a', ['origin', 'first', 'nested']), ('b', ['origin', 'second', 'review', 'research'])]:
            corpus = Corpus(root / name)
            for sid in sessions:
                corpus.put(sid, 'codex', [codex_row('session_meta', {'id':sid, 'cwd':'/synthetic/relations', 'timestamp':'2026-10-07T10:00:00Z'}),
                    codex_message('user', f'{name} {sid} 工作交接：执行 /synthetic/HANDOFF.md'), codex_message('assistant', '研究记录', 2)], [])
            corpus.put('subagent', 'codex', [codex_row('session_meta', {'id':'subagent', 'cwd':'/synthetic/relations',
                'source':{'subagent':{'thread_spawn':{'parent_thread_id':'origin'}}}, 'parent_thread_id':'origin', 'thread_source':'subagent'}), codex_message('user', '子代理不参与关系发现')], [], parent='origin')
            state = corpus.root / 'state'; state.mkdir()
            if name == 'a':
                (state / 'session-metadata.json').write_text(json.dumps({'schema_version':1, 'revision':1,
                    'sessions':{corpus.uid('nested'):{'nest_parent':{'source':'codex','sid':'origin'}, 'nest_initialized':True}}}))
            node = SimpleNamespace(name=name, nid=name*32, port=free_port(), token=TOKEN)
            (corpus.root / 'ids').mkdir(); (corpus.root / 'ids/node-id').write_text(node.nid+'\n')
            (corpus.root / 'proc').mkdir()
            env = node_env(corpus.root, node.port, '127.0.0.0/8'); env['SESSIONDOCK_PROC_ROOT'] = str(corpus.root/'proc')
            corpora.append(corpus); nodes.append(node); environments.append(env)
        original = {p:p.read_bytes() for c in corpora for p in c.paths.values()}
        servers = [ExitStack(), ExitStack()]
        for server in servers: stack.callback(server.close)
        locals_ = [None, None]
        def start_node(i):
            locals_[i] = servers[i].enter_context(isolated_server(corpora[i], args.binary, state_dir=corpora[i].root/'state', extra_env=environments[i]))[0]
        for i in range(2): start_node(i)
        hubroot = root/'hub'; hubroot.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, nodes)
        hub.start(); stack.callback(hub.stop)
        browser = pw.chromium.launch(headless=True); stack.callback(browser.close)
        context = browser.new_context(viewport={'width':1280,'height':900}); stack.callback(context.close)
        page = context.new_page(); errors=[]; page.on('pageerror',lambda e:errors.append(str(e)))
        base = f'http://127.0.0.1:{hub.port}'
        refs = {sid:scoped(nodes[i].nid, corpora[i].uid(sid)) for i, keys in [(0,['origin','first','nested']), (1,['second','review','research'])] for sid in keys}
        twin = scoped(nodes[1].nid, corpora[1].uid('origin'))
        def screenshot(name):
            if args.screenshots:
                args.screenshots.mkdir(parents=True,exist_ok=True)
                page.screenshot(path=str(args.screenshots/(name+'.png')),full_page=True)
        def open_info(sid):
            page.goto(base+'/session-info.html?'+urlencode({'uid':refs.get(sid,sid)}),wait_until='networkidle')
            expect(page.locator('#info-summary')).to_be_visible()
        def add(source, side='predecessor', state='confirmed', kind='continues', evidence='通过交接文档继续工作'):
            page.locator('#add-'+side).click()
            expect(page.locator('#relation-target option[value="'+refs.get(source,source)+'"]')).to_have_count(1)
            page.locator('#relation-search').fill(source if source in refs else '')
            page.locator('#relation-target').select_option(refs.get(source,source))
            page.locator('#relation-kind').select_option(kind)
            page.locator('#relation-state').select_option(state)
            page.locator('#relation-artifact').fill('/synthetic/HANDOFF.md')
            page.locator('#relation-evidence').fill(evidence)
            page.locator('#relation-save').click()
            expect(page.locator('#info-editor')).to_be_hidden()
        # Entry is reachable through the existing row context menu.
        page.goto(base,wait_until='networkidle')
        page.wait_for_function('S.sessions.length >= 6')
        row = page.locator('.item.sd-session-card').first
        expect(row).to_be_visible()
        measure = "el=>{const s=getComputedStyle(el),t=getComputedStyle(el.querySelector('.t'));return {gap:s.gap,paddingTop:s.paddingTop,paddingRight:s.paddingRight,border:s.borderLeftWidth,title:t.fontSize,line:t.lineHeight,icon:el.querySelector('.source-icon').getBoundingClientRect().width}}"
        sidebar_style = row.evaluate(measure)
        screenshot('sidebar')
        open_item_menu(page,refs['first']);page.locator('#item-menu [data-act="info"]').click()
        expect(page.locator('#info-summary')).to_contain_text('a first')
        expect(page.locator('#info-predecessors')).to_contain_text('尚无已记录')
        screenshot('empty')
        page.locator('#info-hints > summary').click()
        expect(page.locator('#hint-content')).to_contain_text('/synthetic/HANDOFF.md')
        screenshot('hints')
        page.locator('#info-hints > summary').click()
        add('origin')
        expect(page.locator('#info-predecessors')).to_contain_text('a origin')
        assert page.locator('#info-predecessors .sd-session-card').first.evaluate(measure) == sidebar_style
        open_info('second'); add('first', state='candidate')
        expect(page.locator('#info-predecessors .si-candidate')).to_have_count(2)
        page.locator('#info-predecessors .si-more > summary').first.click()
        page.locator('#info-predecessors [data-confirm]').click()
        expect(page.locator('#info-predecessors .si-candidate')).to_have_count(0)
        add('review', side='successor', kind='branches', evidence='<img src=x onerror=alert(1)> 修复交接')
        add('research', side='successor', kind='branches')
        expect(page.locator('#info-predecessors .si-relation')).to_have_count(2)
        expect(page.locator('#info-successors .si-relation')).to_have_count(2)
        expect(page.locator('#predecessor-count')).to_have_text('2 · 直接 1')
        compact = page.locator('#info-successors .si-relation').first
        expect(compact.locator('.si-relation-detail')).to_be_hidden()
        assert compact.bounding_box()['height'] <= 76
        compact.locator('.si-more > summary').click()
        compact.locator('.si-evidence > summary').click()
        expect(compact.locator('pre')).to_be_visible()
        screenshot('expanded-relation')
        compact.locator('.si-more > summary').click()
        expect(compact.locator('.si-relation-detail')).to_be_hidden()
        expect(page.locator('#info-successors img')).to_have_count(0)
        # Model-ready catalog excludes both native subagents and displayed children.
        page.locator('#add-predecessor').click()
        expect(page.locator('#relation-target option[value="'+refs['origin']+'"]')).to_have_count(1)
        expect(page.locator('#relation-target option[value="'+refs['nested']+'"]')).to_have_count(0)
        assert '子代理不参与' not in page.locator('#relation-target').inner_text()
        screenshot('editor')
        page.locator('#relation-cancel').click()
        def api_info(uid):return page.request.get(base+'/api/session/info?'+urlencode({'uid':uid})).json()
        result = api_info(refs['second'])
        assert len(result['session']['predecessors'])==1 and len(result['session']['successors'])==2
        assert {x['uid'] for x in result['ancestors']}=={refs['origin'],refs['first']}
        assert not api_info(twin)['descendants'], 'same native SID on another node must stay separate'
        rejected=page.request.post(base+'/api/session/predecessor',data={'uid':refs['second'],'predecessor_uid':refs['nested']})
        assert rejected.status==400, rejected.text()
        forged=page.request.post(locals_[1]+'/api/session/predecessor',data={'uid':corpora[1].uid('second'),'remote_predecessor':{'session_uid':corpora[0].uid('first'),'node_id':nodes[0].nid}})
        assert forged.status==403, forged.text()
        print('PASS page entry, cross-node adjacency, recursive predecessors/successors, evidence, top-level filter and same-SID isolation',flush=True)
        if args.screenshots:
            args.screenshots.mkdir(parents=True,exist_ok=True)
            for width,theme in [(1280,'light'),(1280,'dark'),(390,'light'),(390,'dark')]:
                page.set_viewport_size({'width':width,'height':900})
                page.evaluate('theme=>document.documentElement.dataset.theme=theme',theme)
                assert page.evaluate('document.documentElement.scrollWidth <= innerWidth'), (width,theme)
                for card in page.locator('.si-relation[data-uid]').all():
                    expect(card.locator('.si-relation-detail')).to_be_hidden()
                    assert card.bounding_box()['height'] <= 76, (width,theme)
                page.screenshot(path=str(args.screenshots/f'info-{width}-{theme}.png'),full_page=True)
        page.set_viewport_size({'width':1280,'height':900})
        # Navigation goes to the information page, and its conversation link opens history.
        page.locator('#info-successors .si-relation-title').filter(has_text='research').click()
        expect(page.locator('#info-summary')).to_contain_text('b research')
        expect(page.locator('#info-predecessors .si-relation')).to_have_count(3)
        page.locator('#info-open').click();expect(page.locator('#msgs')).to_contain_text('研究记录')
        # Independent node plus hub restart preserves the canonical receiving-node row.
        hub.stop()
        for server in servers:server.close()
        for i in range(2):start_node(i)
        hub.start();open_info('second')
        expect(page.locator('#info-predecessors .si-relation')).to_have_count(2)
        expect(page.locator('#info-successors .si-relation')).to_have_count(2)
        saved=json.loads((corpora[1].root/'state/session-metadata.json').read_text())
        edge=saved['sessions'][corpora[1].uid('second')]['predecessors'][0]
        assert edge['session_uid']==corpora[0].uid('first') and edge['node_id']==nodes[0].nid
        # A failed save retains the editor and succeeds when explicitly retried.
        page.route('**/api/session/predecessor',lambda route:route.fulfill(status=503,json={'error':'synthetic write failure'}),times=1)
        page.locator('#add-predecessor').click();page.locator('#relation-target').select_option(twin)
        page.locator('#relation-save').click();expect(page.locator('#relation-error')).to_contain_text('synthetic write failure')
        expect(page.locator('#info-editor')).to_be_visible();screenshot('save-error');page.locator('#relation-save').click();expect(page.locator('#info-editor')).to_be_hidden()
        expect(page.locator('#info-predecessors .si-relation')).to_have_count(3)
        twin_card=page.locator('#info-predecessors .si-relation').filter(has=page.locator('a.si-relation-title').filter(has_text='b origin'))
        twin_card.locator('.si-more > summary').click()
        twin_card.locator('[data-remove]').click();expect(page.locator('#info-predecessors .si-relation')).to_have_count(2)
        # A cycle remains finite. Relationships are user assertions, not a native fork DAG.
        open_info('origin');add('research')
        assert len(api_info(refs['origin'])['ancestors'])==3
        page.locator('#info-predecessors .si-more > summary').first.click()
        page.locator('#info-predecessors .si-relation').first.locator('[data-remove]').click()
        expect(page.locator('#info-predecessors .si-relation')).to_have_count(0)
        open_info('second')
        # Existing offline edges remain visible, never retargeted to a same-SID twin.
        servers[0].close();page.locator('#info-refresh').click()
        expect(page.locator('#info-status')).to_contain_text('部分机器')
        expect(page.locator('#info-predecessors')).to_contain_text('first')
        screenshot('offline')
        page.locator('#info-predecessors .si-more > summary').first.click()
        page.locator('#info-predecessors .si-relation').first.locator('[data-remove]').click()
        expect(page.locator('#info-predecessors .si-relation')).to_have_count(0)
        for p,content in original.items():assert p.read_bytes()==content, p
        assert not errors,errors
        print('PASS reload/restart persistence, failed-save retry, removal, finite cycles, offline references, native bytes preserved',flush=True)


if __name__=='__main__':main()
