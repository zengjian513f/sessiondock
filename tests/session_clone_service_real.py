#!/usr/bin/env python3
"""Operator-only production clone API + real Codex complex group, Luna low.
No test-only SQL importer: publication, metadata and recovery use Rust service.
"""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import tempfile
from urllib.request import Request
from session_move_codex_real import AppServer, MODEL, EFFORT, digest, inventory, native_metadata
from history_parity import Corpus, BINARY, isolated_server


def remap_pages(pages, ids):
    result=copy.deepcopy(pages)
    for turn in result:
        turn['id']=ids['turns'][turn['id']]
        for item in turn['items']:
            item['id']=ids['records'][item['id']]
            if item['type']=='collabAgentToolCall':
                item['senderThreadId']=ids['threads'][item['senderThreadId']]
                item['receiverThreadIds']=[ids['threads'][v] for v in item['receiverThreadIds']]
                item['agentsStates']={ids['threads'][k]:v for k,v in item['agentsStates'].items()}
                for agent in item.get('receiverAgents',[]):agent['threadId']=ids['threads'][agent['threadId']]
    return result


def probe(args,root,auth):
    home=root/'codex';home.mkdir(mode=0o700);(home/'auth.json').symlink_to(auth)
    for name in ('claude','grok','state','proc','cwd'):(root/name).mkdir()
    cwd=root/'cwd'
    common={'model':MODEL,'config':{'model_reasoning_effort':EFFORT},'cwd':str(cwd),
            'approvalPolicy':'never','sandbox':'read-only'}
    with AppServer(args.codex,home,cwd,root/'create.log') as server:
        parent=server.call('thread/start',{**common,'historyMode':'paginated'})['thread']
        server.turn(parent['id'],'FAMILY_PARENT');print('Created parent',flush=True)
        child=server.call('thread/fork',{**common,'threadId':parent['id'],'excludeTurns':True})['thread']
        sibling=server.call('thread/fork',{**common,'threadId':parent['id'],'excludeTurns':True})['thread']
        server.turn(child['id'],'CHILD_KEEP')
        removed=server.turn(child['id'],'CHILD_REMOVE')
        child=server.call('thread/revert',{'threadId':child['id'],'beforeTurnId':removed})['thread']
        grandchild=server.call('thread/fork',{**common,'threadId':child['id'],'excludeTurns':True})['thread']
        server.turn(grandchild['id'],'GRANDCHILD');print('Created branches, revert and grandchild',flush=True)
        server.turn(parent['id'],prompt='Ask one independent reviewer to explain an integer-overflow boundary case. '
            'Use spawn_agent to create exactly one subagent named clone_probe, with fresh context, '
            'model gpt-5.6-luna and reasoning effort low (or inherit these current settings if the '
            'tool has no override). Its task is to explain that boundary case briefly, without tools. '
            'If spawn_agent is deferred, first discover it using tool search. '
            'Wait for its completion and summarize the answer. Do not use shell or web tools.')
        agents=[t for t in server.listing() if t.get('parentThreadId')==parent['id']]
        assert len(agents)==1,(agents,json.dumps(server.pages(parent['id'])[-1])[-2400:])
        print('Created native code-mode subagent',flush=True)
        server.call('thread/name/set',{'threadId':child['id'],'name':'Complex clone child'})
        project=server.call('project/create',{'idempotencyKey':'complex-clone','name':'Clone project','roots':[{'path':str(cwd)}]})['project']
        server.call('thread/metadata/update',{'threadId':child['id'],'projectId':project['id']})
        server.call('thread/archive',{'threadId':sibling['id']})
        unrelated=server.call('thread/start',{**common,'historyMode':'paginated'})['thread']
        server.turn(unrelated['id'],'UNRELATED_KEEP')
    with sqlite3.connect(home/'state_5.sqlite') as db:db.execute('UPDATE threads SET is_pinned=1 WHERE id=?',(child['id'],))
    family=[parent,child,sibling,grandchild,agents[0]]
    with AppServer(args.codex,home,cwd,root/'source-cold.log') as server:
        pages={t['id']:server.pages(t['id']) for t in family}
        native={t['id']:server.call('thread/read',{'threadId':t['id']})['thread'] for t in family}
    original=inventory(home)
    uid='codex:'+hashlib.sha1(parent['path'].encode()).hexdigest()[:16]
    if args.production_peer:
        from session_native_transfer import move_native
        # Native fork ancestors are hidden in the normal sidebar. Start from
        # the visible grandchild, as a user would, and require the whole family.
        uid='codex:'+hashlib.sha1(native[grandchild['id']]['path'].encode()).hexdigest()[:16]
        data={'pages':pages,'native':native,'metadata':{t['id']:native_metadata(home,t['id']) for t in family}}
        unrelated_before=native_metadata(home,unrelated['id'])
        result=move_native(root,home,'codex',uid,[child['id'],agents[0]['id']],args.production_peer,
            args.binary,args.new_ids,codex_data=data)
        assert inventory(home)==[row for row in original if row['sid']==unrelated['id']]
        assert native_metadata(home,unrelated['id'])==unrelated_before
        with sqlite3.connect(home/'state_5.sqlite') as db:
            assert not set(data['pages']) & {row[0] for row in db.execute('SELECT id FROM threads')}
        result['checks']+=['source_native_rows_retired','source_unrelated_preserved']
        return result
    with isolated_server(Corpus(root),args.binary,state_dir=root/'state',extra_env={'SESSIONDOCK_PROC_ROOT':str(root/'proc')}) as (base,opener):
        def post(path,body):
            req=Request(base+path,data=json.dumps(body).encode(),headers={'Content-Type':'application/json'})
            try:
                with opener.open(req,timeout=60) as response:return json.load(response)
            except Exception as error:
                if hasattr(error,'read'):print(error.read().decode(),flush=True)
                raise
        plan=post('/api/session/clone/plan',{'uid':uid})
        assert plan['session_count']==5,plan
        result=post('/api/session/clone',{'uid':uid,'operation_id':plan['operation_id']})
        assert result['phase']=='complete',result
    op=json.loads((root/'state/transfers'/plan['operation_id']/'operation.json').read_text())
    ids=op['plan']['identities']
    assert set(ids['threads'])==set(pages)
    assert [r for r in inventory(home) if r['sid'] in set(t['id'] for t in family)|{unrelated['id']}]==original
    print('Production Rust API published the entire new group',flush=True)
    with AppServer(args.codex,home,cwd,root/'native-clone.log') as server:
        active={t['id'] for t in server.listing()};archived={t['id'] for t in server.listing(True)}
        for t in family:
            old=t['id'];new=ids['threads'][old]
            assert new in (archived if old==sibling['id'] else active)
            actual=server.pages(new)
            expected=remap_pages(pages[old],ids)
            assert actual==expected,(old,actual,expected)
            current=server.call('thread/read',{'threadId':new})['thread']
            assert current.get('parentThreadId')==ids['threads'].get(native[old].get('parentThreadId'))
            assert native_metadata(home,new)==native_metadata(home,old)
            file=next(f for f in op['staged']['files'] if f['source']==native[old]['path'])
            assert current['path']==str(home/file['relative'])
        print('Native lists, complete paginated contents, subagent graph, current rollout and metadata match',flush=True)
        for old,word in ((child['id'],'CLONE_CHILD_CONTINUED'),(agents[0]['id'],'CLONE_AGENT_CONTINUED')):
            new=ids['threads'][old]
            server.call('thread/resume',{**common,'threadId':new,'excludeTurns':True})
            turn=server.turn(new,word)
            assert turn in [t['id'] for t in server.pages(new)]
            print('Continued',word,flush=True)
    assert [r for r in inventory(home) if r['sid'] in set(t['id'] for t in family)|{unrelated['id']}]==original
    clones=[r for r in inventory(home) if r['sid'] in ids['threads'].values()]
    with AppServer(args.codex,home,cwd,root/'source-continue.log') as server:
        server.call('thread/resume',{**common,'threadId':child['id'],'excludeTurns':True})
        server.turn(child['id'],'ORIGINAL_CHILD_CONTINUED')
    assert [r for r in inventory(home) if r['sid'] in ids['threads'].values()]==clones
    return {'model':MODEL,'effort':EFFORT,'operation_id':plan['operation_id'],'checks':{
        'production_api':True,'complex_group':True,'code_mode_subagent':True,'native_full_history':True,
        'native_current_rollout':True,'native_metadata':True,'native_listing':True,
        'independent_source_clone_agent_continuation':True,'unrelated_preserved':True}}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--codex',default=shutil.which('codex'))
    parser.add_argument('--keep-workspace',action='store_true')
    parser.add_argument('--production-peer',help='Use Chromium to move the native group to this SSH peer and resume there')
    parser.add_argument('--new-ids',action='store_true',help='Rewrite identities during the cross-node move')
    parser.add_argument('--output',type=Path,default=Path('target/session-clone-service-real.json'))
    args=parser.parse_args();real=Path(os.environ.get('CODEX_HOME',Path.home()/'.codex'))
    args.output.unlink(missing_ok=True)
    if not args.codex or not (real/'auth.json').is_file():parser.error('Codex and controlled existing login required')
    before=digest(real/'config.toml');root=Path(tempfile.mkdtemp(prefix='sessiondock-clone-service-real-'))
    print('Synthetic workspace:',root,flush=True)
    try:
        result=probe(args,root,(real/'auth.json').resolve())
        args.output.parent.mkdir(parents=True,exist_ok=True)
        args.output.write_text(json.dumps(result,indent=2)+'\n');print('PASS',result['checks'],flush=True)
    finally:
        (root/'codex/auth.json').unlink(missing_ok=True)
        if not args.keep_workspace:shutil.rmtree(root)
        assert digest(real/'config.toml')==before,'daily configuration changed; preserve concurrent edits'

if __name__=='__main__':main()
