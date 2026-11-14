#!/usr/bin/env python3
"""Agy cached status and conditional token details, using a private synthetic CLI."""
from contextlib import ExitStack
from types import SimpleNamespace
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
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from sdkhost_fixtures import stop_private_sdkhosts
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
        payload=data.get('raw',data)
        payload['quota']={**quota,'captured_at':payload.get('quota',{}).get('captured_at',quota['captured_at'])}
        route.fulfill(response=response,json=data)
    context.route('**/api/native-sdk/usage',grouped_quota)
    def grouped_snapshot(route):
        response=route.fetch();data=response.json()
        data['agents']=[{**quota,'captured_at':q.get('captured_at',quota['captured_at'])} if q['source']=='agy' else q for q in data['agents']]
        route.fulfill(response=response,json=data)
    context.route('**/api/quotas/refresh*',grouped_snapshot)
    held_open=[]
    context.route('**/api/native-sdk/open',lambda route:held_open.append(route))
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    # /open is held, so selection must not wait for the native connection.
    select_sdk(page, connected=False)
    with page.expect_request('**/api/native-sdk/open'):
        page.locator('#sdk-composer .composer-run-action').click()
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
    (fixture.root/'context-usage.json').write_text(json.dumps({'estimatedTokensUsed':235426,'maxContextTokens':256000}))
    send(page,'用量下拉实际发送 synthetic turn')
    expect(page.locator('.sdk-live')).to_contain_text('Synthetic Agy reply')
    expect(footer).to_have_attribute('title',re.compile('已用 92.0%'))
    expect(footer).to_have_attribute('title',re.compile('235,426 / 256,000 tokens'))
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
    context.unroute('**/api/quotas/refresh*',grouped_snapshot)
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
    expect(footer.locator('.sdk-quota-group .sdk-gauge.low')).to_be_visible()
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
    # Native state updates independently of cumulative result.usage, including
    # compaction, missing estimates, native errors and identity mismatches.
    page.set_viewport_size({'width':1280,'height':900})
    for usage, expected in [({'estimatedTokensUsed':'12800','maxContextTokens':'256000','estimatedAfterCompaction':True},'已用 5.0%'),
                            ({'estimatedTokensUsed':0,'maxContextTokens':256000},'已用 0.0%')]:
        (fixture.root/'context-usage.json').write_text(json.dumps(usage))
        expect(footer).to_have_attribute('title',re.compile(expected),timeout=15000)
        expect(footer).to_have_attribute('title',re.compile('累计 Token：12,800'))
    for usage in ({}, {'estimatedTokensUsed':50}, {'estimatedTokensUsed':50,'maxContextTokens':0}):
        (fixture.root/'context-usage.json').write_text(json.dumps(usage))
        page.locator('#sdk-input').fill('/context');page.locator('#sdk-send').click()
        page.wait_for_function('!NativeSdkView.loadingUsage')
        expect(footer).not_to_have_attribute('title',re.compile('已用 .*tokens'))
    (fixture.root/'context-usage.json').write_text(json.dumps({'estimatedTokensUsed':64000,'maxContextTokens':256000}))
    for mode in ('reject','wrong-sid','end'):
        (fixture.root/'context-mode').write_text(mode)
        page.locator('#sdk-input').fill('/context');page.locator('#sdk-send').click()
        page.wait_for_function('!NativeSdkView.loadingUsage')
        expect(footer).to_have_attribute('title',re.compile('用量 context'))
        expect(footer).not_to_have_attribute('title',re.compile('已用 .*tokens'))
    (fixture.root/'context-mode').unlink()
    page.locator('#sdk-input').fill('/context');page.locator('#sdk-send').click()
    expect(footer).to_have_attribute('title',re.compile('已用 25.0%'))
    expect(footer).not_to_have_attribute('title',re.compile('用量 context'))
    page.reload(wait_until='domcontentloaded')
    expect(page.locator('.native-sdk-pane .sdk-status-row > .sdk-gauges')).to_have_attribute('title',re.compile('已用 25.0%'))
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
    with tempfile.TemporaryDirectory(prefix='sd-agy-usage-') as tmp, ExitStack() as cleanup:
        root = Path(tmp)
        cleanup.callback(stop_private_sdkhosts, root)
        port = free_port()
        fixture = AgyFixture(root, (REPO/'target/debug/ptyhost').resolve())
        with patch.dict(os.environ, fixture.env):
            initialize('--initialize-lifecycle', root/'ledger', args.binary.resolve())
            with isolated_server(Corpus(root), args.binary, host_dir=root/'host', lifecycle_dir=root/'ledger',
                    state_dir=root/'state', launcher_config=fixture.launcher, extra_env={**fixture.extra_env, **node_env(root,port,'127.0.0.0/8')}) as (base, _), sync_playwright() as pw:
                browser = pw.chromium.launch(headless=True)
                try:
                    exercise(browser, base, fixture, args.screenshots)
                    hub_root=root/'hub';hub_root.mkdir()
                    hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hub_root,[SimpleNamespace(name='AgyContextNode',token=TOKEN,port=port)])
                    hub.start();cleanup.callback(hub.stop)
                    page=browser.new_page(viewport={'width':1280,'height':900})
                    page.goto(f'http://127.0.0.1:{hub.port}',wait_until='domcontentloaded')
                    page.wait_for_function('T.listLoaded')
                    select_sdk(page,connected=False)
                    page.wait_for_function('NativeSdkView.pane && !NativeSdkView.historyReady')
                    if page.evaluate('NativeSdkView.readOnly'):
                        page.locator('#sdk-composer .composer-run-action').click()
                    page.wait_for_function('NativeSdkView.connected')
                    assert '~' in page.evaluate('S.sel')
                    footer=page.locator('.native-sdk-pane .sdk-gauges')
                    expect(footer).to_have_attribute('title',re.compile('已用 25.0%'),timeout=15000)
                    before=len(fixture.inputs())
                    page.locator('#sdk-input').fill('/context');page.locator('#sdk-send').click()
                    page.wait_for_function('!NativeSdkView.loadingUsage')
                    assert len(fixture.inputs())==before
                    send(page,'hold:context')
                    page.wait_for_function('NativeSdkView.running')
                    (fixture.root/'context-usage.json').write_text(json.dumps({'estimatedTokensUsed':128000,'maxContextTokens':256000}))
                    expect(footer).to_have_attribute('title',re.compile('已用 50.0%'),timeout=15000)
                    for width,theme in ((1280,'light'),(390,'dark')):
                        page.set_viewport_size({'width':width,'height':900});page.emulate_media(color_scheme=theme)
                        if not page.locator('#sdk-input').is_visible():
                            page.locator('#side .item[data-uid="'+page.evaluate('S.sel')+'"]').click()
                        page.screenshot(path=str(args.screenshots/f'agy-context-hub-{width}-{theme}.png'),full_page=True)
                    page.locator('#sdk-stop').click()
                    page.wait_for_function('!NativeSdkView.running')
                    page.reload(wait_until='domcontentloaded')
                    expect(page.locator('.native-sdk-pane .sdk-gauges')).to_have_attribute('title',re.compile('已用 50.0%'),timeout=15000)
                    page.close()
                    print('PASS Agy context through Hub: busy refresh, slash, cancellation and reload',flush=True)
                finally:
                    browser.close()


if __name__ == '__main__':
    main()
