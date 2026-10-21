#!/usr/bin/env python3
"""Kiro approval capability must not expose the OpenCode permissions editor."""
from __future__ import annotations

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import server
from history_fixtures import BINARY, REPO
from hub_fixtures import Hub, free_port
from kiro_sdk_fixtures import KiroFixture
from node_auth_fixtures import TOKEN, node_env
from opencode_sdk_fixtures import FakeOpenCode, SID
from private_hosts import private_hosts
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize


def exercise(browser, base, fixture, native, hub_mode, width, theme, screenshots):
    label = f'{"hub" if hub_mode else "node"}-{width}-{theme}'
    context = browser.new_context(viewport={'width': width, 'height': 900},
                                  color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(20000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    try:
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        page.wait_for_function('''async sids => {
            await loadSessions(true);
            return sids.every(sid => S.sessions.some(row => row.sid === sid));
        }''', arg=[fixture.sid, SID], polling=300)

        def open_source(sid):
            if page.locator('#detail .mobile-back').is_visible():
                page.locator('#detail .mobile-back').click()
            uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', sid)
            assert ('~' in uid) == hub_mode, uid
            page.locator(f'#side .item[data-uid="{uid}"]').click()
            page.locator('#a-sdk').click()
            page.wait_for_function('SdkView.pane && !SdkView.historyReady')
            page.locator('#sdk-input').click()
            page.wait_for_function('SdkView.connected && !SdkView.readOnly')
            return uid

        open_source(fixture.sid)
        assert page.evaluate('SdkView.capabilities.permissions === true'), 'Keep native approval support enabled'
        if page.locator('#sdk-more').is_visible():
            page.locator('#sdk-more').click()
            expect(page.locator('#sdk-more-menu')).to_be_visible()
        expect(page.locator('[data-sdk-action="permissions"]')).to_be_hidden()
        expect(page.locator('#native-sdk-permission_mode')).to_be_visible()
        expect(page.locator('textarea[aria-label="权限规则 JSON"]')).to_have_count(0)
        if screenshots:
            page.screenshot(path=str(screenshots / f'permissions-kiro-{label}.png'), full_page=True)
        if page.locator('#sdk-more-menu').is_visible():
            page.locator('#sdk-more').click()

        open_source(SID)
        action = page.locator('[data-sdk-action="permissions"]')
        if not action.is_visible():
            page.locator('#sdk-more').click()
        expect(action).to_be_visible()
        action.click()
        editor = page.locator('textarea[aria-label="权限规则 JSON"]')
        expect(editor).to_be_visible()
        expect(page.locator('#sdk-action-dialog')).to_contain_text('OpenCode')
        rules = [{'action': 'shell', 'resource': f'permissions-{label}', 'effect': 'ask'}]
        editor.fill(json.dumps(rules))
        if screenshots:
            page.screenshot(path=str(screenshots / f'permissions-opencode-{label}.png'), full_page=True)
        with page.expect_response(lambda response: response.request.method == 'POST'
                                  and response.url.endswith('/api/native-sdk/settings')) as saved:
            page.locator('#sdk-action-dialog button[type="submit"]').click()
        assert saved.value.ok, saved.value.text()
        page.wait_for_function('rules => JSON.stringify(SdkView.raw.permissions) === JSON.stringify(rules)', arg=rules)
        assert native.sessions[SID]['permissions'] == rules
        assert not fixture.received('session/prompt'), 'This regression must not request a Kiro model'
        assert not any(method == 'POST' and path.endswith('/prompt') for method, path, _ in native.received)
        assert not errors, errors
        print(f'PASS SDK permissions {label}: Kiro approval retained without editor; OpenCode rules saved', flush=True)
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path)
    parser.add_argument('--web-dir', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    hub_binary = (args.hub_binary or binary.with_name('sessiondock-hub')).resolve(strict=True)

    def host_binary(name):
        sibling = binary.with_name(name)
        return (sibling if sibling.is_file() else REPO / 'target' / 'debug' / name).resolve(strict=True)

    if args.web_dir:
        os.environ['SESSIONDOCK_TEST_WEB_DIR'] = str(args.web_dir.resolve(strict=True))
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-perms-') as temporary:
        root = Path(temporary)
        for name in ('claude', 'codex', 'grok', 'state'):
            (root / name).mkdir()
        fixture = KiroFixture(root, host_binary('ptyhost'))
        try:
            with ExitStack() as stack:
                native = stack.enter_context(FakeOpenCode(root))
                config = json.loads(fixture.launcher.read_text())
                config['profiles'].append(native.profile())
                fixture.launcher.write_text(json.dumps(config))
                initialize('--initialize-lifecycle', root / 'ledger', binary)
                node_port = free_port()
                env = {**fixture.env, **node_env(root, node_port, '127.0.0.0/8'),
                       'SESSIONDOCK_NATIVE_SDKS': 'auto',
                       'SESSIONDOCK_SDKHOST_BIN': str(host_binary('sdkhost')),
                       'SESSIONDOCK_STATE_DIR': str(root / 'state'),
                       'SESSIONDOCK_LIFECYCLE_DIR': str(root / 'ledger'),
                       'SESSIONDOCK_PTYHOST_DIR': str(fixture.host_dir),
                       'SESSIONDOCK_OPENCODE_DB': str(native.db),
                       'SESSIONDOCK_OPENCODE_ROOT': str(root / 'mirror'),
                       'SESSIONDOCK_PROC_ROOT': str(root / 'proc')}
                stack.enter_context(private_hosts(fixture.host_dir))
                base = stack.enter_context(server(SimpleNamespace(root=root), binary, 'off', env))
                hub_root = root / 'hub'
                hub_root.mkdir()
                hub = Hub(hub_binary, hub_root,
                          [SimpleNamespace(name='PermissionsNode', token=TOKEN, port=node_port)])
                hub.start()
                stack.callback(hub.stop)
                with sync_playwright() as playwright:
                    browser = playwright.chromium.launch(headless=True)
                    try:
                        for hub_mode in (False, True):
                            origin = f'http://127.0.0.1:{hub.port}' if hub_mode else base
                            for width in (1280, 390):
                                for theme in ('light', 'dark'):
                                    exercise(browser, origin, fixture, native, hub_mode, width, theme, args.screenshots)
                    finally:
                        browser.close()
        finally:
            stop_private_sdkhosts(root)
    print('Native SDK permission editor source gating: PASS')


if __name__ == '__main__':
    main()
