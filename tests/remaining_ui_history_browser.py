#!/usr/bin/env python3
"""User-controlled history cancellation/reload and network recovery branches."""
import argparse,json,tempfile
from pathlib import Path
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,Corpus,codex_row,codex_message,isolated_server
from browser_evidence_fixtures import run_with_evidence


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/history'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual,width):
  cases.append({'ids':ids.split(),'actual':actual,'width':width});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 with tempfile.TemporaryDirectory(prefix='sd-remaining-history-') as tmp,sync_playwright() as pw:
  root=Path(tmp);c=Corpus(root)
  for n in ['claude','codex','grok']:(root/n).mkdir()
  sid='remaining-pages';c.put(sid,'codex',[codex_row('session_meta',{'id':sid,'cwd':str(root)}),*[codex_message('user',f'PAGE ROW {i:04d}',i+1) for i in range(1200)]],[])
  with isolated_server(c,a.binary,extra_env={'SESSIONDOCK_HISTORY_PAGE_EVENTS':'100'}) as (base,_):
   browser=pw.chromium.launch(headless=True)
   try:
    for width in [1280,390]:
     context=browser.new_context(viewport={'width':width,'height':850});page=context.new_page();page.goto(base,wait_until='networkidle');uid=c.uid(sid)
     mode={'read':True}
     def read(route):
      if mode['read']:route.fulfill(status=503,json={'error':'isolated migration read unavailable','code':'not_implemented'})
      else:route.continue_()
     page.route('**/api/messages/*?window=1',read);page.locator(f'#side .item[data-uid="{uid}"]').click();retry=page.get_by_role('button',name='重试读取',exact=True);expect(retry).to_be_visible();mode['read']=False;retry.click();expect(page.locator('#msgs')).to_contain_text('PAGE ROW 1199')
     done('sd.conversation.migration-retry','Actual 503 first-read notice; explicit retry returns real native history',width)
     gap=page.locator('.history-gap-load');held=[];page.route('**/api/messages/*/page?*',lambda r:held.append(r));gap.click();expect(gap).to_contain_text('中止');before=page.locator('#msgs .msg').count();gap.click()
     for r in held:r.abort()
     page.unroute('**/api/messages/*/page?*');expect(gap).not_to_contain_text('中止');assert page.locator('#msgs .msg').count()==before
     done('sd.conversation.history-cancel','Second click cancels pending page load; existing rendered messages preserved',width)
     page.route('**/api/messages/*/page?*',lambda r:r.fulfill(status=409,json={'error':'isolated stale cursor'}));gap.click();expect(page.locator('.history-gap-reload')).to_be_visible();page.unroute('**/api/messages/*/page?*');page.locator('.history-gap-reload').click();expect(page.locator('.history-gap-reload')).to_have_count(0);expect(page.locator('#msgs')).to_contain_text('PAGE ROW 1199')
     done('sd.conversation.history-reload','409 page error then actual reload fetches current real history and clears error',width)
     context.set_offline(True);expect(page.locator('#network-status')).to_be_visible();expect(page.locator('#msgs')).to_contain_text('PAGE ROW 1199');page.locator('#network-status').get_by_role('button',name='立即重连').click();expect(page.locator('#network-status')).to_be_visible()
     context.set_offline(False)
     expect(page.locator('#network-status')).to_have_count(0,timeout=15000);expect(page.locator('#msgs')).to_contain_text('PAGE ROW 1199')
     done('sd.conversation.network-retry','Offline retry preserves rendered history; online reconnect recovers network notice',width)
     page.screenshot(path=str(a.output/f'history-{width}.png'));context.close()
     context=browser.new_context(viewport={'width':width,'height':850});page=context.new_page();page.route('**/api/sessions*',lambda r:r.fulfill(status=401,json={'error':'isolated login expired'}));page.goto(base,wait_until='networkidle');expect(page.locator('.login-expired')).to_be_visible()
     with context.expect_page() as opened:page.locator('.login-expired').get_by_role('button',name='打开登录页').click()
     popup=opened.value;popup.wait_for_load_state();assert popup.url.startswith(base),popup.url;popup.close();expect(page.locator('.login-expired')).to_be_visible()
     done('sd.conversation.login','401 guard displays login; actual button opens local auth entry in new tab and retains original view',width);context.close()
   finally:browser.close()
 print('PASS remaining history recovery inputs',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/history')
