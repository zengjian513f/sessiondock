#!/usr/bin/env python3
"""Keep the native Codex reset-card picker open during background refreshes."""
import argparse
import asyncio
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build, server
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from sdk_ui_fixtures import start_sdk


def exercise(page, fake, uid, screenshot):
    def notify(method, params):
        asyncio.run_coroutine_threadsafe(fake.notify(method, params), fake.loop).result(timeout=5)

    page.locator(f'#side .item[data-uid="{uid}"]').click()
    start_sdk(page)
    pane = page.locator('.sdk-pane')
    turn = {'id': 'reset-picker-turn', 'status': 'inProgress', 'items': []}
    notify('turn/started', {'turn': turn})
    expect(pane.locator('.sdk-status')).to_have_text('处理中')

    menu = pane.locator('#sdk-more-menu')
    more = pane.locator('#sdk-more')
    more.click()
    picker = menu.locator('#sdk-reset-card')
    open_picker = menu.locator('#sdk-reset-card:open')
    expect(picker.locator('option')).to_have_count(2)
    picker.select_option('card-b')
    if page.viewport_size['width'] < 720:
        picker.tap()
    else:
        picker.click()
    expect(open_picker).to_have_count(1)
    original = picker.locator('option:checked').element_handle()
    if screenshot:
        page.screenshot(path=str(screenshot.with_name(f'{screenshot.stem}-open.png')))

    def assert_picker_unchanged():
        assert original.evaluate('option => option === option.parentElement?.selectedOptions[0]'), 'refresh replaced the selected reset card'
        expect(open_picker).to_have_count(1)
        expect(picker).to_be_focused()
        expect(menu).to_be_visible()

    for used in (2200, 2400):
        notify('thread/tokenUsage/updated', {'turnId': turn['id'], 'tokenUsage': {
            'last': {'totalTokens': used}, 'modelContextWindow': 4000}})
        expect(pane.locator('[data-gauge="context"] b')).to_have_text(f'{used // 40}%')
        assert_picker_unchanged()

    # A fresh account response changes the quota and carries the same cards.
    fake.used['secondary'] = 31
    notify('account/rateLimits/updated', {'rateLimits': fake._bucket()})
    expect(pane.locator('[data-gauge="7d"] b')).to_have_text('31%')
    assert_picker_unchanged()
    if screenshot:
        page.screenshot(path=str(screenshot))

    # Changed cards still render, and the current choice can be redeemed.
    fake.cards[1]['title'] = 'Full reset · updated'
    notify('account/rateLimits/updated', {'rateLimits': fake._bucket()})
    expect(picker.locator('option:checked')).to_contain_text('Full reset · updated')
    expect(picker).to_have_value('card-b')
    page.keyboard.press('Escape')
    expect(open_picker).to_have_count(0)
    if not menu.is_visible():
        more.click()
    menu.locator('#sdk-reset-use').click()
    confirm = page.locator('dialog[open]')
    expect(confirm).to_contain_text('Full reset · updated')
    confirm.get_by_role('button', name='兑换', exact=True).click()
    expect(picker.locator('option')).to_have_count(1)
    expect(picker).to_have_value('card-a')
    expect(menu.locator('#sdk-reset-use')).to_have_text('兑换 (1)')
    assert fake.methods('account/rateLimitResetCredit/consume')[-1]['params']['creditId'] == 'card-b'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sdkhost', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-reset-picker-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        corpus = build(root)
        port = free_port()
        state = root / 'state'
        state.mkdir()
        env = {**node_env(root, port, '127.0.0.0/8'), 'SESSIONDOCK_STATE_DIR': str(state)}
        if args.sdkhost:
            env['SESSIONDOCK_SDKHOST_BIN'] = str(args.sdkhost.resolve(strict=True))
        with FakeAppServer(root / 'app.sock', corpus.paths['sdk'], THREAD) as fake, server(corpus, args.binary, root / 'app.sock', env) as base:
            cards = [{**card, 'title': 'Full reset'} for card in fake.cards]
            hubroot = root / 'hub'
            hubroot.mkdir()
            hub = Hub(args.binary.with_name('sessiondock-hub'), hubroot,
                      [SimpleNamespace(name='Quota node', token=TOKEN, port=port)])
            hub.start()
            try:
                browser = pw.chromium.launch(headless=True)
                for entry, url in (('node', base), ('hub', f'http://127.0.0.1:{hub.port}')):
                    for width, theme in ((390, 'light'), (390, 'dark'), (1440, 'light'), (1440, 'dark')):
                        fake.used = {'primary': 40, 'secondary': 25}
                        fake.cards = [dict(card) for card in cards]
                        context = browser.new_context(viewport={'width': width, 'height': 844},
                                                      color_scheme=theme, has_touch=width < 720, service_workers='block')
                        context.route('**/*', lambda route: route.continue_()
                                      if route.request.url.startswith(url + '/') else route.abort())
                        try:
                            page = context.new_page()
                            page.set_default_timeout(10000)
                            page.goto(url)
                            page.wait_for_function('T.listLoaded')
                            uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', THREAD)
                            screenshot = args.screenshots / f'{entry}-{width}-{theme}.png' if args.screenshots else None
                            exercise(page, fake, uid, screenshot)
                        finally:
                            context.close()
                        print(f'PASS reset-card picker: {entry} {width}px {theme}', flush=True)
                browser.close()
            finally:
                hub.stop()


if __name__ == '__main__':
    main()
