#!/usr/bin/env python3
"""Pin toolbar options and use their ellipsis submenus in Chromium."""
import argparse
from copy import deepcopy
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from browser_runtime import choose_toolbar_option, open_toolbar_options
from frontend_framework_browser import launch_chromium
from header_fold_browser import corpus
from history_fixtures import BINARY, isolated_server


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
                    expect(page.locator('#chips button:visible')).to_have_count(3)
                    expect(page.locator('#chips button[data-source=agy]')).to_be_hidden()
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
                    assert page.evaluate('S.view') == 'tree'
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
                    pin.check()
                    expect(page.locator('#chips button[data-source=agy]')).to_be_visible()
                    assert page.evaluate("S.off.has('agy')")
                    pin.uncheck()
                    if width < 720:
                        source.dispatch_event('pointerdown', {'pointerType': 'touch', 'pointerId': 1, 'button': 0, 'clientX': 30, 'clientY': 300})
                        expect(page.locator('#side .item')).to_have_count(1)
                        source.dispatch_event('pointerup', {'pointerType': 'touch', 'pointerId': 1})
                        source.dispatch_event('click')
                    else:
                        source.click(button='right')
                    expect(page.locator('#side .item')).to_have_count(1)
                    assert page.evaluate("!S.off.has('agy') && S.off.has('claude')")
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
                    assert page.evaluate('S.childMode === "shown" && !S.nest')
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
                    trigger.focus()
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
                    # Playwright blocks click() on aria-disabled controls; a DOM
                    # click exercises the page's unavailable-control guard.
                    source.dispatch_event('click')
                    assert page.evaluate("S.off.has('agy')")
                    menu.get_by_role('checkbox', name='在工具栏显示Agy', exact=True).check()
                    expect(page.locator('#chips button[data-source=agy]')).to_be_visible()
                    expect(page.locator('#chips button[data-source=agy]')).to_have_attribute('aria-disabled', 'true')
                    assert not errors, errors
                    print(f'PASS toolbar options {width}px: defaults, independent pin/filter, submenus, persistence, keyboard, bounds', flush=True)
                    context.close()
            finally:
                browser.close()


if __name__ == '__main__':
    main()
