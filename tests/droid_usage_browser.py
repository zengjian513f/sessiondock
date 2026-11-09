#!/usr/bin/env python3
"""Droid ctx, cumulative tokens, credits and native account quotas via Chromium."""
import tempfile
from pathlib import Path
from playwright.sync_api import sync_playwright, expect
from droid_sdk_fixtures import DroidFixture
from droid_usage_fixtures import billing
from history_fixtures import BINARY
from claude_sdk_browser import open_sdk, send, wait_snapshot


def check(page,base,f,state,out,hub=False):
    page.goto(base,wait_until='networkidle')
    if hub:
        row=page.locator('#side .item[data-uid^="droid:"]').first
        expect(row).to_be_visible();f.uid=row.get_attribute('data-uid')
    open_sdk(page,f,expect)
    gauges=page.locator('.native-sdk-pane .sdk-gauges')
    expect(gauges).to_contain_text('上下文')
    expect(gauges).to_contain_text('Standard')
    assert state['calls']==1,state
    send(page,'usage probe')
    wait_snapshot(page,f.uid,lambda r:not r['running'] and bool(r.get('usage')))
    expect(gauges).to_contain_text('1.25 Credits')
    description=gauges.get_attribute('aria-description')
    for text in ['1,200','50','20','300','40','12,000','150,000','原生估计','重置剩余']:
        assert text in description,(text,description)
    # A per-turn completion must not replace the cumulative notification.
    assert '1,250' in description,description
    page.screenshot(path=str(out/'sdk-wide-light.png'),full_page=True)
    (f.root/'context-fail').touch();send(page,'/usage')
    expect(gauges).to_have_attribute('aria-description',__import__('re').compile('上下文查询失败'))
    assert not any(frame.get('params',{}).get('text')=='/usage' for frame in f.frames())
    (f.root/'context-fail').unlink()
    page.set_viewport_size({'width':390,'height':844});page.emulate_media(color_scheme='dark')
    open_sdk(page,f,expect)
    page.screenshot(path=str(out/'sdk-mobile-dark.png'),full_page=True)
    page.set_viewport_size({'width':1280,'height':900});page.emulate_media(color_scheme='light')
    page.goto(base+'/quotas.html',wait_until='networkidle')
    table=page.get_by_role('table',name='Droid',exact=True)
    expect(table).to_contain_text('Standard');expect(table).to_contain_text('Core')
    expect(table).to_contain_text('17%');expect(table).to_contain_text('月度');expect(table).to_contain_text('N/A')
    state['used']=28
    page.locator('#quota-refresh').click();expect(table).to_contain_text('28%')
    page.screenshot(path=str(out/'quotas-wide-light.png'),full_page=True)
    state['fail']=True
    page.locator('#quota-refresh').click();expect(page.locator('[data-source=droid]').first).to_contain_text('查询失败')
    expect(table).to_contain_text('28%')
    page.set_viewport_size({'width':390,'height':844});page.emulate_media(color_scheme='dark');page.reload(wait_until='networkidle')
    expect(table).to_contain_text('28%')
    page.screenshot(path=str(out/'quotas-mobile-dark.png'),full_page=True)
    scroll=page.get_by_role('region',name='Droid 额度表，可横向滚动')
    scroll.hover();page.mouse.wheel(1000,0)
    expect(table.get_by_role('columnheader',name='重置倒计时')).to_be_in_viewport()
    page.screenshot(path=str(out/'quotas-mobile-dark-scrolled.png'),full_page=True)
    saved=(f.root/'state/quotas/droid.json').read_text()
    assert 'SYNTHETIC-DROID' not in saved and 'access_token' not in saved
    assert not any(frame.get('params',{}).get('text','').startswith('/') for frame in f.frames())


def main():
    out=Path('/tmp/sessiondock-droid-usage-browser');out.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-droid-usage-') as root:
        f=DroidFixture(root)
        with billing(f) as state,f.server(BINARY) as base,sync_playwright() as p:
            browser=p.chromium.launch(headless=True)
            page=browser.new_page(viewport={'width':1280,'height':900})
            check(page,base,f,state,out);browser.close()
    from node_auth_fixtures import node_env, free_port, TOKEN
    from hub_fixtures import Hub
    from types import SimpleNamespace
    with tempfile.TemporaryDirectory(prefix='sessiondock-droid-usage-hub-') as root:
        f=DroidFixture(root);port=free_port()
        with billing(f) as state,f.server(BINARY,node_env(Path(root),port,'127.0.0.0/8')):
            hubroot=Path(root)/'hub';hubroot.mkdir()
            hub=Hub(BINARY.with_name('sessiondock-hub'),hubroot,[SimpleNamespace(name='DroidNode',token=TOKEN,port=port)])
            hub.start()
            try:
                with sync_playwright() as p:
                    browser=p.chromium.launch(headless=True);page=browser.new_page(viewport={'width':1280,'height':900})
                    hubout=out/'hub';hubout.mkdir(exist_ok=True)
                    check(page,f'http://127.0.0.1:{hub.port}',f,state,hubout,hub=True);browser.close()
            finally:hub.stop()
    print('Droid usage Chromium node + Hub: PASS')

if __name__=='__main__':main()
