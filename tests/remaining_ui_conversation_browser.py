#!/usr/bin/env python3
"""Conversation tools/search/scroll/file entries through actual browser input."""
import argparse,json,tempfile,re
from pathlib import Path
from playwright.sync_api import expect,sync_playwright
from history_fixtures import BINARY,Corpus,claude_row,isolated_server
from browser_evidence_fixtures import run_with_evidence


def corpus(root):
 c=Corpus(root)
 for n in ['claude','codex','grok','work']:(root/n).mkdir()
 (root/'work/receipt.txt').write_text('download receipt 中文\n')
 sid='remaining-conversation';rows=[];parent=None
 def add(role,content):
  nonlocal parent
  uid=f'r{len(rows)}';rows.append(claude_row(sid,role,uid,parent,content,cwd=str(root/'work')));parent=uid
 for i in range(35):
  add('user',f'QUESTION {i:02d} needle')
  add('assistant',[{'type':'tool_use','id':f'call{i}','name':'Bash','input':{'command':'printf private-fixture'}}])
  add('user',[{'type':'tool_result','tool_use_id':f'call{i}','content':'\n'.join(f'OUTPUT {j} '+'long '*40 for j in range(80))}])
  if i==34:
   add('assistant',[{'type':'tool_use','id':'edit','name':'Edit','input':{'file_path':str(root/'work/file.txt'),'old_string':'old '*100,'new_string':'new '*100}}])
   add('user',[{'type':'tool_result','tool_use_id':'edit','content':'success'}])
  add('assistant',f'CONCLUSION {i:02d} needle\n\n[receipt](receipt.txt)')
 c.put(sid,'claude',rows,[]);return c,sid


def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',type=Path,default=BINARY);p.add_argument('--output',type=Path,default=Path('target/remaining-ui/conversation'));a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);cases=[]
 def done(ids,actual,width):
  cases.append({'ids':ids.split(),'actual':actual,'width':width});(a.output/'result.json').write_text(json.dumps({'scenarios':cases},indent=2))
 with tempfile.TemporaryDirectory(prefix='sd-remaining-conversation-') as tmp,sync_playwright() as pw:
  root=Path(tmp);c,sid=corpus(root)
  with isolated_server(c,a.binary,file_roots=(root/'work',)) as (base,_):
   browser=pw.chromium.launch(headless=True)
   try:
    for width in [1280,390]:
     context=browser.new_context(viewport={'width':width,'height':760},has_touch=True);page=context.new_page();page.goto(base,wait_until='networkidle')
     page.locator(f'#side .item[data-uid="{c.uid(sid)}"]').click();expect(page.locator('#msgs')).to_contain_text('CONCLUSION 34')
     msgs=page.locator('#msgs');msgs.hover();page.mouse.wheel(0,-100000)
     page.wait_for_function('document.querySelector("#msgs").scrollTop===0');expect(msgs.get_by_text('QUESTION 00 needle',exact=True)).to_be_in_viewport()
     page.mouse.wheel(0,100000);page.wait_for_function('(()=>{const e=document.querySelector("#msgs");return e.scrollHeight-e.clientHeight-e.scrollTop<3})()')
     done('sd.conversation.wheel','Real wheel reaches first question and final conclusion; reads scroll position only',width)
     box=msgs.bounding_box();cdp=context.new_cdp_session(page);x=box['x']+box['width']*.7;y=box['y']+50
     before=msgs.evaluate('e=>e.scrollTop')
     cdp.send('Input.dispatchTouchEvent',{'type':'touchStart','touchPoints':[{'x':x,'y':y}]})
     for i in range(1,9):
      cdp.send('Input.dispatchTouchEvent',{'type':'touchMove','touchPoints':[{'x':x,'y':y+i*35}]});page.wait_for_timeout(20)
     cdp.send('Input.dispatchTouchEvent',{'type':'touchEnd','touchPoints':[]});page.wait_for_timeout(250)
     assert msgs.evaluate('e=>e.scrollTop')<before
     done('sd.conversation.touch','Actual CDP touch drag moves conversation away from tail',width)
     # Clicking ordinary message whitespace establishes native scroll focus.
     msgs.click(position={'x':30,'y':100});page.keyboard.press('Control+Home')
     page.wait_for_function('document.querySelector("#msgs").scrollTop===0')
     page.keyboard.press('Control+End');page.wait_for_function('(()=>{const e=document.querySelector("#msgs");return e.scrollHeight-e.clientHeight-e.scrollTop<3})()')
     done('sd.conversation.keyboard-scroll','Native Control+Home/End reaches conversation boundaries after pointer focus',width)
     turns=page.locator('#a-turns')
     if not turns.is_visible():page.locator('#a-more').click()
     turns.click();expect(page.locator('.turn-process.folded')).to_have_count(0)
     done('sd.conversation.compact','Actual expand-all control materializes completed turn processes',width)
     process=page.locator('.turn-process').last
     process.locator('.turn-to-start').click();expect(process.locator('.turn-toolbar')).to_be_in_viewport()
     process.locator('.turn-to-conclusion').click();expect(msgs.get_by_text('CONCLUSION 34 needle',exact=True)).to_be_in_viewport()
     done('sd.conversation.turn-start sd.conversation.turn-conclusion','Turn navigation clicks reveal corresponding start/conclusion',width)
     tool=process.locator('.tool-entry').first
     # Tool group itself can remain folded after turn expansion.
     for toggle in process.locator('.tool-group.folded .fold-toggle').all():toggle.click()
     tool.locator('.tool-toggle').click();expect(tool.locator('.tool-args')).to_be_visible();tool.locator('.tool-args-close').click();expect(tool.locator('.tool-args')).to_be_hidden()
     done('sd.conversation.tool-arguments-close','Open original tool arguments then explicit close hides them',width)
     more=process.locator('.tool-out-actions .more').first;more.click();expect(more).to_have_attribute('aria-expanded','true');expect(process).to_contain_text('OUTPUT 79')
     done('sd.conversation.tool-more','Expand clipped output exposes independent fixture last line',width)
     wrap=process.locator('button[data-diff-wrap]').first;wrap.click();expect(wrap).to_have_attribute('aria-pressed','true');wrap.click();expect(wrap).to_have_attribute('aria-pressed','false')
     done('sd.conversation.file-diff-wrap','Diff wrap control toggles aria state and restores original layout',width)
     link=msgs.locator('a[data-file-ref]').last
     # Finish scrolling the long output before opening its context menu.
     # The menu intentionally closes on conversation scroll events.
     link.scroll_into_view_if_needed()
     page.evaluate('() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
     with page.expect_response(lambda r:r.url.endswith('/api/session/resolve-files')) as resolved:
      link.click(button='right')
     assert resolved.value.ok,resolved.value.text()
     assert any(t.get('kind')=='file' and t.get('path')==str(root/'work/receipt.txt') for t in resolved.value.json().get('targets',[])),resolved.value.text()
     expect(page.locator('#file-menu [data-action=download]')).to_be_visible()
     page.screenshot(path=str(a.output/f'file-menu-{width}.png'))
     page.keyboard.press('ArrowDown');assert page.locator('#file-menu').evaluate('e=>e.contains(document.activeElement)')
     page.keyboard.press('Escape');expect(page.locator('#file-menu')).to_be_hidden()
     done('sd.conversation.file-menu-keyboard','File context menu ArrowDown focus then Escape closes',width)
     link.click(button='right');msgs.click(position={'x':30,'y':20});expect(page.locator('#file-menu')).to_be_hidden()
     done('sd.conversation.file-menu-close','Actual outside pointer dismisses file menu',width)
     link.click(button='right')
     with page.expect_download() as download:page.locator('#file-menu [data-action=download]').click()
     assert Path(download.value.path()).read_text()=='download receipt 中文\n'
     done('sd.conversation.file-download','Click download; independent downloaded bytes match private fixture',width)
     if width<720:page.locator('.mobile-back').click()
     page.locator('#q').fill('needle');page.locator('#q').press('Enter');expect(page.locator('#side-search-exit')).to_be_visible()
     page.locator(f'#side .item[data-uid="{c.uid(sid)}"]').click()
     if not page.locator('#m-next').is_visible():page.locator('#a-more').click()
     expect(page.locator('#m-next')).to_be_visible()
     first=page.locator('#mcount').inner_text();page.locator('#m-next').click();second=page.locator('#mcount').inner_text();assert first!=second
     if not page.locator('#m-prev').is_visible():page.locator('#a-more').click()
     page.locator('#m-prev').click();expect(page.locator('#mcount')).to_have_text(first)
     done('sd.search.previous sd.search.next','Search real history; next/previous buttons change and restore current match',width)
     if width<720:page.locator('.mobile-back').click()
     page.locator('#side-search-exit').click();expect(page.locator('#q')).to_have_value('')
     done('sd.sidebar.search-exit','Exit search clears query and returns normal sidebar',width)
     held=[];page.route('**/api/search?*',lambda r:held.append(r));page.locator('#q').fill('pending');page.locator('#q').press('Enter');expect(page.locator('#search-cancel')).to_be_visible();page.locator('#search-cancel').click();expect(page.locator('#q')).to_have_value('');expect(page.locator('#search-progress')).not_to_be_visible()
     for r in held:r.abort()
     done('sd.search.cancel','Click cancels held network search and restores normal list; fault is frontend boundary only',width)
     page.screenshot(path=str(a.output/f'conversation-{width}.png'));context.close()
   finally:browser.close()
 print('PASS remaining conversation inputs',flush=True)

if __name__=='__main__':run_with_evidence(main,'target/remaining-ui/conversation')
