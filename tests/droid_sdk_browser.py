#!/usr/bin/env python3
"""Droid read-model and SDK flow through Chromium, using an isolated JSON-RPC peer."""
import argparse
import tempfile
from pathlib import Path
from droid_sdk_fixtures import DroidFixture
from history_fixtures import BINARY
from claude_sdk_browser import post, receipt, snapshot, wait_snapshot, open_sdk, send


def check(fixture,base,screenshots):
    from playwright.sync_api import sync_playwright,expect
    with sync_playwright() as p:
        browser=p.chromium.launch(headless=True)
        page=browser.new_page(viewport={'width':1280,'height':900},color_scheme='light')
        page.goto(base,wait_until='networkidle')
        expect(page.locator(f'#side .item[data-uid="{fixture.uid}"]')).to_be_visible()
        open_sdk(page,fixture,expect)
        expect(page.locator('.sdk-history')).to_contain_text('earlier Droid answer')
        page.locator('#sdk-model').select_option('fake-droid-2')
        expect(page.locator('#sdk-send')).to_be_enabled()
        page.locator('#sdk-effort').select_option('high')
        expect(page.locator('#sdk-send')).to_be_enabled()
        send(page,'hello from browser')
        expect(page.locator('.native-sdk-pane')).to_contain_text('Droid final hello from browser')
        raw=wait_snapshot(page,fixture.uid,lambda r:not r['running'])['raw']
        assert raw['model']=='fake-droid-2' and raw['effort']=='high',raw
        expect(page.locator('.native-sdk-pane').get_by_text('hello from browser',exact=True)).to_have_count(1)
        expect(page.locator('.native-sdk-pane').get_by_text('Droid final hello from browser',exact=True)).to_have_count(1)
        if screenshots:page.screenshot(path=str(screenshots/'droid-wide-light.png'),full_page=True)
        send(page,'approval')
        req=wait_snapshot(page,fixture.uid,lambda r:bool(r['requests']))['raw']['requests'][0]
        assert req['type']=='approval'
        page.locator(f'[data-sdk-request="{req["key"]}"]').get_by_role('button',name='拒绝',exact=True).click()
        wait_snapshot(page,fixture.uid,lambda r:not r['running'])
        send(page,'question')
        req=wait_snapshot(page,fixture.uid,lambda r:bool(r['requests']))['raw']['requests'][0]
        assert req['questions'][0]['question']=='Choose one?'
        card=page.locator(f'[data-sdk-request="{req["key"]}"]');card.get_by_text('Beta',exact=True).click();card.get_by_role('button',name='提交',exact=True).click()
        wait_snapshot(page,fixture.uid,lambda r:not r['running'])
        send(page,'tool')
        raw=wait_snapshot(page,fixture.uid,lambda r:not r['running'] and any(i.get('name')=='Read' for i in r['items']))['raw']
        assert any(i.get('result',{}).get('text')=='synthetic tool output' for i in raw['items']),raw
        send(page,'error')
        raw=wait_snapshot(page,fixture.uid,lambda r:(r.get('last_turn') or {}).get('status')=='failed')['raw']
        assert raw['error']=='Synthetic authentication failure'
        send(page,'hold')
        wait_snapshot(page,fixture.uid,lambda r:r['running'])
        page.reload(wait_until='networkidle');open_sdk(page,fixture,expect)
        assert snapshot(page,fixture.uid)['raw']['running']
        page.locator('#sdk-stop').click()
        wait_snapshot(page,fixture.uid,lambda r:not r['running'])
        page.set_viewport_size({'width':390,'height':844});page.emulate_media(color_scheme='dark');open_sdk(page,fixture,expect)
        if screenshots:page.screenshot(path=str(screenshots/'droid-mobile-dark.png'),full_page=True)
        page.set_viewport_size({'width':1280,'height':900})
        if not page.locator('#new-session').is_visible():page.locator('#header-more-btn').click()
        page.locator('#new-session').click();page.locator('#new-session-form label:has(input[value="droid"])').click()
        page.locator('#new-cwd').fill(str(fixture.cwd))
        with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/create')) as creation:page.locator('#new-session-go').click()
        assert creation.value.ok,creation.value.text()
        created=creation.value.json()
        post(page,'open',{'uid':created['uid']})
        post(page,'turn',{'uid':created['uid'],'request_id':'first-turn','text':'new Droid'})
        wait_snapshot(page,created['uid'],lambda r:not r['running'] and bool(r['items']))
        post(page,'turn',{'uid':created['uid'],'request_id':'first-turn','text':'new Droid'})
        assert sum(f.get('method')=='droid.add_user_message' and f['params']['text']=='new Droid' for f in fixture.frames())==1
        assert sum(f.get('method')=='droid.add_user_message' and f['params']['text']=='hold' for f in fixture.frames())==1
        assert not any(f.get('method')=='droid.add_user_message' and f['params']['text'].startswith('/model') for f in fixture.frames())
        browser.close()


def restart_check(fixture):
    from playwright.sync_api import sync_playwright,expect
    with sync_playwright() as p:
        browser=p.chromium.launch(headless=True)
        with fixture.server(BINARY,preserve_sdkhosts=True) as base:
            page=browser.new_page();page.goto(base,wait_until='networkidle');open_sdk(page,fixture,expect)
            send(page,'hold');wait_snapshot(page,fixture.uid,lambda r:r['running']);page.close()
        with fixture.server(BINARY) as base:
            page=browser.new_page();page.goto(base,wait_until='networkidle');open_sdk(page,fixture,expect)
            assert len(fixture.frames('spawn'))==1
            assert sum(f.get('method')=='droid.add_user_message' for f in fixture.frames())==1
            page.locator('#sdk-stop').click();wait_snapshot(page,fixture.uid,lambda r:not r['running'])
        browser.close()


def hub_check(root):
    from node_auth_fixtures import node_env,free_port,TOKEN
    from hub_fixtures import Hub
    from types import SimpleNamespace
    from playwright.sync_api import sync_playwright,expect
    f=DroidFixture(root);port=free_port();extra=node_env(Path(root),port,'127.0.0.0/8')
    with f.server(BINARY,extra) as base:
        hub_root=Path(root)/'hub';hub_root.mkdir()
        hub=Hub(BINARY.with_name('sessiondock-hub'),hub_root,[SimpleNamespace(name='DroidNode',token=TOKEN,port=port)])
        hub.start()
        try:
            with sync_playwright() as p:
                browser=p.chromium.launch(headless=True);page=browser.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
                row=page.locator('#side .item[data-uid^="droid:"]').first;expect(row).to_be_visible();uid=row.get_attribute('data-uid')
                open_sdk(page,f,expect,uid=uid);send(page,'hub Droid');expect(page.locator('.native-sdk-pane')).to_contain_text('Droid final hub Droid');browser.close()
        finally:hub.stop()


def cli_check(root,screenshots):
    import json
    from private_hosts import private_hosts
    from codebuddy_cli_browser import wait_start
    from playwright.sync_api import sync_playwright,expect
    f=DroidFixture(root);config=json.loads(f.launcher.read_text());config['host_binary']=str(BINARY.with_name('ptyhost'));f.launcher.write_text(json.dumps(config))
    with private_hosts(f.root/'host'),f.server(BINARY) as base,sync_playwright() as p:
        browser=p.chromium.launch(headless=True);page=browser.new_page(viewport={'width':1280,'height':900});page.goto(base,wait_until='networkidle')
        page.locator(f'#side .item[data-uid="{f.uid}"]').click();expect(page.locator('#a-chat')).to_be_hidden()
        page.locator('#a-term').click();expect(page.locator('#termpane')).to_be_visible()
        start=wait_start(f,lambda r:'--resume' in r['args']);assert start['args'][-1]==f.sid
        page.wait_for_function("T.ws?.readyState === WebSocket.OPEN")
        page.locator('#termpane .xterm-helper-textarea').focus();page.keyboard.type('DROID_TERMINAL_INPUT');page.keyboard.press('Enter')
        page.wait_for_function("() => Array.from({length:T.term.buffer.active.length},(_,i)=>T.term.buffer.active.getLine(i)?.translateToString()||'').join(' ').includes('Droid CLI echoed: DROID_TERMINAL_INPUT')")
        if screenshots:page.screenshot(path=str(screenshots/'droid-cli-wide.png'),full_page=True)
        page.request.post(base+'/api/session/stop',data={'uid':f.uid})
        if not page.locator('#settings').is_visible():page.locator('#header-more-btn').click()
        page.locator('#settings').click();page.locator('.settings-tab[data-tab="features"]').click();page.locator('[data-agent-start-mode="droid"]').select_option('cli');page.locator('#settings-dialog .modal-close').click()
        page.locator('#new-session').click();page.locator('#new-session-form label:has(input[value="droid"])').click();page.locator('#new-cwd').fill(str(f.cwd))
        with page.expect_response(lambda r:r.url.endswith('/api/term/create')) as created:page.locator('#new-session-go').click()
        assert created.value.ok,created.value.text()
        wait_start(f,lambda r:'--resume' not in r['args']);expect(page.locator('#termpane')).to_be_visible()
        page.set_viewport_size({'width':390,'height':844});page.emulate_media(color_scheme='dark')
        page.reload(wait_until='networkidle')
        expect(page.locator('dialog.app-popup[open]')).to_have_count(0)
        if page.locator('body').evaluate("e => e.classList.contains('mobile-detail')"):
            expect(page.locator('#detail .mobile-back')).to_be_visible();page.locator('#detail .mobile-back').click()
        if not page.locator('#new-session').is_visible():page.locator('#header-more-btn').click()
        page.locator('#new-session').click();expect(page.locator('#new-session-form label:has(input[value="droid"])')).to_be_visible()
        if screenshots:page.screenshot(path=str(screenshots/'droid-picker-mobile-dark.png'),full_page=True)
        browser.close()


def trash_check(root):
    from trash_interactions_browser import confirm
    from playwright.sync_api import sync_playwright,expect
    f=DroidFixture(root);settings=f.path.with_suffix('.settings.json');settings.write_text('{"modelId":"fake-droid"}');trash=Path(root)/'trash';trash.mkdir()
    with f.server(BINARY,{'SESSIONDOCK_TRASH_DIR':str(trash)}) as base,sync_playwright() as p:
        browser=p.chromium.launch(headless=True);page=browser.new_page();page.goto(base,wait_until='networkidle')
        page.locator(f'#side .item[data-uid="{f.uid}"]').click()
        if not page.locator('#a-session-action').is_visible():page.locator('#a-more').click()
        page.locator('#a-session-action').click();confirm(page,True)
        expect(page.locator('#detail')).to_contain_text('已移入回收站');assert not f.path.exists() and not settings.exists()
        page.locator('#detail-open-trash').click();page.locator('.trash-item [data-act=restore]').click()
        expect(page.locator('#trash-list')).to_contain_text('没有已删除的会话');assert f.path.is_file() and settings.is_file()
        browser.close()


def main():
    ap=argparse.ArgumentParser();ap.add_argument('--screenshots',type=Path);args=ap.parse_args()
    if args.screenshots:args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-droid-browser-') as root:
        f=DroidFixture(root)
        with f.server(BINARY) as base:check(f,base,args.screenshots)
    with tempfile.TemporaryDirectory(prefix='sessiondock-droid-restart-') as root:restart_check(DroidFixture(root))
    with tempfile.TemporaryDirectory(prefix='sessiondock-droid-hub-') as root:hub_check(root)
    with tempfile.TemporaryDirectory(prefix='sessiondock-droid-cli-') as root:cli_check(root,args.screenshots)
    with tempfile.TemporaryDirectory(prefix='sessiondock-droid-trash-') as root:trash_check(root)
    print('Droid SDK Chromium: PASS')


if __name__=='__main__':main()
