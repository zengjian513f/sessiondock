#!/usr/bin/env python3
"""Open a linked conversation while another node's terminal discovery is slow."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
import time
from urllib.parse import urlencode

from playwright.sync_api import expect, sync_playwright
from hub_fixtures import FakeNode, Hub, REPO, scoped


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-composer-startup-') as tmp, ExitStack() as stack:
        nodes = [FakeNode(c * 32, name) for c, name in [('a', 'Ready'), ('b', 'Slow')]]
        for node in nodes:
            stack.callback(node.stop)
            node.set(term_sessions=[{'name': 'sessiondock-fixture', 'uid': 'claude:same-file-hash',
                                    'sid': 'same-native-id', 'source': 'claude',
                                    'instance_id': '1' * 32, 'cwd': '/same/project'}])
        nodes[1].set(term_delay=4)
        hub = Hub(REPO / 'target/debug/sessiondock-hub', Path(tmp), nodes)
        hub.start()
        stack.callback(hub.stop)
        with sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            for width, theme in [(1280, 'dark'), (390, 'light'), (1280, 'light'), (390, 'dark')]:
                context = browser.new_context(viewport={'width': width, 'height': 900}, service_workers='block')
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                saved_name = nodes[1].nid + '~sessiondock-fixture'
                saved_uid = scoped(nodes[1].nid, 'claude:same-file-hash')
                context.add_init_script('''localStorage.setItem('sessiondock.hub./.theme', %s);
                    localStorage.setItem('sessiondock.hub./.termviews', %s);
                    localStorage.setItem('sessiondock.hub./.termLayoutPolicyVersion', '2');'''
                    % (json.dumps(json.dumps(theme)), json.dumps(json.dumps([[saved_name, {
                        'uid': saved_uid, 'instance_id': '1' * 32, 'mode': 'collapsed'}]]))))
                base = f'http://127.0.0.1:{hub.port}/'
                started = time.monotonic()
                page.goto(base + '?' + urlencode({'sid': 'claude:same-native-id', 'node': nodes[0].nid}),
                          wait_until='domcontentloaded')
                expect(page.locator('#cinput')).to_be_visible(timeout=2500)
                elapsed = time.monotonic() - started
                page.locator('#cinput').fill('继续核对当前会话 — startup draft')
                print(f'composer {width}px {theme}: {elapsed:.3f}s', flush=True)
                if args.screenshots:
                    args.screenshots.mkdir(parents=True, exist_ok=True)
                    page.screenshot(path=str(args.screenshots / f'{width}-{theme}.png'))
                # The unqueried machine must keep its persisted view until
                # the complete list arrives, rather than be treated as gone.
                assert page.evaluate('name => T.openViews.has(name)', saved_name)
                assert page.evaluate('Object.keys(Nodes.capabilities).length') == 1
                page.wait_for_function('T.list.length === 2', timeout=12000)
                expect(page.locator('#cinput')).to_have_value('继续核对当前会话 — startup draft')
                assert page.evaluate('name => T.openViews.has(name)', saved_name)
                # Once the background inventory arrives, normal user navigation works.
                if width < 600:
                    page.get_by_role('button', name='返回会话列表', exact=True).click()
                page.locator(f'#side .item[data-uid="{saved_uid}"] .t').click()
                expect(page.locator('#cinput')).to_be_visible()
                page.locator('#cinput').fill('另一台机器的草稿')
                assert not errors, errors
                context.close()
            browser.close()
    print('PASS hub_composer_startup_browser', flush=True)


if __name__ == '__main__':
    main()
