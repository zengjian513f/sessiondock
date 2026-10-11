#!/usr/bin/env python3
"""Exercise sidebar icon state styling through actual pages with synthetic live responses."""
import argparse
from pathlib import Path
import tempfile
from playwright.sync_api import sync_playwright,expect
from history_fixtures import Corpus,codex_row,codex_message,isolated_server,BINARY

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--screenshots',type=Path)
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-status-') as temp,sync_playwright() as pw:
        root=Path(temp);corpus=Corpus(root)
        for sid in ['active','inactive']:
            corpus.put(sid,'codex',[codex_row('session_meta',{'id':sid,'cwd':'/synthetic/status'}),codex_message('user',sid+' 会话状态预览'),codex_message('assistant','Synthetic response',2)],[])
        (root/'state').mkdir()
        with isolated_server(corpus,args.binary,state_dir=root/'state') as (base,_):
            browser=pw.chromium.launch();context=browser.new_context();page=context.new_page()
            current={'state':'working'};uid=corpus.uid('active')
            def sessions(route):
                result=route.fetch();data=result.json()
                for row in data.get('sessions',[]):
                    if row['uid']==uid:row['turn']='working' if current['state'] in ('working','managed') else 'waiting' if current['state']=='waiting' else 'idle'
                route.fulfill(response=result,json=data)
            page.route('**/api/sessions?*',sessions)
            page.route('**/api/sessions',sessions)
            page.route('**/api/live*',lambda r:r.fulfill(json={'known':True,'uids':[uid] if current['state']!='stopped' else [],'tmux_uids':[uid] if current['state']=='managed' else [],'working_uids':[]}))
            for width,theme in [(1280,'light'),(1280,'dark'),(390,'light'),(390,'dark')]:
                page.set_viewport_size({'width':width,'height':800})
                for state in ['working','managed','idle','waiting','stopped']:
                    current['state']=state;page.goto(base,wait_until='networkidle')
                    page.evaluate('theme=>document.documentElement.dataset.theme=theme',theme)
                    row=page.locator(f'#side .item[data-uid="{uid}"]')
                    expect(row).to_be_attached()
                    if width < 720 and not row.is_visible():
                        page.locator('.mobile-back').click()
                    expect(row).to_be_visible();dot=row.locator(':scope > .ico > .item-status')
                    expect(page.locator(f'#side .item[data-uid="{corpus.uid("inactive")}"] > .ico > .item-status')).to_be_hidden()
                    if state=='stopped':expect(dot).to_be_hidden()
                    else:
                        expect(dot).to_be_visible()
                        expected='turn-pulse' if state in ('working','managed') else 'none'
                        expect(dot).to_have_css('animation-name',expected)
                        expect(dot).to_have_css('width','17px' if state=='waiting' else '8px');expect(dot).to_have_css('height','17px' if state=='waiting' else '8px')
                        palette={'light':['rgb(34, 197, 94)','rgb(14, 165, 233)','rgb(251, 191, 36)'],'dark':['rgb(74, 222, 128)','rgb(56, 189, 248)','rgb(251, 191, 36)']}
                        expect(dot).to_have_css('background-color',palette[theme][1 if state=='managed' else 2 if state=='waiting' else 0])
                        if state in ('working','managed'):
                            expect(dot).to_have_css('animation-duration','1.4s')
                            page.emulate_media(reduced_motion='reduce');expect(dot).to_have_css('animation-name','none');page.emulate_media(reduced_motion='no-preference')
                    if args.screenshots:
                        args.screenshots.mkdir(parents=True,exist_ok=True)
                        page.locator('#side').screenshot(path=str(args.screenshots/f'sidebar-{width}-{theme}-{state}.png'))
                    page.locator(f'#side .item[data-uid="{corpus.uid("inactive")}"]').click()
                    expect(page.locator('#msgs')).to_contain_text('Synthetic response')
                print('PASS sidebar icon states, inactive hidden, theme, reduced motion and click',width,theme,flush=True)
            browser.close()

if __name__=='__main__':main()
