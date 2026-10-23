#!/usr/bin/env python3
"""Codex direct steer: keyboard/click, durable receipts, rejection and draft races."""
import argparse
import asyncio
from pathlib import Path
import tempfile
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from codex_sdk_model_queue_browser import SelectionServer
from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from codex_sdk_queue_browser import open_page
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env

class DirectServer(SelectionServer):
    hold_steer = False
    reject_steer = False
    async def _handle(self, writer, message):
        if message.get('method')=='turn/steer':
            while self.hold_steer:await asyncio.sleep(0.02)
            if self.reject_steer or message['params']['expectedTurnId']!=self.turn:
                await self.send(writer,{'id':message['id'],'error':{'code':-32000,'message':'Synthetic steer rejected'}})
                return
        await super()._handle(writer,message)


def exercise(browser,base,fake,width,theme,screenshots,label):
    context,page=open_page(browser,base,width,theme)
    errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
    box=page.locator('#sdk-input');send=page.locator('#sdk-send')
    def submit(text):
        box.fill(text);send.click();expect(box).to_have_value('')
    def steer(text):
        box.fill(text)
        with page.expect_response('**/api/codex-sdk/turn') as response:box.press('Control+Enter')
        expect(send).to_be_enabled()
        return response.value
    def action():
        button=page.get_by_role('button',name='引导当前任务',exact=True)
        if not button.is_visible():page.locator('#sdk-more').click()
        return button
    box.fill('idle draft');box.press('Control+Enter');expect(box).to_have_value('idle draft')
    expect(page.locator('#sdk-status-menu')).to_contain_text('当前没有')
    submit('working '+label);expect(send).to_have_text('排队')
    starts=len(fake.methods('turn/start'))
    submit('ordinary queue '+label)
    expect(page.locator('.sdk-queued')).to_contain_text('ordinary queue '+label)
    steer('direct steer '+label);expect(box).to_have_value('')
    params=fake.methods('turn/steer')[-1]['params']
    assert params['expectedTurnId']==fake.turn and params['input'][0]['text']=='direct steer '+label,params
    assert 'model' not in params and 'effort' not in params
    assert len(fake.methods('turn/start'))==starts
    n=len(fake.methods('turn/steer'))
    box.fill('newline');box.press('Shift+Enter');expect(box).to_have_value('newline\n')
    box.dispatch_event('keydown',{'key':'Enter','ctrlKey':True,'isComposing':True})
    assert len(fake.methods('turn/steer'))==n
    page.locator('#sdk-file').set_input_files({'name':'steer-context.txt','mimeType':'text/plain','buffer':b'context attachment'})
    page.locator('#sdk-add').click()
    page.locator('#sdk-attach-menu [data-attach="quote"]').click()
    page.get_by_role('textbox',name='引用文字').fill('quoted steer context')
    fake.reject_steer=True
    steer('rejected '+label);expect(box).to_have_value('rejected '+label)
    expect(page.locator('.sdk-queued').filter(has_text='rejected '+label)).to_contain_text('Synthetic steer rejected')
    expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(2)
    assert 'quoted steer context' in fake.methods('turn/steer')[-1]['params']['input'][0]['text']
    assert 'steer-context.txt' in fake.methods('turn/steer')[-1]['params']['input'][0]['text']
    n=len(fake.methods('turn/steer'));steer('rejected '+label)
    assert len(fake.methods('turn/steer'))==n,'duplicate rejection was sent again'
    fake.reject_steer=False
    page.locator('.sdk-queued').filter(has_text='rejected '+label).get_by_role('button',name='重试引导',exact=True).click()
    expect(page.locator('.sdk-queued').filter(has_text='rejected '+label)).to_have_count(0)
    # A successful receipt rechecked in the editor clears only its own fields.
    steer('rejected '+label);expect(box).to_have_value('')
    expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(0)
    fake.hold_steer=True
    box.fill('delayed '+label)
    with page.expect_request('**/api/codex-sdk/turn'):box.press('Control+Enter')
    box.fill('newer draft '+label)
    with page.expect_response('**/api/codex-sdk/turn'):fake.hold_steer=False
    expect(send).to_be_enabled();expect(box).to_have_value('newer draft '+label)
    box.fill('click steer '+label)
    button=action()
    if screenshots:page.screenshot(path=str(screenshots/f'{label}-steer.png'),full_page=True)
    button.click();expect(box).to_have_value('')
    # Lost HTTP reply, then retry the saved operation after reload: one native steer.
    payloads=[]
    def lose(route):
        payloads.append(route.request.post_data_json);route.fetch();route.abort()
    page.route('**/api/codex-sdk/turn',lose)
    box.fill('lost reply '+label);box.press('Control+Enter')
    expect(page.locator('#sdk-status-menu')).to_contain_text('发送失败')
    expect(box).to_have_value('lost reply '+label)
    page.unroute('**/api/codex-sdk/turn',lose)
    page.reload(wait_until='networkidle')
    if not box.is_visible():page.locator('#a-sdk').click()
    expect(box).to_have_value('lost reply '+label)
    steer('lost reply '+label);expect(box).to_have_value('')
    assert len([m for m in fake.methods('turn/steer') if m['params']['input'][0]['text']=='lost reply '+label])==1
    # A turn ending between HTTP submission and native acceptance must reject,
    # never fall back to turn/start or consume the draft.
    page.locator('.sdk-queued').filter(has_text='ordinary queue '+label).get_by_role('button',name='撤销',exact=True).click()
    expect(page.locator('.sdk-queued')).to_have_count(0)
    fake.hold_steer=True
    box.fill('closing turn '+label)
    with page.expect_request('**/api/codex-sdk/turn'):box.press('Control+Enter')
    for _ in range(100):
        if fake.methods('turn/steer')[-1]['params']['input'][0]['text']=='closing turn '+label:break
        page.wait_for_timeout(50)
    assert fake.methods('turn/steer')[-1]['params']['input'][0]['text']=='closing turn '+label
    fake.complete()
    with page.expect_response('**/api/codex-sdk/turn'):fake.hold_steer=False
    expect(box).to_have_value('closing turn '+label);expect(send).to_have_text('发送')
    assert len(fake.methods('turn/start'))==starts
    row=page.locator('.sdk-queued').filter(has_text='closing turn '+label)
    expect(row.get_by_role('button',name='重试引导')).to_be_disabled()
    row.get_by_role('button',name='撤销',exact=True).click();expect(row).to_have_count(0)
    assert not errors,errors
    context.close();print('PASS',label,'direct/click steer, FIFO, idle/IME, native rejection/retry, late draft, lost reply dedup, closing turn',flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    hub_binary = args.hub_binary or args.binary.with_name('sessiondock-hub')
    if not args.hub_binary and not hub_binary.is_file():
        hub_binary = Path(__file__).resolve().parents[1] / 'target/release/sessiondock-hub'
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-direct-steer-') as tmp, sync_playwright() as pw:
        root = Path(tmp); corpus = build(root); port = free_port()
        work=root/'work';work.mkdir()
        rollout=corpus.paths['sdk'];rollout.write_bytes(rollout.read_bytes().replace(b'/synthetic/sdk',str(work).encode()))
        env = {**environment(root,args.binary), **node_env(root,port,'127.0.0.0/8')}
        browser = pw.chromium.launch(headless=True)
        try:
            with DirectServer(root/'app.sock',corpus.paths['sdk'],THREAD) as fake:
                with server(corpus,args.binary,root/'app.sock',env) as base:
                    hub_root = root/'hub'; hub_root.mkdir()
                    hub = Hub(hub_binary.resolve(),hub_root,
                              [SimpleNamespace(name='ModelNode',token=TOKEN,port=port)])
                    hub.start()
                    try:
                        for mode,url,width,theme in [('node',base,1280,'light'),('node',base,390,'dark'),
                            ('hub',f'http://127.0.0.1:{hub.port}',1280,'dark'),('hub',f'http://127.0.0.1:{hub.port}',390,'light')]:
                            exercise(browser,url,fake,width,theme,args.screenshots,f'{mode}-{width}-{theme}')
                    finally: hub.stop()
        finally: browser.close()


if __name__ == '__main__': main()
