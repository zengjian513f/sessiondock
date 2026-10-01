#!/usr/bin/env python3
"""Detached Codex exec fan-out initializes nest_parent; user choices survive scans/restart."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

from playwright.sync_api import sync_playwright, expect
from history_parity import Corpus, codex_row, get_json
import spawned_by_suite as fixture
from spawned_by_suite import BINARY, BTIME, proc_pid, server_with_env


def stamp(seconds):
    return datetime.fromtimestamp(BTIME + seconds, timezone.utc).isoformat()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='codex-exec-nest-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        corpus = Corpus(root)
        children = [f'exec-worker-{i}' for i in range(6)]
        for sid in ['parent', 'other', 'resumed', 'standalone', 'cycle-parent', 'cycle-child', 'imported-detached', 'imported-attached', *children]:
            created = 101 if sid in ('parent', 'cycle-parent') else 150 if sid in ('other', 'resumed') else 401
            corpus.put(sid, 'codex', [
                codex_row('session_meta', {'id': sid, 'session_id': sid, 'cwd': '/synthetic/work',
                    'timestamp': stamp(created), 'source': 'exec' if sid in children else 'cli',
                    'thread_source': 'user'}),
                codex_row('response_item', {'type': 'message', 'role': 'user',
                    'content': 'Inspect ' + sid})], [])
        proc = root / 'proc'
        proc.mkdir()
        (proc / 'stat').write_text(f'btime {BTIME}\n')
        proc_pid(proc, 100, 'codex', ['codex'], 1, fds={3: corpus.paths['parent']})
        inherited = [('CODEX_THREAD_ID', 'parent'), ('CODEX_SESSION_ID', 'parent')]
        # Reparented nohup dispatcher: no live CLI ancestor remains.
        proc_pid(proc, 200, 'python', ['python', 'dispatch.py'], 1, env=inherited)
        for i, sid in enumerate(children):
            pid = 900 + i
            fixture.START[pid] = 40_000
            proc_pid(proc, pid, 'codex', ['codex', 'exec'], 200,
                     env=inherited, fds={3: corpus.paths[sid]})
        for pid, sid in [(950, 'imported-detached'), (951, 'imported-attached')]:
            fixture.START[pid] = 40_000
            proc_pid(proc, pid, 'codex', ['codex', 'exec'], 200,
                     env=inherited, fds={3: corpus.paths[sid]})
        proc_pid(proc, 400, 'codex', ['codex', 'exec', 'resume', 'resumed'], 200,
                 env=inherited, fds={3: corpus.paths['resumed']})
        proc_pid(proc, 500, 'codex', ['codex', 'exec'], 1, fds={3: corpus.paths['standalone']})
        fixture.START.update({600: 10_000, 601: 40_000})
        proc_pid(proc, 600, 'codex', ['codex'], 1, fds={3: corpus.paths['cycle-parent']})
        proc_pid(proc, 601, 'codex', ['codex', 'exec'], 600,
                 env=[('CODEX_THREAD_ID', 'cycle-parent')], fds={3: corpus.paths['cycle-child']})
        state = root / 'state' 
        state.mkdir(mode=0o700)
        legacy = root / 'legacy/session-meta.json'
        legacy.parent.mkdir()
        legacy.write_text(json.dumps({'version': 1, 'sessions': {
            corpus.uid('cycle-parent'): {'nest_parent': {'source': 'codex', 'sid': 'cycle-child'}},
            corpus.uid('imported-detached'): {'spawned_by': {'source': 'codex', 'sid': 'parent'}, 'nest_independent': True},
            corpus.uid('imported-attached'): {'spawned_by': {'source': 'codex', 'sid': 'parent'}, 'nest_parent': {'source': 'codex', 'sid': 'other'}},
        }}))
        subprocess.run(['python3', str(Path(__file__).with_name('meta_import.py')),
                        '--python-meta', str(legacy), '--out-dir', str(state), '--no-debug-runs'],
                       check=True, capture_output=True, timeout=20)
        env = {'SESSIONDOCK_PROC_ROOT': proc, 'SESSIONDOCK_STATE_DIR': state,
               'SESSIONDOCK_GROK_ACTIVE': root / 'absent'}
        browser = pw.chromium.launch(headless=True)
        for restarted in (False, True):
            with server_with_env(corpus, env, args.binary) as (base, opener):
                # Background discovery must work without opening /api/live.
                deadline = time.monotonic() + 18
                while True:
                    rows = {r['sid']: r for r in get_json(opener, base, '/api/sessions?force=1')['sessions']}
                    if all(rows[s].get('nest_parent') for s in children[2:]):
                        break
                    assert time.monotonic() < deadline, 'exec children never acquired their parent'
                    time.sleep(.1)
                assert all(rows[s]['nest_parent'] == {'source': 'codex', 'sid': 'parent'} for s in children[2:])
                assert 'nest_parent' not in rows['resumed'], 'resuming an existing session is not birth'
                assert 'nest_parent' not in rows['cycle-child'], 'automatic attachment created a manual-parent cycle'
                assert 'nest_parent' not in rows['standalone'], 'same cwd is not launch evidence'
                assert 'nest_parent' not in rows['imported-detached'], 'import lost the detach decision'
                assert rows['imported-attached']['nest_parent']['sid'] == 'other', 'import lost the explicit parent'
                assert all('spawned_by' not in r and 'nest_initialized' not in r for r in rows.values())
                context = browser.new_context(viewport={'width': 1280, 'height': 900})
                page = context.new_page()
                page.goto(base)
                page.wait_for_function('S.sessions.length >= 10')
                if not page.evaluate('S.nest'):
                    page.locator('#nest-toggle').click()
                def item(sid):
                    return page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]')
                expect(item('imported-detached')).to_have_attribute('data-depth', '0')
                expect(item('imported-attached')).to_have_attribute('data-depth', '1')
                item('imported-detached').click()
                expect(page.locator('#msgs')).to_contain_text('Inspect imported-detached')
                for sid in children[2:]:
                    expect(item(sid)).to_have_attribute('data-depth', '1')
                item(children[2]).click()
                expect(page.locator('#msgs')).to_contain_text('Inspect ' + children[2])
                if not restarted:
                    item(children[0]).click(button='right')
                    page.locator('#item-menu [data-act="detach"]').click()
                    expect(item(children[0])).to_have_attribute('data-depth', '0')
                    item(children[1]).click(button='right')
                    page.locator('#item-menu [data-act="attach"]').click()
                    item('other').click()
                    page.wait_for_function('uid => S.sessions.find(s => s.uid === uid).nest_parent?.sid === "other"', arg=corpus.uid(children[1]))
                get_json(opener, base, '/api/live?force=1')
                page.evaluate('pollSessions()')
                expect(item(children[0])).to_have_attribute('data-depth', '0')
                rows = {r['sid']: r for r in get_json(opener, base, '/api/sessions?force=1')['sessions']}
                assert 'nest_parent' not in rows[children[0]]
                assert rows[children[1]]['nest_parent']['sid'] == 'other'
                context.close()
        # Completed workers keep their recorded attachment through another restart.
        for entry in proc.iterdir():
            if entry.is_dir():
                shutil.rmtree(entry)
        with server_with_env(corpus, env, args.binary) as (base, opener):
            assert not get_json(opener, base, '/api/live?force=1')['uids']
            rows = {r['sid']: r for r in get_json(opener, base, '/api/sessions?force=1')['sessions']}
            assert rows[children[2]]['nest_parent']['sid'] == 'parent'
            assert 'nest_parent' not in rows[children[0]]
        browser.close()
    print('PASS six detached codex exec children, background discovery, Chromium open/detach/attach, resume isolation, restart and exit persistence')


if __name__ == '__main__':
    main()
