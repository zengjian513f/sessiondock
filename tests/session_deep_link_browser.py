#!/usr/bin/env python3
"""Native session IDs open root and nested subagent views, without starting a CLI."""
import argparse,json,tempfile
from pathlib import Path
from urllib.parse import urlencode, urlparse, parse_qs
from playwright.sync_api import sync_playwright,expect
from history_parity import BINARY,build_corpus,isolated_server
NODE='a'*32

def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY);args=parser.parse_args()
 with tempfile.TemporaryDirectory(prefix='session-deep-link-') as t:
  corpus=build_corpus(Path(t));before={p:p.read_bytes() for p in corpus.paths.values()}
  with isolated_server(corpus,args.binary) as (base,_),sync_playwright() as pw:
   browser=pw.chromium.launch()
   for width in [1280,390]:
    for sid,agent in [('codex-parent',None),('codex-agent','codex-agent'),('codex-nested-agent','codex-nested-agent'),('codex-parent/agent:codex-agent','codex-agent')]:
     ctx=browser.new_context(viewport={'width':width,'height':900});errors=[]
     def sessions(route):
      response=route.fetch();data=response.json()
      for row in data['sessions']:row['node_id']=NODE
      route.fulfill(response=response,json=data)
     ctx.route('**/api/sessions*',sessions)
     def messages(route):
      response=route.fetch();data=response.json()
      if isinstance(data.get('meta'),dict):data['meta']['node_id']=NODE
      route.fulfill(response=response,json=data)
     ctx.route('**/api/messages/**',messages)
     # External links must overcome stored filters and collapsed ancestors.
     ctx.add_init_script("""window.EventSource=undefined; localStorage.setItem('sessiondock.off',JSON.stringify(['codex']));
       localStorage.setItem('sessiondock.activeOnly','true');
       localStorage.setItem('sessiondock.closed',JSON.stringify(['/synthetic/project']));""" )
     page=ctx.new_page();page.on('pageerror',lambda e:errors.append(str(e)))
     page.goto(base+'/?'+urlencode({'sid':'codex:'+sid,'node':NODE}))
     expect(page.locator('#msgs')).to_contain_text(corpus.expected[agent or sid][-1])
     assert page.evaluate('S.sel')==corpus.uid('codex-parent')
     assert page.evaluate('S.agent')==agent
     selected=page.locator('#side .item.sel')
     expect(selected).to_have_count(1)
     if agent:
      expect(selected).to_have_attribute('data-agent',agent)
     else:
      expect(selected).to_have_attribute('data-uid',corpus.uid('codex-parent'))
     if width==1280: expect(selected).to_be_visible()
     route=parse_qs(urlparse(page.url).query)
     assert route['sid']==['codex:codex-parent'+('/agent:'+agent if agent else '')],route
     assert route['node']==[NODE]
     if not agent:
      # Click a real sidebar row, then use browser history and reload the URL.
      other=page.locator('#side .item[data-uid="'+corpus.uid('claude-compact')+'"]')
      if width==390: page.evaluate('showMobileList()')
      other.click()
      expect(page.locator('#msgs')).to_contain_text(corpus.expected['claude-compact'][-1])
      assert parse_qs(urlparse(page.url).query)['sid']==['claude:claude-compact']
      page.go_back()
      expect(page.locator('#msgs')).to_contain_text(corpus.expected['codex-parent'][-1])
      expect(page.locator('#side .item.sel')).to_have_attribute('data-uid',corpus.uid('codex-parent'))
      page.go_forward()
      expect(page.locator('#msgs')).to_contain_text(corpus.expected['claude-compact'][-1])
      page.reload()
      expect(page.locator('#msgs')).to_contain_text(corpus.expected['claude-compact'][-1])

     assert not errors,errors
     ctx.close();print('PASS native sid deep link',width,sid,flush=True)
   browser.close()
  assert all(p.read_bytes()==v for p,v in before.items())
if __name__=='__main__':main()
