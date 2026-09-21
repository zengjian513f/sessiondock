#!/usr/bin/env python3
"""Native session IDs open root and nested subagent views, without starting a CLI."""
import argparse,json,tempfile
from pathlib import Path
from urllib.parse import urlencode
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
    for sid,agent in [('codex-parent',None),('codex-agent','codex-agent'),('codex-nested-agent','codex-nested-agent')]:
     ctx=browser.new_context(viewport={'width':width,'height':900});errors=[]
     def sessions(route):
      response=route.fetch();data=response.json()
      for row in data['sessions']:row['node_id']=NODE
      route.fulfill(response=response,json=data)
     ctx.route('**/api/sessions*',sessions)
     page=ctx.new_page();page.on('pageerror',lambda e:errors.append(str(e)))
     page.goto(base+'/?'+urlencode({'sid':'codex:'+sid,'node':NODE}))
     expect(page.locator('#msgs')).to_contain_text(corpus.expected[sid][-1])
     assert page.evaluate('S.sel')==corpus.uid('codex-parent')
     assert page.evaluate('S.agent')==agent
     assert not errors,errors
     ctx.close();print('PASS native sid deep link',width,sid,flush=True)
   browser.close()
  assert all(p.read_bytes()==v for p,v in before.items())
if __name__=='__main__':main()
