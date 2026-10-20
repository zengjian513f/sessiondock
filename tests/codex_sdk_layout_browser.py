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

from sdk_ui_fixtures import start_sdk
from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build, server
from codex_sdk_controls_browser import ControlsServer
from history_fixtures import BINARY
from sdk_usage_fixtures import assert_inline_usage, assert_reset_countdown


def single_row(page):
    page.wait_for_function("""() => {
      const row = document.querySelector('.sdk-pane .sdk-status-row');
      return row && Math.abs(CodexSdkView.toolbarWidth - row.clientWidth) <= 1;
    }""")
    measurements = page.locator('.sdk-pane .sdk-status-row').evaluate("""row => {
      const bounds = row.getBoundingClientRect();
      const controls = [...row.querySelectorAll(':scope > .footer-session-toggle, :scope > .sdk-status, :scope > .sdk-gauges, :scope > #sdk-usage-toggle, .sdk-pickers > label, .sdk-pickers > button')]
        .filter(node => node.getClientRects().length && !node.hidden)
        .map(node => { const r = node.getBoundingClientRect(); return {id: node.id || node.className,
          left:r.left, right:r.right, top:r.top, bottom:r.bottom}; });
      return {left:bounds.left, right:bounds.right, top:bounds.top, bottom:bounds.bottom,
        overflow:row.scrollWidth - row.clientWidth, controls};
    }""")
    assert measurements['overflow'] <= 1, measurements
    previous = measurements['left']
    for control in measurements['controls']:
        assert control['left'] >= previous - 1, measurements
        assert control['right'] <= measurements['right'] + 1, measurements
        assert abs((control['top'] + control['bottom']) / 2 - (measurements['controls'][0]['top'] + measurements['controls'][0]['bottom']) / 2) <= 1, measurements
        previous = control['right']
    for selector in ('#sdk-more', '#sdk-input'):
        expect(page.locator(selector)).to_be_visible()
    assert page.locator('#sdk-input').bounding_box()['width'] > 80


def within_viewport(page, menu):
    box = menu.bounding_box()
    assert box and box['x'] >= 0 and box['y'] >= 0, box
    assert box['x'] + box['width'] <= page.viewport_size['width'] + 1, box
    assert box['y'] + box['height'] <= page.viewport_size['height'] + 1, box


def exercise(browser, base, corpus, fake, theme, scale, screenshots):
    context = browser.new_context(viewport={'width': 1440, 'height': 900}, color_scheme=theme,
                                  service_workers='block', has_touch=True)
    context.add_init_script("localStorage.setItem('sessiondock.interfaceScale', " + json.dumps(str(scale)) + ");")
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='networkidle')
    page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
    start_sdk(page)
    expect(page.locator('#sdk-mode')).to_have_value('default')
    expect(page.locator('.sdk-pane .sdk-status')).to_have_text('就绪')
    expect(page.locator('.sdk-pane .sdk-status')).to_have_attribute('data-state', 'ready')
    page.locator('#sdk-input').fill('reset clock keeps this draft')
    assert_reset_countdown(page, page.locator('[data-gauge="5h"]'), fake._bucket()['primary']['resetsAt'])
    expect(page.locator('#sdk-input')).to_have_value('reset clock keeps this draft')
    page.locator('#sdk-input').fill('')
    for selector in ('#sdk-model', '#sdk-effort', '#sdk-tier', '#sdk-mode', '#sdk-control-approval', '#sdk-control-sandbox'):
        assert not page.locator(selector + ' option[value=""]').count(), selector
        assert not any(word in page.locator(selector).text_content() for word in ('沿用', '未返回', 'Default')), selector
    fake.set_settings('fake-sol', 'high')
    model_picker = page.locator('#sdk-model')
    expect(model_picker).to_have_value('fake-sol')
    short_width = model_picker.bounding_box()['width']
    assert 60 * scale / 100 < short_width < 130 * scale / 100, short_width
    if screenshots:
        page.screenshot(path=str(screenshots / f'{theme}-{scale}-short-models.png'))
    fake.set_settings('gpt-6.1-sol-realistic-long-model-name-2026-10-17', 'xhigh')
    expect(page.locator('#sdk-model')).to_have_value('gpt-6.1-sol-realistic-long-model-name-2026-10-17')
    page.wait_for_function('width => document.querySelector("#sdk-model").getBoundingClientRect().width > width + 5', arg=short_width)
    fake.settings['serviceTier'] = 'ultrafast'
    fake.set_settings(fake.settings['model'], fake.settings['effort'])
    expect(page.locator('#sdk-tier')).to_have_value('ultrafast')
    def notify(method, params):
        asyncio.run_coroutine_threadsafe(fake.notify(method, params), fake.loop).result(timeout=5)
    notify('turn/started', {'turn': {'id': 'layout-status-turn', 'status': 'inProgress', 'items': []}})

    widths = (1440, 1280, 1100, 1024, 900, 820, 768, 720, 675, 600, 500, 430, 390, 360, 320)
    for width in widths:
        page.set_viewport_size({'width': width, 'height': 844})
        if not page.locator('#sdk-input').is_visible():
            page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
        status = page.locator('.sdk-pane .sdk-status')
        for native_status, label in [
            ({'type': 'active', 'activeFlags': []}, '处理中'),
            ({'type': 'active', 'activeFlags': ['waitingOnApproval']}, '待批准'),
            ({'type': 'active', 'activeFlags': ['waitingOnUserInput']}, '待回答'),
            ({'type': 'systemError'}, '系统错误'),
        ]:
            notify('thread/status/changed', {'status': native_status})
            expect(status).to_have_text(label)
            expect(status).to_have_attribute('data-state', {
                '处理中': 'working', '待批准': 'waiting', '待回答': 'waiting', '系统错误': 'blocked'}[label])
            assert status.evaluate("e => getComputedStyle(e, '::before').backgroundColor === getComputedStyle(e).color")
            assert status.evaluate("e => getComputedStyle(e, '::before').animationName") == ('sdk-status-breathe' if label == '处理中' else 'none')
            if width == 390 and label == '处理中':
                page.emulate_media(reduced_motion='reduce')
                assert status.evaluate("e => getComputedStyle(e, '::before').animationName") == 'none'
                page.emulate_media(reduced_motion='no-preference')
            single_row(page)
            assert status.evaluate('e => e.scrollWidth <= e.clientWidth + 1'), (width, scale, label)
            expect(status).to_be_disabled()
            if screenshots and label == '处理中' and width in (1440, 390, 320):
                page.screenshot(path=str(screenshots / f'{theme}-{scale}-{width}-working.png'))
        for kind in ('approval', 'sandbox'):
            select = page.locator(f'#sdk-control-{kind}')
            if not select.is_visible():
                page.locator('#sdk-more').click()
            expect(select).to_be_visible()
            assert select.evaluate('e => e instanceof HTMLSelectElement')
            select.click()
            expect(select.locator('option:checked')).to_have_count(1)
            expect(page.locator(f'#sdk-policy-{kind}')).to_have_count(0)
            within_viewport(page, select)
            if screenshots and width in (1440, 390, 320):
                page.screenshot(path=str(screenshots / f'{theme}-{scale}-{width}-{kind}.png'))
            page.keyboard.press('Escape')
            page.locator('#sdk-input').click()
        page.locator('#sdk-input').fill('/unsupported-command')
        page.locator('#sdk-send').click()
        status = page.locator('.sdk-pane .sdk-status')
        expect(status).to_have_text('不支持的命令')
        expect(status).to_have_attribute('data-state', 'blocked')
        single_row(page)
        full_status = status.get_attribute('title')
        assert len(status.text_content()) <= 6
        details = page.get_by_role('dialog', name='完整状态')
        status.tap()
        expect(details).to_be_visible()
        expect(details).to_have_text(full_status)
        within_viewport(page, details)
        if screenshots and width in (675, 390, 320):
            page.screenshot(path=str(screenshots / f'{theme}-{scale}-{width}-status.png'))
        page.keyboard.press('Escape')
        expect(details).to_be_hidden()
        status.press('Enter')
        expect(details).to_be_visible()
        page.locator('#sdk-input').click()
        expect(details).to_be_hidden()
        assert_inline_usage(page)
        expect(page.locator('.sdk-status-row > .sdk-gauges .sdk-credits')).to_have_text('🪙 1,235')
        expect(page.locator('.sdk-status-row > .sdk-gauges [data-gauge="5h"] b')).to_have_text('40%')
        expect(page.locator('[data-gauge="5h"] > span')).to_have_text('5小时')
        expect(page.locator('[data-gauge="7d"] > span')).to_have_text('7天')
        page.locator('#sdk-more').click()
        menu = page.locator('#sdk-more-menu')
        expect(menu).to_be_visible()
        expect(menu.locator('#sdk-reset-card')).to_be_visible()
        expect(page.locator('#sdk-tier')).to_be_visible()
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
    expect(dialog).to_be_hidden()  # Empty permissions feedback is not a menu section.
    expect(dialog.locator('[id^="sdk-goal-"]')).to_have_count(0)
    expect(page.locator('#sdk-control-approval')).to_be_visible()
    expect(page.locator('#sdk-control-sandbox')).to_be_visible()
    expect(dialog.locator('#sdk-control-approval, #sdk-control-sandbox')).to_have_count(0)
    page.keyboard.press('Escape')
    page.locator('#sdk-more').click()
    page.locator('#sdk-reset-use').click()
    page.locator('dialog[open]').get_by_role('button', name='取消', exact=True).click()
    assert len(fake.methods('turn/start')) == starts
    assert not fake.methods('account/rateLimitResetCredit/consume')
    page.locator('#sdk-input').fill('窄屏下保留尚未发送的草稿 / realistic project')
    page.locator('#sdk-more').click()
    page.locator('#sdk-model').select_option('fake-luna')
    page.locator('#sdk-effort').select_option('medium')
    page.keyboard.press('Escape')
    expect(page.locator('#sdk-input')).to_have_value('窄屏下保留尚未发送的草稿 / realistic project')

    # Actual sidebar resizing must preserve status/usage without a viewport resize.
    if scale == 100:
        page.set_viewport_size({'width': 1440, 'height': 900})
        single_row(page)
        prior_width = page.locator('.sdk-pane .sdk-status-row').bounding_box()['width']
        exposed = page.locator('.sdk-pickers > [data-sdk-action], .sdk-pickers > .sdk-policy').count()
        assert exposed > 0, 'wide composer forcibly hid every secondary control'
        grip = page.locator('#drag').bounding_box()
        x, y = grip['x'] + grip['width'] / 2, grip['y'] + 100
        page.mouse.move(x, y)
        page.mouse.down()
        page.mouse.move(x + 320, y, steps=12)
        page.mouse.up()
        page.wait_for_function('prior => document.querySelector(".sdk-pane .sdk-status-row").getBoundingClientRect().width < prior - 200', arg=prior_width)
        single_row(page)
        assert_inline_usage(page)
        expect(page.locator('#sdk-model')).to_have_value('fake-luna')
        expect(page.locator('#sdk-effort')).to_have_value('medium')
    # Live gauge composition changes must reflow without a viewport resize.
    page.set_viewport_size({'width': 1440, 'height': 900})
    rates = fake._bucket(); rates['secondary'] = None; rates['credits']['balance'] = '12345.6'
    rates['primary']['usedPercent']=40.5
    asyncio.run_coroutine_threadsafe(fake.notify('account/rateLimits/updated', {'rateLimits': rates}), fake.loop).result(timeout=5)
    expect(page.locator('.sdk-credits')).to_have_text('🪙 12,346')
    progress=page.locator('[data-gauge="5h"] [role="progressbar"]')
    expect(progress).to_have_attribute('aria-valuenow','40.5')
    assert progress.locator('u').evaluate('e=>e.style.width')=='40.5%'
    assert '40.5%' in page.locator('[data-gauge="5h"]').get_attribute('title')
    # Rolling notifications are sparse: null is unavailable, not a deletion.
    expect(page.locator('[data-gauge="7d"]')).to_be_visible()
    single_row(page)
    # Only the authoritative account read may remove a missing quota window.
    page.route('**/api/codex-sdk/limits?*', lambda route: route.fulfill(status=200,
        content_type='application/json', body=json.dumps({'ok': True, 'raw': {
            'rateLimits': rates, 'rateLimitsByLimitId': {'codex': rates},
            'rateLimitResetCredits': {'availableCount': len(fake.cards), 'credits': fake.cards}}})), times=1)
    notify('turn/completed', {'turn': {'id': 'layout-status-turn', 'status': 'completed', 'items': []}})
    expect(page.locator('[data-gauge="7d"]')).to_be_hidden()
    single_row(page)
    asyncio.run_coroutine_threadsafe(fake.notify('account/rateLimits/updated', {'rateLimits': fake._bucket()}), fake.loop).result(timeout=5)
    expect(page.locator('.sdk-credits')).to_have_text('🪙 1,235')
    single_row(page)
    assert not errors, errors
    notify('turn/completed', {'turn': {'id': 'layout-status-turn', 'status': 'completed', 'items': []}})
    notify('thread/status/changed', {'status': {'type': 'idle'}})
    expect(page.locator('.sdk-pane .sdk-status')).to_have_text('就绪')
    for width in (1440, 390, 320):
        page.set_viewport_size({'width': width, 'height': 844})
        single_row(page)
        expect(page.locator('.sdk-pane .sdk-status')).to_have_text('就绪')
        expect(page.locator('.sdk-pane .sdk-status')).to_be_visible()
        expect(page.locator('.sdk-pane .sdk-status')).to_be_disabled()
        expect(page.locator('#sdk-send')).to_have_text('发送')
        expect(page.locator('#sdk-stop')).to_be_hidden()
        if screenshots:
            page.screenshot(path=str(screenshots / f'{theme}-{scale}-{width}-ready.png'))
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
