#!/usr/bin/env python3
"""Explicit links reveal compact-catalog descendants on a private node and Hub."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import urlencode

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, isolated_server
from hub_fixtures import Hub, free_port, scoped
from nest_tree_browser import corpus
from node_auth_fixtures import node_env, TOKEN


def check_links(browser, base, data, node=None):
    prefix = 'sessiondock.hub./.' if node else 'sessiondock.'
    identity = lambda sid: scoped(node.nid, data.uid(sid)) if node else data.uid(sid)
    root, target = identity('nest-root-a'), identity('nest-grandchild-c')
    for width, theme in ((1400, 'dark'), (390, 'light')):
        for mode in ('hidden', 'nested', 'flat'):
            for sid, uid, agent, text in (
                ('codex:nest-grandchild-c', target, None, 'reply Grandchild C'),
                ('claude:nest-root-a/agent:x', root, 'x', 'Agent X still working answer'),
            ):
                context = browser.new_context(viewport={'width': width, 'height': 900}, service_workers='block')
                # The stored choices model arriving from another site. Reload
                # resets lazy branches, just as it does in a real new document.
                preferences = {'childMode': mode, 'nest': mode == 'nested', 'theme': theme,
                               'view': 'tree', 'off': ['codex'], 'activeOnly': True,
                               'sideCollapsed': True, 'nestClosed': [root, target],
                               'closed': [json.dumps([node.nid, '/proj/alpha'], separators=(',', ':'))
                                          if node else '/proj/alpha']}
                context.add_init_script('window.EventSource=undefined;\n' + '\n'.join(
                    f'localStorage.setItem({json.dumps(prefix + key)}, {json.dumps(json.dumps(value))});'
                    for key, value in preferences.items()))
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                query = {'sid': sid}
                if node:
                    query['node'] = node.nid
                url = base + '/?' + urlencode(query)
                page.goto(url, wait_until='networkidle')
                for navigation in ('initial', 'reload'):
                    if navigation == 'reload':
                        page.reload(wait_until='networkidle')
                    expect(page.locator('#msgs')).to_contain_text(text)
                    assert page.evaluate('S.sel') == uid
                    assert page.evaluate('S.childMode') == ('hidden' if mode == 'hidden' else 'shown')
                    assert page.evaluate('S.nest') == (mode == 'nested')
                    if width == 390:
                        page.locator('#detail .mobile-back').click()
                    selected = page.locator('#side .item.sel')
                    expect(selected).to_have_count(1)
                    expect(selected).to_be_in_viewport()
                    expect(selected).to_have_attribute('data-agent' if agent else 'data-uid', agent or uid)
                    if mode == 'hidden':
                        # Only the ancestry needed to show this row is open.
                        opened = page.evaluate('[...S.lazyOpen.values()].map(row => row.sid).sort()')
                        assert opened == (['nest-root-a'] if agent else ['nest-child-b', 'nest-root-a']), opened
                        assert page.evaluate('S.sessions.every(row => S.lazyOpen.has(row.uid) || !row.agent_items)')
                        # A later compact inventory must retain the selection.
                        page.evaluate('loadSessions(true)')
                        expect(selected).to_be_in_viewport()
                    if navigation == 'initial':
                        page.screenshot(path=f'/tmp/sessiondock-19dfa1-{"hub" if node else "node"}-{width}-{mode}-{"agent" if agent else "session"}.png')
                # Explicitly folding the selected row's ancestor remains a user
                # choice; normal polling must not force it back open.
                if mode == 'hidden':
                    page.locator(f'#side .item[data-uid="{root}"] .nest-caret').click()
                    expect(page.locator('#side .item.sel')).to_have_count(0)
                    page.wait_for_function('!sessionLoadActive')
                    page.evaluate('loadSessions(true)')
                    expect(page.locator('#side .item.sel')).to_have_count(0)
                    # Back/Forward invokes the link route in the same page.
                    other = identity('nest-alone-e')
                    page.locator(f'#side .item[data-uid="{other}"]').click()
                    expect(page.locator('#msgs')).to_contain_text('reply Standalone E')
                    page.go_back()
                    expect(page.locator('#msgs')).to_contain_text(text)
                    if width == 390:
                        page.locator('#detail .mobile-back').click()
                    expect(page.locator('#side .item.sel')).to_be_in_viewport()
                assert not errors, errors
                context.close()
                print(f'PASS {"hub" if node else "node"} {width}px {theme} {mode} {sid}: reveal, selection, reload and folds', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-link-reveal-') as temp, sync_playwright() as pw, ExitStack() as stack:
        data = corpus(Path(temp) / 'node')
        node = SimpleNamespace(name='Synthetic', nid='a'*32, port=free_port(), token=TOKEN)
        for directory in ('proc', 'ids'):
            (data.root / directory).mkdir()
        (data.root / 'ids/node-id').write_text(node.nid + '\n')
        env = node_env(data.root, node.port, '127.0.0.0/8')
        env['SESSIONDOCK_PROC_ROOT'] = str(data.root / 'proc')
        base, _ = stack.enter_context(isolated_server(data, args.binary, state_dir=data.root / 'state', extra_env=env))
        hubroot = Path(temp) / 'hub'
        hubroot.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [node])
        hub.start()
        stack.callback(hub.stop)
        browser = pw.chromium.launch(headless=True)
        stack.callback(browser.close)
        check_links(browser, f'http://127.0.0.1:{hub.port}', data, node)
        check_links(browser, base, data)


if __name__ == '__main__':
    main()
