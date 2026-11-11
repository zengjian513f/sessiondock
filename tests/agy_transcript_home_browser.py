#!/usr/bin/env python3
"""Explicit alternate Agy transcript store through real history/search UI.

Synthetic catalog and transcripts only; no CLI or production session is used.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys
import tempfile

sys.dont_write_bytecode = True
from playwright.sync_api import expect, sync_playwright
from agy_history_browser import (SEEDED, ORPHAN, seed_catalog, write_transcript,
                                record, private_environment)
from history_fixtures import BINARY, Corpus, isolated_server, encoded


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-transcript-home-') as tmp:
        root = Path(tmp)
        native, alternate, mirror, work = [root / name for name in ('cli', 'shared', 'mirror', 'work')]
        for path in (native, alternate, work, root / 'home', root / 'state', root / 'proc'):
            path.mkdir(mode=0o700)
        seed_catalog(native / 'conversation_summaries.db', work)
        shared = write_transcript(alternate, SEEDED, [record(0, 'USER_INPUT', '导入会话的提问'),
            record(1, 'PLANNER_RESPONSE', '共享目录中的完整回答 agyalternatefirst')])
        write_transcript(alternate, ORPHAN, [record(0, 'USER_INPUT', '不能自动导入其他会话')])
        corpus = Corpus(root)
        env = {'SESSIONDOCK_AGY_HOME': str(native), 'SESSIONDOCK_AGY_ROOT': str(mirror),
               'SESSIONDOCK_PROC_ROOT': str(root / 'proc')}
        def server():
            return isolated_server(corpus, args.binary, state_dir=root / 'state', extra_env=env)
        def summary():
            return json.loads((mirror / 'cli' / SEEDED / 'summary.json').read_text())
        with sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            page = browser.new_page(viewport={'width': 1280, 'height': 900})
            page.set_default_timeout(20000)
            def open_history(base):
                page.goto(base, wait_until='domcontentloaded')
                page.wait_for_function('sid => S.sessions.some(r => r.sid === sid)', arg=SEEDED)
                uid = page.evaluate('sid => S.sessions.find(r => r.sid === sid).uid', SEEDED)
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                return uid
            try:
                with private_environment(root):
                    # A sibling store is not implicitly discovered.
                    with server() as (base, _):
                        uid = open_history(base)
                        expect(page.locator('.native-history-warning')).to_contain_text('仅找到 Agy 的会话索引')
                        expect(page.locator('#msgs .msg')).to_have_count(0)
                    env['SESSIONDOCK_AGY_TRANSCRIPT_HOME'] = str(alternate)
                    with server() as (base, _):
                        assert open_history(base) == uid
                        expect(page.locator('#msgs')).to_contain_text('agyalternatefirst')
                        expect(page.locator('.native-history-warning')).to_have_count(0)
                        assert not page.evaluate('sid => S.sessions.some(r => r.sid === sid)', ORPHAN)
                        assert summary()['native_transcript'] == str(shared)
                        assert summary()['native_database'] == str(native / 'conversations' / (SEEDED + '.db'))
                        assert not (native / 'conversations').exists()
                        page.locator('#q').fill('agyalternatefirst')
                        page.locator('#q').press('Enter')
                        hit = page.locator(f'#side .item[data-uid="{uid}"]')
                        expect(hit.locator('.m')).to_contain_text('命中')
                        hit.click()
                        expect(page.locator('#msgs')).to_contain_text('agyalternatefirst')
                        page.locator('#q').fill('')
                        page.locator('#q').press('Enter')
                        expect(page.locator('#side-search-state')).to_be_hidden()
                        with shared.open('ab') as f:
                            f.write(encoded(record(2, 'USER_INPUT', '备用正文新增 agyalternateappend')))
                        expect(page.locator('#msgs')).to_contain_text('agyalternateappend')
                        write_transcript(alternate, SEEDED, [record(0, 'USER_INPUT', '备用正文改写 agyalternaterewrite')])
                        expect(page.locator('#msgs')).to_contain_text('agyalternaterewrite')
                        expect(page.locator('#msgs')).not_to_contain_text('agyalternatefirst')
                        primary = write_transcript(native, SEEDED, [record(0, 'USER_INPUT', 'CLI 正文优先 agyprimarywins')])
                        expect(page.locator('#msgs')).to_contain_text('agyprimarywins')
                        expect(page.locator('#msgs')).not_to_contain_text('agyalternaterewrite')
                        primary.unlink()
                        expect(page.locator('#msgs')).to_contain_text('agyalternaterewrite')
                        shared.unlink()
                        expect(page.locator('.native-history-warning')).to_contain_text('保留上次读取的历史')
                        expect(page.locator('#msgs')).to_contain_text('agyalternaterewrite')
                        write_transcript(alternate, SEEDED, [record(0, 'USER_INPUT', '恢复备用正文 agyalternaterestored')])
                        expect(page.locator('#msgs')).to_contain_text('agyalternaterestored')
                        expect(page.locator('.native-history-warning')).to_have_count(0)
                    with server() as (base, _):
                        assert open_history(base) == uid
                        expect(page.locator('#msgs')).to_contain_text('agyalternaterestored')
                        # Switching to another explicitly configured store must invalidate
                        # the source association as well as its file metadata.
                    second = root / 'other-shared'
                    write_transcript(second, SEEDED, [record(0, 'USER_INPUT', '切换只读目录 agyalternatechanged')])
                    env['SESSIONDOCK_AGY_TRANSCRIPT_HOME'] = str(second)
                    with server() as (base, _):
                        assert open_history(base) == uid
                        expect(page.locator('#msgs')).to_contain_text('agyalternatechanged')
                        expect(page.locator('#msgs')).not_to_contain_text('agyalternaterestored')
                        assert summary()['native_database'] == str(native / 'conversations' / (SEEDED + '.db'))
                        if args.screenshots:
                            args.screenshots.mkdir(parents=True, exist_ok=True)
                            page.screenshot(path=str(args.screenshots / 'agy-transcript-home.png'), full_page=True)
            finally:
                browser.close()
    print('PASS Agy alternate transcript: explicit scope, history/search, updates, primary priority, cache, restart and source switch')


if __name__ == '__main__':
    main()
