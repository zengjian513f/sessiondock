#!/usr/bin/env python3
"""Chromium list/detail reads across native touches, appends and replacements."""

import argparse
import os
from pathlib import Path
import tempfile
import threading

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, codex_message, encoded, isolated_server
from frontend_framework_browser import launch_chromium


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


if __name__ == '__main__':
    main()
