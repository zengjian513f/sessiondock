#!/usr/bin/env python3
"""Click folded branches/groups and filters with many same-basename directories."""

import argparse
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright
from history_fixtures import BINARY, isolated_server
from header_fold_browser import corpus
from sidebar_closed_groups_browser import fixture, seed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    rows = fixture()[:800]
    for i, row in enumerate(rows):
        row['cwd'] = f'/synthetic/runs/run-{i}/checkout/workspace'
    with tempfile.TemporaryDirectory(prefix='sessiondock-path-performance-') as tmp:
        with isolated_server(corpus(Path(tmp)), args.binary) as (base, _), sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            page = browser.new_page(viewport={'width': 1400, 'height': 900})
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.route('**/api/sessions?*', lambda r: r.fulfill(json={'unchanged': True}))
            page.goto(base, wait_until='networkidle')
            page.wait_for_function('S.sessions.length > 0 && T.listLoaded')
            page.evaluate('''() => {
              window.__regexCalls=0; const test=RegExp.prototype.test;
              RegExp.prototype.test=function(...args){__regexCalls++;return test.apply(this,args)};
              window.__fits=0;const fit=fitTimelineDirectories;
              fitTimelineDirectories=function(...args){__fits++;return fit(...args)};
            }''')
            seed(page, rows, 'date')
            assert page.evaluate('__regexCalls') < 50000, page.evaluate('__regexCalls')
            # Verify abbreviation semantics against all peers, independently of
            # the candidate index. Every label must match only its source path.
            assert page.evaluate(r'''() => {
              const plans=timelinePlanCache.plans;
              for(const [path,plan] of plans) for(const label of plan.labels){
                const pattern=new RegExp('^'+label.split('/').map(p=>p==='…'
                  ? '(?:[^/]+/)*[^/]+' : p.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')).join('/')+'$');
                if(!pattern.test(path)) return false;
                for(const peer of plans.keys()) if(peer!==path && pattern.test(peer)) return false;
              }
              window.__plans=plans;return true;
            }''')
            caret=page.locator('.item[data-uid="claude:fold-0"] .nest-caret')
            page.evaluate('''() => {window.__full=0;const render=renderSide;
              renderSide=function(...args){__full++;return render(...args)}}''')
            for i in range(4):
                page.evaluate('''i=>{const row=indexedSessions().byUid.get('claude:fold-0');
                  applyMigrationMeta(row.uid,null,{meta:row},{...row,size:4096+i,cursor:{end:i,head:'test'}})}''', i)
                caret.click()
            assert page.evaluate('__full') == 0
            assert '4K' in page.locator('.item[data-uid="claude:fold-0"] .m').text_content()
            assert caret.get_attribute('aria-expanded') == 'false'
            group=page.locator('#side>.group').first
            group.locator('.ghead').click()
            page.wait_for_timeout(160)
            page.evaluate('__fits=0')
            group.locator('.ghead').click()
            page.wait_for_timeout(160)
            assert page.evaluate('__fits') == 1, page.evaluate('__fits')
            page.locator('#chips .chip[data-source="claude"]').click()
            page.locator('#chips .chip[data-source="claude"]').click()
            page.set_viewport_size({'width': 1250, 'height': 900})
            page.wait_for_timeout(150)
            assert page.evaluate('timelinePathPlans([...S.sessions].reverse())===__plans')
            # A new colliding directory invalidates the plan and remains distinct.
            page.evaluate('''()=>{S.sessions=[...S.sessions,{...S.sessions[0],uid:'claude:extra',sid:'extra',
              cwd:'/synthetic/other/run-0/checkout/workspace'}];renderSide()}''')
            page.wait_for_function("document.querySelector('.item[data-uid=\"claude:extra\"]') !== null")
            assert page.evaluate('timelinePlanCache.plans!==__plans')
            assert page.locator('.item[data-uid="claude:extra"]').count() == 1
            assert not errors, errors
            browser.close()
    print('PASS sidebar path performance: indexed cold plans, cache invalidation, metadata and real disclosure/filter clicks')


if __name__ == '__main__':
    main()
