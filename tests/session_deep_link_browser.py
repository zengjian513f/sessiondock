#!/usr/bin/env python3
"""Native session IDs open root and nested subagent views, without starting a CLI.

Subagent rows hang under their owner in both sidebar modes, so the deep link
must reveal and select them without flipping the user's hierarchy preference.
"""
from browser_runtime import js
import argparse,json,tempfile
from pathlib import Path
from urllib.parse import urlencode, urlparse, parse_qs
from playwright.sync_api import sync_playwright,expect
from history_parity import BINARY,build_corpus,isolated_server,batch35_meta,codex_message,encoded
NODE='a'*32

def add_rotated_sessions(corpus):
    meta = batch35_meta('codex-rotation', '2026-09-11T08:00:00Z')
    meta['ordinal'] = 10
    old = corpus.put('rotation-old', 'codex', [meta,
        codex_message('user', 'Rotation inherited question', 11),
        codex_message('assistant', 'Rotation inherited answer', 12)], [])
    cut = old.stat().st_size
    with old.open('ab') as stream:
        stream.write(encoded(codex_message('assistant', 'Rotation historical tail', 13)))
    meta = batch35_meta('codex-rotation', '2026-09-11T09:00:00Z', history_base={
        'thread_id':'codex-rotation', 'end_byte_offset':cut, 'end_ordinal_exclusive':13})
    meta['ordinal'] = 13
    corpus.put('rotation-current', 'codex', [meta,
        codex_message('user', 'Rotation current question', 14)], [])
    # These are independent native files, with no continuation evidence.
    for label in ('one', 'two'):
        corpus.put('duplicate-' + label, 'codex', [batch35_meta('codex-duplicate'),
            codex_message('user', 'Independent copy ' + label)], [])


def check_rotated_links(browser, base, corpus):
    old, current = corpus.uid('rotation-old'), corpus.uid('rotation-current')
    cases = [
        ('current', 'codex:codex-rotation', NODE, current),
        ('current', 'codex:codex-rotation', None, current),
        ('legacy-old', old, NODE, current),
        ('independent', 'codex:codex-duplicate', NODE, None),
        ('missing-next', 'codex:codex-rotation', NODE, None),
        ('cycle', 'codex:codex-rotation', NODE, None),
        ('other-node', 'codex:codex-rotation', None, None),
        ('other-node', 'codex:codex-rotation', NODE, current),
    ]
    for width in (1280, 390):
        for mode, sid, node, selected in cases:
            ctx = browser.new_context(viewport={'width':width, 'height':900})
            ctx.add_init_script('window.EventSource=undefined;')
            errors = []
            def sessions(route):
                response = route.fetch(); data = response.json()
                for row in data['sessions']:
                    row['node_id'] = NODE
                by_uid = {row['uid']:row for row in data['sessions']}
                assert by_uid[old]['continued_in'] == current, by_uid[old]
                assert not by_uid[current].get('continued_in'), by_uid[current]
                if mode == 'missing-next':
                    by_uid[old]['continued_in'] = 'codex:missing'
                elif mode == 'cycle':
                    by_uid[current]['continued_in'] = old
                elif mode == 'other-node':
                    # Model a second machine's separate current copy. Without
                    # node=, a native ID must not pick whichever was listed first.
                    data['sessions'].append({**by_uid[current], 'uid':current+'-other', 'node_id':'b'*32})
                route.fulfill(response=response, json=data)
            ctx.route('**/api/sessions*', sessions)
            def messages(route):
                response = route.fetch(); data = response.json()
                if isinstance(data.get('meta'), dict): data['meta']['node_id'] = NODE
                route.fulfill(response=response, json=data)
            ctx.route('**/api/messages/**', messages)
            page = ctx.new_page(); page.on('pageerror', lambda error:errors.append(str(error)))
            query = {'sid':sid}
            if node: query['node'] = node
            page.goto(base+'/?'+urlencode(query), wait_until='networkidle')
            page.wait_for_function(js('S.sessions.length > 0', 'runtime.core.state.catalog.sessions.length > 0'))
            if selected:
                expect(page.locator('#msgs')).to_contain_text('Rotation current question')
                expect(page.locator('#msgs')).to_contain_text('Rotation inherited answer')
                assert page.evaluate(js('S.sel', 'runtime.core.state.selection.sel')) == selected
                if selected == current:
                    expect(page.locator('#msgs')).not_to_contain_text('Rotation historical tail')
                    expect(page.locator('#side .item.sel')).to_have_attribute('data-uid', current)
                    page.reload(wait_until='networkidle')
                    expect(page.locator('#msgs')).to_contain_text('Rotation current question')
                    assert page.evaluate(js('S.sel', 'runtime.core.state.selection.sel')) == current
            else:
                assert page.evaluate(js('S.sel', 'runtime.core.state.selection.sel')) is None, (mode, page.evaluate(js('S.sel', 'runtime.core.state.selection.sel')))
                expect(page.locator('#side .item.sel')).to_have_count(0)
                assert parse_qs(urlparse(page.url).query)['sid'] == [sid]
            assert not errors, errors
            ctx.close()
            print('PASS rotated native link', width, mode, 'with node' if node else 'without node', flush=True)


def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--binary',type=Path,default=BINARY);args=parser.parse_args()
 with tempfile.TemporaryDirectory(prefix='session-deep-link-') as t:
  corpus=build_corpus(Path(t));add_rotated_sessions(corpus)
  before={p:p.read_bytes() for p in corpus.paths.values()}
  owner=corpus.uid('codex-parent')
  with isolated_server(corpus,args.binary) as (base,_),sync_playwright() as pw:
   browser=pw.chromium.launch()
   for width in [1280,390]:
    for nest in [False,True]:
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
      # External links must overcome stored filters and collapsed ancestors,
      # including the owner's folded subagent rows, without flipping S.nest.
      ctx.add_init_script("""window.EventSource=undefined; localStorage.setItem('sessiondock.off',JSON.stringify(['codex']));
        localStorage.setItem('sessiondock.activeOnly','true');
        localStorage.setItem('sessiondock.closed',JSON.stringify(['/synthetic/project']));"""
        +f"localStorage.setItem('sessiondock.nest',JSON.stringify({str(nest).lower()}));"
        +f"localStorage.setItem('sessiondock.nestClosed',JSON.stringify(['{owner}']));""")
      page=ctx.new_page();page.on('pageerror',lambda e:errors.append(str(e)))
      page.goto(base+'/?'+urlencode({'sid':'codex:'+sid,'node':NODE}))
      expect(page.locator('#msgs')).to_contain_text(corpus.expected[agent or sid][-1])
      assert page.evaluate(js('S.sel', 'runtime.core.state.selection.sel'))==owner
      assert page.evaluate(js('S.agent', 'runtime.core.state.selection.agent'))==agent
      assert page.evaluate(js('S.nest', 'runtime.core.state.sidebar.nest'))==nest,'deep link must not flip the hierarchy mode'
      selected=page.locator('#side .item.sel')
      expect(selected).to_have_count(1)
      if agent:
       expect(selected).to_have_attribute('data-agent',agent)
      else:
       expect(selected).to_have_attribute('data-uid',owner)
       # A root link keeps the owner's fold: its own subagent rows stay hidden.
       assert page.locator('#side .item.agent[data-owner="'+owner+'"]').count()==0
      if width==1280: expect(selected).to_be_visible()
      route=parse_qs(urlparse(page.url).query)
      assert route['sid']==['codex:codex-parent'+('/agent:'+agent if agent else '')],route
      assert route['node']==[NODE]
      if not agent:
       # Click a real sidebar row, then use browser history and reload the URL.
       other=page.locator('#side .item[data-uid="'+corpus.uid('claude-compact')+'"]')
       if width==390: page.evaluate(js('showMobileList()', 'runtime.shell.showMobileList()'))
       other.click()
       expect(page.locator('#msgs')).to_contain_text(corpus.expected['claude-compact'][-1])
       assert parse_qs(urlparse(page.url).query)['sid']==['claude:claude-compact']
       page.go_back()
       expect(page.locator('#msgs')).to_contain_text(corpus.expected['codex-parent'][-1])
       expect(page.locator('#side .item.sel')).to_have_attribute('data-uid',owner)
       page.go_forward()
       expect(page.locator('#msgs')).to_contain_text(corpus.expected['claude-compact'][-1])
       page.reload()
       expect(page.locator('#msgs')).to_contain_text(corpus.expected['claude-compact'][-1])

      assert not errors,errors
      ctx.close();print('PASS native sid deep link',width,'nest' if nest else 'flat',sid,flush=True)
   check_rotated_links(browser,base,corpus)
   browser.close()
  assert all(p.read_bytes()==v for p,v in before.items())
if __name__=='__main__':main()
