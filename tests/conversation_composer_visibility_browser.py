"""Composer and footer share visibility; terminal full-screen stays editor-free."""
import argparse
from contextlib import ExitStack
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright, expect
from hub_fixtures import FakeNode, Hub, REPO, scoped


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-composer-visibility-') as tmp, ExitStack() as stack:
        active = FakeNode('a' * 32, 'Active')
        inactive = FakeNode('b' * 32, 'Idle')
        for node in (active, inactive):
            stack.callback(node.stop)
        active.set(term_sessions=[{'name': 'composer-fixture', 'uid': 'claude:same-file-hash',
            'sid': 'same-native-id', 'source': 'claude', 'instance_id': '1' * 32, 'cwd': '/same/project'}])
        hub = Hub(REPO / 'target/debug/sessiondock-hub', Path(tmp), [active, inactive])
        hub.start(); stack.callback(hub.stop)
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            for width, theme in [(1280, 'light'), (1280, 'dark'), (390, 'light'), (390, 'dark')]:
                context = browser.new_context(viewport={'width': width, 'height': 850}, color_scheme=theme)
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda e: errors.append(str(e)))
                page.goto(f'http://127.0.0.1:{hub.port}/')
                page.wait_for_function('T.listLoaded && S.sessions.length === 2')
                def select(node):
                    card = page.locator(f'#side .item[data-uid="{scoped(node.nid, "claude:same-file-hash")}"]')
                    if width < 720 and not card.is_visible():
                        page.get_by_role('button', name='返回会话列表', exact=True).click()
                    card.click()
                    page.get_by_role('button', name='对话', exact=True).click()
                select(inactive)
                expect(page.locator('#msgs')).to_be_visible()
                expect(page.locator('#composer')).to_be_hidden()
                expect(page.locator('#conversation-status')).to_be_hidden()
                page.screenshot(path=str(args.screenshots / f'{width}-{theme}-not-started.png'))
                select(active)
                expect(page.locator('#cinput')).to_be_visible()
                expect(page.locator('#conversation-status')).to_be_visible()
                text = f'Keep my conversation draft {width} {theme}'
                with page.expect_response(lambda r: '/api/session/conversation' in r.url and r.request.method == 'POST'):
                    page.locator('#cinput').fill(text)
                page.screenshot(path=str(args.screenshots / f'{width}-{theme}-chat.png'))
                page.get_by_role('button', name='终端', exact=True).click()
                expect(page.locator('#cinput')).to_be_hidden()
                expect(page.locator('#conversation-status')).to_be_hidden()
                page.screenshot(path=str(args.screenshots / f'{width}-{theme}-terminal.png'))
                page.get_by_role('button', name='对话', exact=True).click()
                expect(page.locator('#cinput')).to_be_visible()
                expect(page.locator('#cinput')).to_have_value(text)
                expect(page.locator('#conversation-status')).to_be_visible()
                select(inactive)
                expect(page.locator('#composer')).to_be_hidden()
                expect(page.locator('#conversation-status')).to_be_hidden()
                select(active)
                expect(page.locator('#cinput')).to_have_value(text)
                expect(page.locator('#cinput')).to_be_visible()
                expect(page.locator('#conversation-status')).to_be_visible()
                assert not errors, errors
                context.close()
                print('PASS composer/footer visibility, edit, mode switch, retained draft', width, theme, flush=True)
            browser.close()


if __name__ == '__main__':
    main()
