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
    input["输入特征 [B, 61]（含中文说明）"] --> encode["编码 Linear(61->48) + SiLU"]
    encode --> GRU["时序状态 GRUCell(48, 32)"]
    GRU --> cache["缓存特征 [B, 32]"]
    cache --> pred["预测输出 [B, 3]"]'''
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
    for view, selector in (('chat', '#msgs'), ('sdk', '.sdk-history')):
        page.locator('#a-' + view).click()
        messages = page.locator(selector)
        horizontal, vertical, invalid = (messages.locator('sd-mermaid').nth(i) for i in range(3))
        horizontal.scroll_into_view_if_needed()
        image = horizontal.locator('img')
        expect(image).to_be_visible()
        assert image.evaluate('el => el.complete && el.naturalWidth > 300 && el.naturalHeight > 30')
        labels = image.evaluate('''el => {
          const svg = decodeURIComponent(el.src.split(',')[1]);
          return new DOMParser().parseFromString(svg, 'image/svg+xml').documentElement.textContent;
        }''')
        assert '缓存特征' in labels and 'Linear(61->48)' in labels, labels[-500:]
        preview = horizontal.locator('.mermaid-preview')
        if width == 390:
            preview.hover()
            page.mouse.wheel(200, 0)
            page.wait_for_function('el => el.scrollLeft > 0', arg=preview.element_handle())
            preview.evaluate('el => el.scrollLeft = 0')
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-{view}-{width}-{theme}.png'))
        horizontal.get_by_role('button', name='查看 Mermaid 源码').click()
        expect(image).to_be_hidden()
        code = horizontal.locator('code')
        expect(code).to_be_visible()
        assert code.text_content() == HORIZONTAL
        horizontal.get_by_role('button', name='查看 Mermaid 图形').click()
        expect(image).to_be_visible()
        vertical.scroll_into_view_if_needed()
        expect(vertical.locator('img')).to_be_visible()
        invalid.scroll_into_view_if_needed()
        expect(invalid.locator('code')).to_be_visible()
        expect(invalid.locator('img')).to_have_count(0)
        assert invalid.locator('code').text_content() == INVALID
        expect(messages.locator('pre > code[data-code-lang="text"]')).to_contain_text('通道拼接')
    assert not errors, errors
    print(f'PASS Mermaid {label} {width}/{theme}: Agy conversation and SDK history', flush=True)


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
        expect(chart.locator('img')).to_be_visible()
        page.locator('#q').fill('GRUCell')
        page.locator('#q').press('Enter')
        expect(chart.locator('mark')).to_have_text('GRUCell', ignore_case=True)
        expect(chart.locator('mark')).to_be_visible()
        page.locator('#q').press('Escape')
        expect(chart.locator('img')).to_be_visible()
        initial = chart.locator('img').get_attribute('src')
        page.emulate_media(color_scheme='light')
        expect(page.locator('html')).to_have_attribute('data-theme', 'light')
        expect(chart.locator('img')).not_to_have_attribute('src', initial)
    finally:
        page.close()
    print('PASS Mermaid lazy loading, asset failure, reload, search and theme switch', flush=True)


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
            finally:
                hub.stop()
                browser.close()


if __name__ == '__main__':
    main()
