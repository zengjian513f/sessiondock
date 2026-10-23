#!/usr/bin/env python3
"""Kiro Enter FIFO and Ctrl+Enter native steer via Chromium, node and Hub."""
import argparse
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY
from native_sdk_rename_browser import native
from sdk_generalization_browser import open_view
from sdkhost_fixtures import stop_private_sdkhosts
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env


def exercise(browser, base, root, shots, label):
    context = browser.new_context(viewport={'width':1280,'height':900}, service_workers='block')
    page = context.new_page(); page.set_default_timeout(20000)
    errors=[]; page.on('pageerror',lambda e:errors.append(str(e)))
    uid=open_view(page,base,'kiro')
    box=page.locator('#sdk-input')
    box.fill('idle steer');box.press('Control+Enter')
    expect(box).to_have_value('idle steer')
    expect(page.locator('#sdk-status-menu')).to_contain_text('当前没有')
    box.fill('steer-hold');box.press('Enter')
    page.wait_for_function('NativeSdkView.running')
    box.fill('ordinary queued '+label);box.press('Enter');expect(box).to_have_value('')
    expect(page.locator('.sdk-queue')).to_contain_text('ordinary queued '+label)
    box.fill('native steering '+label);box.press('Control+Enter');expect(box).to_have_value('')
    expect(page.locator('.sdk-live')).to_contain_text('native steering '+label)
    assert page.evaluate('NativeSdkView.running')
    rows=lambda:[json.loads(line) for line in (root/'kiro-acp-received.jsonl').read_text().splitlines()]
    def steers():return [r for r in rows() if r.get('method')=='_session/steer']
    frame=steers()[-1];assert frame['params']['message']=='native steering '+label,frame
    assert 'prompt' not in frame['params'] and 'text' not in frame['params']
    assert not [r for r in rows() if r.get('method')=='session/prompt' and 'ordinary queued '+label in json.dumps(r)]
    assert page.evaluate("NativeSdkView.items.filter(i=>i.role==='assistant' && i.text?.includes('Partial response before stop'))[0].text.includes('continuing after steer')")
    count=len(steers())
    box.fill('newline');box.press('Shift+Enter');expect(box).to_have_value('newline\n')
    box.dispatch_event('keydown',{'key':'Enter','ctrlKey':True,'isComposing':True})
    assert len(steers())==count
    for text in ('reject steer','drop steer'):
        box.fill(text)
        with page.expect_response(lambda r:'/api/native-sdk/steer' in r.url):box.press('Control+Enter')
        expect(page.locator('.sdk-queue')).to_contain_text('Steer unavailable' if text=='reject steer' else 'epoch_changed')
        expect(page.locator('#sdk-send')).to_be_enabled()
        expect(box).to_have_value(text)
        assert not page.locator('.sdk-live').get_by_text(text,exact=True).count()
        # Repeated click must return the saved rejection without another RPC.
        n=len(steers())
        with page.expect_response(lambda r:'/api/native-sdk/steer' in r.url):box.press('Control+Enter')
        expect(page.locator('#sdk-send')).to_be_enabled()
        assert len(steers())==n
    attachment=root/'steer-attachment.txt';attachment.write_text('preserve attachment')
    page.locator('#sdk-file').set_input_files(attachment)
    box.fill('attachment steer');box.press('Control+Enter')
    expect(page.locator('#sdk-status-menu')).to_contain_text('只支持文字')
    expect(box).to_have_value('attachment steer')
    page.get_by_role('button',name='移除附件',exact=True).click()
    box.fill('click steering '+label)
    action=page.get_by_role('button',name='引导当前任务',exact=True)
    if not action.is_visible():page.locator('#sdk-more').click()
    action.click();expect(box).to_have_value('')
    expect(page.locator('.sdk-live')).to_contain_text('click steering '+label)
    gate=root/'fake-native-store/steer-delay';gate.touch()
    box.fill('delayed steer')
    with page.expect_request(lambda r:'/api/native-sdk/steer' in r.url):box.press('Control+Enter')
    box.fill('newer draft')
    with page.expect_response(lambda r:'/api/native-sdk/steer' in r.url):gate.unlink()
    expect(page.locator('#sdk-send')).to_be_enabled();expect(box).to_have_value('newer draft')
    page.reload(wait_until='domcontentloaded');page.wait_for_function('NativeSdkView.connected && NativeSdkView.running')
    expect(page.locator('.sdk-msgs')).to_contain_text('native steering '+label)
    assert len([r for r in steers() if r['params']['message']=='native steering '+label])==1
    if shots:
        for width,theme in ((1280,'light'),(390,'dark')):
            page.set_viewport_size({'width':width,'height':900});page.emulate_media(color_scheme=theme)
            if not box.is_visible():page.locator(f'#side .item[data-uid="{uid}"]').click()
            box.fill('下一步请先检查日志')
            if not action.is_visible():page.locator('#sdk-more').click()
            page.screenshot(path=str(shots/f'{label}-{width}-{theme}.png'),full_page=True)
            if width==390:
                action.click();expect(box).to_have_value('')
            if page.locator('#sdk-more-menu').evaluate("e=>e.matches(':popover-open')"):page.keyboard.press('Escape')
    page.locator('#sdk-stop').click()
    page.wait_for_function('!NativeSdkView.running')
    assert not errors,errors
    assert not [r for r in rows() if r.get('method')=='session/steer']
    context.close();print('PASS',label,'Enter queue, Ctrl+Enter/click native steer, idle/IME/newline, rejection/dropped, dedup and reload',flush=True)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY);parser.add_argument('--screenshots',type=Path)
    args=parser.parse_args()
    if args.screenshots:args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-kiro-steer-') as temp,sync_playwright() as p:
        root=Path(temp);port=free_port();browser=p.chromium.launch(headless=True)
        try:
            with native('kiro',root,args.binary,node_env(root,port,'127.0.0.0/8')) as base:
                exercise(browser,base,root,args.screenshots,'node')
                hub_root=root/'hub';hub_root.mkdir()
                hub=Hub(args.binary.with_name('sessiondock-hub').resolve(),hub_root,[SimpleNamespace(name='SteerNode',token=TOKEN,port=port)])
                hub.start()
                try:exercise(browser,f'http://127.0.0.1:{hub.port}',root,args.screenshots,'hub')
                finally:hub.stop()
        finally:stop_private_sdkhosts(root);browser.close()

if __name__=='__main__':main()
