#!/usr/bin/env python3
"""Sidebar title weight stays uniform across sources, views and live updates."""
import argparse
from pathlib import Path
import re
import tempfile

from playwright.sync_api import expect, sync_playwright
from frontend_framework_browser import launch_chromium
from header_fold_browser import corpus
from history_fixtures import BINARY, isolated_server


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('target/sidebar-title'))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    sources = ['kiro', 'codex', 'claude', 'grok', 'opencode', 'agy', 'codebuddy', 'droid']
    stamp = '2026-11-10T12:00:00Z'
    rows = [dict(uid=f'{source}:title-{i}', sid=f'title-{i}', source=source,
                 title=f'{source} 会话标题：检查列表样式与运行状态 ' + ('较长的标题用于检查换行与省略显示 ' * 3 if i == 0 else ''),
                 cwd='/synthetic/titles', created=stamp, updated=stamp, size=4096,
                 turn='working' if i % 2 else 'idle') for i, source in enumerate(sources)]
    rows[2]['agent_items'] = [dict(id='worker', title='子代理标题：检查运行状态切换',
                                 type='general', active=True, created=stamp, updated=stamp)]

    with tempfile.TemporaryDirectory(prefix='sessiondock-titles-') as tmp, sync_playwright() as pw:
        with isolated_server(corpus(Path(tmp)), args.binary) as (base, _):
            browser = launch_chromium(pw)
            try:
                for theme in ['light', 'dark']:
                    for width in [1280, 390]:
                        live = {'uids': [row['uid'] for row in rows[:4]],
                                'sdk_uids': [rows[0]['uid']], 'tmux_uids': [rows[1]['uid']]}
                        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
                        context.add_init_script("window.EventSource=undefined;"
                            "localStorage.setItem('sessiondock.view', JSON.stringify('date'));"
                            "localStorage.setItem('sessiondock.childMode', JSON.stringify('flat'));")
                        context.route('**/api/sessions*', lambda route: route.fulfill(json={'sessions': rows, 'sig': 'titles'}))
                        context.route('**/api/live*', lambda route: route.fulfill(json=live))
                        page = context.new_page()
                        errors = []
                        page.on('pageerror', lambda error: errors.append(str(error)))
                        page.goto(base, wait_until='networkidle')
                        page.locator('#nest-flat').click()
                        expect(page.locator('#side .item')).to_have_count(9)
                        page.screenshot(path=str(args.screenshots / f'{theme}-{width}.png'), full_page=True)

                        def check():
                            titles = page.locator('#side .item .t')
                            assert titles.count() > 0
                            assert titles.evaluate_all('els => els.every(e => getComputedStyle(e).fontWeight === "400")')
                            for row in rows:
                                card = page.locator(f'#side .item[data-uid="{row["uid"]}"]')
                                if not card.count():
                                    continue
                                badge = card.locator(':scope > .ico > .item-status')
                                if row['uid'] in live['uids']:
                                    expect(badge).to_be_visible()
                                else:
                                    expect(badge).to_be_hidden()

                        check()
                        expect(page.locator('#side .item.agent')).to_have_class(re.compile(r'\blive\b'))
                        expect(page.locator('#side .item-status.sdk')).to_be_visible()
                        expect(page.locator('#side .item-status.tmux')).to_be_visible()
                        expect(page.locator('#side .item-status.turn-working').first).to_be_visible()
                        # Use actual source and scope controls, then change list view.
                        page.locator('#chips button[data-source=codex]').click()
                        expect(page.locator('#side .item[data-uid^="codex:"]')).to_have_count(0)
                        check()
                        page.locator('#chips button[data-source=codex]').click()
                        page.locator('#livecount').click()
                        expect(page.locator('#side .item[data-uid]')).to_have_count(4)
                        check()
                        page.locator('#allcount').click()
                        page.locator('#view [data-v="directory"]').click()
                        check()
                        # Same rows receive a new runtime snapshot without a reload.
                        live.update(uids=[row['uid'] for row in rows[4:]], sdk_uids=[], tmux_uids=[])
                        page.evaluate('refreshLive(true)')
                        check()
                        expect(page.locator('#side .item.agent')).not_to_have_class(re.compile(r'\blive\b'))
                        page.locator('#view [data-v="date"]').click()
                        page.screenshot(path=str(args.screenshots / f'{theme}-{width}-updated.png'), full_page=True)
                        page.reload(wait_until='networkidle')
                        check()
                        assert not errors, errors
                        print('PASS title weight, source/scope/view controls, live markers and reload', theme, width, flush=True)
                        context.close()
            finally:
                browser.close()


if __name__ == '__main__':
    main()
