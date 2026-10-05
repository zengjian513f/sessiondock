#!/usr/bin/env python3
"""Opt-in real-CLI migration helper; all histories and listeners are private.
The peer resumes test identities with its own controlled login. Authentication
is never transferred by the bundle or printed by this helper.
"""
# run_validation: skip
import base64
from transfer_ui import select_target
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
from contextlib import ExitStack
from types import SimpleNamespace


def move_native(base, home, provider, selected, resume_ids, peer_name, binary, new_ids=False, return_before_resume=False, codex_data=None):
    from playwright.sync_api import sync_playwright, expect
    from history_parity import Corpus, isolated_server
    from session_bundle_browser import Peer, node_call
    from hub_http_suite import Hub, free_port, scoped
    from node_auth_suite import TOKEN, node_env
    binary=binary.resolve()
    for folder in ('claude','codex','grok','state','proc','ids','trash'):
        (base/folder).mkdir(exist_ok=True)
    roots={kind:str(base/kind) for kind in ('claude','codex','grok')}
    roots[provider]=str(home if provider=='codex' else home/('projects' if provider=='claude' else 'sessions'))
    family_count=len(codex_data['pages']) if codex_data else 3
    a=SimpleNamespace(name='source',nid='a'*32,port=free_port(),token=TOKEN)
    b=SimpleNamespace(name='destination',nid='b'*32,port=free_port(),token=TOKEN)
    (base/'ids/node-id').write_text(a.nid+'\n')
    source=Corpus(base)
    peer=Peer(peer_name,base,source,roots,b,binary,native_codex=provider=='codex')
    try:
        # Seed only this test-created worktree before migration. The product
        # still verifies cwd equality and never transfers worktree content.
        entries=[]
        for path in sorted((base/'cwd').rglob('*')):
            entry={'relative':str(path.relative_to(base/'cwd')),'mode':path.lstat().st_mode & 0o777}
            if path.is_symlink():entry.update(kind='symlink',target=os.readlink(path))
            elif path.is_dir():entry['kind']='directory'
            else:entry.update(kind='file',bytes=base64.b64encode(path.read_bytes()).decode())
            entries.append(entry)
        peer.call('seed_cwd',path=str(base/'cwd'),entries=entries)
        with sync_playwright() as pw, ExitStack() as stack:
            peer.call('start')
            env=node_env(base,a.port,'127.0.0.0/8')
            env.update({f'SESSIONDOCK_{kind.upper()}_ROOT':path for kind,path in roots.items()})
            env['SESSIONDOCK_PROC_ROOT']=str(base/'proc')
            stack.enter_context(isolated_server(source,binary,state_dir=base/'state',trash_dir=base/'trash',extra_env=env))
            hubroot=base/'hub';hubroot.mkdir()
            hub=Hub(binary.with_name('sessiondock-hub'),hubroot,[a,b]);hub.start();stack.callback(hub.stop)
            browser=pw.chromium.launch(headless=True,**({'executable_path':os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']} if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE') else {}));stack.callback(browser.close)
            page=browser.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
            source_uid=scoped(a.nid,selected)
            page.locator(f'#side .item[data-uid="{source_uid}"]').click()
            page.locator('#a-clone-group').click()
            dialog=page.locator('#clone-group-dialog')
            expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=30000)
            select_target(dialog, b.nid)
            with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as planned:
                dialog.locator('.transfer-segments label').nth(1).click()
            if new_ids:
                with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as planned:
                    dialog.locator('#transfer-new-ids').check()
            assert planned.value.ok,planned.value.text()
            plan=planned.value.json();assert plan['session_count']==family_count,plan
            with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=120000) as moved:
                dialog.locator('.clone-confirm').click()
            assert moved.value.ok,moved.value.text()
            result=moved.value.json();assert result['phase']=='complete',result
            page.wait_for_function('uid => S.sel === uid',arg=result['target_uid'])
            expect(page.locator('#msgs')).to_contain_text({'claude':'def add','grok':'TRANSFER_','codex':'FAMILY_PARENT'}[provider],timeout=30000)
            operation=json.loads((base/'state/transfers'/plan['operation_id']/'operation.json').read_text())
            assert operation['phase']=='retired'
            for file in (operation['plan'] if provider=='codex' else operation['file_plan'])['files']:
                assert not os.path.lexists(file['source']),file['source']
            manifests=list((base/'trash').glob('move-*/manifest.json'));assert manifests
            trash_before={p:hashlib.sha256(p.read_bytes()).hexdigest() for p in (base/'trash').rglob('*') if p.is_file()}
            status,raw=node_call(a,'/api/session/clone/plan',{'uid':selected})
            assert status==409 and json.loads(raw)['code']=='move_recovery_required',raw
            identity_map=operation['plan']['identities']['threads'] if provider=='codex' else {key.split(':',1)[1]:value for key,value in operation['file_plan']['sessions'].items() if key.startswith(provider+':')}
            mapped=[identity_map[sid] if new_ids else sid for sid in resume_ids]
            payload={'root':str(base),'home':str(home),'cwd':str(base/'cwd'),'provider':provider,'ids':mapped}
            if codex_data:
                payload['unrelated']=peer.native_baseline
                from session_clone_service_real import remap_pages
                identities=operation['plan']['identities']
                outputs={f['source']:str(home/f['relative']) for f in operation['staged']['files']}
                payload['expected']={identity_map[old]:{
                    'pages':remap_pages(pages,identities) if new_ids else pages,
                    'metadata':codex_data['metadata'][old],
                    'parent':identity_map.get(codex_data['native'][old].get('parentThreadId')),
                    'path':outputs[codex_data['native'][old]['path']],
                } for old,pages in codex_data['pages'].items()}
            if provider=='claude':
                agent=next(m for m in operation['file_plan']['group']['members'] if m['agent'])
                publication=next(f for f in operation['file_publications'] if f['source']==agent['path'])
                payload['agent']={'id':operation['file_plan']['sessions']['claude:'+agent['sid']] if new_ids else agent['sid'],
                    'path':publication['target'],'parent':mapped[0]}
            worker=Path(__file__).resolve()
            if return_before_resume:
                page.locator('#a-clone-group').click()
                dialog=page.locator('#clone-group-dialog')
                expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=30000)
                select_target(dialog, a.nid)
                with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')):
                    dialog.locator('.transfer-segments label').nth(1).click()
                with page.expect_response(lambda r:r.url.endswith('/api/session/transfer/clone'),timeout=120000) as returned:
                    dialog.locator('.clone-confirm').click()
                assert returned.value.ok,returned.value.text()
                assert returned.value.json()['phase']=='complete'
                page.wait_for_function('uid => S.sel === uid',arg=returned.value.json()['target_uid'])
                target_local=provider+':'+result['target_uid'].split('~',1)[1]
                status,raw=node_call(b,'/api/session/clone/plan',{'uid':target_local})
                assert status==409 and json.loads(raw)['code']=='move_recovery_required',raw
                command=[sys.executable,str(worker),'--resume-worker']
            else:
                command=['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10',peer_name,
                    'python3 '+shlex.quote(str(worker))+' --resume-worker']
            remote=subprocess.run(command,input=json.dumps(payload),capture_output=True,text=True,timeout=600)
            assert remote.returncode==0,remote.stderr[-2000:]
            evidence=json.loads(remote.stdout)
            assert trash_before=={p:hashlib.sha256(p.read_bytes()).hexdigest() for p in trash_before}
            print('PASS real '+provider+' Chromium whole-family move and target native continuation',flush=True)
            return {'production_publish':True,'cross_machine':True,'mode':'move','new_ids':new_ids,
                'family_members':family_count,'round_trip':return_before_resume,'native':evidence,'checks':['native_fork','native_subagent','browser_move',
                    'source_retired_and_fenced','target_native_resume','unchanged_source_trash','daily_defaults_unchanged']}
    finally:
        peer.close()


def resume_worker():
    payload=json.load(sys.stdin);root=Path(payload['root']);home=Path(payload['home']);cwd=Path(payload['cwd'])
    assert root.parent==Path('/tmp') and root.name.startswith('sessiondock-')
    assert home.resolve().is_relative_to(root.resolve()) and cwd.resolve().is_relative_to(root.resolve())
    if payload['provider']=='codex':
        return resume_codex(payload,root,home,cwd)
    provider=payload['provider'];assert provider in ('claude','grok')
    daily=Path.home()/('.claude' if provider=='claude' else '.grok')
    settings=daily/('settings.json' if provider=='claude' else 'config.toml')
    before=hashlib.sha256(settings.read_bytes()).hexdigest() if settings.exists() else None
    auth_name='.credentials.json' if provider=='claude' else 'auth.json'
    login=home/auth_name
    owns_login=not os.path.lexists(login)
    if owns_login:login.symlink_to(daily/auth_name)
    else:assert login.is_symlink() and login.resolve()==(daily/auth_name).resolve()
    model='claude-haiku-4-5-20251001' if provider=='claude' else 'grok-4.6'
    env=dict(os.environ)
    if provider=='claude':
        env.update(HOME=str(home),CLAUDE_CONFIG_DIR=str(home))
        (home/'.claude.json').write_text(json.dumps({'hasCompletedOnboarding':True}))
    else:env['GROK_HOME']=str(home)
    continued=[]
    try:
        for index,sid in enumerate(payload['ids']):
            prompt=('What happens to the add helper when its arguments have different types? Give one example.'
                    if provider=='claude' else 'Reply exactly NATIVE_MOVED_CONTINUED.')
            if provider=='claude':
                command=[str(Path.home()/'.local/bin/claude'),'-p',prompt,'--safe-mode','--setting-sources','',
                    '--tools','','--model',model,'--effort','low','--max-turns','2','--output-format','json','--resume',sid]
            else:
                command=[str(Path.home()/'.local/bin/grok'),'-p',prompt,'-m',model,'--reasoning-effort','low',
                    '--cwd',str(cwd),'--max-turns','2','--disable-web-search','--output-format','json',
                    '--leader-socket',str(root/'resume-leader.sock'),'--resume',sid]
            original_history=next(home.glob('projects/*/'+sid+'.jsonl')) if provider=='claude' else None
            previous_lines=len(original_history.read_text().splitlines()) if original_history else 0
            log=root/f'resume-{index}.log'
            with log.open('w') as output:
                result=subprocess.run(command,env=env,cwd=cwd,stdout=output,stderr=subprocess.STDOUT,timeout=180)
            if result.returncode:
                raise RuntimeError(f'{provider} native resume failed (exit {result.returncode}); '+log.read_text()[-1200:])
            if provider=='claude':
                result=json.loads(log.read_text());assert set(result['modelUsage'])=={model}
                history=next(home.glob('projects/*/'+sid+'.jsonl'))
                actual={json.loads(line).get('message',{}).get('model') for line in history.read_text().splitlines() if json.loads(line).get('type')=='assistant'}
                assert actual=={model},actual
            else:
                summary=next(home.glob('sessions/*/'+sid+'/summary.json'))
                row=json.loads(summary.read_text());assert row['current_model_id']==model and row['reasoning_effort']=='low'
                history=summary.parent/'chat_history.jsonl'
                for line in history.read_text().splitlines():
                    item=json.loads(line)
                    if item.get('model_id'):
                        assert item['model_id'] in (model,'grok-4.6-build') and item['reasoning_effort']=='low'
            if provider=='claude':
                added=[json.loads(line) for line in history.read_text().splitlines()[previous_lines:]]
                assert any(row.get('type')=='assistant' and row.get('message',{}).get('content') for row in added)
            else:assert 'NATIVE_MOVED_CONTINUED' in history.read_text()
            continued.append(sid)
        if provider=='claude' and payload.get('agent'):
            agent=payload['agent'];history=Path(agent['path'])
            assert history.resolve().is_relative_to(root.resolve())
            parent_history=next(home.glob('projects/*/'+agent['parent']+'.jsonl'))
            previous_calls=[item for line in parent_history.read_text().splitlines() for item in json.loads(line).get('message',{}).get('content',[]) if isinstance(item,dict) and item.get('type')=='tool_use' and item.get('name')=='SendMessage']
            assert previous_calls and all(call.get('input',{}).get('to')==agent['id'] for call in previous_calls),previous_calls
            previous_results=[json.loads(line)['toolUseResult'] for line in parent_history.read_text().splitlines() if json.loads(line).get('toolUseResult',{}).get('resumedAgentId')]
            assert previous_results and all(row['resumedAgentId']==agent['id'] and row['pin']['id']==agent['id'] for row in previous_results),previous_results
            before_lines=len(history.read_text().splitlines())
            prompt=f"Continue the independent code review with the existing reviewer. Use SendMessage with to={agent['id']} to ask it for a review of string arguments to def add(a, b): return a + b, without tools. Wait for its answer and summarize it."
            command=[str(Path.home()/'.local/bin/claude'),'-p',prompt,'--safe-mode','--setting-sources','',
                '--tools','SendMessage','--allowedTools','SendMessage','--model',model,'--effort','low',
                '--max-turns','6','--output-format','json','--resume',agent['parent']]
            result=subprocess.run(command,env=env,cwd=cwd,capture_output=True,text=True,timeout=180)
            assert result.returncode==0,result.stdout[-1200:]
            response=json.loads(result.stdout);assert set(response['modelUsage'])=={model}
            added=[json.loads(line) for line in history.read_text().splitlines()[before_lines:]]
            assistants=[row['message'] for row in added if row.get('type')=='assistant']
            parent_history=next(home.glob('projects/*/'+agent['parent']+'.jsonl'))
            calls=[item for line in parent_history.read_text().splitlines() for item in json.loads(line).get('message',{}).get('content',[]) if isinstance(item,dict) and item.get('type')=='tool_use' and item.get('name')=='SendMessage']
            assert any(call.get('input',{}).get('to')==agent['id'] for call in calls),calls
            assert assistants and all(row.get('model')==model for row in assistants),json.dumps({'result':response.get('result'),'expected_agent':agent['id'],'calls':calls[-2:],'files':[p.name for p in history.parent.glob('*.jsonl')],'added_types':[row.get('type') for row in added]})
            assert any(row.get('content') for row in assistants),response.get('result')
            continued.append(agent['id'])
    finally:
        if owns_login:login.unlink(missing_ok=True)
        assert (hashlib.sha256(settings.read_bytes()).hexdigest() if settings.exists() else None)==before,'daily defaults changed'
    print(json.dumps({'model':model,'effort':'low','resumed':continued,'daily_defaults_unchanged':True}))


def resume_codex(payload,root,home,cwd):
    from session_move_codex_real import AppServer, MODEL, EFFORT, digest, inventory, native_metadata
    settings=Path.home()/'.codex/config.toml';before=digest(settings)
    login=home/'auth.json';assert not os.path.lexists(login)
    login.symlink_to(Path.home()/'.codex/auth.json')
    try:
        common={'model':MODEL,'config':{'model_reasoning_effort':EFFORT},'cwd':str(cwd),
            'approvalPolicy':'never','sandbox':'read-only'}
        with AppServer(str(Path.home()/'.local/bin/codex'),home,cwd,root/'target-resume.log') as server:
            active={row['id'] for row in server.listing()}
            archived={row['id'] for row in server.listing(True)}
            for sid,expected in payload['expected'].items():
                assert sid in (archived if expected['metadata']['archived'] else active),(sid,active,archived)
                actual=server.pages(sid)
                assert actual==expected['pages'],('native full history mismatch',sid,actual,expected['pages'])
                current=server.call('thread/read',{'threadId':sid})['thread']
                assert current.get('parentThreadId')==expected['parent'],(sid,current)
                assert current['path']==expected['path'],(sid,current['path'],expected['path'])
                assert native_metadata(home,sid)==expected['metadata'],sid
            for sid in payload['ids']:
                server.call('thread/resume',{**common,'threadId':sid,'excludeTurns':True})
                turn=server.turn(sid,'MOVED_CODEX_CONTINUED')
                assert turn in [row['id'] for row in server.pages(sid)]
        files=inventory(home)  # Actual persisted model and effort, including the subagent.
        unrelated=payload['unrelated']
        assert [row for row in files if row['sid']==unrelated['sid']]==unrelated['files']
        assert native_metadata(home,unrelated['sid'])==unrelated['metadata']
    except Exception as error:
        log=root/'target-resume.log'
        detail=log.read_text()[-600:] if log.exists() else ''
        raise RuntimeError(f'{type(error).__name__}: {str(error)[-1200:]}\n{detail}') from error
    finally:
        login.unlink(missing_ok=True)
        assert digest(settings)==before,'daily configuration changed'
    print(json.dumps({'model':MODEL,'effort':EFFORT,'resumed':payload['ids'],
        'full_history':True,'current_rollouts':True,'native_metadata':True,'native_lists':True,
        'subagent_graph':True,'target_unrelated_preserved':True,'daily_defaults_unchanged':True}))


if __name__=='__main__':
    assert sys.argv[1:]==['--resume-worker']
    resume_worker()
