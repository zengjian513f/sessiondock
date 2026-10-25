#!/usr/bin/env python3
"""Kiro Enter FIFO and Ctrl+Enter native steer via Chromium, node and Hub."""
import argparse
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY
from sdk_send_gesture_fixtures import exercise_idle_send_gestures, exercise_send_gestures
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
    def finish_idle(text):
        expect(page.locator('.sdk-live')).to_contain_text(text)
        page.wait_for_function('!NativeSdkView.running')
        rows=[json.loads(line) for line in (root/'kiro-acp-received.jsonl').read_text().splitlines()]
        assert len([r for r in rows if r.get('method')=='session/prompt' and text in json.dumps(r)])==1
    exercise_idle_send_gestures(page,'kiro',label,finish_idle,shots)
    box.fill('steer-hold');box.press('Enter')
    page.wait_for_function('NativeSdkView.running')
    page.set_viewport_size({'width':390,'height':900})
    if not box.is_visible():page.locator(f'#side .item[data-uid="{uid}"]').click()
    exercise_send_gestures(page,'kiro',label,shots)
    page.set_viewport_size({'width':1280,'height':900})
    box.fill('recall queued '+label);box.press('Enter');expect(box).to_have_value('')
    recalled=page.locator('.sdk-queued').filter(has_text='recall queued '+label)
    recalled.get_by_role('button',name='撤回编辑',exact=True).click()
    expect(recalled).to_have_count(0)
    expect(box).to_have_value('recall queued '+label)
    box.fill('ordinary queued '+label);box.press('Enter');expect(box).to_have_value('')
    expect(page.locator('.sdk-queue')).to_contain_text('ordinary queued '+label)
    queued=page.locator('.sdk-queued').filter(has_text='ordinary queued '+label)
    expect(queued.get_by_role('button',name='立即引导',exact=True)).to_be_visible()
    if shots:
        for width,theme in ((1280,'light'),(390,'dark')):
            page.set_viewport_size({'width':width,'height':900});page.emulate_media(color_scheme=theme)
            page.wait_for_timeout(250)
            if not box.is_visible():
                row=page.locator(f'#side .item[data-uid="{uid}"]')
                if row.is_visible():row.click()
                else:page.locator('#a-sdk').click()
            expect(queued).to_be_visible()
            page.screenshot(path=str(shots/f'{label}-queue-{width}-{theme}.png'),full_page=True)
    queue_id=queued.get_attribute('data-queue-id')
    box.fill('preserve unrelated draft')
    gate=root/'fake-native-store/steer-delay';gate.touch()
    with page.expect_request(lambda r:'/api/native-sdk/queue_steer' in r.url):
        queued.get_by_role('button',name='立即引导',exact=True).click()
    expect(queued.get_by_role('button',name='立即引导',exact=True)).to_be_disabled()
    gate.unlink()
    expect(queued).to_have_count(0)
    expect(box).to_have_value('preserve unrelated draft')
    expect(page.locator('.sdk-live')).to_contain_text('ordinary queued '+label)
    page.set_viewport_size({'width':1280,'height':900});page.emulate_media(color_scheme='light')
    # A second browser/stale click reuses the persisted receipt, never another RPC.
    response=page.request.post(base+'/api/native-sdk/queue_steer',data={'uid':uid,'id':queue_id,'request_id':'duplicate-steer'})
    assert response.ok,response.text()
    box.fill('native steering '+label);box.press('Control+Enter');expect(box).to_have_value('')
    expect(page.locator('.sdk-live')).to_contain_text('native steering '+label)
    assert page.evaluate('NativeSdkView.running')
    rows=lambda:[json.loads(line) for line in (root/'kiro-acp-received.jsonl').read_text().splitlines()]
    def steers():return [r for r in rows() if r.get('method')=='_session/steer']
    assert len([r for r in steers() if r['params']['message']=='ordinary queued '+label])==1
    frame=steers()[-1];assert frame['params']['message']=='native steering '+label,frame
    assert 'prompt' not in frame['params'] and 'text' not in frame['params']
    assert not [r for r in rows() if r.get('method')=='session/prompt' and 'ordinary queued '+label in json.dumps(r)]
    assert page.evaluate("NativeSdkView.items.filter(i=>i.role==='assistant' && i.text?.includes('Partial response before stop'))[0].text.includes('continuing after steer')")
    # Rejection after conversion stays a steer failure, never a FIFO retry.
    box.fill('reject steer');box.press('Enter');expect(box).to_have_value('')
    rejected=page.locator('.sdk-queued').filter(has_text='reject steer')
    rejected_id=rejected.get_attribute('data-queue-id')
    rejected.get_by_role('button',name='立即引导',exact=True).click()
    rejected=page.locator(f'.sdk-queue [data-queue-id="{rejected_id}"]')
    expect(rejected).to_contain_text('Steer unavailable')
    assert not rejected.get_by_role('button',name='重试',exact=True).count()
    rejected.get_by_role('button',name='撤销',exact=True).click()
    expect(rejected).to_have_count(0)
    assert not [r for r in rows() if r.get('method')=='session/prompt' and 'reject steer' in json.dumps(r)]
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
    box.press('Enter');expect(box).to_have_value('')
    attached=page.locator('.sdk-queued').filter(has_text='attachment steer')
    expect(attached).to_be_visible()
    assert not attached.get_by_role('button',name='立即引导',exact=True).count()
    attached_id=attached.get_attribute('data-queue-id')
    response=page.request.post(base+'/api/native-sdk/queue_steer',data={'uid':uid,'id':attached_id})
    assert not response.ok
    attached.get_by_role('button',name='撤销',exact=True).click()
    expect(attached).to_have_count(0)
    page.locator('#sdk-file').set_input_files(attachment)
    page.get_by_role('button',name='移除附件',exact=True).click()
    box.fill('click steering '+label)
    action=page.get_by_role('button',name='引导当前任务',exact=True)
    if not action.is_visible():page.locator('#sdk-more').click()
    action.click();expect(box).to_have_value('')
    expect(page.locator('.sdk-live')).to_contain_text('click steering '+label)
    gate=root/'fake-native-store/steer-delay';gate.touch()
    box.fill('delayed steer')
    with page.expect_request(lambda r:'/api/native-sdk/steer' in r.url):box.press('Control+Enter')
    pending=page.locator('.sdk-queue .sdk-queued').filter(has_text='delayed steer')
    expect(pending).to_be_visible()
    expect(pending).to_have_attribute('data-role','user')
    assert not page.locator('.sdk-request').filter(has_text='delayed steer').count()
    if shots:
        for width,theme in ((1280,'light'),(390,'dark')):
            page.set_viewport_size({'width':width,'height':900});page.emulate_media(color_scheme=theme)
            page.wait_for_timeout(250)  # Let responsive navigation finish before choosing its entry.
            if not box.is_visible():
                row=page.locator(f'#side .item[data-uid="{uid}"]')
                if row.is_visible():row.click()
                else:page.locator('#a-sdk').click()
            expect(pending).to_be_visible()
            page.screenshot(path=str(shots/f'{label}-pending-{width}-{theme}.png'),full_page=True)
        page.set_viewport_size({'width':1280,'height':900});page.emulate_media(color_scheme='light')
    box.fill('newer draft')
    with page.expect_response(lambda r:'/api/native-sdk/steer' in r.url):gate.unlink()
    expect(page.locator('#sdk-send')).to_be_enabled();expect(box).to_have_value('newer draft')
    page.reload(wait_until='domcontentloaded');page.wait_for_function('NativeSdkView.connected && NativeSdkView.running')
    expect(page.locator('.sdk-msgs')).to_contain_text('native steering '+label)
    assert len([r for r in steers() if r['params']['message']=='native steering '+label])==1
    if shots:
        for width,theme in ((1280,'light'),(390,'dark')):
            page.set_viewport_size({'width':width,'height':900});page.emulate_media(color_scheme=theme)
            page.wait_for_timeout(250)  # Let responsive navigation finish before choosing its entry.
            if not box.is_visible():
                row=page.locator(f'#side .item[data-uid="{uid}"]')
                if row.is_visible():row.click()
                else:page.locator('#a-sdk').click()
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
