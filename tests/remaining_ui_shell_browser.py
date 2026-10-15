#!/usr/bin/env python3
"""Hub chips and shell environment failure controls, with explicit HTTP boundaries."""
import argparse,json,tempfile
from pathlib import Path
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY
from hub_fixtures import FakeNode,Hub
from browser_evidence_fixtures import run_with_evidence


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/shell'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual,width):
  cases.append({'ids':ids.split(),'actual':actual,'width':width});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 nodes=[FakeNode('a'*32,'NodeA'),FakeNode('b'*32,'NodeB')];hub=None
 try:
  with tempfile.TemporaryDirectory(prefix='sd-remaining-shell-') as tmp,sync_playwright() as pw:
   hub=Hub(a.binary.resolve().with_name('sessiondock-hub'),Path(tmp),nodes);hub.start();base=f'http://127.0.0.1:{hub.port}'
   browser=pw.chromium.launch(headless=True)
   try:
    for width in [1280,390]:
     context=browser.new_context(viewport={'width':width,'height':850},has_touch=True);page=context.new_page();requests=[]
     states={n.nid:{'configured':True,'stale':True,'started_at':'fixture-old','changed':['PRIVATE_TEST_FLAG']} for n in nodes}
     def env(route):
      nid=route.request.url.split('/api/nodes/')[1].split('/')[0];route.fulfill(json=states[nid])
     def restart(route):
      requests.append(route.request.url);route.fulfill(status=503,json={'error':'isolated restart refused'})
     page.route('**/api/nodes/*/api/shell-env',env);page.route('**/api/nodes/*/api/shell-env/restart',restart)
     page.goto(base,wait_until='networkidle');notice=page.locator('#shell-env-notice');expect(notice).to_be_visible(timeout=10000)
     notice.get_by_role('button',name='重启 NodeA 后端',exact=True).click();expect(page.locator('.app-popup')).to_contain_text('isolated restart refused');page.locator('.app-popup .modal-close').click()
     assert len(requests)==1 and nodes[0].nid in requests[0]
     done('sd.shell.shell-env-restart','Real per-node restart click; injected 503 shown, only requested node URL attempted; frontend branch evidence',width)
     notice.get_by_role('button',name='全部重启 (2)',exact=True).click();expect(page.locator('.app-popup')).to_contain_text('isolated restart refused');page.locator('.app-popup .modal-close').click()
     for _ in range(100):
      if len(requests)>=3:break
      page.wait_for_timeout(30)
     assert len(requests)==3 and {n.nid for n in nodes}=={u.split('/api/nodes/')[1].split('/')[0] for u in requests[1:]}
     done('sd.shell.shell-env-all','All-restart sends both node requests and retains actionable failure; boundary only, no production restart',width)
     for n in nodes:notice.get_by_role('button',name='忽略 '+n.name,exact=True).click()
     expect(notice).to_have_count(0);assert len(requests)==3
     done('sd.shell.shell-env-ignore','Ignore each node removes notices with no additional restart request',width)
     chip=page.locator(f'#node-chips button[data-node="{nodes[0].nid}"]')
     if not chip.count():chip=page.locator('#node-chips button').first
     box=chip.bounding_box();point={'x':box['x']+box['width']/2,'y':box['y']+box['height']/2};cdp=context.new_cdp_session(page)
     cdp.send('Input.dispatchTouchEvent',{'type':'touchStart','touchPoints':[point]});page.wait_for_timeout(700);cdp.send('Input.dispatchTouchEvent',{'type':'touchEnd','touchPoints':[]})
     expect(chip).to_have_attribute('aria-pressed','true');expect(page.locator('#node-chips button').nth(1)).to_have_attribute('aria-pressed','false')
     done('sd.sidebar.node-longpress','Trusted touch hold isolates one real Hub node scope and release preserves selection',width)
     page.locator('#header-more-btn').click();trigger=page.locator('#header-menu [data-toolbar-menu]').first;trigger.hover();target=trigger.get_attribute('aria-controls');expect(page.locator('#'+target)).to_be_visible();page.keyboard.press('Escape');page.keyboard.press('Escape')
     done('sd.toolbar.submenu-hover','Real hover on toolbar submenu trigger displays its options',width)
     # Disabled source is a current product entry: pointer/keyboard cannot dispatch.
     unavailable=page.locator('#chips button[data-unavailable-reason]').first
     if unavailable.count():
      request_count=[];page.on('request',lambda r:request_count.append(r.url) if r.method=='POST' else None)
      box=unavailable.bounding_box();x=box['x']+box['width']/2;y=box['y']+box['height']/2
      page.mouse.click(x,y);page.mouse.dblclick(x,y);page.mouse.click(x,y,button='right')
      for _ in range(100):
       page.keyboard.press('Tab')
       if unavailable.evaluate('(el)=>el===document.activeElement'):break
      expect(unavailable).to_be_focused()
      assert unavailable.get_attribute('title');assert not request_count
      done('sd.shell.unavailable','Unavailable chip remains focusable with reason; click/double/right click produce no POST',width)
     # The startup meta check reads a distinct served build; it is not invoked directly.
     context.close();context=browser.new_context(viewport={'width':width,'height':850});page=context.new_page()
     def build(route):
      response=route.fetch();data=response.json();data['build']='isolated-new-build';route.fulfill(response=response,json=data)
     page.route('**/api/meta',build);page.goto(base,wait_until='networkidle');expect(page.locator('.version-stale')).to_be_visible();page.locator('.version-stale').get_by_role('button',name='稍后',exact=True).click();expect(page.locator('.version-stale')).to_be_hidden();assert page.evaluate('staleBuildShown')
     done('sd.shell.update-later','Automatic meta check detects changed build; Later hides card without removing stale-send guard',width)
     page.screenshot(path=str(a.output/f'shell-{width}.png'));context.close()
   finally:browser.close()
 finally:
  if hub:hub.stop()
  for n in nodes:n.stop()
 print('PASS remaining shell/hub input branches',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/shell')
