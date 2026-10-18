"""Synthetic OpenCode v2 API and CLI; no real CLI, provider, or credentials.

The Rust bridge invokes the launcher profile with `api METHOD PATH --data`.
This CLI forwards only to the fixture's loopback HTTP server. Native-style
records are also written to a private SQLite fixture for the ordinary mirror.
"""
# run_validation: skip
from __future__ import annotations

from contextlib import closing
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sqlite3
import socket
import sys
import threading
import time
from urllib.parse import parse_qs, urlsplit
from urllib.request import ProxyHandler, Request, build_opener
from urllib.error import HTTPError

from fake_opencode_composer import SCHEMA

SID = 'ses_sessiondock_sdk_seed'
MODEL = {'providerID': 'synthetic', 'id': 'fake-opencode', 'variant': 'low'}


class FakeOpenCode:
    def __init__(self, root):
        self.stream_release = threading.Event()
        self.root = Path(root)
        self.cwd = self.root / 'work'
        self.cwd.mkdir(exist_ok=True)
        self.db = self.root / 'opencode.db'
        self.sessions = {}
        self.messages = {}
        self.active = set()
        self.turns = {}
        self.permissions = {}
        self.forms = {}
        self.received = []
        self.pending = {}
        self.inbox = {}
        self.forks = 0
        self.lose_ack = None
        self.lock = threading.RLock()
        self.closed = threading.Event()
        self.threads = []
        with closing(sqlite3.connect(self.db)) as connection, connection:
            connection.executescript(SCHEMA)
            connection.execute('INSERT INTO project VALUES (?, ?, ?, ?, ?)',
                               ('sdkproject', str(self.cwd), 1, 1, '[]'))
        self.create(SID, str(self.cwd))
        self.append(SID, {'id': 'msg_sdk_prior', 'type': 'user', 'text': 'SDK native history',
                          'time': {'created': 1}})

    def create(self, sid, cwd, model=None):
        info = {'id': sid, 'projectID': 'sdkproject', 'location': {'directory': cwd},
                'model': model or dict(MODEL), 'title': 'OpenCode SDK synthetic',
                'agent': 'build', 'permissions': [], 'cost': 0,
                'tokens': {'input': 0, 'output': 0, 'reasoning': 0, 'cache': {'read': 0, 'write': 0}},
                'time': {'created': 1, 'updated': 1}}
        self.sessions[sid] = info
        self.messages[sid] = []
        self.permissions[sid] = []
        self.forms[sid] = []
        self.inbox[sid] = []
        with closing(sqlite3.connect(self.db)) as connection, connection:
            connection.execute('INSERT INTO session_v2 '
                               '(id,project_id,slug,directory,title,version,agent,model,time_created,time_updated) '
                               'VALUES (?,?,?,?,?,?,?,?,?,?)',
                               (sid, 'sdkproject', sid, cwd, info['title'], '2.0.32', 'build',
                                json.dumps(info['model']), 1, 1))
        return info

    def append(self, sid, record):
        with self.lock:
            previous = next((i for i, row in enumerate(self.messages[sid]) if row['id'] == record['id']), None)
            if previous is None:
                self.messages[sid].append(record)
            else:
                self.messages[sid][previous] = record
            # Keep ordering stable, even while one assistant row streams.
            seq = next(i for i, row in enumerate(self.messages[sid]) if row['id'] == record['id']) + 1
            data = {k: v for k, v in record.items() if k not in ('id', 'type')}
            with closing(sqlite3.connect(self.db)) as connection, connection:
                connection.execute('INSERT INTO session_message VALUES (?,?,?,?,?,?,?) '
                                   'ON CONFLICT(id) DO UPDATE SET data=excluded.data,time_updated=excluded.time_updated',
                                   (record['id'], sid, record['type'], seq, seq, time.time_ns() // 1000000,
                                    json.dumps(data)))

    def projected(self, sid):
        rows = self.messages[sid]
        boundary = self.sessions[sid].get('revert', {}).get('messageID')
        if boundary:
            index = next((i for i, row in enumerate(rows) if row['id'] == boundary), len(rows))
            rows = rows[:index]
        return rows

    def sync_projection(self, sid):
        # The ordinary read-only mirror must observe native revert projection,
        # rather than displaying stale messages removed by the native boundary.
        with closing(sqlite3.connect(self.db)) as connection, connection:
            connection.execute('DELETE FROM session_message WHERE session_id=?', (sid,))
        for record in list(self.projected(sid)):
            self.append(sid, record)

    def __enter__(self):
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                self.dispatch()

            def do_POST(self):
                self.dispatch()

            def do_PATCH(self):
                self.dispatch()

            def do_DELETE(self):
                self.dispatch()

            def dispatch(self):
                body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                data = json.loads(body) if body else None
                with fixture.lock:
                    fixture.received.append((self.command, self.path, data))
                    status, response = fixture.handle(self.command, self.path, data)
                    drop_ack = fixture.lose_ack and self.command == 'POST' and urlsplit(self.path).path.endswith('/' + fixture.lose_ack)
                    if drop_ack:
                        fixture.lose_ack = None
                if drop_ack:
                    self.connection.shutdown(socket.SHUT_RDWR)
                    self.connection.close()
                    return
                encoded = b'' if response is None else json.dumps(response).encode()
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(encoded)))
                self.end_headers()
                self.wfile.write(encoded)

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.url = f'http://127.0.0.1:{self.server.server_port}'
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.closed.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(5)
        for thread in self.threads:
            thread.join(5)

    def profile(self):
        home = self.root / 'home'
        home.mkdir(exist_ok=True)
        return {'id': 'opencode-sdk-fake', 'source': 'opencode',
                'executable': sys.executable, 'args': [str(Path(__file__).resolve()), '--fake-cli'],
                'env': {'FAKE_OPENCODE_SDK_URL': self.url, 'HOME': str(home),
                        'OPENCODE_PASSWORD': 'synthetic-secret-do-not-leak'},
                'env_remove': ['OPENCODE_SERVER_PASSWORD']}

    def handle(self, method, raw_path, body):
        parsed = urlsplit(raw_path)
        path, query = parsed.path, parse_qs(parsed.query)
        parts = path.strip('/').split('/')
        if path == '/api/session' and method == 'POST':
            sid = body.get('id', 'ses_synthetic_created')
            if sid in self.sessions:
                return 409, {'message': 'session already exists'}
            return 200, {'data': self.create(sid, body['location']['directory'], body.get('model'))}
        if path == '/api/session/active':
            return 200, {'data': {sid: {'type': 'running'} for sid in self.active}}
        if path == '/api/model':
            return 200, {'location': {'directory': str(self.cwd)}, 'data': [
                {'id': MODEL['id'], 'providerID': MODEL['providerID'], 'name': 'Synthetic OpenCode',
                 'variants': [{'id': 'low', 'name': 'Low'}, {'id': 'high', 'name': 'High'}], 'enabled': True,
                 'limit': {'context': 100000, 'input': 90000, 'output': 10000},
                 'capabilities': {'input': ['text', 'image'], 'output': ['text'], 'tools': True}}]}
        if path == '/api/agent':
            return 200, {'data': [{'id': 'build', 'name': 'Build', 'mode': 'primary', 'hidden': False},
                                   {'id': 'plan', 'name': 'Plan', 'mode': 'primary', 'hidden': False}]}
        if path == '/api/command':
            return 200, {'data': [{'name': 'synthetic-command', 'description': 'Synthetic native callback'}]}
        if len(parts) < 3 or parts[2] not in self.sessions:
            return 404, {'message': 'Session not found', '_tag': 'SessionNotFoundError'}
        sid = parts[2]
        operation = parts[3:]
        if not operation and method == 'PATCH':
            self.sessions[sid].update(body)
            return 204, None
        if not operation:
            return 200, {'data': self.sessions[sid]}
        if operation == ['agent']:
            self.sessions[sid]['agent'] = body['agent']
            return 204, None
        if operation == ['fork']:
            self.forks += 1
            fork = f'ses_synthetic_fork_{self.forks}'
            info = self.create(fork, self.sessions[sid]['location']['directory'], self.sessions[sid]['model'])
            for index, record in enumerate(self.projected(sid)):
                if record['id'] == body.get('before'):
                    break
                self.append(fork, dict(record, id=f'msg_fork_{self.forks}_{index}'))
            return 200, {'data': info}
        if operation == ['revert', 'stage']:
            assert body['files'] is False, 'conversation revert must never touch files'
            self.sessions[sid]['revert'] = {'messageID': body['messageID']}
            self.sync_projection(sid)
            return 200, {'data': self.sessions[sid]['revert']}
        if operation == ['revert'] and method == 'DELETE':
            self.sessions[sid].pop('revert', None)
            self.sync_projection(sid)
            return 204, None
        if operation == ['revert', 'commit']:
            boundary = self.sessions[sid].pop('revert', {}).get('messageID')
            index = next((i for i, row in enumerate(self.messages[sid]) if row['id'] == boundary), len(self.messages[sid]))
            self.messages[sid] = self.messages[sid][:index]
            self.sync_projection(sid)
            return 204, None
        if operation == ['compact']:
            mid = body['id']
            record = {'id': mid, 'type': 'compaction', 'status': 'completed', 'reason': 'manual',
                      'summary': 'synthetic native compact', 'recent': '', 'time': {'created': time.time() * 1000}}
            self.append(sid, record)
            return 200, {'data': {'id': mid, 'type': 'compaction', 'delivery': body['delivery'], 'payload': {}}}
        if operation == ['shell']:
            self.append(sid, {'id': body['id'], 'type': 'shell', 'command': body['command'],
                              'shellID': 'shell_synthetic', 'status': 'exited', 'exit': 0,
                              'output': {'output': 'synthetic shell output', 'cursor': 22, 'size': 22, 'truncated': False},
                              'time': {'created': time.time() * 1000}})
            return 204, None
        if operation == ['command']:
            self.append(sid, {'id': f'msg_command_{len(self.messages[sid])}', 'type': 'system',
                              'text': f"native command: {body['name']} {body['text']}",
                              'time': {'created': time.time() * 1000}})
            return 204, None
        if operation == ['model']:
            self.sessions[sid]['model'] = body['model']
            return 204, None
        if operation == ['message']:
            # Always paginate: a bridge that forgets cursor.next loses data.
            start = int(query.get('cursor', ['0'])[0])
            rows = list(reversed(self.projected(sid)))
            end = min(start + 2, len(rows))
            return 200, {'data': rows[start:end], 'cursor': {'previous': None,
                        'next': str(end) if end < len(rows) else None}}
        if operation == ['inbox']:
            return 200, {'data': self.inbox[sid]}
        if len(operation) == 2 and operation[0] == 'inbox':
            if method == 'DELETE':
                self.inbox[sid] = [entry for entry in self.inbox[sid] if entry['id'] != operation[1]]
            elif method == 'PATCH':
                entry = next((entry for entry in self.inbox[sid] if entry['id'] == operation[1]), None)
                if not entry:
                    return 409, {'message': 'input already delivered'}
                entry['delivery'] = body['delivery']
            return 204, None
        if operation == ['permission']:
            return 200, {'data': self.permissions[sid]}
        if operation == ['form']:
            return 200, {'data': self.forms[sid]}
        if len(operation) == 3 and operation[2] == 'reply':
            key = (sid, operation[0], operation[1])
            if key not in self.pending:
                return 404, {'message': 'request no longer pending'}
            collection = self.permissions if operation[0] == 'permission' else self.forms
            collection[sid] = [r for r in collection[sid] if r['id'] != operation[1]]
            self.pending.pop(key).set()
            return 204, None
        if operation == ['interrupt']:
            was_running = sid in self.active
            if sid in self.turns:
                self.turns[sid][1].set()
            self.active.discard(sid)
            for key, event in list(self.pending.items()):
                if key[0] == sid:
                    event.set()
            self.permissions[sid] = []
            self.forms[sid] = []
            return 200, {'interrupted': was_running}
        if operation == ['prompt']:
            mid = body['id']
            if any(row['id'] == mid for row in self.messages[sid]):
                return 409, {'message': 'duplicate native message'}
            assert body.get('delivery') in ('queue', 'steer'), 'bridge must choose explicit native delivery'
            if sid in self.active:
                entry = {'id': mid, 'sessionID': sid, 'type': 'user', 'payload': {'text': body['text'], 'files': body.get('files', [])},
                         'delivery': body['delivery'], 'time': {'created': time.time() * 1000}}
                self.inbox[sid].append(entry)
                return 200, {'data': entry}
            self.append(sid, {'id': mid, 'type': 'user', 'text': body['text'], 'files': body.get('files', []),
                              'time': {'created': time.time() * 1000}})
            self.active.add(sid)
            cancelled = threading.Event()
            self.turns[sid] = (mid, cancelled)
            thread = threading.Thread(target=self.reply, args=(sid, mid, body['text'], cancelled), daemon=True)
            self.threads.append(thread)
            thread.start()
            return 200, {'data': {'id': mid, 'sessionID': sid, 'type': 'user',
                                 'payload': {'text': body['text']}, 'delivery': 'queue',
                                 'time': {'created': time.time() * 1000}}}
        return 404, {'message': f'Unknown native operation {operation}'}

    def reply(self, sid, mid, text, cancelled):
        assistant = {'id': mid + '_reply', 'type': 'assistant', 'agent': 'build',
                     'model': self.sessions[sid]['model'], 'time': {'created': time.time() * 1000},
                     'content': [{'type': 'reasoning', 'text': 'synthetic reasoning'},
                                 {'type': 'text', 'text': 'partial native reply'}]}
        self.append(sid, assistant)
        if text == 'first SDK turn':
            self.stream_release.wait(15)
        elif self.closed.wait(0.8):
            return
        if text in ('approval', 'question'):
            kind = 'permission' if text == 'approval' else 'form'
            rid = 'permission_native' if kind == 'permission' else 'form_native'
            event = threading.Event()
            with self.lock:
                self.pending[(sid, kind, rid)] = event
                if kind == 'permission':
                    self.permissions[sid] = [{'id': rid, 'sessionID': sid, 'action': 'shell',
                                              'resources': ['synthetic command'], 'message': 'native approval survives reload'}]
                else:
                    self.forms[sid] = [{'id': rid, 'sessionID': sid, 'title': 'native question',
                                       'fields': [{'key': 'choice', 'type': 'string', 'title': 'Choose',
                                                   'options': [{'label': 'First', 'value': 'first'},
                                                               {'label': 'Second', 'value': 'second'}]}]}]
            while not event.wait(0.1):
                if self.closed.is_set():
                    return
        if text.startswith('slow'):
            while not self.closed.wait(0.1):
                if cancelled.is_set():
                    break
        with self.lock:
            interrupted = cancelled.is_set()
            assistant['content'].append({'type': 'tool', 'id': mid + '_tool', 'name': 'read',
                                         'time': {'created': 1}, 'state': {'status': 'error',
                                             'input': {'path': 'synthetic-file'}, 'error': 'synthetic tool error'}})
            assistant['content'][1]['text'] = 'native reply completed: ' + text
            assistant['time']['completed'] = time.time() * 1000
            assistant['tokens'] = {'input': 1200, 'output': 100, 'reasoning': 40, 'cache': {'read': 600, 'write': 200}}
            assistant['cost'] = 0.0125
            info = self.sessions[sid]
            info['cost'] += assistant['cost']
            for key in ('input', 'output', 'reasoning'):
                info['tokens'][key] += assistant['tokens'][key]
            for key in ('read', 'write'):
                info['tokens']['cache'][key] += assistant['tokens']['cache'][key]
            assistant['finish'] = 'error' if interrupted or text == 'error' else 'stop'
            if interrupted or text == 'error':
                assistant['error'] = {'type': 'aborted' if interrupted else 'provider.synthetic',
                                      'message': 'interrupted' if interrupted else 'synthetic provider failure'}
            self.append(sid, assistant)
            if self.turns.get(sid, (None,))[0] == mid:
                self.active.discard(sid)


def fake_cli():
    args = sys.argv[2:]
    if args == ['--version']:
        print('opencode v2.0.32')
        return
    if args == ['service', 'get', 'disabled']:
        print('false')
        return
    if args == ['service', 'status']:
        print(os.environ['FAKE_OPENCODE_SDK_URL'])
        return
    if not args or args[0] != 'api':
        raise SystemExit('fixture accepts only api machine requests')
    method, path = args[1:3]
    data = args[args.index('--data') + 1].encode() if '--data' in args else None
    url = os.environ['FAKE_OPENCODE_SDK_URL']
    assert urlsplit(url).hostname == '127.0.0.1', 'fixture must stay on loopback'
    request = Request(url + path, method=method, data=data, headers={'Content-Type': 'application/json'})
    opener = build_opener(ProxyHandler({}))
    try:
        with opener.open(request, timeout=10) as response:
            sys.stdout.buffer.write(response.read())
    except HTTPError as error:
        sys.stdout.buffer.write(error.read())
        print(f'HTTP {error.code}', file=sys.stderr)
        raise SystemExit(1)


if __name__ == '__main__':
    fake_cli()
