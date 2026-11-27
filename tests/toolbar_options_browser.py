#!/usr/bin/env python3
"""Pin toolbar options and use their ellipsis submenus in Chromium."""
import argparse
import json
from copy import deepcopy
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from browser_runtime import choose_toolbar_option, open_directory_filter, open_toolbar_options
from frontend_framework_browser import launch_chromium
from header_fold_browser import corpus
from history_fixtures import BINARY, isolated_server


def long_press(context, page, button):
    touch = context.new_cdp_session(page)
    try:
        bounds = button.bounding_box()
        point = {'x': bounds['x'] + bounds['width']/2, 'y': bounds['y'] + bounds['height']/2}
        touch.send('Input.dispatchTouchEvent', {'type': 'touchStart', 'touchPoints': [point]})
        page.wait_for_timeout(700)
        touch.send('Input.dispatchTouchEvent', {'type': 'touchEnd', 'touchPoints': []})
    finally:
        touch.detach()


def check_directory_filter(context, page, payload, width, screenshots_dir):
    original = payload.copy()
    rows = deepcopy(payload['sessions'])
    for source in ('kiro', 'codebuddy', 'droid'):
        rows.append(dict(rows[0], source=source, agent_type=source, uid=f'{source}:directory-source',
                         sid='directory-source', title=f'Directory {source}'))
    shared, older = '/synthetic/workspace/project', '/synthetic/archived/project'
    long_path = '/synthetic/workspace/branches/active/release-review/feature-with-a-long-directory-name-for-mobile'
    paths = [shared, older, shared, long_path, shared, '', older, shared, long_path, older]
    shared_count = paths.count(shared)
    for row, path, hour in zip(rows, paths, (12, 11, 8, 10, 9, 7, 6, 5, 4, 3), strict=True):
        row.update(cwd=path, updated=f'2026-09-11T{hour:02d}:00:00Z')
    payload.update(sessions=rows, sig='directory-fixture')
    assert page.evaluate('pollSessions()')
    page.route('**/api/live*', lambda route: route.fulfill(json={'uids': [rows[0]['uid'], rows[1]['uid']]}))
    assert page.evaluate('refreshLive(true).then(() => S.live.size === 2)')
    page.route('**/api/search?*', lambda route: route.fulfill(json={
        'results': [dict(row, hits=1) for row in rows],
    }))
    items = page.locator('#side .item')
    search = page.locator('#q')
    menu = open_directory_filter(page)
    labels = menu.locator('.directory-filter-row')
    checks = menu.get_by_role('checkbox')
    shared_check = menu.get_by_role('checkbox', name=shared, exact=True)
    expect(labels).to_have_text([shared, older, long_path, '(未知)'])
    expect(menu.get_by_role('checkbox', checked=True)).to_have_count(4)
    if screenshots_dir:
        page.screenshot(path=str(screenshots_dir / f'directory-filter-{width}.png'), full_page=True)

    shared_check.locator('../span').click(button='right')
    expect(menu.get_by_role('checkbox', checked=True)).to_have_count(1)
    expect(shared_check).to_be_checked()
    expect(items).to_have_count(shared_count)
    shared_check.click(button='right')
    expect(menu.get_by_role('checkbox', checked=True)).to_have_count(4)
    expect(page.locator('#directory-filter')).to_have_attribute('aria-description', '显示所有文件夹')

    long_check = menu.get_by_role('checkbox', name=long_path, exact=True)
    unknown_check = menu.get_by_role('checkbox', name='(未知)', exact=True)
    long_check.uncheck()
    unknown_check.uncheck()
    for checkbox, count in ((unknown_check, 1), (long_check, paths.count(long_path))):
        label_text = checkbox.locator('../span')
        if width < 720:
            long_press(context, page, label_text)
        else:
            label_text.click(button='right')
        expect(menu.get_by_role('checkbox', checked=True)).to_have_count(1)
        expect(checkbox).to_be_checked()
        expect(items).to_have_count(count)
    if screenshots_dir:
        page.screenshot(path=str(screenshots_dir / f'directory-shortcut-solo-{width}.png'), full_page=True)
    page.reload(wait_until='networkidle')
    expect(items).to_have_count(paths.count(long_path))
    menu = open_directory_filter(page)
    if width < 720:
        long_press(context, page, long_check)
    else:
        long_check.click(button='right')
    expect(menu.get_by_role('checkbox', checked=True)).to_have_count(2)
    expect(shared_check).to_be_checked()
    expect(menu.get_by_role('checkbox', name=older, exact=True)).to_be_checked()
    expect(items).to_have_count(shared_count + paths.count(older))
    if screenshots_dir:
        page.screenshot(path=str(screenshots_dir / f'directory-shortcut-restored-{width}.png'), full_page=True)
    menu.get_by_role('button', name='全选', exact=True).click()

    menu.get_by_role('button', name='全不选').click()
    expect(items).to_have_count(0)
    expect(page.locator('#session-total')).to_have_text('0')
    expect(menu.get_by_role('checkbox', checked=True)).to_have_count(0)
    shared_check.focus()
    shared_check.press('Space')
    expect(shared_check).to_be_checked()
    expect(items).to_have_count(shared_count)
    expect(page.locator('#session-total')).to_have_text(str(shared_count))
    if screenshots_dir:
        other_theme = 'light' if width < 720 else 'dark'
        page.evaluate('applyTheme', other_theme)
        page.screenshot(path=str(screenshots_dir / f'directory-selected-{other_theme}-{width}.png'), full_page=True)
        page.evaluate('applyTheme', 'dark' if width < 720 else 'light')

    # New and reordered folders are picked up only on the next opening.
    added = dict(rows[0], uid='claude:directory-refresh', sid='directory-refresh',
                 title='New folder session', cwd='/synthetic/new', updated='2026-09-11T13:00:00Z')
    rows[1]['updated'] = '2026-09-11T14:00:00Z'
    payload.update(sessions=[*rows, added], sig='directory-refreshed')
    assert page.evaluate('pollSessions()')
    expect(labels).to_have_text([shared, older, long_path, '(未知)'])
    expect(shared_check).to_be_focused()
    expect(shared_check).to_be_checked()
    expect(items).to_have_count(shared_count)
    page.keyboard.press('Escape')
    expect(page.locator('#directory-filter')).to_be_focused()
    menu = open_directory_filter(page)
    expect(labels).to_have_text([older, added['cwd'], shared, long_path, '(未知)'])
    expect(menu.get_by_role('checkbox', name=added['cwd'], exact=True)).not_to_be_checked()
    search.click()
    expect(menu).to_be_hidden()
    page.locator('#livecount').click()
    expect(items).to_have_count(1)
    page.locator('#allcount').click()
    search.fill('project')
    expect(page.locator('#side-search-count')).to_have_text(f'{shared_count} 条')
    search.press('Enter')
    expect(page.locator('#side-search-label')).to_have_text('搜索结果')
    expect(page.locator('#side-search-count')).to_have_text(f'{shared_count} 条')
    menu = open_directory_filter(page)
    shared_check.click(button='right')
    expect(page.locator('#side-search-count')).to_have_text(f'{len(rows)} 条')
    menu.get_by_role('button', name='全不选').click()
    expect(page.locator('#side-search-count')).to_have_text('0 条')
    expect(checks).to_have_count(5)
    page.reload(wait_until='networkidle')
    expect(page.locator('#side .empty')).to_have_text('没有会话')
    menu = open_directory_filter(page)
    expect(menu.get_by_role('checkbox', checked=True)).to_have_count(0)
    shared_check.check()
    page.reload(wait_until='networkidle')
    expect(items).to_have_count(shared_count)
    menu = open_directory_filter(page)
    expect(menu.get_by_role('checkbox', checked=True)).to_have_count(1)
    expect(shared_check).to_be_checked()
    menu.get_by_role('button', name='全选', exact=True).click()
    expect(items).to_have_count(len(rows) + 1)
    expect(menu.get_by_role('checkbox', checked=True)).to_have_count(5)
    search.click()

    # Scroll long catalogs inside the menu while keeping the bulk actions in view.
    extra = [dict(added, uid=f'claude:directory-{i}', sid=f'directory-{i}', cwd=f'/synthetic/extra-{i:02d}')
             for i in range(40)]
    payload.update(sessions=[*rows, added, *extra], sig='directory-long-list')
    assert page.evaluate('pollSessions()')
    menu = open_directory_filter(page)
    expect(checks).to_have_count(45)
    checks.last.scroll_into_view_if_needed()
    expect(checks.last).to_be_in_viewport()
    expect(menu.get_by_role('button', name='全不选')).to_be_in_viewport()
    assert menu.evaluate('el => {const r = el.getBoundingClientRect(); return r.right <= innerWidth && r.bottom <= innerHeight;}')
    search.click()
    payload.update(sessions=[], sig='directory-empty')
    assert page.evaluate('pollSessions()')
    menu = open_directory_filter(page)
    expect(menu.get_by_text('暂无会话目录')).to_be_visible()
    expect(checks).to_have_count(0)
    if screenshots_dir:
        page.screenshot(path=str(screenshots_dir / f'directory-empty-{width}.png'), full_page=True)
    page.keyboard.press('Escape')
    check_temporary_directories(page, payload, rows, width, screenshots_dir)
    payload.update(original)
    page.unroute('**/api/live*')
    page.unroute('**/api/search?*')
    page.reload(wait_until='networkidle')
    expect(items).to_have_count(7)
    print(f'PASS directory filter {width}px: shortcuts, snapshot, keyboard, search, scope, bulk, persistence, scrolling', flush=True)


def check_temporary_directories(page, payload, rows, width, screenshots_dir):
    paths = ['/tmp/first/task', '/tmp/second', '/tmp', '/tmp-project', '/workspace']
    temporary = [dict(row, cwd=path) for row, path in zip(rows[:len(paths)], paths, strict=True)]
    payload.update(sessions=temporary, sig='directory-temporary')
    assert page.evaluate('pollSessions()')
    menu = open_directory_filter(page)
    if screenshots_dir:
        page.screenshot(path=str(screenshots_dir / f'directory-tmp-{width}.png'), full_page=True)
    expect(menu.locator('.directory-filter-row')).to_have_text(['/tmp/...', '/tmp-project', '/workspace'])
    grouped = menu.get_by_role('checkbox', name='/tmp/...', exact=True)
    grouped.uncheck()
    items = page.locator('#side .item')
    expect(items).to_have_count(2)
    for row in temporary[3:]:
        expect(page.locator(f'#side .item[data-uid="{row["uid"]}"]')).to_be_visible()
    page.keyboard.press('Escape')

    # Old browser preferences can contain individual paths in both selections.
    page.evaluate("""paths => {
      store.set('visibleDirectories', paths.slice(0, 1));
      store.set('visibleDirectories.lastMultiple', [paths[0], paths[1], paths[4]]);
    }""", paths)
    page.reload(wait_until='networkidle')
    expect(items).to_have_count(3)
    menu = open_directory_filter(page)
    expect(grouped).to_be_checked()
    grouped.click(button='right')
    expect(menu.get_by_role('checkbox', checked=True)).to_have_count(2)
    expect(items).to_have_count(4)
    menu.get_by_role('checkbox', name='/workspace', exact=True).uncheck()
    page.keyboard.press('Escape')
    temporary.append(dict(temporary[0], uid='claude:tmp-new', sid='tmp-new', cwd='/tmp/new',
                          title='New temp session'))
    payload.update(sig='directory-temporary-new')
    assert page.evaluate('pollSessions()')
    expect(items).to_have_count(4)
    expect(items.filter(has_text='New temp session')).to_be_visible()
    page.route('**/api/search?*', lambda route: route.fulfill(json={
        'results': [dict(row, hits=1) for row in temporary],
    }))
    page.locator('#q').fill('temp')
    page.locator('#q').press('Enter')
    expect(page.locator('#side-search-label')).to_have_text('搜索结果')
    expect(page.locator('#side-search-count')).to_have_text('4 条')
    open_directory_filter(page).get_by_role('button', name='全选', exact=True).click()
    expect(page.locator('#side-search-count')).to_have_text('6 条')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots-dir', type=Path)
    args = parser.parse_args()
    if args.screenshots_dir:
        args.screenshots_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-toolbar-options-') as temporary, sync_playwright() as pw:
        data = corpus(Path(temporary))
        (data.root / 'state').mkdir()
        with isolated_server(data, args.binary, state_dir=data.root / 'state') as (base, _):
            browser = launch_chromium(pw)
            try:
                for width in (1900, 390):
                    context = browser.new_context(viewport={'width': width, 'height': 900}, has_touch=width < 720,
                                                  service_workers='block')
                    context.add_init_script("localStorage.setItem('sessiondock.theme', JSON.stringify('" + ('dark' if width < 720 else 'light') + "'))")
                    page = context.new_page()
                    errors = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    payload = page.request.get(base + '/api/sessions').json()
                    rows = payload['sessions']
                    for source in ('codex', 'grok', 'agy', 'opencode', 'shell'):
                        row = deepcopy(rows[0])
                        row.update(uid=f'{source}:toolbar-fixture', sid='toolbar-fixture', source=source,
                                   title=f'Toolbar {source}', agent_type=source)
                        rows.append(row)
                    context.route('**/api/sessions*', lambda route: route.fulfill(json=payload)
                                  if '/api/sessions/unread' not in route.request.url else route.continue_())
                    page.goto(base, wait_until='networkidle')
                    page.wait_for_function('S.sessions.length === 7')
                    expect(page.locator('#header-more-btn')).to_be_visible()
                    expect(page.locator('#view button:visible')).to_have_count(2)
                    expect(page.locator('#view [data-v=directory]')).to_be_visible()
                    expect(page.locator('#view [data-v=date]')).to_be_visible()
                    expect(page.locator('#chips button:visible')).to_have_count(page.evaluate('Object.keys(SOURCES).length'))
                    assert page.evaluate('S.view === "date" && S.childMode === "hidden" && S.off.size === 0')
                    assert page.locator('#view button').evaluate_all('(buttons) => buttons.map(b => b.dataset.v)') == ['date', 'directory', 'group', 'tree']
                    if args.screenshots_dir:
                        page.screenshot(path=str(args.screenshots_dir / f'toolbar-defaults-{width}.png'), full_page=True)
                    check_directory_filter(context, page, payload, width, args.screenshots_dir)
                    expect(page.locator('#chips button[data-source=agy]')).to_be_visible()
                    expect(page.locator('#nest button:visible')).to_have_count(3)

                    # Pinning leaves the selected view intact; an unpinned current
                    # view stays usable through the submenu.
                    menu = open_toolbar_options(page, 'view')
                    tree = menu.get_by_role('checkbox', name='在工具栏显示项目树', exact=True)
                    expect(tree).not_to_be_checked()
                    if args.screenshots_dir:
                        page.screenshot(path=str(args.screenshots_dir / f'toolbar-view-{width}.png'))
                    tree.check()
                    expect(page.locator('#view [data-v=tree]')).to_be_visible()
                    assert page.evaluate('S.view') == 'date'
                    tree.uncheck()
                    menu.get_by_role('checkbox', name='在工具栏显示时间轴', exact=True).uncheck()
                    expect(menu).to_be_visible()
                    expect(page.locator('#view [data-v=date]')).to_be_hidden()
                    menu.locator('button[data-v=date]').click()
                    expect(page.locator('#header-menu')).to_be_hidden()
                    assert page.evaluate('S.view') == 'date'
                    expect(page.locator('#side .group')).not_to_have_count(0)
                    menu = open_toolbar_options(page, 'view')
                    expect(menu.locator('button[data-v=date]')).to_have_attribute('aria-pressed', 'true')
                    menu.get_by_role('checkbox', name='在工具栏显示按目录聚合', exact=True).uncheck()
                    expect(page.locator('#view')).to_be_hidden()
                    page.keyboard.press('Escape')
                    expect(menu).to_be_hidden()
                    expect(page.locator('#header-menu')).to_be_visible()
                    page.keyboard.press('Escape')
                    choose_toolbar_option(page, 'view', 'directory')
                    assert page.evaluate('S.view') == 'directory'

                    # Source pinning and source filtering are separate decisions.
                    menu = open_toolbar_options(page, 'sources')
                    source = menu.locator('button[data-source=agy]')
                    expect(source).to_contain_text('Agy')
                    before = page.locator('#side .item').count()
                    source.click()
                    expect(source).to_have_attribute('aria-pressed', 'false')
                    expect(page.locator('#side .item')).to_have_count(before - 1)
                    pin = menu.get_by_role('checkbox', name='在工具栏显示Agy', exact=True)
                    pin.uncheck()
                    pin.check()
                    expect(page.locator('#chips button[data-source=agy]')).to_be_visible()
                    assert page.evaluate("S.off.has('agy')")
                    page.locator('#chips button[data-source=codex]').click(button='right')
                    expect(page.locator('#side .item')).to_have_count(1)
                    menu = open_toolbar_options(page, 'sources')
                    menu.locator('button[data-source=codex]').click(button='right')
                    expect(page.locator('#side .item')).to_have_count(before - 1)
                    assert page.evaluate("[...S.off]") == ['agy']

                    # An unpinned choice shares the toolbar's most recent multi-selection.
                    menu.locator('button[data-source=grok]').click()
                    expect(page.locator('#side .item')).to_have_count(before - 2)
                    pin.uncheck()
                    if width < 720:
                        long_press(context, page, source)
                    else:
                        source.click(button='right')
                    expect(page.locator('#side .item')).to_have_count(1)
                    assert page.evaluate("!S.off.has('agy') && S.off.has('claude')")
                    if args.screenshots_dir:
                        page.screenshot(path=str(args.screenshots_dir / f'toolbar-source-solo-{width}.png'), full_page=True)
                    page.reload(wait_until='networkidle')
                    expect(page.locator('#side .item')).to_have_count(1)
                    menu = open_toolbar_options(page, 'sources')
                    source = menu.locator('button[data-source=agy]')
                    if width < 720:
                        long_press(context, page, source)
                    else:
                        source.click(button='right')
                    expect(page.locator('#side .item')).to_have_count(before - 2)
                    assert page.evaluate("[...S.off]") == ['agy', 'grok']
                    assert page.evaluate("JSON.parse(localStorage.getItem('sessiondock.off'))") == ['agy', 'grok']
                    expect(source).to_have_attribute('aria-pressed', 'false')
                    if args.screenshots_dir:
                        page.screenshot(path=str(args.screenshots_dir / f'toolbar-source-restored-{width}.png'), full_page=True)
                    source.click(button='right')
                    expect(page.locator('#side .item')).to_have_count(1)
                    source.click()
                    expect(page.locator('#side .item')).to_have_count(0)
                    for selected in ('claude', 'codex', 'grok'):
                        menu.locator(f'button[data-source={selected}]').click()
                    for checkbox in menu.get_by_role('checkbox').all():
                        checkbox.uncheck()
                    expect(page.locator('#chips')).to_be_hidden()
                    page.keyboard.press('Escape')
                    page.keyboard.press('Escape')

                    # Folding modes follow the same pinning model.
                    menu = open_toolbar_options(page, 'nest')
                    for checkbox in menu.get_by_role('checkbox').all():
                        checkbox.uncheck()
                    expect(page.locator('#nest')).to_be_hidden()
                    assert page.evaluate('S.childMode === "hidden" && !S.nest')
                    menu.locator('button[data-mode=nested]').click()
                    assert page.evaluate('S.nest')
                    page.reload(wait_until='networkidle')
                    page.wait_for_function('S.sessions.length === 7')
                    for host in ('view', 'chips', 'nest'):
                        expect(page.locator('#' + host)).to_be_hidden()
                    assert page.evaluate('S.view === "directory" && S.nest && S.off.has("agy")')

                    # Keyboard navigation and bounded, readable mobile submenus.
                    page.locator('#header-more-btn').press('ArrowDown')
                    trigger = page.locator('#header-menu [data-toolbar-menu=nest]')
                    for _ in range(20):
                        if trigger.evaluate('e => e === document.activeElement'):
                            break
                        page.keyboard.press('ArrowDown')
                    expect(trigger).to_be_focused()
                    trigger.press('ArrowRight')
                    menu = page.locator('#toolbar-nest-menu')
                    expect(menu.locator('button[data-mode=hidden]')).to_be_focused()
                    menu.locator('button[data-mode=hidden]').press('End')
                    expect(menu.locator('button[data-mode=flat]')).to_be_focused()
                    menu.locator('button[data-mode=flat]').press('ArrowLeft')
                    expect(trigger).to_be_focused()
                    expect(menu).to_be_hidden()
                    trigger.press('ArrowRight')
                    bounds = menu.bounding_box()
                    assert bounds['x'] >= 0 and bounds['x'] + bounds['width'] <= width
                    assert bounds['y'] >= 0 and bounds['y'] + bounds['height'] <= 900
                    expect(menu.locator('button[data-mode=nested]')).to_contain_text('层叠')
                    page.locator('#q').click()
                    expect(menu).to_be_hidden()
                    expect(page.locator('#header-menu')).to_be_hidden()
                    choose_toolbar_option(page, 'nest', 'flat')
                    assert not page.evaluate('S.nest')
                    # An unpinned source with no sessions keeps the same disabled
                    # reason and cannot change filtering; pinning remains possible.
                    payload['sessions'] = [row for row in rows if row['source'] != 'agy']
                    page.reload(wait_until='networkidle')
                    page.wait_for_function('S.sessions.length === 6')
                    menu = open_toolbar_options(page, 'sources')
                    source = menu.locator('button[data-source=agy]')
                    expect(source).to_have_attribute('aria-disabled', 'true')
                    expect(source).to_have_attribute('title', 'Agy 在当前选择的机器上没有会话。')
                    # A real pointer can reach aria-disabled controls; activation
                    # must leave filtering unchanged.
                    bounds = source.bounding_box()
                    page.mouse.click(bounds['x'] + bounds['width']/2, bounds['y'] + bounds['height']/2)
                    previous = page.evaluate("[...S.off]")
                    page.mouse.click(bounds['x'] + bounds['width']/2, bounds['y'] + bounds['height']/2, button='right')
                    assert page.evaluate("[...S.off]") == previous
                    assert page.evaluate("S.off.has('agy')")
                    menu.get_by_role('checkbox', name='在工具栏显示Agy', exact=True).check()
                    expect(page.locator('#chips button[data-source=agy]')).to_be_visible()
                    expect(page.locator('#chips button[data-source=agy]')).to_have_attribute('aria-disabled', 'true')
                    # Exercise each actual toolbar and duplicate submenu entry,
                    # including persisted preferences after navigation.
                    for value in ('tree', 'directory', 'date', 'group'):
                        menu = open_toolbar_options(page, 'view')
                        option = menu.locator(f'button[data-v={value}]')
                        option.locator('..').get_by_role('checkbox').check()
                        option.click()
                        assert page.evaluate('S.view') == value
                        page.locator(f'#view button[data-v={value}]').click()
                        assert page.evaluate('S.view') == value
                    for value in ('hidden', 'nested', 'flat'):
                        menu = open_toolbar_options(page, 'nest')
                        option = menu.locator(f'button[data-mode={value}]')
                        option.locator('..').get_by_role('checkbox').check()
                        option.click()
                        page.locator(f'#nest button[data-mode={value}]').click()
                        expect(page.locator(f'#nest button[data-mode={value}]')).to_have_attribute('aria-pressed', 'true')
                    page.reload(wait_until='networkidle')
                    assert page.evaluate('S.view') == 'group'
                    expect(page.locator('#nest button[data-mode=flat]')).to_have_attribute('aria-pressed', 'true')
                    assert not errors, errors
                    print(f'PASS toolbar options {width}px: pins, source solo/restore, persistence, keyboard, bounds', flush=True)
                    context.close()
                # Saved legacy modes remain explicit choices, including false.
                for nested in (True, False):
                    context = browser.new_context(service_workers='block')
                    context.add_init_script("localStorage.setItem('sessiondock.nest', " + json.dumps(json.dumps(nested)) + ")")
                    page = context.new_page()
                    page.goto(base, wait_until='networkidle')
                    expect(page.locator('#nest-' + ('toggle' if nested else 'flat'))).to_have_attribute('aria-pressed', 'true')
                    assert page.evaluate('S.childMode') == 'shown'
                    context.close()
                print('PASS saved legacy nesting preferences (true/false)', flush=True)
            finally:
                browser.close()


if __name__ == '__main__':
    main()
