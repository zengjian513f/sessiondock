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
  windows='claude-windows-files'
  windows_targets={
   r'X:\workspace':('X:/workspace','directory'),
   'Y:/workspace':('Y:/workspace','directory'),
   'Z:\\':('Z:/','directory'),
   r'x:\项目 with spaces\report.md:12:3':('x:/项目 with spaces/report.md','file'),
   r'Q:\report.md':('Q:\\report.md','file'),
   'R:/report.md':('R:/report.md','file'),
  }
  text='Windows directories (X:\\workspace, Y:/workspace, Z:\\). '
  text+=r'Unicode `x:\项目 with spaces\report.md:12:3`; [drive file](Q:\report.md); `R:/report.md`. '
  text+='Reject `X:relative.md` and [unsafe](javascript:alert(1)).'
  corpus.put(windows,'claude',[claude_row(windows,'user','u0',None,text,cwd=str(directory))],[])
  web='claude-web-links'
  web_targets=['https://links.example.test/artifact/abc',
   'https://links.example.test/demos/skate/',
   'https://links.example.test/search?q=sea&lang=zh#result',
   'https://links.example.test/report(final)',
   'https://www.links.example.test/demo',
   'https://links.example.test/label',
   'https://links.example.test/code']
  text=f'网页已经上线：\n\n**{web_targets[0]}**\n\n**{web_targets[1]}**\n\n'
  text+=f'- 裸链接：{web_targets[2]}。\n- 斜体：*{web_targets[3]}*。\n'
  text+='- （www.links.example.test/demo）\n'
  text+=f'- [**查看网页**]({web_targets[5]})\n- `{web_targets[6]}`\n\n'
  text+='普通文件 report.md 和目录 ./docs/ 保持原样。\n\n```text\nhttps://links.example.test/literal\n```'
  corpus.put(web,'claude',[claude_row(web,'assistant','a0',None,text,cwd=str(directory))],[])
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
    context.route('https://**.example.test/**',lambda route:route.fulfill(content_type='text/html',body='<h1>Web destination</h1>'))
    checked_windows=set()
    def windows_resolution(route):
     ref=route.request.post_data_json['refs'][0]
     if ref not in windows_targets:route.continue_();return
     # Exercise the real semantic-reference index first. Linux cannot open
     # Windows drives, so supply Windows-node filesystem results only after
     # verifying that the backend recognized the complete original reference.
     response=route.fetch();value=response.json()
     assert response.ok and value['errors'][0]['code']=='file_not_found',value
     checked_windows.add(ref)
     path,kind=windows_targets[ref]
     route.fulfill(response=response,json={'targets':[{'ref':ref,'path':path,'kind':kind}],
      'resolved':{ref:path},'errors':[]})
    context.route('**/api/session/resolve-files',windows_resolution)
    context.add_init_script("window.copiedPaths=[]; Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async text=>window.copiedPaths.push(text)}})")
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
    if width<600:page.locator('.mobile-back').click()
    page.locator(f'#side .item[data-uid="{corpus.uid(windows)}"]').click()
    expect(page.locator('#msgs a[data-file-ref="X:relative.md"]')).to_have_count(0)
    expect(page.locator('#msgs a[href^="javascript:"]')).to_have_count(0)
    for ref,(target,kind) in windows_targets.items():
     link=page.locator(f'#msgs a[data-file-ref={json.dumps(ref,ensure_ascii=False)}]')
     expect(link).to_have_count(1)
     with context.expect_page() as opened:link.click()
     expect(opened.value.locator('h1')).to_have_text('FileDock destination')
     assert destinations[-1]=={'node':[NODE],'path':[target]},destinations
     opened.value.close()
     link.click(button='right')
     expect(page.locator('#file-menu-target')).to_have_text(target)
     expect(page.locator('#file-menu [data-action="copy-path"]')).to_be_visible()
     page.locator('#file-menu [data-action="copy-path"]').click()
     page.wait_for_function('path=>window.copiedPaths.at(-1)===path',arg=target)
     link.click(button='right')
     expect(page.locator('#file-menu-target')).to_have_text(target)
     if kind=='file':
      expect(page.locator('#file-menu [data-action="download"]')).to_be_visible()
      page.locator('#file-menu [data-action="copy-directory"]').click()
      parent=target[:3] if ref in [r'Q:\report.md','R:/report.md'] else 'x:/项目 with spaces'
      page.wait_for_function('path=>window.copiedPaths.at(-1)===path',arg=parent)
     else:
      expect(page.locator('#file-menu [data-action="download"]')).to_be_hidden()
      page.locator('#file-menu').press('Escape')
    assert checked_windows==set(windows_targets),checked_windows
    if width<600:page.locator('.mobile-back').click()
    page.locator(f'#side .item[data-uid="{corpus.uid(web)}"]').click()
    links=page.locator('#msgs a[data-reference-kind="web"]')
    expect(links).to_have_count(len(web_targets))
    expect(page.locator('#msgs b a')).to_have_count(2)
    expect(page.locator('#msgs i a')).to_have_count(1)
    expect(page.locator('#msgs a b')).to_have_text('查看网页')
    expect(page.locator('#msgs a a')).to_have_count(0)
    expect(page.locator('#msgs pre a')).to_have_count(0)
    expect(page.locator('#msgs a[data-file-ref]')).to_have_count(0)
    for target in web_targets:
     link=page.locator(f'#msgs a[href={json.dumps(target)}]')
     expect(link).to_have_count(1)
     with context.expect_page() as opened:link.click()
     expect(opened.value.locator('h1')).to_have_text('Web destination')
     assert opened.value.url==target,opened.value.url
     opened.value.close()
    print(f'PASS web links: bold/plain/italic/parenthesized/www/query/fragment/Markdown/code-span clicks; width={width}',flush=True)
    assert not errors,errors;context.close()
   browser.close()
  assert all(p.read_bytes()==v for p,v in before.items())
 print('PASS SessionDock file-reference click, Windows drive recognition/backend grants/routing/copy/drive roots, exact node/Unicode path, cwd precedence, explicit sibling path, unique fallback, ambiguous/missing errors, independent directory entry; desktop/mobile')
if __name__=='__main__':main()
