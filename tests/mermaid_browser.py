#!/usr/bin/env python3
"""Render native Agy Mermaid fences in conversation and SDK history on node and Hub."""
import argparse
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from agy_sdk_fixtures import SID, TITLE
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_start_stop_fixtures import provider


HORIZONTAL = '''flowchart LR
    subgraph morning["09:31 盘前特征缓存"]
        input["输入特征 [B, 61]（含中文说明）"] --> encode["编码 Linear(61->48) + SiLU"]
        input --> GRU["时序状态 GRUCell(48, 32)"]
        input --> pool["池化特征 Conv1d(48, 32)"]
        encode --> cache["缓存特征 [B, 32]"]
        GRU --> cache
        pool --> cache
    end
    subgraph trading["09:32 盘中预测"]
        window["当日窗口 [N, 29, 64]"] --> seed0["Seed 0 盘中主干"]
        window --> seed1["Seed 1 盘中主干"]
        window --> seed2["Seed 2 盘中主干"]
        seed0 --> pred["预测输出 [B, 3]"]
        seed1 --> pred
        seed2 --> pred
    end
    cache --> seed0 & seed1 & seed2'''
VERTICAL = '''flowchart TD
    input["原始输入"] --> join["通道拼接"]
    join --> out["输出三头预测"]'''
INVALID = 'flowchart TD\n    input --> ['
BODY = (f'## 模型结构图\n\n盘前特征缓存。\n\n```mermaid\n{HORIZONTAL}\n```\n\n'
        f'## 原结构\n\n```mermaid\n{VERTICAL}\n```\n\n'
        f'## 不完整的定义\n\n```mermaid\n{INVALID}\n```\n\n'
        f'```text\n{VERTICAL}\n```')
ASSET = '**/vendor/mermaid/mermaid.tiny.js*'


def check(page, base, label, width, theme, screenshots):
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='domcontentloaded')
    expect(page.locator('html')).to_have_attribute('data-theme', theme)
    page.locator('#side .item').filter(has_text=TITLE).click()
    mouse = page.context.new_cdp_session(page)
    for view, selector in (('chat', '#msgs'), ('sdk', '.sdk-history')):
        page.locator('#a-' + view).click()
        messages = page.locator(selector)
        horizontal, vertical, invalid = (messages.locator('sd-mermaid').nth(i) for i in range(3))
        horizontal.scroll_into_view_if_needed()
        diagram = horizontal.frame_locator('iframe').locator('svg').first
        expect(diagram).to_be_visible()
        expect(diagram).to_contain_text('缓存特征')
        expect(diagram).to_contain_text('Linear(61->48)')
        assert diagram.locator('.cluster').count() == 2
        assert diagram.evaluate('el => getComputedStyle(el.ownerDocument.documentElement).backgroundColor') == (
            horizontal.evaluate('el => getComputedStyle(el).backgroundColor'))
        preview = horizontal.locator('.mermaid-preview')
        fit = horizontal.get_by_role('button', name='适应窗口')
        fitted = fit.inner_text()
        assert preview.evaluate('el => el.clientHeight <= 220 && el.scrollWidth === el.clientWidth')
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-{view}-{width}-{theme}.png'))
        for _ in range(4):
            horizontal.get_by_role('button', name='放大图表').click()
        assert fit.inner_text() != fitted
        assert preview.evaluate('el => el.scrollWidth > el.clientWidth')
        preview.hover(position={'x': 40, 'y': 35})
        before = preview.evaluate('el => el.scrollLeft')
        page.mouse.wheel(-200, 0)
        page.wait_for_function('([el, left]) => el.scrollLeft < left', arg=[preview.element_handle(), before])
        horizontal.get_by_role('button', name='缩小图表').click()
        if screenshots and width == 390 and theme == 'dark':
            page.screenshot(path=str(screenshots / f'{label}-{view}-zoomed.png'))
        fit.click()
        expect(fit).to_have_text(fitted)
        assert preview.evaluate('el => el.scrollLeft === 0 && el.scrollTop === 0')
        horizontal.get_by_role('button', name='源码', exact=True).click()
        expect(preview).to_be_hidden()
        code = horizontal.locator('code')
        expect(code).to_be_visible()
        assert code.text_content() == HORIZONTAL
        horizontal.get_by_role('button', name='图形', exact=True).click()
        expect(diagram).to_be_visible()
        vertical.scroll_into_view_if_needed()
        frame = vertical.frame_locator('iframe')
        label_text = frame.locator('foreignObject p').filter(has_text='通道拼接')
        expect(label_text).to_be_visible()
        bounds = label_text.evaluate('text => text.getBoundingClientRect().toJSON()')
        frame_bounds = vertical.locator('iframe').bounding_box()
        x = frame_bounds['x'] + bounds['x']
        y = frame_bounds['y'] + bounds['y'] + bounds['height'] / 2
        # Chromium's native input dispatch keeps the selection drag inside the
        # sandboxed frame instead of using Playwright's cross-frame DnD interception.
        for action, delta, buttons in (('mousePressed', .5, 1),
                                        ('mouseMoved', bounds['width'] - .5, 1),
                                        ('mouseReleased', bounds['width'] - .5, 0)):
            mouse.send('Input.dispatchMouseEvent', {
                'type': action, 'x': x + delta, 'y': y, 'button': 'left',
                'buttons': buttons, 'clickCount': 1,
            })
        assert '通道拼接' in label_text.evaluate('el => el.ownerDocument.getSelection().toString()')
        invalid.scroll_into_view_if_needed()
        expect(invalid.get_by_role('status')).to_have_text('Mermaid 源码')
        expect(invalid.locator('code')).to_be_visible()
        expect(invalid.locator('iframe')).to_have_count(0)
        assert invalid.locator('code').text_content() == INVALID
        expect(messages.locator('pre > code[data-code-lang="text"]')).to_contain_text('通道拼接')
    assert not errors, errors
    print(f'PASS Mermaid {label} {width}/{theme}: Agy conversation and SDK history, fit, zoom, pan, text selection', flush=True)


def check_fallback_and_search(browser, base, screenshots):
    page = browser.new_page(viewport={'width': 1280, 'height': 900}, color_scheme='dark')
    try:
        requests = []
        page.on('request', lambda request: requests.append(request.url))
        page.goto(base)
        expect(page.locator('#side .item').filter(has_text=TITLE)).to_be_visible()
        assert not any('/vendor/mermaid/' in url for url in requests)
        page.route(ASSET, lambda route: route.abort())
        with page.expect_event('requestfailed', predicate=lambda request: '/vendor/mermaid/' in request.url):
            page.locator('#side .item').filter(has_text=TITLE).click()
            page.locator('#a-chat').click()
        chart = page.locator('#msgs sd-mermaid').first
        chart.scroll_into_view_if_needed()
        expect(chart.locator('code')).to_be_visible()
        assert chart.locator('code').text_content() == HORIZONTAL
        if screenshots:
            page.screenshot(path=str(screenshots / 'node-chat-1280-dark-fallback.png'))
        page.unroute(ASSET)
        page.reload()
        page.locator('#a-chat').click()
        chart.scroll_into_view_if_needed()
        expect(chart.frame_locator('iframe').locator('svg').first).to_be_visible()
        page.locator('#q').fill('GRUCell')
        page.locator('#q').press('Enter')
        expect(chart.locator('mark')).to_have_text('GRUCell', ignore_case=True)
        expect(chart.locator('mark')).to_be_visible()
        chart.get_by_role('button', name='图形', exact=True).click()
        expect(chart.locator('iframe')).to_be_visible()
        page.locator('#q').press('Escape')
        expect(chart.locator('iframe')).to_be_visible()
        initial = chart.locator('iframe').get_attribute('srcdoc')
        page.emulate_media(color_scheme='light')
        expect(page.locator('html')).to_have_attribute('data-theme', 'light')
        expect(chart.locator('iframe')).not_to_have_attribute('srcdoc', initial)
        frame_html = chart.frame_locator('iframe').locator('html')
        expect(frame_html).to_have_css('background-color', chart.evaluate('el => getComputedStyle(el).backgroundColor'))
    finally:
        page.close()
    print('PASS Mermaid lazy loading, asset failure, reload, search and theme switch', flush=True)


def check_scroll_stability(browser, base, screenshots):
    page = browser.new_page(viewport={'width': 1280, 'height': 680}, color_scheme='dark')
    try:
        held = []
        page.route(ASSET, lambda route: held.append(route))
        page.goto(base)
        with page.expect_request(ASSET):
            page.locator('#side .item').filter(has_text=TITLE).click()
            page.locator('#a-chat').click()
        box = page.locator('#msgs')
        first = box.locator('sd-mermaid').first
        expect(first.get_by_role('status')).to_have_text('正在绘制图表…')
        box.hover(position={'x': 4, 'y': 100})
        page.mouse.wheel(0, -2000)
        page.wait_for_function('el => el.scrollTop === 0', arg=box.element_handle())
        position = '''box => [
          box.scrollTop, box.scrollHeight,
          ...Array.from(box.querySelectorAll('sd-mermaid'), el => [el.offsetHeight, el.getBoundingClientRect().top]),
        ]'''
        before = box.evaluate(position)
        if screenshots:
            page.screenshot(path=str(screenshots / 'scroll-before.png'))
        for route in held:
            route.continue_()
        expect(first.frame_locator('iframe').locator('svg').first).to_be_visible()
        expect(box.locator('sd-mermaid[data-ready]')).to_have_count(2)
        assert box.evaluate(position) == before
        first.get_by_role('button', name='放大图表').click()
        assert box.evaluate(position) == before
        if screenshots:
            page.screenshot(path=str(screenshots / 'scroll-after.png'))
    finally:
        page.close()
    print('PASS Mermaid reserves its size before loading and preserves the conversation scroll position', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-mermaid-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        options = {'headless': True}
        if executable := os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = executable
        browser = pw.chromium.launch(**options)
        with provider('agy', root, BINARY) as (base, peer, env):
            transcript = peer.native / 'brain' / SID / '.system_generated/logs/transcript_full.jsonl'
            user, assistant = map(json.loads, transcript.read_text().splitlines())
            assistant['content'] = BODY
            transcript.write_text('\n'.join(json.dumps(row, ensure_ascii=False) for row in (user, assistant)) + '\n')
            hubroot = root / 'hub'
            hubroot.mkdir()
            node_port = int(env['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1])
            hub = Hub(BINARY.with_name('sessiondock-hub'), hubroot,
                      [SimpleNamespace(name='MermaidNode', token=TOKEN, port=node_port)])
            hub.start()
            try:
                for endpoint, label in ((base, 'node'), (f'http://127.0.0.1:{hub.port}', 'hub')):
                    for width in (1280, 390):
                        for theme in ('light', 'dark'):
                            page = browser.new_page(viewport={'width': width, 'height': 900}, color_scheme=theme)
                            try:
                                check(page, endpoint, label, width, theme, args.screenshots)
                            finally:
                                page.close()
                check_fallback_and_search(browser, base, args.screenshots)
                check_scroll_stability(browser, base, args.screenshots)
            finally:
                hub.stop()
                browser.close()


if __name__ == '__main__':
    main()
