#!/usr/bin/env python3
"""Hub background task refresh must not dismiss an open session menu (no CLI)."""
import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright

from header_fold_browser import settle
from hub_fixtures import FakeNode, Hub, REPO, scoped


def check_menu(page, uid, width, theme, screenshots):
    page.set_viewport_size({'width': width, 'height': 846})
    page.emulate_media(color_scheme=theme)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator('#msgs')).to_contain_text('Menu refresh reply')
    settle(page)
    trigger = page.locator('#a-more')
    menu = page.locator('#session-actions-menu')
    trigger.click()
    expect(menu).to_be_visible()
    assert menu.locator('#a-session-info').count() == 0
    info = menu.locator('#a-session-action')
    info.focus()
    focused = info.element_handle()
    # Exercise the same real Hub HTTP response and MutationObserver as the
    # periodic/list-event refresh, without editing DOM or invoking layout.
    for _ in range(3):
        with page.expect_response(lambda r: r.url.endswith('/api/session/transfers')) as response:
            page.evaluate('refreshTransferTasks()')
        assert response.value.ok
        settle(page)
        expect(menu).to_be_visible()
        expect(trigger).to_have_attribute('aria-expanded', 'true')
        assert focused.evaluate('e => e === document.activeElement')
    if screenshots:
        page.screenshot(path=str(screenshots / f'menu-{width}-{theme}.png'))
    page.keyboard.press('Escape')
    expect(menu).to_be_hidden()
    expect(trigger).to_be_focused()
    trigger.click()
    page.locator('#msgs').click(position={'x': 15, 'y': 15})
    expect(menu).to_be_hidden()
    trigger.click()
    page.set_viewport_size({'width': width + 1, 'height': 846})
    expect(menu).to_be_hidden()
    expect(trigger).to_have_attribute('aria-expanded', 'false')
    focused.dispose()
    settle(page)
    trigger.click()
    page.evaluate('refreshTransferTasks()')
    settle(page)
    page.keyboard.press('Escape')
    page.locator('#a-info').click()
    frame = page.frame_locator('#session-content-view iframe')
    expect(frame.get_by_role('heading', name='会话信息', exact=True)).to_be_visible()
    expect(frame.locator('main')).to_contain_text('00000000-1234-4567-89ab-000000000001')



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=REPO / 'target/release/sessiondock')
    parser.add_argument('--hub-binary', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    node = FakeNode('a' * 32, 'MenuNode')
    hub = None
    try:
        state = node.state()
        row = {**state['row'], 'title': '实现并测试 Rust backend · 会话菜单刷新回归',
               'sid': '00000000-1234-4567-89ab-000000000001',
               'cwd': '/synthetic/projects/session-menu-refresh'}
        node.set(row=row, messages=[{'role': 'assistant', 'text': 'Menu refresh reply'}])
        with tempfile.TemporaryDirectory(prefix='sessiondock-menu-refresh-') as directory:
            hub_binary = args.hub_binary or args.binary.resolve().with_name('sessiondock-hub')
            hub = Hub(hub_binary, Path(directory), [node])
            hub.start()
            with sync_playwright() as pw:
                launch = {'headless': True}
                if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                    launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                browser = pw.chromium.launch(**launch)
                try:
                    for width in (675, 390, 1280):
                        for theme in ('light', 'dark'):
                            context = browser.new_context(viewport={'width': width, 'height': 846},
                                                          color_scheme=theme, service_workers='block')
                            page = context.new_page()
                            errors = []
                            page.on('pageerror', lambda error: errors.append(str(error)))
                            page.goto(f'http://127.0.0.1:{hub.port}/', wait_until='networkidle')
                            check_menu(page, scoped(node.nid, row['uid']), width, theme, args.screenshots)
                            assert not errors, errors
                            context.close()
                            print(f'PASS menu refresh {width}px {theme}', flush=True)
                finally:
                    browser.close()
    finally:
        if hub:
            hub.stop()
        node.stop()
    print('PASS session menu refresh: HTTP refresh, focus, action, Escape, outside click and resize')


if __name__ == '__main__':
    main()
