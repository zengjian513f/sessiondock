#!/usr/bin/env python3
"""Report entry always starts with bug, ordered before feature, retaining drafts."""
import argparse
from pathlib import Path
import re
import tempfile

from playwright.sync_api import expect, sync_playwright
from bug_report_node_browser import Boundary, open_report as open_list_report
from hub_fixtures import Hub, FakeNode, REPO


def open_report(page):
    if page.evaluate('!!S.sel'):
        button = page.locator('#detail [data-report-bug]')
        expect(button).to_have_count(1)
        if not button.is_visible():
            page.locator('#a-more').click()
        button.click()
    else:
        open_list_report(page)


def exercise(browser, base, screenshots):
    for width, theme in [(1280, 'light'), (1280, 'dark'), (390, 'light'), (390, 'dark')]:
        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme,
                                      service_workers='block')
        boundary = Boundary()
        context.route(re.compile(r'.*/api/session/conversation.*'), boundary.conversation)
        context.route(re.compile(r'.*/api/bug-report(/capture)?(\?.*)?$'), boundary.handle)
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('S.sessions.length && Nodes.list.length')
        for entry in ('list', 'session'):
            if entry == 'session':
                page.locator('#side .item').first.click()
            open_report(page)
            dialog = page.locator('#bug-report-dialog')
            bug = dialog.locator('input[value=bug]')
            feature = dialog.locator('input[value=feature]')
            expect(bug).to_be_checked()
            expect(dialog.locator('#bug-report-type label')).to_have_text(['缺陷报告', '功能需求'])
            page.wait_for_function('!bugReportDraftObject().loading')
            text = '输入 / 前有空白应作为普通文本；切换类型后重新打开报告，保留详细描述。'
            page.fill('#bug-report-description', text)
            dialog.locator('label:has(input[value=feature])').click()
            expect(feature).to_be_checked()
            dialog.locator('.modal-close').click()
            open_report(page)
            expect(bug).to_be_checked()
            expect(page.locator('#bug-report-description')).to_have_value(text)
            if screenshots:
                page.screenshot(path=str(screenshots / f'report-{entry}-{width}-{theme}.png'), full_page=True)
            # Explicitly selecting feature still submits feature on this opening.
            dialog.locator('label:has(input[value=feature])').click()
            dialog.locator('#bug-report-go').click()
            expect(dialog).not_to_be_visible()
            assert boundary.calls[-1][1]['report_type'] == 'feature', boundary.calls
            open_report(page)
            expect(bug).to_be_checked()
            page.wait_for_function('!bugReportDraftObject().loading')
            page.fill('#bug-report-description', text)
            dialog.locator('#bug-report-go').click()
            expect(dialog).not_to_be_visible()
            assert boundary.calls[-1][1]['report_type'] == 'bug', boundary.calls
        assert not errors, errors
        context.close()
        print(f'PASS report defaults: list/session {width} {theme}, reopen/draft/feature and bug submit', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--hub-binary', type=Path, default=REPO / 'target/debug/sessiondock-hub')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    node = FakeNode('a' * 32, 'NodeA')
    try:
        with tempfile.TemporaryDirectory(prefix='sd-report-defaults-') as temp:
            hub = Hub(args.hub_binary.resolve(), Path(temp), [node])
            hub.start()
            try:
                with sync_playwright() as p:
                    browser = p.chromium.launch(headless=True)
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', args.screenshots)
                    finally:
                        browser.close()
            finally:
                hub.stop()
    finally:
        node.stop()


if __name__ == '__main__':
    main()
