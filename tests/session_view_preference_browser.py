"""Offline/external session clicks follow settings without starting a runtime."""
import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_start_stop_fixtures import provider
from session_start_fixtures import choose_mode


def exercise(browser, base, source, shots, label):
    has_chat = source not in ('kiro', 'codebuddy')
    for width, theme in ((1280, 'light'), (390, 'dark')):
        context = browser.new_context(viewport={'width': width, 'height': 900},
                                      color_scheme=theme, service_workers='block')
        page = context.new_page()
        page.set_default_timeout(15000)
        errors, writes = [], []
        state = {'external': False}
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.on('request', lambda request: writes.append(request.url)
                if request.method == 'POST' and request.url.split('?')[0].endswith(
                    ('/open', '/takeover', '/stop', '/turn', '/create')) else None)

        def runtime(route):
            response = route.fetch()
            data = response.json()
            data.update(known=True, sdk=False, tmux=False,
                        external=state['external'], running=state['external'])
            route.fulfill(response=response, json=data)

        def live(route):
            route.fulfill(json={'known': True, 'uids': [state['uid']] if state.get('uid') and state['external'] else [],
                                'sdk_uids': [], 'tmux_uids': [], 'working_uids': []})

        page.route('**/api/session/runtime*', runtime)
        page.route('**/api/live*', live)
        try:
            page.goto(base, wait_until='networkidle')
            page.wait_for_function('T.listLoaded')
            row = page.locator(f'#side .item[data-uid^="{source}:"]').first
            if not row.is_visible():
                page.locator(f'#chips button[data-source="{source}"]').click()
            state['uid'] = row.get_attribute('data-uid')
            row = page.locator(f'#side .item[data-uid="{state["uid"]}"]')
            assert ('~' in state['uid']) == (label == 'hub')

            def back():
                if page.locator('#detail .mobile-back').is_visible():
                    page.locator('#detail .mobile-back').click()

            for external in (False, True):
                state['external'] = external
                for mode in ('cli', 'sdk', 'cli'):
                    print(label, source, width, 'external' if external else 'offline', mode, flush=True)
                    back()
                    choose_mode(page, source, mode)
                    page.locator('#settings-dialog .modal-close').click()
                    row.click()
                    page.wait_for_function("SdkView.selection?.mode !== 'loading' && !SdkView.discovering")
                    name = f'{label}-{source}-{width}-{"external" if external else "offline"}-{mode}'
                    page.screenshot(path=str(shots/f'{name}.png'), full_page=True)
                    expected = 'sdk' if mode == 'sdk' else 'chat' if has_chat else 'terminal'
                    assert page.evaluate('SdkView.selection.mode') == expected
                    if mode == 'sdk':
                        expect(page.locator('#sdk-input')).to_be_visible()
                        expect(page.locator('#sdk-send')).to_be_disabled()
                    elif has_chat:
                        expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
                    else:
                        expect(page.locator('#sdk-input')).not_to_be_visible()
                    back()
                    row.click()  # Warm selection and persisted preference after reload.
                    page.reload(wait_until='networkidle')
                    page.wait_for_function("SdkView.selection?.mode !== 'loading' && !SdkView.discovering")
                    assert page.evaluate('SdkView.selection.mode') == expected
                    assert not writes, writes
                    # Leave an explicit opposite view behind before the next
                    # click, so the persisted choice competes with settings.
                    if has_chat:
                        page.locator('#a-chat' if mode == 'sdk' else '#a-sdk').click()
                        back()
                        row.click()
                        page.wait_for_function("SdkView.selection?.mode !== 'loading' && !SdkView.discovering")
                        assert page.evaluate('SdkView.selection.mode') == expected
                        assert not writes, writes
            assert not errors, errors
        finally:
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='codex,claude,grok,agy,opencode,kiro,codebuddy')
    parser.add_argument('--screenshots', type=Path, default=Path('target/session-view-preference'))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-view-preference-') as temporary, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(temporary)/source
                root.mkdir()
                with provider(source, root, args.binary) as (base, peer, extra):
                    exercise(browser, base, source, args.screenshots, 'node')
                    hubroot = root/'hub'
                    hubroot.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot,
                              [SimpleNamespace(name='Preferences', token=TOKEN,
                                  port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', source, args.screenshots, 'hub')
                    finally:
                        hub.stop()
        finally:
            browser.close()
    print('PASS session view preference: offline/external, SDK/CLI, reload, prior views, no runtime writes')


if __name__ == '__main__':
    main()
