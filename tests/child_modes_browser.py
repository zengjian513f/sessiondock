#!/usr/bin/env python3
"""Three child display modes through real list responses and Chromium clicks."""
import argparse
import json
import os
from pathlib import Path
import tempfile
from urllib.parse import quote

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, get_json, isolated_server
from nest_tree_browser import corpus


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
                    expect(page.locator('#side .item.agent, #side .nest-caret')).to_have_count(0)
                    expect(page.locator('#nest-hidden')).to_have_attribute('aria-pressed', 'true')
                    assert page.evaluate('JSON.parse(localStorage.getItem("sessiondock.childMode"))') == 'hidden'
                    # Both response shapes retain distinct conditional signatures/cache entries.
                    again = get_json(opener, base, '/api/sessions?children=hidden&sig=' + quote(hidden['sig']))
                    assert again['unchanged'] is True
                    restored = get_json(opener, base, '/api/sessions?sig=' + quote(hidden['sig']))
                    assert len(restored['sessions']) == 5 and restored['sig'] != hidden['sig']
                    page.evaluate('pollSessions()')
                    expect(page.locator('#side .item')).to_have_count(2)
                    with page.expect_response(lambda response: '/api/sessions?children=hidden' in response.url):
                        page.reload(wait_until='networkidle')
                    expect(page.locator('#side .item')).to_have_count(2)
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
                    print(f'PASS child modes {width}px: hidden payload, cache/signature, polling, persistence, filtering, flat/tree and open detail', flush=True)
            finally:
                browser.close()


if __name__ == '__main__':
    main()
