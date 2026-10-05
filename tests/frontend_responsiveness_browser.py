#!/usr/bin/env python3
"""Large sidebar navigation must leave unrelated rows and messages alone.

Real Chromium clicks/filtering/reloads over a private Rust fixture; no native
CLI or production sessions. Timings are reported, never used as machine gates.
"""
import argparse
import json
import os
import re
from pathlib import Path
import tempfile
import time

from playwright.sync_api import expect, sync_playwright

from frontend_framework_browser import launch_chromium
from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
from sidebar_closed_groups_browser import fixture


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-responsiveness-') as temporary:
        root = Path(temporary)
        for source in ('claude', 'codex', 'grok'):
            (root / source).mkdir()
        corpus = Corpus(root)
        for sid in ('speed-main', 'speed-other'):
            corpus.put(sid, 'codex', [
                codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/responsiveness'}),
                *[codex_message('user' if i % 2 == 0 else 'assistant', f'{sid} message {i}') for i in range(600)],
                codex_message('assistant', f'END {sid}')], [])
        with isolated_server(corpus, args.binary) as (base, _), sync_playwright() as playwright:
            browser = launch_chromium(playwright)
            context = browser.new_context(viewport={'width': 1400, 'height': 900}, service_workers='block')
            try:
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(base, wait_until='networkidle')
                page.wait_for_function('S.sessions.length===2 && T.listLoaded')
                page.route('**/api/sessions*', lambda route: route.fulfill(json={'unchanged': True}))
                page.evaluate('''rows=>{S.sessions=[...S.sessions,...rows];S.closed=new Set();
                    S.nest=false;S.off.clear();renderSide()}''', fixture()[:2000])
                # Above the windowing threshold the DOM holds only rows near the viewport.
                logical = '''[...document.querySelectorAll('#side > .group')]
                    .flatMap(group => group._rows || []).filter(row => !row.agent).length'''
                page.wait_for_function(logical + ' === 2002')
                assert 0 < page.locator('#side .item').count() < 200
                page.evaluate('''() => {
                  window.__changedRows=new Set();
                  window.__rowObserver=new MutationObserver(records=>{
                    for(const record of records){const row=record.target instanceof Element
                      ? record.target.closest('.item') : record.target.parentElement?.closest('.item');
                      if(row)__changedRows.add(row.dataset.uid || row.dataset.owner)}
                  });
                  __rowObserver.observe(document.querySelector('#side'),{subtree:true,attributes:true,childList:true});
                }''')
                timings = []
                for sid in ('speed-main', 'speed-other', 'speed-main'):
                    page.evaluate('__changedRows.clear()')
                    started = time.monotonic()
                    page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
                    expect(page.locator('#msgs')).to_contain_text(f'END {sid}')
                    timings.append(round((time.monotonic() - started) * 1000, 1))
                    page.evaluate('''() => window.__firstMessage=document.querySelector('#msgs .msg[data-role="user"]')''')
                    # Periodic live paints must retain the actual loaded conversation.
                    page.evaluate('paintLive()')
                    assert page.evaluate("__firstMessage === document.querySelector('#msgs .msg[data-role=user]')")
                page.locator('#q').fill('no-match-responsiveness')
                expect(page.locator('#side .item')).to_have_count(0)
                page.locator('#q').fill('')
                page.wait_for_function(logical + ' === 2002')
                assert 0 < page.locator('#side .item').count() < 200
                page.locator(f'#side .item[data-uid="{corpus.uid("speed-other")}"]').click()
                expect(page.locator('#msgs')).to_contain_text('END speed-other')
                # An actual decoded catalog update must retain unrelated UI rows,
                # while parent and child metadata changes still reach their controls.
                rows = page.evaluate('S.sessions')
                owner = next(row for row in rows if row['uid'] == 'claude:fold-0')
                owner['title'] = 'Changed parent sentinel'
                response = {'sessions': rows, 'sig': 'responsiveness-parent'}
                page.route('**/api/sessions*', lambda route: route.fulfill(json=response))
                page.evaluate('''window.__previousRows=new Map(S.sessions.map(row=>[row.uid,row]))''')
                page.evaluate('pollSessions()')
                expect(page.locator('#side .item[data-uid="claude:fold-0"] .t')).to_have_text('Changed parent sentinel')
                page.evaluate('''window.__previousRows=new Map(S.sessions.map(row=>[row.uid,row]))''')
                owner['agent_items'][0]['title'] = 'Changed worker sentinel'
                response['sig'] = 'responsiveness-child'
                page.evaluate('pollSessions()')
                expect(page.locator('#side .item.agent[data-owner="claude:fold-0"][data-agent="agent-0"] .t')).to_have_text('Changed worker sentinel')
                assert not errors, errors
                print('PASS frontend_responsiveness_browser: 2002 sessions, targeted selection, '
                      'stable live history, filtering and cached navigation; open_ms=' + json.dumps(timings), flush=True)
            finally:
                context.close()
                browser.close()


if __name__ == '__main__':
    main()
