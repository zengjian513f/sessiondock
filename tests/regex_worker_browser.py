#!/usr/bin/env python3
"""Regex worker failure/retry through actual search controls, with real audit persistence."""
import argparse
import json
from pathlib import Path
import tempfile
from playwright.sync_api import expect, sync_playwright
from search_browser import corpus
from history_fixtures import BINARY, isolated_server
from diagnostic_retention_browser import until, rows


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--output',type=Path,default=Path('target/regex-worker'))
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-regex-worker-') as tmp, sync_playwright() as pw:
        data=corpus(Path(tmp));audit=data.root/'audit';audit.mkdir()
        originals={p:p.read_bytes() for p in data.paths.values()}
        with isolated_server(data,args.binary,audit_dir=audit) as (base,_):
            browser=pw.chromium.launch(headless=True)
            try:
                for index, width in enumerate([1280,390]):
                    context=browser.new_context(viewport={'width':width,'height':900},service_workers='block')
                    page=context.new_page()
                    context.route('**/regex-worker.js*',lambda route:route.fulfill(content_type='application/javascript',body="throw new Error('synthetic worker failure')"),times=5)
                    page.goto(base)
                    page.locator('#opts [data-o=regex]').click()
                    page.locator('#q').fill('Synthetic.*')
                    page.wait_for_timeout(1000)
                    page.screenshot(path=str(args.output/f'failure-{width}.png'))
                    expect(page.locator('#stat')).to_contain_text('正则匹配暂不可用',timeout=3000)
                    until(page,lambda:any(r['event']=='browser.search.regex_worker_failed' for r in rows(audit)))
                    # Repeated explicit retries during the same outage stay
                    # visible but coalesce diagnostic failures, without logging input.
                    for _ in range(4):
                        with page.expect_response('**/regex-worker.js*'):
                            page.locator('#q').press('Enter')
                        expect(page.locator('#stat')).to_contain_text('正则匹配暂不可用')
                    # Enter is the existing retry action; query content is never logged.
                    page.locator('#q').press('Enter')
                    expect(page.locator('#stat')).not_to_contain_text('暂不可用')
                    expect(page.locator('#side .item[data-uid="'+data.uid('search-main')+'"]')).to_be_visible()
                    expect(page.locator('#side mark').first).to_be_visible()
                    page.locator('#q').press('Escape')
                    expect(page.locator('#q')).to_have_value('')
                    expect(page.locator('#side .item')).to_have_count(3)
                    page.screenshot(path=str(args.output/f'recovered-{width}.png'))
                    until(page, lambda:sum(r['event']=='browser.search.regex_worker_recovered' for r in rows(audit))==index+1)
                    context.close()
                failures=[r for r in rows(audit) if r['event']=='browser.search.regex_worker_failed']
                assert len(failures)==2, failures
                recovered=[r for r in rows(audit) if r['event']=='browser.search.regex_worker_recovered']
                assert all(r['data']['failures']==5 for r in recovered), recovered
                assert all('Synthetic' not in json.dumps(r['data']) and r['page_id'] for r in failures)
                assert all(p.read_bytes()==value for p,value in originals.items())
                (args.output/'result.json').write_text(json.dumps({'passed':['wide/narrow actual regex input','visible worker failure','Enter retry with real backend search results','Escape reset','one metadata-only failure event per outage','native fixtures unchanged'],'failure_events':len(failures)},indent=2))
                print('PASS regex worker failure, Enter retry, Escape and persisted metadata at 1280/390',flush=True)
            finally:browser.close()


if __name__=='__main__':main()
