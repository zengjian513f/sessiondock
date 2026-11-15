#!/usr/bin/env python3
"""Chromium list/detail reads across native touches, appends and replacements."""

import argparse
import json
import os
from pathlib import Path
import shutil
import tempfile
import threading
import time
import uuid

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, codex_message, encoded, isolated_server
from frontend_framework_browser import launch_chromium


def node_cpu_ticks(pid: int) -> int:
    raw = Path(f'/proc/{pid}/stat').read_text()
    fields = raw[raw.rfind(')') + 2:].split()
    return int(fields[11]) + int(fields[12])


def verify_large_tree_refresh_and_mutations(binary: Path):
    with tempfile.TemporaryDirectory(prefix='sessiondock-large-index-') as temporary:
        root = Path(temporary)
        corpus = Corpus(root)
        work_cwd = root / 'workspace' / 'deep' / 'project'
        work_cwd.mkdir(parents=True, exist_ok=True)
        grok_root = root / 'grok'
        codex_root = root / 'codex'
        claude_root = root / 'claude'
        proc_root = root / 'proc'
        proc_root.mkdir()
        (proc_root / 'uptime').write_text('100000.00 200000.00\n')
        (proc_root / 'stat').write_text('cpu  1000 0 1000 100000 0 0 0 0 0 0\nbtime 1760000000\n')
        for idx in range(400):
            pid = 20000 + idx
            pdir = proc_root / str(pid)
            pdir.mkdir()
            (pdir / 'cmdline').write_bytes(f'/usr/bin/python3\0worker.py\0--id={idx}\0'.encode())
            (pdir / 'environ').write_bytes(f'HOME={root}\0PATH=/usr/bin\0'.encode())
            (pdir / 'stat').write_text(
                f'{pid} (python3) S 1 {pid} {pid} 0 -1 4194304 10 0 0 0 20 5 0 0 20 0 2 0 500000 10485760 2048 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0\n'
            )
            os.symlink(str(work_cwd), pdir / 'cwd')
            (pdir / 'fd').mkdir()

        chat_bytes = (
            json.dumps({'role': 'user', 'content': 'Synthetic grok user'}) + '\n'
            + json.dumps({'role': 'assistant', 'content': 'Synthetic grok reply'}) + '\n'
        ).encode()
        ev_bytes = b'{"type":"turn_done"}\n'
        side_bytes = b'{"note":"artifact"}\n'
        target_grok_dir = None
        target_grok_sid = None
        for p in range(20):
            pdir = grok_root / f'proj-{p:02d}'
            pdir.mkdir(parents=True)
            for j in range(50):
                n = p * 50 + j
                gsid = f'grok-large-{n:04d}'
                sdir = pdir / gsid
                sdir.mkdir()
                (sdir / 'summary.json').write_bytes(json.dumps({
                    'session_id': gsid,
                    'title': f'Grok large {n}',
                    'created_at': '2026-10-10T08:00:00Z',
                    'updated_at': '2026-10-10T08:05:00Z',
                    'cwd': str(work_cwd),
                }).encode())
                (sdir / 'chat_history.jsonl').write_bytes(chat_bytes)
                (sdir / 'events.jsonl').write_bytes(ev_bytes)
                for k in range(7):
                    (sdir / f'artifact_{k}.json').write_bytes(side_bytes)
                if p == 19 and j == 49:
                    target_grok_dir = sdir
                    target_grok_sid = gsid

        deep_append_path = None
        deep_append_sid = None
        for d in range(10):
            ddir = codex_root / '2026' / '10' / f'{1 + d:02d}' / 'leaf'
            ddir.mkdir(parents=True)
            for j in range(100):
                n = d * 100 + j
                csid = str(uuid.UUID(int=0x300000000000 + n))
                cpath = ddir / f'rollout-2026-10-01T12-00-00-{csid}.jsonl'
                cpath.write_bytes(b''.join(encoded(row) for row in [
                    codex_row('session_meta', {'id': csid, 'cwd': str(work_cwd)}),
                    codex_message('user', f'Codex large {n}'),
                    codex_row('turn_context', {'model': 'initial-large-model'}),
                    codex_row('event_msg', {'type': 'task_complete'}),
                ]))
                if d == 9 and j == 99:
                    deep_append_path = cpath
                    deep_append_sid = csid

        for p in range(10):
            pdir = claude_root / f'-synthetic-claude-{p:02d}'
            pdir.mkdir(parents=True)
            for j in range(100):
                n = p * 100 + j
                lsid = str(uuid.UUID(int=0x400000000000 + n))
                (pdir / f'{lsid}.jsonl').write_bytes((
                    json.dumps({'type': 'user', 'uuid': f'u-{n}', 'parentUuid': None,
                                'sessionId': lsid, 'cwd': str(work_cwd),
                                'timestamp': '2026-10-12T10:00:00Z', 'isSidechain': False,
                                'message': {'role': 'user', 'content': f'Claude large {n}'}}) + '\n'
                ).encode())

        with isolated_server(
            corpus,
            binary,
            extra_env={'SESSIONDOCK_PROC_ROOT': str(proc_root)},
        ) as (base, opener), sync_playwright() as pw:
            browser = launch_chromium(pw)
            try:
                has_proc_stat = os.name != 'nt' and Path(f'/proc/{opener.sessiondock_pid}/stat').exists()
                opener.open(base + '/api/sessions?children=hidden').read()
                while not json.loads(opener.open(base + '/api/sessions/cleanup-counts').read())['ready']:
                    time.sleep(0.05)
                if has_proc_stat:
                    hz = os.sysconf('SC_CLK_TCK')
                    t0 = node_cpu_ticks(opener.sessiondock_pid)
                for _ in range(6):
                    time.sleep(0.55)
                    opener.open(base + '/api/sessions?children=hidden').read()
                if has_proc_stat:
                    list_ticks = node_cpu_ticks(opener.sessiondock_pid) - t0
                    list_ms = list_ticks * 1000.0 / hz
                    assert list_ms <= 80.0, f'unchanged session list refreshes used {list_ms:.1f}ms ({list_ticks} ticks)'

                page = browser.new_page()
                page.goto(base, wait_until='domcontentloaded')
                page.wait_for_function('T.listLoaded && S.sessions.length === 3000', timeout=20000)
                page.evaluate("() => fetch('api/live?force=1').then(r => r.json())")
                if has_proc_stat:
                    tp0 = node_cpu_ticks(opener.sessiondock_pid)
                for _ in range(4):
                    time.sleep(0.55)
                    page.evaluate('() => pollSessions()')
                if has_proc_stat:
                    poll_ticks = node_cpu_ticks(opener.sessiondock_pid) - tp0
                    poll_ms = poll_ticks * 1000.0 / hz
                    assert poll_ms <= 80.0, f'unchanged in-page list polls used {poll_ms:.1f}ms ({poll_ticks} ticks)'

                page.evaluate("() => fetch('api/live?force=1').then(r => r.json())")
                if has_proc_stat:
                    t1 = node_cpu_ticks(opener.sessiondock_pid)
                for _ in range(3):
                    time.sleep(0.55)
                    page.reload(wait_until='domcontentloaded')
                    page.wait_for_function('T.listLoaded && S.sessions.length === 3000', timeout=15000)
                if has_proc_stat:
                    reload_ticks = node_cpu_ticks(opener.sessiondock_pid) - t1
                    reload_ms = reload_ticks * 1000.0 / hz
                    # Repeated refreshes and full page reloads across 12,000 files / 3,000 sessions
                    # past CHECK_TTL must stay bounded when the tree and process table are unchanged.
                    assert reload_ms <= 400.0, f'unchanged reloads used {reload_ms:.1f}ms ({reload_ticks} ticks)'
                    print(
                        f'PASS large unchanged tree CPU delta: 6 list refreshes={list_ms:.1f}ms, '
                        f'4 in-page polls={poll_ms:.1f}ms, 3 browser reloads={reload_ms:.1f}ms',
                        flush=True,
                    )

                # 1. Deep-directory append reflects within existing latency.
                assert deep_append_path is not None and deep_append_sid is not None
                with deep_append_path.open('ab') as handle:
                    handle.write(b''.join(encoded(row) for row in [
                        codex_message('user', 'Deep appended message body'),
                        codex_row('turn_context', {'model': 'deep-appended-model'}),
                        codex_row('event_msg', {'type': 'task_complete'}),
                    ]))
                page.wait_for_function(
                    "sid => S.sessions.find(r => r.sid === sid)?.model === 'deep-appended-model'",
                    arg=deep_append_sid,
                    timeout=5000,
                )
                deep_uid = page.evaluate('sid => S.sessions.find(r => r.sid === sid).uid', deep_append_sid)
                page.goto(f'{base}/?sid={deep_uid}', wait_until='domcontentloaded')
                expect(page.locator('#msgs')).to_contain_text('Deep appended message body')

                # 2. Deep-directory creation reflects within existing latency.
                created_sid = str(uuid.UUID(int=0x399999999999))
                created_dir = codex_root / '2026' / '11' / '15' / 'deep' / 'branch'
                created_dir.mkdir(parents=True)
                created_path = created_dir / f'rollout-2026-11-15T18-00-00-{created_sid}.jsonl'
                created_path.write_bytes(b''.join(encoded(row) for row in [
                    codex_row('session_meta', {'id': created_sid, 'cwd': str(work_cwd)}),
                    codex_message('user', 'Deep newly created session'),
                    codex_row('turn_context', {'model': 'deep-created-model'}),
                    codex_row('event_msg', {'type': 'task_complete'}),
                ]))
                page.wait_for_function(
                    'sid => S.sessions.some(r => r.sid === sid)',
                    arg=created_sid,
                    timeout=5000,
                )

                # 3. Deep-directory deletion reflects within existing latency.
                created_path.unlink()
                assert target_grok_dir is not None and target_grok_sid is not None
                shutil.rmtree(target_grok_dir)
                page.wait_for_function(
                    '({csid, gsid}) => !S.sessions.some(r => r.sid === csid || r.sid === gsid)',
                    arg={'csid': created_sid, 'gsid': target_grok_sid},
                    timeout=5000,
                )
                assert page.evaluate('S.sessions.length') == 2999
                print('PASS large synthetic tree deep append, creation and deletion reflected promptly', flush=True)
            finally:
                browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--rounds', type=int, default=24)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-index-append-') as temporary:
        root = Path(temporary)
        corpus = Corpus(root)
        sid = 'growing-native'
        path = corpus.put(sid, 'codex', [
            codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/append'}),
            codex_message('user', 'Growing native identity'),
            codex_row('turn_context', {'model': 'synthetic-model'}),
            codex_row('event_msg', {'type': 'task_started', 'turn_id': 'active'}),
            codex_row('world_state', {'padding': 'x' * (1200 * 1024)})], [])
        stop = threading.Event()
        growing = threading.Event()
        suffix = encoded(codex_row('world_state', {'padding': 'x' * 16384}))
        def append():
            with path.open('ab', buffering=0) as stream:
                while not stop.is_set():
                    if growing.is_set():
                        stream.write(suffix)
                    # A writer may finish metadata updates after appending.
                    # Touching an otherwise valid prefix must not hide its row.
                    for _ in range(8):
                        os.utime(path, None)
                        if stop.wait(.0005):
                            return
        log = root / 'server.log'
        with isolated_server(corpus, args.binary, log_path=log) as (base, _), sync_playwright() as pw:
            browser = launch_chromium(pw)
            writer = threading.Thread(target=append)
            writer.start()
            try:
                page = browser.new_page()
                for iteration in range(args.rounds):
                    if iteration == 10:
                        growing.set()
                    page.goto(base + '/?sid=codex:' + sid, wait_until='domcontentloaded')
                    row = page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]')
                    expect(row).to_contain_text('Growing native identity')
                    actual = page.evaluate('uid => S.sessions.find(row => row.uid === uid)', corpus.uid(sid))
                    assert actual['sid'] == sid and actual.get('turn') == 'working' and actual['supported'], actual
                    assert actual['model'] == 'synthetic-model', actual
                    if iteration % 4 == 0:
                        print(f'PASS growing native reload {iteration + 1}/{args.rounds}', flush=True)
                stop.set()
                writer.join(timeout=5)
                expect(page.locator('#msgs')).to_contain_text('Growing native identity')
                # Same-length rewrites, truncation and replacement must still
                # invalidate the scalar cache on the next ordinary page load.
                original = path.read_bytes()
                changed = original.replace(b'synthetic-model', b'rewritten-model').replace(b'task_started', b'turn_aborted')
                assert len(changed) == len(original)
                path.write_bytes(changed)
                page.reload(wait_until='domcontentloaded')
                page.wait_for_function("uid => S.sessions.find(row => row.uid === uid)?.model === 'rewritten-model'", arg=corpus.uid(sid))
                assert page.evaluate('uid => S.sessions.find(row => row.uid === uid).turn', corpus.uid(sid)) == 'aborted'
                compact = b''.join(encoded(row) for row in [
                    codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/append'}),
                    codex_message('user', 'Replaced native identity'),
                    codex_row('turn_context', {'model': 'compact-model'}),
                    codex_row('event_msg', {'type': 'task_complete'})])
                for replacement in (False, True):
                    if replacement:
                        other = path.with_suffix('.replacement')
                        other.write_bytes(compact.replace(b'compact-model', b'inode-model--'))
                        other.replace(path)
                    else:
                        path.write_bytes(compact)
                    page.reload(wait_until='domcontentloaded')
                    expect(page.locator('#msgs')).to_contain_text('Replaced native identity')
                    actual = page.evaluate('uid => S.sessions.find(row => row.uid === uid)', corpus.uid(sid))
                    assert actual['model'] == ('inode-model--' if replacement else 'compact-model'), actual
                    assert actual['sid'] == sid and actual['turn'] == 'idle', actual
            except Exception:
                print('NATIVE READ DIAGNOSTICS', '\n'.join(line for line in log.read_text().splitlines()
                    if 'native_state.read_failed' in line), flush=True)
                raise
            finally:
                stop.set()
                writer.join(timeout=5)
                browser.close()
        print('PASS native touches/appends stay readable; same-size rewrite, truncation and inode replacement refresh model/turn/detail', flush=True)
    verify_large_tree_refresh_and_mutations(args.binary)


if __name__ == '__main__':
    main()
