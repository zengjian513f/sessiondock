#!/usr/bin/env python3
"""Opt-in native Codex migration probe; private homes, Luna low, no production sessions.

Creates paginated forks and a same-thread revert through the real app-server,
copies only native history files, then checks native discovery, resume, history,
and metadata separately. This is a prerequisite experiment, not an assertion
that SessionDock migration is implemented. The JSON report records unsupported
or missing capabilities explicitly. Run directly or with --include-real.
"""
import argparse
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import sqlite3
import subprocess
import tempfile
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


def import_experimental_metadata(source, target, sids, rollback=False):
    """Same-schema isolated experiment only; never a production import API.

    Native backfill must first create the target thread rows. Copy only named
    metadata and selected threads' projections; preserve unrelated rows. Schema
    inspection is mandatory; the result is not a cross-version SQL contract.
    """
    imported = {}
    for filename in ('state_5.sqlite', 'thread_history_1.sqlite'):
        src = sqlite3.connect(f'file:{source / filename}?mode=ro', uri=True)
        dst = sqlite3.connect(target / filename)
        try:
            before = list(dst.iterdump()) if rollback else None
            dst.execute('PRAGMA foreign_keys=ON')
            dst.execute('BEGIN IMMEDIATE')

            def rows(table, key, ids, replace=False):
                quote = lambda name: '"' + name.replace('"', '""') + '"'
                schema = src.execute('PRAGMA table_info(' + quote(table) + ')').fetchall()
                assert schema and schema == dst.execute('PRAGMA table_info(' + quote(table) + ')').fetchall(), table
                columns = ','.join(quote(col[1]) for col in schema)
                count = 0
                for identity in ids:
                    records = src.execute(f'SELECT {columns} FROM {quote(table)} WHERE {quote(key)}=?',
                        (identity,)).fetchall()
                    existing = dst.execute(f'SELECT {columns} FROM {quote(table)} WHERE {quote(key)}=?',
                        (identity,)).fetchall()
                    if not replace and existing:
                        assert existing == records, f'conflicting {table} record'
                        continue
                    if replace:
                        dst.execute(f'DELETE FROM {quote(table)} WHERE {quote(key)}=?', (identity,))
                    dst.executemany(f'INSERT INTO {quote(table)} ({columns}) VALUES (' +
                        ','.join('?' for _ in schema) + ')', records)
                    count += len(records)
                imported[table] = count

            if filename == 'state_5.sqlite':
                projects = {native_metadata(source, sid)['project_id'] for sid in sids} - {None}
                rows('projects', 'id', projects)
                rows('project_roots', 'project_id', projects)
                rows('thread_dynamic_tools', 'thread_id', sids, replace=True)
                fields = ('name', 'is_pinned', 'project_id')
                source_columns = src.execute('PRAGMA table_info(threads)').fetchall()
                assert source_columns == dst.execute('PRAGMA table_info(threads)').fetchall()
                for sid in sids:
                    metadata = native_metadata(source, sid)
                    result = dst.execute('UPDATE threads SET name=?,is_pinned=?,project_id=? WHERE id=?',
                        tuple(metadata[key] for key in fields) + (sid,))
                    assert result.rowcount == 1, 'native backfill did not create thread row'
            else:
                for table in ('thread_turns', 'thread_items', 'thread_history_projection_state'):
                    rows(table, 'thread_id', sids, replace=True)
            if rollback:
                dst.rollback()
                assert list(dst.iterdump()) == before, f'{filename} rollback changed original data'
            else:
                dst.commit()
        finally:
            src.close()
            dst.close()
    return imported


def probe(binary, root, auth):
    cwd = root / 'cwd'
    cwd.mkdir()
    homes = [root / name for name in ('A', 'B')]
    for home in homes:
        home.mkdir(mode=0o700)
        # Reuse the existing controlled login; never read or print auth contents.
        (home / 'auth.json').symlink_to(auth)
    source, target = homes
    common = {'model': MODEL, 'config': {'model_reasoning_effort': EFFORT},
        'cwd': str(cwd), 'approvalPolicy': 'never', 'sandbox': 'read-only'}
    report = {'cli': subprocess.check_output([binary, '--version'], text=True).strip(),
        'model': MODEL, 'effort': EFFORT, 'source': {}, 'copied': {}, 'checks': {}}
    tools = [{'type': 'function', 'name': 'migration_probe_echo',
        'description': 'Probe definition; do not invoke.',
        'inputSchema': {'type': 'object', 'properties': {}}}]
    with AppServer(binary, source, cwd, root / 'source.log') as server:
        parent = server.call('thread/start', {**common, 'historyMode': 'paginated',
            'dynamicTools': tools})['thread']
        sid = parent['id']
        server.turn(sid, 'PARENT_ONE')
        server.turn(sid, 'PARENT_TWO')
        child = server.call('thread/fork', {**common, 'threadId': sid,
            'excludeTurns': True})['thread']
        sibling = server.call('thread/fork', {**common, 'threadId': sid,
            'excludeTurns': True})['thread']
        server.turn(child['id'], 'CHILD_ONE')
        cutoff = server.turn(child['id'], 'CHILD_TWO')
        reverted = server.call('thread/revert', {'threadId': child['id'],
            'beforeTurnId': cutoff})['thread']
        assert reverted['id'] == child['id'] and reverted['path'] != child['path']
        grandchild = server.call('thread/fork', {**common, 'threadId': child['id'],
            'excludeTurns': True})['thread']
        server.turn(grandchild['id'], 'GRANDCHILD_ONE')
        server.turn(sid, prompt='This is an authorized isolated native CLI subagent test. '
            'Use spawn_agent to create exactly one subagent named migration_probe, with fresh context, '
            'model gpt-5.6-luna and reasoning effort low (or inherit these current settings if the '
            'tool has no override). Its only task is to reply SUBAGENT_READY without tools. '
            'If spawn_agent is deferred, first discover it using tool search. '
            'Wait for its completion, then reply SUBAGENT_DONE. Do not use shell or web tools.')
        subagents = [row for row in server.listing() if row.get('parentThreadId') == sid]
        assert len(subagents) == 1, 'native model did not create the required single subagent'
        server.call('thread/name/set', {'threadId': child['id'], 'name': 'Move probe child'})
        project = server.call('project/create', {'idempotencyKey': 'move-probe',
            'name': 'Move probe project', 'roots': [{'path': str(cwd)}]})
        project_id = project['project']['id']
        server.call('thread/metadata/update', {'threadId': child['id'], 'projectId': project_id})
        server.call('thread/archive', {'threadId': sibling['id']})
        for role, thread in [('parent', parent), ('child', reverted), ('sibling', sibling),
                ('grandchild', grandchild), ('subagent', subagents[0])]:
            current = server.call('thread/read', {'threadId': thread['id']})['thread']
            report['source'][role] = {'sid': thread['id'], 'name': current.get('name'),
                'project_id': current.get('projectId'),
                'parent_thread_id': current.get('parentThreadId'),
                'path': str(Path(current['path']).relative_to(source)),
                'turns': [turn['id'] for turn in server.pages(thread['id'])]}
    # Compare a restarted source, not only the creating process's live history.
    with AppServer(binary, source, cwd, root / 'source-cold.log') as server:
        report['source_cold'] = {}
        source_pages = {}
        for role, expected in report['source'].items():
            source_pages[role] = server.pages(expected['sid'])
            report['source_cold'][role] = [turn['id'] for turn in source_pages[role]]
    # No pin mutation RPC is exposed by this version. Set this synthetic
    # fixture bit only after the native process has stopped, then inspect it.
    with sqlite3.connect(source / 'state_5.sqlite') as db:
        db.execute('UPDATE threads SET is_pinned=1 WHERE id=?', (child['id'],))
    files = inventory(source)
    assert any(file['history_base'] for file in files), 'fixture has no physical history dependency'
    assert sum(file['sid'] == child['id'] for file in files) >= 2, 'fixture has no rollout rotation'
    identities = {file['rollout_id']: file for file in files}
    for file in files:
        if base := file['history_base']:
            inherited = identities[base['thread_id']]
            raw = (source / inherited['path']).read_bytes()
            cut = base['end_byte_offset']
            assert cut <= len(raw) and (cut == 0 or raw[cut - 1:cut] == b'\n')
    report['files'] = files
    for directory in ('sessions', 'archived_sessions'):
        if (source / directory).exists():
            shutil.copytree(source / directory, target / directory)
    assert not list(target.glob('*.sqlite')), 'destination must start without state databases'
    assert inventory(target) == files, 'copy changed native file bytes'
    with AppServer(binary, target, cwd, root / 'target.log') as server:
        visible = server.listing()
        archived = server.listing(True)
        report['checks']['native_active_ids'] = [thread['id'] for thread in visible]
        report['checks']['native_archived_ids'] = [thread['id'] for thread in archived]
        for role, expected in report['source'].items():
            if role == 'sibling':
                actual = server.call('thread/read', {'threadId': expected['sid']})['thread']
                report['copied'][role] = {'archived': any(t['id'] == expected['sid'] for t in archived),
                    'same_history': [t['id'] for t in server.pages(expected['sid'])] == expected['turns'],
                    'same_current_rollout': str(Path(actual['path']).relative_to(target)) == expected['path']}
                continue
            try:
                result = server.call('thread/resume', {**common,
                    'threadId': expected['sid'], 'excludeTurns': True})
                thread = result['thread']
                actual = {'resume': True, 'name': thread.get('name'),
                    'project_id': thread.get('projectId'),
                    'path': str(Path(thread['path']).relative_to(target)),
                    'turns': [turn['id'] for turn in server.pages(expected['sid'])]}
                actual['same_history'] = actual['turns'] == expected['turns']
                actual['same_current_rollout'] = actual['path'] == expected['path']
                actual['same_metadata'] = all(actual[k] == expected[k] for k in ('name', 'project_id'))
                report['copied'][role] = actual
            except RpcError as error:
                report['copied'][role] = {'resume': False, 'error': str(error)}
        sentinel = server.call('thread/start', {**common, 'historyMode': 'paginated'})['thread']
        server.turn(sentinel['id'], 'TARGET_UNRELATED')
        server.call('thread/name/set', {'threadId': sentinel['id'], 'name': 'Keep target session'})
    sentinel_before = native_metadata(target, sentinel['id'])
    sids = [row['sid'] for row in report['source'].values()]
    import_experimental_metadata(source, target, sids, rollback=True)
    report['checks']['per_database_transaction_rollback'] = True
    report['imported_rows'] = import_experimental_metadata(source, target, sids)
    assert native_metadata(target, sentinel['id']) == sentinel_before
    report['checks']['unrelated_target_metadata_preserved'] = True
    with AppServer(binary, target, cwd, root / 'target-imported.log') as server:
        report['after_import'] = {}
        active_ids = {thread['id'] for thread in server.listing()}
        archived_ids = {thread['id'] for thread in server.listing(True)}
        for role, expected in report['source'].items():
            pages = server.pages(expected['sid'])
            turns = [turn['id'] for turn in pages]
            current = server.call('thread/read', {'threadId': expected['sid']})['thread']
            metadata = native_metadata(target, expected['sid'])
            report['after_import'][role] = {'same_history': turns == report['source_cold'][role],
                'native_listed': expected['sid'] in (archived_ids if role == 'sibling' else active_ids),
                'same_turn_contents': pages == source_pages[role],
                'same_current_rollout': str(Path(current['path']).relative_to(target)) == expected['path'],
                'same_parent': current.get('parentThreadId') == expected['parent_thread_id'],
                'same_metadata': metadata == native_metadata(source, expected['sid'])}
            assert all(report['after_import'][role].values()), report['after_import'][role]
        assert any(t['id'] == sentinel['id'] for t in server.listing())
        server.call('thread/resume', {**common, 'threadId': child['id'], 'excludeTurns': True})
        resumed_turn = server.turn(child['id'], 'CHILD_RESUMED')
        assert resumed_turn in [t['id'] for t in server.pages(child['id'])]
        report['checks']['native_continue_after_import'] = True
    assert inventory(source) == files, 'source histories changed during destination resume'
    report['checks']['source_bytes_unchanged'] = True
    report['checks']['pin_import'] = native_metadata(target, child['id'])['is_pinned'] == 1
    inventory(target)  # Assert the actual model and effort of all continued turns.
    report['checks']['subagent_import'] = report['after_import']['subagent']
    report['checks']['dynamic_tool_execution'] = 'not_executed'
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', default=shutil.which('codex'))
    parser.add_argument('--output', type=Path,
        default=Path(__file__).resolve().parents[1] / 'target/session-move-codex-report.json',
        help='JSON evidence report')
    parser.add_argument('--keep-workspace', action='store_true', help='retain synthetic files for diagnosis')
    args = parser.parse_args()
    if not args.codex:
        parser.error('codex binary is required')
    real_home = Path(os.environ.get('CODEX_HOME', Path.home() / '.codex'))
    auth = real_home / 'auth.json'
    if not auth.is_file():
        parser.error('existing Codex login is required')
    config = real_home / 'config.toml'
    before = digest(config)
    temporary = Path(tempfile.mkdtemp(prefix='sessiondock-move-codex-'))
    try:
        report = probe(args.codex, temporary, auth.resolve())
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        print(json.dumps({'cli': report['cli'], 'source_cold': report['source_cold'],
            'copied': report['copied'], 'after_import': report['after_import'],
            'checks': report['checks']}, ensure_ascii=False, indent=2))
    finally:
        for home in ('A', 'B'):
            (temporary / home / 'auth.json').unlink(missing_ok=True)
        if args.keep_workspace:
            print('Synthetic workspace:', temporary, flush=True)
        else:
            shutil.rmtree(temporary)
        assert digest(config) == before, 'daily Codex config changed; do not overwrite concurrent edits'


if __name__ == '__main__':
    main()
