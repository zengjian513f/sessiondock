#!/usr/bin/env python3
"""Compact SDK composer across viewport/sidebar widths and interface scales.

Chromium uses private histories and a fake app-server. No real model or account.
"""
from __future__ import annotations

import argparse
import asyncio
import json
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build, server
from codex_sdk_controls_browser import ControlsServer
from history_fixtures import BINARY


def single_row(page):
    page.wait_for_function("""() => {
      const bar = document.querySelector('.sdk-options');
      return bar && Math.abs(CodexSdkView.toolbarWidth - bar.clientWidth) <= 1;
    }""")
    measurements = page.locator('.sdk-options').evaluate("""bar => {
      const bounds = bar.getBoundingClientRect();
      const controls = [...bar.querySelectorAll('.sdk-pickers > label, .sdk-pickers > button, .sdk-pickers > #sdk-permissions')]
        .filter(node => node.getClientRects().length && !node.hidden)
        .map(node => { const r = node.getBoundingClientRect(); return {id: node.id || node.className,
          left:r.left, right:r.right, top:r.top, bottom:r.bottom}; });
      return {left:bounds.left, right:bounds.right, top:bounds.top, bottom:bounds.bottom,
        overflow:bar.scrollWidth - bar.clientWidth, controls};
    }""")
    assert measurements['overflow'] <= 1, measurements
    for control in measurements['controls']:
        assert control['left'] >= measurements['left'] - 1, measurements
        assert control['right'] <= measurements['right'] + 1, measurements
        assert abs(control['top'] - measurements['top']) <= 2, measurements
        assert abs(control['bottom'] - measurements['bottom']) <= 2, measurements
    expect(page.locator('#sdk-model')).to_be_visible()
    expect(page.locator('#sdk-effort')).to_be_visible()
    expect(page.locator('#sdk-more')).to_be_visible()
    expect(page.locator('#sdk-input')).to_be_visible()
    assert page.locator('#sdk-input').bounding_box()['width'] > 80


def within_viewport(page, menu):
    box = menu.bounding_box()
    assert box and box['x'] >= 0 and box['y'] >= 0, box
    assert box['x'] + box['width'] <= page.viewport_size['width'] + 1, box
    assert box['y'] + box['height'] <= page.viewport_size['height'] + 1, box


def exercise(browser, base, corpus, fake, theme, scale, screenshots):
    context = browser.new_context(viewport={'width': 1440, 'height': 900}, color_scheme=theme,
                                  service_workers='block')
    context.add_init_script("localStorage.setItem('sessiondock.interfaceScale', " + json.dumps(str(scale)) + ");")
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='networkidle')
    page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
    expect(page.locator('#sdk-mode')).to_have_value('default')
    for selector in ('#sdk-model', '#sdk-effort', '#sdk-mode', '#sdk-control-approval', '#sdk-control-sandbox'):
        assert not page.locator(selector + ' option[value=""]').count(), selector
        assert not any(word in page.locator(selector).text_content() for word in ('沿用', '未返回', 'Default')), selector
    fake.set_settings('gpt-6.1-sol-realistic-long-model-name-2026-10-17', 'xhigh')
    expect(page.locator('#sdk-model')).to_have_value('gpt-6.1-sol-realistic-long-model-name-2026-10-17')

    widths = (1440, 1280, 1100, 1024, 900, 820, 768, 720, 600, 500, 430, 390, 360, 320)
    for width in widths:
        page.set_viewport_size({'width': width, 'height': 844})
        if not page.locator('#sdk-input').is_visible():
            page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
        single_row(page)
        usage = page.locator('#sdk-usage-toggle')
        if usage.is_visible():
            usage.click()
            menu = page.locator('#sdk-usage-menu')
            expect(menu).to_be_visible()
            expect(menu.locator('.sdk-credits')).to_have_text('Credits 1,235')
            expect(menu.locator('[data-gauge="5h"] b')).to_have_text('40%')
            within_viewport(page, menu)
            if screenshots and width in (1024, 390, 320):
                page.screenshot(path=str(screenshots / f'{theme}-{scale}-{width}-usage.png'))
            page.keyboard.press('Escape')
            expect(menu).to_be_hidden()
        else:
            expect(page.locator('.sdk-credits')).to_be_visible()
        page.locator('#sdk-more').click()
        menu = page.locator('#sdk-more-menu')
        expect(menu).to_be_visible()
        expect(menu.locator('#sdk-reset-card')).to_be_visible()
        expect(page.locator('[data-sdk-action]')).to_have_count(4)
        expect(page.locator('#sdk-actions select, #sdk-controls, #sdk-controls-dialog')).to_have_count(0)
        if menu.locator('#sdk-mode').count():
            expect(menu.locator('#sdk-mode')).to_be_visible()
        else:
            expect(page.locator('.sdk-pickers > .sdk-mode')).to_be_visible()
        within_viewport(page, menu)
        if screenshots and width in (1280, 390, 320):
            page.screenshot(path=str(screenshots / f'{theme}-{scale}-{width}-more.png'))
        page.locator('#sdk-input').click()
        expect(menu).to_be_hidden()
        if screenshots and width in (1440, 1280, 1024, 768, 600, 390, 320):
            page.screenshot(path=str(screenshots / f'{theme}-{scale}-{width}-composer.png'))

    # Controls remain usable at the narrowest width; opening them never sends a turn.
    starts = len(fake.methods('turn/start'))
    if not page.locator('[data-sdk-action="review"]').is_visible(): page.locator('#sdk-more').click()
    page.locator('[data-sdk-action="review"]').click()
    dialog = page.locator('#sdk-action-dialog')
    expect(dialog).to_be_visible()
    dialog.get_by_role('button', name='取消', exact=True).click()
    page.locator('#sdk-more').click()
    dialog = page.locator('#sdk-permissions')
    expect(dialog).to_be_visible()
    expect(dialog.locator('[id^="sdk-goal-"]')).to_have_count(0)
    expect(dialog.locator('#sdk-control-approval')).to_be_visible()
    page.keyboard.press('Escape')
    page.locator('#sdk-more').click()
    page.locator('#sdk-reset-use').click()
    page.locator('dialog[open]').get_by_role('button', name='取消', exact=True).click()
    assert len(fake.methods('turn/start')) == starts
    assert not fake.methods('account/rateLimitResetCredit/consume')
    page.locator('#sdk-input').fill('窄屏下保留尚未发送的草稿 / realistic project')
    page.locator('#sdk-model').select_option('fake-luna')
    page.locator('#sdk-effort').select_option('medium')
    expect(page.locator('#sdk-input')).to_have_value('窄屏下保留尚未发送的草稿 / realistic project')

    # Actual sidebar resizing must switch layout even without a viewport resize.
    if scale == 100:
        page.set_viewport_size({'width': 1280, 'height': 900})
        single_row(page)
        exposed = page.locator('.sdk-pickers > [data-sdk-action], .sdk-pickers > #sdk-permissions').count()
        assert exposed > 0, 'wide composer forcibly hid every secondary control'
        grip = page.locator('#drag').bounding_box()
        x, y = grip['x'] + grip['width'] / 2, grip['y'] + 100
        page.mouse.move(x, y)
        page.mouse.down()
        page.mouse.move(x + 240, y, steps=12)
        page.mouse.up()
        page.wait_for_function('prior => document.querySelectorAll(".sdk-pickers > [data-sdk-action], .sdk-pickers > #sdk-permissions").length < prior', arg=exposed)
        single_row(page)
        expect(page.locator('#sdk-model')).to_have_value('fake-luna')
        expect(page.locator('#sdk-effort')).to_have_value('medium')
    # Live gauge composition changes must reflow without a viewport resize.
    page.set_viewport_size({'width': 1440, 'height': 900})
    rates = fake._bucket(); rates['secondary'] = None; rates['credits']['balance'] = '12345.6'
    asyncio.run_coroutine_threadsafe(fake.notify('account/rateLimits/updated', {'rateLimits': rates}), fake.loop).result(timeout=5)
    expect(page.locator('.sdk-credits')).to_have_text('Credits 12,346')
    expect(page.locator('[data-gauge="7d"]')).to_be_hidden()
    single_row(page)
    asyncio.run_coroutine_threadsafe(fake.notify('account/rateLimits/updated', {'rateLimits': fake._bucket()}), fake.loop).result(timeout=5)
    expect(page.locator('.sdk-credits')).to_have_text('Credits 1,235')
    single_row(page)
    assert not errors, errors
    print(f'PASS compact SDK composer: {theme}, {scale}%, {len(widths)} widths, popovers/actions/draft/sidebar', flush=True)
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-layout-') as temporary, sync_playwright() as p:
        root = Path(temporary)
        corpus = build(root)
        with ControlsServer(root/'app.sock', corpus.paths['sdk'], THREAD) as fake, server(corpus, args.binary, root/'app.sock') as base:
            fake.cards[0]['title'] = '限额重置卡 · realistic-long-description 中文说明 ' * 4
            browser = p.chromium.launch(headless=True)
            try:
                for theme, scale in (('light', 100), ('dark', 100), ('light', 125), ('dark', 125)):
                    exercise(browser, base, corpus, fake, theme, scale, args.screenshots)
            finally:
                browser.close()


if __name__ == '__main__':
    main()
