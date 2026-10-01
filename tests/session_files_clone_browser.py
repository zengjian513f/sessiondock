#!/usr/bin/env python3
"""Claude/Grok complex family copies through actual node/Hub Chromium controls."""
import argparse
import json
import os
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
import tempfile
from playwright.sync_api import sync_playwright, expect
from history_parity import Corpus, BINARY, isolated_server, get_json
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN
from session_files_browser import fixture, uid, claude_row, encoded
from session_transfer_browser import ident
from session_transfer_byte_fixtures import varied_layout, assert_identity_only


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-files-publish-') as tmp, sync_playwright() as pw:
        root=Path(tmp);roots,claude,side,agent=fixture(root/'node');corpus=Corpus(root/'node')
        # A separate parent owns the resumed agent; no shared message UUIDs or
        # fork fields connect it to the selected family.
        foreign_agent='aabcdef1234567890';foreign_parent=ident(80)
        owner=claude[2].parent/(foreign_parent+'.jsonl')
        def row(sid,kind,n,parent,text,**extra):
            return claude_row(sid,kind,ident(n),parent,text,cwd=str(corpus.root/'cwd'),**extra)
        owner.write_bytes(encoded(row(foreign_parent,'user',801,None,'Independent agent owner'))+
                          encoded(row(foreign_parent,'assistant',802,ident(801),'Independent owner answer')))
        with owner.open('ab') as stream:
            stream.write(encoded(row(foreign_parent,'assistant',809,ident(802),[{'type':'tool_use','id':'cross-send','name':'Bash','input':{'command':'synthetic'}}])))
            stream.write(encoded(row(foreign_parent,'user',810,ident(809),[{'type':'tool_result','tool_use_id':'cross-send',
                'content':json.dumps({'success':True,'resumedAgentId':'missing-unrelated-agent'})}])))
        foreign_side=owner.with_suffix('')/'subagents'/('agent-'+foreign_agent+'.jsonl')
        foreign_side.parent.mkdir(parents=True)
        foreign_side.write_bytes(encoded(row(foreign_parent,'user',803,None,'Foreign agent question',isSidechain=True,agentId=foreign_agent))+
                                encoded(row(foreign_parent,'assistant',804,ident(803),'Foreign agent answer',isSidechain=True,agentId=foreign_agent)))
        with claude[2].open('ab') as stream:
            for kind in ('attachment','system','mode','permission-mode','atis-latch','file-history-delta'):
                stream.write(encoded({'type':kind,'sessionId':ident(2),'fixture_metadata':True}))
            stream.write(encoded(row(ident(2),'assistant',805,ident(154),[{'type':'tool_use','id':'cross-send','name':'SendMessage',
                'input':{'to':foreign_agent[:7],'message':'Cross owner request literal '+foreign_agent[:7]}}])))
            reply={'success':True,'resumedAgentId':foreign_agent,'message':'Resuming agent '+foreign_agent[:7]}
            stream.write(encoded(row(ident(2),'user',806,ident(805),[{'type':'tool_result','tool_use_id':'cross-send','content':json.dumps(reply)}],toolUseResult=reply)))
            # An unrelated tool's JSON and literal user text must not be edges.
            stream.write(encoded(row(ident(2),'assistant',807,ident(806),[{'type':'tool_use','id':'not-send','name':'Bash','input':{'command':'synthetic'}}])))
            stream.write(encoded(row(ident(2),'user',808,ident(807),[{'type':'tool_result','tool_use_id':'not-send',
                'content':json.dumps({'success':True,'resumedAgentId':'missing-unrelated-agent'})}])))
        # A Grok agent owned by an otherwise independent family, referenced by
        # its durable alias through an update stream. Call IDs are session-local.
        grok_alias='ag1.abcdefabcdefabcdefabcdefabcdefab'
        for number in (80,81):
            folder=corpus.root/'grok/project'/ident(number);folder.mkdir(parents=True)
            (folder/'summary.json').write_text(json.dumps({'info':{'id':ident(number),'cwd':str(corpus.root/'cwd')},
                'generated_title':'Foreign Grok '+str(number),'agent_id':grok_alias if number==81 else 'ag1.foreign-owner'}))
            (folder/'chat_history.jsonl').write_bytes(encoded({'type':'user','content':'Foreign Grok question'})+
                encoded({'type':'assistant','content':'Foreign Grok answer '+str(number),
                         'tool_calls':[{'id':'cross-grok','name':'Bash','arguments':'{}'}]})+
                encoded({'type':'tool_result','tool_call_id':'cross-grok','content':'Synthetic completed tool'})+
                encoded({'type':'assistant','content':'Foreign Grok answer '+str(number)}))
            (folder/'updates.jsonl').write_bytes(encoded({'toolCallId':'cross-grok','rawInput':{'subagent_id':grok_alias},
                'rawOutput':'=== Task '+grok_alias+' ===\n'}))
        grok_owner=corpus.root/'grok/project'/ident(80)
        meta=grok_owner/'subagents'/grok_alias/'meta.json';meta.parent.mkdir(parents=True)
        meta.write_text(json.dumps({'subagent_id':grok_alias,'parent_session_id':ident(80),'child_session_id':ident(81)}))
        parent=corpus.root/'grok/project'/ident(10)
        with (parent/'chat_history.jsonl').open('ab') as stream:
            stream.write(encoded({'type':'assistant','tool_calls':[{'id':'cross-grok','name':'send_subagent_message','arguments':'{}'}]}))
        with (parent/'updates.jsonl').open('ab') as stream:
            stream.write(encoded({'params':{'update':{'toolCallId':'cross-grok','rawInput':{'subagent_id':grok_alias,'message':'Literal '+grok_alias},
                'rawOutput':'=== Task '+grok_alias+' ===\n'}}}))
        result_alias='ag1.resultonlyabcdefabcdefabcdefabcd'
        isolated=corpus.root/'grok/project'/ident(82);isolated.mkdir()
        (isolated/'summary.json').write_text(json.dumps({'info':{'id':ident(82),'cwd':str(corpus.root/'cwd')},
            'generated_title':'Result-only agent','agent_id':result_alias}))
        (isolated/'chat_history.jsonl').write_bytes(encoded({'type':'user','content':'Result-only question'})+
            encoded({'type':'assistant','content':'Result-only answer'}))
        with (parent/'chat_history.jsonl').open('ab') as stream:
            stream.write(encoded({'type':'assistant','tool_calls':[{'id':'result-only','name':'spawn_subagent','arguments':'{}'}]}))
        result={'subagent_id':result_alias,'text':'Literal '+result_alias}
        with (parent/'updates.jsonl').open('ab') as stream:
            stream.write(encoded({'params':{'update':{'toolCallId':'result-only','rawOutput':json.dumps(result)}}}))
        with (grok_owner/'chat_history.jsonl').open('ab') as stream:
            stream.write(encoded({'type':'assistant','tool_calls':[{'id':'result-only','name':'Bash','arguments':'{}'}]}))
        with (grok_owner/'updates.jsonl').open('ab') as stream:
            stream.write(encoded({'toolCallId':'result-only','rawOutput':json.dumps({'subagent_id':'missing-unrelated-agent'})}))
        checkpoint_alias='ag1.checkpointabcdefabcdefabcdefabcd'
        checkpoint_agent=corpus.root/'grok/project'/ident(83);checkpoint_agent.mkdir()
        (checkpoint_agent/'summary.json').write_text(json.dumps({'info':{'id':ident(83),'cwd':str(corpus.root/'cwd')},
            'generated_title':'Checkpoint-only agent','agent_id':checkpoint_alias}))
        (checkpoint_agent/'chat_history.jsonl').write_bytes(encoded({'type':'user','content':'Checkpoint agent question'})+
            encoded({'type':'assistant','content':'Checkpoint agent answer'}))
        checkpoint_id=ident(890)
        checkpoint={'checkpoint_id':checkpoint_id,'prompt_index_at_compaction':2,'schema_version':1,
            'created_at':'2026-01-01T00:00:00Z','original_user_info':'Literal '+checkpoint_alias,
            'reread_file_paths':['/example/'+checkpoint_id], 'compacted_history':[
                {'type':'user','content':'Literal '+checkpoint_alias},
                {'type':'assistant','tool_calls':[{'id':'checkpoint-call','name':'spawn_subagent','arguments':'{}'}]},
                {'type':'tool_result','tool_call_id':'checkpoint-call','content':json.dumps({'subagent_id':checkpoint_alias})},
                {'type':'assistant','tool_calls':[{'id':'checkpoint-resume','name':'send_subagent_message',
                    'arguments':json.dumps({'subagent_id':checkpoint_alias,'message':'Literal '+checkpoint_alias})}]}]}
        checkpoint_path=parent/'compaction_checkpoints'/(checkpoint_id+'.json')
        checkpoint_path.parent.mkdir(exist_ok=True);checkpoint_path.write_text(json.dumps(checkpoint,indent=2))
        with (parent/'updates.jsonl').open('ab') as stream:
            stream.write(encoded({'params':{'update':{'sessionUpdate':'compaction_checkpoint',
                'checkpoint_id':checkpoint_id,'checkpoint_file':'compaction_checkpoints/'+checkpoint_id+'.json',
                'schema_version':1,'prompt_index_at_compaction':2,'created_at':checkpoint['created_at']}}}))
        (corpus.root/'state').mkdir();(corpus.root/'proc').mkdir()
        for folder in ('claude','grok'):varied_layout(corpus.root/folder)
        # Preserve a final metadata record with no newline. Message fixtures
        # remain complete, since the UI intentionally hides incomplete tails.
        with claude[2].open('ab') as stream:
            stream.write((' { "type" : "mode", "mode" : "default", "sessionId" : "'+ident(2)+'" } ').encode())
        original={str(p):p.read_bytes() for folder in ('claude','grok') for p in (corpus.root/folder).rglob('*') if p.is_file()}
        node=SimpleNamespace(name='source',nid='c'*32,port=free_port(),token=TOKEN)
        (corpus.root/'ids').mkdir();(corpus.root/'ids/node-id').write_text(node.nid+'\n')
        hubroot=root/'hub';hubroot.mkdir();hub=None
        browser=pw.chromium.launch(headless=True,**({'executable_path':os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']} if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE') else {}))
        operations=[]
        try:
            for restart in (False,True):
                with ExitStack() as stack:
                    env=node_env(corpus.root,node.port,'127.0.0.0/8');env.update(SESSIONDOCK_CLAUDE_ROOT=roots['claude'],SESSIONDOCK_PROC_ROOT=str(corpus.root/'proc'))
                    base,opener=stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',extra_env=env))
                    if hub is None:hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node])
                    hub.start();stack.callback(hub.stop)
                    context=browser.new_context(service_workers='block');stack.callback(context.close)
                    page=context.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                    if not restart:
                        for source,selected in [('claude',uid('claude',claude[2])),('grok',uid('grok',corpus.root/'grok/project'/ident(10)))]:
                            selected=scoped(node.nid,selected)
                            page.reload(wait_until='networkidle')
                            if source=='grok':
                                history=parent/'updates.jsonl';saved_history=history.read_bytes()
                                history.write_bytes(saved_history.replace(result_alias.encode(),b'missing-result-agent'))
                                page.locator(f'#side .item[data-uid="{selected}"]').click()
                                with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as missing:
                                    page.locator('#a-clone-group').click()
                                assert missing.value.status==409,missing.value.text()
                                expect(page.locator('#clone-group-dialog .transfer-error')).to_contain_text('missing-result-agent')
                                expect(page.locator('#clone-group-dialog .clone-confirm')).to_be_disabled()
                                page.locator('#clone-group-dialog .clone-cancel').click()
                                history.write_bytes(saved_history)
                                print('PASS Grok missing explicit result dependency blocks browser confirmation',flush=True)
                            page.locator(f'#side .item[data-uid="{selected}"]').click()
                            page.locator('#a-clone-group').click()
                            dialog=page.locator('#clone-group-dialog')
                            expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=20000)
                            expect(dialog.locator('.clone-members tbody tr')).to_have_count(6 if source=='claude' else 8)
                            with page.expect_response(lambda r:r.url.endswith('/api/session/clone') and r.request.method=='POST') as reply:
                                dialog.locator('.clone-confirm').click()
                            response=reply.value;assert response.ok,response.text();result=response.json()
                            page.wait_for_function('(id)=>S.sel===id',arg=result['target_uid'])
                            expect(page.locator('#msgs')).to_contain_text('Branch A final' if source=='claude' else 'Grok answer 10')
                            op=json.loads((corpus.root/'state/transfers'/result['operation_id']/'operation.json').read_text())
                            assert op['phase']=='complete'
                            assert_identity_only(op,original)
                            print('PASS '+source+' only identity/reference bytes change; layout, CRLF, escapes and EOF are preserved',flush=True)
                            ids=op['file_plan']['sessions']
                            if source=='claude':
                                page.locator('#a-view-switch').click()
                                page.locator(f'#session-view-menu button[data-agent="{ids["claude:"+agent]}"]').click()
                                expect(page.locator('#msgs')).to_contain_text('Agent answer')
                                assert (corpus.root/'claude/file-history'/ids['claude:'+ident(2)]/'abcdef@v1').is_file()
                                clone=claude[2].with_name(ids['claude:'+ident(2)]+'.jsonl')
                                rows=[json.loads(line) for line in clone.read_text().splitlines()]
                                metadata=[row for row in rows if row.get('fixture_metadata')]
                                assert len(metadata)==6 and all('message' not in row for row in metadata),metadata
                                print('PASS Claude metadata rows retain absent message fields after browser cloning',flush=True)
                                call=next(block for row in rows for block in row.get('message',{}).get('content',[]) if isinstance(block,dict)
                                          and block.get('name')=='SendMessage' and block.get('input',{}).get('message','').startswith('Cross owner request'))
                                assert call['input']['to']==ids['claude:'+foreign_agent]
                                assert call['input']['message']=='Cross owner request literal '+foreign_agent[:7]
                                resumed=[row['toolUseResult']['resumedAgentId'] for row in rows if row.get('toolUseResult',{}).get('resumedAgentId')]
                                assert ids['claude:'+foreign_agent] in resumed and foreign_agent not in resumed
                                owner_clone=owner.with_name(ids['claude:'+foreign_parent]+'.jsonl')
                                assert 'missing-unrelated-agent' in owner_clone.read_text()
                                sessions=get_json(opener,base,'/api/sessions')['sessions']
                                foreign=next(row for row in sessions if row['sid']==ids['claude:'+foreign_parent])
                                page.locator(f'#side .item[data-uid="{scoped(node.nid,foreign["uid"])}"]').click()
                                page.locator('#a-view-switch').click()
                                page.locator(f'#session-view-menu button[data-agent="{ids["claude:"+foreign_agent]}"]').click()
                                expect(page.locator('#msgs')).to_contain_text('Foreign agent answer')
                                print('PASS Claude cross-owner SendMessage includes parent/agent and rewrites resolved short destination',flush=True)
                            else:
                                meta=corpus.root/'grok/project'/ids['grok:'+ident(80)]/'subagents'/ids['grok:'+grok_alias]/'meta.json'
                                assert json.loads(meta.read_text())['child_session_id']==ids['grok:'+ident(81)]
                                copied=corpus.root/'grok/project'/ids['grok:'+ident(10)]/'updates.jsonl'
                                update=json.loads(copied.read_text().splitlines()[-3])['params']['update']
                                assert update['rawInput']['subagent_id']==ids['grok:'+grok_alias]
                                assert update['rawInput']['message']=='Literal '+grok_alias
                                assert update['rawOutput']=='=== Task '+ids['grok:'+grok_alias]+' ===\n'
                                foreign=corpus.root/'grok/project'/ids['grok:'+ident(80)]/'updates.jsonl'
                                unrelated=json.loads(foreign.read_text().splitlines()[0])
                                assert unrelated['rawInput']['subagent_id']==grok_alias
                                assert unrelated['rawOutput']=='=== Task '+grok_alias+' ===\n'
                                agent_result=json.loads(json.loads(copied.read_text().splitlines()[-2])['params']['update']['rawOutput'])
                                assert agent_result=={'subagent_id':ids['grok:'+result_alias],'text':'Literal '+result_alias}
                                assert 'missing-unrelated-agent' in foreign.read_text()
                                records=op['file_plan']['records']
                                mapped_checkpoint=records['grok:'+checkpoint_id]
                                pointer=json.loads(copied.read_text().splitlines()[-1])['params']['update']
                                assert pointer['checkpoint_id']==mapped_checkpoint!=checkpoint_id
                                assert pointer['checkpoint_file']=='compaction_checkpoints/'+mapped_checkpoint+'.json'
                                cloned_checkpoint=json.loads((copied.parent/pointer['checkpoint_file']).read_text())
                                assert cloned_checkpoint['checkpoint_id']==mapped_checkpoint
                                history=cloned_checkpoint['compacted_history']
                                assert history[0]==checkpoint['compacted_history'][0]
                                assert cloned_checkpoint['original_user_info']==checkpoint['original_user_info']
                                assert cloned_checkpoint['reread_file_paths']==checkpoint['reread_file_paths']
                                assert history[1]['tool_calls'][0]['id']==history[2]['tool_call_id']!= 'checkpoint-call'
                                assert json.loads(history[2]['content'])['subagent_id']==ids['grok:'+checkpoint_alias]
                                arguments=json.loads(history[3]['tool_calls'][0]['arguments'])
                                assert arguments=={'subagent_id':ids['grok:'+checkpoint_alias],'message':'Literal '+checkpoint_alias}
                                assert not (copied.parent/'compaction_checkpoints'/(checkpoint_id+'.json')).exists()
                                print('PASS Grok checkpoint-only agent dependency, checkpoint pointer/file and embedded tool identities rewritten; prose preserved',flush=True)

                                print('PASS Grok result-only dependency included and serialized native identity rewritten; other-session Bash result unchanged',flush=True)
                                print('PASS Grok cross-owner durable alias closes group; chat/update calls scoped to owner and ordinary tool data preserved',flush=True)
                                rows=get_json(opener,base,'/api/sessions')['sessions']
                                child=next(row for row in rows if row['sid']==ids['grok:'+ident(13)])
                                page.locator(f'#side .item[data-uid="{scoped(node.nid,child["uid"])}"]').click()
                                expect(page.locator('#msgs')).to_contain_text('Grok answer 13')
                                foreign=next(row for row in rows if row['sid']==ids['grok:'+ident(81)])
                                page.locator(f'#side .item[data-uid="{scoped(node.nid,foreign["uid"])}"]').click()
                                expect(page.locator('#msgs')).to_contain_text('Foreign Grok answer 81')
                                isolated=next(row for row in rows if row['sid']==ids['grok:'+ident(82)])
                                page.locator(f'#side .item[data-uid="{scoped(node.nid,isolated["uid"])}"]').click()
                                expect(page.locator('#msgs')).to_contain_text('Result-only answer')
                            assert all(Path(p).read_bytes()==raw for p,raw in original.items())
                            operations.append((selected,result,op))
                            print('PASS real Hub/Chromium '+source+' whole-family copy, opened new history/agent, source intact',flush=True)
                        # Fail after earlier files have been published: rollback
                        # must remove only this transaction's owned files.
                        selected=operations[0][0]
                        page.locator(f'#side .item[data-uid="{selected}"]').click()
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone/plan')) as planned:
                            page.locator('#a-clone-group').click()
                        plan=planned.value.json()
                        journal=corpus.root/'state/transfers'/plan['operation_id']/'operation.json'
                        pending=json.loads(journal.read_text())
                        last=Path(pending['file_publications'][-1]['staging'])
                        last.write_bytes(last.read_bytes()+b'changed')
                        with page.expect_response(lambda r:r.url.endswith('/api/session/clone') and r.request.method=='POST') as failed:
                            page.locator('#clone-group-dialog .clone-confirm').click()
                        assert not failed.value.ok
                        assert json.loads(journal.read_text())['phase']=='failed'
                        assert all(not Path(f['target']).exists() for f in pending['file_publications'])
                        assert all(Path(p).read_bytes()==raw for p,raw in original.items())
                        print('PASS failed file-family publication rolls back owned files and preserves source',flush=True)
                    else:
                        for selected,result,op in operations:
                            target=Path(op['file_publications'][0]['target']);before=target.read_bytes()
                            # A retry after restart cannot republish or overwrite a continued clone.
                            target.write_bytes(before+b'\n')
                            reply=context.request.post(f'http://127.0.0.1:{hub.port}/api/session/clone',data={'uid':selected,'operation_id':op['id']})
                            assert reply.ok,reply.text();assert reply.json()['target_uid']==result['target_uid']
                            assert target.read_bytes()==before+b'\n'
                        assert all(Path(p).read_bytes()==raw for p,raw in original.items())
                        print('PASS file-family publication retries survive node/Hub restart without replacing continued files',flush=True)
        finally:browser.close()


if __name__=='__main__':main()
