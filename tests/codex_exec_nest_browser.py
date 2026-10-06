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
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright, expect
from history_fixtures import Corpus, claude_row, codex_row, get_json
import spawned_by_fixtures as fixture
from spawned_by_fixtures import BINARY, BTIME, proc_pid, server_with_env


def stamp(seconds):
    return datetime.fromtimestamp(BTIME + seconds, timezone.utc).isoformat()


def first_publication(browser, root, binary):
    """New exec files must carry their parent in the first list and DOM render."""
    corpus = Corpus(root)
    def put(sid, created):
        corpus.put(sid, 'codex', [
            codex_row('session_meta', {'id': sid, 'session_id': sid,
                'cwd': '/synthetic/work', 'timestamp': stamp(created),
                'source': 'exec', 'thread_source': 'user'}),
            codex_row('response_item', {'type': 'message', 'role': 'user',
                'content': 'Inspect ' + sid})], [])
    put('parent', 101)
    proc = root / 'proc'
    proc.mkdir()
    (proc / 'stat').write_text(f'btime {BTIME}\n')
    proc_pid(proc, 100, 'codex', ['codex'], 1, fds={3: corpus.paths['parent']})
    proc_pid(proc, 200, 'python', ['python', 'dispatch.py'], 1,
             env=[('CODEX_THREAD_ID', 'parent'), ('CODEX_SESSION_ID', 'parent')])
    state = root / 'state'
    state.mkdir(mode=0o700)
    with server_with_env(corpus, {'SESSIONDOCK_PROC_ROOT': proc,
            'SESSIONDOCK_STATE_DIR': state, 'SESSIONDOCK_GROK_ACTIVE': root / 'absent'},
            binary) as (base, opener):
        context = browser.new_context(viewport={'width': 1280, 'height': 900})
        page = context.new_page()
        page.goto(base)
        page.wait_for_function('S.sessions.length === 1')
        if not page.evaluate('S.nest'):
            page.locator('#nest-toggle').click()
        # Prime the shared process cache before the new CLI exists.
        get_json(opener, base, '/api/live?force=1')
        put('fresh-worker', 401)
        fixture.START[990] = 40_000
        proc_pid(proc, 990, 'codex', ['codex', 'exec'], 200,
                 env=[('CODEX_THREAD_ID', 'parent'), ('CODEX_SESSION_ID', 'parent')],
                 fds={3: corpus.paths['fresh-worker']})
        uid = corpus.uid('fresh-worker')
        page.evaluate('''uid => {
            window.workerDepths = [];
            const sample = () => {
                const item = document.querySelector('#side .item[data-uid="' + uid + '"]');
                if (item) window.workerDepths.push(item.dataset.depth);
            };
            window.workerObserver = new MutationObserver(sample);
            window.workerObserver.observe(document.querySelector('#side'),
                {childList: true, subtree: true, attributes: true, attributeFilter: ['data-depth']});
        }''', uid)
        # User refresh forces inventory discovery inside the scanner's TTL.
        with page.expect_response(lambda response: '/api/sessions?force=1' in response.url) as response:
            page.evaluate('loadSessions(true)')
        rows = {r['sid']: r for r in response.value.json()['sessions']}
        assert rows['fresh-worker'].get('nest_parent') == {'source': 'codex', 'sid': 'parent'}, \
            'first list published a new exec worker without its parent'
        item = page.locator(f'#side .item[data-uid="{uid}"]')
        expect(item).to_have_attribute('data-depth', '1')
        item.click()
        expect(page.locator('#msgs')).to_contain_text('Inspect fresh-worker')
        assert page.evaluate('window.workerDepths') and set(page.evaluate('window.workerDepths')) == {'1'}, \
            'exec worker briefly rendered as a root'
        page.evaluate('window.workerObserver.disconnect()')
        # Automatic signature polling must also publish the parent immediately.
        get_json(opener, base, '/api/live?force=1')
        put('polled-worker', 401)
        fixture.START[991] = 40_000
        proc_pid(proc, 991, 'codex', ['codex', 'exec'], 200,
                 env=[('CODEX_THREAD_ID', 'parent')], fds={3: corpus.paths['polled-worker']})
        page.wait_for_timeout(550)  # Expire the 500ms inventory cache, not the 3s process cache.
        with page.expect_response(lambda response: urlsplit(response.url).path == '/api/sessions') as response:
            page.evaluate('pollSessions()')
        wire = response.value.json()
        published = wire.get('sessions', [item['row'] for item in
            wire.get('list_delta', {}).get('collections', {}).get('sessions', {}).get('upsert', [])
            if 'row' in item])
        rows = {r['sid']: r for r in published}
        assert rows['polled-worker'].get('nest_parent') == {'source': 'codex', 'sid': 'parent'}, \
            'first automatic list poll published an exec worker without its parent'
        item = page.locator(f'#side .item[data-uid="{corpus.uid("polled-worker")}"]')
        expect(item).to_have_attribute('data-depth', '1')
        item.click()
        expect(page.locator('#msgs')).to_contain_text('Inspect polled-worker')
        context.close()
    print('PASS first list publication and every Chromium render nest a newly discovered exec worker', flush=True)


def inactive_children(browser, root, binary):
    """The active filter explains omitted exec children without making them selectable."""
    corpus = Corpus(root)
    children = ['nodes', 'groups', 'audit']
    for sid in ['parent', 'unrelated', *children]:
        corpus.put(sid, 'codex', [
            codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/work',
                'timestamp': stamp(101 if sid == 'parent' else 401), 'source': 'exec',
                'thread_source': 'user'}),
            codex_row('response_item', {'type': 'message', 'role': 'user',
                'content': 'Inspect ' + sid})], [])
    corpus.put('other-source', 'claude', [claude_row('other-source', 'user', 'u0', None,
        'Inspect other-source', cwd='/synthetic/work', timestamp=stamp(401))], [])
    state = root / 'state'
    state.mkdir(mode=0o700)
    (state / 'session-metadata.json').write_text(json.dumps({'schema_version': 1,
        'revision': 1, 'sessions': {corpus.uid(sid): {
            'nest_parent': {'source': 'codex', 'sid': 'parent'}, 'nest_initialized': True}
            for sid in [*children, 'other-source']}}))
    proc = root / 'proc'
    proc.mkdir()
    (proc / 'stat').write_text(f'btime {BTIME}\n')
    proc_pid(proc, 100, 'codex', ['codex'], 1, fds={3: corpus.paths['parent']})
    proc_pid(proc, 400, 'codex', ['codex', 'exec'], 100, fds={3: corpus.paths['audit']})
    with server_with_env(corpus, {'SESSIONDOCK_PROC_ROOT': proc,
            'SESSIONDOCK_STATE_DIR': state, 'SESSIONDOCK_GROK_ACTIVE': root / 'absent'},
            binary) as (base, opener):
        for width in [1280, 390]:
            context = browser.new_context(viewport={'width': width, 'height': 900})
            page = context.new_page()
            page.goto(base)
            expect(page.locator('#session-total')).to_have_text('6')
            expect(page.locator('#session-active')).to_have_text('2')
            if not page.evaluate('S.nest'):
                page.locator('#nest-toggle').click()
            def item(sid):
                return page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]')
            parent = item('parent')
            hint = parent.locator('.nest-inactive')
            expect(hint).to_be_hidden()
            page.locator('#livecount').click()
            expect(hint).to_have_text('有 3 个不活跃会话（已被筛选隐藏）')
            expect(hint).to_be_visible()
            for sid in ['nodes', 'groups', 'unrelated', 'other-source']:
                expect(item(sid)).to_have_count(0)
            expect(page.locator('#side .gcount')).to_have_text('2')
            parent.locator('.nest-caret').click()
            expect(hint).to_be_hidden()
            expect(parent.locator('.nest-caret')).to_have_attribute('title', '展开 1 项，另有 3 个不活跃会话')
            parent.locator('.nest-caret').press('Enter')
            expect(hint).to_be_visible()
            # Source exclusions remain exclusions, not inactive children.
            page.locator('#chips button[data-source="claude"]').click()
            expect(hint).to_have_text('有 2 个不活跃会话（已被筛选隐藏）')
            page.locator('#nest-flat').click()
            expect(hint).to_be_hidden()
            page.locator('#nest-toggle').click()
            expect(hint).to_be_visible()
            page.locator('#q').fill('Inspect parent')
            expect(hint).to_be_hidden()
            page.locator('#q').fill('')
            expect(hint).to_have_text('有 2 个不活跃会话（已被筛选隐藏）')
            # Finish the remaining worker; background live polling updates both
            # list membership and the hint, including an inactive-only branch.
            shutil.rmtree(proc / '400')
            expect(page.locator('#session-active')).to_have_text('1', timeout=15000)
            expect(hint).to_have_text('有 3 个不活跃会话（已被筛选隐藏）')
            expect(parent.locator('.nest-caret')).to_be_visible()
            parent.locator('.nest-caret').click()
            expect(hint).to_be_hidden()
            expect(parent.locator('.nest-caret')).to_have_attribute('title', '展开 3 个不活跃会话')
            parent.locator('.nest-caret').click()
            expect(hint).to_be_visible()
            page.locator('#allcount').click()
            expect(hint).to_be_hidden()
            for sid in children:
                expect(item(sid)).to_have_attribute('data-depth', '1')
                item(sid).click()
                expect(page.locator('#msgs')).to_contain_text('Inspect ' + sid)
                if page.evaluate('document.body.classList.contains("mobile-detail")'):
                    page.locator('.dhead .mobile-back').first.click()
            context.close()
            proc_pid(proc, 400, 'codex', ['codex', 'exec'], 100,
                     fds={3: corpus.paths['audit']})
            get_json(opener, base, '/api/live?force=1')
    print('PASS inactive child hint, fold/keyboard, source/search/flat filters, live exit, all histories, desktop/mobile', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='codex-exec-nest-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        browser = pw.chromium.launch(headless=True)
        first_publication(browser, root / 'first-publication', args.binary)
        inactive_children(browser, root / 'inactive-children', args.binary)
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
                        '--python-meta', str(legacy), '--out-dir', str(state)],
                       check=True, capture_output=True, timeout=20)
        env = {'SESSIONDOCK_PROC_ROOT': proc, 'SESSIONDOCK_STATE_DIR': state,
               'SESSIONDOCK_GROK_ACTIVE': root / 'absent'}
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
