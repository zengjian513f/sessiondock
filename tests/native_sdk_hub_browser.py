#!/usr/bin/env python3
"""Native SDK create/draft/SSE/request routing through a real node and Hub.

Uses Claude's synthetic stream-json peer and private loopback services only.
Wide/narrow, light/dark; all SEND, approvals and questions use browser controls.
"""
from __future__ import annotations
import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from claude_sdk_fixtures import ClaudeFixture
from browser_runtime import wait_for_async
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from new_session_model_browser import open_dialog


def exercise(browser, base, fixture, width, theme, screenshots):
    context = browser.new_context(viewport={'width':width,'height':900}, color_scheme=theme,
                                  service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base+'/') else route.abort())
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded && Object.values(Nodes.capabilities).some(c=>c.native_sdks?.claude)')
    row = page.locator('#side .item[data-uid]').filter(has_text='earlier synthetic Claude').first
    if not row.count():
        row = page.locator('#side .item[data-uid^="claude:"]').first
    row.click()
    expect(page.locator('#sdk-input')).to_be_visible()
    uid = page.evaluate('S.sel')
    assert '~' in uid and uid != fixture.uid
    wait_for_async(page,"async uid => (await (await fetch('/api/live')).json()).sdk_uids.includes(uid)",arg=uid)
    page.locator('#sdk-input').fill('approval')
    page.locator('#sdk-send').click()
    card = page.locator('[data-sdk-request]').filter(has_text='printf synthetic')
    expect(card).to_be_visible()
    with page.expect_response(lambda response: '/api/session/conversation' in response.url and response.request.method=='POST' and response.ok):
        page.locator('#sdk-input').fill('Hub draft 中文 retained across refresh')
    page.reload(wait_until='domcontentloaded')
    expect(page.locator('#sdk-input')).to_have_value('Hub draft 中文 retained across refresh')
    expect(page.locator('[data-sdk-request]')).to_contain_text('printf synthetic')
    page.locator('[data-sdk-request]').get_by_role('button',name='拒绝',exact=True).click()
    expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final approval')
    page.locator('#sdk-input').fill('question')
    page.locator('#sdk-send').click()
    expect(page.locator('[data-sdk-request]')).to_be_visible()
    question = page.locator('[data-sdk-request]')
    question.locator('input[type=radio]').first.check()
    question.get_by_role('button',name='提交',exact=True).click()
    expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final question')
    def lose_ack(route):
        route.fetch()
        route.abort('failed')
    page.route('**/api/native-sdk/turn',lose_ack,times=1)
    lost_text = f'Hub acknowledged input with lost response {width}-{theme}'
    with page.expect_event('requestfailed',predicate=lambda request: '/api/native-sdk/turn' in request.url):
        page.locator('#sdk-input').fill(lost_text)
        page.locator('#sdk-send').click()
        expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final '+lost_text)
    page.reload(wait_until='domcontentloaded')
    expect(page.locator('#sdk-input')).to_be_enabled()
    expect(page.locator('#sdk-input')).to_have_value('')
    page.locator('#sdk-add').click()
    with page.expect_file_chooser() as chooser:
        page.locator('#sdk-attach-menu [data-attach=file]').click()
    chooser.value.set_files({'name':'native-sdk-附件.txt','mimeType':'text/plain','buffer':b'synthetic SDK attachment'})
    expect(page.locator('#sdk-compose-items')).to_contain_text('native-sdk-附件.txt')
    attachment_text = f'Hub attachment retry {width}-{theme}'
    page.locator('#sdk-input').fill(attachment_text)
    try:
        with page.expect_response(lambda response: '/api/native-sdk/turn' in response.url and response.request.method=='POST') as sent:
            page.locator('#sdk-send').click()
    except Exception:
        print('SDK attachment diagnostics',page.locator('.sdk-status').inner_text(),page.evaluate('''() => {
            const state=CodexSdkCompose.states.get(NativeSdkView),d=state?.draft;
            return {input:NativeSdkView.input?.value,sending:state?.sending,text:d?.text,loading:d?.loading,loadFailed:d?.loadFailed,
                error:d?.storageError,files:d?.attachments?.map(a=>({name:a.name,staging:!!a.staging,error:a.uploadError,uploaded:a.uploaded}))};
        }'''),flush=True)
        raise
    assert sent.value.ok, sent.value.text()
    expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final '+attachment_text)
    before_retry = len(fixture.frames('user'))
    retried = page.request.post(base+'/api/native-sdk/turn',data=sent.value.request.post_data_json)
    assert retried.ok, retried.text()
    assert len(fixture.frames('user')) == before_retry, 'attachment retry submitted a second native input'
    native = [frame for frame in fixture.frames('user') if str(frame.get('message',{}).get('content','')).startswith('Hub attachment retry')]
    matching = [frame for frame in native if 'sessiondock_attachments' in frame['message']['content']]
    assert matching and len(matching) == len({frame['uuid'] for frame in matching}), native
    if screenshots:
        screenshots.mkdir(parents=True,exist_ok=True)
        page.screenshot(path=str(screenshots/f'hub-{width}-{theme}.png'))
    if page.locator('#detail .mobile-back').is_visible():
        page.locator('#detail .mobile-back').click()
    open_dialog(page)
    page.locator('input[name=new-source][value=claude]').check()
    page.locator('#new-cwd').fill(str(fixture.cwd))
    with page.expect_response(lambda response: '/api/native-sdk/create' in response.url) as created:
        page.locator('#new-session-go').click()
    assert created.value.ok, created.value.text()
    receipt = created.value.json()
    assert '~' in receipt['uid'] and receipt['session']['uid'] == receipt['uid'], receipt
    expect(page.locator('#sdk-input')).to_be_visible()
    with page.expect_response(lambda response: '/api/session/conversation' in response.url and response.request.method=='POST' and response.ok):
        page.locator('#sdk-input').fill('Hub empty create draft')
    # Flush through the user blur/save path before reloading the receipt.
    page.locator('.sdk-status').click()
    page.reload(wait_until='domcontentloaded')
    expect(page.locator('#sdk-input')).to_have_value('Hub empty create draft')
    assert page.evaluate('S.sel') == receipt['uid']
    page.locator('#sdk-input').fill('Hub first native input')
    page.locator('#sdk-send').click()
    expect(page.locator('.sdk-live')).to_contain_text('Claude synthetic final Hub first native input')
    page.reload(wait_until='domcontentloaded')
    expect(page.locator('#sdk-input')).to_be_enabled()
    expect(page.locator('#sdk-input')).to_have_value('')
    assert not list((fixture.root/'host').glob('sessiondock-*.json'))
    assert not errors, errors
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--hub-binary',type=Path)
    parser.add_argument('--screenshots',type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-sdk-hub-') as temporary:
        root = Path(temporary)
        fixture = ClaudeFixture(root)
        port = free_port()
        environment = node_env(root,port,'127.0.0.0/8')
        with fixture.server(args.binary,environment):
            hub_root = root/'hub'; hub_root.mkdir()
            hub = Hub((args.hub_binary or args.binary.with_name('sessiondock-hub')).resolve(),hub_root,
                      [SimpleNamespace(name='NativeSdkNode',token=TOKEN,port=port)])
            hub.start()
            try:
                with sync_playwright() as pw:
                    browser = pw.chromium.launch(headless=True)
                    try:
                        for width,theme in [(1280,'light'),(390,'dark'),(1280,'dark'),(390,'light')]:
                            print(f'Hub native SDK: {width} {theme}',flush=True)
                            exercise(browser,f'http://127.0.0.1:{hub.port}',fixture,width,theme,args.screenshots)
                    finally:
                        browser.close()
            finally:
                hub.stop()
    print('PASS native SDK Hub: create/namespace, drafts/reload, attachments/retry, streaming, approvals/questions, no PTY')


if __name__ == '__main__':
    main()
