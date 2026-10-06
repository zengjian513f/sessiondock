#!/usr/bin/env python3
"""Three child display modes through real list responses and Chromium clicks."""
import argparse
import json
import os
from pathlib import Path
import tempfile
from urllib.parse import quote

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, get_json, isolated_server
from nest_tree_browser import corpus


def sidebar_layout(page):
    return page.evaluate("""async () => {
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        return ['.side-search', '#side', '#detail'].map(selector => {
            const rect = document.querySelector(selector).getBoundingClientRect();
            return [rect.top, rect.height];
        });
    }""")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-child-modes-') as temp, sync_playwright() as pw:
        data = corpus(Path(temp))
        with isolated_server(data, args.binary, state_dir=data.root / 'state') as (base, opener):
            full = get_json(opener, base, '/api/sessions')
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**launch)
            try:
                for width in (1400, 390):
                    context = browser.new_context(viewport={'width': width, 'height': 900}, service_workers='block')
                    page = context.new_page()
                    errors = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    page.goto(base, wait_until='networkidle')
                    expect(page.locator('#side .item')).to_have_count(7)
                    page.locator('#nest-toggle').click()
                    expect(page.locator('#side .item[data-depth="2"]')).to_have_count(1)
                    with page.expect_response(lambda response: '/api/sessions?children=hidden' in response.url) as request:
                        page.locator('#nest-hidden').click()
                    hidden = request.value.json()
                    assert {row['sid'] for row in hidden['sessions']} == {'nest-root-a', 'nest-alone-e'}, hidden
                    assert all('agent_items' not in row for row in hidden['sessions'])
                    assert len(json.dumps(hidden)) < len(json.dumps(full))
                    expect(page.locator('#side .item')).to_have_count(2)
                    expect(page.locator('#side .item.agent')).to_have_count(0)
                    expect(page.locator('#side .nest-caret')).to_have_count(1)
                    expect(page.locator('#nest-hidden')).to_have_attribute('aria-pressed', 'true')
                    assert page.evaluate('JSON.parse(localStorage.getItem("sessiondock.childMode"))') == 'hidden'
                    # Both response shapes retain distinct conditional signatures/cache entries.
                    again = get_json(opener, base, '/api/sessions?children=hidden&sig=' + quote(hidden['sig']))
                    assert again['unchanged'] is True
                    restored = get_json(opener, base, '/api/sessions?sig=' + quote(hidden['sig']))
                    assert len(restored['sessions']) == 5 and restored['sig'] != hidden['sig']
                    root_uid = next(row['uid'] for row in hidden['sessions'] if row['sid'] == 'nest-root-a')
                    root_caret = page.locator(f'#side .item[data-uid="{root_uid}"] .nest-caret')
                    expect(root_caret).to_have_attribute('aria-expanded', 'false')
                    assert next(row for row in hidden['sessions'] if row['uid'] == root_uid)['child_count'] == 3
                    with page.expect_response(lambda response: '/api/sessions?' in response.url and 'expanded=' in response.url) as branch:
                        root_caret.click()
                    first_level = branch.value.json()
                    assert {row['sid'] for row in first_level['sessions']} == {'nest-root-a', 'nest-alone-e', 'nest-child-b'}
                    assert all('agent_items' not in row for row in first_level['sessions'] if row['sid'] != 'nest-root-a')
                    expect(page.locator('#side .item')).to_have_count(5)
                    child_uid = next(row['uid'] for row in first_level['sessions'] if row['sid'] == 'nest-child-b')
                    child_caret = page.locator(f'#side .item[data-uid="{child_uid}"] .nest-caret')
                    expect(child_caret).to_have_attribute('aria-expanded', 'false')
                    child_caret.click()
                    expect(page.locator('#side .item[data-depth="2"]')).to_have_count(1)
                    expect(page.locator('#side .item')).to_have_count(6)
                    page.evaluate('loadSessions(true)')
                    expect(page.locator('#side .item')).to_have_count(6)
                    # Removing an open ancestor must not promote stale grandchildren to roots.
                    root_file = data.paths['nest-root-a']
                    root_bytes = root_file.read_bytes()
                    try:
                        root_file.unlink()
                        page.evaluate('loadSessions(true)')
                        expect(page.locator(f'#side .item[data-uid="{child_uid}"]')).to_have_count(0)
                        expect(page.locator('#side .item[data-depth="2"]')).to_have_count(0)
                        assert page.evaluate('S.lazyOpen.size') == 0
                    finally:
                        root_file.write_bytes(root_bytes)
                    page.evaluate('loadSessions(true)')
                    root_caret.click()
                    expect(page.locator('#side .item')).to_have_count(5)
                    root_caret.click()
                    expect(page.locator('#side .item')).to_have_count(2)
                    page.wait_for_function('!sessionLoadActive')
                    assert page.evaluate('S.lazyOpen.size') == 0
                    assert page.evaluate('S.sessions.every(s => !s.agent_items)')
                    root_caret.click()
                    expect(page.locator('#side .item')).to_have_count(5)
                    # Open branches are transient: reloading starts with main rows only.
                    with page.expect_response(lambda response: '/api/sessions?children=hidden' in response.url):
                        page.reload(wait_until='networkidle')
                    expect(page.locator('#side .item')).to_have_count(2)
                    # A collapsed branch may finish after a newer compact response. It must not reopen.
                    pending = []
                    def delay_branch(route):
                        if 'expanded=' in route.request.url:
                            pending.append(route)
                        else:
                            route.continue_()
                    stable_layout = sidebar_layout(page)
                    page.route('**/api/sessions?*', delay_branch)
                    root_caret.click()
                    page.wait_for_function('document.querySelector(".nest-caret[aria-busy=true]")')
                    assert pending
                    expect(page.locator('#stat')).to_be_empty()
                    assert sidebar_layout(page) == stable_layout
                    root_caret.click()
                    page.wait_for_function('!sessionLoadActive')
                    response = pending[0].fetch()
                    pending[0].fulfill(response=response)
                    page.unroute('**/api/sessions?*', delay_branch)
                    expect(page.locator('#side .item')).to_have_count(2)
                    assert page.evaluate('S.lazyOpen.size') == 0
                    expect(page.locator('#stat')).to_be_empty()
                    assert sidebar_layout(page) == stable_layout
                    # Failed expansion retains the main rows and a usable retry arrow.
                    def fail_branch(route):
                        if 'expanded=' in route.request.url:
                            route.fulfill(status=503, json={'error': 'synthetic unavailable'})
                        else:
                            route.continue_()
                    page.route('**/api/sessions?*', fail_branch)
                    root_caret.click()
                    expect(page.locator('.app-popup-message')).to_have_text('子会话加载失败，请点击箭头重试')
                    expect(page.locator('#stat')).to_be_empty()
                    assert sidebar_layout(page) == stable_layout
                    expect(page.locator('#side .item')).to_have_count(2)
                    expect(root_caret).to_have_attribute('aria-expanded', 'false')
                    page.get_by_role('button', name='知道了', exact=True).click()
                    page.unroute('**/api/sessions?*', fail_branch)
                    root_caret.click()
                    expect(page.locator('#side .item')).to_have_count(5)
                    root_caret.click()
                    expect(page.locator('#side .item')).to_have_count(2)
                    page.wait_for_function('!sessionLoadActive')
                    # Hidden children stay hidden during filtering and after a list refresh.
                    page.locator('#q').fill('Agent X')
                    expect(page.locator('#side .item')).to_have_count(0)
                    page.locator('#q').fill('')
                    page.evaluate('loadSessions(true)')
                    expect(page.locator('#side .item')).to_have_count(2)
                    page.locator('#nest-flat').click()
                    expect(page.locator('#side .item')).to_have_count(7)
                    expect(page.locator('#side .item[data-uid][data-depth="0"]')).to_have_count(5)
                    expect(page.locator('#side .item.agent')).to_have_count(2)
                    # Switching modes does not lose the selection/detail view.
                    page.locator('#side .item.agent[data-agent="x"]').click()
                    page.wait_for_function('S.agent === "x" && document.querySelector("#msgs .msg")')
                    if width == 390:
                        page.locator('.mobile-back').click()
                    page.locator('#nest-hidden').click()
                    expect(page.locator('#side .item')).to_have_count(2)
                    assert page.evaluate('S.agent') == 'x'
                    page.locator('#nest-toggle').click()
                    expect(page.locator('#side .item')).to_have_count(7)
                    expect(page.locator('#side .item[data-depth="2"]')).to_have_count(1)
                    assert page.evaluate('S.agent') == 'x'
                    assert not errors, errors
                    context.close()
                    print(f'PASS child modes {width}px: lazy arrows, one-level requests, collapse/release, stale-response race, stable layout during loading/failure, refresh, flat/tree and open detail', flush=True)
            finally:
                browser.close()


if __name__ == '__main__':
    main()
