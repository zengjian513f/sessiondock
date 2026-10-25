"""Real Chromium pointer/keyboard gestures for the shared SDK send button."""
# run_validation: skip
from playwright.sync_api import expect


def exercise_idle_send_gestures(page, source, label, finish, screenshots=None):
    box=page.locator('#sdk-input');button=page.locator('#sdk-send')
    requests=[]
    def record(request):
        if request.method=='POST' and ('/api/codex-sdk/turn' in request.url or '/api/native-sdk/steer' in request.url or '/api/native-sdk/turn' in request.url):requests.append(request)
    page.on('request',record)
    cdp=page.context.new_cdp_session(page)
    try:
        for gesture in ('keyboard','ctrl-click','touch-hold','action'):
            text=f'idle {gesture} {label}';box.fill(text);before=len(requests)
            if screenshots:page.screenshot(path=str(screenshots/f'{source}-{label}-idle-{gesture}-before.png'),full_page=True)
            if gesture=='keyboard':box.press('Control+Enter')
            elif gesture=='ctrl-click':button.click(modifiers=['Control'])
            elif gesture=='action':
                action=page.get_by_role('button',name='引导当前任务',exact=True)
                if not action.is_visible():page.locator('#sdk-more').click()
                action.click()
            else:
                button.scroll_into_view_if_needed();r=button.bounding_box()
                cdp.send('Input.dispatchTouchEvent',{'type':'touchStart','touchPoints':[{'x':r['x']+r['width']/2,'y':r['y']+r['height']/2,'id':1}]})
                page.wait_for_timeout(650)
                cdp.send('Input.dispatchTouchEvent',{'type':'touchEnd','touchPoints':[]})
            try:expect(box).to_have_value('')
            finally:
                if screenshots:page.screenshot(path=str(screenshots/f'{source}-{label}-idle-{gesture}-after.png'),full_page=True)
            expect(button).to_be_enabled()
            finish(text)
            assert len(requests)==before+1,(gesture,len(requests)-before)
            request=requests[-1]
            assert '/turn' in request.url and 'expected_turn_id' not in request.post_data_json,request.post_data_json
            expect(page.locator('#sdk-status-menu')).not_to_contain_text('当前没有')
    finally:
        cdp.detach();page.remove_listener('request',record)


def exercise_send_gestures(page, source, label, screenshots=None):
    box=page.locator('#sdk-input');button=page.locator('#sdk-send')
    view='CodexSdkView' if source=='codex' else 'NativeSdkView'
    requests=[]
    def record(request):
        if request.method=='POST' and ('/api/codex-sdk/turn' in request.url or '/api/native-sdk/steer' in request.url or '/api/native-sdk/turn' in request.url):requests.append(request)
    page.on('request',record)
    box.fill('Ctrl click '+label);page.keyboard.down('Control')
    expect(button).to_have_text('引导')
    page.evaluate(view+'.paintStatus()');expect(button).to_have_text('引导')
    button.click();expect(box).to_have_value('')
    expect(button).to_be_enabled();page.keyboard.up('Control');expect(button).to_have_text('排队')
    page.keyboard.down('Control');expect(button).to_have_text('引导')
    page.evaluate("window.dispatchEvent(new Event('blur'))")
    expect(button).to_have_text('排队');page.keyboard.up('Control')
    cdp=page.context.new_cdp_session(page)
    def touch(kind,x,y):
        cdp.send('Input.dispatchTouchEvent',{'type':kind,'touchPoints':[] if kind in ('touchEnd','touchCancel') else [{'x':x,'y':y,'id':1}]})
    def position():
        button.scroll_into_view_if_needed();r=button.bounding_box();return r['x']+r['width']/2,r['y']+r['height']/2
    box.fill('touch hold '+label);x,y=position();before=len(requests)
    touch('touchStart',x,y);page.wait_for_timeout(650)
    expect(button).to_have_text('引导');expect(box).to_have_value('')
    if screenshots:page.screenshot(path=str(screenshots/f'{source}-{label}-held.png'),full_page=True)
    touch('touchEnd',x,y);expect(button).to_have_text('排队');page.wait_for_timeout(150)
    assert len(requests)==before+1,'long press sent more than one input'
    box.fill('cancelled touch '+label);before=len(requests);x,y=position()
    touch('touchStart',x,y);touch('touchCancel',x,y);page.wait_for_timeout(600)
    expect(box).to_have_value('cancelled touch '+label);assert len(requests)==before
    touch('touchStart',x,y);touch('touchMove',x-35,y-35);page.wait_for_timeout(600);touch('touchEnd',x-35,y-35)
    expect(box).to_have_value('cancelled touch '+label);assert len(requests)==before
    touch('touchStart',x,y);page.evaluate("window.dispatchEvent(new Event('blur'))");page.wait_for_timeout(600);touch('touchEnd',x,y)
    expect(box).to_have_value('cancelled touch '+label);assert len(requests)==before
    box.fill('short tap '+label);x,y=position()
    touch('touchStart',x,y);touch('touchEnd',x,y);expect(box).to_have_value('')
    row=page.locator('.sdk-queued').filter(has_text='short tap '+label)
    expect(row).to_be_visible();row.get_by_role('button',name='撤销',exact=True).click();expect(row).to_have_count(0)
    cdp.detach();page.remove_listener('request',record)
