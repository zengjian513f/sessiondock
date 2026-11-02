"""Transfer entry points follow native source support on nodes and the Hub."""
import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_start_stop_fixtures import provider


def exercise(browser, base, source, shots, label):
    supported = source in ('claude', 'codex', 'grok')
    for width, theme in ((1280, 'light'), (390, 'dark')):
        print(label, source, width, theme, flush=True)
        context = browser.new_context(viewport={'width': width, 'height': 900},
                                      color_scheme=theme, has_touch=True, service_workers='block')
        page = context.new_page()
        page.set_default_timeout(15000)
        errors, plans = [], []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.on('request', lambda request: plans.append(request.post_data_json)
                if request.url.endswith('/api/session/clone/plan') else None)
        try:
            page.goto(base, wait_until='networkidle')
            page.wait_for_function('T.listLoaded')
            assert page.evaluate('SessionDockCapabilities.config.session_clone_local_codex === true')
            rows = page.locator(f'#side .item[data-uid^="{source}:"]')
            row = rows.filter(has_text='Synthetic Kiro V3 Session').first if source == 'kiro' else rows.first
            if not row.is_visible():
                page.locator(f'#chips button[data-source="{source}"]').click()
            uid = row.get_attribute('data-uid')
            assert ('~' in uid) == (label == 'hub')
            row = page.locator(f'#side .item[data-uid="{uid}"]')
            row.click()
            page.wait_for_function("SdkView.selection?.mode !== 'loading'")
            action = page.locator('#a-clone-group')
            page.locator('#a-more').click()
            page.screenshot(path=str(shots/f'{label}-{source}-{width}-header.png'), full_page=True)
            if supported:
                expect(action).not_to_have_attribute('hidden', '')
            else:
                expect(action).to_be_hidden()
                # A late invocation must not bypass the same source gate.
                page.evaluate('uid => cloneSessionGroup(uid)', uid)
                expect(page.locator('#clone-group-dialog')).to_have_count(0)
            page.keyboard.press('Escape')
            if page.locator('#detail .mobile-back').is_visible():
                page.locator('#detail .mobile-back').click()
            if width == 390:
                touch = context.new_cdp_session(page)
                box = row.bounding_box()
                touch.send('Input.dispatchTouchEvent', {'type': 'touchStart', 'touchPoints': [
                    {'x': box['x'] + 100, 'y': box['y'] + box['height'] / 2}]})
                expect(page.locator('#item-menu')).to_be_visible()
                touch.send('Input.dispatchTouchEvent', {'type': 'touchEnd', 'touchPoints': []})
                touch.detach()
            else:
                row.click(button='right')
            menu_action = page.locator('#item-menu [data-act="clone"]')
            page.screenshot(path=str(shots/f'{label}-{source}-{width}-sidebar.png'), full_page=True)
            if supported:
                expect(menu_action).to_be_visible()
                expect(menu_action).not_to_have_attribute('aria-disabled', 'true')
                with page.expect_response(lambda response: response.url.endswith('/api/session/clone/plan')):
                    menu_action.click()
                expect(page.locator('#clone-group-dialog')).to_be_visible()
                assert plans[-1]['uid'] == uid
                page.locator('#clone-group-dialog .clone-cancel').click()
            else:
                expect(menu_action).to_be_hidden()
                assert not plans, plans
            assert not errors, errors
        finally:
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='kiro,agy,opencode,codebuddy,grok,claude,codex')
    parser.add_argument('--screenshots', type=Path, default=Path('target/transfer-source-review'))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-transfer-sources-') as temporary, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(temporary)/source
                root.mkdir()
                # The legacy-named transfer capability requires a configured
                # Codex root even when the selected session uses another CLI.
                extra_env = {}
                if source in ('claude', 'codebuddy'):
                    (root/'codex').mkdir()
                    extra_env['SESSIONDOCK_CODEX_ROOT'] = str(root/'codex')
                with provider(source, root, args.binary, extra_env=extra_env) as (base, peer, extra):
                    exercise(browser, base, source, args.screenshots, 'node')
                    hubroot = root/'hub'
                    hubroot.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot,
                              [SimpleNamespace(name='Transfer sources', token=TOKEN,
                                  port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', source, args.screenshots, 'hub')
                    finally:
                        hub.stop()
        finally:
            browser.close()
    print('PASS transfer source gates: node/Hub, header/overflow/sidebar/long press, supported previews')


if __name__ == '__main__':
    main()
