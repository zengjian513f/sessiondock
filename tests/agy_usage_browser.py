#!/usr/bin/env python3
"""Agy usage directly in the conversation menu, using a private synthetic CLI."""
import argparse
import json
import os
from pathlib import Path
import tempfile
from unittest.mock import patch

from playwright.sync_api import expect, sync_playwright
from agy_sdk_browser import select_sdk, send
from agy_sdk_fixtures import AgyFixture
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from send_browser import initialize


def exercise(browser, base, fixture, screenshots):
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(10000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    select_sdk(page)
    page.locator('#sdk-more').click()
    detail = page.locator('#native-usage-detail')
    expect(detail).to_be_visible()
    expect(page.locator('#sdk-usage-toggle')).to_have_count(0)
    expect(detail).to_contain_text('原生尚未提供用量数据')
    expect(page.locator('#native-usage-refresh')).to_be_enabled()
    page.screenshot(path=str(screenshots/'agy-usage-empty.png'))
    page.keyboard.press('Escape')
    send(page, '用量下拉实际发送 synthetic turn')
    expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply')
    page.locator('#sdk-input').fill('保留草稿 retained draft')
    for width in (1280, 390, 320):
        page.set_viewport_size({'width': width, 'height': 900})
        if width < 500 and not page.locator('#sdk-input').is_visible():
            selected = page.evaluate('S.sel')
            page.locator(f'#side .item[data-uid="{selected}"]').click()
        for theme in ('light', 'dark'):
            print(f'Agy usage: {width}px {theme}', flush=True)
            page.emulate_media(color_scheme=theme)
            page.locator('#sdk-more').click()
            expect(detail).to_be_visible()
            expect(detail.locator('.sdk-usage-total')).to_have_text('12,800')
            expect(detail).to_contain_text('12,000')
            expect(detail).to_contain_text('800')
            expect(detail).to_contain_text('9,000')
            expect(detail).to_contain_text('已用 28%')
            expect(detail).to_contain_text('剩余 72%')
            expect(page.locator('#native-sdk-mode')).to_be_visible()
            menu = page.locator('#sdk-more-menu')
            bounds = menu.bounding_box()
            assert bounds['x'] >= 0 and bounds['y'] >= 0 and bounds['x']+bounds['width'] <= width+1, bounds
            assert menu.evaluate('n => n.scrollWidth <= n.clientWidth+1')
            page.screenshot(path=str(screenshots/f'agy-usage-{width}-{theme}.png'))
            page.keyboard.press('Escape')
            expect(detail).to_be_hidden()
            expect(page.locator('#sdk-input')).to_have_value('保留草稿 retained draft')
    # Refresh the real synthetic collector; it must not submit an inference turn.
    page.locator('#sdk-more').click()
    before = len(fixture.inputs())
    (fixture.root/'quota-hold').touch()
    page.locator('#native-usage-refresh').click()
    expect(page.locator('#native-usage-refresh')).to_be_disabled()
    expect(detail).to_contain_text('剩余 72%')
    page.screenshot(path=str(screenshots/'agy-usage-loading.png'))
    (fixture.root/'quota-remaining').write_text('0.09')
    (fixture.root/'quota-hold').unlink()
    expect(detail).to_contain_text('已用 91%')
    expect(detail.locator('.sdk-gauge.low')).to_be_visible()
    (fixture.root/'quota-fail').touch()
    page.locator('#native-usage-refresh').click()
    expect(detail).to_contain_text('查询失败')
    expect(detail).to_contain_text('剩余 9%')
    page.screenshot(path=str(screenshots/'agy-usage-failed.png'))
    (fixture.root/'quota-fail').unlink()
    # Multiple native quota groups and long diagnostics must stay readable.
    def long_quota(route):
        response = route.fetch()
        data = response.json()
        payload = data.get('raw', data)
        payload['quota']['buckets'].append({'group': 'Claude / GPT Enterprise shared premium 模型额度长名称',
            'window': 'weekly', 'remaining_percent': 0, 'resets_at': '2026-10-25T00:00:00Z'})
        payload['quota']['buckets'].append({'group': '缺失额度字段', 'window': '5h'})
        payload['quota']['error'] = 'Delayed native quota diagnostic. ' * 80
        route.fulfill(response=response, json=data)
    page.route('**/api/native-sdk/usage', long_quota, times=1)
    page.locator('#native-usage-refresh').click()
    expect(detail).to_contain_text('Enterprise shared premium')
    expect(detail).to_contain_text('额度暂不可用')
    expect(detail).to_contain_text('已用 100%')
    page.locator('#native-usage-refresh').scroll_into_view_if_needed()
    page.screenshot(path=str(screenshots/'agy-usage-long.png'))
    page.locator('#native-usage-refresh').click()
    expect(detail).not_to_contain_text('Delayed native quota diagnostic.')
    page.keyboard.press('Escape')
    page.locator('#sdk-input').fill('/usage')
    page.locator('#sdk-send').click()
    expect(page.locator('#sdk-input')).to_have_value('')
    expect(detail).to_be_visible()
    assert len(fixture.inputs()) == before, 'usage refresh must never submit a native turn'
    assert not errors, errors
    context.close()
    print('PASS Agy usage: direct menu, tokens, quota groups, refresh/loading/failure, slash, 3 widths, light/dark', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--frontend', type=Path)
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-agy-usage-after'))
    args = parser.parse_args()
    os.environ.setdefault('PLAYWRIGHT_BROWSERS_PATH', str(Path.home()/'.cache/ms-playwright'))
    if args.frontend:
        os.environ['SESSIONDOCK_TEST_WEB_DIR'] = str(args.frontend.resolve())
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-agy-usage-') as tmp:
        root = Path(tmp)
        fixture = AgyFixture(root, (REPO/'target/debug/ptyhost').resolve())
        with patch.dict(os.environ, fixture.env):
            initialize('--initialize-lifecycle', root/'ledger', args.binary.resolve())
            with isolated_server(Corpus(root), args.binary, host_dir=root/'host', lifecycle_dir=root/'ledger',
                    state_dir=root/'state', launcher_config=fixture.launcher, extra_env=fixture.extra_env) as (base, _), sync_playwright() as pw:
                browser = pw.chromium.launch(headless=True)
                try:
                    exercise(browser, base, fixture, args.screenshots)
                finally:
                    browser.close()


if __name__ == '__main__':
    main()
