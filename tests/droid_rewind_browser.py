#!/usr/bin/env python3
"""Droid selective native file rewind through the real browser controls."""
import argparse
import json
import tempfile
from pathlib import Path
from playwright.sync_api import sync_playwright, expect
from droid_sdk_fixtures import DroidFixture
from history_fixtures import BINARY
from sdk_usage_fixtures import reveal
from claude_sdk_browser import open_sdk, receipt


def check(f, base, screenshots):
    original=f.path.read_text()
    existing=f.cwd/'existing.txt';created=f.cwd/'created.txt'
    existing.write_text('AFTER\n');created.write_text('CREATED\n')
    with sync_playwright() as p:
        browser=p.chromium.launch(headless=True)
        page=browser.new_page(viewport={'width':1280,'height':900})
        page.goto(base,wait_until='networkidle');uid=page.locator('#side .item[data-uid^="droid:"]').first.get_attribute('data-uid');open_sdk(page,f,expect,uid=uid)
        def dialog():
            reveal(page, '[data-sdk-action=rewind]').click()
            page.locator('#sdk-action-dialog select').select_option('seed-user-0')
            page.locator('#sdk-action-dialog button[type=submit]').click()
            expect(page.locator('#sdk-action-dialog')).to_contain_text('无法恢复：expired.txt')
        dialog()
        expect(page.get_by_role('checkbox',name=f'恢复 {existing}',exact=True)).not_to_be_checked()
        page.locator('#sdk-action-dialog [data-action=cancel]').click()
        assert not [v for v in f.frames() if v.get('method')=='droid.execute_rewind']
        assert existing.read_text()=='AFTER\n' and created.exists()
        (f.root/'rewind-fail').touch()
        dialog()
        with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/rewind')) as failed:
            page.locator('#sdk-action-dialog button[type=submit]').click()
        assert not failed.value.ok
        assert existing.read_text()=='AFTER\n' and created.exists()
        assert f.path.read_text()==original
        (f.root/'rewind-fail').unlink()
        dialog()
        if screenshots:page.screenshot(path=str(screenshots/'rewind-wide-light.png'),full_page=True)
        page.set_viewport_size({'width':390,'height':844});page.emulate_media(color_scheme='dark')
        if screenshots:page.screenshot(path=str(screenshots/'rewind-mobile-dark.png'),full_page=True)
        page.get_by_role('checkbox',name=f'恢复 {existing}',exact=True).check()
        page.get_by_role('checkbox',name=f'删除 {created}',exact=True).check()
        with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/rewind')) as response:
            page.locator('#sdk-action-dialog button[type=submit]').click()
        assert response.value.ok,response.value.text()
        result=receipt(response.value.json());assert result['uid']!=uid,result
        repeated=page.request.post(base+'/api/native-sdk/rewind',data=response.value.request.post_data_json)
        assert repeated.ok,repeated.text()
        assert receipt(repeated.json())['uid']==result['uid']
        expect(page.locator('#sdk-input')).to_be_enabled()
        page.locator('.native-sdk-pane .sdk-status').click()
        expect(page.locator('#sdk-status-menu')).to_be_visible()
        expect(page.locator('#sdk-status-menu')).to_contain_text('恢复 1 个文件')
        if screenshots:page.screenshot(path=str(screenshots/'rewind-result-mobile-dark.png'),full_page=True)
        page.keyboard.press('Escape')
        assert existing.read_text()=='BEFORE\n' and not created.exists()
        assert f.path.read_text()==original
        bindings=json.loads((f.root/'state/native-sdk-bindings.json').read_text())
        assert not any(row.get('session_id')==f.sid for row in bindings.values()),bindings
        assert any(row.get('session_id')==result['session_id'] for row in bindings.values()),bindings
        requests=[v for v in f.frames() if v.get('method')=='droid.execute_rewind'];assert len(requests)==2,requests
        assert requests[0]['params']['messageId']=='seed-user-0'
        browser.close()
        return response.value.request.post_data_json,result['uid']


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--screenshots',type=Path);args=parser.parse_args()
    if args.screenshots:args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-droid-rewind-') as root:
        f=DroidFixture(root)
        with f.server(BINARY,preserve_sdkhosts=True) as base:replay,successor=check(f,base,args.screenshots)
        attempts=len([v for v in f.frames() if v.get('method')=='droid.execute_rewind'])
        with f.server(BINARY) as base,sync_playwright() as p:
            browser=p.chromium.launch(headless=True);page=browser.new_page();page.goto(base,wait_until='networkidle')
            result=page.request.post(base+'/api/native-sdk/rewind',data=replay)
            assert result.ok,result.text()
            assert receipt(result.json())['uid']==successor
            assert len([v for v in f.frames() if v.get('method')=='droid.execute_rewind'])==attempts,'restart replay executed native rewind again'
            browser.close()
    from node_auth_fixtures import node_env,free_port,TOKEN
    from hub_fixtures import Hub
    from types import SimpleNamespace
    with tempfile.TemporaryDirectory(prefix='sessiondock-droid-rewind-hub-') as root:
        f=DroidFixture(root);port=free_port();extra=node_env(Path(root),port,'127.0.0.0/8')
        with f.server(BINARY,extra):
            hub_root=Path(root)/'hub';hub_root.mkdir()
            hub=Hub(BINARY.with_name('sessiondock-hub'),hub_root,[SimpleNamespace(name='DroidNode',token=TOKEN,port=port)])
            hub.start()
            try:check(f,f'http://127.0.0.1:{hub.port}',None)
            finally:hub.stop()
    print('Droid file rewind Chromium node/Hub: PASS')


if __name__=='__main__':main()
