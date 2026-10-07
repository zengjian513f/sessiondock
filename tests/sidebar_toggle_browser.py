#!/usr/bin/env python3
"""Large sidebar: windowed rows, resource viewport rendering, reuse on nesting, and scroll hydration."""

import argparse
import os
from pathlib import Path
import tempfile
from playwright.sync_api import expect, sync_playwright
from header_fold_browser import corpus
from history_fixtures import BINARY, isolated_server

def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--binary',type=Path,default=BINARY)
    args=ap.parse_args()
    with tempfile.TemporaryDirectory(prefix='sidebar-toggle-') as tmp, sync_playwright() as pw:
        with isolated_server(corpus(Path(tmp)),args.binary) as (base,_):
            launch={'headless':True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path']=os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser=pw.chromium.launch(**launch)
            try:
                page=browser.new_page(viewport={'width':1400,'height':900})
                # Give asynchronous layout/visibility callbacks scheduling
                # room comparable to a slower client.
                page.context.new_cdp_session(page).send('Emulation.setCPUThrottlingRate', {'rate': 4})
                errors=[]
                page.on('pageerror',lambda e:errors.append(str(e)))
                page.goto(base,wait_until='networkidle')
                page.wait_for_function('S.sessions.length > 0 && T.listLoaded')
                rows=[dict(uid=f'claude:toggle-{i}',sid=f'toggle-{i}',source='claude',title=f'Row {i}',
                           cwd='/synthetic/toggle',created='2026-10-01T00:00:00Z',updated='2026-10-01T00:00:00Z',size=100,
                           **({'nest_parent':{'source':'claude','sid':f'toggle-{i-1}'}} if i%20==1 else {})) for i in range(1200)]
                page.route('**/api/sessions?*',lambda route:route.fulfill(json={'sessions':rows,'sig':'toggle-fixture'}))
                page.evaluate('''rows=>{S.sessions=rows;S.results=null;S.term='';S.closed.clear();S.nestClosed.clear();S.off.clear();S.view='tree';S.nest=false;renderView();renderSide();}''',rows)
                logical='[...document.querySelectorAll("#side > .group")].reduce((n,g)=>n+(g._rows?.length||0),0)'
                # 1200 open rows exceed the windowing threshold: the DOM holds the viewport only.
                assert page.evaluate(logical)==1200
                assert 0 < page.locator('#side .item').count() < 120
                unchanged=page.locator('.item[data-uid="claude:toggle-3"]')
                handle=unchanged.element_handle()
                resource=page.get_by_role('button',name='列表资源',exact=True)
                resource.click()
                page.locator('.item-resources .item-resource-value').first.wait_for()
                assert 0 < page.locator('.item-resources').count() < 80
                for index in range(2):
                    page.locator('#nest-flat' if page.evaluate('S.nest') else '#nest-toggle').click()
                    assert unchanged.evaluate('(e,old)=>e===old',handle)
                    assert page.evaluate(logical)==1200 and page.locator('#side .item').count() < 120
                    nested=page.get_by_role('button',name='分层显示',exact=True).get_attribute('aria-pressed')=='true'
                    assert page.locator('.item[data-uid="claude:toggle-1"]').get_attribute('data-depth')==('1' if nested else '0')
                last_uid=page.evaluate('[...document.querySelectorAll("#side > .group")].at(-1)._rows.at(-1).s.uid')
                last=page.locator(f'#side .item[data-uid="{last_uid}"]')
                page.evaluate("document.querySelector('#side').scrollTop=document.querySelector('#side').scrollHeight")
                last.locator('.item-resource-value').first.wait_for()
                assert page.locator('.item-resources').count() < 100
                for cycle in range(12):
                    resource.click()
                    assert page.locator('.item-resources:visible').count()==0
                    page.evaluate('() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
                    resource.click()
                    try:
                        # Turning the column on changes row heights. The last
                        # logical row can now lie beyond the observer's margin;
                        # scroll it into view before expecting lazy resource DOM.
                        last.scroll_into_view_if_needed()
                        expect(last).to_be_in_viewport()
                        last.locator('.item-resource-value').first.wait_for()
                    except Exception:
                        print('RESOURCE DIAGNOSTICS', cycle, page.evaluate('''uid => {
                            const side=document.querySelector('#side'), row=side.querySelector(`[data-uid="${uid}"]`);
                            return {sessions:S.sessions.length, last:S.sessions.at(-1)?.uid,
                                top:side.scrollTop,height:side.scrollHeight,client:side.clientHeight,
                                row:row?.getBoundingClientRect().toJSON(), box:side.getBoundingClientRect().toJSON(),
                                html:row?.outerHTML, windows:[...side.querySelectorAll('.group')].map(g=>g._window)};
                        }''',last_uid), flush=True)
                        raise
                assert not errors,errors
                print('PASS 1200 rows: windowed rows, viewport-only resource DOM, scroll hydration, nesting identity/depth and 12 resource toggle cycles',flush=True)
            finally:browser.close()
if __name__=='__main__':main()
