#!/usr/bin/env python3
"""Retained Codex loading does not wait for fleet inventory or reopen the harness."""
import argparse
from pathlib import Path
import re
import tempfile
import time
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_start_stop_fixtures import provider
from sdk_ui_fixtures import start_sdk
from sdkhost_fixtures import stop_private_sdkhosts


def exercise(browser, base, peer, label, shots):
    for width, theme in ((1280, 'light'), (390, 'dark')):
        context = browser.new_context(viewport={'width': width, 'height': 900},
                                      color_scheme=theme, service_workers='block')
        page = context.new_page()
        errors, opens, held_node, held_fleet = [], [], [], []
        page.on('pageerror', lambda error: errors.append(str(error)))
        context.on('request', lambda request: opens.append(request.url)
                   if request.method == 'POST' and request.url.endswith('/api/codex-sdk/open') else None)
        try:
            page.goto(base, wait_until='domcontentloaded')
            page.locator('#side .item[data-uid^="codex:"]').first.click()
            start_sdk(page)
            with page.expect_response('**/api/session/conversation'):
                page.locator('#sdk-input').fill('Draft retained while another node is slow')
            def native_counts():
                return [len(peer.clients), *[len(peer.methods(method)) for method in
                    ('initialize', 'thread/resume', 'thread/turns/list', 'turn/start')]]
            before_native, before_opens = native_counts(), len(opens)
            pattern = re.compile(r'/api/term/list(?:\?|$)')
            if label == 'hub':
                def inventory(route):
                    target = parse_qs(urlsplit(route.request.url).query).get('nodes')
                    (held_node if target else held_fleet).append(route)
                page.route(pattern, inventory)
            # Hold the selected node until ownership has returned. This forces
            # selectFor to await the bootstrap promise, instead of winning a race.
            with page.expect_response('**/api/session/runtime?*'):
                page.reload(wait_until='domcontentloaded')
            if label == 'hub':
                expect(page.locator('#detail')).to_contain_text('earlier synthetic answer')
                page.wait_for_function("SdkView.selection?.mode === 'loading' && !T.listLoaded")
                assert held_node and not held_fleet
                held_node.pop().continue_()
            began = time.monotonic()
            page.wait_for_function('SdkView.connected', timeout=3000)
            elapsed = round((time.monotonic() - began) * 1000)
            expect(page.locator('#sdk-send')).to_be_enabled()
            expect(page.locator('#sdk-input')).to_have_value('Draft retained while another node is slow')
            assert len(opens) == before_opens, 'retained thread was reopened'
            if label == 'hub':
                assert held_fleet, 'full inventory was not held'
            page.screenshot(path=str(shots/f'{label}-{width}-loaded.png'), full_page=True)
            # Another frontend and a warm view switch reuse the same host and
            # native subscription; no initialize/resume/recovery/inference RPC.
            second = context.new_page()
            second.on('pageerror', lambda error: errors.append(str(error)))
            second.goto(page.url, wait_until='domcontentloaded')
            second.wait_for_function('SdkView.connected', timeout=3000)
            page.locator('#a-chat').click()
            page.locator('#a-sdk').click()
            page.wait_for_function('SdkView.connected', timeout=3000)
            assert native_counts() == before_native, (before_native, native_counts())
            assert len(opens) == before_opens, opens
            assert not errors, errors
            print(f'PASS {label}/{width}: loaded in {elapsed} ms after owning node ready; '
                  f'fleet held={label == "hub"}, two pages and warm switch reuse native channel without open', flush=True)
        finally:
            for route in [*held_node, *held_fleet]:
                route.abort()
            page.unroute_all(behavior='ignoreErrors')
            context.unroute_all(behavior='ignoreErrors')
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path(tempfile.gettempdir())/'sd-sdk-loading')
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-loading-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        browser = pw.chromium.launch(headless=True)
        try:
            with provider('codex', root, args.binary) as (base, peer, extra):
                exercise(browser, base, peer, 'node', args.screenshots)
                hub_root = root/'hub'; hub_root.mkdir()
                hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                          [SimpleNamespace(name='LoadingNode', token=TOKEN,
                           port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                hub.start()
                try:
                    exercise(browser, f'http://127.0.0.1:{hub.port}', peer, 'hub', args.screenshots)
                finally:
                    hub.stop()
        finally:
            browser.close()
            stop_private_sdkhosts(root)


if __name__ == '__main__':
    main()
