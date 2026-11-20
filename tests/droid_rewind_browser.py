#!/usr/bin/env python3
"""Droid selective native file rewind through the real browser controls."""
import argparse
import json
import tempfile
from pathlib import Path
from urllib.parse import quote
from playwright.sync_api import sync_playwright, expect
from droid_sdk_fixtures import DroidFixture, native_records
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
            page.locator('.sdk-history .msg[data-role="user"], .sdk-live .msg[data-role="user"]').first.get_by_role('button', name='回退对话与文件', exact=True).click()
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


def seed_long_history(f):
    rows = native_records(f.sid, f.cwd, [
        ('First retained question', 'First retained answer'),
        ('Earlier question', 'Earlier answer'),
        ('Last retained question', 'Last retained progress'),
        ('Rewind from this question', 'Removed answer'),
    ])

    def message(mid, role, content, **extra):
        return {'type': 'message', 'id': mid, 'timestamp': rows[1]['timestamp'],
                'message': {'role': role, 'content': content, **extra}}

    rows.insert(1, message('initial-context', 'user', [
        {'type': 'text', 'text': f'Initial context block {i}'} for i in range(6)
    ], visibility='llm_only'))
    boundary = next(i for i, row in enumerate(rows) if row['id'] == 'seed-user-3')
    work = []
    for i in range(120):
        call = f'read-{i}'
        if i == 30:
            work.append(message('compact-notice', 'user', [
                {'type': 'text', 'text': 'Droid compacted the context'},
            ], systemNotice=True, visibility='user_only'))
        work.extend([
            message(f'call-{i}', 'assistant', [
                {'type': 'tool_use', 'id': call, 'name': 'Read',
                 'input': {'file_path': f'fixture-{i:03}.txt'}},
            ]),
            message(f'result-{i}', 'user', [
                {'type': 'tool_result', 'toolUseId': call, 'content': f'Read output {i}'},
            ]),
        ])
    rows[boundary:boundary] = work
    f.path.write_text(''.join(json.dumps(row) + '\n' for row in rows))


def check_long_history(f, base, screenshots, label):
    original = f.path.read_text()
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        page = browser.new_page(viewport={'width': 1280, 'height': 900}, color_scheme='light')
        page.goto(base, wait_until='networkidle')
        uid = page.locator('#side .item[data-uid^="droid:"]').first.get_attribute('data-uid')
        open_sdk(page, f, expect, uid=uid)
        history = page.locator('.sdk-history')
        target = history.locator('.msg[data-role="user"]').filter(has_text='Rewind from this question')
        target.get_by_role('button', name='回退对话与文件', exact=True).click()
        with page.expect_response(lambda r: r.url.endswith('/api/native-sdk/rewind')) as response:
            page.get_by_role('button', name='创建回退分支', exact=True).click()
        assert response.value.ok, response.value.text()
        branch = receipt(response.value.json())['uid']
        assert branch != uid
        page.wait_for_function('uid => NativeSdkView.uid === uid && NativeSdkView.connected', arg=branch)
        expect(history.locator('.history-gap')).to_be_visible()
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-long-rewind-wide.png'), full_page=True)
        window = page.request.get(f'{base}/api/messages/{quote(branch, safe="")}?window=1').json()
        assert window['partial']['omitted'] > 0
        messages = window['messages']
        expected = {'seed-user-0', 'seed-user-2', 'seed-assistant-2', 'result-119'}
        assert expected <= {row.get('native_id') for row in messages}, expected - {
            row.get('native_id') for row in messages
        }
        turn = next(row['turn_id'] for row in messages if row.get('native_id') == 'seed-user-2')
        for mid, role in [('compact-notice', 'system'), ('result-119', 'tool_result')]:
            row = next(row for row in messages if row.get('native_id') == mid)
            assert row['role'] == role and row['turn_id'] == turn, row
        for text in ('First retained question', 'Last retained question'):
            expect(history.get_by_text(text, exact=True)).to_be_visible()
        expect(history).not_to_contain_text('Rewind from this question')
        expect(history).not_to_contain_text('Removed answer')
        expect(page.locator('#sdk-input')).to_have_value('Rewind from this question')
        progress = history.get_by_text('Last retained progress', exact=True)
        if not progress.is_visible():
            history.locator('.turn-process.folded > .turn-toolbar .fold-toggle').last.click()
        expect(progress).to_be_visible()
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-long-rewind-expanded.png'), full_page=True)
        history.locator('.history-gap-load').click()
        expect(history.locator('.history-gap')).to_have_count(0)
        expect(history.get_by_text('Earlier answer', exact=True)).to_be_visible()
        assert f.path.read_text() == original
        page.reload(wait_until='domcontentloaded')
        expect(history.get_by_text('First retained question', exact=True)).to_be_visible()
        expect(history.get_by_text('Last retained question', exact=True)).to_be_visible()
        page.set_viewport_size({'width': 390, 'height': 844})
        page.emulate_media(color_scheme='dark')
        page.locator(f'#side .item[data-uid="{branch}"]').click()
        expect(history.get_by_text('First retained question', exact=True)).to_be_in_viewport()
        expect(history.get_by_text('Last retained question', exact=True)).to_be_in_viewport()
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-long-rewind-mobile.png'), full_page=True)
        browser.close()


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
    with tempfile.TemporaryDirectory(prefix='sessiondock-droid-rewind-history-') as root:
        for label in ('node', 'hub'):
            path = Path(root) / label
            path.mkdir()
            f = DroidFixture(path)
            seed_long_history(f)
            port = free_port()
            with f.server(BINARY, node_env(path, port, '127.0.0.0/8')) as base:
                if label == 'node':
                    check_long_history(f, base, args.screenshots, label)
                else:
                    hub_root = path / 'hub'
                    hub_root.mkdir()
                    hub = Hub(BINARY.with_name('sessiondock-hub'), hub_root,
                              [SimpleNamespace(name='DroidNode', token=TOKEN, port=port)])
                    hub.start()
                    try:
                        check_long_history(f, f'http://127.0.0.1:{hub.port}', args.screenshots, label)
                    finally:
                        hub.stop()
    print('Droid file rewind and long branch history Chromium node/Hub: PASS')


if __name__=='__main__':main()
