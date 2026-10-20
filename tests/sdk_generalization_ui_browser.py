#!/usr/bin/env python3
"""Native identity reconciliation, historical menus and Codex Token details."""
import argparse
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from sdk_start_stop_fixtures import provider
from sdk_selection_fixtures import snapshots
from sdk_ui_fixtures import start_sdk
from popups import on_popup


def exercise(browser,base,source,events,screenshots):
    context=browser.new_context(viewport={'width':1440,'height':900},service_workers='block')
    page=context.new_page()
    on_popup(page,lambda dialog:dialog.accept())
    errors=[]
    page.on('pageerror',lambda error:errors.append(str(error)))
    api='codex-sdk' if source=='codex' else 'native-sdk'
    caps={'fork':source!='agy','revert':source=='opencode','rewind':source=='grok','models':True,'settings':True}
    raw={'items':[],'requests':[],'running':False,'last_turn':{'status':'completed'},
         'model':'shared-model','effort':'low','capabilities':caps,
         'summary':{'settings':{'model':'shared-model','effort':'low'},'queue':[],'status':{'type':'idle'},
                    'usage':{'total':{'inputTokens':12000,'outputTokens':3000,'cachedInputTokens':8000,
                                     'reasoningOutputTokens':1000,'totalTokens':15000},
                             'last':{'totalTokens':2000},'modelContextWindow':200000}}}
    def identity(index):
        return {'grok':f'prompt:{index}','agy':f'step:{index}',
                'claude':f'native-user-{index}','opencode':f'message:native-user-{index}',
                'codex':f'turn-{index}'}[source]
    def live(index):
        return {'id':f'live-user-{index}','role':'user','text':'重复输入内容',
                **({'prompt_index':index} if source=='grok' else
                   {'native_id':f'agy:synthetic:step:{index}'} if source=='agy' else
                   {'native_id':f'native-user-{index}'})}
    history=[]
    activity={'state':'idle'}
    for index in (11,12):
        history.extend([{'role':'user','text':'重复输入内容','turn_id':identity(index)},
                        {'role':'assistant','phase':'final','text':f'历史答复 {index}','turn_id':identity(index)}])
    def messages(route):
        response=route.fetch()
        data=response.json()
        data.update(messages=history,reset=True,start=0,end=104,message_total=len(history),partial=None,activity=activity)
        route.fulfill(response=response,json=data)
    context.route('**/api/messages/**',messages)
    context.route(f'**/api/{api}/open',lambda r:r.fulfill(json={'ok':True,'capabilities':caps,'raw':{}}))
    context.route(f'**/api/{api}/events?*',lambda r:r.continue_(url=events.url))
    context.route(f'**/api/{api}/usage',lambda r:r.fulfill(json={'ok':True,'raw':{}}))
    context.route(f'**/api/{api}/limits*',lambda r:r.fulfill(json={'ok':True,'raw':{}}))
    model_requests=[]
    def models(route):
        if route.request.method=='POST':
            model_requests.append(route.request.post_data_json)
        route.fulfill(json={'ok':True,'raw':{'models':[{'id':'shared-model','name':'共享原生模型','efforts':['low','high']}]}})
    context.route(f'**/api/{api}/models*',models)
    events.packet={'seq':1,'type':'snapshot','raw':raw}
    page.goto(base,wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    row=page.locator(f'#side .item[data-uid^="{source}:"]').first
    if not row.is_visible():page.locator(f'#chips button[data-source="{source}"]').click()
    row.click()
    uid=row.get_attribute('data-uid')
    page.locator('#a-sdk').click()
    start_sdk(page)
    if source=='codex':
        page.locator('#sdk-more').click()
        detail=page.locator('#sdk-token-detail')
        expect(detail).to_be_visible()
        for text in ('15,000','12,000','3,000','8,000','1,000','思考计入输出'):
            expect(detail).to_contain_text(text)
        expect(detail).not_to_contain_text('成本')
    else:
        # Equal text with a different native identity must not crop any history.
        raw['items']=[live(13)]
        events.publish()
        expect(page.locator('.sdk-live .msg[data-role="user"]')).to_have_count(1)
        expect(page.locator('.sdk-history .msg[data-role="user"]')).to_have_count(2)
        expect(page.locator('.sdk-history')).to_contain_text('历史答复 12')
        # Once the exact native identity appears, only its own tail is replaced.
        raw['items']=[live(12)]
        events.publish()
        expect(page.locator('.sdk-history .msg[data-role="user"]')).to_have_count(1)
        expect(page.locator('.sdk-history')).to_contain_text('历史答复 11')
        expect(page.locator('.sdk-history')).not_to_contain_text('历史答复 12')
        if source!='agy':
            page.locator('.sdk-history .sdk-bubble-toggle').click()
            expect(page.locator('.sdk-bubble-menu:popover-open')).to_contain_text('分支')
            actions=[]
            def action(route):
                actions.append(route.request.post_data_json)
                route.fulfill(json={'ok':True,'uid':uid})
            context.route('**/api/native-sdk/fork',action)
            context.route('**/api/native-sdk/rewind',action)
            context.route('**/api/native-sdk/revert',action)
            page.locator('.sdk-bubble-menu:popover-open button').filter(has_text='分支').click()
            expect(page.locator('#sdk-input')).to_have_value('重复输入内容')
            assert actions[-1].get('prompt_index' if source=='grok' else 'before') == (11 if source=='grok' else 'native-user-11'),actions
            assert 'restore_text' not in actions[-1],actions
            if source in ('grok','opencode'):
                page.locator('#sdk-input').fill('保留草稿')
                page.locator('.sdk-history .sdk-bubble-toggle').click()
                page.locator('.sdk-bubble-menu:popover-open button').filter(has_text='回退').click()
                expect(page.locator('#sdk-input')).to_have_value('保留草稿\n\n重复输入内容')
                assert actions[-1].get('prompt_index' if source=='grok' else 'message_id') == (11 if source=='grok' else 'native-user-11'),actions
            page.locator('.sdk-history .sdk-bubble-toggle').click()
    for width,theme in ((1440,'light'),(390,'dark')):
        page.set_viewport_size({'width':width,'height':900})
        page.emulate_media(color_scheme=theme)
        page.evaluate('() => new Promise(done=>requestAnimationFrame(()=>requestAnimationFrame(done)))')
        if not page.locator('.sdk-pane').is_visible():
            page.locator('#side .item.sel').click()
            start_sdk(page)
        if source=='codex' and not page.locator('#sdk-more-menu').is_visible():
            page.locator('#sdk-more').click()
        elif source not in ('codex','agy') and not page.locator('.sdk-bubble-menu:popover-open').count():
            page.locator('.sdk-history .sdk-bubble-toggle').click()
        expect(page.locator('.sdk-pane')).to_be_visible()
        page.screenshot(path=str(screenshots/f'{source}-{width}-{theme}.png'),full_page=True)
    if source!='codex':
        page.set_viewport_size({'width':1440,'height':900})
        page.keyboard.press('Escape')
        for outcome,label in [('interrupted','已中断'),('incomplete','未完成：上一回合未正常结束')]:
            raw['last_turn']={'status':outcome}
            events.publish()
            expect(page.locator('.sdk-pane .sdk-status')).to_have_attribute('title',label)
            page.reload(wait_until='domcontentloaded')
            page.locator('#a-sdk').click()
            start_sdk(page)
            expect(page.locator('.sdk-pane .sdk-status')).to_have_attribute('title',label)
        if source=='claude':
            raw['last_turn']=None
            activity['state']='working'
            events.publish()
            page.reload(wait_until='domcontentloaded')
            page.locator('#a-sdk').click()
            start_sdk(page)
            expect(page.locator('.sdk-pane .sdk-status')).to_have_attribute('data-state','ready')
            history.append({'role':'user','text':'新的未完成输入','turn_id':'later-native-user'})
            page.reload(wait_until='domcontentloaded')
            page.locator('#a-sdk').click()
            start_sdk(page)
            expect(page.locator('.sdk-pane .sdk-status')).to_have_attribute('title','未完成：上一回合未正常结束')
        page.set_viewport_size({'width':1440,'height':900})
        page.keyboard.press('Escape')
        page.locator('#new-session').click()
        page.locator(f'#new-session-form input[value="{source}"]').check()
        page.locator('#new-model').click()
        expect(page.locator('#new-model-options')).to_contain_text('共享原生模型')
        assert any(row.get('source')==source and 'uid' not in row for row in model_requests),model_requests
        page.keyboard.press('Escape')
    assert not errors,errors
    context.close()
    print(f'PASS {source}: repeated input identity, history menus or native Token details',flush=True)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--screenshots',type=Path,default=Path('target/sdk-generalization-ui'))
    args=parser.parse_args()
    args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-parity-ui-') as temporary,sync_playwright() as playwright:
        browser=playwright.chromium.launch(headless=True)
        try:
            for source in ('grok','agy','opencode','claude','codex'):
                root=Path(temporary)/source
                root.mkdir()
                with provider(source,root,args.binary) as (base,_,_),snapshots() as events:
                    exercise(browser,base,source,events,args.screenshots)
        finally:browser.close()


if __name__=='__main__':
    main()
