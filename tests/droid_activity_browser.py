#!/usr/bin/env python3
"""Droid native activity projection in Node and Hub Chromium."""
import argparse
import json
import tempfile
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from droid_activity_fixtures import DroidActivityFixture
from claude_sdk_browser import open_sdk, send, wait_snapshot
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import node_env, free_port, TOKEN


def check(page, base, fixture, output, hub=False):
    errors=[]
    page.on('pageerror',lambda error: errors.append(str(error)))
    page.goto(base,wait_until='networkidle')
    uid=page.locator('#side .item[data-uid^="droid:"]').first.get_attribute('data-uid')
    open_sdk(page,fixture,expect,uid=uid)
    send(page,'activity projection')
    raw=wait_snapshot(page,uid,lambda raw: len(raw.get('background_tasks',[])) == 1 and raw['background_tasks'][0]['status']=='settled_after_execution')
    for text in ['子代理可用','Inspect implementation','Hook visible output','isolated-mcp','Inspect isolated feature','Isolated mission accepted','任务工作进程完成','后台任务结束']:
        expect(page.locator('.sdk-live')).to_contain_text(text)
    expect(page.locator('.sdk-live')).not_to_contain_text('SUPPRESSED_SECRET')
    expect(page.locator('.sdk-live')).not_to_contain_text('HIDDEN_HOOK')
    assert not page.evaluate('NativeSdkView.running'), 'detached heartbeat must not reopen a turn'
    assert page.evaluate("NativeSdkView.items.filter(item => item.id === 'activity:background:bg1').length") == 1
    for button in page.get_by_role('button',name='展开本轮过程').all(): button.click()
    for hidden in ('AUTH_SECRET','SUPPRESSED_SECRET','HIDDEN_HOOK'): assert hidden not in json.dumps(raw)
    page.screenshot(path=str(output/'activity-wide-light.png'),full_page=True)
    page.set_viewport_size({'width':390,'height':844});page.emulate_media(color_scheme='dark')
    open_sdk(page,fixture,expect,uid=uid)
    expect(page.locator('.sdk-live')).to_contain_text('后台任务结束')
    for button in page.get_by_role('button',name='展开本轮过程').all(): button.click()
    page.get_by_text('后台任务结束',exact=False).last.scroll_into_view_if_needed()
    page.screenshot(path=str(output/'activity-mobile-dark.png'),full_page=True)
    # Notification refreshes the existing read model; only persisted provenance
    # supplies a navigable child, and availability does not claim an active turn.
    page.set_viewport_size({'width':1280,'height':900})
    page.locator('#a-view-switch').click()
    child=page.locator('#session-view-menu button[data-agent="60000000-0000-4000-8000-000000000099"]')
    expect(child).to_contain_text('Live notified child')
    child.click()
    expect(page.locator('#detail')).to_contain_text('Live child history answer')
    page.screenshot(path=str(output/'activity-child-navigation.png'),full_page=True)
    assert not errors,errors


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('/tmp/sessiondock-droid-activity-browser'))
    args=parser.parse_args();args.output.mkdir(parents=True, exist_ok=True)
    for hub_mode in (False, True):
        with tempfile.TemporaryDirectory(prefix='sessiondock-droid-activity-') as directory, ExitStack() as stack:
            root=Path(directory);fixture=DroidActivityFixture(root)
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
    print('Droid activity Chromium node + Hub: PASS')


if __name__ == '__main__':main()
