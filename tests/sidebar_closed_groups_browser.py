#!/usr/bin/env python3
"""Mostly folded sidebar groups: hidden work, refresh, counts and lazy expansion.

Synthetic list responses over an isolated service; Chromium clicks real group
and branch disclosures, then receives updates through the normal list poll.
"""

import argparse
from datetime import datetime, timedelta, timezone
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from header_fold_browser import corpus
from history_fixtures import BINARY, isolated_server

GROUPS = 50
PER_GROUP = 100


def fixture():
    rows = []
    for group in range(GROUPS):
        stamp = (datetime(2026, 9, 20, tzinfo=timezone.utc) - timedelta(days=group)).isoformat()
        for offset in range(PER_GROUP):
            index = group * PER_GROUP + offset
            row = dict(uid=f'claude:fold-{index}', sid=f'fold-{index}', source='claude',
                       title=f'Synthetic {index}', cwd=f'/synthetic/project-{group:02d}',
                       created=stamp, updated=stamp, size=100)
            if 1 <= offset <= 5:
                row['nest_parent'] = dict(source='claude', sid=f'fold-{group * PER_GROUP}')
            if offset == 0:
                row['agent_items'] = [dict(id=f'agent-{a}', title=f'Worker {a}', type='general',
                                         updated=stamp, created=stamp) for a in range(20)]
            if group == 1 and offset == 1:
                row['agent_items'] = [dict(id='nested-worker', title='Nested worker', type='general',
                                          updated=stamp, created=stamp)]
            rows.append(row)
    return rows


def seed(page, rows, view='tree'):
    page.evaluate('''({rows, view}) => {
      S.sel = null; S.agent = null; S.results = null; S.term = ''; S.off.clear();
      S.activeOnly = false; S.nest = true; S.view = view; S.sessions = rows; S.sig = 'fold-0';
      S.nestClosed = new Set(rows.filter(s => s.agent_items && s.uid !== 'claude:fold-101').map(s => s.uid));
      const keys = [...new Set(rows.map(s => view === 'tree' ? s.cwd : dayKey(s.updated)))];
      S.closed = new Set(keys.slice(1)); renderView(); renderSide();
    }''', dict(rows=rows, view=view))


def instrument(page):
    page.evaluate('''() => {
      window.__foldWork = {rows: []};
      const expand = expandRows;
      expandRows = function(s, ...args) { __foldWork.rows.push(s.uid); return expand(s, ...args); };
    }''')


def reset_work(page):
    page.evaluate('window.__foldWork = {rows: []}')


def group(page, key):
    return page.locator('#side > .group').filter(has=page.locator(f'.gname[title="{key}"]'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-folded-groups-') as temporary:
        with isolated_server(corpus(Path(temporary)), args.binary) as (base, _), sync_playwright() as pw:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**launch)
            try:
                page = browser.new_page(viewport={'width': 1400, 'height': 900})
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(base, wait_until='networkidle')
                page.wait_for_function('S.sessions.length > 0 && T.listLoaded')
                instrument(page)
                rows = fixture()
                response = dict(sessions=rows, sig='fold-0')
                page.route('**/api/sessions*', lambda route: route.fulfill(json=response))
                seed(page, rows)
                assert page.locator('#side .item').count() == 95
                assert page.locator('#side .item.agent').count() == 0
                assert len(page.evaluate('__foldWork.rows')) == 95
                assert group(page, '/synthetic/project-01').locator('.gcount').text_content() == '95'
                # A hidden descendant changes its project's latest activity. The
                # collapsed heading moves, but no hidden session/agent is built.
                rows[101] = dict(rows[101], title='Hidden update sentinel', updated='2026-09-21T00:00:00Z')
                response['sig'] = 'fold-1'
                reset_work(page)
                page.evaluate('pollSessions()')
                page.wait_for_function("document.querySelector('#side > .group')?.dataset.key === '/synthetic/project-01'")
                assert page.locator('#side > .group').first.get_attribute('data-key') == '/synthetic/project-01'
                assert len(page.evaluate('__foldWork.rows')) == 95
                assert page.locator('#side .item.agent').count() == 0
                # Opening the group still skips its closed branch. Only opening
                # that branch creates its children in current activity order.
                hidden = group(page, '/synthetic/project-01')
                hidden.locator('.ghead').click()
                expect(hidden.locator('.item')).to_have_count(95)
                reset_work(page)
                hidden.locator('.item[data-uid="claude:fold-100"] .nest-caret').click()
                expect(hidden.locator('.item')).to_have_count(121)
                assert hidden.locator('.gcount').text_content() == '100'
                assert hidden.locator('.item[data-uid="claude:fold-101"] .t').text_content() == 'Hidden update sentinel'
                assert len(page.evaluate('__foldWork.rows')) == 6
                assert page.evaluate('''() => {
                  const n = document.querySelector('.item[data-uid="claude:fold-100"]');
                  return n.nextElementSibling.dataset.uid;
                }''') == 'claude:fold-101'
                # A closed project's checkbox must still select the same
                # exposed sessions, even though it has no session row objects.
                hidden.locator('.ghead').click()
                page.evaluate('setPicking(true)')
                page.wait_for_function("!document.querySelector('#side-tools').hidden && !!document.querySelector('#side .ghead-pick')")
                hidden.locator('.ghead-pick').click()
                page.wait_for_function('''() => document.querySelector('#side-picked').textContent.includes('100')''')
                assert hidden.locator('.ghead-pick').is_checked()
                assert page.evaluate('pickedSessions.size') == 100
                assert page.evaluate('pickedSessions.has("claude:fold-101")')
                page.locator('#side-pick-cancel').click()
                # Date view: a hidden updated session moves to an open day.
                rows = fixture()
                response['sessions'] = rows
                seed(page, rows, 'date')
                old_key = page.evaluate('dayKey(S.sessions[106].updated)')
                today_key = page.evaluate('dayKey(S.sessions[6].updated)')
                assert group(page, old_key).locator('.gcount').text_content() == '95'
                rows[106] = dict(rows[106], updated=rows[6]['updated'], title='Moved day sentinel')
                response['sig'] = 'fold-date-1'
                reset_work(page)
                page.evaluate('pollSessions()')
                assert group(page, old_key).locator('.gcount').text_content() == '94'
                assert group(page, today_key).locator('.gcount').text_content() == '96'
                assert group(page, today_key).locator('.item[data-uid="claude:fold-106"] .t').text_content() == 'Moved day sentinel'
                assert len(page.evaluate('__foldWork.rows')) == 96
                assert page.locator('#side .item.agent').count() == 0
                group(page, old_key).locator('.ghead').click()
                expect(group(page, old_key).locator('.item')).to_have_count(94)
                assert not errors, errors
                # Selection reactivity uses a fresh scene so its additional
                # clicks do not race the earlier exact lazy-expansion counters.
                rows = fixture()
                response['sessions'] = rows
                seed(page, rows)
                page.evaluate('setPicking(true)')
                hidden = group(page, '/synthetic/project-01')
                hidden.locator('.ghead-pick').click()
                hidden.locator('.ghead').click()
                page.wait_for_function('''() => document.querySelectorAll('#side .item-pick:checked').length === 95''')
                selected_row = hidden.locator('.item[data-uid="claude:fold-106"]')
                row_handle = selected_row.element_handle()
                selected_row.evaluate("node => node.scrollIntoView({block: 'center'})")
                page.evaluate('new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)))')
                assert page.locator('#side-picked').text_content() == '已选 95 项'
                reset_work(page)
                selected_row.locator('.body').click()
                page.wait_for_function('''() => document.querySelector('#side-picked').textContent.includes('94')''', timeout=5000)
                assert not selected_row.locator('.item-pick').is_checked()
                assert 'picked' not in selected_row.get_attribute('class').split()
                assert hidden.locator('.ghead-pick').evaluate('node => node.indeterminate')
                assert selected_row.evaluate('(node, old) => node === old', row_handle)
                page.locator('#side-pick-all').click()
                page.wait_for_function('''() => document.querySelector('#side-pick-all').textContent === '全不选' ''')
                assert selected_row.locator('.item-pick').is_checked()
                assert 'picked' in selected_row.get_attribute('class').split()
                page.locator('#side-pick-cancel').click()
                print('PASS closed groups browser: 5000 sessions, 49/50 folded; hidden updates, '
                      'lazy ordering, nested counts, closed-group selection and date migration')
            finally:
                browser.close()


if __name__ == '__main__':
    main()
