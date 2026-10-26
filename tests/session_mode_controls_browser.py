"""Mode-specific menu/header controls, exclusive switch consent and independent stops."""
import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace
from playwright.sync_api import sync_playwright, expect
from history_fixtures import BINARY
from sdk_start_stop_fixtures import provider
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from private_hosts import private_hosts
from session_start_fixtures import choose_mode


def exercise(browser, base, source, shots, label):
    for width, theme in [(1280,'light'),(390,'dark')]:
        print(label,source,width,flush=True)
        context=browser.new_context(viewport={'width':width,'height':900},color_scheme=theme,service_workers='block')
        page=context.new_page();page.set_default_timeout(20000)
        errors=[];writes=[]
        page.on('pageerror',lambda e:errors.append(str(e)))
        page.on('request',lambda r:writes.append((r.url.rsplit('/',1)[-1],r.post_data_json))
            if r.method=='POST' and any(r.url.endswith('/'+s) for s in ('stop','takeover','open')) else None)
        try:
            page.goto(base,wait_until='networkidle');page.wait_for_function('T.listLoaded')
            choose_mode(page,source,'cli');page.locator('#settings-dialog .modal-close').click()
            card=page.locator(f'#side .item[data-uid^="{source}:"]').first
            if not card.is_visible():page.locator(f'#chips button[data-source="{source}"]').click()
            uid=card.get_attribute('data-uid')
            card=page.locator(f'#side .item[data-uid="{uid}"]')
            def menu(action):
                if page.locator('#detail .mobile-back').is_visible():page.locator('#detail .mobile-back').click()
                card.click(button='right')
                button=page.locator(f'#item-menu [data-act="{action}"]')
                expect(button).not_to_have_attribute('aria-disabled','true')
                expect(page.locator('#item-menu [data-run-mode]')).to_have_count(2)
                page.screenshot(path=str(shots/f'{label}-{source}-{width}-{action}-menu.png'))
                return button
            def power():
                button=page.locator('#a-session-toggle')
                if not button.is_visible():page.locator('#a-more').click()
                return button
            def stopped(mode):page.wait_for_function('mode => !sessionModeActive(sessionRunRow(S.sel),mode) && !ConsoleUI.busy.has(S.sel)',arg=mode)
            def started(mode):page.wait_for_function('mode => sessionModeActive(sessionRunRow(S.sel),mode) && !ConsoleUI.busy.has(S.sel)',arg=mode)
            menu('start-sdk')
            expect(page.locator('#item-menu [data-run-mode]')).to_have_count(2)
            for mode in ('sdk','cli'):
                expect(page.locator(f'#item-menu [data-run-mode="{mode}"]')).to_have_text(f'启动{mode.upper()}模式')
                expect(page.locator(f'#item-menu [data-act="stop-{mode}"]')).to_have_count(0)
            page.screenshot(path=str(shots/f'{label}-{source}-{width}-idle-menu.png'))
            page.locator('#item-menu [data-act="start-sdk"]').click();started('sdk')
            expect(page.locator('#a-sdk')).to_have_attribute('aria-pressed','true')
            expect(power()).to_have_attribute('data-run-mode','sdk')
            if source == 'codex':
                power().click()
                popup=page.locator('dialog.app-popup[open]')
                if popup.is_visible():popup.locator('[data-popup-action="ok"]').click()
                stopped('sdk')
                # The default is CLI, but the SDK header starts SDK again.
                power().click();started('sdk')
                assert writes[-1][0] == 'open',writes
            # Start CLI despite the SDK being active, cancel before any mutation.
            action=menu('start-cli');before=len(writes);action.click()
            exclusive=source in ('claude','grok','agy','kiro','codebuddy')
            dialog=page.locator('dialog.app-popup[open]')
            if exclusive:
                expect(dialog).to_contain_text('启动CLI模式会先停止当前SDK模式')
                # SDK view reconnects can issue /open; cancellation must not stop SDK or launch CLI.
                assert not any(path in ('stop','takeover') for path,_ in writes[before:]),writes
                page.screenshot(path=str(shots/f'{label}-{source}-{width}-switch-confirm.png'))
                dialog.locator('[data-popup-action="cancel"]').click()
                started('sdk');assert not any(path in ('stop','takeover') for path,_ in writes[before:]),writes
                if source == 'claude' and label == 'node' and width == 1280:
                    for response in ({'error':'Synthetic mode stop failure'}, {'ok':True,'stopped':False,'stage':'uncertain'}):
                        page.route('**/api/native-sdk/stop',lambda route, *, response=response:
                            route.fulfill(status=503 if 'error' in response else 200,json=response),times=1)
                        start_at=len(writes)
                        menu('start-cli').click();dialog.locator('[data-popup-action="ok"]').click()
                        expect(dialog).to_contain_text('启动失败')
                        assert [path for path,_ in writes[start_at:]] == ['stop'],writes
                        dialog.locator('[data-popup-action="ok"]').click();started('sdk')
                menu('start-cli').click();dialog.locator('[data-popup-action="ok"]').click()
            started('cli')
            expect(page.locator('#a-term')).to_have_attribute('aria-pressed','true')
            assert page.evaluate('sessionModeActive(sessionRunRow(S.sel),"sdk")') is (not exclusive)
            # Current CLI power stops only CLI; SDK stays loaded for dual sources.
            expect(power()).to_have_attribute('data-run-mode','cli');power().click()
            if dialog.is_visible():dialog.locator('[data-popup-action="ok"]').click()
            stopped('cli')
            assert page.evaluate('sessionModeActive(sessionRunRow(S.sel),"sdk")') is (not exclusive)
            assert any(path=='stop' and body.get('mode')=='cli' for path,body in writes),writes
            # CLI header start must ignore the default SDK preference.
            if page.locator('#detail .mobile-back').is_visible():page.locator('#detail .mobile-back').click()
            choose_mode(page,source,'sdk');page.locator('#settings-dialog .modal-close').click();card.click()
            if page.locator('#a-chat').is_visible():
                page.locator('#a-chat').click()
                expect(power()).to_have_attribute('data-run-mode','cli');power().click()
            else:
                # Terminal entry can itself launch; use the explicit menu for SDK-only sources.
                menu('start-cli').click()
            started('cli')
            # Starting SDK either coexists, or requires stopping CLI first.
            if exclusive:
                before=len(writes);menu('start-sdk').click()
                expect(dialog).to_contain_text('启动SDK模式会先停止当前CLI模式')
                assert len(writes)==before
                dialog.locator('[data-popup-action="cancel"]').click();started('cli')
                menu('start-sdk').click();dialog.locator('[data-popup-action="ok"]').click();started('sdk')
                stopped('cli')
            else:
                menu('stop-sdk').click()
                if dialog.is_visible():dialog.locator('[data-popup-action="ok"]').click()
                stopped('sdk');started('cli')
                menu('start-sdk').click();started('sdk');started('cli')
            expect(power()).to_have_attribute('data-run-mode','sdk')
            page.screenshot(path=str(shots/f'{label}-{source}-{width}-sdk-header.png'))
            power().click()
            if dialog.is_visible():dialog.locator('[data-popup-action="ok"]').click()
            stopped('sdk')
            if not exclusive:
                started('cli');menu('stop-cli').click()
                if dialog.is_visible():dialog.locator('[data-popup-action="ok"]').click()
                stopped('cli')
            assert not errors,errors
        except Exception:
            print('FAIL state',errors,writes,page.evaluate('({sel:S.sel,mode:SessionContentView.kind,sdk:[...S.liveSdk],cli:[...S.liveTmux],status:SdkView.notice?.textContent,dialogs:[...document.querySelectorAll("dialog[open]")].map(x=>x.textContent)})'),flush=True)
            page.screenshot(path=str(shots/f'{label}-{source}-{width}-failure.png'))
            raise
        finally:context.close()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--sources',default='codex,claude,grok,agy,opencode,kiro,codebuddy')
    parser.add_argument('--screenshots',type=Path,default=Path(tempfile.gettempdir())/'sessiondock-mode-controls')
    args=parser.parse_args();args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-mode-controls-') as tmp,sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root=Path(tmp)/source;root.mkdir()
                with private_hosts(root,hosts=('menu-host',)),provider(source,root,args.binary,cli='shared') as (base,peer,extra):
                    exercise(browser,base,source,args.screenshots,'node')
                    hubroot=root/'hub';hubroot.mkdir()
                    hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hubroot,[SimpleNamespace(name='Modes',token=TOKEN,port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':',1)[1]))])
                    hub.start()
                    try:exercise(browser,f'http://127.0.0.1:{hub.port}',source,args.screenshots,'hub')
                    finally:hub.stop()
        finally:browser.close()
    print('PASS mode controls: explicit menus, current-mode header, switch cancel/confirm, independent CLI/SDK stops')
if __name__=='__main__':main()
