#!/usr/bin/env python3
"""Search exposes folded date/project groups, descendants and subagents."""
import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, isolated_server
from search_browser import corpus


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-search-no-fold-') as directory:
        data = corpus(Path(directory))
        with isolated_server(data, args.binary) as (base, opener), sync_playwright() as pw:
            sessions = json.load(opener.open(base + '/api/sessions'))
            result = json.load(opener.open(base + '/api/search?q=Needle'))
            parent_uid, child_uid = data.uid('search-main'), data.uid('codex-search')

            def enrich(rows):
                for row in rows:
                    row['updated'] = '2026-09-25T06:00:00Z'
                    row['cwd'] = '/synthetic/search'
                    if row['uid'] == parent_uid:
                        row['agent_items'] = [dict(id='worker', title='Synthetic worker', type='general',
                            created=row['updated'], updated=row['updated'])]
                    if row['uid'] == child_uid:
                        row['nest_parent'] = dict(source='claude', sid='search-main')
                return rows

            enrich(sessions['sessions'])
            enrich(result['results'])
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**launch)
            try:
                for view in ('date', 'tree'):
                    context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
                    context.route('**/api/sessions?**', lambda route: route.fulfill(json=sessions))
                    context.route('**/api/sessions', lambda route: route.fulfill(json=sessions))
                    context.route('**/api/search?**', lambda route: route.fulfill(json=result))
                    def messages(route):
                        response = route.fetch()
                        payload = response.json()
                        if payload.get('meta'):
                            enrich([payload['meta']])
                        route.fulfill(response=response, json=payload)
                    context.route('**/api/messages/**', messages)
                    context.route('**/api/watch?**', lambda route: route.abort())
                    page = context.new_page()
                    errors = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    page.goto(base, wait_until='networkidle')
                    page.locator(f'#view [data-v="{view}"]').click()
                    if not page.locator('#nest-toggle').get_attribute('aria-pressed') == 'true':
                        page.locator('#nest-toggle').click()
                    parent = page.locator(f'#side .item[data-uid="{parent_uid}"]')
                    child = page.locator(f'#side .item[data-uid="{child_uid}"]')
                    worker = page.locator('#side .item.agent[data-agent="worker"]')
                    expect(child).to_be_visible()
                    expect(worker).to_be_visible()
                    parent.locator('.nest-caret').click()
                    expect(child).to_have_count(0)
                    group = page.locator('#side > .group').first
                    group.locator('.ghead').click()
                    expect(parent).to_have_count(0)
                    saved = page.evaluate('JSON.stringify([[...S.closed], [...S.nestClosed]])')

                    # Typing filters titles and must already expose saved folds.
                    page.locator('#q').fill('Synthetic')
                    expect(parent).to_be_visible()
                    expect(worker).to_be_visible()
                    expect(page.locator('#side .group.closed')).to_have_count(0)
                    expect(page.locator('#side .nest-caret')).to_have_count(0)
                    group.locator('.ghead').click()
                    expect(parent).to_be_visible()

                    # Enter searches real fixture bodies; the matched child is
                    # visible even when its parent was previously folded.
                    old = page.locator('#stat').get_attribute('data-seq') or ''
                    page.locator('#q').fill('Needle')
                    page.locator('#q').press('Enter')
                    page.wait_for_function("old => (document.querySelector('#stat').dataset.seq || '') !== old", arg=old)
                    expect(parent).to_be_visible()
                    expect(child).to_be_visible()
                    expect(worker).to_be_visible()
                    expect(page.locator('#side .nest-caret')).to_have_count(0)
                    expect(group.locator('.caret')).to_be_hidden()
                    group.locator('.ghead').click()
                    expect(child).to_be_visible()
                    child.click()
                    expect(page.locator('#msgs')).to_contain_text('Needle from another provider')
                    assert page.evaluate('JSON.stringify([[...S.closed], [...S.nestClosed]])') == saved

                    page.locator('#side-search-exit').click()
                    expect(group).to_have_class('group closed')
                    expect(parent).to_have_count(0)
                    group.locator('.ghead').click()
                    expect(parent).to_be_visible()
                    expect(child).to_have_count(0)
                    expect(worker).to_have_count(0)
                    parent.locator('.nest-caret').click()
                    expect(child).to_be_visible()
                    expect(worker).to_be_visible()
                    assert not errors, errors
                    context.close()
                    print(f'PASS search no folding: {view}; type, submit, click result, exit restores folds', flush=True)
            finally:
                browser.close()


if __name__ == '__main__':
    main()
