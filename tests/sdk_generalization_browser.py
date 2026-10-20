#!/usr/bin/env python3
"""SDK parity regressions through Chromium and private native protocol peers."""
import argparse
import re
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from native_sdk_footer_browser import provider
from sdk_ui_fixtures import start_sdk
from popups import on_popup


def open_view(page, base, source):
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    row=page.locator(f'#side .item[data-uid^="{source}:"]').first
    if not row.is_visible():
        page.locator(f'#chips button[data-source="{source}"]').click()
    row.click()
    page.locator('#a-sdk').click()
    start_sdk(page)
    return page.evaluate('NativeSdkView.uid')


def send(page, text):
    page.locator('#sdk-input').fill(text)
    page.locator('#sdk-send').click()

def new_catalog(page, source):
    send(page,'/new')
    expect(page.locator('#new-session-dialog')).to_be_visible()
    page.locator(f'#new-session-form input[value="{source}"]').check()
    page.locator('#new-model').click()
    expect(page.locator('#new-model-options .new-model-option').first).to_be_visible()
    page.keyboard.press('Escape')
    page.keyboard.press('Escape')
    expect(page.locator('#new-session-dialog')).not_to_be_visible()


def photograph(page, folder, source, state):
    for width, theme in [(1440,'light'),(390,'dark')]:
        page.set_viewport_size({'width':width,'height':900})
        page.emulate_media(color_scheme=theme)
        page.evaluate('() => new Promise(done=>requestAnimationFrame(()=>requestAnimationFrame(done)))')
        if not page.locator('.sdk-pane').is_visible():
            if page.locator('#side .item.sel').is_visible():
                page.locator('#side .item.sel').click()
            else:
                page.locator('#a-sdk').click()
        expect(page.locator('.sdk-pane')).to_be_visible()
        page.screenshot(path=str(folder/f'{source}-{state}-{width}-{theme}.png'),full_page=True)
    page.set_viewport_size({'width':1440,'height':900})
    page.emulate_media(color_scheme='light')


def exercise(browser, base, source, root, screenshots):
    context=browser.new_context(viewport={'width':1440,'height':900},service_workers='block')
    context.route('**/*',lambda route:route.continue_() if route.request.url.startswith(base+'/') else route.abort())
    page=context.new_page()
    page.set_default_timeout(20000)
    errors=[]
    page.on('pageerror',lambda error:errors.append(str(error)))
    on_popup(page,lambda dialog:dialog.accept())
    uid=open_view(page,base,source)
    status=page.locator('.sdk-pane .sdk-status')
    if source in ('grok','opencode'):
        send(page,'generalize-progress')
        expect(status).to_have_attribute('data-state','retrying')
        expect(status).to_contain_text('正在重试')
        expect(status).to_have_attribute('title',re.compile('synthetic capacity retry'))
        photograph(page,screenshots,source,'retry')
        gate=root/'generalize-release'
        gate.write_text('retry')
        expect(status).to_have_attribute('data-state','maintenance')
        expect(status).to_contain_text('压缩中')
        expect(page.locator('.sdk-live')).not_to_contain_text('auto_compact')
        photograph(page,screenshots,source,'compact')
        gate.write_text('compact')
        if source=='grok':
            expect(page.locator('#sdk-model')).to_have_value('fake-other')
            expect(page.locator('#sdk-effort')).to_have_value('high')
            gate.write_text('model')
        page.wait_for_function('!NativeSdkView.running && NativeSdkView.raw.last_turn?.status==="completed"')
        expect(status).not_to_have_attribute('data-state','retrying')
        expect(page.locator('.sdk-live')).not_to_contain_text('压缩')
    if source in ('grok','claude','opencode'):
        # Reopening must use the native final outcome, rather than turn idle
        # availability into a successful preceding result.
        send(page,{'grok':'wait-for-cancel','claude':'hold','opencode':'slow parity'}[source])
        expect(page.locator('#sdk-stop')).to_be_visible()
        page.locator('#sdk-stop').click()
        page.wait_for_function('!NativeSdkView.running')
        expect(status).to_have_attribute('data-state','warning')
        expect(status).to_contain_text('已中断')
        page.reload(wait_until='domcontentloaded')
        page.wait_for_function('NativeSdkView.connected')
        expect(status).to_contain_text('已中断')
        photograph(page,screenshots,source,'interrupted')
        # /new is a UI command; it must not create a model turn.
        new_catalog(page,source)
        expect(page.locator('#sdk-input')).to_have_value('')
        send(page,'/rename SDK parity renamed')
        expect(page.locator('#sdk-input')).to_have_value('')
        expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_contain_text('SDK parity renamed')
        # Native history menus use UUID/message/prompt identity even after
        # reopen. No text-based boundary is supplied to the adapter.
        if source in ('claude','opencode'):
            toggles=page.locator('.sdk-pane .sdk-bubble-toggle')
            expect(toggles.first).to_be_visible()
            toggles.first.click()
            expect(page.locator('.sdk-bubble-menu:popover-open')).to_contain_text('分支')
            page.keyboard.press('Escape')
    if source in ('grok','claude'):
        send(page,'generalize-error-exit')
        try:
            page.wait_for_function('NativeSdkView.raw.runtime_error && NativeSdkView.raw.last_turn?.status==="failed"')
        except Exception:
            print('result-before-exit',source,page.evaluate('NativeSdkView.raw'),flush=True)
            raise
        raw=page.evaluate('NativeSdkView.raw')
        assert raw['last_turn']['status']=='failed',raw
        assert raw['error_kind']=='turn',raw
        assert 'disconnected' not in raw['error'],raw
        photograph(page,screenshots,source,'result-before-exit')
    if source=='agy':
        send(page,'/rename unavailable')
        expect(status).to_have_attribute('title',re.compile('尚未提供改名'))
        expect(page.locator('#sdk-input')).to_have_value('/rename unavailable')
        new_catalog(page,source)
        photograph(page,screenshots,source,'commands')
    assert not errors,errors
    context.close()
    print(f'PASS {source}: native outcome, protocol status and shared commands',flush=True)

def success_before_exit(browser,base,source,screenshots):
    # Each crash scenario owns a fresh native process. Admission/retry after
    # a crashed transport is covered separately by native_sdk_queue_browser.
    context=browser.new_context(viewport={'width':1440,'height':900},service_workers='block')
    page=context.new_page()
    open_view(page,base,source)
    send(page,'generalize-success-exit')
    try:
        page.wait_for_function('NativeSdkView.raw.runtime_error && NativeSdkView.raw.last_turn?.status==="completed"')
    except Exception:
        print('success-before-exit',source,page.evaluate('NativeSdkView.raw'),flush=True)
        raise
    assert not page.evaluate('NativeSdkView.raw.error')
    photograph(page,screenshots,source,'success-before-exit')
    context.close()
    print(f'PASS {source}: native success retained after process exit',flush=True)


def main():
    expect.set_options(timeout=15000)
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--screenshots',type=Path,default=Path('target/sdk-generalization'))
    parser.add_argument('--sources',default='grok,opencode,claude,agy')
    args=parser.parse_args()
    args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-generalize-') as temporary, sync_playwright() as playwright:
        browser=playwright.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root=Path(temporary)/source
                root.mkdir()
                with provider(source,root,args.binary) as base:
                    exercise(browser,base,source,root,args.screenshots)
                if source in ('grok','claude'):
                    root=Path(temporary)/(source+'-success-exit')
                    root.mkdir()
                    with provider(source,root,args.binary) as base:
                        success_before_exit(browser,base,source,args.screenshots)
        finally:
            browser.close()


if __name__=='__main__':
    main()
