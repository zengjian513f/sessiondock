#!/usr/bin/env python3
"""Real native media pages, failure/retry/reload and browser link entry."""
import argparse,json,tempfile
from pathlib import Path
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,Corpus,codex_row,isolated_server
from media_browser import PNG,image
from browser_evidence_fixtures import run_with_evidence


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/media'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual,width):
  cases.append({'ids':ids.split(),'actual':actual,'width':width});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 with tempfile.TemporaryDirectory(prefix='sd-remaining-media-') as tmp,sync_playwright() as pw:
  root=Path(tmp);c=Corpus(root)
  for n in ['claude','codex','grok']:(root/n).mkdir()
  for sid,count in [('media-many',25),('media-single',1)]:
   c.put(sid,'codex',[codex_row('session_meta',{'id':sid,'cwd':str(root)}),codex_row('response_item',{'type':'message','role':'user','content':[{'type':'input_text','text':sid}]+[image('codex',PNG) for _ in range(count)]})],[])
  with isolated_server(c,a.binary) as (base,_):
   browser=pw.chromium.launch(headless=True)
   try:
    for width in [1280,390]:
     context=browser.new_context(viewport={'width':width,'height':850});page=context.new_page();mode={'status':503}
     def media(route):
      if mode['status']:route.fulfill(status=mode['status'],json={'error':'private media fault'})
      else:route.continue_()
     page.route('**/api/media/*',media);page.goto(base,wait_until='networkidle');page.locator(f'#side .item[data-uid="{c.uid("media-single")}"]').click()
     expect(page.locator('.media-load-retry')).to_be_visible();mode['status']=None;page.locator('.media-load-retry').click();page.wait_for_function('document.querySelector("#msgs img").naturalWidth>0')
     done('sd.conversation.media-retry','Actual image error UI retry fetches real native image after 503 removed',width)
     with context.expect_page() as opened:page.locator('#msgs img').click()
     popup=opened.value;popup.wait_for_load_state();assert '/api/media/' in popup.url;popup.close()
     done('sd.conversation.media-open','Image click opens real authorized image URL in new page',width)
     mode['status']=404;page.reload(wait_until='networkidle');expect(page.locator('.media-load-reload')).to_be_visible();mode['status']=None
     page.locator('.media-load-reload').click();page.wait_for_function('document.querySelector("#msgs img")?.naturalWidth>0')
     done('sd.conversation.media-reload','404 media credential branch then actual reload button recovers fresh real image',width)
     if width<720:page.locator('.mobile-back').click()
     page.locator(f'#side .item[data-uid="{c.uid("media-many")}"]').click();more=page.locator('.media-more');expect(more).to_be_visible()
     before=page.locator('#msgs img').count();cursor=page.evaluate('JSON.stringify([...S.cursors])');more.click();expect(page.locator('#msgs img')).to_have_count(25)
     assert before<25 and page.evaluate('JSON.stringify([...S.cursors])')==cursor
     done('sd.conversation.media-more','Next media page appends all 25 independently seeded images without live cursor change',width)
     page.screenshot(path=str(a.output/f'media-{width}.png'));context.close()
   finally:browser.close()
 print('PASS remaining native media UI',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/media')
