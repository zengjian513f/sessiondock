#!/usr/bin/env python3
"""Five native SDK rename paths via Chromium, owned CLI peers and real node/Hub."""
import argparse
from contextlib import contextmanager
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import sync_playwright, expect
from sdk_title_rename_fixtures import exercise_title_rename
from history_fixtures import BINARY
from native_sdk_footer_browser import provider
from sdk_generalization_browser import open_view, send
from kiro_sdk_fixtures import KiroFixture
from codex_sdk_browser import server
from send_browser import initialize
from sdkhost_fixtures import stop_private_sdkhosts
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env

@contextmanager
def native(source,root,binary,extra):
    if source!='kiro':
        with provider(source,root,binary,extra) as base:yield base
        return
    for name in ('claude','codex','grok','state'):(root/name).mkdir()
    fixture=KiroFixture(root,binary.with_name('ptyhost'))
    initialize('--initialize-lifecycle',root/'ledger',binary.resolve())
    with server(SimpleNamespace(root=root),binary,'off',{
        **fixture.env,'SESSIONDOCK_NATIVE_SDKS':'auto','SESSIONDOCK_LIFECYCLE_DIR':str(root/'ledger'),
        'SESSIONDOCK_PTYHOST_DIR':str(fixture.host_dir),'SESSIONDOCK_STATE_DIR':str(root/'state'),**extra}) as base:yield base

def exercise(browser,base,source,root,shots,label):
    context=browser.new_context(viewport={'width':1280,'height':900},service_workers='block')
    context.route(lambda url:not url.startswith(base+'/'),lambda route:route.abort())
    page=context.new_page();page.set_default_timeout(20000)
    errors=[];page.on('pageerror',lambda error:errors.append(str(error)))
    turns=[];page.on('request',lambda request:turns.append(request.url) if '/api/native-sdk/turn' in request.url else None)
    uid=open_view(page,base,source)
    box=page.locator('#sdk-input');row=page.locator(f'#side .item[data-uid="{uid}"]')
    title=f'{source} 原生改名 · E203-S003'
    # Browser submission reaches the turn endpoint with leading whitespace intact.
    prompts = []
    def accept_prompt(route):
        prompts.append(route.request.post_data_json)
        route.fulfill(json={'ok':True,'accepted':True,'state':'queued'})
    page.route('**/api/native-sdk/turn', accept_prompt)
    for text in (' /rename name', '\t/model short', '\n/status', '\u3000/unknown-command'):
        send(page, text)
        expect(box).to_have_value('')
        assert prompts[-1]['text'] == text, prompts
    page.unroute('**/api/native-sdk/turn', accept_prompt)
    turns.clear()
    send(page,'/rename '+title)
    expect(box).to_have_value('');expect(row).to_contain_text(title)
    expect(page.locator('.dhead h2')).to_have_text(title)
    # Real reload/inventory, rather than a frontend-only alias.
    page.reload(wait_until='domcontentloaded');page.wait_for_function('NativeSdkView.connected')
    expect(page.locator('.dhead h2')).to_have_text(title)
    send(page,'/rename')
    dialog=page.get_by_role('dialog',name='重命名会话',exact=True)
    field=dialog.get_by_role('textbox',name='会话名称')
    expect(field).to_have_value(title);field.fill('')
    dialog.get_by_role('button',name='保存',exact=True).click();expect(dialog).to_be_visible()
    field.fill(title)
    if shots:
        for width,theme in [(1280,'light'),(390,'dark')]:
            page.set_viewport_size({'width':width,'height':900});page.emulate_media(color_scheme=theme)
            page.screenshot(path=str(shots/f'{label}-{source}-{width}-{theme}-dialog.png'),full_page=True)
        page.set_viewport_size({'width':1280,'height':900});page.emulate_media(color_scheme='light')
    dialog.get_by_role('button',name='取消',exact=True).click();expect(box).to_have_value('/rename')
    send(page,'/rename');field.fill('  '+title+' dialog  ')
    dialog.get_by_role('button',name='保存',exact=True).click()
    expect(box).to_have_value('');expect(page.locator('.dhead h2')).to_have_text(title+' dialog')
    exercise_title_rename(page, title+' dialog', shots, label+'-'+source)
    page.set_viewport_size({'width':1280,'height':900});page.emulate_media(color_scheme='light')
    assert not turns,'rename was submitted as a model turn'
    if source in ('kiro','agy'):
        send(page,'/rename reject rename')
        expect(page.locator('#sdk-status-menu')).to_contain_text('改名失败')
        expect(box).to_have_value('/rename reject rename')
        expect(page.locator('.dhead h2')).to_have_text(title+' dialog')
        if source=='kiro':
            send(page,'/rename unconfirmed rename')
            expect(page.locator('#sdk-status-menu')).to_contain_text('原生未确认改名')
            expect(box).to_have_value('/rename unconfirmed rename')
        # A new draft typed while a native mutation is pending must survive.
        gate=(root/'fake-native-store' if source=='kiro' else root)/'rename-hold';gate.touch()
        send(page,'/rename '+title+' delayed')
        page.wait_for_timeout(100)
        box.fill('new draft while rename pending');gate.unlink()
        expect(page.locator('.dhead h2')).to_have_text(title+' delayed')
        expect(box).to_have_value('new draft while rename pending')
        if source=='agy':(root/'release/rename').unlink(missing_ok=True)
        send(page,'wait-for-cancel' if source=='kiro' else 'hold:rename')
        page.wait_for_function('NativeSdkView.running')
        send(page,'/rename '+title+' busy')
        expect(box).to_have_value('');expect(page.locator('.dhead h2')).to_have_text(title+' busy')
        assert page.evaluate('NativeSdkView.running'),'rename interrupted the current turn'
        if source=='agy':
            rows=[json.loads(line) for line in (root/'native-events.jsonl').read_text().splitlines()]
            turn=[r for r in rows if r.get('kind')=='input'][-1]
            renamed=[r for r in rows if r.get('kind')=='rename'][-1]
            assert turn['pid']==renamed['pid'],(turn,renamed)
            assert not [r for r in rows if r.get('kind') in ('cancel','signal')]
            (root/'release/rename').touch()
        else:page.locator('#sdk-stop').click()
        page.wait_for_function('!NativeSdkView.running')
    if source=='kiro':
        long_title='原生长名称'*25
        send(page,'/rename '+long_title)
        expect(box).to_have_value('')
        expect(page.locator('.dhead h2')).to_have_text(long_title[:77]+'...')
        page.reload(wait_until='domcontentloaded');page.wait_for_function('NativeSdkView.connected')
        expect(page.locator('.dhead h2')).to_have_text(long_title[:77]+'...')
    if label=='node':
        page.evaluate('flushBrowserAudit()')
        page.wait_for_function('browserAuditQueue.length===0')
        records=[json.loads(line) for path in (root/'audit').rglob('*.jsonl') for line in path.read_text().splitlines()]
        events=[r for r in records if r.get('event')=='browser.sdk.rename.finished']
        assert any(r['data'].get('outcome')=='succeeded' for r in events),events
        assert any(r['data'].get('outcome')=='cancelled' for r in events),events
        if source in ('kiro','agy'):assert any(r['data'].get('outcome')=='failed' for r in events),events
    if shots:page.screenshot(path=str(shots/f'{label}-{source}-renamed.png'),full_page=True)
    assert not errors,errors
    context.close();print('PASS',label,source,'inline/dialog, native title, reload, cancellation and input preservation',flush=True)

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--screenshots',type=Path);parser.add_argument('--sources',default='kiro,agy,claude,grok,opencode')
    args=parser.parse_args()
    if args.screenshots:args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-native-rename-') as temp,sync_playwright() as p:
        browser=p.chromium.launch(headless=True)
        for source in args.sources.split(','):
            root=Path(temp)/source;root.mkdir();port=free_port()
            (root/'audit').mkdir(mode=0o700)
            extra={**node_env(root,port,'127.0.0.0/8'),'SESSIONDOCK_AUDIT_DIR':str(root/'audit')}
            try:
                with native(source,root,args.binary,extra) as base:
                    exercise(browser,base,source,root,args.screenshots,'node')
                    hub_root=root/'hub';hub_root.mkdir()
                    hub=Hub(args.binary.with_name('sessiondock-hub').resolve(),hub_root,[SimpleNamespace(name='RenameNode',token=TOKEN,port=port)])
                    hub.start()
                    try:exercise(browser,f'http://127.0.0.1:{hub.port}',source,root,args.screenshots,'hub')
                    finally:hub.stop()
            finally:stop_private_sdkhosts(root)
        browser.close()

if __name__=='__main__':main()
