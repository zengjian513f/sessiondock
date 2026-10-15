#!/usr/bin/env python3
"""Actual sidebar/menu/settings inputs; private histories and persistent metadata."""
import argparse
import json
from pathlib import Path
import tempfile
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, build_corpus, isolated_server
from browser_evidence_fixtures import run_with_evidence


def header(page, selector):
    if not page.locator(selector).is_visible():page.locator('#header-more-btn').click()
    page.locator(selector).click()


def main():
    p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY)
    p.add_argument('--output',type=Path,default=Path('target/remaining-ui/sidebar'))
    a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);results=[]
    def done(ids,actual,width):
        results.append({'ids':ids.split(),'actual':actual,'width':width})
        (a.output/'result.json').write_text(json.dumps({'scenarios':results},ensure_ascii=False,indent=2))
    with tempfile.TemporaryDirectory(prefix='sd-remaining-sidebar-') as tmp,sync_playwright() as pw:
        root=Path(tmp);corpus=build_corpus(root);(root/'state').mkdir();(root/'trash').mkdir()
        with isolated_server(corpus,a.binary,state_dir=root/'state',trash_dir=root/'trash') as (base,_):
            browser=pw.chromium.launch(headless=True)
            try:
                for width in [1280,390]:
                    context=browser.new_context(viewport={'width':width,'height':900},has_touch=True,permissions=['clipboard-read','clipboard-write'],color_scheme='light' if width==1280 else 'dark')
                    page=context.new_page();page.goto(base,wait_until='networkidle')
                    page.locator('#allcount').click();page.keyboard.press('ArrowLeft')
                    expect(page.locator('#livecount')).to_be_focused();expect(page.locator('#livecount')).to_have_attribute('aria-checked','true')
                    expect(page.locator('#side .item')).to_have_count(0)
                    page.keyboard.press('ArrowRight');expect(page.locator('#allcount')).to_be_focused()
                    expect(page.locator('#side .item').first).to_be_visible()
                    page.keyboard.press('Tab');page.keyboard.press('Shift+Tab');expect(page.locator('#allcount')).to_be_focused()
                    done('sd.sidebar.scope-keyboard','ArrowLeft/Right switches actual empty/all lists and roving Tab focus',width)
                    row=page.locator('#side .item').first
                    row.click(button='right');expect(page.locator('#item-menu')).to_be_visible()
                    page.keyboard.press('Escape');expect(page.locator('#item-menu')).to_be_hidden()
                    done('sd.sidebar.menu-escape','Right-click then Escape hides row menu',width)
                    row.click(button='right');page.locator('#allcount').click();expect(page.locator('#item-menu')).to_be_hidden()
                    done('sd.sidebar.menu-outside','Outside pointer closes row menu',width)
                    uid=row.get_attribute('data-uid');row.click(button='right')
                    page.locator('#item-menu [data-act=copy-identity]').click()
                    copied=page.evaluate('navigator.clipboard.readText()');assert 'UUID：' in copied and 'agent：' in copied and '目录：' in copied,copied
                    done('sd.sidebar.menu-copy','Actual menu click writes machine/path/source/UUID clipboard',width)
                    row.click(button='right');page.locator('#item-menu [data-act=pick]').click()
                    page.locator('#side-pick-all').click()
                    assert page.locator('#side .item-pick').count()>0
                    assert page.locator('#side .item-pick:not(:checked)').count()==0
                    page.keyboard.press('Escape');expect(page.locator('#side-pick-all')).to_be_hidden()
                    done('sd.sidebar.pick-all sd.sidebar.pick-escape','Menu multi-select, all visible rows selected, Escape leaves pick mode',width)
                    chip=page.locator('#chips button[data-source=claude]');chip.click()
                    expect(chip).to_have_attribute('aria-pressed','false')
                    assert not page.locator('#side .item[data-uid^="claude:"]').count()
                    chip.click();expect(chip).to_have_attribute('aria-pressed','true')
                    done('sd.sidebar.source-click','Agent filter toggles real native rows',width)
                    cdp=context.new_cdp_session(page);box=chip.bounding_box();point={'x':box['x']+box['width']/2,'y':box['y']+box['height']/2}
                    cdp.send('Input.dispatchTouchEvent',{'type':'touchStart','touchPoints':[point]});page.wait_for_timeout(700)
                    cdp.send('Input.dispatchTouchEvent',{'type':'touchEnd','touchPoints':[]})
                    expect(chip).to_have_attribute('aria-pressed','true');expect(page.locator('#chips button[data-source=codex]')).to_have_attribute('aria-pressed','false')
                    assert all(x.startswith('claude:') for x in page.locator('#side .item[data-uid]').evaluate_all('els=>els.map(e=>e.dataset.uid)'))
                    done('sd.sidebar.source-longpress','Trusted touch hold isolates Claude; release does not toggle it off',width)
                    # Settings are preferences: inspect storage and actual reloaded UI.
                    header(page,'#settings')
                    page.locator('.settings-tab[data-tab=appearance]').click()
                    page.locator('#setting-list-path').select_option('tint')
                    assert page.evaluate("localStorage.getItem('sessiondock.timelineDirectoryTintOnly')")=='true'
                    expect(page.locator('body')).to_have_class(__import__('re').compile('cwd-tint-only'))
                    page.locator('#setting-list-path').select_option('full')
                    page.locator('.settings-tab[data-tab=features]').click()
                    field=page.locator('#setting-cleanup-days');field.fill('7');field.press('Tab');expect(field).to_have_value('7')
                    field.fill('0');field.press('Tab');expect(field).to_have_value('7')
                    assert page.evaluate("localStorage.getItem('sessiondock.cleanupDays')")=='7'
                    page.mouse.click(2,2);expect(page.locator('#settings-dialog')).to_be_hidden()
                    page.reload(wait_until='networkidle');header(page,'#settings');expect(field).to_have_value('7')
                    page.locator('.settings-tab[data-tab=appearance]').click();expect(page.locator('#setting-list-path')).to_have_value('full')
                    scale=page.locator('#setting-scale');scale.click();scale.press('Home');scale.press('ArrowRight')
                    page.mouse.click(2,2)
                    done('sd.settings.path sd.settings.cleanup-days sd.settings.backdrop','Path preference updates display; valid days persist, invalid input restores 7; backdrop closes; reload restores',width)
                    # Reset via actual settings control, retaining scale interaction evidence.
                    header(page,'#settings');page.locator('#setting-scale-reset').click();page.mouse.click(2,2)
                    page.locator('#side .item').first.click()
                    page.locator('#a-more').click();expect(page.locator('#session-actions-menu')).to_be_visible()
                    page.keyboard.press('Escape');expect(page.locator('#a-more')).to_be_focused()
                    page.keyboard.press('ArrowDown');expect(page.locator('#session-actions-menu')).to_be_visible()
                    page.keyboard.press('End');assert page.locator('#session-actions-menu').evaluate('e=>e.contains(document.activeElement)')
                    page.keyboard.press('Home');page.keyboard.press('ArrowDown');page.keyboard.press('Escape')
                    expect(page.locator('#session-actions-menu')).to_be_hidden();expect(page.locator('#a-more')).to_be_focused()
                    done('sd.conversation.actions sd.conversation.actions-keyboard','Click opens actions; ArrowDown/Home/End/Escape navigate and restore trigger focus',width)
                    page.screenshot(path=str(a.output/f'sidebar-{width}.png'))
                    context.close()
            finally:browser.close()
    print('PASS remaining sidebar/settings inputs',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/sidebar')
