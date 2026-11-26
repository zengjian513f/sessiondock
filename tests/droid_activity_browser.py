#!/usr/bin/env python3
"""Droid native activity projection in Node and Hub Chromium."""
import argparse
import json
import tempfile
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from droid_activity_fixtures import CHILD_SID, DroidActivityFixture
from claude_sdk_browser import open_sdk, send, wait_snapshot
from grok_sdk_browser import api
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
    page.wait_for_function("NativeSdkView.raw?.background_commands?.some(task => task.output_available)")
    running=page.evaluate('NativeSdkView.raw')
    assert running['background_commands'] == [
        {'task':'bg1','description':'Execute','command':'synthetic parent shell','output_available':True}
    ], running['background_commands']
    assert 'Droid own tail' not in json.dumps(running)
    sample = api(page, 'background_output', {'uid': uid, 'task': 'bg1'})['raw']
    assert sample['clipped'] and sample['output'].endswith('Droid own tail')
    assert len(sample['output'].encode()) <= 64 * 1024 and 'early prefix' not in sample['output']
    page.locator('.sdk-pane button.background-commands').click()
    dialog = page.get_by_role('dialog', name='后台输出')
    expect(dialog.get_by_label('日志输出')).to_contain_text('Droid own tail')
    expect(dialog.get_by_text('下载完整日志')).to_have_count(0)
    page.screenshot(path=str(output/'background-parent.png'))
    dialog.get_by_role('button', name='关闭').click()
    assert api(page, 'background_output', {'uid': uid, 'task': 'foreground'})['raw'] is None
    assert api(page, 'background_output', {'uid': uid, 'task': 'unbound'})['raw'] is None
    page.wait_for_function("""async ({uid, child}) => {
        await loadSessions(true);
        return S.sessions.find(row => row.uid === uid)?.agent_items?.some(item => item.id === child)
    }""", arg={'uid': uid, 'child': CHILD_SID})
    child = {'uid': uid, 'agent': CHILD_SID}
    page.wait_for_function("""async body => {
        const response = await fetch('/api/native-sdk/background_tasks', {method:'POST',
            headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
        return (await response.json()).raw?.background_commands?.[0]?.output_available === true;
    }""", arg=child)
    assert api(page, 'background_tasks', child)['raw']['background_commands'] == [
        {'task':'bg1','description':'Execute','command':'synthetic child shell','output_available':True}
    ]
    assert api(page, 'background_output', {**child, 'task':'bg1'})['raw']['output'] == 'Droid child tail'
    page.locator('#a-view-switch').click()
    page.locator(f'#session-view-menu button[data-agent="{CHILD_SID}"]').click()
    if page.locator('#a-sdk').get_attribute('aria-pressed') != 'true':
        page.locator('#a-sdk').click()
    expect(page.locator('.native-sdk-pane').get_by_text('Live child history answer')).to_be_visible()
    page.locator('.sdk-pane .background-commands').click()
    expect(dialog.get_by_label('日志输出')).to_have_text('Droid child tail')
    page.screenshot(path=str(output/'background-child.png'))
    dialog.get_by_role('button', name='关闭').click()
    page.locator('#a-view-switch').click()
    page.locator('#session-view-menu button[data-agent=""]').click()
    if page.locator('#a-sdk').get_attribute('aria-pressed') != 'true':
        page.locator('#a-sdk').click()
    (fixture.root / 'settle-background').touch()
    raw=wait_snapshot(page,uid,lambda raw: len(raw.get('background_tasks',[])) == 1 and raw['background_tasks'][0]['status']=='settled_after_execution')
    assert not raw['raw']['background_commands']
    assert api(page, 'background_output', {'uid': uid, 'task': 'bg1'})['raw'] is None
    frames = [json.loads(line) for line in fixture.received.read_text().splitlines()]
    assert sum(frame.get('type') == 'spawn' for frame in frames) == 1, frames
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
    child=page.locator(f'#session-view-menu button[data-agent="{CHILD_SID}"]')
    expect(child).to_contain_text('Live notified child')
    child.click()
    detail = page.locator('#detail')
    expect(detail.get_by_role('heading', name='Live notified child', exact=False)).to_be_visible()
    expect(page.locator('.sdk-pane .background-commands')).to_have_count(0)
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
                viewport = {'width':390,'height':844} if hub_mode else {'width':1280,'height':900}
                page=browser.new_page(viewport=viewport,color_scheme='dark' if hub_mode else 'light')
                check(page,base,fixture,output,hub_mode);browser.close()
    print('Droid activity Chromium node + Hub: PASS')


if __name__ == '__main__':main()
