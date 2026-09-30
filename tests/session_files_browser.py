#!/usr/bin/env python3
"""Synthetic Claude/Grok bundle and browser history checks; private fixtures only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright, expect
from history_parity import Corpus, BINARY, claude_row, encoded, isolated_server, get_json
from session_transfer_browser import command, ident, fingerprint


def uid(source,path):
    return source+':'+hashlib.sha1(str(path).encode()).hexdigest()[:16]


def fixture(root):
    for name in ('claude/projects','grok','codex','cwd'):(root/name).mkdir(parents=True)
    project=root/'claude/projects/project';project.mkdir()
    roots={'claude':str(root/'claude/projects'),'grok':str(root/'grok')}
    main={}
    for i,label in ((1,'Parent'),(2,'Branch A'),(3,'Branch B')):
        sid=ident(i);path=project/(sid+'.jsonl')
        rows=[claude_row(sid,'user',ident(101),None,'Shared ancestor literal '+ident(1)),
              claude_row(sid,'assistant',ident(102),ident(101),'Shared answer'),
              claude_row(sid,'user',ident(110+i),ident(102),label),
              claude_row(sid,'assistant',ident(120+i),ident(110+i),label+' answer')]
        for row in rows:row['cwd']=str(root/'cwd')
        path.write_bytes(b''.join(map(encoded,rows)));main[i]=path
    agent='a1234567890abcdef'
    side=project/ident(2)/'subagents'/('agent-'+agent+'.jsonl');side.parent.mkdir(parents=True)
    side.write_bytes(b''.join(encoded(claude_row(ident(2),kind,ident(n),parent,text,isSidechain=True,agentId=agent,cwd=str(root/'cwd')))
        for kind,n,parent,text in [('user',130,None,'Agent question'),('assistant',131,ident(130),'Agent answer')]))
    side.with_suffix('.meta.json').write_text(json.dumps({'agentId':agent,'parentSessionId':ident(2),'agentType':'explore'}))
    output=project/ident(2)/'tool-results/result.txt';output.parent.mkdir();output.write_bytes(b'opaque result '+ident(1).encode())
    with main[2].open('ab') as stream:
        stream.write(encoded(claude_row(ident(2),'assistant',ident(150),ident(122),[{'type':'tool_use','id':'toolu_fixture','name':'Bash','input':{'command':'synthetic'}}],cwd=str(root/'cwd'))))
        stream.write(encoded(claude_row(ident(2),'user',ident(151),ident(150),[{'type':'tool_result','tool_use_id':'toolu_fixture','content':'<persisted-output>\nFull output saved to: '+str(output)+'\n</persisted-output>'}],toolUseResult={'outputFile':str(output)},cwd=str(root/'cwd'))))
        stream.write(encoded(claude_row(ident(2),'assistant',ident(152),ident(151),'Branch A final',cwd=str(root/'cwd'))))
    history=root/'claude/file-history'/ident(2)/'abcdef@v1';history.parent.mkdir(parents=True);history.write_bytes(b'original file\x00bytes')
    for i in (10,11,12,13):
        path=root/'grok/project'/ident(i);path.mkdir(parents=True)
        summary={'info':{'id':ident(i),'cwd':str(root/'cwd')},'generated_title':f'Grok branch {i}',
                 'parent_session_id':ident(10) if i in (11,12) else None,
                 'agent_id':ident(13) if i==13 else 'ag1.'+str(i)*16,'reasoning_effort':'low'}
        (path/'summary.json').write_text(json.dumps(summary))
        (path/'chat_history.jsonl').write_bytes(encoded({'type':'user','content':f'Grok question {i}'})+encoded({'type':'assistant','content':f'Grok answer {i}'}))
        (path/'updates.jsonl').write_bytes(encoded({'session_id':ident(i),'update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'literal '+ident(i)}}}))
        (path/'compaction_checkpoints').mkdir();(path/'compaction_checkpoints/opaque.bin').write_bytes(b'checkpoint')
    parent=root/'grok/project'/ident(10)
    meta=parent/'subagents'/ident(13)/'meta.json';meta.parent.mkdir(parents=True)
    meta.write_text(json.dumps({'subagent_id':ident(13),'parent_session_id':ident(10),'child_session_id':ident(13),'status':'completed'}))
    with (parent/'chat_history.jsonl').open('ab') as stream:
        stream.write(encoded({'type':'assistant','tool_calls':[{'id':'call_native','name':'get_command_or_subagent_output','arguments':json.dumps({'task_ids':[ident(13)]})}]}))
        stream.write(encoded({'type':'tool_result','tool_call_id':'call_native','content':'=== Task '+ident(13)+' ===\nLiteral '+ident(13)+'\n<subagent_result>\nsubagent_id: '+ident(13)+'\n</subagent_result>'}))
    with (parent/'updates.jsonl').open('ab') as stream:
        stream.write(encoded({'method':'session/update','params':{'sessionId':ident(10),'_meta':{'eventId':'event_native'},'update':{'sessionUpdate':'subagent_spawned','subagent_id':ident(13),'parent_session_id':ident(10)}}}))
    return roots,main,side,agent


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args();transfer=args.binary.with_name('sessiondock-transfer')
    with tempfile.TemporaryDirectory(prefix='sessiondock-files-') as tmp, sync_playwright() as pw:
        root=Path(tmp);roots,claude,side,agent=fixture(root/'source');before=fingerprint(root/'source')
        browser=pw.chromium.launch(headless=True,**({'executable_path':os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']} if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE') else {}))
        try:
            for source,selected in [('claude',uid('claude',claude[2])),('grok',uid('grok',root/'source/grok/project'/ident(11)))]:
                expected=4
                group=command(transfer,{'operation':'group','uid':selected,'roots':roots})
                assert len(group['members'])==expected,group
                for member in group['members']:
                    again=command(transfer,{'operation':'group','uid':member['uid'],'roots':roots})
                    assert {m['uid'] for m in again['members']}=={m['uid'] for m in group['members']}
                for fresh in (False,True):
                    plan=command(transfer,{'operation':'plan_files','uid':selected,'roots':roots,'new_ids':fresh})
                    destination=root/(source+str(fresh))
                    command(transfer,{'operation':'stage_files','plan':plan,'destination':str(destination)})
                    for other in ('claude','grok','codex'):(destination/other).mkdir(exist_ok=True)
                    if not fresh:
                        for file in plan['files']:assert (destination/file['provider']/file['target']).read_bytes()==Path(file['source']).read_bytes()
                    if source=='claude':
                        mapping=plan['sessions'];new=mapping['claude:'+ident(2)];new_agent=mapping['claude:'+agent]
                        assert (destination/'claude/file-history'/new/'abcdef@v1').read_bytes()==b'original file\x00bytes'
                        assert (destination/'claude/projects/project'/new/'tool-results/result.txt').read_bytes()==b'opaque result '+ident(1).encode()
                        rows=[json.loads(line) for line in (destination/'claude/projects/project'/(new+'.jsonl')).read_text().splitlines()]
                        result=next(r for r in rows if r.get('toolUseResult'))
                        expected_path=str(root/'source/claude/projects/project'/new/'tool-results/result.txt')
                        assert result['toolUseResult']['outputFile']==expected_path
                        assert expected_path in result['message']['content'][0]['content']
                    if source=='grok':
                        ids=plan['sessions'];parent=destination/'grok/project'/ids['grok:'+ident(10)]
                        child=ids['grok:'+ident(13)]
                        meta=json.loads((parent/'subagents'/child/'meta.json').read_text())
                        assert meta['child_session_id']==child and meta['parent_session_id']==ids['grok:'+ident(10)]
                        chat=[json.loads(line) for line in (parent/'chat_history.jsonl').read_text().splitlines()]
                        assert json.loads(chat[-2]['tool_calls'][0]['arguments'])['task_ids']==[child]
                        assert '=== Task '+child+' ===' in chat[-1]['content']
                        assert 'Literal '+ident(13) in chat[-1]['content']
                        assert chat[-1]['tool_call_id']==chat[-2]['tool_calls'][0]['id']
                        event=json.loads((parent/'updates.jsonl').read_text().splitlines()[-1])
                        assert event['params']['sessionId']==ids['grok:'+ident(10)]
                        assert event['params']['update']['subagent_id']==child
                    corpus=Corpus(destination)
                    with isolated_server(corpus,args.binary,extra_env={'SESSIONDOCK_CLAUDE_ROOT':str(destination/'claude/projects')}) as (base,opener):
                        rows=get_json(opener,base,'/api/sessions')['sessions'];context=browser.new_context(service_workers='block');page=context.new_page()
                        page.goto(base,wait_until='networkidle')
                        for old in ((1,2,3) if source=='claude' else (10,11,12,13)):
                            new=plan['sessions'][source+':'+ident(old)]
                            row=next(r for r in rows if r['sid']==new)
                            page.locator(f'#side .item[data-uid="{row["uid"]}"]').click()
                            expect(page.locator('#msgs')).to_contain_text('Shared answer' if source=='claude' else f'Grok answer {old}')
                            if source=='claude':expect(page.locator('#msgs')).to_contain_text('Shared ancestor literal '+ident(1))
                            if source=='claude' and old==2:
                                page.locator('#a-view-switch').click()
                                page.locator(f'#session-view-menu button[data-agent="{new_agent}"]').click()
                                expect(page.locator('#msgs')).to_contain_text('Agent answer')
                        context.close()
                    assert fingerprint(root/'source')==before
                    print(f'PASS {source} {"clone" if fresh else "move"} bundle: whole family, browser opens branches/agents, source intact',flush=True)
        finally:browser.close()


if __name__=='__main__':main()
