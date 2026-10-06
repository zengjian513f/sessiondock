#!/usr/bin/env python3
"""Click whole-group clone in the real page; synthetic native DBs and histories.
Covers branch siblings, archived dependencies, revert generations, grandchildren,
subagents, code-mode references, metadata, idempotent retry and process restart.
"""

import argparse
from transfer_ui import select_target, seed_names
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, isolated_server, get_json
from session_transfer_browser import fixture, ident
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN


def prepare_confirmed(page, node, operation):
    """Explicit execution preparation for publication fault injection only."""
    from node_auth_suite import GOOD
    reply = page.request.post(f'http://127.0.0.1:{node.port}/api/session/transfer/manifest',
                              headers=GOOD, data={'operation_id': operation})
    assert reply.ok, reply.text()


def prepare(root):
    cwd = root / 'workspace'
    cwd.mkdir(parents=True)
    corpus = fixture(root, cwd=cwd)
    home = root / 'codex'
    with sqlite3.connect(home / 'state_5.sqlite') as db:
        db.executescript('''
        CREATE TABLE threads(id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, name TEXT,
          is_pinned INTEGER, project_id TEXT REFERENCES projects(id), archived INTEGER, source TEXT,
          thread_section_id TEXT REFERENCES thread_sections(id));
        CREATE TABLE thread_spawn_edges(parent_thread_id TEXT, child_thread_id TEXT PRIMARY KEY, status TEXT);
        CREATE TABLE thread_dynamic_tools(thread_id TEXT, position INTEGER, name TEXT,
          description TEXT, input_schema TEXT, PRIMARY KEY(thread_id, position));
        CREATE TABLE projects(id TEXT PRIMARY KEY, name TEXT);
        CREATE TABLE project_roots(project_id TEXT REFERENCES projects(id), position INTEGER, path TEXT,
          PRIMARY KEY(project_id,position));
        CREATE TABLE thread_sections(id TEXT PRIMARY KEY, name TEXT);
        ''')
        db.execute('INSERT INTO projects VALUES (?,?)', ('external-project', 'Keep this project'))
        db.execute('INSERT INTO projects VALUES (?,?)', ('unrelated-project', 'Do not transfer'))
        db.execute('INSERT INTO project_roots VALUES (?,?,?)', ('external-project', 0, str(cwd)))
        db.execute('INSERT INTO thread_sections VALUES (?,?)', ('pinned-section', 'Pinned'))
        for key in ('parent', 'a', 'b', 'grandchild', 'parent-agent', 'a-agent', 'unrelated'):
            p = corpus.paths[key]
            meta = json.loads(p.read_text().splitlines()[0])['payload']
            db.execute('INSERT INTO threads VALUES (?,?,?,?,?,?,?,?)', (meta['id'], str(p),
                'Native ' + key, key == 'a', 'external-project', key == 'b', json.dumps(meta.get('source', 'cli')), 'pinned-section'))
        for parent, child in ((1,6),(2,7)):
            db.execute('INSERT INTO thread_spawn_edges VALUES (?,?,?)', (ident(parent), ident(child), 'completed'))
    with sqlite3.connect(home / 'state_5.sqlite') as db:
        for table,category in (('thread_attachments','attachment_type'),('thread_artifacts','artifact_type')):
            db.execute(f'CREATE TABLE {table}(id TEXT PRIMARY KEY,thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,'
                       f'{category} TEXT NOT NULL,identity_key TEXT NOT NULL,payload TEXT NOT NULL,created_at INTEGER NOT NULL,'
                       f'UNIQUE(thread_id,{category},identity_key))')
            for number in (2,8):
                # Same client identity is legal on different threads. Its payload is opaque.
                db.execute(f'INSERT INTO {table} VALUES (?,?,?,?,?,?)',
                           (f'{table}-{number}',ident(number),'test.note','shared-external-key',
                            json.dumps({'id':ident(number),'thread_id':ident(number),'text':'Literal '+ident(number)},indent=2),1234))
    with sqlite3.connect(home / 'thread_history_1.sqlite') as db:
        db.executescript('''
        CREATE TABLE thread_turns(thread_id TEXT, turn_id TEXT, rollout_ordinal INTEGER,
          rollout_byte_offset INTEGER, rollout_end_ordinal INTEGER, rollout_end_byte_offset INTEGER,
          first_user_item_id TEXT, final_agent_item_id TEXT, PRIMARY KEY(thread_id,turn_id));
        CREATE TABLE thread_items(thread_id TEXT,turn_id TEXT,item_id TEXT,item_json TEXT,
          PRIMARY KEY(thread_id,turn_id,item_id));
        CREATE TABLE thread_history_projection_state(thread_id TEXT PRIMARY KEY,
          next_rollout_byte_offset INTEGER,next_rollout_ordinal INTEGER);
        ''')
        db.execute('INSERT INTO thread_items VALUES (?,?,?,?)', (ident(7),ident(20),'exec-projection-only',json.dumps({
            'type':'collabAgentToolCall','id':'exec-projection-only','senderThreadId':ident(7),
            'receiverThreadIds':[ident(6)],'agentsStates':{ident(6):{'status':'completed','message':ident(6)}}})))
    # Realtime history has its own projection and identities, including references
    # to ordinary agent turns/items. Speech text is data, never an ID reference.
    realtime = [
        {'id':'voice-start','realtime_session_id':'voice-session','type':'realtime_session_started'},
        {'id':'voice-text','realtime_session_id':'voice-session','type':'transcript_segment',
         'role':'assistant','text':'Literal voice-session '+ident(2)},
        {'id':'voice-promoted','realtime_session_id':'voice-session','type':'bem_item_promoted',
         'turn_id':ident(20),'item_id':'exec-projection-only','presentation':{'type':'inline_markdown'}},
        {'id':'voice-end','realtime_session_id':'voice-session','type':'realtime_session_closed','outcome':'ended'},
    ]
    with sqlite3.connect(home / 'thread_history_1.sqlite') as db, corpus.paths['a'].open('a') as stream:
        db.execute('CREATE TABLE thread_realtime_items(thread_id TEXT NOT NULL,item_id TEXT NOT NULL, '
                   'rollout_ordinal INTEGER NOT NULL,created_at_ms INTEGER NOT NULL,item_type TEXT NOT NULL, '
                   'item_json TEXT NOT NULL,PRIMARY KEY(thread_id,item_id),UNIQUE(thread_id,rollout_ordinal))')
        for ordinal,item in enumerate(realtime,100):
            stream.write(json.dumps({'type':'realtime_item','payload':item})+'\n')
            db.execute('INSERT INTO thread_realtime_items VALUES (?,?,?,?,?,?)',
                       (ident(2),item['id'],ordinal,1234,item['type'],json.dumps(item)))
        # A projection-only record must also get a stable identity mapping.
        item={'id':'voice-only','realtime_session_id':'voice-only-session','type':'realtime_session_closed','outcome':'failed'}
        db.execute('INSERT INTO thread_realtime_items VALUES (?,?,?,?,?,?)',
                   (ident(2),item['id'],104,1235,item['type'],json.dumps(item)))
    with sqlite3.connect(home / 'goals_1.sqlite') as db:
        db.executescript("""
        CREATE TABLE thread_goals (
          thread_id TEXT PRIMARY KEY NOT NULL, goal_id TEXT NOT NULL, objective TEXT NOT NULL,
          status TEXT NOT NULL CHECK(status IN ('active','paused','blocked','usage_limited','budget_limited','complete')),
          token_budget INTEGER, tokens_used INTEGER NOT NULL DEFAULT 0, time_used_seconds INTEGER NOT NULL DEFAULT 0,
          created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL);
        CREATE TABLE thread_goal_continuation_deferrals (
          thread_id TEXT PRIMARY KEY NOT NULL REFERENCES thread_goals(thread_id) ON DELETE CASCADE);
        """)
        for key,status,budget in (('parent','paused',10000),('a','blocked',None),('unrelated','complete',5000)):
            sid=json.loads(corpus.paths[key].read_text().splitlines()[0])['payload']['id']
            db.execute('INSERT INTO thread_goals VALUES (?,?,?,?,?,?,?,?,?)',
                (sid,ident(850 if key!='unrelated' else 851),'Goal literal '+sid,status,budget,123,45,1000,2000))
        db.execute('INSERT INTO thread_goal_continuation_deferrals VALUES (?)',(ident(2),))
    with corpus.paths['a'].open('a') as stream:
        stream.write(json.dumps({'type':'event_msg','payload':{'type':'thread_goal_updated','threadId':ident(2),
            'goal':{'threadId':ident(2),'objective':'Goal literal '+ident(2),'status':'blocked','tokenBudget':None,
                    'tokensUsed':123,'timeUsedSeconds':45,'createdAt':1000,'updatedAt':2000}}})+'\n')
    # Current Codex serializes native function outputs as content arrays too.
    # Preserve each item's envelope, ordinary text and non-text content.
    agent = corpus.paths['a-agent']
    rows = [json.loads(line) for line in agent.read_text().splitlines()]
    for row in rows:
        p = row['payload']
        if p.get('type') == 'function_call_output':
            p['output'] = [{'type':'input_text','text':p['output']},
                           {'type':'input_text','text':'Tool completed successfully'},
                           {'type':'input_image','image_url':'data:image/png;base64,c3ludGhldGlj','detail':'low'}]
    agent.write_text(''.join(json.dumps(row)+'\n' for row in rows))
    # A code-mode call with literal targets and native JSON result. Text content
    # in the result deliberately contains a UUID and must remain untouched.
    with corpus.paths['a-agent'].open('a') as stream:
        for ordinal, payload in enumerate((
            {'type':'custom_tool_call','id':'ctc-probe','call_id':'call-code','name':'exec',
             'input':'// Historical command mentions '+ident(6)+'\n'
                     'await tools.exec_command({cmd: "echo '+ident(6)+'"}); '
                     'const r = await tools.wait_agent({targets: ["'+ident(6)+'"]}); text(r);'},
            {'type':'custom_tool_call_output','call_id':'call-code','output':[{'type':'input_text',
             'text':json.dumps({'status':{ident(6):{'completed':ident(6)}},'timed_out':False})}]}), 11):
            stream.write(json.dumps({'type':'response_item','ordinal':ordinal,'payload':payload})+'\n')
    # External tool identities are not native thread relationships.
    with corpus.paths['a-agent'].open('a') as stream:
        for payload in (
            {'type':'custom_tool_call','call_id':'external-agent','name':'external_service','input':'lookup'},
            {'type':'custom_tool_call_output','call_id':'external-agent',
             'output':json.dumps({'agent_id':ident(999),'text':'Historical UUID '+ident(6)})}):
            stream.write(json.dumps({'type':'response_item','payload':payload})+'\n')
    state = root / 'state'
    state.mkdir()
    (state / 'session-metadata.json').write_text(json.dumps({'schema_version':1,'revision':1,'sessions':{
        corpus.uid('a'):{'starred':True,'starred_at':1000,'fork_parent_visible':True,'group':'待办'},
        corpus.uid('grandchild'):{'nest_parent':{'source':'codex','sid':ident(1)}}}}))
    proc = root / 'proc'
    proc.mkdir()
    return corpus


def native_rows(home):
    result = {}
    for name in ('state_5.sqlite','thread_history_1.sqlite','goals_1.sqlite'):
        with sqlite3.connect(home / name) as db:
            for table, in db.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall():
                result[name,table] = db.execute('SELECT * FROM '+table).fetchall()
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-clone-browser-') as tmp, sync_playwright() as pw:
        root=Path(tmp)
        corpus=prepare(root/'node')
        custom_title, name_raw, names_before=seed_names(corpus.root/'codex',ident(2))
        before={str(p):p.read_bytes() for p in corpus.paths.values()}
        db_before=native_rows(corpus.root/'codex')
        node=SimpleNamespace(name='test',nid='a'*32,port=free_port(),token=TOKEN)
        (corpus.root/'ids').mkdir();(corpus.root/'ids/node-id').write_text(node.nid+'\n')
        destination=prepare(root/'destination')
        destination_node=SimpleNamespace(name='destination',nid='b'*32,port=free_port(),token=TOKEN)
        (destination.root/'ids').mkdir();(destination.root/'ids/node-id').write_text(destination_node.nid+'\n')
        destination_before={str(p):p.read_bytes() for p in destination.paths.values()}
        destination_db_before=native_rows(destination.root/'codex')
        hubroot=root/'hub';hubroot.mkdir()
        hub=None
        launch={'headless':True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):launch['executable_path']=os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser=pw.chromium.launch(**launch)
        operation=None; completed=None; interrupted=None
        try:
            for restart in (False,True):
                with ExitStack() as stack:
                    env=node_env(corpus.root,node.port,'127.0.0.0/8')
                    env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
                    env['SESSIONDOCK_CODEX_INDEX']=str(corpus.root/'codex/session_index.jsonl')
                    base,opener=stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',extra_env=env))
                    target_env=node_env(destination.root,destination_node.port,'127.0.0.0/8')
                    target_env['SESSIONDOCK_PROC_ROOT']=str(destination.root/'proc')
                    stack.enter_context(isolated_server(destination,args.binary,state_dir=destination.root/'state',extra_env=target_env))
                    if hub is None:hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node,destination_node])
                    hub.start();stack.callback(hub.stop)
                    context=browser.new_context(service_workers='block',has_touch=True,viewport={'width':1280,'height':900});stack.callback(context.close)
                    page=context.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                    selected=scoped(node.nid,corpus.uid('a'))
                    if restart and interrupted:
                        recovered=json.loads(interrupted.read_text())
                        assert recovered['phase']=='failed', recovered['error']
                        assert all(not (corpus.root/'codex'/f['relative']).exists() for f in recovered['staged']['files'])
                        print('PASS restart compensates an interrupted publication with a committed native database',flush=True)
                    if not restart:
                        page.locator(f'#side .item[data-uid="{selected}"]').click()
                        expect(page.locator('#a-clone-group svg use')).to_have_attribute('href','#i-transfer')
                        expect(page.locator('#a-clone-group')).to_have_attribute('aria-label','移动 / 复制整组')
                        # Native title hints must retain input guards without a
                        # custom overlay, modal or clone-plan request.
                        plans=[]
                        page.on('request',lambda r:plans.append(r.url) if r.url.endswith('/api/session/clone/plan') else None)
                        running={'value':True}
                        def live_reply(route):
                            response=route.fetch()
                            data=response.json();data['uids']=[selected] if running['value'] else []
                            route.fulfill(response=response,json=data)
                        page.route('**/api/live*',live_reply)
                        page.evaluate('async () => await pollLive(true)')
                        action=page.locator('#a-clone-group')
                        expect(action).to_have_attribute('aria-disabled','true')
                        if not action.is_visible():page.locator('#a-more').click()
                        action.hover()
                        tip=page.locator('#control-unavailable-tooltip')
                        expect(action).to_have_attribute('title','会话正在运行，请先停止后再移动或复制整组。')
                        expect(tip).to_have_count(0)
                        assert float(action.evaluate("b => getComputedStyle(b).opacity"))==.55
                        page.mouse.move(0,0)
                        action.hover()
                        expect(tip).to_have_count(0)
                        action.click(force=True)
                        expect(page.locator('dialog[open]')).to_have_count(0)
                        action.focus();page.keyboard.press('Enter');expect(tip).to_have_count(0)
                        page.keyboard.press('Escape');expect(tip).to_have_count(0)
                        page.locator(f'#side .item[data-uid="{selected}"]').click(button='right')
                        menu_action=page.locator('#item-menu [data-act="clone"]')
                        expect(menu_action).to_have_attribute('aria-disabled','true')
                        menu_action.hover();assert '正在运行' in menu_action.get_attribute('title')
                        menu_action.tap(force=True);expect(tip).to_have_count(0)
                        assert not plans,'running action must not open a plan'
                        running['value']=False
                        page.evaluate('async () => await pollLive(true)')
                        expect(action).not_to_have_attribute('aria-disabled','true')
                        expect(action).to_have_attribute('title','移动 / 复制整组')
                        expect(menu_action).not_to_have_attribute('aria-disabled','true')
                        expect(tip).to_have_count(0)
                        page.keyboard.press('Escape')
                        # Offline machines and empty agent-type filters use the
                        # same native hint; no toggle/solo action or alert is sent.
                        page.evaluate("id => {Nodes.list.find(n=>n.id===id).online=false; renderNodes();}",destination_node.nid)
                        machine=page.locator(f'#node-chips button[data-node="{destination_node.nid}"]')
                        expect(machine).to_have_attribute('aria-disabled','true')
                        off=page.evaluate('[...Nodes.off]')
                        machine.hover();assert '离线' in machine.get_attribute('title')
                        machine.tap(force=True);expect(tip).to_have_count(0)
                        machine.click(button='right',force=True)
                        assert page.evaluate('[...Nodes.off]')==off
                        agent=page.locator('#chips button[data-source="claude"]')
                        expect(agent).to_have_attribute('aria-disabled','true')
                        sources=page.evaluate('[...S.off]')
                        agent.hover();assert '没有会话' in agent.get_attribute('title')
                        agent.tap(force=True);expect(tip).to_have_count(0)
                        agent.focus();page.keyboard.press('Enter')
                        assert page.evaluate('[...S.off]')==sources
                        expect(page.locator('dialog[open]')).to_have_count(0)
                        page.keyboard.press('Escape');expect(tip).to_have_count(0)
                        page.evaluate("id => {Nodes.list.find(n=>n.id===id).online=true; renderNodes();}",destination_node.nid)
                        expect(machine).not_to_have_attribute('aria-disabled','true')
                        print('PASS native title hints without custom overlays: running action/menu, offline machine, empty agent; hover/touch/keyboard and live recovery',flush=True)

                        # Both a flat toolbar and its overflow menu use the same
                        # icon + managed label, never a naked wrapping text node.
                        for width in (1280,440,390,320):
                            page.set_viewport_size({'width':width,'height':900})
                            page.evaluate(('() => { ' + 'showMobileDetail(); layoutSessionHead()' + ' }'))
                            button=page.locator('#a-clone-group')
                            assert button.evaluate("b => [...b.childNodes].filter(n => n.nodeType===Node.TEXT_NODE).every(n => !n.textContent.trim())")
                            if button.is_visible():
                                bounds=button.bounding_box()
                                assert bounds['height']<=36 and bounds['width']<=36,bounds
                            else:
                                page.locator('#a-more').click()
                                expect(button).to_be_visible()
                                expect(button.locator('span')).to_have_text('移动 / 复制整组')
                                page.locator('#a-more').click()
                        page.set_viewport_size({'width':440,'height':900})
                        page.evaluate(('() => { ' + 'showMobileDetail(); layoutSessionHead()' + ' }'))
                        # Let the viewport resize and its header observer finish before opening the menu.
                        page.evaluate('() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
                        page.screenshot(path='target/transfer-toolbar-mobile.png')
                        button=page.locator('#a-clone-group')
                        if not button.is_visible():page.locator('#a-more').click()
                        button.click()
                        expect(page.locator('#clone-group-dialog')).to_be_visible()
                        page.locator('#clone-group-dialog .clone-cancel').click()
                        page.set_viewport_size({'width':1280,'height':900})
                        page.evaluate("Nodes.machines.push({id:'offline-picker-fixture',name:'Offline fixture',online:false})")
                        page.locator(f'#side .item[data-uid="{selected}"]').click(button='right')
                        page.locator('#item-menu [data-act="clone"]').click()
                        dialog=page.locator('#clone-group-dialog')
                        expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=20000)
                        expect(dialog.locator('#transfer-source')).to_be_disabled()
                        expect(dialog.locator('#transfer-source')).to_have_value(node.name)
                        expect(dialog.locator('#transfer-source')).to_have_attribute('type','text')
                        assert float(dialog.locator('#transfer-source').evaluate('e => getComputedStyle(e).opacity')) < 1
                        expect(dialog.locator('.clone-status')).to_contain_text('整组 6 个会话')
                        expect(dialog.locator('.transfer-selected .transfer-session-name')).to_have_text(custom_title)
                        expect(dialog.locator('.clone-members tbody tr')).to_have_count(6)
                        expect(dialog.locator('.clone-members thead')).to_contain_text('历史文件')
                        expect(dialog.locator('.transfer-selected .transfer-number').first).to_have_text('2')
                        expect(dialog.locator('.transfer-identity')).to_be_hidden()
                        expect(dialog.locator('#transfer-target')).to_have_js_property('value', node.nid)
                        target_picker=dialog.locator('#transfer-target')
                        target_options=dialog.locator('#transfer-target-options')
                        target_picker.click()
                        expect(target_options).to_be_visible()
                        expect(target_picker).to_have_attribute('aria-expanded','true')
                        expect(dialog.locator('select')).to_have_count(0)
                        expect(target_options.locator('[data-value="offline-picker-fixture"]')).to_be_disabled()
                        expect(target_options.locator('[aria-selected="true"]')).to_have_attribute('data-value',node.nid)
                        # All machines are ordinary rows, including the first;
                        # there is no title, divider or Android native select.
                        assert target_options.locator('[role="option"]').evaluate_all("rows => rows.every(row => {const s=getComputedStyle(row); return s.borderTopWidth==='0px' && s.borderBottomWidth==='0px';})")
                        page.keyboard.press('End')
                        expect(target_options.locator(f'[data-value="{destination_node.nid}"]')).to_be_focused()
                        page.keyboard.press('ArrowDown')
                        expect(target_options.locator(f'[data-value="{node.nid}"]')).to_be_focused()
                        page.keyboard.press('Escape')
                        expect(target_options).to_be_hidden()
                        expect(dialog).to_be_visible()
                        expect(target_picker).to_be_focused()
                        target_picker.press('ArrowDown')
                        page.keyboard.press('End');page.keyboard.press('Enter')
                        expect(target_picker).to_have_js_property('value',destination_node.nid)
                        expect(target_options).to_be_hidden()
                        select_target(dialog,node.nid)
                        target_picker.click()
                        dialog.locator('#transfer-title').click()
                        expect(target_options).to_be_hidden()
                        expect(target_picker).to_have_attribute('aria-expanded','false')
                        target_picker.click();target_picker.click()
                        expect(target_options).to_be_hidden()
                        requests=[]
                        page.on('request',lambda r:requests.append(r.url) if r.url.endswith('/api/session/clone') else None)
                        dialog.locator('.transfer-segments label').nth(1).click()
                        expect(dialog.locator('.clone-confirm')).to_be_disabled()
                        expect(dialog.locator('.transfer-notice')).to_contain_text('需要选择另一台机器')
                        select_target(dialog, destination_node.nid)
                        expect(dialog.locator('.transfer-identity')).to_be_visible()
                        expect(dialog.locator('#transfer-new-ids')).not_to_be_checked()
                        dialog.locator('#transfer-new-ids').check()
                        expect(dialog.locator('.clone-confirm')).to_be_enabled()
                        dialog.locator('.clone-confirm').click()
                        expect(dialog.locator('.transfer-error')).to_contain_text('未配置回收站')
                        expect(dialog.locator('.transfer-error')).to_be_visible()
                        expect(dialog.locator('#transfer-source')).to_be_disabled()
                        expect(dialog.locator('#transfer-source')).to_have_value(node.name)
                        expect(dialog.locator('#transfer-source')).to_have_attribute('type','text')
                        assert float(dialog.locator('#transfer-source').evaluate('e => getComputedStyle(e).opacity')) < 1
                        dialog.locator('.transfer-segments label').nth(0).click()
                        expect(dialog.locator('#transfer-new-ids')).to_be_checked()
                        dialog.locator('#transfer-new-ids').uncheck()
                        assert not requests,'unsupported selections must never execute a local clone'
                        page.evaluate("document.documentElement.dataset.theme='dark'")
                        Path('target').mkdir(exist_ok=True)
                        dialog.screenshot(path='target/transfer-panel-desktop.png')
                        page.set_viewport_size({'width':390,'height':844})
                        for width in (1280,390,320):
                            page.set_viewport_size({'width':width,'height':844})
                            source_box=dialog.locator('#transfer-source').bounding_box()
                            target_box=dialog.locator('#transfer-target').bounding_box()
                            assert abs(source_box['y']-target_box['y']) < 1,(source_box,target_box)
                            assert abs(source_box['width']-target_box['width']) < 1,(source_box,target_box)
                            assert source_box['x']+source_box['width'] <= target_box['x']
                            expect(dialog.locator('.transfer-identity-help, .transfer-scope-note, .transfer-footer-note, .transfer-origin')).to_have_count(0)
                        page.set_viewport_size({'width':390,'height':844})
                        box=dialog.bounding_box()
                        assert box['x']>=0 and box['x']+box['width']<=391,box
                        assert box['y']>=0 and box['y']+box['height']<=845,box
                        expect(dialog.locator('.clone-cancel')).to_be_visible()
                        dialog.screenshot(path='target/transfer-panel-mobile.png')
                        for scale in ('1','1.4'):
                            page.evaluate("scale => document.documentElement.style.setProperty('--interface-scale',scale)",scale)
                            target_picker.tap()
                            expect(target_options).to_be_visible()
                            menu_box=dialog.locator('.transfer-target-menu').bounding_box()
                            assert menu_box['x']>=0 and menu_box['x']+menu_box['width']<=391,menu_box
                            assert menu_box['y']>=0 and menu_box['y']+menu_box['height']<=845,menu_box
                            page.screenshot(path=f'target/transfer-target-mobile-{scale}.png')
                            target_options.locator(f'[data-value="{node.nid}"]').tap()
                            expect(target_picker).to_have_js_property('value',node.nid)
                            expect(target_options).to_be_hidden()
                        page.evaluate("document.documentElement.style.removeProperty('--interface-scale')")
                        select_target(dialog, node.nid)
                        expect(dialog.locator('.transfer-identity')).to_be_hidden()
                        expect(dialog.locator('.clone-confirm')).to_be_enabled()
                        page.set_viewport_size({'width':1280,'height':900})
                        assert native_rows(destination.root/'codex')==destination_db_before
                        assert all(Path(p).read_bytes()==content for p,content in destination_before.items())
                        print('PASS transfer table, machine/mode/identity controls, unsupported-action guard and mobile layout',flush=True)
                        for text in (custom_title,'Common ancestor','Parent agent','A agent'):
                            expect(dialog).to_contain_text(text)
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone') and r.request.method=='POST') as reply:
                            dialog.locator('.clone-confirm').click()
                        response=reply.value
                        assert response.ok,response.text()
                        completed=response.json();operation=completed['operation_id']
                        expect(dialog).to_have_count(0)
                        page.wait_for_function('(uid)=>S.sel===uid',arg=completed['target_uid'])
                        expect(page.locator('#msgs')).to_contain_text('Branch A current')
                        expect(page.locator('#msgs')).to_contain_text('Branch A old')
                        expect(page.locator('#msgs')).to_contain_text('Common ancestor 原文 '+ident(1))
                        expect(page.locator('#msgs')).not_to_contain_text('Discarded old branch tail')
                        # The confirmation really published files, not a staging-only receipt.
                        saved=json.loads((corpus.root/'state/transfers'/operation/'operation.json').read_text())
                        goal_map=saved['plan']['identities']
                        expected_name=name_raw.replace(('"id" : "'+ident(2)+'"').encode(),('"id" : "'+goal_map['threads'][ident(2)]+'"').encode())
                        assert (corpus.root/'codex/session_index.jsonl').read_bytes()==names_before+expected_name
                        expect(page.locator('.dtitle h2')).to_contain_text(custom_title)
                        with sqlite3.connect(corpus.root/'codex/goals_1.sqlite') as db:
                            goal=db.execute('SELECT goal_id,objective,status,token_budget,tokens_used,time_used_seconds FROM thread_goals WHERE thread_id=?',
                                (goal_map['threads'][ident(2)],)).fetchone()
                            assert goal==(goal_map['records'][ident(850)],'Goal literal '+ident(2),'blocked',None,123,45),goal
                            assert goal[0]!=ident(850)
                            assert db.execute('SELECT count(*) FROM thread_goal_continuation_deferrals WHERE thread_id=?',(goal_map['threads'][ident(2)],)).fetchone()[0]==1
                        rollout=next(f for f in saved['staged']['files'] if f['source']==str(corpus.paths['a']))
                        events=[json.loads(line) for line in (corpus.root/'codex'/rollout['relative']).read_text().splitlines()]
                        event=next(row['payload'] for row in events if row.get('payload',{}).get('type')=='thread_goal_updated')
                        assert event['threadId']==event['goal']['threadId']==goal_map['threads'][ident(2)]
                        assert event['goal']['objective']=='Goal literal '+ident(2)
                        print('PASS cloned native goals preserve status/budget/accounting/deferrals and remap goal and event identities',flush=True)
                        with sqlite3.connect(corpus.root/'codex/thread_history_1.sqlite') as db:
                            projected=db.execute('SELECT item_id,rollout_ordinal,created_at_ms,item_json FROM thread_realtime_items WHERE thread_id=? ORDER BY rollout_ordinal',
                                                 (goal_map['threads'][ident(2)],)).fetchall()
                        assert len(projected)==5,projected
                        for index,(item_id,ordinal,created,item_json) in enumerate(projected):
                            item=json.loads(item_json)
                            assert item_id==item['id'] and item_id in goal_map['records'].values()
                            assert ordinal==100+index and created==(1235 if index==4 else 1234)
                            assert item['realtime_session_id']==goal_map['records']['voice-only-session' if index==4 else 'voice-session']
                        assert json.loads(projected[1][3])['text']=='Literal voice-session '+ident(2)
                        promoted=json.loads(projected[2][3])
                        assert promoted['turn_id']==goal_map['turns'][ident(20)]
                        assert promoted['item_id']==goal_map['records']['exec-projection-only']
                        assert [row['payload'] for row in events if row['type']=='realtime_item']==[json.loads(row[3]) for row in projected[:4]]
                        print('PASS realtime rollout and projections share rewritten session/item/turn identities; text and ordering preserved',flush=True)
                        with sqlite3.connect(corpus.root/'codex/state_5.sqlite') as db:
                            for table in ('thread_attachments','thread_artifacts'):
                                source=db.execute(f'SELECT * FROM {table} WHERE thread_id=?',(ident(2),)).fetchone()
                                copied=db.execute(f'SELECT * FROM {table} WHERE thread_id=?',(goal_map['threads'][ident(2)],)).fetchone()
                                assert copied==(goal_map['records'][source[0]],goal_map['threads'][ident(2)],*source[2:])
                                assert copied[0]!=source[0]
                        print('PASS attachment and legacy artifact membership IDs remapped; opaque payload bytes and client keys preserved',flush=True)
                        assert saved['phase']=='complete'
                        starred_copy = next(row for row in saved['metadata_after'].values() if row.get('starred'))
                        assert starred_copy['group'] == '待办'
                        persisted = json.loads((corpus.root / 'state/session-metadata.json').read_text())
                        assert any(row.get('group') == '待办'
                                   for uid, row in persisted['sessions'].items() if uid in saved['metadata_after'])
                        ids=saved['plan']['identities']['threads']
                        page.locator('#a-view-switch').click()
                        page.locator(f'#session-view-menu button[data-agent="{ids[ident(7)]}"]').click()
                        expect(page.locator('#msgs')).to_contain_text('A agent answer')
                        expect(page.locator('#msgs')).to_contain_text('Identity reference check '+ident(6))
                        cloned_file=next(f for f in saved['staged']['files'] if f['source']==str(corpus.paths['a-agent']))
                        records=[json.loads(l)['payload'] for l in (corpus.root/'codex'/cloned_file['relative']).read_text().splitlines()]
                        code=next(r for r in records if r.get('type')=='custom_tool_call')
                        output=next(r for r in records if r.get('type')=='custom_tool_call_output')
                        assert 'targets: ["'+ids[ident(6)]+'"]' in code['input']
                        assert '// Historical command mentions '+ident(6) in code['input']
                        assert 'cmd: "echo '+ident(6)+'"' in code['input']
                        assert json.loads(output['output'][0]['text'])['status']=={ids[ident(6)]:{'completed':ident(6)}}
                        native_outputs=[r['output'] for r in records if r.get('type')=='function_call_output']
                        assert len(native_outputs)==2
                        assert json.loads(native_outputs[0][0]['text'])=={'agent_id':ids[ident(6)]}
                        assert json.loads(native_outputs[1][0]['text'])=={'status':{ids[ident(6)]:{'completed':ident(6)}}}
                        for items in native_outputs:
                            assert items[1]=={'type':'input_text','text':'Tool completed successfully'}
                            assert items[2]=={'type':'input_image','image_url':'data:image/png;base64,c3ludGhldGlj','detail':'low'}
                        assert set(ids).isdisjoint(ids.values())
                        with sqlite3.connect(corpus.root/'codex/state_5.sqlite') as db:
                            row=db.execute('SELECT name,is_pinned,project_id FROM threads WHERE id=?',(ids[ident(2)],)).fetchone()
                            assert row==('Native a',1,'external-project'),row
                            assert db.execute('SELECT parent_thread_id FROM thread_spawn_edges WHERE child_thread_id=?',(ids[ident(7)],)).fetchone()==(ids[ident(2)],)
                            receipt=json.loads(db.execute('SELECT receipt FROM _sessiondock_clone_journal WHERE operation_id=?',(operation,)).fetchone()[0])
                            shared={'projects','project_roots','thread_sections'}
                            assert all(not t['rows'] for t in receipt['inserted']['tables'] if t['name'] in shared)
                            projects=next(t['rows'] for t in receipt['planned']['tables'] if t['name']=='projects')
                            assert [r['id'] for r in projects]==['external-project']
                        print('PASS Chromium/Hub confirms complex group clone, opens new history and agent; native metadata and code-mode references',flush=True)
                    else:
                        # Retry after process restart must return the same group and
                        # must not replace any already-continued clone contents.
                        saved=json.loads((corpus.root/'state/transfers'/operation/'operation.json').read_text())
                        target=corpus.root/'codex'/saved['staged']['files'][0]['relative']
                        with target.open('ab') as stream:stream.write(b'{"type":"unknown","payload":{"text":"continued"}}\n')
                        continued=target.read_bytes()
                    reply=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/clone',data={'uid':selected,'operation_id':operation})
                    assert reply.ok,reply.text()
                    assert reply.json()['target_uid']==completed['target_uid']
                    if restart:assert target.read_bytes()==continued
                    assert all(Path(p).read_bytes()==raw for p,raw in before.items())
                    after=native_rows(corpus.root/'codex')
                    for key,rows in db_before.items():assert all(row in after[key] for row in rows),(key,'source DB row changed')
                    if restart:print('PASS persisted retry after restart preserves continued clone, original files and original database rows',flush=True)
                    # A second operation is deliberately interrupted after one
                    # database commit and one atomic file publication. The receipt
                    # is written in the same SQLite transaction as the inserted rows.
                    planned=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/clone/plan',data={'uid':selected})
                    assert planned.ok,planned.text()
                    next_id=planned.json()['operation_id']
                    journal=corpus.root/'state/transfers'/next_id/'operation.json'
                    prepare_confirmed(page,node,next_id)
                    next_op=json.loads(journal.read_text())
                    if not restart:
                        interrupted=journal
                        next_op['phase']='publishing'
                        # Reproduce the legacy journal shape, which did not
                        # include shared association rows, for upgrade recovery.
                        for database in next_op['rewritten']['databases']:
                            database['tables']=[t for t in database['tables'] if t['name'] not in {'projects','project_roots','thread_sections'}]
                        journal.write_text(json.dumps(next_op))
                        f=next_op['staged']['files'][0]
                        target=corpus.root/'codex'/f['relative'];target.parent.mkdir(parents=True,exist_ok=True)
                        marker=target.with_name('.sessiondock-'+next_id+'-'+target.name+'.pending')
                        marker.write_bytes((journal.parent/'staging'/f['relative']).read_bytes())
                        os.link(marker,target)
                        database=next_op['rewritten']['databases'][0]
                        with sqlite3.connect(database['path']) as db:
                            db.execute('CREATE TABLE IF NOT EXISTS _sessiondock_clone_journal(operation_id TEXT PRIMARY KEY,receipt TEXT NOT NULL)')
                            for table in database['tables']:
                                columns=table['columns']
                                for row in table['rows']:
                                    db.execute('INSERT INTO '+table['name']+' ('+','.join(columns)+') VALUES ('+','.join('?' for _ in columns)+')',[row[k] for k in columns])
                            db.execute('INSERT INTO _sessiondock_clone_journal VALUES (?,?)',(next_id,json.dumps(database,ensure_ascii=False,separators=(',',':'))))
                    else:
                        # Force the second database to fail after the first commits.
                        # No replacement of existing data is permitted on rollback.
                        with sqlite3.connect(corpus.root/'codex/thread_history_1.sqlite') as db:
                            db.execute("CREATE TRIGGER clone_failure BEFORE INSERT ON thread_items BEGIN SELECT RAISE(ABORT,'synthetic clone failure'); END")
                        names_before_failure=(corpus.root/'codex/session_index.jsonl').read_bytes()
                        failed=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/clone',data={'uid':selected,'operation_id':next_id})
                        assert not failed.ok,failed.text()
                        saved=json.loads(journal.read_text());assert saved['phase']=='aborted',saved.get('error')
                        assert [p.name for p in journal.parent.iterdir()]==['operation.json']
                        assert all(not (corpus.root/'codex'/f['relative']).exists() for f in saved['staged']['files'])
                        with sqlite3.connect(corpus.root/'codex/state_5.sqlite') as db:
                            assert not db.execute('SELECT id FROM threads WHERE id=?',(saved['plan']['identities']['threads'][ident(1)],)).fetchall()
                        assert all(Path(p).read_bytes()==raw for p,raw in before.items())
                        assert (corpus.root/'codex/session_index.jsonl').read_bytes()==names_before_failure
                        print('PASS second-database failure rolls back only owned rows/files and retains source plus existing clone',flush=True)

        finally:
            browser.close()
            if hub:hub.stop()

if __name__=='__main__':main()
