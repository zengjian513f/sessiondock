#!/usr/bin/env python3
"""Native paused-goal forks, Chromium clone, then native goal/attachment isolation.
No model turns, login material or active goals are created. Run explicitly.
"""
# run_validation: skip
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, Corpus, isolated_server
from session_move_codex_real import AppServer, MODEL, EFFORT, digest
from session_files_browser import uid
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--codex',default=str(Path.home()/'.local/bin/codex'))
    args=parser.parse_args()
    defaults=Path.home()/'.codex/config.toml';before=digest(defaults)
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-goals-') as tmp, sync_playwright() as pw:
        root=Path(tmp);corpus=Corpus(root/'node');home=corpus.root/'codex'
        for name in ('codex','claude','grok','cwd','state','proc','ids','trash'):(corpus.root/name).mkdir(parents=True)
        (home/'config.toml').write_text('[features]\ngoals=true\n')
        common={'model':MODEL,'config':{'model_reasoning_effort':EFFORT},'cwd':str(corpus.root/'cwd'),
                'approvalPolicy':'never','sandbox':'read-only'}
        expected={};attachments={}
        with AppServer(args.codex,home,corpus.root/'cwd',root/'create.log') as native:
            started=native.call('thread/start',common)
            assert started['model']==MODEL and started['reasoningEffort']==EFFORT,started
            parent=started['thread']
            result=native.call('thread/goal/set',{'threadId':parent['id'],'objective':'Native paused goal literal '+parent['id'],'status':'paused','tokenBudget':5000})
            expected[parent['id']]=result['goal']
            child=native.call('thread/fork',{**common,'threadId':parent['id'],'excludeTurns':True})['thread']
            sibling=native.call('thread/fork',{**common,'threadId':parent['id'],'excludeTurns':True})['thread']
            for index,thread in enumerate((child,sibling)):
                expected[thread['id']]=native.call('thread/goal/set',{
                    'threadId':thread['id'],'objective':'Native branch goal '+thread['id'],
                    'status':'paused','tokenBudget':6000+index*1000})['goal']
            assert all(goal and goal['status']=='paused' for goal in expected.values())
            for sid in expected:
                attachments[sid]=native.call('thread/attachment/add',{'threadId':sid,
                    'attachmentType':'test.note','identityKey':'shared-external-key',
                    'payload':{'text':'Literal '+sid,'resourceId':'external-resource','thread_id':sid}})['attachment']
        print('PASS native CLI creates three paused-goal fork snapshots without model requests',flush=True)
        paths={}
        for path in home.rglob('*.jsonl'):
            rows=[json.loads(line) for line in path.read_text().splitlines()]
            if rows and rows[0].get('type')=='session_meta':paths[rows[0]['payload']['id']]=path
        node=SimpleNamespace(name='source',nid='c'*32,port=free_port(),token=TOKEN)
        (corpus.root/'ids/node-id').write_text(node.nid+'\n')
        hubroot=root/'hub';hubroot.mkdir()
        with ExitStack() as stack:
            env=node_env(corpus.root,node.port,'127.0.0.0/8');env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
            stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',trash_dir=corpus.root/'trash',extra_env=env))
            hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node]);hub.start();stack.callback(hub.stop)
            browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
            page=browser.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
            selected=scoped(node.nid,uid('codex',paths[child['id']]))
            page.locator(f'#side .item[data-uid="{selected}"]').click()
            with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as planned:
                page.locator('#a-clone-group').click()
            assert planned.value.ok,planned.value.text()
            dialog=page.locator('#clone-group-dialog');expect(dialog.locator('.clone-members tbody tr')).to_have_count(3)
            with page.expect_response(lambda r:r.url.endswith('/api/session/clone')) as copied:
                dialog.locator('.clone-confirm').click()
            assert copied.value.ok,copied.value.text()
            op=json.loads((corpus.root/'state/transfers'/copied.value.json()['operation_id']/'operation.json').read_text())
            mapping=op['plan']['identities']['threads']
            page.wait_for_function('(id)=>S.sel===id',arg=copied.value.json()['target_uid'])
        with AppServer(args.codex,home,corpus.root/'cwd',root/'verify.log') as native:
            for old,goal in expected.items():
                cloned=native.call('thread/goal/get',{'threadId':mapping[old]})['goal']
                assert cloned=={**goal,'threadId':mapping[old]},(goal,cloned)
                assert native.call('thread/goal/get',{'threadId':old})['goal']==goal
            for old,attachment in attachments.items():
                expected_attachment={**attachment,'id':op['plan']['identities']['records'][attachment['id']]}
                assert expected_attachment['id']!=attachment['id']
                assert native.call('thread/attachment/list',{'threadId':mapping[old]})['data']==[expected_attachment]
                assert native.call('thread/attachment/list',{'threadId':old})['data']==[attachment]
            native.call('thread/attachment/remove',{'threadId':mapping[child['id']],
                'attachmentType':'test.note','identityKey':'shared-external-key'})
            assert native.call('thread/attachment/list',{'threadId':mapping[child['id']]})['data']==[]
            assert native.call('thread/attachment/list',{'threadId':child['id']})['data']==[attachments[child['id']]]
            print('PASS native attachment lists preserve opaque metadata; deleting cloned membership leaves original intact',flush=True)
            changed=native.call('thread/goal/set',{'threadId':mapping[child['id']],'status':'paused','tokenBudget':7000})['goal']
            assert changed['tokenBudget']==7000
            assert native.call('thread/goal/get',{'threadId':child['id']})['goal']==expected[child['id']]
        with sqlite3.connect(home/'goals_1.sqlite') as db:
            rows=dict(db.execute('SELECT thread_id,goal_id FROM thread_goals'))
            assert all(rows[old]!=rows[new] for old,new in mapping.items())
        assert digest(defaults)==before,'daily Codex configuration changed'
        print('PASS Chromium clones native fork goals; native reads preserve snapshots and clone-only updates leave source goals unchanged',flush=True)


if __name__=='__main__':main()
