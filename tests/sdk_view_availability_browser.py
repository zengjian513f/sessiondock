"""SDK view entries survive failed Hub capability observations."""
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
    for width, theme in ((1280, 'light'), (390, 'dark')):
        print(source, width, theme, flush=True)
        context = browser.new_context(viewport={'width': width, 'height': 900},
                                      color_scheme=theme, service_workers='block')
        page = context.new_page()
        errors, writes = [], []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.on('request', lambda request: writes.append(request.url)
                if request.method == 'POST' and request.url.split('?')[0].endswith(
                    ('/open', '/takeover', '/stop', '/turn', '/create')) else None)
        try:
            page.goto(base, wait_until='networkidle')
            page.wait_for_function('T.listLoaded')
            row = page.locator(f'#side .item[data-uid^="{source}:"]').first
            if not row.is_visible():
                page.locator(f'#chips button[data-source="{source}"]').click()
            state = {'uid': row.get_attribute('data-uid')}
            row.click()
            page.wait_for_function("SdkView.selection?.mode === 'sdk' && !SdkView.discovering")
            expect(page.locator('#sdk-input')).to_be_visible()
            if source not in ('kiro', 'codebuddy'):
                page.locator('#a-chat').click()
            page.screenshot(path=str(shots/f'{label}-{source}-{width}-before.png'), full_page=True)
            # A failed node observation is not an authoritative SDK disable.
            # Keep the Hub's real response shape, including false capability
            # flags, while the runtime endpoint remains independently usable.
            fault = {'mode': 'failed'}
            def capabilities(route):
                response = route.fetch()
                data = response.json()
                nid = state['uid'].split(':', 1)[1].split('~', 1)[0]
                cap = data['capabilities'][nid]
                cap.update(codex_sdk=False, native_sdks={}, enabled=False)
                data['errors'] = ([{'node_id': nid, 'error': 'synthetic term timeout'}]
                                  if fault['mode'] == 'failed' else [])
                data['partial'] = bool(data['errors'])
                route.fulfill(response=response, json=data)
            page.route('**/api/term/list*', capabilities)
            page.evaluate('loadTermList(true)')
            page.wait_for_function("Nodes.errors.get('term')?.length > 0", timeout=30000)
            # Inspect the whole failed-state page before testing its entry.
            page.screenshot(path=str(shots/f'{label}-{source}-{width}-capability-failed.png'), full_page=True)
            expect(page.locator('#a-sdk')).to_be_visible()
            assert not page.evaluate('agentSdkAvailable(sessionSourceOf(S.sel), nodeOf(S.sel))')
            page.locator('#a-sdk').click()
            expect(page.locator('#sdk-input')).to_be_visible()
            expect(page.locator('#sdk-send')).to_be_disabled()
            expect(page.locator('#a-session-toggle')).to_have_attribute('aria-disabled', 'true')
            page.locator('#sdk-input').fill('保留 SDK 草稿')
            if source not in ('kiro', 'codebuddy'):
                page.locator('#a-chat').click()
            if page.locator('#detail .mobile-back').is_visible():
                page.locator('#detail .mobile-back').click()
            row.click()
            page.wait_for_function("SdkView.selection?.mode === 'sdk' && !SdkView.discovering")
            expect(page.locator('#sdk-input')).to_have_value('保留 SDK 草稿')
            expect(page.locator('#sdk-send')).to_be_disabled()
            page.screenshot(path=str(shots/f'{label}-{source}-{width}-capability-sdk.png'), full_page=True)
            assert not writes, writes
            # A successful observation explicitly disabling SDK must still
            # hide the entry; cached capability must not overrule it.
            fault['mode'] = 'disabled'
            page.evaluate('loadTermList(true)')
            page.wait_for_function("!Nodes.errors.get('term')?.length", timeout=30000)
            expect(page.locator('#a-sdk')).to_be_hidden()
            page.unroute('**/api/term/list*')
            page.evaluate('loadTermList(true)')
            page.wait_for_function("SdkView.available(S.sel, S.agent)", timeout=30000)
            expect(page.locator('#a-sdk')).to_be_visible()
            assert not writes, writes
            assert not errors, errors
        finally:
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='codex,claude,grok,agy,opencode,kiro,codebuddy')
    parser.add_argument('--screenshots', type=Path, default=Path('target/sdk-view-availability'))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-view-preference-') as temporary, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(temporary)/source
                root.mkdir()
                with provider(source, root, args.binary) as (base, peer, extra):
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
    print('PASS SDK view availability: failed observations, confirmed disable, recovery, no runtime writes')


if __name__ == '__main__':
    main()
