#!/usr/bin/env python3
"""Native parent menus/visibility and ambiguous Hub link candidate selection."""
import argparse,json,tempfile
from pathlib import Path
from contextlib import ExitStack
from types import SimpleNamespace
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,isolated_server
from session_clone_browser import prepare
from session_transfer_browser import ident
from hub_fixtures import Hub,free_port,scoped
from node_auth_fixtures import node_env,TOKEN
from browser_evidence_fixtures import run_with_evidence


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/relations'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual,width):
  cases.append({'ids':ids.split(),'actual':actual,'width':width});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 with tempfile.TemporaryDirectory(prefix='sd-remaining-relations-') as tmp,sync_playwright() as pw,ExitStack() as stack:
  root=Path(tmp);corpora=[prepare(root/'a'),prepare(root/'b')];nodes=[]
  for i,c in enumerate(corpora):
   n=SimpleNamespace(name=['Source','Destination'][i],nid=['a','b'][i]*32,port=free_port(),token=TOKEN);nodes.append(n);(c.root/'ids').mkdir();(c.root/'ids/node-id').write_text(n.nid);env=node_env(c.root,n.port,'127.0.0.0/8');env['SESSIONDOCK_PROC_ROOT']=str(c.root/'proc');stack.enter_context(isolated_server(c,a.binary,state_dir=c.root/'state',extra_env=env))
  (root/'hub').mkdir();hub=Hub(a.binary.resolve().with_name('sessiondock-hub'),root/'hub',nodes);hub.start();stack.callback(hub.stop);browser=pw.chromium.launch(headless=True);stack.callback(browser.close);base=f'http://127.0.0.1:{hub.port}'
  for width in [1280,390]:
   context=browser.new_context(viewport={'width':width,'height':850});page=context.new_page();page.goto(base+'/?sid=codex:'+ident(2),wait_until='networkidle');notice=page.locator('#session-link-notice');expect(notice).to_contain_text('多个会话');expect(notice.locator('button')).to_have_count(2);notice.locator('button').filter(has_text='Source').click();expect(page.locator('#msgs')).to_contain_text('Branch A current');assert page.evaluate('S.sel')==scoped(nodes[0].nid,corpora[0].uid('a'))
   done('sd.conversation.link-candidate','Ambiguous native SID on real two-node Hub; click Source candidate opens exact node/history',width)
   trigger=page.locator('#a-fork-chain');trigger.click();expect(page.locator('#fork-chain-menu')).to_be_visible();page.keyboard.press('Escape');expect(page.locator('#fork-chain-menu')).to_be_hidden();expect(trigger).to_be_focused()
   trigger.click();page.locator('#msgs').click(position={'x':40,'y':180});expect(page.locator('#fork-chain-menu')).to_be_hidden()
   done('sd.conversation.parent-escape sd.conversation.parent-outside','Actual parent chain open followed by Escape/focus restore and outside pointer dismissal',width)
   if width<720:page.locator('.mobile-back').click()
   uid=scoped(nodes[0].nid,corpora[0].uid('a'));row=page.locator(f'#side .item[data-uid="{uid}"]');row.hover();page.wait_for_timeout(100);row.click(button='right');hide=page.locator('#item-menu [data-act=hide]');expect(hide).to_be_visible();hide.click();expect(row).to_have_count(0)
   metadata=json.loads((corpora[0].root/'state/session-metadata.json').read_text());assert metadata['sessions'][corpora[0].uid('a')].get('fork_parent_visible',False) is False
   page.reload(wait_until='networkidle');expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_count(0)
   done('sd.sidebar.menu-hide','Hide parent context action persists false in independent metadata and survives reload',width)
   # Restore through chain UI for the next viewport.
   page.goto(base+'/?sid=codex:'+ident(2)+'&node='+nodes[0].nid,wait_until='networkidle');expect(page.locator('#msgs')).to_contain_text('Branch A current');action=page.locator('#a-session-action')
   if not action.is_visible():page.locator('#a-more').click()
   action.click();page.screenshot(path=str(a.output/f'relations-{width}.png'));context.close()
 print('PASS remaining parent/link entries',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/relations')
