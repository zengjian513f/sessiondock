#!/usr/bin/env python3
"""Inline group management, session submenus and offline-safe deletion through Chromium."""

import argparse
from contextlib import ExitStack
from io import BytesIO
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace

from PIL import Image
from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, codex_row, isolated_server
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots-dir', type=Path)
    args = parser.parse_args()
    if args.screenshots_dir: args.screenshots_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-groups-browser-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        corpora, nodes, states = [], [], []
        for name in ('a', 'b'):
            corpus = Corpus(root / name)
            for sid in ('same', 'other'):
                corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': '/synthetic', 'timestamp': '2026-09-30T00:00:00Z'}),
                    codex_row('response_item', {'type': 'message', 'role': 'user', 'content': f'{name} {sid}'})], [])
            state = corpus.root / 'state'
            state.mkdir()
            (state / 'session-metadata.json').write_text(json.dumps({'schema_version': 1, 'revision': 1,
                'label_catalog': {'labels': ['旧标签'], 'groups': ['历史分组']},
                'sessions': {corpus.uid('same'): {'starred': True, 'starred_at': 1, 'labels': ['旧标签'], 'group': '历史分组'}}}))
            ids = corpus.root / 'ids'; ids.mkdir()
            (ids / 'node-id').write_text(name * 32 + '\n')
            corpora.append(corpus); states.append(state)
            nodes.append(SimpleNamespace(name=name, nid=name * 32, port=free_port(), token=TOKEN))
        native = {p: p.read_bytes() for c in corpora for p in c.paths.values()}
        launch = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**launch)
        contexts, stacks = [], [ExitStack(), ExitStack()]
        hub = None
        errors = []
        def start_node(i):
            return stacks[i].enter_context(isolated_server(corpora[i], args.binary, state_dir=states[i],
                extra_env=node_env(corpora[i].root, nodes[i].port, '127.0.0.0/8')))[0]
        def page_at(base, mobile=False, clock=False):
            context = browser.new_context(service_workers='block', viewport={'width': 390 if mobile else 1280, 'height': 900}, has_touch=mobile)
            contexts.append(context)
            # An obsolete saved dropdown selection must never hide sessions/groups.
            context.add_init_script("localStorage.setItem('sessiondock.groupFilter', JSON.stringify('历史分组'))")
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page(); page.on('pageerror', lambda error: errors.append(str(error)))
            if clock: page.clock.install()
            page.goto(base, wait_until='networkidle')
            try:
                page.wait_for_function('SessionDockGroups.available && S.sessions.length > 0')
            except Exception as exc:
                raise AssertionError({'page_errors': errors, 'catalog': page.request.get(base + '/api/groups').text(),
                    'ui': page.evaluate('({groups: typeof SessionDockGroups, sessions: S.sessions.length})')}) from exc
            expect(page.locator('#session-group-filter, #session-group-filters')).to_have_count(0)
            return page
        def tree(page):
            page.locator('#view [data-v="tree"]').click()
        def check_group_icon(page):
            button = page.get_by_role('button', name='会话分组', exact=True)
            # Inspect the rendered button, so a monochrome desktop font fallback
            # fails even if the markup still contains the label character.
            for selected in (False, True):
                if selected:
                    button.click()
                    expect(button).to_have_class('on')
                    expect(page.locator('#session-group-add')).to_be_visible()
                image = Image.open(BytesIO(button.screenshot())).convert('RGB')
                yellow = sum(r > 100 and g > 80 and b < r * .75 and g > b * 1.3
                    for r, g, b in (image.getpixel((x, y))
                        for y in range(image.height) for x in range(image.width)))
                assert yellow > 5, f'group icon lost its yellow emoji rendering: selected={selected}, pixels={yellow}'
            tree(page)
            print('PASS yellow group icon and view switching:', page.viewport_size, flush=True)
        def edit(page, uid, hold=False, hover=False):
            item = page.locator(f'#side .item[data-uid="{uid}"]')
            if hold:
                item.dispatch_event('pointerdown', {'pointerType': 'touch', 'pointerId': 1, 'button': 0, 'clientX': 30, 'clientY': 300})
                expect(page.locator('#item-menu')).to_be_visible()
                item.dispatch_event('pointerup', {'pointerType': 'touch', 'pointerId': 1})
                item.dispatch_event('click')
            else: item.click(button='right')
            trigger = page.locator('#item-menu [data-act="group"]')
            trigger.hover() if hover else trigger.click()
            expect(page.locator('#session-group-menu')).to_be_visible()
            expect(page.locator('#session-group-menu button').first).to_contain_text('未分组')
        def create(page, name, enter=True):
            page.locator('#view [data-v="group"]').click()
            expect(page.locator('#side > :last-child')).to_have_id('session-group-create-row')
            page.locator('#session-group-add').click()
            page.locator('#session-group-name').fill(name)
            if enter: page.locator('#session-group-name').press('Enter')
            else: page.locator('#session-group-create-row button').click()
            expect(page.locator('#session-group-name')).to_have_count(0)
            expect(page.get_by_role('button', name=f'删除分组 {name}', exact=True)).to_be_enabled()
        def assign(page, name):
            page.locator('#session-group-menu button').filter(has_text=name or '未分组').click()
            expect(page.locator('#session-group-menu')).to_be_hidden()
            expect(page.locator('#side-pick-group')).to_be_enabled() if page.locator('#side-pick-group').is_visible() else None
            page.wait_for_function('!document.querySelector(".session-group-delete:disabled")')
            page.wait_for_function('document.querySelector("#session-group-status").textContent === ""')
        def create_from_menu(page, name):
            page.locator('#session-group-menu-add').click()
            input = page.locator('#session-group-menu-name')
            expect(input).to_be_focused()
            input.fill(f'  {name}  ')
            input.press('Enter')
            expect(page.locator('#session-group-menu')).to_be_hidden()
            expect(input).to_have_count(0)
            page.wait_for_function('document.querySelector("#session-group-status").textContent === ""')
            if page.locator('#side-pick-group').is_visible(): expect(page.locator('#side-pick-group')).to_be_enabled()
        def remove(page, name):
            page.locator('#view [data-v="group"]').click()
            page.get_by_role('button', name=f'删除分组 {name}', exact=True).click()
            expect(page.get_by_role('button', name=f'删除分组 {name}', exact=True)).to_have_count(0, timeout=20000)
            expect(page.locator('#session-group-add')).to_be_enabled(timeout=20000)
        def stored(i):
            return json.loads((states[i] / 'session-metadata.json').read_text())
        def menu_refresh_focus(page, uid):
            tree(page)
            page.locator(f'#side .item[data-uid="{uid}"]').click(button='right')
            trigger = page.locator('#item-menu [data-act="group"]')
            trigger.focus(); trigger.press('ArrowRight')
            menu = page.locator('#session-group-menu')
            expect(menu).to_be_visible()
            # Navigate away from the checked assignment with real keyboard input.
            # A refresh must keep this choice, even when a new item precedes it.
            names = menu.locator('button').evaluate_all('(buttons) => buttons.map(button => button.dataset.groupName)')
            page.keyboard.press('Home')
            for _ in range(names.index('待办')): page.keyboard.press('ArrowDown')
            focused = menu.locator('button[data-group-name="待办"]')
            expect(focused).to_be_focused()
            expect(focused).to_have_attribute('aria-checked', 'false')
            extra = 'AA刷新分组'
            def catalog_refresh(route):
                response = route.fetch(); data = response.json()
                data['groups'] = [*data['groups'], extra]
                route.fulfill(response=response, json=data)
            page.route('**/api/groups', catalog_refresh)
            try:
                # Exercise the normal background refresh without a wall-clock sleep.
                # With the UI event channel healthy that fallback read is slow.
                page.clock.fast_forward(60000)
                expect(menu.locator(f'button[data-group-name="{extra}"]')).to_be_visible()
                refreshed = menu.locator('button').evaluate_all('(buttons) => buttons.map(button => button.dataset.groupName)')
                assert refreshed.index(extra) < refreshed.index('待办'), refreshed
                expect(focused).to_be_focused()
                page.keyboard.press('ArrowDown')
                expect(menu.locator('button').nth((refreshed.index('待办') + 1) % len(refreshed))).to_be_focused()
                page.keyboard.press('ArrowUp')
                expect(focused).to_be_focused()
            finally:
                page.unroute('**/api/groups', catalog_refresh)
            # Restore the real catalog through the same refresh path before continuing.
            page.clock.fast_forward(60000)
            expect(menu.locator(f'button[data-group-name="{extra}"]')).to_have_count(0)
            expect(focused).to_be_focused()
            focused.press('ArrowLeft')
            expect(menu).to_be_hidden(); expect(trigger).to_be_focused()
            trigger.press('Escape')
            expect(page.locator('#item-menu')).to_be_hidden()
        try:
            bases = [start_node(i) for i in range(2)]
            local = [page_at(base, clock=i == 0) for i, base in enumerate(bases)]
            check_group_icon(local[0])
            uid_a, uid_b = corpora[0].uid('same'), corpora[1].uid('same')
            assert local[0].locator('#session-group-dialog').count() == 0
            create(local[0], '待办')
            expect(local[0].locator('#side .gname').filter(has_text='未分组')).to_have_count(0)
            expect(local[0].locator('#side .item')).to_have_count(1)
            local[0].locator('#session-group-add').click()
            local[0].locator('#session-group-name').fill('取消创建')
            # A refresh must not discard the inline draft/focus.
            local[0].evaluate('renderSide()')
            expect(local[0].locator('#session-group-name')).to_have_value('取消创建')
            expect(local[0].locator('#session-group-name')).to_be_focused()
            local[0].locator('#session-group-name').press('Escape')
            expect(local[0].locator('#session-group-name')).to_have_count(0)
            if args.screenshots_dir: local[0].locator('#side').screenshot(path=str(args.screenshots_dir / 'desktop.png'))
            menu_refresh_focus(local[0], uid_a)
            tree(local[0]); edit(local[0], uid_a, hover=True)
            expect(local[0].locator('#session-group-menu [aria-checked="true"]')).to_contain_text('历史分组')
            assign(local[0], '待办')
            expect(local[0].locator(f'#side .item[data-uid="{uid_a}"] .session-row-group')).to_contain_text('待办')
            # Create from the assignment menu without switching to Group view.
            other = corpora[0].uid('other')
            edit(local[0], other)
            local[0].locator('#session-group-menu-add').focus()
            local[0].keyboard.press('Enter')
            input = local[0].locator('#session-group-menu-name')
            expect(input).to_be_focused()
            input.fill('   '); input.press('Enter')
            expect(input).to_be_focused()
            input.fill('取消菜单创建')
            for key in ('ArrowLeft', 'ArrowDown', 'Home', 'End'):
                input.press(key); expect(input).to_be_focused()
            local[0].dispatch_event('body', 'sessiondock-ui-sessions')
            local[0].clock.fast_forward(60000)
            expect(input).to_have_value('取消菜单创建')
            expect(input).to_be_focused()
            input.press('Escape')
            expect(input).to_have_count(0)
            expect(local[0].locator('#session-group-menu')).to_be_visible()
            expect(local[0].locator('#session-group-menu-add')).to_be_focused()
            assert '取消菜单创建' not in stored(0)['group_catalog']['groups']
            local[0].locator('#session-group-menu-add').click()
            input.fill('菜单新组')
            input.dispatch_event('keydown', {'key': 'Enter', 'isComposing': True})
            assert '菜单新组' not in stored(0)['group_catalog']['groups']
            # A failed write keeps the draft and focus so Enter can retry.
            def fail_create(route):
                if route.request.method == 'POST': route.fulfill(status=503, json={'error': '暂时无法创建'})
                else: route.continue_()
            local[0].route('**/api/groups', fail_create)
            input.press('Enter')
            expect(local[0].locator('#session-group-status')).to_have_text('暂时无法创建')
            expect(input).to_have_value('菜单新组'); expect(input).to_be_focused()
            local[0].unroute('**/api/groups', fail_create)
            input.press('Enter')
            expect(local[0].locator('#session-group-menu')).to_be_hidden()
            expect(local[0].locator(f'#side .item[data-uid="{other}"] .session-row-group')).to_contain_text('菜单新组')
            assert stored(0)['sessions'][other]['group'] == '菜单新组'
            assert stored(0)['sessions'][uid_a]['group'] == '待办'
            remove(local[0], '菜单新组')
            create(local[1], '稍后', enter=False); tree(local[1]); edit(local[1], uid_b); assign(local[1], '稍后')
            assert stored(0)['sessions'][uid_a]['starred'] is True
            assert '待办' not in stored(1)['group_catalog']['groups']
            assert 'labels' not in stored(0)['sessions'][uid_a]
            assert 'label_catalog' not in stored(0)
            hubroot = root / 'hub'; hubroot.mkdir()
            (hubroot / 'hub-cache').mkdir()
            (hubroot / 'hub-cache/labels.json').write_text(json.dumps({'labels': ['缓存旧标签'], 'groups': ['缓存分组']}))
            hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, nodes); hub.start()
            page = page_at(f'http://127.0.0.1:{hub.port}')
            for i in range(2):
                assert set(stored(i)['group_catalog']['groups']) == {'历史分组', '缓存分组', '待办', '稍后'}
            a, b = scoped(nodes[0].nid, uid_a), scoped(nodes[1].nid, uid_b)
            for view in ('tree', 'date'):
                page.locator(f'#view [data-v="{view}"]').click()
                expect(page.locator('#side .item')).to_have_count(4)
            page.locator('#view [data-v="group"]').click()
            expect(page.locator('#side .item')).to_have_count(2)
            for name in ('历史分组', '缓存分组', '待办', '稍后'):
                expect(page.get_by_role('button', name=f'删除分组 {name}', exact=True)).to_be_enabled()
            page.reload(wait_until='networkidle')
            page.wait_for_function('SessionDockGroups.available')
            expect(page.locator('#side .item')).to_have_count(2)
            tree(page)
            page.evaluate("store.set('labelFilter', ['旧标签'])")
            page.reload(wait_until='networkidle'); page.wait_for_function('SessionDockGroups.available')
            expect(page.locator('#side .item')).to_have_count(4)
            tree(page)
            # The second level opens with keyboard and closes independently.
            page.locator(f'#side .item[data-uid="{a}"]').click(button='right')
            trigger = page.locator('#item-menu [data-act="group"]'); trigger.focus(); trigger.press('ArrowRight')
            expect(page.locator('#session-group-menu')).to_be_visible()
            page.locator('#session-group-menu button').first.press('ArrowLeft')
            expect(page.locator('#session-group-menu')).to_be_hidden(); expect(trigger).to_be_focused()
            trigger.click(); page.locator('#session-group-menu button').first.press('Escape')
            expect(page.locator('#item-menu')).to_be_visible(); trigger.press('Escape')
            expect(page.locator('#item-menu')).to_be_hidden()
            # Cross-node multi-select assigns immediately without a dialog.
            page.locator(f'#side .item[data-uid="{a}"]').click(button='right')
            page.locator('#item-menu [data-act="pick"]').click()
            page.locator(f'#side .item[data-uid="{b}"]').click()
            page.locator('#side-pick-group').click(); create_from_menu(page, '搁置')
            for i, uid in enumerate((uid_a, uid_b)): assert stored(i)['sessions'][uid]['group'] == '搁置'
            page.locator('#side-pick-cancel').click()
            mobile = page_at(bases[0], mobile=True); mobile.emulate_media(color_scheme='dark')
            check_group_icon(mobile)
            edit(mobile, uid_a, hold=True)
            create_from_menu(mobile, '手机新组')
            expect(mobile.locator(f'#side .item[data-uid="{uid_a}"] .session-row-group')).to_contain_text('手机新组')
            assert stored(0)['sessions'][uid_a]['group'] == '手机新组'
            remove(mobile, '手机新组'); tree(mobile); edit(mobile, uid_a, hold=True)
            mobile.locator('#session-group-menu-add').click()
            bounds = mobile.locator('#session-group-menu').bounding_box()
            assert bounds['x'] >= 0 and bounds['x'] + bounds['width'] <= 390
            input_bounds = mobile.locator('#session-group-menu-name').bounding_box()
            assert input_bounds['x'] >= bounds['x'] and input_bounds['x'] + input_bounds['width'] <= bounds['x'] + bounds['width']
            mobile.locator('#session-group-menu-name').fill('关闭丢弃草稿')
            mobile.locator('#view [data-v="tree"]').click()
            expect(mobile.locator('#session-group-menu')).to_be_hidden()
            edit(mobile, uid_a, hold=True)
            expect(mobile.locator('#session-group-menu-name')).to_have_count(0)
            assert '关闭丢弃草稿' not in stored(0)['group_catalog']['groups']
            if args.screenshots_dir: mobile.screenshot(path=str(args.screenshots_dir / 'mobile.png'))
            assign(mobile, '')
            expect(mobile.locator(f'#side .item[data-uid="{uid_a}"] .session-row-group')).to_be_hidden()
            assert 'group' not in stored(0)['sessions'][uid_a]
            # Delete with a node offline. Rejoin must not resurrect its catalog/assignment.
            for context in contexts: context.close()
            contexts.clear(); stacks[1].close()
            page = page_at(f'http://127.0.0.1:{hub.port}')
            remove(page, '搁置')
            expect(page.locator('#session-group-status')).to_contain_text('恢复连接', timeout=20000)
            assert '搁置' in stored(1)['group_catalog']['groups']
            assert stored(1)['sessions'][uid_b]['group'] == '搁置'
            for context in contexts: context.close()
            contexts.clear(); hub.stop(); hub.start()
            bases[1] = start_node(1); rejoined = page_at(bases[1])
            rejoined.locator('#view [data-v="group"]').click()
            expect(rejoined.get_by_role('button', name='删除分组 搁置', exact=True)).to_have_count(0, timeout=25000)
            assert 'group' not in stored(1)['sessions'][uid_b]
            # Recreate the same name with a newer operation; deletion does not permanently reserve names.
            create(rejoined, '搁置')
            tree(rejoined); edit(rejoined, uid_b); assign(rejoined, '搁置')
            page = page_at(f'http://127.0.0.1:{hub.port}')
            page.locator('#view [data-v="group"]').click()
            expect(page.get_by_role('button', name='删除分组 搁置', exact=True)).to_be_enabled()
            for context in contexts: context.close()
            contexts.clear(); hub.stop()
            for stack in stacks: stack.close()
            bases = [start_node(i) for i in range(2)]
            for i, base in enumerate(bases):
                solo = page_at(base)
                solo.locator('#view [data-v="group"]').click()
                expect(solo.get_by_role('button', name='删除分组 搁置', exact=True)).to_be_enabled()
                assert stored(i)['sessions'][corpora[i].uid('same')]['starred'] is True
            # Standalone deletion of a populated group clears the assignment atomically.
            remove(solo, '搁置')
            assert 'group' not in stored(1)['sessions'][uid_b]
            tree(solo); edit(solo, uid_b)
            assert solo.locator('#session-group-menu [data-group-name]').evaluate_all('(buttons) => buttons.map(button => button.dataset.groupName)')[0] == ''
            assert set(solo.locator('#session-group-menu [data-group-name]').evaluate_all('(buttons) => buttons.map(button => button.dataset.groupName)')) == {'', '历史分组', '缓存分组', '待办', '稍后'}
            solo.locator('#session-group-menu button').first.press('Escape')
            solo.locator('#item-menu [data-act="group"]').press('Escape')
            remaining = solo.evaluate('SessionDockGroups.names')
            for name in remaining: remove(solo, name)
            expect(solo.locator('#side .group')).to_have_count(0)
            expect(solo.locator('#side .item')).to_have_count(0)
            expect(solo.locator('#side > :last-child')).to_have_id('session-group-create-row')
            assert stored(1)['group_catalog']['groups'] == []
            assert stored(1)['group_catalog']['changes']
            create(solo, '空列表新建')
            assert all(p.read_bytes() == raw for p, raw in native.items())
            assert not errors, errors
            print('PASS groups browser: no filter dropdown, obsolete filter ignored across views/reload, inline create/cancel/delete, menu create on Enter with single/batch/mobile assignment, blank/cancel/IME/retry/draft focus, no dialog/ungrouped section, submenus hover/click/keyboard/touch, keyboard focus survives catalog refresh, immediate assignment, offline deletion/rejoin, same-name recreation, restarts, standalone deletion, native files unchanged')
        finally:
            for context in contexts: context.close()
            if hub: hub.stop()
            for stack in stacks: stack.close()
            browser.close()


if __name__ == '__main__':
    main()
