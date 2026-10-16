"""Synthetic Agy picker and native files for browser deletion tests."""
from pathlib import Path
import json
import sqlite3
from agy_history_browser import SCHEMA, record

IDS = [f'a6000000-0000-4000-8000-{i:012d}' for i in range(10, 16)]


def seed(native, work):
    native.mkdir()
    (native / 'conversations').mkdir()
    with sqlite3.connect(native / 'conversation_summaries.db') as c:
        c.execute(SCHEMA)
        for sid in IDS:
            c.execute('INSERT INTO conversation_summaries VALUES (?,?,?,?,?,?,?,?)',
                (sid, 'Review Closed Research Reports · 同名 Agy 会话', '2026-10-16T00:00:00Z', json.dumps([work.as_uri()]), '', 'IDLE', 2, ''))
            (native / 'conversations' / (sid + '.db')).write_bytes(b'native synthetic database')
            logs = native / 'brain' / sid / '.system_generated/logs'
            logs.mkdir(parents=True)
            (logs / 'transcript_full.jsonl').write_text(json.dumps(record(0, 'USER_INPUT', 'fixture ' + sid)) + '\n')


FAKE = r'''
import os, sys, tty, json, sqlite3, shutil
from pathlib import Path
if '--version' in sys.argv:
    print('1.4.0');sys.exit(0)
if 'models' in sys.argv:
    sys.exit(0)
if '--conversation' not in sys.argv:
    sys.exit(2)
sid = sys.argv[sys.argv.index('--conversation')+1]
home = Path(os.environ['DELETE_NATIVE'])
log = Path(os.environ['DELETE_LOG'])
mode = (home / 'mode').read_text() if (home / 'mode').exists() else 'ok'
with log.open('a') as f:f.write(json.dumps({'start':sid,'pid':os.getpid()})+'\n')
tty.setraw(0)
def show(menu=None):
    ruler='─'*158
    text = ruler+'\r\n> \r\n'+ruler+'\r\n'
    text += menu if menu else '  ? for shortcuts'
    sys.stdout.write('\033[2J\033[H'+text+'\033[2;3H');sys.stdout.flush()
def picker(confirm=False):
    current = '' if mode == 'wrong' else '[CURRENT] '
    show('   CLI    Other   (tab to cycle)\r\n\r\n  Conversations\r\n  Search: '+sid+
        '\r\n> '+current+'Review Closed Research Reports · 同名 Agy 会话'+(' [Delete? (y/n)]' if confirm else '')+
        '  2 steps just now\r\n\r\nKeyboard: ↑/↓ Navigate  f4 Delete  esc Go back / Clear search')
show();buf=b''
while True:
    data=os.read(0,4096)
    if not data:break
    with log.open('a') as f:f.write(json.dumps({'sid':sid,'input':data.decode(errors='replace')})+'\n')
    buf+=data
    if b'/resume\r' in buf:
        show('   CLI    Other   (tab to cycle)\r\n\r\n  Conversations\r\n  Type to search conversations...\r\n> [CURRENT] Review Closed Research Reports · 同名 Agy 会话\r\n\r\nKeyboard: ↑/↓ Navigate  f4 Delete');buf=b''
    elif sid.encode() in buf:
        picker();buf=b''
    elif b'\x1bOS' in buf:
        picker(True);buf=b''
    elif buf==b'y':
        if mode!='refuse':
            with sqlite3.connect(home/'conversation_summaries.db') as c:c.execute('DELETE FROM conversation_summaries WHERE conversation_id=?',(sid,))
            (home/'conversations'/(sid+'.db')).unlink()
            shutil.rmtree(home/'brain'/sid)
        show('Delete failed' if mode=='refuse' else 'No conversations available.')
        buf=b''
'''
