"""Shared fixture helpers extracted from the former `session_move_codex_real.py` (removed 2026-10-06
with the non-browser suites); imported by browser suites."""
# run_validation: skip
from contextlib import closing
import hashlib
import json
import os
import queue
import sqlite3
import subprocess
import threading
import time


MODEL = 'gpt-5.6-luna'


EFFORT = 'low'


class RpcError(RuntimeError):
    pass


class AppServer:
    def __init__(self, binary, home, cwd, log):
        self.serial = 0
        self.events = []
        self.incoming = queue.Queue()
        env = {k: v for k, v in os.environ.items() if k in (
            'PATH', 'LANG', 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY',
            'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')}
        env.update(HOME=str(home), CODEX_HOME=str(home))
        self.log = log.open('w')
        self.process = subprocess.Popen([
            binary, '-c', f'model="{MODEL}"', '-c', f'model_reasoning_effort="{EFFORT}"',
            '-c', 'approval_policy="never"', '-c', 'sandbox_mode="read-only"',
            '-c', 'features.multi_agent=true',
            'app-server', '--stdio'], cwd=cwd, env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log, text=True)

        def read():
            try:
                for line in self.process.stdout:
                    self.incoming.put(json.loads(line))
            except Exception as error:
                self.incoming.put(error)
            finally:
                self.incoming.put(EOFError('app-server exited'))

        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()
        try:
            self.call('initialize', {'clientInfo': {'name': 'sessiondock_move_probe',
                'version': '1'}, 'capabilities': {'experimentalApi': True}})
            self.send({'method': 'initialized'})
        except BaseException:
            self.close()
            raise

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + '\n')
        self.process.stdin.flush()

    def receive(self, deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError('native app-server response timed out')
        try:
            event = self.incoming.get(timeout=remaining)
        except queue.Empty as error:
            raise TimeoutError('native app-server response timed out') from error
        if isinstance(event, Exception):
            raise event
        if 'method' in event and 'id' in event:
            # Never execute dynamic tools or grant local execution permissions.
            # The native subagent/tool-search fixture runs inside app-server.
            self.send({'id': event['id'], 'error': {'code': -32601,
                'message': 'migration probe does not execute tools'}})
        return event

    def call(self, method, params):
        self.serial += 1
        number = self.serial
        self.send({'id': number, 'method': method, 'params': params})
        deadline = time.monotonic() + 120
        while True:
            event = self.receive(deadline)
            if event.get('id') == number:
                if 'error' in event:
                    raise RpcError(f'{method}: {event["error"]}')
                return event['result']
            self.events.append(event)

    def turn(self, sid, word=None, prompt=None):
        result = self.call('turn/start', {'threadId': sid, 'model': MODEL,
            'effort': EFFORT, 'input': [{'type': 'text',
                'text': prompt or f'Do not use tools. Reply with exactly {word}.', 'text_elements': []}]})
        turn = result['turn']['id']
        deadline = time.monotonic() + 120
        while True:
            for i, event in enumerate(self.events):
                data = event.get('params', {})
                if (event.get('method') == 'turn/completed' and data.get('threadId') == sid
                        and data.get('turn', {}).get('id') == turn):
                    self.events.pop(i)
                    assert data['turn']['status'] == 'completed', data['turn']
                    return turn
            self.events.append(self.receive(deadline))

    def pages(self, sid):
        data, cursor = [], None
        while True:
            result = self.call('thread/turns/list', {'threadId': sid, 'cursor': cursor,
                'limit': 1, 'sortDirection': 'asc', 'itemsView': 'full'})
            data.extend(result['data'])
            cursor = result.get('nextCursor')
            if not cursor:
                return data

    def listing(self, archived=False):
        data, cursor = [], None
        while True:
            result = self.call('thread/list', {'archived': archived, 'cursor': cursor,
                'limit': 1, 'sourceKinds': ['vscode', 'appServer', 'cli', 'subAgent',
                    'subAgentThreadSpawn', 'subAgentOther', 'exec']})
            data.extend(result['data'])
            cursor = result.get('nextCursor')
            if not cursor:
                return data

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.terminate()
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait()
        self.reader.join(timeout=2)
        self.process.stdout.close()
        self.log.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None


def inventory(home):
    files = []
    for directory in ('sessions', 'archived_sessions'):
        for path in sorted((home / directory).rglob('*.jsonl')):
            rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
            meta = next(row['payload'] for row in rows if row['type'] == 'session_meta')
            for row in rows:
                if row['type'] == 'turn_context':
                    assert (row['payload']['model'], row['payload']['effort']) == (MODEL, EFFORT)
            files.append({'path': str(path.relative_to(home)), 'sha256': digest(path),
                'bytes': path.stat().st_size, 'sid': meta['id'],
                'rollout_id': path.stem.rsplit('_', 1)[1] if '_' in path.stem else meta['id'],
                'history_base': meta.get('history_base'),
                'dynamic_tools': meta.get('dynamic_tools'), 'source': meta.get('source')})
    return files


def native_metadata(home, sid):
    with closing(sqlite3.connect(f'file:{home / "state_5.sqlite"}?mode=ro', uri=True)) as db:
        row = db.execute('SELECT name, is_pinned, project_id, archived FROM threads WHERE id=?',
            (sid,)).fetchone()
        return dict(zip(('name', 'is_pinned', 'project_id', 'archived'), row))
