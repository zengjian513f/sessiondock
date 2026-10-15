#!/usr/bin/env python3
"""Real sidebar wheel, touch, divider and collapse inputs with persisted results."""
import argparse
import json
from pathlib import Path
import tempfile
from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, build_corpus, isolated_server
from browser_evidence_fixtures import run_with_evidence


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--output',type=Path,default=Path('target/sidebar-input'))
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    evidence=[]
    with tempfile.TemporaryDirectory(prefix='sessiondock-sidebar-input-') as tmp, sync_playwright() as pw:
        corpus=build_corpus(Path(tmp))
        with isolated_server(corpus,args.binary) as (base,_):
            browser=pw.chromium.launch(headless=True)
            try:
                for touch in [False,True]:
                    context=browser.new_context(viewport={'width':814 if touch else 1280,'height':380},has_touch=touch)
                    page=context.new_page();page.goto(base,wait_until='networkidle')
                    page.locator(f'#side .item[data-uid="{corpus.uid("claude-branch")}"]').click()
                    expect(page.locator('#msgs')).to_contain_text('Claude selected answer')
                    side=page.locator('#side');side.hover()
                    page.mouse.wheel(0,10000)
                    page.wait_for_function("document.querySelector('#side').scrollTop > 0")
                    expect(side.locator('.item').last).to_be_in_viewport()
                    page.mouse.wheel(0,-10000)
                    page.wait_for_function("document.querySelector('#side').scrollTop === 0")
                    expect(side.locator('.item').first).to_be_in_viewport()
                    box=page.locator('#drag').bounding_box();x=box['x']+box['width']/2;y=box['y']+100
                    before=page.locator('#left').bounding_box()['width']
                    if touch:
                        cdp=context.new_cdp_session(page)
                        cdp.send('Input.dispatchTouchEvent',{'type':'touchStart','touchPoints':[{'x':x,'y':y}]})
                        cdp.send('Input.dispatchTouchEvent',{'type':'touchMove','touchPoints':[{'x':x+80,'y':y}]})
                        cdp.send('Input.dispatchTouchEvent',{'type':'touchEnd','touchPoints':[]})
                    else:
                        page.mouse.move(x,y);page.mouse.down();page.mouse.move(x+80,y,steps=8);page.mouse.up()
                    page.wait_for_function('width => Math.abs(document.querySelector("#left").getBoundingClientRect().width-width)<3',arg=before+80)
                    assert abs(float(page.evaluate("localStorage.getItem('sessiondock.width')"))-(before+80))<3
                    page.reload(wait_until='networkidle')
                    assert abs(page.locator('#left').bounding_box()['width']-(before+80))<3
                    if touch:
                        box=page.locator('#drag').bounding_box();x=box['x']+box['width']/2
                        cdp.send('Input.dispatchTouchEvent',{'type':'touchStart','touchPoints':[{'x':x,'y':y}]})
                        cdp.send('Input.dispatchTouchEvent',{'type':'touchMove','touchPoints':[{'x':x-50,'y':y}]})
                        cdp.send('Input.dispatchTouchEvent',{'type':'touchCancel','touchPoints':[]})
                        assert abs(page.locator('#left').bounding_box()['width']-(before+80))<3
                        bounds=side.bounding_box();tx=bounds['x']+80
                        cdp.send('Input.dispatchTouchEvent',{'type':'touchStart','touchPoints':[{'x':tx,'y':bounds['y']+bounds['height']-30}]})
                        for i in range(1,9):
                            cdp.send('Input.dispatchTouchEvent',{'type':'touchMove','touchPoints':[{'x':tx,'y':bounds['y']+bounds['height']-30-i*25}]})
                            page.wait_for_timeout(20)
                        cdp.send('Input.dispatchTouchEvent',{'type':'touchEnd','touchPoints':[]})
                        page.wait_for_function("document.querySelector('#side').scrollTop > 0")
                    else:
                        page.locator('#drag').dblclick()
                        page.wait_for_function('width => Math.abs(document.querySelector("#left").getBoundingClientRect().width-width)<3',arg=before)
                    toggle=page.locator('#side-toggle');toggle.click()
                    expect(toggle).to_have_attribute('aria-expanded','false')
                    page.keyboard.press('Tab');page.keyboard.press('Shift+Tab')
                    expect(toggle).to_be_focused();page.keyboard.press('Enter')
                    expect(toggle).to_have_attribute('aria-expanded','true')
                    page.screenshot(path=str(args.output/f'sidebar-{touch}.png'))
                    evidence.append({'touch':touch,'width_before':before,'persisted_width':before+80,'passed':['real row click','wheel reaches first/last','divider resize and reload','touch cancel/scroll' if touch else 'double-click reset','collapse and Tab/Enter expand']})
                    context.close()
                (args.output/'result.json').write_text(json.dumps({'scenarios':evidence},indent=2)+'\n')
                print('PASS sidebar real wheel/touch/divider/persistence/collapse and Tab',flush=True)
            finally:browser.close()


if __name__ == '__main__':
    run_with_evidence(main, 'target/sidebar-input')
