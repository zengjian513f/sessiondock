#!/usr/bin/env python3
"""Conversation file references open the independent FileDock page."""
import argparse,json,os,tempfile
from pathlib import Path
from urllib.parse import urlsplit,parse_qs,urlencode
from playwright.sync_api import expect,sync_playwright
from history_parity import BINARY,build_corpus,claude_row,codex_row,codex_message,isolated_server
NODE='a'*32

def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY);args=parser.parse_args()
 with tempfile.TemporaryDirectory(prefix='sessiondock-filedock-') as t:
  corpus=build_corpus(Path(t));directory=corpus.root/'files';directory.mkdir();file=directory/'文档 with spaces.md';file.write_text('# linked file')
  sid='claude-filedock';corpus.put(sid,'claude',[claude_row(sid,'user','u0',None,f'Open `{file}` and `missing.txt`.',cwd=str(directory))],[])
  # A prior mention of another project's same-named file must not override cwd.
  current=directory/'SCREENING.md';current.write_text('# current project')
  other=directory/'other';other.mkdir();(other/current.name).write_text('# other project')
  fallback=other/'fallback.md';fallback.write_text('# unique fallback')
  second=directory/'second';second.mkdir()
  for parent in [other,second]:(parent/'ambiguous.md').write_text('# ambiguous')
  refs=f'`{current}` `other/SCREENING.md` `./other` `./second`'
  regression='codex-filedock'
  corpus.put(regression,'codex',[
   codex_row('session_meta',{'id':regression,'cwd':str(directory),'source':'cli'}),
   codex_message('user',refs),
   codex_message('assistant','Read `SCREENING.md`, `fallback.md`, and `ambiguous.md`.')],[])
  before={p:p.read_bytes() for p in corpus.paths.values()}
  with isolated_server(corpus,args.binary,file_roots=(directory,)) as (base,_),sync_playwright() as pw:
   browser=pw.chromium.launch(**({'executable_path':os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']} if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE') else {}))
   for width in [1280,390]:
    context=browser.new_context(viewport={'width':width,'height':900},service_workers='block');errors=[];destinations=[]
    context.on('page',lambda p:p.on('pageerror',lambda e:errors.append(str(e))))
    def meta(route):
     response=route.fetch();value=response.json();value['node_id']=NODE;route.fulfill(response=response,json=value)
    context.route('**/api/meta',meta)
    def destination(route):
     destinations.append(parse_qs(urlsplit(route.request.url).query));route.fulfill(content_type='text/html',body='<h1>FileDock destination</h1>')
    context.route('**/files/?*',destination)
    page=context.new_page();page.goto(base);page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]').click()
    with context.expect_page() as opened:page.locator(f'#msgs a[data-file-ref={json.dumps(str(file),ensure_ascii=False)}]').click()
    preview=opened.value;expect(preview.locator('h1')).to_have_text('FileDock destination')
    assert destinations[-1]=={'node':[NODE],'path':[str(file)]},destinations
    with context.expect_page() as opened:page.locator('#msgs a[data-file-ref="missing.txt"]').click()
    missing=opened.value;expect(missing.locator('#file-retry')).to_be_visible();assert 'FileDock destination' not in missing.locator('body').inner_text()
    direct=context.new_page();direct.goto(base+'/file.html?'+urlencode({'node':NODE,'path':str(directory)}));expect(direct.locator('h1')).to_have_text('FileDock destination');assert destinations[-1]['path']==[str(directory)]
    if width<600:page.locator('.mobile-back').click()
    page.locator(f'#side .item[data-uid="{corpus.uid(regression)}"]').click()
    for ref,target in [('SCREENING.md',current),('other/SCREENING.md',other/current.name),('fallback.md',fallback)]:
     with context.expect_page() as opened:page.locator(f'#msgs a[data-file-ref={json.dumps(ref)}]').click()
     expect(opened.value.locator('h1')).to_have_text('FileDock destination')
     assert destinations[-1]=={'node':[NODE],'path':[str(target)]},destinations
    with context.expect_page() as opened:page.locator('#msgs a[data-file-ref="ambiguous.md"]').click()
    expect(opened.value.locator('#file-content')).to_contain_text('多个同名文件')
    expect(opened.value.locator('#file-retry')).to_be_visible()
    assert not errors,errors;context.close()
   browser.close()
  assert all(p.read_bytes()==v for p,v in before.items())
 print('PASS SessionDock file-reference click, exact node/Unicode path, cwd precedence, explicit sibling path, unique fallback, ambiguous/missing errors, independent directory entry; desktop/mobile')
if __name__=='__main__':main()
