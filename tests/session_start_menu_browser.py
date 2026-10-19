"""Explicit CLI context submenu: all five sources through private real PTYs."""
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
from popups import on_popup


def exercise(browser, base, source, shots, label):
    for width, theme in ((1280, 'light'), (390, 'dark')):
        print(label, source, width, flush=True)
        context = browser.new_context(viewport={'width':width, 'height':900}, color_scheme=theme,
                                      service_workers='block')
        page = context.new_page()
        page.set_default_timeout(15000)
        on_popup(page, lambda dialog: dialog.accept())
        errors, launches = [], []
        page.on('pageerror', lambda e: errors.append(str(e)))
        page.on('request', lambda r: launches.append(r.post_data_json)
                if r.method == 'POST' and '/api/term/takeover' in r.url else None)
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible(): page.locator(f'#chips button[data-source="{source}"]').click()
        uid = row.get_attribute('data-uid')
        row.click(button='right')
        start = page.locator('#item-menu [data-act="stop"]')
        # Keyboard entry, return, mouse entry and dismissal preserve the parent.
        start.focus(); page.keyboard.press('ArrowRight')
        expect(page.locator('#session-start-menu')).to_be_visible()
        expect(page.locator('[data-start-mode="sdk"]')).to_have_attribute('aria-disabled', 'true')
        page.keyboard.press('Escape')
        expect(page.locator('#session-start-menu')).to_be_hidden()
        expect(page.locator('#item-menu')).to_be_visible()
        start.click()
        page.screenshot(path=str(shots/f'{label}-{source}-{width}-cli-menu.png'))
        with page.expect_response(lambda r: '/api/term/takeover' in r.url) as response:
            page.locator('[data-start-mode="cli"]').click()
        assert response.value.ok, response.value.text()
        page.wait_for_function(r"""() => [...T.views.values()].some(view => {
          const b = view.term?.buffer?.active;
          return b && Array.from({length:b.length},(_,i)=>b.getLine(i)?.translateToString()).join('\n').includes('RS_SHELL_READY');
        })""")
        expect(page.locator('#termpane')).to_be_visible()
        expect(page.locator('#a-term')).to_have_attribute('aria-pressed', 'true')
        assert len(launches) == 1 and launches[0]['uid'] == uid, launches
        assert page.evaluate('SessionContentView.kind') != 'sdk'
        page.screenshot(path=str(shots/f'{label}-{source}-{width}-cli-running.png'))
        if page.locator('#detail .mobile-back').is_visible(): page.locator('#detail .mobile-back').click()
        row.click(button='right')
        expect(start).to_have_text('停止会话')
        start.click()
        page.wait_for_function('uid => !S.liveTmux.has(uid)', arg=uid)
        assert not errors, errors
        context.close()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='codex,claude,grok,agy,opencode')
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-start-cli'))
    args=parser.parse_args(); args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-start-menu-') as temporary, sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root=Path(temporary)/source; root.mkdir()
                with private_hosts(root, hosts=('menu-host',)), provider(source, root, args.binary, cli=True) as (base, peer, extra):
                    exercise(browser, base, source, args.screenshots, 'node')
                    hub_root=root/'hub'; hub_root.mkdir()
                    hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hub_root,
                        [SimpleNamespace(name='MenuNode', token=TOKEN,port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':',1)[1]))])
                    hub.start()
                    try: exercise(browser,f'http://127.0.0.1:{hub.port}',source,args.screenshots,'hub')
                    finally: hub.stop()
        finally: browser.close()
    print('PASS five-source CLI submenu: node/Hub, wide/narrow, keyboard, explicit launch, terminal and stop')

if __name__=='__main__': main()
