#!/usr/bin/env python3
"""Droid child provenance, read-only navigation and independent forks in Chromium."""
import argparse
import tempfile
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from droid_children_fixtures import DroidChildrenFixture
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import node_env, free_port, TOKEN


def check(page, base, fixture, output, hub=False):
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='networkidle')
    fork = page.locator('#side .item').filter(has=page.locator('.t', has_text='Droid independent fork')).first
    expect(fork).to_be_visible();fork.click()
    page.locator('#a-fork-chain').click()
    page.locator('#fork-chain-menu .chain-toggle').click()
    page.keyboard.press('Escape')
    parent = page.locator('#side .item').filter(has=page.locator('.t', has_text='Droid parent session')).first
    expect(parent).to_be_visible()
    uid = parent.get_attribute('data-uid')
    parent.click()
    page.locator('#a-view-switch').click()
    child = page.locator(f'#session-view-menu button[data-agent="{fixture.child}"]')
    expect(child).to_contain_text('Droid delegated child')
    expect(page.locator(f'#session-view-menu button[data-agent="{fixture.fork}"]')).to_have_count(0)
    child.click()
    expect(page.locator('#detail')).to_contain_text('Child history answer')
    expect(page.locator('#detail')).not_to_contain_text('Parent history answer')
    expect(page.locator('#a-sdk')).to_be_hidden()
    expect(page.locator('#sdk-input')).to_have_count(0)
    page.screenshot(path=str(output / 'child-wide-light.png'), full_page=True)
    # Existing child navigation is read-only; attempting the API does not spawn a writer.
    response = page.request.post(base + '/api/native-sdk/open', data={'uid': uid, 'agent': fixture.child})
    assert not response.ok and 'subagent' in response.text(), response.text()
    page.locator('#a-view-switch').click()
    page.locator('#session-view-menu button[data-agent=""]').click()
    expect(page.locator('#detail')).to_contain_text('Parent history answer')
    fork = page.locator('#side .item').filter(has=page.locator('.t', has_text='Droid independent fork')).first
    expect(fork).to_be_visible(); fork.click()
    expect(page.locator('#detail')).to_contain_text('Fork history answer')
    expect(page.locator('#side')).not_to_contain_text('Droid orphan child')
    ambiguous = page.locator('#side .item').filter(has=page.locator('.t', has_text='Droid ambiguous child')).first
    expect(ambiguous).to_be_visible(); ambiguous.click()
    expect(page.locator('#detail')).to_contain_text('父线程 ID 在已配置索引中存在歧义')
    # Inspect the same browser-visible list response to distinguish fork and child.
    rows = page.request.get(base + '/api/sessions').json()['sessions']
    parent_row = next(row for row in rows if row['uid'] == uid)
    assert [row['id'] for row in parent_row['agent_items']] == [fixture.child]
    assert parent_row['agent_items'][0]['active'] is False
    fork_row = next(row for row in rows if row['sid'] == fixture.fork)
    assert fork_row['forked_from_id'] == fixture.sid and not fork_row.get('agent_items')
    assert not any(row['sid'] == fixture.orphan for row in rows)
    broken = next(row for row in rows if row['sid'] == fixture.ambiguous)
    assert broken['supported'] is False
    parent.click();page.locator('#a-view-switch').click();child.click()
    page.set_viewport_size({'width':390, 'height':844});page.emulate_media(color_scheme='dark')
    # A viewport transition can restore the list; select the same child explicitly.
    if page.locator('#side').is_visible():
        parent.click();page.locator('#a-view-switch').click();child.click()
    expect(page.locator('#detail')).to_contain_text('Child history answer')
    page.screenshot(path=str(output / 'child-mobile-dark.png'), full_page=True)
    assert not errors, errors
    assert not fixture.frames('spawn'), 'read-only child navigation must not start Droid'


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('/tmp/sessiondock-droid-children-browser'))
    args=parser.parse_args();args.output.mkdir(parents=True, exist_ok=True)
    for hub_mode in (False, True):
        with tempfile.TemporaryDirectory(prefix='sessiondock-droid-children-') as directory, ExitStack() as stack:
            root=Path(directory);fixture=DroidChildrenFixture(root)
            port=free_port();extra=node_env(root,port,'127.0.0.0/8') if hub_mode else None
            base=stack.enter_context(fixture.server(args.binary,extra))
            if hub_mode:
                hub_root=root/'hub';hub_root.mkdir()
                hub=Hub(args.binary.with_name('sessiondock-hub'),hub_root,[SimpleNamespace(name='DroidNode',port=port,token=TOKEN)])
                hub.start();stack.callback(hub.stop);base=f'http://127.0.0.1:{hub.port}'
            output=args.output/('hub' if hub_mode else 'node');output.mkdir(exist_ok=True)
            with sync_playwright() as playwright:
                browser=playwright.chromium.launch(headless=True)
                page=browser.new_page(viewport={'width':1280,'height':900},color_scheme='light')
                check(page,base,fixture,output,hub_mode);browser.close()
    print('Droid child history Chromium node + Hub: PASS')


if __name__ == '__main__':main()
