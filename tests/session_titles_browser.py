#!/usr/bin/env python3
"""Chromium point-title queries, native rename and no hot discovery; synthetic data only."""
import argparse
import json
import statistics
import tempfile
import time
from pathlib import Path
from urllib.parse import quote
from playwright.sync_api import sync_playwright
from history_parity import BINARY, Corpus, claude_row, encoded, isolated_server, batch35_meta, codex_message


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='title-query-') as tmp:
        corpus=Corpus(Path(tmp))
        for i in range(500):
            sid=f'title-{i}'
            corpus.put(sid,'claude',[claude_row(sid,'user',f'u{i}',None,f'Original {i}')],[])
        corpus.put('named-codex','codex',[batch35_meta('named-codex'), codex_message('user','Codex original')],[])
        names=corpus.root/'names.jsonl'
        names.write_bytes(encoded({'id':'named-codex','thread_name':'Named first'}))
        (corpus.root/'grok').mkdir()
        with isolated_server(corpus,args.binary,extra_env={'SESSIONDOCK_CODEX_INDEX':str(names)}) as (base,opener), sync_playwright() as pw:
            browser=pw.chromium.launch()
            page=browser.new_page()
            page.route(base+'/',lambda route:route.fulfill(content_type='text/html',body='''
              <button id="refresh">Refresh linked session titles</button><pre id="titles"></pre>
              <script>document.querySelector('button').onclick=async()=>{
                const r=await fetch('/api/sessions/titles?ids=claude:title-0,claude:title-1');
                document.querySelector('pre').textContent=JSON.stringify(await r.json());
              }</script>'''))
            page.goto(base+'/')
            def click():
                start=time.perf_counter()
                with page.expect_response('**/api/sessions/titles?*') as response:
                    page.locator('#refresh').click()
                row=response.value.json()
                page.wait_for_function('document.querySelector("pre").textContent.length>0')
                return row,response.value.request.timing['responseEnd']
            cold,cold_ms=click()
            assert cold['index_initialized'] is True,cold
            assert len(cold['sessions'])==2
            assert 'Original 0' in page.locator('#titles').inner_text()
            # A new file must NOT be discovered by a point query after bootstrap.
            corpus.put('late','claude',[claude_row('late','user','u',None,'Late')],[])
            with corpus.paths['title-0'].open('ab') as f:
                f.write(encoded({'type':'custom-title','customTitle':'Renamed title','sessionId':'title-0'}))
            renamed,_=click()
            assert renamed['index_initialized'] is False
            assert renamed['summary_reads']==1,renamed
            assert any(s['title']=='Renamed title' for s in renamed['sessions']),renamed
            page.wait_for_function('document.querySelector("pre").textContent.includes("Renamed title")')
            assert page.request.get(base+'/api/sessions/titles?ids=codex:named-codex').json()['sessions'][0]['title']=='Named first'
            names.write_bytes(encoded({'id':'named-codex','thread_name':'Codex renamed'}))
            named=page.request.get(base+'/api/sessions/titles?ids=codex:named-codex').json()
            assert named['sessions'][0]['title']=='Codex renamed' and named['summary_reads']==0,named
            hot=[]
            for _ in range(20):
                start=time.perf_counter()
                result=page.request.get(base+'/api/sessions/titles?ids='+quote('claude:title-0,claude:title-1')).json()
                hot.append((time.perf_counter()-start)*1000)
                assert result['summary_reads']==0 and not result['index_initialized']
            missing=page.request.get(base+'/api/sessions/titles?ids=claude:late').json()
            assert missing['sessions']==[] and missing['missing']==['claude:late'],missing
            # Normal list discovery still picks up newly created sessions.
            listed=page.request.get(base+'/api/sessions?force=1').json()
            assert next(r for r in listed['sessions'] if r['sid']=='named-codex')['title']=='Codex renamed'
            assert len(page.request.get(base+'/api/sessions/titles?ids=claude:late').json()['sessions'])==1
            browser.close()
            print(json.dumps({'synthetic_sessions':500,'requested':2,'cold_http_ms':round(cold_ms,2),
                 'hot_http_p50_ms':round(statistics.median(hot),2),'hot_http_max_ms':round(max(hot),2),
                 'renamed_summary_reads':1,'hot_summary_reads':0,'hot_discovery':False}))

if __name__=='__main__':main()
