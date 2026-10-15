#!/usr/bin/env python3
"""Deletion/cancellation/retry and empty cleanup via private real backend."""
import argparse,json,tempfile
from pathlib import Path
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,build_corpus,isolated_server
from remaining_ui_sidebar_browser import header
from browser_evidence_fixtures import run_with_evidence


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/lifecycle'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual,width):
  cases.append({'ids':ids.split(),'actual':actual,'width':width});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 with tempfile.TemporaryDirectory(prefix='sd-remaining-life-') as tmp,sync_playwright() as pw:
  root=Path(tmp);c=build_corpus(root);(root/'state').mkdir();(root/'trash').mkdir()
  with isolated_server(c,a.binary,state_dir=root/'state',trash_dir=root/'trash') as (base,_):
   browser=pw.chromium.launch(headless=True)
   try:
    for width,sid in [(1280,'claude-abandoned'),(390,'claude-compact')]:
     context=browser.new_context(viewport={'width':width,'height':850});context.add_init_script('Object.defineProperty(navigator,"standalone",{value:true})');page=context.new_page()
     failure={'on':True}
     def fail(route):
      if failure['on']:route.fulfill(status=503,json={'error':'private list failure'})
      else:route.continue_()
     page.route('**/api/sessions?*',fail);page.route('**/api/sessions',fail);page.goto(base,wait_until='networkidle')
     expect(page.locator('.load-retry')).to_be_visible();failure['on']=False;page.locator('.load-retry').click();expect(page.locator('#side .item[data-uid]').first).to_be_visible()
     done('sd.sidebar.retry','503 initial list then real retry button restores real native rows',width)
     row=page.locator(f'#side .item[data-uid="{c.uid(sid)}"]');text=row.locator('.t');box=text.bounding_box()
     previous=page.url;page.mouse.move(box['x']+4,box['y']+box['height']/2);page.mouse.down();page.mouse.move(box['x']+min(90,box['width']-5),box['y']+box['height']/2,steps=10);page.mouse.up()
     assert page.evaluate('getSelection().toString()').strip();assert page.url==previous
     done('sd.sidebar.text-select','Actual text drag preserves selection and does not navigate',width)
     page.locator('#q').click();row.click(button='right');page.locator('#item-menu [data-act=delete]').click();popup=page.locator('dialog.app-popup');expect(popup).to_be_visible();popup.locator('.modal-close').click();expect(popup).to_have_count(0);assert c.paths[sid].exists()
     done('sd.shell.popup-close','Delete confirmation X cancels; original native history remains',width)
     row.click(button='right');page.locator('#item-menu [data-act=delete-tree]').click();expect(page.locator('#delete-tree-dialog')).to_be_visible();page.keyboard.press('Escape');expect(page.locator('#delete-tree-dialog')).to_have_count(0);assert c.paths[sid].exists()
     done('sd.tree-delete.escape','Tree-delete preview Escape closes without native deletion',width)
     row.click();delete=page.locator('#a-session-action')
     if not delete.is_visible():page.locator('#a-more').click()
     delete.click();expect(popup).to_be_visible();popup.get_by_role('button',name='取消',exact=True).click();assert c.paths[sid].exists()
     done('sd.lifecycle.delete','Actual title action enters delete confirmation; cancel preserves history',width)
     if width<720:page.locator('.mobile-back').click()
     row.click(button='right');page.locator('#item-menu [data-act=delete]').click();popup.get_by_role('button',name='确定',exact=True).click();expect(row).to_have_count(0)
     assert not c.paths[sid].exists();assert list((root/'trash').rglob('manifest.json'))
     done('sd.sidebar.menu-delete','Row-menu confirmed delete removes native origin and persists trash manifest',width)
     if width<720 and page.locator('.mobile-back').is_visible():page.locator('.mobile-back').click()
     header(page,'#session-cleanup');expect(page.locator('#session-cleanup-dialog')).to_be_visible();expect(page.locator('#session-cleanup-close')).to_be_enabled()
     expect(page.locator('#session-cleanup-start')).to_be_disabled();page.locator('#session-cleanup-close').click();expect(page.locator('#session-cleanup-dialog')).to_be_hidden()
     header(page,'#session-cleanup');expect(page.locator('#session-cleanup-close')).to_be_enabled();page.keyboard.press('Escape');expect(page.locator('#session-cleanup-dialog')).to_be_hidden()
     done('sd.lifecycle.cleanup-open sd.lifecycle.cleanup-close sd.lifecycle.cleanup-escape','Real empty cleanup preview, disabled start, cancel and Escape',width)
     header(page,'#settings');page.locator('.settings-tab[data-tab=appearance]').click()
     install=page.locator('[data-pwa-install-button]');expect(install).to_be_disabled();assert install.get_attribute('title')
     page.keyboard.press('Escape')
     done('sd.shell.pwa-install','Simulated standalone state disables installation control with its reason. Native installation UI not exercised.',width)
     before=page.evaluate('performance.timeOrigin')
     with page.expect_navigation():header(page,'#page-reload')
     assert page.evaluate('performance.timeOrigin')!=before
     done('sd.toolbar.reload','Actual application reload button creates new document in simulated standalone environment; native install not claimed',width)
     page.screenshot(path=str(a.output/f'lifecycle-{width}.png'));context.close()
   finally:browser.close()
 print('PASS remaining lifecycle cancellation/retry paths',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/lifecycle')
