#!/usr/bin/env python3
"""Fully expanded large sidebar: visible-window rendering and its semantics.

An isolated Rust service supplies two real Claude sessions; the list endpoint is
replaced in Chromium by ~4000 synthetic rows across 40 project groups and three
sources, with nested children and subagent rows, every group and branch open.
The suite measures initial render, single-row rerender, live repaint, scroll
long tasks and DOM size, then exercises the user paths windowing must keep:
wheel scrolling, a deep link to a row far below the window, a drag range that
crosses unrendered rows, a group checkbox, search filtering, group and branch
collapse/expand, text selection inside a rendered row, and scroll restoration
across list updates. Budgets are generous; ``--report-only`` prints timings
without enforcing them (used for the pre-windowing baseline).
"""

import argparse
from datetime import datetime, timedelta, timezone
import json
import re
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright

from frontend_framework_browser import launch_chromium
from header_fold_browser import SID, corpus
from history_parity import BINARY, isolated_server

GROUPS = 40
PER_GROUP = 100
SOURCES = ('claude', 'codex', 'grok')
BASE = datetime(2026, 9, 30, tzinfo=timezone.utc)
# DOM rows stay proportional to the viewport, not to the 4000-row corpus.
ROW_BUDGET = 240
RERENDER_BUDGET_MS = 250


def fixture():
    rows = []
    for group in range(GROUPS):
        source = SOURCES[group % len(SOURCES)]
        for offset in range(PER_GROUP):
            index = group * PER_GROUP + offset
            stamp = (BASE - timedelta(minutes=index)).isoformat()
            row = dict(uid=f'{source}:scale-{index:04d}', sid=f'scale-{index:04d}', source=source,
                       title=f'Scale row {index:04d}', cwd=f'/synthetic/scale-{group:02d}',
                       created=stamp, updated=stamp, size=1000 + index)
            if offset in (1, 2):
                row['nest_parent'] = dict(source=source, sid=f'scale-{group * PER_GROUP:04d}')
            if offset == 0:
                row['agent_items'] = [dict(id=f'worker-{a}', title=f'Scale worker {index:04d}.{a}',
                                           type='general', created=stamp, updated=stamp) for a in range(2)]
            rows.append(row)
    return rows


SEED = '''rows => {
  S.sel = null; S.agent = null; S.results = null; S.term = ''; S.off.clear(); S.activeOnly = false;
  S.view = 'tree'; S.nest = true; S.closed = new Set(); S.nestClosed = new Set();
  S.sessions = rows; renderView(); renderSide();
}'''

MEASURE = '''async () => {
  const side = document.querySelector('#side');
  const frame = () => new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)));
  const timed = fn => { const a = performance.now(); fn(); void side.scrollHeight; void side.firstElementChild?.offsetHeight;
    return Math.round((performance.now() - a) * 10) / 10; };
  const result = {};
  side.scrollTop = 0;
  side.replaceChildren();
  await frame();
  result.initial_ms = timed(() => renderSide());
  await frame();
  result.rerender_unchanged_ms = timed(() => renderSide());
  const changed = index => {
    S.sessions = S.sessions.map((row, i) => i === index ? {...row, title: row.title + ' *'} : row);
  };
  changed(S.sessions.findIndex(row => row.sid === 'scale-1505'));
  result.update_offscreen_ms = timed(() => patchSide(visible()));
  changed(S.sessions.findIndex(row => row.sid === 'scale-0003'));
  result.update_visible_ms = timed(() => patchSide(visible()));
  S.live.add(S.sessions.find(row => row.sid === 'scale-0004').uid);
  result.paint_live_ms = timed(() => paintLive());
  S.live.clear();
  paintLive();
  result.logical_rows = [...side.querySelectorAll(':scope > .group')]
    .reduce((sum, group) => sum + (group._rows?.length || 0), 0);
  result.dom_rows = side.querySelectorAll('.item').length;
  result.dom_nodes = side.getElementsByTagName('*').length;
  return result;
}'''

SCROLL_PROBE = '''() => {
  window.__scaleTasks = [];
  window.__scaleFrames = {last: performance.now(), max: 0, on: true};
  new PerformanceObserver(list => { for (const entry of list.getEntries()) __scaleTasks.push(entry.duration); })
    .observe({type: 'longtask'});
  const tick = now => {
    if (!__scaleFrames.on) return;
    __scaleFrames.max = Math.max(__scaleFrames.max, now - __scaleFrames.last);
    __scaleFrames.last = now;
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}'''


def frame(page):
    page.evaluate('new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)))')


def side_state(page):
    return page.evaluate('''() => {
      const side = document.querySelector('#side');
      return {top: side.scrollTop, height: side.scrollHeight, rows: side.querySelectorAll('.item').length};
    }''')


def row(page, uid):
    return page.locator(f'#side .item[data-uid="{uid}"]')


def in_viewport(page, selector):
    return page.evaluate('''selector => {
      const side = document.querySelector('#side'), node = side.querySelector(selector);
      if (!node) return false;
      const box = side.getBoundingClientRect(), r = node.getBoundingClientRect();
      return r.bottom > box.top && r.top < box.bottom;
    }''', selector)


def measure(page, report_only):
    metrics = page.evaluate(MEASURE)
    frame(page)
    page.evaluate(SCROLL_PROBE)
    page.locator('#side').hover()
    for _ in range(50):
        page.mouse.wheel(0, 700)
        page.wait_for_timeout(16)
    for _ in range(20):
        page.mouse.wheel(0, -700)
        page.wait_for_timeout(16)
    frame(page)
    scroll = page.evaluate('''() => { __scaleFrames.on = false;
      return {long_tasks: __scaleTasks.length, long_task_ms: Math.round(__scaleTasks.reduce((a, b) => a + b, 0)),
              max_frame_gap_ms: Math.round(__scaleFrames.max)}; }''')
    metrics.update(scroll)
    metrics['dom_rows_after_scroll'] = page.locator('#side .item').count()
    print('METRICS ' + json.dumps(metrics), flush=True)
    assert metrics['logical_rows'] == GROUPS * PER_GROUP + 2 * GROUPS + 2, metrics
    if report_only:
        return
    assert metrics['dom_rows'] <= ROW_BUDGET and metrics['dom_rows_after_scroll'] <= ROW_BUDGET, metrics
    for key in ('rerender_unchanged_ms', 'update_offscreen_ms', 'update_visible_ms', 'paint_live_ms'):
        assert metrics[key] <= RERENDER_BUDGET_MS, (key, metrics)


def check_scroll_content(page):
    """Jump to the very bottom: the last logical row is real, visible and clickable."""
    page.evaluate("document.querySelector('#side').scrollTop = 0")
    expect(row(page, 'claude:scale-0000')).to_be_visible()
    last_uid = page.evaluate("[...document.querySelectorAll('#side > .group')].at(-1)._rows.at(-1).s.uid")
    page.evaluate("document.querySelector('#side').scrollTop = document.querySelector('#side').scrollHeight")
    page.wait_for_function('''uid => {
      const side = document.querySelector('#side'), box = side.getBoundingClientRect();
      const node = side.querySelector(`.item[data-uid="${uid}"]`), r = node?.getBoundingClientRect();
      return !!r && r.top < box.bottom && r.bottom <= box.bottom + 1;
    }''', arg=last_uid)
    assert row(page, 'claude:scale-3999').count() == 1
    last = row(page, last_uid)
    last.locator('.t').click()
    page.wait_for_function('uid => S.sel === uid', arg=last_uid)
    expect(last).to_have_class(re.compile(r'\bsel\b'))
    assert page.locator('#side .item').count() <= ROW_BUDGET


def check_deep_link(page, base, real_uid):
    """A shared link to the oldest row reveals it far below the rendered window."""
    page.goto(f'{base}/?sid=claude:{SID}', wait_until='networkidle')
    page.wait_for_function('uid => S.sel === uid', arg=real_uid)
    target = row(page, real_uid)
    expect(target).to_have_class(re.compile(r'\bsel\b'))
    page.wait_for_function('''uid => {
      const side = document.querySelector('#side'), node = side.querySelector(`.item[data-uid="${uid}"]`);
      if (!node) return false;
      const box = side.getBoundingClientRect(), r = node.getBoundingClientRect();
      return side.scrollTop > 100000 && r.top >= box.top - 1 && r.bottom <= box.bottom + 1;
    }''', arg=real_uid)
    assert page.locator('#side .item').count() <= ROW_BUDGET
    expect(page.locator('.dhead h2')).to_contain_text('Fold sweep question')


def check_drag_range(page):
    """多选 drag from the top: auto-scroll carries the range over unrendered rows."""
    page.evaluate("document.querySelector('#side').scrollTop = 0")
    row(page, 'claude:scale-0003').click(button='right')
    page.locator('#item-menu [data-act="pick"]').click()
    page.wait_for_function("S.picking && !!document.querySelector('#side .item-pick')")
    row(page, 'claude:scale-0003').locator('.t').click()   # entering picked it; start from none
    page.wait_for_function('pickedSessions.size === 0')
    anchor = row(page, 'claude:scale-0003')
    box = anchor.bounding_box()
    page.mouse.move(box['x'] + box['width'] / 2, box['y'] + box['height'] / 2)
    page.mouse.down()
    side = page.locator('#side').bounding_box()
    page.mouse.move(box['x'] + box['width'] / 2, side['y'] + side['height'] - 4, steps=6)
    page.wait_for_function('pickedSessions.size > 320', timeout=20000)
    page.mouse.up()
    picked = page.evaluate('''() => {
      const order = sidebarPickOrder();
      const set = [...pickedSessions];
      const idx = set.map(uid => order.indexOf(uid)).sort((a, b) => a - b);
      return {size: set.length, first: idx[0], last: idx.at(-1), contiguous: idx.every((v, i) => v === idx[0] + i),
              anchor: order.indexOf('claude:scale-0003'), dom: document.querySelectorAll('#side .item').length};
    }''')
    assert picked['contiguous'] and picked['first'] == picked['anchor'], picked
    assert picked['size'] > 320 and picked['dom'] <= ROW_BUDGET, picked
    expect(page.locator('#side-picked')).to_have_text(f"已选 {picked['size']} 项")
    # Group checkboxes follow logical membership: rows above the anchor stay out,
    # the next group was crossed entirely while mostly unrendered.
    groups = page.locator('#side > .group')
    assert groups.nth(0).locator('.ghead-pick').evaluate('box => box.indeterminate')
    assert groups.nth(1).locator('.ghead-pick').is_checked()
    assert page.evaluate("pickedSessions.has('codex:scale-0150') && !pickedSessions.has('claude:scale-0002')")
    # Group checkbox covers rows that were never rendered.
    page.locator('#side-pick-cancel').click()
    page.evaluate('setPicking(true)')
    page.evaluate("document.querySelector('#side').scrollTop = 0")
    second = page.locator('#side > .group').nth(1)
    second.locator('.ghead-pick').click()
    page.wait_for_function("document.querySelector('#side-picked').textContent === '已选 100 项'")
    assert page.evaluate("pickedSessions.has('codex:scale-0199')")
    page.locator('#side-pick-cancel').click()


def check_search_and_folds(page):
    page.evaluate("document.querySelector('#side').scrollTop = 0")
    page.locator('#q').fill('"Scale row 37"')
    page.wait_for_function("document.querySelector('#side-search-count')?.textContent === '100 条'")
    expect(row(page, 'codex:scale-3700')).to_be_visible()
    assert page.locator('#side .item').count() <= 120
    page.locator('#q').fill('')
    page.wait_for_function("!S.term && document.querySelector('#side .item[data-uid=\"claude:scale-0000\"]')")
    # Group collapse/expand keeps the window bounded and the logical count.
    first = page.locator('#side > .group').first
    first.locator('.ghead').click()
    expect(first).to_have_class(re.compile(r'\bclosed\b'))
    expect(first.locator('.gcount')).to_have_text('100')
    first.locator('.ghead').click()
    expect(first).not_to_have_class(re.compile(r'\bclosed\b'))
    # Branch fold: the parent's children and workers vanish and return in place.
    parent = row(page, 'claude:scale-0000')
    parent.locator('.nest-caret').click()
    expect(row(page, 'claude:scale-0001')).to_have_count(0)
    expect(page.locator('#side .item.agent[data-owner="claude:scale-0000"]')).to_have_count(0)
    expect(row(page, 'claude:scale-0003')).to_be_visible()
    parent.locator('.nest-caret').click()
    expect(row(page, 'claude:scale-0001')).to_be_visible()
    expect(page.locator('#side .item.agent[data-owner="claude:scale-0000"]')).to_have_count(2)
    assert page.locator('#side .item').count() <= ROW_BUDGET


def check_date_view(page):
    """Timeline view rows carry a directory line; windowing measures them separately."""
    page.locator('#view [data-v="date"]').click()
    page.wait_for_function("S.view === 'date' && document.querySelector('#side .item .cwd-path')")
    assert page.locator('#side .item').count() <= ROW_BUDGET
    page.evaluate("document.querySelector('#side').scrollTop = document.querySelector('#side').scrollHeight / 2")
    page.wait_for_function('''() => {
      const side = document.querySelector('#side'), box = side.getBoundingClientRect();
      return [...side.querySelectorAll('.item[data-uid]')].some(node => {
        const r = node.getBoundingClientRect(); return r.top >= box.top && r.bottom <= box.bottom; });
    }''')
    assert page.locator('#side .item').count() <= ROW_BUDGET
    page.locator('#view [data-v="tree"]').click()
    page.wait_for_function("S.view === 'tree'")


def check_selection_and_restore(page):
    """Text selection survives scrolling and updates; scroll position survives rerenders."""
    page.evaluate("document.querySelector('#side').scrollTop = 60000")
    frame(page)
    uid = page.evaluate('''() => {
      const side = document.querySelector('#side'), top = side.getBoundingClientRect().top + 80;
      return [...side.querySelectorAll('.item[data-uid]')].find(node => node.getBoundingClientRect().top >= top).dataset.uid;
    }''')
    title = row(page, uid).locator('.t')
    text = title.text_content()
    box = title.bounding_box()
    page.mouse.move(box['x'] + 2, box['y'] + box['height'] / 2)
    page.mouse.down()
    page.mouse.move(box['x'] + box['width'] - 2, box['y'] + box['height'] / 2, steps=5)
    page.mouse.up()
    assert page.evaluate('getSelection().toString()') == text
    page.locator('#side').hover()
    page.mouse.wheel(0, 2500)
    frame(page)
    page.mouse.wheel(0, -2500)
    frame(page)
    assert page.evaluate('getSelection().toString()') == text
    page.evaluate('getSelection().removeAllRanges()')
    frame(page)
    before = side_state(page)
    rows = page.evaluate('S.sessions')
    rows[2500]['title'] = 'Restore sentinel'
    page.evaluate('rows => { S.sessions = rows; patchSide(visible()); }', rows)
    frame(page)
    after = side_state(page)
    assert abs(after['top'] - before['top']) <= 1 and abs(after['height'] - before['height']) <= 400, (before, after)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--report-only', action='store_true', help='print timings without budgets or checks')
    args = parser.parse_args()
    rows = fixture()
    with tempfile.TemporaryDirectory(prefix='sessiondock-sidebar-scale-') as temporary:
        data = corpus(Path(temporary))
        with isolated_server(data, args.binary) as (base, _), sync_playwright() as playwright:
            browser = launch_chromium(playwright)
            context = browser.new_context(viewport={'width': 1400, 'height': 900}, service_workers='block')
            try:
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.goto(base, wait_until='networkidle')
                page.wait_for_function('S.sessions.length === 2 && T.listLoaded')
                real = page.evaluate('S.sessions')
                oldest = (BASE - timedelta(days=30)).isoformat()
                for session in real:
                    session['updated'] = oldest
                listed = {'sessions': rows + real, 'sig': 'scale-0'}
                real_uid = next(s['uid'] for s in real if s['sid'] == SID)

                def sessions(route):
                    if route.request.url.split('?')[0].endswith('/api/sessions'):
                        route.fulfill(json=listed)
                    else:
                        route.continue_()
                context.route('**/api/sessions*', sessions)
                page.evaluate(SEED, rows + real)
                measure(page, args.report_only)
                if not args.report_only:
                    check_scroll_content(page)
                    check_deep_link(page, base, real_uid)
                    page.evaluate(SEED, page.evaluate('S.sessions'))
                    check_drag_range(page)
                    check_search_and_folds(page)
                    check_date_view(page)
                    check_selection_and_restore(page)
                assert not errors, errors
                print(f'PASS sidebar_scale_browser: {len(rows) + len(real)} sessions, all groups open'
                      + ('' if args.report_only else '; window bounded, deep link, cross-window drag range, '
                         'group pick, search, folds, date view, text selection and scroll restore'), flush=True)
            finally:
                context.close()
                browser.close()


if __name__ == '__main__':
    main()
