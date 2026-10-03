#!/usr/bin/env python3
"""OpenCode mirror polling: private SQLite fixtures and real page interactions.

Run directly after building target/debug/sessiondock. No CLI is launched.
The 65-second idle observation crosses the former full-history polling period.
"""
from browser_runtime import js
import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import threading
import time

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, Corpus, isolated_server
from opencode_browser import SEEDED, seed


def execute(db, sql, args=()):
    with closing(sqlite3.connect(db, timeout=10)) as connection, connection:
        connection.execute(sql, args)


def wait_for(predicate, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.1)
    raise AssertionError('mirror did not converge')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-opencode-mirror-') as temporary:
        root = Path(temporary).resolve()
        for name in ('home', 'work', 'host', 'state', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        db = root / 'opencode.db'
        mirror = root / 'mirror'
        directory = mirror / 'fakeproject' / SEEDED
        messages = directory / 'messages.jsonl'
        summary = directory / 'summary.json'
        env = {'SESSIONDOCK_OPENCODE_DB': str(db), 'SESSIONDOCK_OPENCODE_ROOT': str(mirror)}

        def server():
            return isolated_server(Corpus(root), args.binary, host_dir=root / 'host',
                                   state_dir=root / 'state', extra_env=env)

        def open_session(page, base, sid=SEEDED):
            page.goto(base, wait_until='domcontentloaded')
            page.wait_for_function(js('sid => S.sessions.some(r => r.sid === sid)', 'sid => runtime.core.state.catalog.sessions.some(r => r.sid === sid)'), arg=sid, timeout=20000)
            uid = page.evaluate(js('sid => S.sessions.find(r => r.sid === sid).uid', 'sid => runtime.core.state.catalog.sessions.find(r => r.sid === sid).uid'), sid)
            page.locator(f'#side .item[data-uid="{uid}"]').click()
            return page.locator('#msgs')

        with sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(service_workers='block')
            allowed_base = ['']
            context.route('**/*', lambda route: route.continue_()
                          if route.request.url.startswith(allowed_base[0] + '/') else route.abort())
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            try:
                with server() as (base, _):
                    allowed_base[0] = base
                    # Start without a database, then create only the configured one.
                    page.goto(base, wait_until='networkidle')
                    seed(db, root / 'work')
                    msgs = open_session(page, base)
                    expect(msgs).to_contain_text('目录里有 zebracorn 文件。', timeout=20000)

                    # WAL commits must be noticed while the main DB is unchanged.
                    writer = sqlite3.connect(db, timeout=10)
                    try:
                        writer.execute('PRAGMA journal_mode=WAL')
                        writer.execute('PRAGMA wal_autocheckpoint=0')
                        main_stamp = db.stat().st_mtime_ns
                        writer.execute("UPDATE session_v2 SET title='mirror updated' WHERE id=?", (SEEDED,))
                        writer.commit()
                        page.wait_for_function(js("() => S.sessions.some(r => r.title === 'mirror updated')", "() => runtime.core.state.catalog.sessions.some(r => r.title === 'mirror updated')"), timeout=20000)
                        assert db.stat().st_mtime_ns == main_stamp

                        # Message changes need no session timestamp change, and an
                        # older row may change below the aggregate max timestamp.
                        data = {'text': 'older row changed', 'files': [], 'agents': []}
                        writer.execute('UPDATE session_message SET data=?, time_updated=time_updated+1 WHERE id=?',
                                       (json.dumps(data), 'msg_seed_1'))
                        writer.commit()
                        expect(msgs).to_contain_text('older row changed', timeout=20000)

                        stream = {'time': {'created': 1790500000100}, 'content': [
                            {'type': 'text', 'text': 'stream settled without session update'}]}
                        writer.execute('INSERT INTO session_message VALUES (?, ?, ?, ?, ?, ?, ?)',
                                       ('msg_stream', SEEDED, 'assistant', 100, 100, 100, json.dumps(stream)))
                        writer.commit()
                        # A following user row settles the assistant without
                        # changing its own timestamp or data.
                        wait_for(lambda: messages.exists())
                        page.wait_for_timeout(2200)
                        expect(msgs).not_to_contain_text('stream settled without session update')
                        writer.execute('INSERT INTO session_message VALUES (?, ?, ?, ?, ?, ?, ?)',
                                       ('msg_after', SEEDED, 'user', 101, 101, 101,
                                        json.dumps({'text': 'after stream', 'files': [], 'agents': []})))
                        writer.commit()
                        expect(msgs).to_contain_text('stream settled without session update', timeout=20000)
                        expect(msgs).to_contain_text('after stream')
                        writer.execute('DELETE FROM session_message WHERE seq >= 100')
                        writer.commit()
                        expect(msgs).not_to_contain_text('after stream', timeout=20000)
                        expect(msgs).not_to_contain_text('stream settled without session update', timeout=20000)
                    finally:
                        writer.close()

                    # Views make full project/message reads observable without a
                    # tracing hook in production: every read yields different bytes.
                    with closing(sqlite3.connect(db)) as connection, connection:
                        connection.executescript('''
                            ALTER TABLE project RENAME TO project_rows;
                            CREATE VIEW project AS SELECT id, hex(randomblob(16)) AS worktree FROM project_rows;
                            ALTER TABLE session_message RENAME TO message_rows;
                            CREATE VIEW session_message AS SELECT id, session_id, type, seq, time_created,
                                random() AS time_updated, data FROM message_rows;
                        ''')
                    wait_for(lambda: json.loads(summary.read_text())['project']['worktree'] != str(root / 'work'))
                    # A commit between the pre-snapshot version sample and
                    # snapshot may legitimately cause one redundant pass.
                    page.wait_for_timeout(2200)
                    stamps = (summary.stat().st_mtime_ns, messages.stat().st_mtime_ns)
                    # Continue driving the page during idle; crosses 60 polls.
                    for _ in range(13):
                        page.locator('#q').fill('older row')
                        page.locator('#q').press('Enter')
                        page.wait_for_timeout(2500)
                        page.locator('#q').fill('')
                        page.locator('#q').press('Enter')
                        page.wait_for_timeout(2500)
                        assert stamps == (summary.stat().st_mtime_ns, messages.stat().st_mtime_ns), \
                            'unchanged DB was queried again'
                    with closing(sqlite3.connect(db)) as connection, connection:
                        connection.executescript('''
                            DROP VIEW project;
                            ALTER TABLE project_rows RENAME TO project;
                            DROP VIEW session_message;
                            ALTER TABLE message_rows RENAME TO session_message;
                        ''')
                    wait_for(lambda: json.loads(summary.read_text())['project']['worktree'] == str(root / 'work'))

                    # A read error closes the persistent connection. Recovery
                    # must not reuse its version or row cache.
                    execute(db, 'ALTER TABLE session_message RENAME TO temporarily_unavailable')
                    page.wait_for_timeout(2200)
                    with closing(sqlite3.connect(db)) as connection, connection:
                        connection.execute('ALTER TABLE temporarily_unavailable RENAME TO session_message')
                        connection.execute("UPDATE session_v2 SET title='reconnected' WHERE id=?", (SEEDED,))
                    page.wait_for_function(js("() => S.sessions.some(r => r.title === 'reconnected')", "() => runtime.core.state.catalog.sessions.some(r => r.title === 'reconnected')"), timeout=20000)

                    # A burst of commits ends without any further native activity;
                    # the very last update must converge (snapshot/version race).
                    failures = []
                    def burst():
                        try:
                            with closing(sqlite3.connect(db, timeout=10)) as connection, connection:
                                for index in range(100):
                                    connection.execute('UPDATE session_v2 SET title=? WHERE id=?',
                                                       (f'burst {index}', SEEDED))
                                    connection.commit()
                                    time.sleep(0.025)
                        except Exception as error:
                            failures.append(error)
                    worker = threading.Thread(target=burst)
                    worker.start()
                    page.wait_for_function(js("() => S.sessions.some(r => r.title === 'burst 99')", "() => runtime.core.state.catalog.sessions.some(r => r.title === 'burst 99')"), timeout=20000)
                    worker.join(timeout=10)
                    assert not worker.is_alive() and not failures, failures

                    # Replace the DB at the same configured path, reusing native
                    # ids and timestamps, then observe the replacement on page.
                    replacement = root / 'replacement.db'
                    seed(replacement, root / 'work')
                    execute(replacement, "UPDATE session_v2 SET title='replacement' WHERE id=?", (SEEDED,))
                    # Checkpoint/close fixture writers before an atomic replacement.
                    with closing(sqlite3.connect(db)) as connection, connection:
                        connection.execute('PRAGMA wal_checkpoint(TRUNCATE)')
                    os.replace(replacement, db)
                    page.wait_for_function(js("() => S.sessions.some(r => r.title === 'replacement')", "() => runtime.core.state.catalog.sessions.some(r => r.title === 'replacement')"), timeout=20000)
                    msgs = open_session(page, base)
                    expect(msgs).to_contain_text('看看这张图', timeout=20000)
                    expect(msgs).not_to_contain_text('older row changed')
                    stamps = (summary.stat().st_mtime_ns, messages.stat().st_mtime_ns)

                # Service restart preserves identical mirror stamps and renders
                # native history. Orphans left by an earlier process are swept.
                orphan = mirror / 'fakeproject' / 'ses_orphan'
                orphan.mkdir()
                (orphan / 'summary.json').write_text('{}')
                with server() as (base, _):
                    allowed_base[0] = base
                    msgs = open_session(page, base)
                    expect(msgs).to_contain_text('看看这张图', timeout=20000)
                    wait_for(lambda: not orphan.exists())
                    assert stamps == (summary.stat().st_mtime_ns, messages.stat().st_mtime_ns)
                    # Idle recovery checks only known file paths, then restores
                    # missing mirrors without needing a database commit.
                    summary.unlink()
                    messages.unlink()
                    wait_for(lambda: summary.is_file() and messages.is_file(), timeout=75)
                    msgs = open_session(page, base)
                    expect(msgs).to_contain_text('看看这张图', timeout=20000)
                    # Native removal drops both page row and mirror.
                    with closing(sqlite3.connect(db)) as connection, connection:
                        connection.execute('DELETE FROM session_message WHERE session_id=?', (SEEDED,))
                        connection.execute('DELETE FROM session_v2 WHERE id=?', (SEEDED,))
                    page.wait_for_function(js('sid => !S.sessions.some(r => r.sid === sid)', 'sid => !runtime.core.state.catalog.sessions.some(r => r.sid === sid)'), arg=SEEDED, timeout=20000)
                    assert not directory.exists()
                assert not errors, errors
            finally:
                context.close()
                browser.close()
    print('PASS OpenCode mirror browser: initial DB, WAL, old-row update, stream settle, revert, '
          'idle query avoidance, reconnect, final commit, replacement, restart, deletion')


if __name__ == '__main__':
    main()
