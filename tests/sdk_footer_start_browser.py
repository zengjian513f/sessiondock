"""Native SDK footer start and failed-open retry, never editor clicks."""
import argparse
from contextlib import contextmanager
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import sync_playwright, expect
from codebuddy_sdk_fixtures import CodebuddyFixture
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from sdk_start_stop_fixtures import provider
from session_start_fixtures import choose_mode


@contextmanager
def sdk_only(source, root, binary):
    if source != 'codebuddy':
        with provider(source, root, binary) as data: yield data
    else:
        extra = node_env(root, free_port(), '127.0.0.0/8')
        fixture = CodebuddyFixture(root)
        with fixture.server(binary, extra) as base: yield base, fixture, extra


def exercise(browser, base, source, shots, label):
    for width, theme in [(1280,'light'), (390,'dark')]:
        print('SDK footer',source,label,width,flush=True)
        context=browser.new_context(viewport={'width':width,'height':900},color_scheme=theme,
                                    service_workers='block',has_touch=width<720)
        page=context.new_page();page.set_default_timeout(20000)
        starts, errors = [], []
        page.on('pageerror',lambda error: errors.append(str(error)))
        page.on('request',lambda request: starts.append(request.url)
                if request.method=='POST' and (request.url.endswith('/open') or request.url.endswith('/takeover')) else None)
        page.goto(base,wait_until='networkidle')
        page.wait_for_function('T.listLoaded')
        choose_mode(page,source,'cli')
        page.locator('#settings-dialog .modal-close').click()
        row=page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible():page.locator(f'#chips button[data-source="{source}"]').click()
        uid=row.get_attribute('data-uid');row.click()
        page.locator('#a-sdk').click()
        editor=page.locator('#sdk-input'); action=page.locator('#sdk-composer .composer-run-action')
        expect(action).to_have_text('启动')
        editor.click();editor.fill('启动按钮保留草稿，不自动发送');editor.click()
        assert not starts,starts
        page.screenshot(path=str(shots/f'{label}-{source}-{width}-idle.png'))
        page.route('**/api/native-sdk/open', lambda route: route.fulfill(
            status=503, json={'error':'Synthetic startup failure'}), times=1)
        action.click()
        expect(page.locator('#sdk-composer .sdk-status')).to_contain_text('无法打开')
        expect(action).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        page.wait_for_function('!ConsoleUI.busy.has(S.sel)')
        page.screenshot(path=str(shots/f'{label}-{source}-{width}-failed.png'))
        action.click()
        page.wait_for_function('SdkView.connected && !ConsoleUI.busy.has(S.sel)')
        expect(editor).to_have_value('启动按钮保留草稿，不自动发送')
        expect(editor).to_be_focused();expect(action).to_be_hidden()
        expect(page.locator('#a-sdk')).to_have_attribute('aria-pressed','true')
        assert len(starts)==2 and starts[0].endswith('/api/native-sdk/open'),starts
        page.screenshot(path=str(shots/f'{label}-{source}-{width}-started.png'))
        assert not errors,errors
        result=page.request.post(base+'/api/native-sdk/stop',data={'uid':uid})
        assert result.ok,result.text()
        context.close()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--screenshots',type=Path,default=Path(tempfile.gettempdir())/'sessiondock-sdk-footer-start')
    parser.add_argument('--sources',default='kiro,codebuddy')
    args=parser.parse_args();args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-footer-') as tmp, sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root=Path(tmp)/source;root.mkdir()
                with sdk_only(source,root,args.binary) as (base,peer,extra):
                    exercise(browser,base,source,args.screenshots,'node')
                    hub_root=root/'hub';hub_root.mkdir()
                    hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hub_root,
                        [SimpleNamespace(name='SdkNode',token=TOKEN,port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':',1)[1]))])
                    hub.start()
                    try:exercise(browser,f'http://127.0.0.1:{hub.port}',source,args.screenshots,'hub')
                    finally:hub.stop()
        finally:browser.close()
    print(f'PASS {args.sources} SDK footer: node/Hub, wide/light, narrow/dark, failed-open retry, draft retained')

if __name__=='__main__':main()
