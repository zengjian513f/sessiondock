#!/usr/bin/env python3
"""Agy native parent ownership through Chromium; synthetic catalog, no CLI."""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
sys.dont_write_bytecode = True
from playwright.sync_api import expect, sync_playwright
from agy_history_browser import SCHEMA, record, write_transcript, private_environment, execute
from history_fixtures import BINARY, Corpus, isolated_server, encoded, get_json

PARENT = 'a6000000-0000-4000-8000-000000000011'
CHILD = 'a6000000-0000-4000-8000-000000000012'
GRANDCHILD = 'a6000000-0000-4000-8000-000000000013'
MISSING = 'a6000000-0000-4000-8000-000000000014'
ORPHAN = 'a6000000-0000-4000-8000-000000000015'


def run(binary, screenshots):
    screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-nesting-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        for name in ('home', 'native', 'work', 'proc', 'state', 'host', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        native = root / 'native'
        db = native / 'conversation_summaries.db'
        with sqlite3.connect(db) as connection:
            connection.execute(SCHEMA)
            for sid, title, parent, status in (
                (PARENT, '复核已关闭研报实验 / Agy native parent', '', 'CASCADE_RUN_STATUS_IDLE'),
                (CHILD, '', PARENT, 'CASCADE_RUN_STATUS_IDLE'),
                (GRANDCHILD, 'Nested reviewer / 核对实验结果与历史结论', CHILD, 'CASCADE_RUN_STATUS_RUNNING'),
                (ORPHAN, '暂时缺失的父会话', MISSING, 'CASCADE_RUN_STATUS_IDLE')):
                connection.execute('INSERT INTO conversation_summaries VALUES (?,?,?,?,?,?,?,?)',
                    (sid, title, '2026-10-16T14:23:00Z', json.dumps([(root / 'work').as_uri()]),
                     parent, status, 1, 'self'))
                write_transcript(native, sid, [record(0, 'PLANNER_RESPONSE', 'History ' + sid)])
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        try:
            with private_environment(root):
                for restart in (False, True):
                    with isolated_server(Corpus(root), binary, state_dir=root / 'state', host_dir=root / 'host',
                        extra_env={'SESSIONDOCK_AGY_HOME': str(native), 'SESSIONDOCK_AGY_ROOT': str(root / 'mirror'),
                                   'SESSIONDOCK_PROC_ROOT': str(root / 'proc')}) as (base, opener):
                        for width, theme in ([(1280, 'light')] if restart else
                                              [(1280, 'light'), (1280, 'dark'), (390, 'light'), (390, 'dark')]):
                            context = browser.new_context(viewport={'width': width, 'height': 900},
                                                          color_scheme=theme, service_workers='block')
                            context.route('**/*', lambda route: route.continue_()
                                          if route.request.url.startswith(base + '/') else route.abort())
                            page = context.new_page()
                            errors = []
                            page.on('pageerror', lambda error: errors.append(str(error)))
                            page.set_default_timeout(15000)
                            page.goto(base)
                            page.wait_for_function('sid => S.sessions.some(r => r.sid === sid && r.agents === 2)', arg=PARENT)
                            rows = page.evaluate('S.sessions')
                            parent = next(row for row in rows if row['sid'] == PARENT)
                            assert not any(row['sid'] in (CHILD, GRANDCHILD, ORPHAN) for row in rows)
                            agents = {row['id']: row for row in parent['agent_items']}
                            assert not agents[CHILD]['active'] and agents[GRANDCHILD]['active']
                            child = page.locator(f'#side .item.agent[data-agent="{CHILD}"]')
                            caret = page.locator(f'#side .item[data-uid="{parent["uid"]}"] .nest-caret')
                            if not child.is_visible():
                                caret.click()
                            expect(child).to_be_visible()
                            caret.click()
                            expect(child).not_to_be_visible()
                            caret.click()
                            child.click()
                            expect(page.locator('#msgs')).to_contain_text('History ' + CHILD)
                            assert page.evaluate('S.agent') == CHILD
                            page.screenshot(path=str(screenshots / f'{theme}-{width}-child.png'))
                            page.reload()
                            expect(page.locator('#msgs')).to_contain_text('History ' + CHILD)
                            if width == 1280:
                                page.locator(f'#side .item.agent[data-agent="{GRANDCHILD}"]').click()
                                expect(page.locator('#msgs')).to_contain_text('History ' + GRANDCHILD)
                                child.click()
                            if not restart and width == 1280 and theme == 'light':
                                initial = get_json(opener, base, '/api/messages/' + parent['uid'] + '?agent=' + CHILD)
                                path = native / 'brain' / CHILD / '.system_generated/logs/transcript_full.jsonl'
                                with path.open('ab') as stream:
                                    stream.write(encoded(record(1, 'PLANNER_RESPONSE', 'agy child live tail')))
                                expect(page.locator('#msgs')).to_contain_text('agy child live tail')
                                delta = get_json(opener, base, '/api/messages/' + parent['uid'] + '?agent=' + CHILD
                                    + '&start=' + str(initial['end']) + '&anchor=' + initial['anchor']
                                    + '&head=' + initial['version']['head'])
                                assert not delta['reset'] and [m['text'] for m in delta['messages']] == ['agy child live tail']
                                old = page.locator('#stat').get_attribute('data-seq') or ''
                                page.locator('#q').fill('agy child live tail')
                                page.locator('#q').press('Enter')
                                page.wait_for_function('old => String(document.querySelector("#stat").dataset.seq || "") !== old', arg=old)
                                child.click()
                                expect(page.locator('#msgs')).to_contain_text('agy child live tail')
                                assert page.evaluate('S.agent') == CHILD
                                page.locator('#q').fill('')
                                page.locator('#q').press('Enter')
                                expect(page.locator('#side-search-state')).to_be_hidden()
                            if width == 390:
                                page.locator('.dhead .mobile-back').first.click()
                            page.screenshot(path=str(screenshots / f'{theme}-{width}-tree.png'))
                            assert not errors, errors
                            context.close()
                            print(f'PASS native Agy ownership: {theme} {width}, restart={restart}', flush=True)
                        # Native parent changes must invalidate graph ownership without metadata writes.
                        if restart:
                            context = browser.new_context()
                            page = context.new_page()
                            page.goto(base)
                            execute(db, 'UPDATE conversation_summaries SET parent_conversation_id=? WHERE conversation_id=?', ('', CHILD))
                            page.wait_for_function('sid => S.sessions.some(r => r.sid === sid && r.agents === 1)', arg=CHILD)
                            uid = page.evaluate('sid => S.sessions.find(r => r.sid === sid).uid', CHILD)
                            page.locator(f'#side .item[data-uid="{uid}"]').click()
                            expect(page.locator('#msgs')).to_contain_text('agy child live tail')
                            context.close()
                            print('PASS native parent change updates ownership', flush=True)
        finally:
            browser.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-agy-nesting-review'))
    args = parser.parse_args()
    run(args.binary, args.screenshots)
