#!/usr/bin/env python3
"""Click whole-group clone in the real page; synthetic native DBs and histories.
Covers branch siblings, archived dependencies, revert generations, grandchildren,
subagents, code-mode references, metadata, idempotent retry and process restart.
"""
import argparse
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


def prepare(root):
    cwd = root / 'workspace'
    cwd.mkdir(parents=True)
    corpus = fixture(root, cwd=cwd)
    home = root / 'codex'
    with sqlite3.connect(home / 'state_5.sqlite') as db:
        db.executescript('''
        CREATE TABLE threads(id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, name TEXT,
          is_pinned INTEGER, project_id TEXT, archived INTEGER, source TEXT);
        CREATE TABLE thread_spawn_edges(parent_thread_id TEXT, child_thread_id TEXT PRIMARY KEY, status TEXT);
        CREATE TABLE thread_dynamic_tools(thread_id TEXT, position INTEGER, name TEXT,
          description TEXT, input_schema TEXT, PRIMARY KEY(thread_id, position));
        CREATE TABLE projects(id TEXT PRIMARY KEY, name TEXT);
        ''')
        db.execute('INSERT INTO projects VALUES (?,?)', ('external-project', 'Keep this project'))
        for key in ('parent', 'a', 'b', 'grandchild', 'parent-agent', 'a-agent', 'unrelated'):
            p = corpus.paths[key]
            meta = json.loads(p.read_text().splitlines()[0])['payload']
            db.execute('INSERT INTO threads VALUES (?,?,?,?,?,?,?)', (meta['id'], str(p),
                'Native ' + key, key == 'a', 'external-project', key == 'b', json.dumps(meta.get('source', 'cli'))))
        for parent, child in ((1,6),(2,7)):
            db.execute('INSERT INTO thread_spawn_edges VALUES (?,?,?)', (ident(parent), ident(child), 'completed'))
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
             'input':'const r = await tools.multi_agent_v1__wait_agent({targets: ["'+ident(6)+'"]}); text(r);'},
            {'type':'custom_tool_call_output','call_id':'call-code','output':[{'type':'input_text',
             'text':json.dumps({'status':{ident(6):{'completed':ident(6)}},'timed_out':False})}]}), 11):
            stream.write(json.dumps({'type':'response_item','ordinal':ordinal,'payload':payload})+'\n')
    # External tool identities are not native thread relationships.
    with corpus.paths['a-agent'].open('a') as stream:
        for payload in (
            {'type':'custom_tool_call','call_id':'external-agent','name':'external_service','input':'lookup'},
            {'type':'custom_tool_call_output','call_id':'external-agent',
             'output':json.dumps({'agent_id':ident(999)})}):
            stream.write(json.dumps({'type':'response_item','payload':payload})+'\n')
    state = root / 'state'
    state.mkdir()
    (state / 'session-metadata.json').write_text(json.dumps({'schema_version':1,'revision':1,'sessions':{
        corpus.uid('a'):{'starred':True,'starred_at':1000,'fork_parent_visible':True},
        corpus.uid('grandchild'):{'nest_parent':{'source':'codex','sid':ident(1)}}}}))
    proc = root / 'proc'
    proc.mkdir()
    return corpus


def native_rows(home):
    result = {}
    for name in ('state_5.sqlite','thread_history_1.sqlite'):
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
        before={str(p):p.read_bytes() for p in corpus.paths.values()}
        db_before=native_rows(corpus.root/'codex')
        node=SimpleNamespace(name='test',nid='a'*32,port=free_port(),token=TOKEN)
        (corpus.root/'ids').mkdir();(corpus.root/'ids/node-id').write_text(node.nid+'\n')
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
                    base,opener=stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',extra_env=env))
                    if hub is None:hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node])
                    hub.start();stack.callback(hub.stop)
                    context=browser.new_context(service_workers='block',viewport={'width':1280,'height':900});stack.callback(context.close)
                    page=context.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                    selected=scoped(node.nid,corpus.uid('a'))
                    if restart and interrupted:
                        recovered=json.loads(interrupted.read_text())
                        assert recovered['phase']=='failed', recovered['error']
                        assert all(not (corpus.root/'codex'/f['relative']).exists() for f in recovered['staged']['files'])
                        print('PASS restart compensates an interrupted publication with a committed native database',flush=True)
                    if not restart:
                        page.locator(f'#side .item[data-uid="{selected}"]').click(button='right')
                        page.locator('#item-menu [data-act="clone"]').click()
                        dialog=page.locator('#clone-group-dialog')
                        expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=20000)
                        expect(dialog.locator('.clone-status')).to_contain_text('整组 6 个会话')
                        expect(dialog.locator('.clone-members li')).to_have_count(6)
                        for text in ('Branch B','Common ancestor','Parent agent','A agent','Fork of revert'):
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
                        assert saved['phase']=='complete'
                        ids=saved['plan']['identities']['threads']
                        page.locator('#a-view-switch').click()
                        page.locator(f'#session-view-menu button[data-agent="{ids[ident(7)]}"]').click()
                        expect(page.locator('#msgs')).to_contain_text('A agent answer')
                        expect(page.locator('#msgs')).to_contain_text('Identity reference check '+ident(6))
                        cloned_file=next(f for f in saved['staged']['files'] if f['source']==str(corpus.paths['a-agent']))
                        records=[json.loads(l)['payload'] for l in (corpus.root/'codex'/cloned_file['relative']).read_text().splitlines()]
                        code=next(r for r in records if r.get('type')=='custom_tool_call')
                        output=next(r for r in records if r.get('type')=='custom_tool_call_output')
                        assert ids[ident(6)] in code['input'] and ident(6) not in code['input']
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
                    next_op=json.loads(journal.read_text())
                    if not restart:
                        interrupted=journal
                        next_op['phase']='publishing'
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
                        failed=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/clone',data={'uid':selected,'operation_id':next_id})
                        assert not failed.ok,failed.text()
                        saved=json.loads(journal.read_text());assert saved['phase']=='failed',saved.get('error')
                        assert all(not (corpus.root/'codex'/f['relative']).exists() for f in saved['staged']['files'])
                        with sqlite3.connect(corpus.root/'codex/state_5.sqlite') as db:
                            assert not db.execute('SELECT id FROM threads WHERE id=?',(saved['plan']['identities']['threads'][ident(1)],)).fetchall()
                        assert all(Path(p).read_bytes()==raw for p,raw in before.items())
                        print('PASS second-database failure rolls back only owned rows/files and retains source plus existing clone',flush=True)

        finally:
            browser.close()
            if hub:hub.stop()

if __name__=='__main__':main()
