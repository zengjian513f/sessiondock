#!/usr/bin/env python3
"""Droid tool arguments/output: wrapped defaults, scrollable tail and full expansion."""
import argparse
import json
import tempfile
from pathlib import Path
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from droid_sdk_fixtures import DroidFixture
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN, free_port, node_env


def check(browser, base, label, shots):
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
        page = context.new_page()
        page.goto(base, wait_until='networkidle')
        page.locator('#side .item[data-uid^="droid:"]').first.click()
        expect(page.get_by_role('button', name='展开本轮过程', exact=True)).to_be_visible()
        for button in page.get_by_role('button', name='展开本轮过程', exact=True).all():
            button.click()
        for button in page.get_by_role('button', name='展开工具调用组', exact=True).all():
            button.click()
        tool = page.locator('.tool-entry').filter(has_text='Script').first
        expect(tool).to_be_visible()
        args = tool.locator('.tool-args')
        if not args.is_visible():
            tool.get_by_role('button', name='展开原始参数', exact=True).click()
        expect(args).to_be_visible()
        assert args.evaluate('(e) => e.scrollWidth <= e.clientWidth + 1')
        output = tool.locator('.tool-out')
        output.scroll_into_view_if_needed()
        expect(output).to_contain_text('LAST RESULT')
        page.wait_for_function("""() => {
          const p = document.querySelector('.tool-out');
          return p && p.clientHeight > 0 && p.scrollHeight > p.clientHeight
            && p.scrollHeight - p.clientHeight - p.scrollTop < 2;
        }""")
        assert output.evaluate('(e) => e.scrollWidth <= e.clientWidth + 1')
        page.screenshot(path=str(shots / f'{label}-{width}-{theme}-collapsed.png'), full_page=True)
        output.hover()
        page.mouse.wheel(0, -100000)
        page.wait_for_function("document.querySelector('.tool-out').scrollTop === 0")
        expect(output).to_contain_text('FIRST LINE')
        tool.get_by_role('button', name='展开全文', exact=True).click()
        assert output.evaluate('(e) => e.scrollHeight <= e.clientHeight + 1')
        page.screenshot(path=str(shots / f'{label}-{width}-{theme}-expanded.png'), full_page=True)
        tool.get_by_role('button', name='收起', exact=True).click()
        page.wait_for_function("""() => { const p=document.querySelector('.tool-out');
          return p.scrollHeight - p.clientHeight - p.scrollTop < 2; }""")
        wrap = tool.locator('.tool-wrap').first
        expect(wrap).to_have_attribute('aria-pressed', 'true')
        wrap.click()
        expect(wrap).to_have_attribute('aria-pressed', 'false')
        assert args.evaluate('(e) => e.scrollWidth > e.clientWidth')
        wrap.click()
        page.screenshot(path=str(shots / f'{label}-{width}-{theme}-final.png'), full_page=True)
        context.close()
        print(f'PASS {label} {width}/{theme}: wrapped arguments/output, tail, wheel, full expansion, collapse', flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--screenshots', type=Path, required=True)
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-tool-output-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        fixture = DroidFixture(root)
        rows = list(map(json.loads, fixture.path.read_text().splitlines()))
        rows.insert(-1, {'type': 'message', 'id': 'script', 'message': {'role': 'assistant', 'content': [
            {'type': 'tool_use', 'id': 'call', 'name': 'Script', 'input': {'script': 'long_script_argument ' * 160}}]}})
        rows.insert(-1, {'type': 'message', 'id': 'result', 'message': {'role': 'user', 'content': [
            {'type': 'tool_result', 'tool_use_id': 'call', 'content':
             'FIRST LINE\n' + '\n'.join(f'output {i} ' + 'long-value ' * 60 for i in range(80)) + '\nLAST RESULT'}]}})
        fixture.path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
        port = free_port()
        with fixture.server(BINARY, node_env(root, port, '127.0.0.0/8')) as base:
            browser = pw.chromium.launch(headless=True)
            check(browser, base, 'node', args.screenshots)
            hubroot = root / 'hub'
            hubroot.mkdir()
            hub = Hub(BINARY.with_name('sessiondock-hub'), hubroot,
                      [SimpleNamespace(name='DroidNode', token=TOKEN, port=port)])
            hub.start()
            try:
                check(browser, f'http://127.0.0.1:{hub.port}', 'hub', args.screenshots)
            finally:
                hub.stop()
                browser.close()


if __name__ == '__main__':
    main()
