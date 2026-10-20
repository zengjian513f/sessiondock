"""Composer and footer share visibility; terminal full-screen stays editor-free."""
import argparse
from contextlib import ExitStack
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright, expect
from hub_fixtures import FakeNode, Hub, REPO, scoped


def exercise_start(browser, base, screenshots, label):
    from session_start_fixtures import choose_mode
    from popups import on_popup
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        context = browser.new_context(viewport={'width': width, 'height': 850}, color_scheme=theme)
        page = context.new_page()
        errors, launches = [], []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.on('request', lambda request: launches.append(request.post_data_json)
                if request.method == 'POST' and '/api/term/takeover' in request.url else None)
        on_popup(page, lambda dialog: dialog.accept())
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded')
        choose_mode(page, 'codex', 'cli')
        page.locator('#settings-dialog .modal-close').click()
        page.locator('#side .item[data-uid^="codex:"]').first.click()
        expect(page.locator('#composer')).to_be_visible()
        expect(page.locator('#csend')).to_be_disabled()
        draft = f'Preserved before CLI start {width}'
        with page.expect_response(lambda response: '/api/session/conversation' in response.url
                                  and response.request.method == 'POST'):
            page.locator('#cinput').fill(draft)
        assert not launches
        with page.expect_response(lambda response: '/api/term/takeover' in response.url) as started:
            page.locator('#conversation-status .footer-session-toggle').click()
        assert started.value.ok, started.value.text()
        expect(page.locator('#a-term')).to_have_attribute('aria-pressed', 'true')
        page.wait_for_function(r"""() => [...T.views.values()].some(view => {
            const b = view.term?.buffer?.active;
            return b && Array.from({length:b.length},(_,i)=>b.getLine(i)?.translateToString()).join('\n').includes('RS_SHELL_READY');
        })""")
        assert len(launches) == 1, launches
        page.locator('#a-chat').click()
        expect(page.locator('#cinput')).to_have_value(draft)
        power = page.locator('#conversation-status .footer-session-toggle')
        expect(power).to_have_attribute('aria-label', '停止会话')
        page.screenshot(path=str(screenshots/f'{label}-{width}-cli-started.png'))
        power.click()
        expect(power).to_have_attribute('aria-label', '启动会话')
        expect(power).to_be_enabled()
        expect(page.locator('#composer')).to_be_visible()
        expect(page.locator('#csend')).to_be_disabled()
        expect(page.locator('#cesc')).to_be_disabled()
        expect(page.locator('#cinput')).to_have_value(draft)
        page.screenshot(path=str(screenshots/f'{label}-{width}-cli-stopped.png'))
        assert not errors, errors
        context.close()
        print('PASS conversation footer CLI start/stop and saved draft', label, width, flush=True)


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
                expect(page.locator('#composer')).to_be_visible()
                expect(page.locator('#csend')).to_be_disabled()
                expect(page.locator('#cesc')).to_be_disabled()
                expect(page.locator('#conversation-status')).to_be_visible()
                expect(page.locator('#conversation-status .footer-session-toggle')).to_be_visible()
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
                expect(page.locator('#composer')).to_be_visible()
                expect(page.locator('#csend')).to_be_disabled()
                expect(page.locator('#cesc')).to_be_disabled()
                expect(page.locator('#conversation-status')).to_be_visible()
                select(active)
                expect(page.locator('#cinput')).to_have_value(text)
                expect(page.locator('#cinput')).to_be_visible()
                expect(page.locator('#conversation-status')).to_be_visible()
                assert not errors, errors
                context.close()
                print('PASS composer/footer visibility, edit, mode switch, retained draft', width, theme, flush=True)
            from types import SimpleNamespace
            from sdk_start_stop_fixtures import provider
            from node_auth_fixtures import TOKEN
            from private_hosts import private_hosts
            root = Path(tmp)/'cli'; root.mkdir()
            with private_hosts(root, hosts=('menu-host',)), provider(
                    'codex', root, REPO/'target/debug/sessiondock', cli=True) as (base, _, extra):
                exercise_start(browser, base, args.screenshots, 'node')
                (root/'hub').mkdir()
                cli_hub = Hub(REPO/'target/debug/sessiondock-hub', root/'hub', [
                    SimpleNamespace(name='CLI', token=TOKEN,
                                    port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                cli_hub.start()
                try:
                    exercise_start(browser, f'http://127.0.0.1:{cli_hub.port}', args.screenshots, 'hub')
                finally:
                    cli_hub.stop()
            browser.close()


if __name__ == '__main__':
    main()
