#!/usr/bin/env python3
"""Flat timeline scrolling must fill the viewport, including resource columns."""

import argparse
from datetime import datetime, timedelta, timezone
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from frontend_framework_browser import launch_chromium
from header_fold_browser import corpus
from history_fixtures import BINARY, isolated_server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    now = datetime(2026, 10, 8, 5, tzinfo=timezone.utc)
    rows = []
    for index in range(6400):
        date = (now - timedelta(days=index // 278, minutes=index % 278)).isoformat()
        rows.append(dict(uid=f'codex:flat-{index}', sid=f'flat-{index}', source='codex',
            title=('Synthetic independent session with a long two-line title ' * 3 if index % 3 else '(untitled)') + str(index),
            cwd='/synthetic' + '/very-long-shared-parent-directory' * 12
                + f'/variant-{index % 9}/nested/empty_workdir',
            node_id='a'*32, node_name='Synthetic node', created=date, updated=date, size=123456))
    with tempfile.TemporaryDirectory(prefix='sessiondock-flat-scroll-') as temporary, sync_playwright() as pw:
        with isolated_server(corpus(Path(temporary)), args.binary) as (base, _):
            browser = launch_chromium(pw)
            try:
                for width, height in [(445, 850), (720, 1000), (1280, 900)]:
                    context = browser.new_context(viewport={'width': width, 'height': height})
                    context.add_init_script("window.EventSource=undefined; localStorage.setItem('sessiondock.view', JSON.stringify('date'));")
                    context.route('**/api/sessions*', lambda route: route.fulfill(json={'sessions':rows, 'sig':'flat-scroll'}))
                    page = context.new_page()
                    page.goto(base, wait_until='networkidle')
                    page.locator('#nest-flat').click()
                    page.get_by_role('button', name='列表资源', exact=True).click()
                    expect(page.locator('#side .item').first).to_be_visible()
                    seen_groups = set()
                    for step in range(90):
                        if step in (20, 40):
                            page.get_by_role('button', name='列表资源', exact=True).click()
                        page.locator('#side').hover()
                        page.mouse.wheel(0, 600 if step < 60 else -600)
                        page.evaluate('() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))')
                        state = page.evaluate('''() => {
                            const side=document.querySelector('#side'), box=side.getBoundingClientRect();
                            const points=[.2,.4,.6,.8,.95].map(f=> {
                                const y=box.top+side.clientHeight*f;
                                const node=document.elementFromPoint(box.left+side.clientWidth/2,y);
                                return {y,tag:node?.className,filled:!!node?.closest('.item,.ghead'),
                                    group:node?.closest('.group')?.dataset.key};
                            });
                            return {points,top:side.scrollTop,height:side.scrollHeight,client:side.clientHeight,
                                windows:[...side.querySelectorAll('.group')].map(g=>g._window),
                                dom:side.querySelectorAll('.item').length};
                        }''')
                        assert all(p['filled'] for p in state['points']), (width, step, state)
                        assert state['top'] + state['client'] < state['height'] - 1000, state
                        seen_groups.update(p['group'] for p in state['points'])
                    assert len(seen_groups) > 1, seen_groups
                    print('PASS flat timeline filled while scrolling', width, flush=True)
                    context.close()
            finally:
                browser.close()


if __name__ == '__main__':
    main()
