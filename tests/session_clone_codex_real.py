#!/usr/bin/env python3
"""Operator-only new-identity clone experiment, Luna low in temporary CLI homes.

Uses the real Rust planner/stager plus a TEST-ONLY same-schema SQL importer.
This validates the import recipe; it does not publish a production clone API.
Code-mode subagent references remain unsupported and are covered by diagnostics
in session_transfer_browser.py, not silently omitted from this experiment.
"""
import argparse
from contextlib import closing
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import tempfile

from session_move_codex_real import AppServer, MODEL, EFFORT, digest, inventory, native_metadata


def transfer(binary, request):
    result = subprocess.run([str(binary)], input=json.dumps(request), text=True,
                            capture_output=True, timeout=45)
    value = json.loads(result.stdout)
    assert result.returncode == 0, value
    return value


def rows(db, table, sids=None):
    columns = [r[1] for r in db.execute(f'PRAGMA table_info("{table}")')]
    assert columns, table
    selected = db.execute(f'SELECT * FROM "{table}"').fetchall()
    return [dict(zip(columns, row)) for row in selected
            if sids is None or dict(zip(columns, row)).get('thread_id', dict(zip(columns, row)).get('id')) in sids]


def remap_item(item, identities):
    result = copy.deepcopy(item)
    result['id'] = identities['records'][result['id']]
    # This experiment only generates plain text user/agent items. Unsupported
    # projection variants require their typed adapter before extending coverage.
    assert result['type'] in ('userMessage', 'agentMessage', 'reasoning'), result['type']
    return result


def import_clone(source, target, plan, staged, rollback=False):
    """Insert only mapped rows. No OR REPLACE, whole DB copy, or production homes."""
    ids = plan['identities']
    sids = set(ids['threads'])
    outputs = {f['source']: f for f in staged['files']}
    source_threads = {}
    with closing(sqlite3.connect(f'file:{source}/state_5.sqlite?mode=ro', uri=True)) as db:
        source_threads = {r['id']: r for r in rows(db, 'threads', sids)}
    assert set(source_threads) == sids

    def offset(sid, old, ordinal=None, end=False):
        if old is None:
            return None
        candidates = []
        for f in plan['files']:
            if f['thread'] != sid:
                continue
            output = outputs[f['source']]
            if str(old) not in output['boundaries']:
                continue
            if ordinal is None:
                if f['source'] != source_threads[sid]['rollout_path']:
                    continue
            else:
                whole = Path(f['source']).read_bytes()
                raw = whole[:old] if end else whole[old:]
                if not raw:
                    continue
                actual = json.loads(raw.splitlines()[-1 if end else 0]).get('ordinal')
                if actual != ordinal:
                    continue
            candidates.append(output['boundaries'][str(old)])
        assert candidates and len(set(candidates)) == 1, (sid, old, ordinal, candidates)
        return candidates[0]

    counts = {}
    for filename, tables in (
        ('state_5.sqlite', ['projects', 'project_roots', 'threads', 'thread_dynamic_tools']),
        ('thread_history_1.sqlite', ['thread_turns', 'thread_items', 'thread_history_projection_state']),
    ):
        with closing(sqlite3.connect(f'file:{source / filename}?mode=ro', uri=True)) as src, \
             closing(sqlite3.connect(target / filename)) as dst:
            before = list(dst.iterdump())
            dst.execute('PRAGMA foreign_keys=ON')
            dst.execute('BEGIN IMMEDIATE')
            projects = {r['project_id'] for r in source_threads.values() if r['project_id']}
            for table in tables:
                assert src.execute(f'PRAGMA table_info("{table}")').fetchall() == \
                       dst.execute(f'PRAGMA table_info("{table}")').fetchall(), (filename, table)
                selected = rows(src, table, None if table in ('projects', 'project_roots') else sids)
                if table == 'projects':
                    selected = [r for r in selected if r['id'] in projects]
                if table == 'project_roots':
                    selected = [r for r in selected if r['project_id'] in projects]
                for record in selected:
                    row = dict(record)
                    if table == 'threads':
                        sid = row['id']
                        row['id'] = ids['threads'][sid]
                        row['rollout_path'] = str(target / outputs[row['rollout_path']]['relative'])
                        assert row['thread_section_id'] is None
                    elif 'thread_id' in row:
                        sid = row['thread_id']
                        row['thread_id'] = ids['threads'][sid]
                    if table in ('thread_turns', 'thread_items'):
                        row['turn_id'] = ids['turns'][row['turn_id']]
                    if table == 'thread_turns':
                        for key in ('first_user_item_id', 'final_agent_item_id'):
                            if row[key]:
                                row[key] = ids['records'][row[key]]
                        row['rollout_byte_offset'] = offset(sid, row['rollout_byte_offset'], row['rollout_ordinal'])
                        row['rollout_end_byte_offset'] = offset(sid, row['rollout_end_byte_offset'], row['rollout_end_ordinal'], end=True)
                    elif table == 'thread_items':
                        row['item_id'] = ids['records'][row['item_id']]
                        row['item_json'] = json.dumps(remap_item(json.loads(row['item_json']), ids))
                    elif table == 'thread_history_projection_state':
                        row['next_rollout_byte_offset'] = (offset(sid, row['next_rollout_byte_offset'],
                            row['next_rollout_ordinal'] - 1, end=True) if row['next_rollout_byte_offset'] else 0)
                    columns = ','.join('"' + key + '"' for key in row)
                    marks = ','.join('?' for _ in row)
                    dst.execute(f'INSERT INTO "{table}" ({columns}) VALUES ({marks})', list(row.values()))
                counts[table] = len(selected)
            if rollback:
                dst.rollback()
                assert list(dst.iterdump()) == before, filename
            else:
                dst.commit()
    return counts


def probe(binary, transfer_binary, root, auth):
    cwd = root / 'cwd'
    cwd.mkdir()
    source, target = root / 'A', root / 'B'
    for home in (source, target):
        home.mkdir(mode=0o700)
        (home / 'auth.json').symlink_to(auth)
    common = {'model': MODEL, 'config': {'model_reasoning_effort': EFFORT},
              'cwd': str(cwd), 'approvalPolicy': 'never', 'sandbox': 'read-only'}
    with AppServer(binary, source, cwd, root / 'source.log') as server:
        parent = server.call('thread/start', {**common, 'historyMode': 'paginated'})['thread']
        server.turn(parent['id'], 'CLONE_PARENT')
        print('Native turn complete: CLONE_PARENT', flush=True)
        child = server.call('thread/fork', {**common, 'threadId': parent['id'], 'excludeTurns': True})['thread']
        sibling = server.call('thread/fork', {**common, 'threadId': parent['id'], 'excludeTurns': True})['thread']
        server.turn(child['id'], 'CLONE_CHILD_KEEP')
        print('Native turn complete: CLONE_CHILD_KEEP', flush=True)
        removed = server.turn(child['id'], 'CLONE_CHILD_DISCARD')
        print('Native turn complete: CLONE_CHILD_DISCARD', flush=True)
        child = server.call('thread/revert', {'threadId': child['id'], 'beforeTurnId': removed})['thread']
        grandchild = server.call('thread/fork', {**common, 'threadId': child['id'], 'excludeTurns': True})['thread']
        server.turn(grandchild['id'], 'CLONE_GRANDCHILD')
        print('Native turn complete: CLONE_GRANDCHILD', flush=True)
        server.call('thread/name/set', {'threadId': child['id'], 'name': 'Clone child name'})
        project = server.call('project/create', {'idempotencyKey': 'clone-probe', 'name': 'Clone project',
                                                'roots': [{'path': str(cwd)}]})['project']
        server.call('thread/metadata/update', {'threadId': child['id'], 'projectId': project['id']})
        server.call('thread/archive', {'threadId': sibling['id']})
    with sqlite3.connect(source / 'state_5.sqlite') as db:
        db.execute('UPDATE threads SET is_pinned=1 WHERE id=?', (child['id'],))
    selected = [parent, child, sibling, grandchild]
    with AppServer(binary, source, cwd, root / 'source-cold.log') as server:
        selected = [server.call('thread/read', {'threadId': t['id']})['thread'] for t in selected]
        expected = {t['id']: server.pages(t['id']) for t in selected}
    # Initialize the target natively before inserting any clone files or rows.
    with AppServer(binary, target, cwd, root / 'target-init.log') as server:
        sentinel = server.call('thread/start', {**common, 'historyMode': 'paginated'})['thread']
        server.turn(sentinel['id'], 'UNRELATED_TARGET')
        print('Native turn complete: UNRELATED_TARGET', flush=True)
    sentinel_before = inventory(target)
    sentinel_meta = native_metadata(target, sentinel['id'])
    original = inventory(source)
    uid = 'codex:' + hashlib.sha1(parent['path'].encode()).hexdigest()[:16]
    plan = transfer(transfer_binary, {'operation': 'plan_codex', 'uid': uid,
                                     'roots': {'codex': str(source)}, 'mode': 'clone'})
    assert not plan['reference_issues'], plan['reference_issues']
    assert set(plan['identities']['threads']) == set(expected)
    stage = root / 'staging'
    staged = transfer(transfer_binary, {'operation': 'stage_codex', 'plan': plan, 'destination': str(stage)})
    for file in staged['files']:
        destination = target / file['relative']
        destination.parent.mkdir(parents=True, exist_ok=True)
        with destination.open('xb') as stream:
            stream.write((stage / file['relative']).read_bytes())
    import_clone(source, target, plan, staged, rollback=True)
    imported = import_clone(source, target, plan, staged)
    assert native_metadata(target, sentinel['id']) == sentinel_meta
    identities = plan['identities']
    print('PASS native fixture, Rust staging, new-ID SQL import and per-database rollback', flush=True)
    with AppServer(binary, target, cwd, root / 'clone-resume.log') as server:
        active = {t['id'] for t in server.listing()}
        archived = {t['id'] for t in server.listing(True)}
        for thread in selected:
            sid = thread['id']
            new = identities['threads'][sid]
            assert new in (archived if sid == sibling['id'] else active)
            pages = copy.deepcopy(expected[sid])
            for turn in pages:
                turn['id'] = identities['turns'][turn['id']]
                turn['items'] = [remap_item(i, identities) for i in turn['items']]
            assert server.pages(new) == pages, (sid, 'new-ID history differs')
            assert native_metadata(target, new) == native_metadata(source, sid)
            current = server.call('thread/read', {'threadId': new})['thread']
            original_file = next(f for f in staged['files'] if f['source'] == thread['path'])
            assert current['path'] == str(target / original_file['relative'])
        new_child = identities['threads'][child['id']]
        server.call('thread/resume', {**common, 'threadId': new_child, 'excludeTurns': True})
        clone_turn = server.turn(new_child, 'ONLY_NEW_CLONE')
        assert clone_turn in [t['id'] for t in server.pages(new_child)]
    assert inventory(source) == original, 'clone modified source histories'
    target_after = inventory(target)
    assert [f for f in target_after if f['sid'] == sentinel['id']] == sentinel_before
    with AppServer(binary, source, cwd, root / 'source-continue.log') as server:
        server.call('thread/resume', {**common, 'threadId': child['id'], 'excludeTurns': True})
        source_turn = server.turn(child['id'], 'ONLY_OLD_SOURCE')
        assert source_turn in [t['id'] for t in server.pages(child['id'])]
        assert clone_turn not in [t['id'] for t in server.pages(child['id'])]
    assert inventory(target) == target_after, 'source continuation modified clone'
    inventory(source)  # Enforce actual model/effort after continuing the source.
    print('PASS native new-ID list/history/metadata/current rollout; both groups continue independently', flush=True)
    return {'cli': subprocess.check_output([binary, '--version'], text=True).strip(),
            'model': MODEL, 'effort': EFFORT, 'identities': identities, 'imported_rows': imported,
            'checks': {'history_contents': True, 'native_listing': True, 'metadata': True,
                       'current_rollout': True, 'per_database_rollback': True,
                       'source_preserved_during_clone': True, 'unrelated_target': True,
                       'independent_continuation': True},
            'limitations': ['same schema only', 'no code-mode subagent import', 'no production publish or recovery']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', default=shutil.which('codex'))
    parser.add_argument('--binary', type=Path, default=Path('target/debug/sessiondock'))
    parser.add_argument('--transfer-binary', type=Path)
    parser.add_argument('--keep-workspace', action='store_true')
    parser.add_argument('--output', type=Path, default=Path('target/session-clone-codex-report.json'))
    args = parser.parse_args()
    home = Path(os.environ.get('CODEX_HOME', Path.home() / '.codex'))
    auth = home / 'auth.json'
    if not args.codex or not auth.is_file():
        parser.error('Codex and existing controlled login required')
    before = digest(home / 'config.toml')
    root = Path(tempfile.mkdtemp(prefix='sessiondock-clone-codex-'))
    print('Isolated workspace:', root, flush=True)
    try:
        report = probe(args.codex, args.transfer_binary or args.binary.with_name('sessiondock-transfer'),
                       root, auth.resolve())
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    finally:
        for name in ('A', 'B'):
            (root / name / 'auth.json').unlink(missing_ok=True)
        if not args.keep_workspace:
            shutil.rmtree(root)
        assert digest(home / 'config.toml') == before, 'daily configuration changed; preserve concurrent edits'


if __name__ == '__main__':
    main()
