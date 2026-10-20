"""Credential-free Agy 1.4.2 NDJSON fixture for native SDK browser tests.

Only the fake CLI writes its synthetic native store. The bridge must never
write that store or create its own mirror. No real CLI/account/model is used.
"""
# run_validation: skip
from __future__ import annotations

import json
import os
from pathlib import Path
import sqlite3
import sys

SID = 'a7600000-0000-4000-8000-000000000001'
MODEL = 'synthetic-agy-low'
TITLE = 'Agy SDK synthetic history'
SCHEMA = '''CREATE TABLE IF NOT EXISTS conversation_summaries (
    conversation_id TEXT PRIMARY KEY, title TEXT, last_modified_time TEXT,
    workspace_uris TEXT, parent_conversation_id TEXT, status TEXT,
    step_count INTEGER, agent_name TEXT)'''

FAKE = r"""
import json, os, signal, sqlite3, subprocess, sys, threading, time, uuid
from pathlib import Path
args=sys.argv[1:]
root=Path(os.environ['AGY_SDK_FIXTURE'])
native=Path(os.environ['HOME'])/'.gemini/antigravity-cli'
log=root/'native-events.jsonl'
def record(value):
    with log.open('a') as output:output.write(json.dumps(value,ensure_ascii=False)+'\n')
def emit(value):
    print(json.dumps(value,ensure_ascii=False),flush=True)
def option(name):
    return args[args.index(name)+1] if name in args else None
record({'kind':'launch','argv':args,'cwd':os.getcwd(),'pid':os.getpid()})
if '--version' in args:print('1.4.2');sys.exit(0)
if option('-p')=='/usage':
    deadline=time.monotonic()+20
    while (root/'quota-hold').exists():
        if time.monotonic()>deadline:sys.exit(1)
        threading.Event().wait(0.02)
    if (root/'quota-fail').exists():sys.exit(1)
    remaining=float((root/'quota-remaining').read_text()) if (root/'quota-remaining').exists() else 0.72
    emit({'status':'SUCCESS','num_turns':0,'command':{'name':'usage','data':{'groups':[{'name':'Agy Standard','buckets':[{'id':'standard','window':'5h','remaining_fraction':remaining,'reset_time':'2026-10-20T00:00:00Z'}]}]}}});sys.exit(0)
if 'models' in args:
    print('synthetic-agy-low\tSynthetic Agy Low\nsynthetic-agy-high\tSynthetic Agy High');sys.exit(0)
assert not sys.stdin.isatty(), 'SDK must not use a PTY'
assert '--dangerously-skip-permissions' not in args, 'bridge must not auto-approve'
assert option('--input-format')=='stream-json' and option('--output-format')=='stream-json', args
sid=option('--conversation') or str(uuid.uuid4())
path=native/'conversations'/f'{sid}.db'
assert '--conversation' not in args or path.exists(), 'resume must not fabricate missing native store'
(native/'conversations').mkdir(parents=True,exist_ok=True)
connection=sqlite3.connect(path)
connection.execute('CREATE TABLE IF NOT EXISTS fixture_steps (next_index INTEGER)')
if not connection.execute('SELECT count(*) FROM fixture_steps').fetchone()[0]:
    connection.execute('INSERT INTO fixture_steps VALUES (0)')
connection.commit()
index=connection.execute('SELECT next_index FROM fixture_steps').fetchone()[0]
with sqlite3.connect(native/'conversation_summaries.db') as catalog:
    catalog.execute('''CREATE TABLE IF NOT EXISTS conversation_summaries (
      conversation_id TEXT PRIMARY KEY,title TEXT,last_modified_time TEXT,workspace_uris TEXT,
      parent_conversation_id TEXT,status TEXT,step_count INTEGER,agent_name TEXT)''')
    catalog.execute('INSERT OR IGNORE INTO conversation_summaries VALUES (?,?,?,?,?,?,?,?)',
       (sid,'Agy SDK synthetic history','2026-10-18T00:00:00Z',json.dumps([Path.cwd().as_uri()]),'','IDLE',index,''))
logs=native/'brain'/sid/'.system_generated/logs';logs.mkdir(parents=True,exist_ok=True)
transcript=logs/'transcript_full.jsonl'
def native_record(kind,content):
    with transcript.open('a') as output:
        output.write(json.dumps({'type':kind,'content':content,'step_index':index,
          'created_at':'2026-10-18T00:00:00Z','status':'DONE','source':'MODEL'},ensure_ascii=False)+'\n')
def step(kind,**fields):
    emit({'event':'step_update','step_update':dict(conversation_id=sid,step_index=index,step_type=kind,**fields)})
def advance():
    global index
    index+=1
    connection.execute('UPDATE fixture_steps SET next_index=?',(index,));connection.commit()
def result(status,response='',error=None):
    payload=dict(conversation_id=sid,status=status,response=response,num_turns=index,
                 duration_seconds=0.01,usage=dict(input_tokens=12000,output_tokens=800,thinking_tokens=300,cache_read_tokens=9000,total_tokens=12800))
    if error:payload['error']=error
    emit({'event':'result','result':payload})
def interrupted(*_):
    record({'kind':'signal','sid':sid,'signal':'SIGINT'})
    result('INTERRUPTED',error='synthetic native interrupt')
    sys.exit(0)
signal.signal(signal.SIGINT,interrupted)
init_sid='a7600000-0000-4000-8000-999999999999' if (root/'wrong-sid').exists() else sid
emit({'event':'init','conversation_id':init_sid,'init':dict(cwd=os.getcwd(),model=option('--model'),permission_mode='request-review',tools=['view_file','run_command'])})
for line in sys.stdin:
    message=json.loads(line)
    assert message['event']=='user',message
    text=message['message']['content']
    record({'kind':'input','sid':sid,'text':text,'model':option('--model'),'effort':option('--effort'),'mode':option('--mode'),'pid':os.getpid()})
    native_record('USER_INPUT','<USER_REQUEST>\n'+text+'\n</USER_REQUEST>')
    step('user_input',state='DONE');advance()
    if text=='__DESCENDANT_PIPE__':
        descendant=subprocess.Popen([sys.executable,'-c','import time; time.sleep(120)'])
        record({'kind':'descendant','sid':sid,'pid':descendant.pid,'parent_pid':os.getpid()})
        os._exit(0)
    if text=='__CRASH__':os._exit(7)
    if text=='__ERROR__':
        print('synthetic transport diagnostic api_key='+os.environ['AGY_SDK_FAKE_KEY'],file=sys.stderr,flush=True)
        result('ERROR',error='synthetic native error');continue
    if text.startswith('hold:'):
        token=text.split(':',1)[1]
        assert Path(token).name==token
        step('agent_response',state='ACTIVE',text_delta='Agy running '+token+' ')
        deadline=time.monotonic()+25
        while not (root/'release'/token).exists():
            if time.monotonic()>deadline:result('ERROR',error='fixture hold timeout');sys.exit(1)
            threading.Event().wait(0.02)
        step('agent_response',state='DONE',text_delta='finished '+token);advance()
        answer='Agy running '+token+' finished '+token
    else:
        step('tool',state='ACTIVE',tool_name='view_file',tool_info={'name':'view_file','parameters':{'AbsolutePath':str(Path.cwd()/'synthetic.txt')}})
        step('tool',state='DONE',tool_name='view_file',tool_info={'output':'synthetic tool output'})
        advance()
        step('agent_response',state='ACTIVE',text_delta='Synthetic Agy reply: ')
        step('agent_response',state='DONE',text_delta=text);advance()
        answer='Synthetic Agy reply: '+text
    native_record('PLANNER_RESPONSE',answer)
    with sqlite3.connect(native/'conversation_summaries.db') as catalog:
        catalog.execute('UPDATE conversation_summaries SET step_count=?,last_modified_time=? WHERE conversation_id=?',(index,'2026-10-18T00:00:01Z',sid))
    result('SUCCESS',answer)
connection.close()
"""


class AgyFixture:
    def __init__(self, root: Path, host_binary: Path):
        self.root = root.resolve()
        for name in ('home', 'work', 'host', 'ledger', 'state', 'mirror', 'proc',
                     'release', 'claude', 'codex', 'grok', 'opencode', 'bin'):
            (root / name).mkdir(mode=0o700, exist_ok=True)
        self.work = root / 'work'
        self.native = root / 'home/.gemini/antigravity-cli'
        self.native.mkdir(parents=True, mode=0o700)
        self.mirror = root / 'mirror'
        self.proc = root / 'proc'
        (self.proc / 'stat').write_text('btime 1700000000\n')
        self.executable = root / 'bin/agy'
        self.executable.write_text('#!' + str(Path(sys.executable).resolve()) + '\n' + FAKE)
        self.executable.chmod(0o700)
        self.env = {'HOME': str(root / 'home'), 'PATH': '/usr/bin:/bin',
                    'LANG': 'C.UTF-8', 'AGY_SDK_FIXTURE': str(root),
                    'AGY_SDK_FAKE_KEY': 'synthetic-key-must-be-redacted',
                    'XDG_CONFIG_HOME': str(root / 'home/config'),
                    'XDG_CACHE_HOME': str(root / 'home/cache'),
                    'XDG_DATA_HOME': str(root / 'home/data')}
        self.launcher = root / 'launcher.json'
        self.launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(host_binary),
            'host_dir': str(root / 'host'), 'adapters': [], 'profiles': [
                {'id': 'agy-sdk-fixture', 'source': 'agy', 'executable': str(self.executable),
                 'args': [], 'env': self.env}]}))
        self.launcher.chmod(0o600)
        self.extra_env = {'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_AGY_HOME': str(self.native),
                          'SESSIONDOCK_AGY_ROOT': str(self.mirror),
                          'SESSIONDOCK_OPENCODE_ROOT': str(root / 'opencode'),
                          'SESSIONDOCK_OPENCODE_DB': str(root / 'opencode/empty.db'),
                          'SESSIONDOCK_PROC_ROOT': str(self.proc)}
        self.seed(SID)

    def seed(self, sid):
        (self.native / 'conversations').mkdir(exist_ok=True)
        with sqlite3.connect(self.native / 'conversations' / (sid + '.db')) as connection:
            connection.execute('CREATE TABLE fixture_steps (next_index INTEGER)')
            connection.execute('INSERT INTO fixture_steps VALUES (2)')
        with sqlite3.connect(self.native / 'conversation_summaries.db') as connection:
            connection.execute(SCHEMA)
            connection.execute('INSERT INTO conversation_summaries VALUES (?,?,?,?,?,?,?,?)',
                (sid, TITLE, '2026-10-18T00:00:00Z', json.dumps([self.work.as_uri()]), '', 'IDLE', 2, ''))
        logs = self.native / 'brain' / sid / '.system_generated/logs'
        logs.mkdir(parents=True)
        logs.joinpath('transcript_full.jsonl').write_text('\n'.join(json.dumps(row) for row in [
            {'type': 'USER_INPUT', 'step_index': 0, 'status': 'DONE', 'content': '<USER_REQUEST>earlier user</USER_REQUEST>'},
            {'type': 'PLANNER_RESPONSE', 'step_index': 1, 'status': 'DONE', 'content': 'earlier Agy answer'}]) + '\n')

    def events(self):
        path = self.root / 'native-events.jsonl'
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def inputs(self, text=None):
        return [event for event in self.events() if event['kind'] == 'input'
                and (text is None or event['text'] == text)]

    def release(self, token):
        (self.root / 'release' / token).touch()

    def occupy(self, sid=SID):
        """Synthetic fd evidence; no real unrelated process is started or signaled."""
        fd = self.proc / '424242/fd'
        fd.mkdir(parents=True, exist_ok=True)
        (fd / '3').symlink_to(self.native / 'conversations' / (sid + '.db'))

    def unoccupy(self):
        (self.proc / '424242/fd/3').unlink(missing_ok=True)
