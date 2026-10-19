#!/usr/bin/env python3
"""Four native SDKs: selections apply immediately, independently of SEND."""
import argparse
import json
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from native_sdk_footer_browser import provider
from sdk_selection_fixtures import snapshots



def exercise(browser, base, source, events, width, theme, screenshots):
    label = f'{source}-{width}-{theme}'
    context = browser.new_context(viewport={'width':width,'height':900}, color_scheme=theme,
                                  service_workers='block', permissions=['local-network-access'])
    context.route('**/*',lambda route:route.continue_() if route.request.url.startswith(base+'/') else route.abort())
    page = context.new_page()
    errors = []
    page.on('pageerror',lambda error:errors.append(str(error)))
    capabilities = {'models':True,'settings':True,'mode':source in ('grok','agy'),
                    'permission_mode':source=='claude','fast_mode':source=='claude'}
    raw = {'model':'short','effort':'low','running':True,'items':[],'requests':[],
           'capabilities':capabilities,'agent':'build','agents':['build','plan'],
           'settings':{'permission_mode':'default','fast_mode':False,'mode':'auto'},
           'modes':{'currentModeId':'auto','availableModes':['auto','plan']} if source in ('grok','agy') else []}
    events.packet = {'type':'snapshot','seq':1,'raw':raw}
    context.route('**/api/native-sdk/open',lambda route:route.fulfill(json={'ok':True,'capabilities':capabilities}))
    context.route('**/api/native-sdk/events?*',lambda route:route.continue_(url=events.url))
    context.route('**/api/native-sdk/models',lambda route:route.fulfill(json={'ok':True,'raw':[
        {'id':'short','efforts':['low','high']},{'id':'long','efforts':['low','high']}]}))
    context.route('**/api/native-sdk/usage',lambda route:route.fulfill(json={'ok':True}))
    context.route('**/api/native-sdk/settings',lambda route:route.fulfill(json={'ok':True}))
    submissions = []
    def submit(route):
        submissions.append(route.request.post_data_json)
        route.fulfill(json={'ok':True,'accepted':True,'state':'queued'})
    context.route('**/api/native-sdk/turn',submit)
    page.goto(base,wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    row = page.locator(f'#side .item[data-uid^="{source}:"]').first
    if not row.is_visible():page.locator(f'#chips button[data-source="{source}"]').click()
    row.click();page.locator('#a-sdk').click()
    expect(page.locator('#sdk-model')).to_have_value('short')
    box,send = page.locator('#sdk-input'),page.locator('#sdk-send')
    box.fill('草稿保留，选择立即执行 / '+label)
    def publish():
        events.publish()
        page.wait_for_function('seq=>NativeSdkView.seq===seq',arg=events.packet['seq'])
    def shot(name):
        if screenshots:page.screenshot(path=str(screenshots/f'{label}-{name}.png'))
    def apply(key,value):
        if key in ('model','effort'):raw[key]=value
        elif key=='mode':raw['modes']['currentModeId']=value
        elif key=='native_agent':raw['agent']=value
        else:raw['settings'][key]=value
    definitions = [('model','long','short'),('effort','high','low')]
    definitions += [('mode','plan','auto')] if source in ('grok','agy') else (
        [('permission_mode','plan','default'),('fast_mode','true','false')] if source=='claude'
        else [('native_agent','plan','build')])
    for key,chosen,original in definitions:
        control = page.locator(('#sdk-' if key in ('model','effort') else '#native-sdk-')+key)
        if not control.is_visible():page.locator('#sdk-more').click()
        with page.expect_response('**/api/native-sdk/settings') as response:control.select_option(chosen)
        body = response.value.request.post_data_json
        assert body[key]==(True if key=='fast_mode' else chosen),body
        expect(control).to_have_value(chosen);expect(control).to_be_disabled();expect(send).to_be_disabled()
        publish()
        expect(control).to_have_value(chosen);expect(control).to_be_disabled()
        expect(box).to_have_value('草稿保留，选择立即执行 / '+label)
        assert not submissions,'changing an option sent a model prompt'
        shot(key+'-pending')
        apply(key,True if key=='fast_mode' else chosen);publish()
        expect(control).to_have_value(chosen);expect(control).to_be_enabled();expect(send).to_be_enabled()
        page.route('**/api/native-sdk/settings',lambda route:route.fulfill(status=409,
            json={'ok':False,'error':'Synthetic setting rejected'}),times=1)
        if not control.is_visible():page.locator('#sdk-more').click()
        control.select_option(original)
        expect(page.locator('#sdk-status-menu')).to_contain_text('Synthetic setting rejected')
        expect(control).to_have_value(chosen);expect(control).to_be_enabled()
        page.keyboard.press('Escape')
    def send_text(text):
        box.fill(text);send.click();expect(box).to_have_value('')
    send_text('queued before a later immediate setting '+label)
    control = page.locator('#sdk-model')
    if not control.is_visible():page.locator('#sdk-more').click()
    with page.expect_response('**/api/native-sdk/settings') as response:control.select_option('short')
    assert response.value.request.post_data_json['model']=='short'
    raw['model']='short';publish()
    expect(control).to_be_enabled()
    page.keyboard.press('Escape')
    send_text('second queued message '+label)
    for body in submissions:assert 'model' not in body and 'effort' not in body,body
    publish()
    expect(control).to_have_value('short');expect(page.locator('#sdk-effort')).to_have_value('high')
    before=len(submissions)
    box.fill('/effort low')
    with page.expect_response('**/api/native-sdk/settings') as response:send.click()
    assert response.value.request.post_data_json['effort']=='low'
    raw['effort']='low';publish()
    expect(box).to_have_value('')
    expect(page.locator('#sdk-effort')).to_have_value('low')
    assert len(submissions)==before,'slash setting became a prompt'
    shot('confirmed')
    raw.update(model='external-model',effort='low');publish()
    expect(control).to_have_value('external-model')
    assert not errors,errors
    context.close()
    print(f'PASS {label}: immediate model/effort/settings without SEND, delayed confirmation/rejection, draft, later queue no overrides',flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-native-selections-') as temporary, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in ('grok', 'agy', 'opencode', 'claude'):
                root = Path(temporary) / source; root.mkdir()
                with provider(source, root, args.binary) as base, snapshots() as events:
                    for width, theme in [(1280, 'light'), (390, 'dark')]:
                        exercise(browser, base, source, events, width, theme, args.screenshots)
        finally: browser.close()


if __name__ == '__main__': main()
