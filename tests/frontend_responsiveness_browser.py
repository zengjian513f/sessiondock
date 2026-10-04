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
from browser_runtime import js, scoped_frontend
from frontend_framework_browser import launch_chromium
from history_parity import BINARY, Corpus, codex_message, codex_row, isolated_server
from sidebar_closed_groups_browser import fixture


def inactive_parent_metadata(page, stable_uid):
    """A filtered parent clone still observes its original shallow catalog row."""
    # Keep accepted synthetic state fixed: a timer must not repaint the clone
    # and accidentally rescue a missing notifyRow subscription.
    page.evaluate("""stableUid => {
      const runtime = window.SessionDockRuntime;
      runtime.core.network.pause('idle');
      runtime.core.state.live.live = new Set(['claude:fold-0', stableUid]);
      runtime.core.state.sidebar.closed.clear();
      runtime.core.state.sidebar.nestClosed.clear();
    }""", stable_uid)
    page.locator('#view [data-v="tree"]').click()
    nesting = page.get_by_role('button', name='分层显示', exact=True)
    expect(nesting).to_have_attribute('aria-pressed', 'false')
    nesting.click()
    expect(nesting).to_have_attribute('aria-pressed', 'true')
    page.locator('#livecount').click()
    expect(page.locator('#livecount')).to_have_attribute('aria-checked', 'true')
    parent = page.locator('#side .item[data-uid="claude:fold-0"]')
    stable = page.locator(f'#side .item[data-uid="{stable_uid}"]')
    expect(page.locator('#side .item:not(.agent)')).to_have_count(2)
    expect(page.locator('#side .item[data-uid="claude:fold-1"]')).to_have_count(0)
    expect(parent.locator('.nest-inactive')).to_have_text('有 5 个不活跃会话（已被筛选隐藏）')
    parent_handle, stable_handle = parent.element_handle(), stable.element_handle()
    assert page.evaluate("""() => {
      const runtime = window.SessionDockRuntime;
      window.__cloneCatalog = runtime.core.state.catalog.sessions;
      window.__cloneOriginal = runtime.core.index.indexedSessions().byUid.get('claude:fold-0');
      const projected = runtime.filters.visible().find(row => row.uid === __cloneOriginal.uid);
      window.__cloneGroupings = 0;
      const groupBy = runtime.sidebarView.groupBy;
      runtime.sidebarView.groupBy = (...args) => { __cloneGroupings++; return groupBy(...args); };
      return projected !== __cloneOriginal && projected.sidebarInactiveChildren === 5;
    }""")
    before = parent.locator('.m').text_content()
    expected = page.evaluate("""() => {
      const runtime = window.SessionDockRuntime;
      __cloneOriginal.size = 8192;
      __cloneOriginal.updated = '2001-02-03T04:05:06Z';
      runtime.core.state.catalog.notifyRow(__cloneOriginal);
      return runtime.sidebarView.itemMeta(__cloneOriginal);
    }""")
    assert expected != before and '8K' in expected and '2001-02-03' in expected
    expect(parent.locator('.m')).to_have_text(expected)
    expect(parent.locator('.nest-inactive')).to_have_text('有 5 个不活跃会话（已被筛选隐藏）')
    assert parent.evaluate('(node, old) => node === old', parent_handle)
    assert stable.evaluate('(node, old) => node === old', stable_handle)
    assert page.evaluate("""() => {
      const catalog = window.SessionDockRuntime.core.state.catalog;
      return catalog.sessions === __cloneCatalog
        && catalog.sessions.find(row => row.uid === __cloneOriginal.uid) === __cloneOriginal
        && __cloneGroupings === 0;
    }""")
    page.locator('#allcount').click()
    expect(page.locator('#livecount')).to_have_attribute('aria-checked', 'false')
    expect(page.locator('#side .item[data-uid="claude:fold-1"]')).to_have_attribute('data-depth', '1')
    expect(parent.locator('.nest-inactive')).to_have_count(0)
    expect(parent.locator('.m')).to_have_text(expected)
    assert stable.evaluate('(node, old) => node === old', stable_handle)


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
                page.wait_for_function(js('S.sessions.length===2 && T.listLoaded',
                    'runtime.core.state.catalog.sessions.length===2 && runtime.terminal.state.listLoaded'))
                page.route('**/api/sessions*', lambda route: route.fulfill(json={'unchanged': True}))
                page.evaluate(js('''rows=>{S.sessions=[...S.sessions,...rows];S.closed=new Set();
                    S.nest=false;S.off.clear();renderSide()}''', '''rows=>{
                    runtime.core.state.catalog.sessions=[...runtime.core.state.catalog.sessions,...rows];
                    runtime.core.state.sidebar.closed=new Set();runtime.core.state.sidebar.nest=false;
                    runtime.core.state.sidebar.off.clear();}'''), fixture()[:2000])
                expect(page.locator('#side .item:not(.agent)')).to_have_count(2002)
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
                    if scoped_frontend():
                        assert page.evaluate('[...__changedRows].length') <= 2, page.evaluate('[...__changedRows]')
                    page.evaluate('''() => window.__firstMessage=document.querySelector('#msgs .msg[data-role="user"]')''')
                    # Periodic live paints must retain the actual loaded conversation.
                    page.evaluate(js('paintLive()', 'runtime.status.paintLive()'))
                    assert page.evaluate("__firstMessage === document.querySelector('#msgs .msg[data-role=user]')")
                if scoped_frontend():
                    assert page.evaluate("""() => {
                      const runtime = window.SessionDockRuntime;
                      return ['renderSide', 'refreshSidebarRows', 'paintSidebarSelection', 'patchSide']
                        .every(name => !(name in runtime.sidebarView))
                        && !('paintTurn' in runtime.status)
                        && !('selectPendingSidebarRow' in runtime.pendingStage);
                    }""")
                    stable = page.locator(f'#side .item[data-uid="{corpus.uid("speed-main")}"]')
                    stable_handle = stable.element_handle()
                    claude_chip = page.locator('#chips button[data-source="claude"]')
                    claude_chip.click()
                    expect(page.locator('#side .item[data-uid="claude:fold-6"]')).to_have_count(0)
                    expect(page.locator('#side .item:not(.agent)')).to_have_count(2)
                    assert stable.evaluate('(node, old) => node === old', stable_handle)
                    claude_chip.click()
                    expect(page.locator('#side .item:not(.agent)')).to_have_count(2002)
                    assert stable.evaluate('(node, old) => node === old', stable_handle)
                    nesting = page.get_by_role('button', name='分层显示', exact=True)
                    for depth in ('1', '0'):
                        nesting.click()
                        expect(nesting).to_have_attribute('aria-pressed', 'true' if depth == '1' else 'false')
                        expect(page.locator('#side .item[data-uid="claude:fold-1"]')).to_have_attribute('data-depth', depth)
                        assert stable.evaluate('(node, old) => node === old', stable_handle)
                    page.locator(f'#side .item[data-uid="{corpus.uid("speed-other")}"] .t').click()
                    expect(page.locator('#msgs')).to_contain_text('END speed-other')
                    expect(page.locator(f'#side .item[data-uid="{corpus.uid("speed-other")}"]')).to_have_class(
                        re.compile(r'\bsel\b'))
                    assert stable.evaluate('(node, old) => node === old', stable_handle)
                    changed = page.locator('#side .item[data-uid="claude:fold-6"]')
                    changed_handle = changed.element_handle()
                    page.evaluate("""() => {
                      const runtime = window.SessionDockRuntime;
                      runtime.core.state.live.live.add('claude:fold-6');
                      runtime.core.state.unread.unread.set('claude:fold-6', {count: 3});
                    }""")
                    expect(changed).to_have_class(re.compile(r'\blive\b'))
                    expect(changed.locator('.item-status')).to_have_text('3')
                    assert stable.evaluate('(node, old) => node === old', stable_handle)
                    assert changed.evaluate('(node, old) => node === old', changed_handle)
                    page.evaluate("""() => {
                      const runtime = window.SessionDockRuntime;
                      runtime.core.state.live.live.delete('claude:fold-6');
                      runtime.core.state.unread.unread.delete('claude:fold-6');
                    }""")
                    expect(changed).not_to_have_class(re.compile(r'\blive\b'))
                    expect(changed.locator('.item-status')).not_to_have_text('3')
                    assert stable.evaluate('(node, old) => node === old', stable_handle)
                    assert changed.evaluate('(node, old) => node === old', changed_handle)
                    # The actual input control keeps user whitespace while the
                    # applied sidebar filter follows the trimmed computed term.
                    page.locator('#q').fill('  speed-  ')
                    expect(page.locator('#q')).to_have_value('  speed-  ')
                    expect(page.locator('#side .item:not(.agent)')).to_have_count(2)
                    assert page.evaluate("""() => {
                      const search = window.SessionDockRuntime.core.state.search;
                      return search.query === '  speed-  ' && search.term === 'speed-'
                        && search.filterTerm === 'speed-' && !search.filterPending;
                    }""")
                    assert stable.evaluate('(node, old) => node === old', stable_handle)
                page.locator('#q').fill('no-match-responsiveness')
                expect(page.locator('#side .item')).to_have_count(0)
                page.locator('#q').fill('')
                expect(page.locator('#side .item:not(.agent)')).to_have_count(2002)
                page.locator(f'#side .item[data-uid="{corpus.uid("speed-other")}"]').click()
                expect(page.locator('#msgs')).to_contain_text('END speed-other')
                # An actual decoded catalog update must retain unrelated UI rows,
                # while parent and child metadata changes still reach their controls.
                rows = page.evaluate(js('S.sessions', 'runtime.core.state.catalog.sessions'))
                owner = next(row for row in rows if row['uid'] == 'claude:fold-0')
                owner['title'] = 'Changed parent sentinel'
                response = {'sessions': rows, 'sig': 'responsiveness-parent'}
                page.route('**/api/sessions*', lambda route: route.fulfill(json=response))
                page.evaluate(js('''window.__previousRows=new Map(S.sessions.map(row=>[row.uid,row]))''',
                    '''window.__previousRows=new Map(runtime.core.state.catalog.sessions.map(row=>[row.uid,row]))'''))
                page.evaluate(js('pollSessions()', 'runtime.core.list.pollSessions()'))
                expect(page.locator('#side .item[data-uid="claude:fold-0"] .t')).to_have_text('Changed parent sentinel')
                if scoped_frontend():
                    assert page.evaluate(js('false', '''runtime.core.state.catalog.sessions.every(row=>
                        row.uid==='claude:fold-0' || row===__previousRows.get(row.uid))'''))
                    assert page.evaluate(js('false', '''runtime.core.state.catalog.sessions.find(row=>row.uid==='claude:fold-0')
                        .agent_items===__previousRows.get('claude:fold-0').agent_items'''))
                page.evaluate(js('''window.__previousRows=new Map(S.sessions.map(row=>[row.uid,row]))''',
                    '''window.__previousRows=new Map(runtime.core.state.catalog.sessions.map(row=>[row.uid,row]))'''))
                owner['agent_items'][0]['title'] = 'Changed worker sentinel'
                response['sig'] = 'responsiveness-child'
                page.evaluate(js('pollSessions()', 'runtime.core.list.pollSessions()'))
                expect(page.locator('#side .item.agent[data-owner="claude:fold-0"][data-agent="agent-0"] .t')).to_have_text('Changed worker sentinel')
                if scoped_frontend():
                    assert page.evaluate(js('false', '''runtime.core.state.catalog.sessions.find(row=>row.uid==='claude:fold-0')
                        .agent_items[1]===__previousRows.get('claude:fold-0').agent_items[1]'''))
                if scoped_frontend():
                    inactive_parent_metadata(page, corpus.uid('speed-main'))
                assert not errors, errors
                print('PASS frontend_responsiveness_browser: 2002 sessions, targeted selection, '
                      'stable live history, filtering and cached navigation; open_ms=' + json.dumps(timings), flush=True)
            finally:
                context.close()
                browser.close()


if __name__ == '__main__':
    main()
