#!/usr/bin/env python3
"""Hub migration task shortcut, responsive menus, and background refresh (no CLI)."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import tempfile
import time

from playwright.sync_api import expect, sync_playwright

from header_fold_browser import settle
from hub_fixtures import FakeNode, Hub, REPO, scoped


def seed_task(root, uid, suffix, title, activity, *, legacy=False):
    operation = f'00000000-0000-4000-8000-{suffix:012d}'
    request = {'operation_id': operation, 'uid': uid, 'target_node': 'b' * 32}
    journal = {'request': request, 'phase': 'aborted', 'execution_requested': True,
               'created_ms': activity - (4 if legacy else 1) * 60 * 60 * 1000,
               'preview': {'sessions': [{'uid': uid, 'title': title}]}}
    if not legacy:
        journal['activity_ms'] = activity
    path = root / 'transfers' / f'{operation}.json'
    path.write_text(json.dumps(journal))
    if legacy:
        os.utime(path, ns=(activity * 1_000_000,) * 2)
    return operation


def check_transfer_shortcut(page, uid, width, theme, screenshots, titles):
    button = page.locator('#transfer-tasks')
    trigger = page.locator('#header-more-btn')
    menu = page.locator('#header-menu')
    panel = page.get_by_role('dialog', name='迁移任务')

    def capture(state):
        if screenshots:
            page.screenshot(path=str(screenshots / f'transfer-{state}-{width}-{theme}.png'))

    def check_tasks():
        for title in titles:
            expect(panel).to_contain_text(title)
        expect(panel.locator('tbody tr')).to_have_count(len(titles))
        page.keyboard.press('Escape')
        expect(panel).to_have_count(0)

    expect(button).not_to_have_attribute('data-menu-only', '')
    if width == 1280:
        expect(page.locator('.header-actions > #transfer-tasks')).to_be_visible()
        expect(button).to_have_accessible_name('迁移任务')
        expect(button).not_to_contain_text('迁移任务')
        capture('recent')
        button.click()
        check_tasks()

        trigger.press('ArrowDown')
        focused = menu.locator('button:focus').element_handle()
        # Expiring the shortcut should preserve the already open menu.
        page.clock.fast_forward(18_000)
        expect(button).to_have_attribute('data-menu-only', '')
        expect(menu).to_be_visible()
        assert focused.evaluate('e => e === document.activeElement')
        focused.dispose()
        page.keyboard.press('Escape')
    else:
        page.clock.fast_forward(18_000)
        expect(button).to_have_attribute('data-menu-only', '')

    trigger.click()
    action = menu.get_by_role('menuitem', name='迁移任务')
    expect(action).to_be_visible()
    expect(action).to_contain_text('迁移任务')
    folded = menu.locator(':scope > button[id]').evaluate_all('items => items.map(item => item.id)')
    assert folded == page.evaluate('HEADER_ACTIONS.filter(id => document.getElementById(id).parentElement.id === "header-menu")')
    capture('overflow')
    action.click()
    check_tasks()

    page.locator(f'#side .item[data-uid="{uid}"]').click()
    expect(page.locator('#msgs')).to_contain_text('Menu refresh reply')
    if width == 1280:
        page.locator('#side-toggle').click()
    expect(page.locator('#a-global-transfer-tasks')).to_have_attribute('data-menu-only', '')
    page.locator('#a-more').click()
    action = page.locator('#session-actions-menu').get_by_role('menuitem', name='迁移任务')
    expect(action).to_be_visible()
    capture('detail-overflow')
    action.click()
    check_tasks()


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
                            expect(page.locator('#header-menu #transfer-tasks')).to_be_attached()
                            check_menu(page, scoped(node.nid, row['uid']), width, theme, args.screenshots)
                            assert not errors, errors
                            context.close()
                            print(f'PASS menu refresh {width}px {theme}', flush=True)

                    hub.stop()
                    uid = scoped(node.nid, row['uid'])
                    now = int(time.time() * 1000)
                    titles = ('长时间迁移刚结束 · 旧账本', '几小时前的迁移任务')
                    recent = seed_task(hub.root, uid, 1, titles[0], now - 59 * 60 * 1000, legacy=True)
                    old = seed_task(hub.root, uid, 2, titles[1], now - 3 * 60 * 60 * 1000)
                    hub.start()
                    for width, theme in ((1280, 'light'), (390, 'dark')):
                        context = browser.new_context(viewport={'width': width, 'height': 846},
                                                      color_scheme=theme, service_workers='block')
                        page = context.new_page()
                        page.clock.install(time=datetime.fromtimestamp((now + 44_000) / 1000, timezone.utc))
                        page.goto(f'http://127.0.0.1:{hub.port}/', wait_until='networkidle')
                        response = page.request.get(f'http://127.0.0.1:{hub.port}/api/session/transfers').json()
                        assert [task['request']['operation_id'] for task in response['recent']] == [recent, old]
                        assert response['recent'][0]['activity_ms'] == now - 59 * 60 * 1000
                        assert response['recent'][1]['activity_ms'] == now - 3 * 60 * 60 * 1000
                        check_transfer_shortcut(page, uid, width, theme, args.screenshots, titles)
                        context.close()
                        print(f'PASS migration task shortcut and expiry {width}px {theme}', flush=True)
                finally:
                    browser.close()
    finally:
        if hub:
            hub.stop()
        node.stop()
    print('PASS session menu refresh: HTTP refresh, focus, action, Escape, outside click and resize')


if __name__ == '__main__':
    main()
