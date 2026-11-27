"""Synthetic Agy picker and native files for browser deletion tests."""
from pathlib import Path
import json
import sqlite3
from agy_history_browser import SCHEMA, record

IDS = [f'a6000000-0000-4000-8000-{i:012d}' for i in range(10, 16)]
EMPTY_IDS = [f'a6000000-0000-4000-8000-{i:012d}' for i in range(20, 23)]
PICKER_RELOADS = {IDS[4]: ('editor', 'unfiltered', 'draft', 'no_match')}


def seed_empty(native, work):
    """Native zero-step carriers omitted by the Agy resume picker."""
    with sqlite3.connect(native / 'conversation_summaries.db') as c:
        for sid in EMPTY_IDS:
            c.execute('INSERT INTO conversation_summaries VALUES (?,?,?,?,?,?,?,?)',
                (sid, '', '0001-01-01T00:00:00Z', json.dumps([work.as_uri()]), '', 'CASCADE_RUN_STATUS_IDLE', 0, ''))
            with sqlite3.connect(native / 'conversations' / (sid + '.db')) as db:
                db.execute('CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_payload BLOB)')
                db.execute('CREATE TABLE parent_references (idx INTEGER PRIMARY KEY, data BLOB)')
            for directory in ('.user_uploaded', 'scratch'):
                (native / 'brain' / sid / directory).mkdir(parents=True)


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


def retain_native_steps(native, sid):
    """A stale zero-step summary must never stand in for actual native history."""
    path = native / 'conversations' / (sid + '.db')
    path.unlink()
    with sqlite3.connect(path) as db:
        db.execute('CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_payload BLOB)')
        db.execute('CREATE TABLE parent_references (idx INTEGER PRIMARY KEY, data BLOB)')
        db.execute("INSERT INTO steps VALUES (0, X'0102')")
    with sqlite3.connect(native / 'conversation_summaries.db') as c:
        c.execute('UPDATE conversation_summaries SET step_count=0 WHERE conversation_id=?', (sid,))
    (native / 'brain' / sid / '.system_generated/logs/transcript_full.jsonl').unlink()


FAKE = f'PICKER_RELOADS = {PICKER_RELOADS!r}\nSTALE_SID = {IDS[5]!r}\n' + r'''
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
with sqlite3.connect(home/'conversation_summaries.db') as c:
    empty = c.execute('SELECT step_count FROM conversation_summaries WHERE conversation_id=?',(sid,)).fetchone()==(0,)
previous_starts = sum(json.loads(line).get('start') == sid for line in log.read_text().splitlines()) if log.exists() else 0
with log.open('a') as f:f.write(json.dumps({'start':sid,'pid':os.getpid()})+'\n')
tty.setraw(0)
def show(menu=None, draft=''):
    ruler='─'*158
    text = ruler+'\r\n> '+draft+'\r\n'+ruler+'\r\n'
    text += menu if menu else '  ? for shortcuts'
    sys.stdout.write('\033[2J\033[H'+text+f'\033[2;{3+len(draft)}H');sys.stdout.flush()
def picker(confirm=False):
    current = '' if mode == 'wrong' else '[CURRENT] '
    show('   CLI    Other   (tab to cycle)\r\n\r\n  Conversations\r\n  Search: '+sid+
        '\r\n> '+current+'Review Closed Research Reports · 同名 Agy 会话'+(' [Delete? (y/n)]' if confirm else '')+
        '  2 steps just now\r\n\r\nKeyboard: ↑/↓ Navigate  f4 Delete  esc Go back / Clear search')
if mode == 'trust':
    show('Accessing workspace:\r\n\r\n'+str(Path.cwd())+'\r\n\r\nDo you trust the contents of this project?\r\n\r\nAntigravity CLI requires permission to read, edit, and execute files here.\r\n\r\n> Yes, I trust this folder\r\n  No, exit\r\n\r\n  ↑/↓ Navigate · enter Confirm')
else:
    show()
def unfiltered_picker():
    show('   CLI    Other   (tab to cycle)\r\n\r\n  Conversations\r\n  Type to search conversations...\r\n> [CURRENT] Review Closed Research Reports · 同名 Agy 会话\r\n\r\nKeyboard: ↑/↓ Navigate  f4 Delete')

resets = iter(PICKER_RELOADS.get(sid, ()) if mode == 'reload' else ())
stale_worker = mode == 'reload' and sid == STALE_SID and not previous_starts
escape_clears_filter = False
buf=b''
while True:
    data=os.read(0,4096)
    if not data:break
    with log.open('a') as f:f.write(json.dumps({'sid':sid,'input':data.decode(errors='replace')})+'\n')
    buf+=data
    if b'/resume\r' in buf:
        if empty and mode=='ok':
            show('   CLI    Other   (tab to cycle)\r\n\r\n  Conversations\r\n  No conversations available.\r\n  Press Enter to return.\r\n\r\nKeyboard: enter Select  tab Switch Tab  esc Go back / Clear search')
        else:
            unfiltered_picker()
        buf=b''
    elif sid.encode() in buf:
        reset = next(resets, None)
        if reset == 'editor':
            show()
        elif reset == 'unfiltered':
            unfiltered_picker()
        elif reset == 'draft':
            show(draft=sid)
        elif stale_worker or reset == 'no_match':
            escape_clears_filter = True
            show('   CLI    Other   (tab to cycle)\r\n\r\n  Conversations\r\n  Search: '+sid+
                 '\r\n  No matching conversations.\r\n\r\nKeyboard: ↑/↓ Navigate  f4 Delete  esc Go back / Clear search')
        else:
            picker()
        buf=b''
    elif buf == b'\x7f' * len(sid):
        show();buf=b''
    elif buf == b'\x1b':
        if escape_clears_filter:
            unfiltered_picker()
            escape_clears_filter = False
        else:
            show()
        buf=b''
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
