#!/usr/bin/env python3
"""SDK overflow disclosure and separators follow actual contents, using fake RPC."""
import argparse
import asyncio
import re
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build, server
from codex_sdk_controls_browser import ControlsServer
from history_fixtures import BINARY
from sdk_usage_fixtures import assert_inline_usage


def exercise(browser, base, corpus, fake, theme, scale, screenshots):
    context = browser.new_context(viewport={'width': 1912, 'height': 948},
                                  color_scheme=theme, service_workers='block')
    context.add_init_script(f"localStorage.setItem('sessiondock.interfaceScale', '{scale}')")
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(10000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='networkidle')
    page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
    expect(page.locator('#sdk-mode')).to_have_value('default')
    expect(page.locator('.sdk-pane .sdk-status-row .sdk-status')).to_have_text('就绪')
    more, menu = page.locator('#sdk-more'), page.locator('#sdk-more-menu')
    permissions = page.locator('#sdk-permissions')

    def capture(state):
        if screenshots:
            page.screenshot(path=str(screenshots / f'{theme}-{scale}-{state}.png'))

    def notify(method, params):
        asyncio.run_coroutine_threadsafe(fake.notify(method, params), fake.loop).result(timeout=5)

    def separator(expected):
        expect(permissions).to_have_css('border-top-width', re.compile(r'(?!0px$).+px') if expected else '0px')

    def resize(width):
        page.set_viewport_size({'width': width, 'height': 948 if width > 720 else 844})
        page.evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
        if not page.locator('#sdk-input').is_visible():
            page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
        expect(page.locator('#sdk-input')).to_be_visible()

    # All controls fit inline; empty groups and unavailable reset cards leave no ⋯.
    expect(page.locator('.sdk-reset')).to_have_attribute('hidden', '')
    expect(more).to_be_hidden()
    expect(page.locator('.sdk-pickers [data-sdk-action]')).to_have_count(4)
    expect(permissions).to_be_hidden()
    assert_inline_usage(page)
    page.locator('#sdk-input').fill('Keep this draft while resizing and changing policies')
    capture('wide-empty')

    # A successful real policy interaction creates a message-only menu.
    page.locator('#sdk-control-approval').select_option('never')
    page.keyboard.press('Escape')
    expect(more).to_be_visible()
    more.click()
    expect(menu).to_be_visible()
    expect(page.locator('#sdk-controls-result')).to_have_text('权限设置已应用。')
    expect(menu.locator('#sdk-actions')).to_be_hidden()
    separator(False)
    capture('wide-message-only')
    page.keyboard.press('Escape')

    for width in (390, 320):
        resize(width)
        expect(more).to_be_visible()
        more.click()
        expect(menu).to_be_visible()
        expect(menu.locator('#sdk-actions')).to_be_visible()
        expect(menu.locator('#sdk-model')).to_be_visible()
        separator(True)
        capture(f'{width}-two-groups')
        page.keyboard.press('Escape')
        assert_inline_usage(page)
    # Widening with the menu open closes it and returns controls inline.
    more.click()
    resize(1912)
    expect(menu).to_be_hidden()
    more.click()
    separator(False)
    page.keyboard.press('Escape')
    expect(page.locator('#sdk-input')).to_have_value('Keep this draft while resizing and changing policies')

    page.reload(wait_until='networkidle')
    expect(page.locator('#sdk-mode')).to_have_value('default')
    expect(more).to_be_hidden()
    # A reset card arriving without a resize must reveal the menu, and removing
    # its last content while it is open must dismiss it and hide the trigger.
    saved_cards = fake.cards
    fake.cards = [dict(fake.overflow_card)]
    notify('turn/completed', {'turn': {'id': 'overflow-refresh', 'status': 'completed', 'items': []}})
    expect(more).to_be_visible()
    more.click()
    expect(menu.locator('#sdk-reset-card')).to_be_visible()
    expect(permissions).to_be_hidden()
    capture('wide-reset-only')
    fake.cards = saved_cards
    notify('turn/completed', {'turn': {'id': 'overflow-refresh', 'status': 'completed', 'items': []}})
    expect(more).to_be_hidden()
    expect(menu).to_be_hidden()
    expect(page.locator('#sdk-input')).to_be_focused()

    # With no messages, a narrow menu contains controls only, with no separator.
    resize(390)
    expect(more).to_be_visible()
    more.click()
    expect(menu.locator('#sdk-model')).to_be_visible()
    expect(permissions).to_be_hidden()
    separator(False)
    capture('390-controls-only')
    page.keyboard.press('Escape')
    resize(1912)
    expect(more).to_be_hidden()
    assert not errors, errors
    context.close()
    print(f'PASS SDK overflow: {theme}, {scale}%, empty/controls/message/reset/two groups, resize and live updates', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-overflow-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        corpus = build(root)
        with ControlsServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake:
            fake.overflow_card = dict(fake.cards[0])
            fake.cards = []
            with server(corpus, args.binary, root/'app.sock') as base:
                browser = pw.chromium.launch(headless=True)
                try:
                    for theme, scale in (('light', 100), ('dark', 100), ('light', 125), ('dark', 125)):
                        exercise(browser, base, corpus, fake, theme, scale, args.screenshots)
                finally:
                    browser.close()


if __name__ == '__main__':
    main()
