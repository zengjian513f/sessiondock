#!/usr/bin/env python3
"""Droid tool arguments/output: wrapped defaults, scrollable tail and full expansion."""
import argparse
import json
import re
import tempfile
from pathlib import Path
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from droid_sdk_fixtures import DroidFixture
from history_fixtures import BINARY
from bubble_scroll_fixtures import check_scroll_inset
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN, free_port, node_env


def check(browser, base, label, shots):
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme, has_touch=True)
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
        assert tool.evaluate('(e) => getComputedStyle(e).outlineStyle') == 'none'
        border = tool.evaluate('(e) => getComputedStyle(e).borderColor')
        tool.locator('.tool-head').hover()
        assert tool.evaluate('(e) => getComputedStyle(e).borderColor') == border
        output = tool.locator('.tool-out')
        output.scroll_into_view_if_needed()
        expect(output).to_contain_text('LAST RESULT')
        page.wait_for_function("""() => {
          const p = document.querySelector('.tool-out');
          return p && p.clientHeight > 0 && p.scrollHeight > p.clientHeight
            && p.scrollHeight - p.clientHeight - p.scrollTop < 2;
        }""")
        assert output.evaluate('(e) => e.scrollWidth <= e.clientWidth + 1')
        check_scroll_inset(output)
        page.screenshot(path=str(shots / f'{label}-{width}-{theme}-collapsed.png'), full_page=True)
        outer = page.locator('.sdk-msgs')
        tail = output.evaluate('(e) => e.scrollTop')
        outer_before = outer.evaluate('(e) => e.scrollTop')
        output.hover()
        page.mouse.wheel(0, -160)
        page.wait_for_function('before => document.querySelector(".sdk-msgs").scrollTop < before', arg=outer_before)
        assert output.evaluate('(e) => e.scrollTop') == tail
        output.scroll_into_view_if_needed()

        def swipe(body, dx, dy):
            box = body.bounding_box()
            cdp = context.new_cdp_session(page)
            x, y = box['x'] + box['width'] * .6, box['y'] + 25
            cdp.send('Input.dispatchTouchEvent', {'type': 'touchStart', 'touchPoints': [{'x': x, 'y': y}]})
            for step in range(1, 7):
                cdp.send('Input.dispatchTouchEvent', {'type': 'touchMove',
                    'touchPoints': [{'x': x + step * dx, 'y': y + step * dy}]})
                page.wait_for_timeout(20)
            cdp.send('Input.dispatchTouchEvent', {'type': 'touchEnd', 'touchPoints': []})
            page.wait_for_timeout(300)
            cdp.detach()

        outer_before = outer.evaluate('(e) => e.scrollTop')
        swipe(output, 0, 12)
        assert output.evaluate('(e) => e.scrollTop') == tail
        assert outer.evaluate('(e) => e.scrollTop') < outer_before
        output.scroll_into_view_if_needed()
        output.click(position={'x': 40, 'y': 40})
        expect(tool).to_have_class(re.compile('text-scroll-active'))
        assert tool.evaluate('(e) => getComputedStyle(e).outlineStyle') == 'none'
        assert output.evaluate('(e) => !e.isContentEditable && e.contains(getSelection().anchorNode)')
        before = output.inner_text()
        page.keyboard.type('do-not-edit')
        page.keyboard.press('Backspace')
        assert output.inner_text() == before
        swipe(output, 0, 12)
        assert output.evaluate('(e) => e.scrollTop') < tail
        output.hover()
        page.mouse.wheel(0, -100000)
        page.wait_for_function("document.querySelector('.tool-out').scrollTop === 0")
        # A real text selection remains copyable and retains nested-scroll ownership.
        output.dblclick(position={'x': 40, 'y': 20})
        assert page.evaluate('getSelection().toString().length > 0')
        page.screenshot(path=str(shots / f'{label}-{width}-{theme}-selected.png'), full_page=True)
        page.locator('#sdk-input').click()
        expect(tool).not_to_have_class(re.compile('text-scroll-active'))
        output.scroll_into_view_if_needed()
        output.hover()
        page.mouse.wheel(0, 120)
        page.wait_for_timeout(150)
        assert output.evaluate('(e) => e.scrollTop') == 0
        expect(output).to_contain_text('FIRST LINE')
        tool.get_by_role('button', name='展开全文', exact=True).click()
        assert output.evaluate('(e) => e.scrollHeight <= e.clientHeight + 1')
        check_scroll_inset(output)
        page.screenshot(path=str(shots / f'{label}-{width}-{theme}-expanded.png'), full_page=True)
        tool.get_by_role('button', name='收起', exact=True).click()
        page.wait_for_function("""() => { const p=document.querySelector('.tool-out');
          return p.scrollHeight - p.clientHeight - p.scrollTop < 2; }""")
        wrap = tool.locator('.tool-wrap').first
        expect(wrap).to_have_attribute('aria-pressed', 'true')
        wrap.click()
        expect(wrap).to_have_attribute('aria-pressed', 'false')
        assert args.evaluate('(e) => e.scrollWidth > e.clientWidth')
        expect(tool).not_to_have_class(re.compile('text-scroll-active'))
        args.hover()
        page.mouse.wheel(160, 0)
        page.wait_for_function('e => e.scrollLeft > 0', arg=args.element_handle())
        wrap.click()
        tool.locator('.tool-wrap').nth(1).click()
        output.scroll_into_view_if_needed()
        expect(tool).not_to_have_class(re.compile('text-scroll-active'))
        assert output.evaluate('e => e.scrollWidth > e.clientWidth')
        before = (output.evaluate('e => e.scrollTop'), outer.evaluate('e => e.scrollTop'))
        page.screenshot(path=str(shots / f'{label}-{width}-{theme}-horizontal-start.png'), full_page=True)
        swipe(output, -12, 0)
        assert output.evaluate('e => e.scrollLeft') > 0
        assert (output.evaluate('e => e.scrollTop'), outer.evaluate('e => e.scrollTop')) == before
        page.screenshot(path=str(shots / f'{label}-{width}-{theme}-horizontal-end.png'), full_page=True)
        context.close()
        print(f'PASS {label} {width}/{theme}: tool tail, selection, expansion, unselected horizontal wheel/swipe and vertical list scroll', flush=True)


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
