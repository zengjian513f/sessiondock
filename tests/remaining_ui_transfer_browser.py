#!/usr/bin/env python3
"""Real Hub/two nodes: transfer controls, lost reply retry and durable clone."""
import argparse,json,tempfile
from pathlib import Path
from contextlib import ExitStack
from types import SimpleNamespace
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,isolated_server
from session_clone_browser import prepare
from hub_fixtures import Hub,free_port,scoped
from node_auth_fixtures import node_env,TOKEN
from browser_evidence_fixtures import run_with_evidence
from remaining_ui_sidebar_browser import header


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/transfer'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual,width):
  cases.append({'ids':ids.split(),'actual':actual,'width':width});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 with tempfile.TemporaryDirectory(prefix='sd-remaining-transfer-') as tmp,sync_playwright() as pw,ExitStack() as stack:
  root=Path(tmp);source=prepare(root/'source');dest=prepare(root/'dest');nodes=[]
  for i,c in enumerate([source,dest]):
   n=SimpleNamespace(name=['Source','Destination'][i],nid=['a','b'][i]*32,port=free_port(),token=TOKEN);nodes.append(n)
   (c.root/'ids').mkdir();(c.root/'ids/node-id').write_text(n.nid)
   env=node_env(c.root,n.port,'127.0.0.0/8');env['SESSIONDOCK_PROC_ROOT']=str(c.root/'proc')
   stack.enter_context(isolated_server(c,a.binary,state_dir=c.root/'state',extra_env=env))
  (root/'hub').mkdir();hub=Hub(a.binary.resolve().with_name('sessiondock-hub'),root/'hub',nodes);hub.start();stack.callback(hub.stop)
  browser=pw.chromium.launch(headless=True);stack.callback(browser.close)
  original={str(p):p.read_text() for p in source.paths.values()}
  for width in [1280,390]:
   context=browser.new_context(viewport={'width':width,'height':720},has_touch=True);page=context.new_page();page.goto(f'http://127.0.0.1:{hub.port}',wait_until='networkidle')
   selected=scoped(nodes[0].nid,source.uid('a'));row=page.locator(f'#side .item[data-uid="{selected}"]')
   row.hover();page.wait_for_timeout(200);row.click(button='right');page.locator('#item-menu [data-act=clone]').click();dialog=page.locator('#clone-group-dialog');expect(dialog).to_be_visible()
   expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=20000)
   done('sd.sidebar.menu-clone','Actual row context menu opens real six-session backend plan',width)
   picker=dialog.locator('#transfer-target');picker.click();page.keyboard.press('End');page.keyboard.press('Enter')
   expect(picker).to_have_js_property('value',nodes[1].nid)
   picker.press('ArrowDown');page.keyboard.press('Escape');expect(page.locator('#transfer-target-menu')).to_be_hidden();expect(picker).to_be_focused()
   done('sd.transfer.target-keyboard','End/Enter selects other real node; ArrowDown/Escape closes only picker',width)
   dialog.locator('label:has(input[value=move])').click();expect(dialog.locator('input[value=move]')).to_be_checked()
   ids=dialog.locator('#transfer-new-ids');ids.check();expect(ids).to_be_checked();ids.uncheck();expect(ids).not_to_be_checked()
   dialog.locator('label:has(input[value=clone])').click();expect(dialog.locator('input[value=clone]')).to_be_checked()
   expect(ids).to_be_checked()
   done('sd.transfer.mode-copy sd.transfer.mode-move sd.transfer.identity','Switch move/copy modes and mode-specific new-ID choice',width)
   page.screenshot(path=str(a.output/f'plan-{width}.png'))
   dialog.locator('.clone-cancel').click();expect(dialog).to_have_count(0)
   assert all(Path(p).read_text()==v for p,v in original.items())
   done('sd.transfer.close','Cancel real plan; independent native source content unchanged',width)
   row.hover();page.wait_for_timeout(200);row.click(button='right');page.locator('#item-menu [data-act=clone]').click();expect(dialog).to_be_visible();page.keyboard.press('Escape');expect(dialog).to_have_count(0)
   done('sd.transfer.escape','Escape closes transfer without native changes',width)
   # Local copy: response is deliberately lost after the real commit. UI retry
   # must preserve operation identity, not publish another tree.
   row.hover();page.wait_for_timeout(200);row.click(button='right');page.locator('#item-menu [data-act=clone]').click();expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=20000)
   received=[]
   def lose(route):
    received.append(route.request.post_data_json)
    response=route.fetch(timeout=60000)
    if len(received)==1:route.abort('connectionreset')
    else:route.fulfill(response=response)
   page.route('**/api/session/clone',lose)
   dialog.locator('.clone-confirm').click();expect(dialog.locator('.clone-confirm')).to_contain_text('重试',timeout=30000)
   dialog.locator('.clone-confirm').click();expect(dialog).to_have_count(0,timeout=30000)
   assert len(received)==2 and received[0]['operation_id']==received[1]['operation_id'],received
   assert all(Path(p).read_text()==v for p,v in original.items())
   journals=list((source.root/'state').rglob('*.json'))
   durable=[p for p in journals if received[0]['operation_id'] in p.read_text()]
   assert durable,received
   done('sd.transfer.retry','Lost actual clone reply then click retry reuses operation_id; independent journal exists and original files unchanged',width)
   page.screenshot(path=str(a.output/f'copy-{width}.png'));context.close()
 print('PASS remaining transfer UI',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/transfer')
