#!/usr/bin/env python3
"""Scaled sidebar windows keep visible rows, anchors and deep-link targets rendered."""

import argparse
from datetime import datetime, timedelta, timezone
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from browser_runtime import choose_toolbar_option
from frontend_framework_browser import launch_chromium
from header_fold_browser import SID, corpus
from history_fixtures import BINARY, isolated_server


FRAME = '() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))'
PROBE = '''() => {
  const side = document.querySelector('#side'), box = side.getBoundingClientRect();
  return {top:side.scrollTop, zoom:side.currentCSSZoom, count:S.sessions.length,
    dom:side.querySelectorAll('.item').length,
    points:[.15,.3,.5,.7,.9,.98].map(f => {
      const node = document.elementFromPoint(box.left + box.width / 2, box.top + box.height * f);
      return {filled:!!node?.closest('.item,.ghead'), kind:node?.className,
        group:node?.closest('.group')?.dataset.key};
    })};
}'''


def rows():
    result = []
    sources = ('codex', 'kiro', 'agy', 'grok', 'claude', 'opencode')
    now = datetime(2026, 10, 29, tzinfo=timezone.utc)
    for index in range(1286):
        source = sources[index % len(sources)]
        stamp = (now - timedelta(days=index // 99, minutes=index % 99)).isoformat()
        row = dict(uid=f'{source}:zoom-{index}', sid=f'zoom-{index}', source=source,
                   title=f'会话 {index:04d}：核对滚动中的条目与子会话显示',
                   cwd=f'/synthetic/scroll/project-{index // 99:02d}',
                   node_id='a' * 32, node_name='Synthetic node',
                   created=stamp, updated=stamp, size=123456)
        if index % 33:
            parent = index - index % 33
            row['nest_parent'] = dict(source=sources[parent % len(sources)], sid=f'zoom-{parent}')
        else:
            row['agent_items'] = [dict(id=f'worker-{index}', title='核对滚动的子代理',
                                      type='subagent', created=stamp, updated=stamp)]
        result.append(row)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    parser.add_argument('--case', help='Run only case labels containing this text')
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    cases = [(675,846,125,'tree','light'), (675,846,75,'tree','dark'),
             (1280,900,150,'tree','dark'), (1280,900,50,'tree','light'),
             (675,846,125,'date','dark'), (1280,900,75,'date','light'),
             (675,846,75,'directory','light'), (1280,900,125,'directory','dark'),
             (675,846,125,'group','light'), (1280,900,75,'group','dark')]
    with tempfile.TemporaryDirectory(prefix='sessiondock-sidebar-zoom-') as temporary, sync_playwright() as pw:
        (Path(temporary) / 'state').mkdir()
        with isolated_server(corpus(Path(temporary)), args.binary,
                             state_dir=Path(temporary) / 'state') as (base, _):
            browser = launch_chromium(pw)
            try:
                for width, height, scale, view, theme in cases:
                    case = f'{view}-{width}-{scale}-{theme}'
                    if args.case and args.case not in case:
                        continue
                    context = browser.new_context(viewport={'width':width,'height':height}, service_workers='block')
                    context.add_init_script(f'''window.EventSource=undefined;
                      localStorage.setItem('sessiondock.interfaceScale','{scale}');
                      localStorage.setItem('sessiondock.theme','"{theme}"');
                    ''')
                    native = context.request.get(base + '/api/sessions').json()['sessions']
                    sessions = rows() + native
                    if view == 'group':
                        for index, session in enumerate(sessions):
                            session['group'] = f'滚动检查 {index // 99:02d}'
                        catalog = sorted({session['group'] for session in sessions})
                        context.route('**/api/groups', lambda route: route.fulfill(json={'groups':catalog}))
                    context.route('**/api/sessions*', lambda route: route.fulfill(json={'sessions':sessions,'sig':'zoom'}))
                    page = context.new_page()
                    page.goto(base, wait_until='networkidle')
                    choose_toolbar_option(page, 'view', view)
                    choose_toolbar_option(page, 'nest', 'nested')
                    expect(page.locator('#side .item').first).to_be_visible()
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / f'{case}-top.png'))
                    groups = set()
                    for step in range(55):
                        page.locator('#side').hover()
                        page.mouse.wheel(0, 450 if step < 40 else -450)
                        page.evaluate(FRAME)
                        state = page.evaluate(PROBE)
                        if args.screenshots and (step == 8 or not all(p['filled'] for p in state['points'])):
                            page.screenshot(path=str(args.screenshots / f'{case}-scroll.png'))
                        assert all(p['filled'] for p in state['points']), (case, step, state)
                        assert state['count'] == len(sessions), (case, state)
                        assert state['dom'] < 250, (case, state)
                        groups.update(p['group'] for p in state['points'])
                    assert len(groups) > 1, (case, groups)
                    # Native deep-link resolution must reveal a row outside the window.
                    # The desktop keeps the sidebar visible while opening the conversation.
                    if width > 720:
                        page.goto(f'{base}/?sid=claude:{SID}', wait_until='networkidle')
                        uid = next(row['uid'] for row in native if row['sid'] == SID)
                        page.wait_for_function('''uid => {
                          const side=document.querySelector('#side'), row=side.querySelector(`.item[data-uid="${uid}"]`);
                          if (!row?.classList.contains('sel')) return false;
                          const box=side.getBoundingClientRect(), rect=row.getBoundingClientRect();
                          return rect.top >= box.top - 1 && rect.bottom <= box.bottom + 1;
                        }''', arg=uid)
                        expect(page.locator('.dhead h2')).to_contain_text('Fold sweep question')
                        if args.screenshots:
                            page.screenshot(path=str(args.screenshots / f'{case}-deep-link.png'))
                    print('PASS scaled sidebar scroll and reveal', case, flush=True)
                    context.close()
            finally:
                browser.close()


if __name__ == '__main__':
    main()
