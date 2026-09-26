#!/usr/bin/env python3
"""Global actions follow the hidden session list; isolated server, fake capabilities, no CLI."""
import argparse
import os
from pathlib import Path
import re
import tempfile

from playwright.sync_api import expect, sync_playwright
from header_fold_browser import corpus, SID
from history_parity import BINARY, isolated_server


def click_action(page, selector):
    button = page.locator(selector)
    if not button.is_visible():
        page.locator('#a-more').click()
    button.click()


def dialogs(page):
    click_action(page, '#a-global-settings')
    expect(page.locator('#settings-dialog')).to_be_visible()
    page.keyboard.press('Escape')
    click_action(page, '#a-global-new-session')
    expect(page.locator('#new-session-dialog')).to_be_visible()
    page.locator('#new-cwd').fill('/synthetic/work')
    page.locator('#new-session-dialog .modal-cancel').click()
    expect(page.locator('#new-session-dialog')).not_to_be_visible()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-global-actions-') as directory:
        data = corpus(Path(directory))
        with isolated_server(data, args.binary) as (base, _), sync_playwright() as pw:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**launch)
            context = browser.new_context(viewport={'width': 1280, 'height': 900})
            context.add_init_script("Object.defineProperty(navigator, 'standalone', {value: true})")
            def document(route):
                response = route.fetch()
                body = re.sub(r'&quot;terminal_create&quot;\s*:\s*false', '&quot;terminal_create&quot;:true', response.text())
                route.fulfill(response=response, body=body)
            context.route(base + '/', document)
            context.route('**/api/term/list', lambda route: route.fulfill(json={
                'enabled': True, 'sessions': [], 'pending': [], 'sources': {'claude': True}}))
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.goto(base, wait_until='networkidle')
            page.locator(f'#side .item[data-uid="{data.uid(SID)}"]').click()
            expect(page.locator('#msgs')).to_contain_text('reply Sweep')
            expect(page.locator('#new-session')).to_be_visible()
            expect(page.locator('[id^="a-global-"]')).to_have_count(0)
            page.locator('#side-toggle').click()
            expect(page.locator('#left')).not_to_be_visible()
            expect(page.locator('#settings')).not_to_be_visible()
            expect(page.locator('[id^="a-global-"]')).to_have_count(3)
            dialogs(page)
            # Reload from the session toolbar restores the selected session and collapsed list.
            if not page.locator('#a-global-page-reload').is_visible():
                page.locator('#a-more').click()
            with page.expect_navigation(wait_until='networkidle'):
                page.locator('#a-global-page-reload').click()
            expect(page.locator('#msgs')).to_contain_text('reply Sweep')
            expect(page.locator('#left')).not_to_be_visible()
            dialogs(page)
            page.locator('#side-toggle').click()
            expect(page.locator('[id^="a-global-"]')).to_have_count(0)
            expect(page.locator('#settings')).to_be_visible()
            # Mobile detail hides the entire header; both dialogs must remain reachable.
            for width in (390, 320):
                page.set_viewport_size({'width': width, 'height': 844})
                if page.locator('#left').is_visible():
                    page.locator(f'#side .item[data-uid="{data.uid(SID)}"]').click()
                expect(page.locator('header')).not_to_be_visible()
                dialogs(page)
                assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                page.locator('.mobile-back').click()
                expect(page.locator('[id^="a-global-"]')).to_have_count(0)
                expect(page.locator('header')).to_be_visible()
                page.locator(f'#side .item[data-uid="{data.uid(SID)}"]').click()
            # Resize back to desktop restores a single set of top-level controls.
            page.set_viewport_size({'width': 1280, 'height': 900})
            expect(page.locator('[id^="a-global-"]')).to_have_count(0)
            expect(page.locator('#settings')).to_be_visible()
            assert not errors, errors
            context.close()
            browser.close()
    print('PASS session global actions: collapsed desktop, reload, mobile dialogs and resize')


if __name__ == '__main__':
    main()
