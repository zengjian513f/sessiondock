#!/usr/bin/env python3
"""Timeline directory colors: distinct on screen, stable while scrolling.

Only the final directory is tinted, plus the ancestors needed to tell apart
directories on screen that share that final name. Directories that stay on
screen keep their color across scrolling; reloads reuse remembered colors;
screens with more directories than primary colors use the softer second tier.
"""

import argparse
from datetime import datetime, timedelta, timezone
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from frontend_framework_browser import launch_chromium
from header_fold_browser import SID, corpus
from history_fixtures import BINARY, isolated_server

PRIMARY = ['blue', 'amber', 'teal', 'rose', 'violet', 'olive', 'rust', 'cyan',
           'fuchsia', 'jade', 'orange', 'sky', 'indigo', 'raspberry', 'brown', 'gold']
# Same final name, different parents: `src` needs one more level, `api` two.
TWINS = {
    '~/Projects/alpha/src': 'alpha/src',
    '~/Projects/beta/src': 'beta/src',
    '/srv/east/service/api': 'east/service/api',
    '/srv/west/service/api': 'west/service/api',
}

SCREEN = '''() => {
    const side = document.querySelector('#side'), box = side.getBoundingClientRect();
    return [...side.querySelectorAll('.cwd-path')].filter(element => {
        const rect = element.getBoundingClientRect();
        return rect.height && rect.bottom > box.top && rect.top < box.bottom;
    }).map(element => ({path: element.dataset.path, color: element.dataset.cwdColor || '',
        tint: element.querySelector('.cwd-leaf')?.textContent || '',
        tintColor: getComputedStyle(element.querySelector('.cwd-leaf')).color,
        restColor: getComputedStyle(element).color}));
}'''


SHOWN = '''() => [...document.querySelectorAll('#side .cwd-path')].filter(element => element.offsetHeight)
    .map(element => ({shown: element.innerText.trim(), tint: element.querySelector('.cwd-leaf').textContent}))'''


def sessions(count, short=False):
    now = datetime(2026, 10, 14, 5, tzinfo=timezone.utc)
    directories = [path.replace('~', '/home/example', 1) for path in TWINS] + [f'/home/example/Projects/workspace-{n:03d}' for n in range(140)]
    rows = []
    for index in range(count):
        date = (now - timedelta(minutes=index * 7)).isoformat()
        cwd = directories[index % len(directories)] if index % 5 != 4 else directories[4 + index % 3]
        if index == 6:
            cwd = ''
        rows.append(dict(uid=f'codex:color-{index}', sid=f'color-{index}', source='codex',
            title=('短' if short else '目录配色 synthetic session with a realistic title ') + str(index),
            cwd=cwd, node_id='a' * 32, node_name='Synthetic node', created=date, updated=date, size=4096))
    return rows


def open_page(browser, base, rows, theme, viewport):
    context = browser.new_context(viewport=viewport)
    context.add_init_script("window.EventSource=undefined;"
        "localStorage.setItem('sessiondock.view', JSON.stringify('date'));"
        f"localStorage.setItem('sessiondock.theme', JSON.stringify('{theme}'));")
    context.route('**/api/sessions*', lambda route: route.fulfill(json={'sessions': rows, 'sig': 'colors'}))
    page = context.new_page()
    page.goto(base, wait_until='networkidle')
    if page.locator('#nest-flat').is_visible():
        page.locator('#nest-flat').click()
    expect(page.locator('#side .cwd-path').first).to_be_visible()
    settle(page)
    return context, page


def open_settings(page):
    if not page.locator('#settings').is_visible():
        page.locator('#header-more-btn').click()
    page.locator('#settings').click()
    page.locator('.settings-tab[data-tab="appearance"]').click()


def settle(page):
    page.evaluate('() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))')


def screen(page):
    state = page.evaluate(SCREEN)
    assert state, 'no directory on screen'
    colors = {}
    for row in state:
        if row['path'] == '(未知)':
            assert not row['color'], row
            continue
        assert row['color'], ('visible directory left gray', row)
        assert colors.setdefault(row['path'], row['color']) == row['color'], ('one path, two colors', row)
        assert row['tintColor'] != row['restColor'], ('tint not applied', row)
    distinct = list(colors.values())
    return state, colors, distinct


def wheel(page, delta):
    page.locator('#side').hover()
    page.mouse.wheel(0, delta)
    settle(page)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    shots = args.screenshots or Path(tempfile.mkdtemp(prefix='sessiondock-dir-colors-shots-'))
    shots.mkdir(parents=True, exist_ok=True)
    rows = sessions(900)
    with tempfile.TemporaryDirectory(prefix='sessiondock-dir-colors-') as temporary, sync_playwright() as pw:
        corpus_data = corpus(Path(temporary))
        with isolated_server(corpus_data, args.binary) as (base, _):
            browser = launch_chromium(pw)
            try:
                for theme, viewport in [('light', {'width': 1280, 'height': 900}),
                                        ('dark', {'width': 445, 'height': 850})]:
                    context, page = open_page(browser, base, rows, theme, viewport)
                    page.screenshot(path=str(shots / f'{theme}-{viewport["width"]}-top.png'))
                    state, colors, distinct = screen(page)
                    assert len(set(distinct)) == len(distinct), ('repeated color on screen', colors)
                    assert set(distinct) <= set(PRIMARY), ('second tier used with primaries free', colors)
                    seen_twins = {row['path']: row['tint'] for row in state if row['path'] in TWINS}
                    assert len(seen_twins) == len(TWINS), ('twins not all on first screen', seen_twins)
                    for path, tint in seen_twins.items():
                        # A narrow label may abbreviate the middle; the tint still
                        # starts at the distinguishing ancestor and ends at the leaf.
                        start, *_, leaf = TWINS[path].split('/')
                        assert tint == TWINS[path] or (tint.startswith(start + '/') and tint.endswith('/' + leaf)
                                                       and '…' in tint), (path, tint)
                    singles = [row for row in state if row['path'].startswith('~/Projects/workspace-')]
                    assert singles and all(row['tint'] == row['path'].rsplit('/', 1)[1] for row in singles), singles
                    first = colors

                    # Small steps: everything that stays on screen keeps its color.
                    previous = colors
                    for step in range(25):
                        wheel(page, 180)
                        _, colors, distinct = screen(page)
                        assert len(set(distinct)) == len(distinct), (step, colors)
                        for path in previous.keys() & colors.keys():
                            assert previous[path] == colors[path], ('color changed on screen', step, path)
                        previous = colors
                    page.screenshot(path=str(shots / f'{theme}-{viewport["width"]}-scrolled.png'))

                    # Return to the top: remembered colors come back.
                    page.evaluate("document.querySelector('#side').scrollTop = 0")
                    settle(page)
                    _, back, _ = screen(page)
                    assert back == first, ('colors differ after returning', first, back)

                    page.reload(wait_until='networkidle')
                    expect(page.locator('#side .cwd-path').first).to_be_visible()
                    settle(page)
                    _, reloaded, _ = screen(page)
                    assert reloaded == first, ('colors differ after reload', first, reloaded)
                    print('PASS directory colors', theme, viewport['width'], len(first), 'directories', flush=True)

                    # The appearance option changes every row and survives reload.
                    open_settings(page)
                    expect(page.locator('#setting-list-path')).to_have_value('full')
                    page.locator('#setting-list-path').select_option('tint')
                    page.screenshot(path=str(shots / f'{theme}-{viewport["width"]}-settings.png'))
                    page.locator('#settings-dialog .modal-close').click()
                    settle(page)
                    assert page.evaluate("localStorage.getItem('sessiondock.timelineDirectoryTintOnly')") == 'true'
                    for row in page.evaluate(SHOWN):
                        assert row['shown'] == row['tint'] and '…' not in row['shown'], row
                    page.screenshot(path=str(shots / f'{theme}-{viewport["width"]}-tint-only.png'))
                    page.reload(wait_until='networkidle')
                    expect(page.locator('#side .cwd-path').first).to_be_visible()
                    settle(page)
                    assert all(row['shown'] == row['tint'] for row in page.evaluate(SHOWN))
                    open_settings(page)
                    expect(page.locator('#setting-list-path')).to_have_value('tint')
                    page.locator('#setting-list-path').select_option('full')
                    page.locator('#settings-dialog .modal-close').click()
                    settle(page)
                    assert page.evaluate("localStorage.getItem('sessiondock.timelineDirectoryTintOnly')") == 'false'
                    assert page.locator('#side .item .cwd-path[data-path="~/Projects/alpha/src"]').first.inner_text().startswith('~/')
                    print('PASS list path appearance option and persistence', theme, viewport['width'], flush=True)
                    context.close()

                # The session header shows the whole path with the sidebar's color,
                # unaffected by the sidebar's tinted-only display.
                data = corpus_data
                context = browser.new_context(viewport={'width': 1280, 'height': 700})
                context.add_init_script("window.EventSource=undefined;"
                    "localStorage.setItem('sessiondock.view', JSON.stringify('date'));"
                    "if (localStorage.getItem('sessiondock.timelineDirectoryTintOnly') === null) "
                    "localStorage.setItem('sessiondock.timelineDirectoryTintOnly', 'true');")
                page = context.new_page()
                page.goto(base, wait_until='networkidle')
                page.locator(f'#side .item[data-uid="{data.uid(SID)}"] .cwd-path').click()
                expect(page.locator('#msgs')).to_contain_text('reply Sweep')
                assert page.evaluate("localStorage.getItem('sessiondock.timelineDirectoryTintOnly')") == 'true'
                settle(page)
                side = page.locator(f'#side .item[data-uid="{data.uid(SID)}"] .cwd-path')
                header = page.locator('.dhead .meta-cwd .cwd-path')
                path = side.get_attribute('data-path')
                assert header.get_attribute('data-path') == path, (path, header.get_attribute('data-path'))
                assert side.get_attribute('data-cwd-color') and \
                    header.get_attribute('data-cwd-color') == side.get_attribute('data-cwd-color')
                assert side.inner_text() == path.rsplit('/', 1)[1], side.inner_text()
                assert header.inner_text() == path, header.inner_text()
                assert header.locator('.cwd-leaf').inner_text() == path.rsplit('/', 1)[1]
                assert header.evaluate("e => getComputedStyle(e.querySelector('.cwd-leaf')).color") != \
                    header.evaluate("e => getComputedStyle(e).color")
                page.locator('.dhead').screenshot(path=str(shots / 'light-header.png'))
                print('PASS header directory uses the sidebar color and whole path', flush=True)
                # Full-path mode also stays unchanged when clicking a directory,
                # in both list views (including freshly created row elements).
                open_settings(page)
                page.locator('#setting-list-path').select_option('full')
                page.locator('#settings-dialog .modal-close').click()
                for view in ['date', 'directory']:
                    page.reload(wait_until='networkidle')
                    page.locator(f'#view [data-v="{view}"]').click()
                    page.locator(f'#side .item[data-uid="{data.uid(SID)}"] .cwd-path').click()
                    expect(page.locator('#msgs')).to_contain_text('reply Sweep')
                    assert page.evaluate("localStorage.getItem('sessiondock.timelineDirectoryTintOnly')") == 'false'
                print('PASS directory clicks open sessions without changing path display', flush=True)
                context.close()

                # More directories on one screen than primary colors.
                context, page = open_page(browser, base, sessions(400, short=True), 'light',
                                          {'width': 1280, 'height': 2600})
                _, colors, distinct = screen(page)
                assert len(colors) > len(PRIMARY), ('screen too small for the overflow case', len(colors))
                assert set(PRIMARY) <= set(distinct), ('second tier before primaries were used', colors)
                assert len(set(distinct)) == len(distinct) or len(distinct) > 32, colors
                page.screenshot(path=str(shots / 'light-overflow.png'))
                print('PASS second tier only after primaries', len(colors), 'directories', flush=True)
                context.close()
            finally:
                browser.close()
    print('screenshots', shots, flush=True)


if __name__ == '__main__':
    main()
