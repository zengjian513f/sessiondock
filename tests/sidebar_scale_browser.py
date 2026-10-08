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
across list updates. Native fixture activity invalidates the list while
the selection and scroll stay put. A small synthetic mix then checks multi-select
delete confirmation, cancel, and request order; those requests are answered in
the browser and never delete fixture files. Budgets are generous;
``--report-only`` prints timings without enforcing them (used for the
pre-windowing baseline).
"""

import argparse
from datetime import datetime, timedelta, timezone
import json
import re
from pathlib import Path
import tempfile

from browser_runtime import choose_toolbar_option
from playwright.sync_api import expect, sync_playwright

from frontend_framework_browser import launch_chromium
from header_fold_browser import SID, corpus
from history_fixtures import BINARY, claude_row, isolated_server

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
    choose_toolbar_option(page, "view", 'date')
    page.wait_for_function("S.view === 'date' && document.querySelector('#side .item .cwd-path')")
    assert page.locator('#side .item').count() <= ROW_BUDGET
    page.evaluate("document.querySelector('#side').scrollTop = document.querySelector('#side').scrollHeight / 2")
    page.wait_for_function('''() => {
      const side = document.querySelector('#side'), box = side.getBoundingClientRect();
      return [...side.querySelectorAll('.item[data-uid]')].some(node => {
        const r = node.getBoundingClientRect(); return r.top >= box.top && r.bottom <= box.bottom; });
    }''')
    assert page.locator('#side .item').count() <= ROW_BUDGET
    choose_toolbar_option(page, "view", 'tree')
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


def check_poll_keeps_selection_and_scroll(page, listed, data):
    """Native activity triggers the real SSE/poll path around the selected row."""
    page.evaluate("document.querySelector('#side').scrollTop = 60000")
    frame(page)
    uid = page.evaluate('''() => {
      const side = document.querySelector('#side'), top = side.getBoundingClientRect().top + 80;
      const node = [...side.querySelectorAll('.item[data-uid]')].find(row => row.getBoundingClientRect().top >= top);
      return node ? node.dataset.uid : '';
    }''')
    assert uid, 'no visible row to select before the list refresh'
    row(page, uid).locator('.t').click()
    page.wait_for_function('selected => S.sel === selected', arg=uid)
    expect(row(page, uid)).to_have_class(re.compile(r'\bsel\b'))
    frame(page)
    before = side_state(page)
    sessions = page.evaluate('S.sessions')
    target = next(session for session in sessions
                  if session.get('uid') != uid and str(session.get('title', '')).startswith('Scale row'))
    target['title'] = 'Poll refresh sentinel'
    listed['sessions'] = sessions
    listed['sig'] = 'scale-poll-1'
    with page.expect_response(lambda response: response.request.method == 'GET'
                               and '/api/sessions' in response.url, timeout=15000) as caught:
        # Membership changes invalidate the list; appends only update unread
        # summaries and intentionally do not request the full list.
        data.put('scale-new', 'claude',
                 [claude_row('scale-new', 'user', 'u0', None, 'New scale fixture')], [])
    payload = caught.value.json()
    page.wait_for_function("S.sig === 'scale-poll-1'")
    assert payload['sig'] == 'scale-poll-1', payload.get('sig')
    frame(page)
    after = side_state(page)
    assert abs(after['top'] - before['top']) <= 1 and abs(after['height'] - before['height']) <= 400, (before, after)
    expect(row(page, uid)).to_have_class(re.compile(r'\bsel\b'))
    page.locator('#q').fill('"Poll refresh sentinel"')
    page.wait_for_function("document.querySelector('#side-search-count')?.textContent === '1 条'")
    expect(page.locator('#side .item .t', has_text='Poll refresh sentinel')).to_be_visible()


def confirm_dialog(page):
    dialog = page.locator('dialog.app-popup')
    expect(dialog).to_be_visible()
    return dialog


def cancel_confirm(page, title, body):
    dialog = confirm_dialog(page)
    expect(dialog.locator('h2')).to_have_text(title)
    expect(dialog.locator('.app-popup-message')).to_have_text(body)
    dialog.locator('[data-popup-action="cancel"]').click()
    expect(page.locator('dialog.app-popup')).to_have_count(0)


def menu_delete(page, uid):
    row(page, uid).click(button='right')
    delete = page.locator('#item-menu [data-act="delete"]')
    expect(delete).to_be_visible()
    expect(delete).to_have_text('删除会话')
    expect(delete).not_to_have_attribute('aria-disabled', 'true')
    delete.click()


def check_pick_delete_confirm_cancel(page, listed):
    """Mixed multi-select: confirmation copy, cancel, then blocked request order."""
    stamp = '2026-10-01T00:00:00+00:00'
    def recorded(source, sid, title):
        return dict(uid=f'{source}:{sid}', sid=sid, source=source, title=title,
                    cwd='/synthetic/mix', created=stamp, updated=stamp, size=10)
    rows = [
        recorded('claude', 'mix-a', '普通会话甲'),
        recorded('opencode', 'mix-c', 'OpenCode 丙'),
        recorded('agy', 'mix-d', 'Agy 丁'),
        recorded('codex', 'mix-e', '运行中戊'),
    ]
    # Pending discard follows this list, not the order the rows are picked.
    pending = [
        dict(name='mix-pending-b', source='claude', title='新建乙', cwd='/synthetic/mix',
             record_id='rec-b', instance_id='inst-b', running=False, stale=True, state='exited'),
        dict(name='mix-pending-a', source='codex', title='新建甲', cwd='/synthetic/mix',
             record_id='rec-a', instance_id='inst-a', running=False, stale=True, state='exited'),
    ]
    listed['sessions'] = rows
    listed['sig'] = 'mix-delete'
    terms = page.evaluate('''() => ({
      enabled: true, sessions: T.list || [], sources: T.sources || {},
      resume_sources: T.resume_sources || {}, home: T.home || '', errors: [],
    })''')
    terms['pending'] = pending
    calls = []

    def fulfill_live(route):
        route.fulfill(json={'uids': ['codex:mix-e'], 'tmux_uids': [], 'working_uids': []})

    def fulfill_terms(route):
        route.fulfill(json=terms)

    def blocked(kind):
        def handler(route):
            calls.append((kind, route.request.post_data_json or {}))
            route.fulfill(json={'error': 'blocked'})
        return handler

    block_delete = blocked('delete')
    block_discard = blocked('discard')
    block_kill = blocked('kill')
    page.route('**/api/live*', fulfill_live)
    page.route('**/api/term/list*', fulfill_terms)
    page.route('**/api/sessions/delete', block_delete)
    page.route('**/api/term/discard', block_discard)
    page.route('**/api/term/kill', block_kill)
    try:
        page.reload(wait_until='domcontentloaded')
        page.wait_for_function('''() => T.listLoaded && S.live.has("codex:mix-e")
          && ["claude:mix-a", "opencode:mix-c", "agy:mix-d", "tmux:mix-pending-a", "tmux:mix-pending-b"]
            .every(uid => !!document.querySelector('#side .item[data-uid="' + uid + '"]'))''')
        menu_delete(page, 'claude:mix-a')
        cancel_confirm(page, '删除会话「普通会话甲」?', '文件会移入服务端回收站，不会永久删除。')
        menu_delete(page, 'opencode:mix-c')
        cancel_confirm(page, '删除会话「OpenCode 丙」?',
                       'OpenCode 会话会从 OpenCode 直接删除（连同子会话），不进回收站，无法恢复。')
        menu_delete(page, 'agy:mix-d')
        cancel_confirm(page, '删除会话「Agy 丁」?',
                       '其中 1 个 Agy 会话将跳过：请在 agy 的 /resume 菜单删除原生会话，SessionDock 会自动更新列表。')
        assert calls == [], calls
        row(page, 'claude:mix-a').click(button='right')
        page.locator('#item-menu [data-act="pick"]').click()
        for uid in ('tmux:mix-pending-a', 'opencode:mix-c', 'agy:mix-d', 'codex:mix-e', 'tmux:mix-pending-b'):
            row(page, uid).locator('.t').click()
        expect(page.locator('#side-picked')).to_have_text('已选 6 项')
        expect(page.locator('#side-pick-delete')).to_have_text('删除 / 丢弃 (6)')
        page.locator('#side-pick-delete').click()
        cancel_confirm(
            page, '删除 / 丢弃选中的 6 个会话?',
            '2 个新建会话将停止并丢弃，未发送的草稿也会清除；若已生成会话记录，记录会保留。\n'
            '文件会移入服务端回收站，不会永久删除。\n'
            '其中 1 个 Agy 会话将跳过：请在 agy 的 /resume 菜单删除原生会话，SessionDock 会自动更新列表。\n'
            '其中 1 个 OpenCode 会话会从 OpenCode 直接删除（连同子会话），不进回收站，无法恢复。\n'
            '其中 1 个还在运行，会被跳过，需要先停止。')
        expect(page.locator('#side-picked')).to_have_text('已选 6 项')
        for uid in ('claude:mix-a', 'opencode:mix-c', 'agy:mix-d', 'codex:mix-e',
                    'tmux:mix-pending-a', 'tmux:mix-pending-b'):
            expect(row(page, uid)).to_be_visible()
        assert calls == [], calls
        page.locator('#side-pick-delete').click()
        confirm_dialog(page).locator('[data-popup-action="ok"]').click()
        alert = page.locator('dialog.app-popup')
        expect(alert.locator('h2')).to_have_text('已删除 / 丢弃 0 个，6 个操作失败:')
        expect(alert.locator('.app-popup-message')).to_have_text(
            '· 新建乙: blocked\n· 新建甲: blocked\n· claude:mix-a: blocked\n'
            '· opencode:mix-c: blocked\n· agy:mix-d: blocked\n…')
        alert.locator('[data-popup-action="ok"]').click()
        expect(page.locator('dialog.app-popup')).to_have_count(0)
        assert [kind for kind, _ in calls] == ['discard', 'discard', 'delete'], calls
        assert calls[0][1]['record_id'] == 'rec-b' and calls[1][1]['record_id'] == 'rec-a', calls
        assert calls[2][1]['uids'] == ['claude:mix-a', 'opencode:mix-c', 'agy:mix-d', 'codex:mix-e'], calls
        expect(page.locator('#side-picked')).to_have_text('已选 6 项')
        for uid in ('claude:mix-a', 'opencode:mix-c', 'agy:mix-d', 'codex:mix-e',
                    'tmux:mix-pending-a', 'tmux:mix-pending-b'):
            expect(row(page, uid)).to_be_visible()
    finally:
        page.unroute('**/api/live*', fulfill_live)
        page.unroute('**/api/term/list*', fulfill_terms)
        page.unroute('**/api/sessions/delete', block_delete)
        page.unroute('**/api/term/discard', block_discard)
        page.unroute('**/api/term/kill', block_kill)


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
                    check_poll_keeps_selection_and_scroll(page, listed, data)
                    check_pick_delete_confirm_cancel(page, listed)
                assert not errors, errors
                print(f'PASS sidebar_scale_browser: {len(rows) + len(real)} sessions, all groups open'
                      + ('' if args.report_only else '; window bounded, deep link, cross-window drag range, '
                         'group pick, search, folds, date view, text selection and scroll restore, '
                         'SSE refresh keeps the selection and scroll, '
                         'multi-select delete confirms, cancels, and keeps request order'), flush=True)
            finally:
                context.close()
                browser.close()


if __name__ == '__main__':
    main()
