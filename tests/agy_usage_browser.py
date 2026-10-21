#!/usr/bin/env python3
"""Agy cached status and conditional token details, using a private synthetic CLI."""
import argparse
import json
import os
import re
from pathlib import Path
import tempfile
from unittest.mock import patch

from playwright.sync_api import expect, sync_playwright
from agy_sdk_browser import select_sdk, send
from agy_sdk_fixtures import AgyFixture
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from send_browser import initialize
from sdk_usage_fixtures import assert_inline_usage, assert_usage_outside_menu, refresh_usage


def exercise(browser, base, fixture, screenshots):
    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(10000)
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    buckets = [
        {'id':'gemini-weekly','group':'Gemini Models','window':'weekly','remaining_percent':68.25167536735535,'resets_at':'2026-10-23T22:06:57Z'},
        {'id':'gemini-5h','group':'Gemini Models','window':'5h','remaining_percent':99.7},
        {'id':'3p-weekly','group':'Claude and GPT models','window':'seven_day','remaining_percent':53.429800271987915},
        {'id':'3p-5h','group':'Claude and GPT models','window':'5h','remaining_percent':100}]
    quota={'source':'agy','captured_at':'2026-10-19T10:00:00Z','buckets':buckets}
    context.route('**/api/quotas',lambda route:route.fulfill(json={'agents':[quota]}))
    def grouped_quota(route):
        response=route.fetch();data=response.json()
        data.get('raw',data)['quota']=quota
        route.fulfill(response=response,json=data)
    context.route('**/api/native-sdk/usage',grouped_quota)
    held_open=[]
    context.route('**/api/native-sdk/open',lambda route:held_open.append(route))
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    # /open is held, so selection must not wait for the native connection.
    select_sdk(page, connected=False)
    footer=page.locator('.native-sdk-pane .sdk-status-row > .sdk-gauges')
    expect(footer.locator('.sdk-quota-name')).to_have_text(['Gemini','Claude/GPT'])
    assert held_open,'cold quota must render while native open is blocked'
    for route in held_open:route.continue_()
    context.unroute('**/api/native-sdk/open')
    page.wait_for_function('NativeSdkView.connected && !NativeSdkView.loadingUsage && !!NativeSdkView.raw.session_id')
    # Screenshot 155 combination: no token data, four account windows, native
    # model/mode not yet returned. The menu must not duplicate quota or an empty card.
    if page.locator('#sdk-more').is_visible():page.locator('#sdk-more').click()
    expect(page.locator('#sdk-more-menu .sdk-quota-group')).to_have_count(0)
    assert_usage_outside_menu(page)
    page.screenshot(path=str(screenshots/'agy-no-tokens-four-windows-1280-light.png'),full_page=True)
    page.keyboard.press('Escape')
    page.set_viewport_size({'width':390,'height':900})
    page.evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))')
    if not page.locator('#sdk-input').is_visible():
        selected=page.evaluate('S.sel');page.locator(f'#side .item[data-uid="{selected}"]').click()
    for theme in ('light','dark'):
        page.emulate_media(color_scheme=theme)
        if page.locator('#sdk-more').is_visible():page.locator('#sdk-more').click()
        assert_usage_outside_menu(page)
        page.screenshot(path=str(screenshots/f'agy-no-tokens-four-windows-390-{theme}.png'),full_page=True)
        page.keyboard.press('Escape')
    page.set_viewport_size({'width':1280,'height':900})
    page.emulate_media(color_scheme='light')
    send(page,'用量下拉实际发送 synthetic turn')
    expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply')
    page.locator('#sdk-input').fill('保留草稿 retained draft')
    for width in (1280,390,320):
        page.set_viewport_size({'width':width,'height':900})
        page.evaluate('new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))')
        if width<500 and not page.locator('#sdk-input').is_visible():
            selected=page.evaluate('S.sel')
            page.locator(f'#side .item[data-uid="{selected}"]').click()
        for theme in ('light','dark'):
            print(f'Agy usage: {width}px {theme}',flush=True)
            page.emulate_media(color_scheme=theme)
            # Token totals live in the footer description, never in ⋯.
            for text in ('累计 Token：12,800','输入 Token：12,000','输出 Token：800','缓存读取：9,000'):
                expect(footer).to_have_attribute('title',re.compile(re.escape(text)))
            assert_usage_outside_menu(page)
            expect(footer.locator('.sdk-quota-name')).to_have_text(['Gemini','Claude/GPT'])
            expect(footer.locator('.sdk-quota-windows .sdk-gauge > span')).to_have_text(['5小时','7天','5小时','7天'])
            for raw in ('weekly','seven_day','7d'):expect(footer).not_to_contain_text(raw)
            expect(footer).to_have_attribute('title',re.compile('剩余 68.3%'))
            if page.locator('#sdk-more').is_visible():
                page.locator('#sdk-more').click()
                menu=page.locator('#sdk-more-menu');bounds=menu.bounding_box()
                assert bounds['x']>=0 and bounds['y']>=0 and bounds['x']+bounds['width']<=width+1,bounds
                assert menu.evaluate('n=>n.scrollWidth<=n.clientWidth+1')
                expect(menu).not_to_contain_text('12,800')
            expect(page.locator('#native-sdk-mode')).to_be_visible()
            page.screenshot(path=str(screenshots/f'agy-usage-{width}-{theme}.png'),full_page=True)
            page.keyboard.press('Escape')
            assert_inline_usage(page)
            page.screenshot(path=str(screenshots/f'agy-footer-{width}-{theme}.png'),full_page=True)
            expect(page.locator('#sdk-input')).to_have_value('保留草稿 retained draft')
    context.unroute('**/api/native-sdk/usage',grouped_quota)
    before=len(fixture.inputs())
    (fixture.root/'quota-hold').touch()
    refresh_usage(page)
    page.wait_for_function('NativeSdkView.loadingUsage')
    expect(footer).to_have_attribute('title',re.compile('剩余 68.3%'))
    expect(footer).to_have_attribute('title',re.compile('累计 Token：12,800'))
    page.screenshot(path=str(screenshots/'agy-usage-loading.png'),full_page=True)
    (fixture.root/'quota-remaining').write_text('0.09')
    (fixture.root/'quota-hold').unlink()
    expect(footer).to_have_attribute('title',re.compile('剩余 9%'))
    expect(footer.locator('.sdk-gauge.low')).to_be_visible()
    page.wait_for_function('!NativeSdkView.loadingUsage')
    (fixture.root/'quota-fail').touch()
    refresh_usage(page)
    expect(footer).to_have_attribute('title',re.compile('查询失败'))
    expect(footer).to_have_attribute('title',re.compile('剩余 9%'))
    page.screenshot(path=str(screenshots/'agy-usage-failed.png'),full_page=True)
    (fixture.root/'quota-fail').unlink()
    page.wait_for_function('!NativeSdkView.loadingUsage')
    def long_quota(route):
        response=route.fetch();data=response.json();payload=data.get('raw',data)
        payload['quota']['buckets'].append({'group':'Claude / GPT Enterprise shared premium 模型额度长名称','window':'weekly','remaining_percent':0})
        payload['quota']['buckets'].append({'group':'缺失额度字段','window':'5h'})
        payload['quota']['error']='Delayed native quota diagnostic. '*80
        route.fulfill(response=response,json=data)
    context.route('**/api/native-sdk/usage',long_quota,times=1)
    refresh_usage(page)
    expect(footer).to_have_attribute('title',re.compile('Enterprise shared premium'))
    expect(footer).not_to_contain_text('缺失额度字段')
    expect(footer).to_contain_text('100%')
    # The long diagnostic stays in the footer description and never opens ⋯.
    expect(footer).to_have_attribute('title',re.compile('Delayed native quota diagnostic.'))
    expect(page.locator('#sdk-more-menu')).to_be_hidden()
    assert_inline_usage(page)
    page.screenshot(path=str(screenshots/'agy-usage-long.png'),full_page=True)
    page.wait_for_function('!NativeSdkView.loadingUsage')
    refresh_usage(page)
    expect(footer).not_to_have_attribute('title',re.compile('Delayed native quota diagnostic.'))
    assert len(fixture.inputs())==before,'usage refresh must never submit a native turn'
    assert not errors, errors
    context.close()
    print('PASS Agy usage: conditional token details, inline quota, cached refresh/loading/failure, immediate slash, 3 widths, light/dark', flush=True)


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
