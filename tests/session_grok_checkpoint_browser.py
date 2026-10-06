#!/usr/bin/env python3
"""Operator-only Grok compaction, browser clone and native checkpoint replay.
Private GROK_HOME/cwd; existing authentication is linked, never read or copied.
"""
# run_validation: skip

import argparse
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY, Corpus, isolated_server
from session_files_browser import uid
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN
from grok_files_real_fixtures import MODEL, EFFORT


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None


class Acp:
    def __init__(self, binary, home, cwd):
        self.counter=0;self.messages=queue.Queue();self.cwd=cwd
        self.stderr=tempfile.TemporaryFile()
        self.process=subprocess.Popen([binary,'agent','--no-leader','-m',MODEL,'--reasoning-effort',EFFORT,'stdio'],
            env=dict(os.environ,GROK_HOME=str(home)),cwd=cwd,stdin=subprocess.PIPE,stdout=subprocess.PIPE,
            stderr=self.stderr,text=True,bufsize=1)
        def read():
            for line in self.process.stdout:
                try:self.messages.put(json.loads(line))
                except ValueError:pass
            self.messages.put(None)
        threading.Thread(target=read,daemon=True).start()
        try:
            self.call('initialize',{'protocolVersion':1,'clientCapabilities':{}})
            self.call('authenticate',{'methodId':'cached_token','_meta':{'headless':True}})
        except BaseException:
            self.close();raise
    def call(self,method,params):
        self.counter+=1;ident=self.counter
        self.process.stdin.write(json.dumps({'jsonrpc':'2.0','id':ident,'method':method,'params':params})+'\n')
        self.process.stdin.flush()
        while True:
            message=self.messages.get(timeout=180)
            assert message is not None,(method,'ACP exited')
            if message.get('id')==ident and ('result' in message or 'error' in message):
                assert 'error' not in message,(method,message['error'])
                return message['result']
            if 'method' in message and 'id' in message:
                self.process.stdin.write(json.dumps({'jsonrpc':'2.0','id':message['id'],
                    'error':{'code':-32601,'message':'Tools unavailable in isolated checkpoint test'}})+'\n')
                self.process.stdin.flush()
    def prompt(self,sid,text):
        return self.call('session/prompt',{'sessionId':sid,'prompt':[{'type':'text','text':text}]})
    def close(self):
        self.process.terminate()
        try:self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:self.process.kill();self.process.wait()
        self.stderr.close()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--grok',default=str(Path.home()/'.local/bin/grok'))
    args=parser.parse_args()
    real=Path(os.environ.get('GROK_HOME',Path.home()/'.grok'));config=real/'config.toml';before=digest(config)
    assert (real/'auth.json').is_file(),'Existing controlled Grok login required'
    with tempfile.TemporaryDirectory(prefix='sessiondock-grok-checkpoint-') as tmp, sync_playwright() as pw:
        root=Path(tmp);root.chmod(0o700);corpus=Corpus(root/'node')
        for name in ('codex','claude/projects','grok','cwd','state','proc','ids','trash'):
            (corpus.root/name).mkdir(parents=True)
        home=root/'native';home.mkdir(mode=0o700);(home/'sessions').symlink_to(corpus.root/'grok')
        login=home/'auth.json';login.symlink_to((real/'auth.json').resolve());cwd=corpus.root/'cwd'
        def models():
            for summary in (corpus.root/'grok').glob('*/*/summary.json'):
                row=json.loads(summary.read_text())
                assert row['current_model_id']==MODEL and row['reasoning_effort']==EFFORT,(row['current_model_id'],row['reasoning_effort'])
                for line in (summary.parent/'chat_history.jsonl').read_text().splitlines():
                    item=json.loads(line)
                    if item.get('model_id'):
                        assert item['model_id'] in (MODEL,'grok-4.6-build') and item['reasoning_effort']==EFFORT
        try:
            acp=Acp(args.grok,home,cwd)
            try:
                sid=acp.call('session/new',{'cwd':str(cwd),'mcpServers':[]})['sessionId']
                acp.call('session/set_model',{'sessionId':sid,'modelId':MODEL,'_meta':{'reasoningEffort':EFFORT}})
                models()
                acp.prompt(sid,'Remember checkpoint marker ORCHID_742. Reply exactly CHECKPOINT_READY. Use no tools.')
                acp.prompt(sid,'/compact')
                acp.prompt(sid,'Reply exactly AFTER_COMPACTION. Use no tools.')
            finally:acp.close()
            models()
            folder=next((corpus.root/'grok').glob('*/'+sid))
            checkpoints=list((folder/'compaction_checkpoints').glob('*.json'))
            assert checkpoints,'Native compact did not write a checkpoint'
            checkpoint=json.loads(checkpoints[-1].read_text());old_checkpoint=checkpoint['checkpoint_id']
            print('PASS native Grok compaction checkpoint generated with explicit model/effort',flush=True)
            source={p.relative_to(folder):p.read_bytes() for p in folder.rglob('*') if p.is_file()}
            node=SimpleNamespace(name='source',nid='c'*32,port=free_port(),token=TOKEN)
            (corpus.root/'ids/node-id').write_text(node.nid+'\n');hubroot=root/'hub';hubroot.mkdir()
            with ExitStack() as stack:
                env=node_env(corpus.root,node.port,'127.0.0.0/8');env['SESSIONDOCK_PROC_ROOT']=str(corpus.root/'proc')
                stack.enter_context(isolated_server(corpus,args.binary,state_dir=corpus.root/'state',extra_env=env))
                hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[node]);hub.start();stack.callback(hub.stop)
                browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
                page=browser.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                selected=scoped(node.nid,uid('grok',folder))
                page.locator(f'#side .item[data-uid="{selected}"]').click(button='right')
                page.locator('#item-menu [data-act="clone"]').click()
                dialog=page.locator('#clone-group-dialog');expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=20000)
                with page.expect_response(lambda r:r.url.endswith('/api/session/clone')) as copied:
                    dialog.locator('.clone-confirm').click()
                assert copied.value.ok,copied.value.text()
                operation=json.loads((corpus.root/'state/transfers'/copied.value.json()['operation_id']/'operation.json').read_text())
                plan=operation['file_plan'];clone=plan['sessions']['grok:'+sid]
                mapped=plan['records']['grok:'+old_checkpoint];assert mapped!=old_checkpoint
                page.wait_for_function('(id)=>S.sel===id',arg=copied.value.json()['target_uid'])
                expect(page.locator('#msgs')).to_contain_text('AFTER_COMPACTION')
            cloned=folder.with_name(clone)
            assert (cloned/'compaction_checkpoints'/(mapped+'.json')).is_file()
            acp=Acp(args.grok,home,cwd)
            try:
                acp.call('session/load',{'sessionId':clone,'cwd':str(cwd),'mcpServers':[]})
                acp.call('_x.ai/rewind/execute',{'sessionId':clone,'targetPromptIndex':checkpoint['prompt_index_at_compaction'],'force':True})
                acp.prompt(clone,'What was the checkpoint marker? Reply with the marker only. Use no tools.')
            finally:acp.close()
            history=[json.loads(line) for line in (cloned/'chat_history.jsonl').read_text().splitlines()]
            answers=[row for row in history if row.get('type')=='assistant']
            assert 'ORCHID_742' in json.dumps(answers[-1]),'Native checkpoint replay lost marker'
            assert all((folder/p).read_bytes()==raw for p,raw in source.items()),'Source changed'
            models()
            print('PASS Chromium clone remaps native checkpoint; Grok loads, rewinds and recalls compacted context; source unchanged',flush=True)
        finally:
            login.unlink(missing_ok=True)
            assert digest(config)==before,'Daily Grok configuration changed'


if __name__=='__main__':main()
