"""Per-agent settings govern report SDK workers and first message handoff on node/Hub."""
import argparse
import json
import re
import tempfile
from pathlib import Path
from types import SimpleNamespace
from playwright.sync_api import expect, sync_playwright
from sdk_start_stop_fixtures import provider
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from bug_report_node_browser import open_report, wait_drafts
from session_start_fixtures import choose_mode


def exercise(browser, base, source, root, label):
    print(label, source, 'report SDK creation/first SEND', flush=True)
    page = browser.new_page(viewport={'width':1280,'height':900})
    page.set_default_timeout(20000)
    errors=[]
    page.on('pageerror',lambda e: errors.append(str(e)))
    page.goto(base, wait_until='networkidle')
    choose_mode(page, source, 'cli')
    page.locator('#settings-dialog .modal-close').click()
    page.reload(wait_until='networkidle')
    choose_mode(page, source, 'sdk')
    expect(page.locator(f'[data-agent-start-mode="{source}"]')).to_have_value('sdk')
    page.screenshot(path=f'/tmp/sessiondock-preference-{label}-{source}-settings.png')
    page.locator('#settings-dialog .modal-close').click()
    from new_session_model_browser import open_dialog
    open_dialog(page)
    page.locator(f'input[name="new-source"][value="{source}"]').check()
    page.locator('#new-cwd').fill(str(root/'report-work'))
    with page.expect_response(lambda r: r.url.endswith('/create') and r.request.method=='POST') as created:
        page.locator('#new-session-go').click()
    assert created.value.ok, created.value.text()
    expect(page.locator('#sdk-input')).to_be_visible()
    page.wait_for_function('SdkView.connected')
    expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label','停止会话')
    text='First ordinary SDK message '+source+' '+label
    page.locator('#sdk-input').fill(text)
    with page.expect_response(lambda r: r.url.endswith('/turn') and r.request.method=='POST') as sent:
        page.locator('#sdk-send').click()
    assert sent.value.ok, sent.value.text()
    expect(page.locator('#sdk-input')).to_have_value('')
    page.screenshot(path=f'/tmp/sessiondock-preference-{label}-{source}-first.png')
    original=page.evaluate('S.sel')
    page.wait_for_function('uid => S.sessions.some(row => row.uid === uid)',arg=original)
    from session_global_actions_browser import click_action
    click_action(page, '[data-continue-session]')
    page.locator('#continue-session-description').fill('Continue using the configured SDK')
    # Hold the initial SDK stream snapshot after create/draft persistence.
    # An open response is not evidence that the SEND controls are connected.
    held = []
    turns = []
    page.route('**/api/*-sdk/events?**', lambda route: held.append(route))
    page.on('request', lambda request: turns.append(request.post_data_json)
            if request.method == 'POST' and request.url.endswith('/turn') else None)
    page.locator('#continue-session-go').click()
    page.wait_for_function("document.querySelector('#sdk-input')?.value.includes('Continue using the configured SDK')")
    expect(page.locator('#sdk-send')).to_be_disabled()
    page.screenshot(path=f'/tmp/sessiondock-preference-{label}-{source}-continuation-wait.png')
    page.wait_for_timeout(1000)  # Keep the stream behind the completed start/open calls.
    assert held and not turns, (held, turns)
    with page.expect_response(lambda r: r.url.endswith('/turn') and r.request.method=='POST') as continued:
        for route in held:
            route.continue_()
        page.unroute('**/api/*-sdk/events?**')
    assert continued.value.ok, continued.value.text()
    expect(page.locator('#sdk-input')).to_have_value('')
    assert len(turns) == 1 and 'Continue using the configured SDK' in turns[0]['text'], turns
    if label == 'hub':
        page.set_viewport_size({'width':390,'height':844})
        page.emulate_media(color_scheme='dark')
        page.locator('.item.sel').click()
        expect(page.locator('#sdk-input')).to_be_visible()
    page.screenshot(path=f'/tmp/sessiondock-preference-{label}-{source}-continuation-sent.png')
    if label == 'hub':
        page.set_viewport_size({'width':1280,'height':900})
        page.emulate_media(color_scheme='light')
    assert page.evaluate('S.sel') != original
    page.wait_for_function("""original => {
      const local = uid => uid.replace(/^([^:]+):[a-f0-9]{32}~/, '$1:');
      return S.sessions.find(row => row.uid === S.sel)?.predecessors?.some(edge => local(edge.session_uid) === local(original));
    }""",arg=original)
    if source == 'claude' and label == 'node':
        for cancel in (True, False):
            page.locator(f'.item[data-uid="{original}"]').click()
            click_action(page, '[data-continue-session]')
            text = 'Keep this continuation draft after ' + ('leaving' if cancel else 'open failure')
            page.locator('#continue-session-description').fill(text)
            held = []
            pattern = '**/api/*-sdk/events?**' if cancel else '**/api/*-sdk/open'
            def intercept(route):
                if cancel:
                    held.append(route)
                else:
                    route.fulfill(status=503, content_type='application/json',
                                  body=json.dumps({'error':'synthetic SDK attachment failure'}))
            page.route(pattern, intercept)
            before = len(turns)
            page.locator('#continue-session-go').click()
            expect(page.locator('#sdk-input')).to_have_value(
                re.compile('.*' + text, re.S))
            if cancel:
                page.wait_for_timeout(1000)
                assert held
                cancelled = page.evaluate('S.sel')
                page.locator(f'.item[data-uid="{original}"]').click()
                for route in held:
                    route.abort()
                page.unroute(pattern)
                page.wait_for_function('!newSessionCreating')
                page.locator(f'.item[data-uid="{cancelled}"]').click()
            else:
                expect(page.locator('.sdk-status.blocked')).to_have_attribute(
                    'title', re.compile('.*synthetic SDK attachment failure.*'))
                page.unroute(pattern)
            page.wait_for_timeout(300)
            assert len(turns) == before, turns
            expect(page.locator('#sdk-input')).to_have_value(
                re.compile('.*' + text, re.S))
            page.screenshot(path=f'/tmp/sessiondock-preference-{label}-{source}-continuation-{"cancel" if cancel else "failure"}.png')
    open_report(page)
    page.locator(f'input[name="bug-report-source"][value="{source}"]').check()
    page.locator('#bug-report-description').fill('Launch preference report '+label+' '+source)
    wait_drafts(page)
    lost=[]
    if label == 'node':
        def lose_reply(route):
            lost.append(route.fetch().json())
            route.abort('failed')
        page.route('**/api/bug-report', lose_reply, times=1)
        page.locator('#bug-report-go').click()
        expect(page.locator('#bug-report-error')).not_to_be_empty()
    if lost:
        page.locator('#bug-report-go').click()
        reply=lost[0]
    else:
        with page.expect_response(lambda r: r.url.endswith('/api/bug-report') and r.request.method=='POST') as response:
            page.locator('#bug-report-go').click()
        reply=response.value.json()
        assert response.value.ok, reply
        assert response.value.request.post_data_json['mode']=='sdk'
    assert reply['worker']['mode']=='sdk', reply
    if lost: assert reply['report_id']==lost[0]['report_id'], (lost,reply)
    expect(page.locator('#bug-report-toast')).to_be_visible()
    page.locator('#bug-report-toast').get_by_role('button',name='打开',exact=True).click()
    expect(page.locator('#sdk-input')).to_be_visible()
    expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label','停止会话')
    page.wait_for_function('SdkView.connected')
    # Wait for the node's durable SDK receipt, not a fabricated browser success.
    manifest=root/'reports'/reply['report_id']/'manifest.json'
    import time
    deadline=time.monotonic()+20
    while time.monotonic()<deadline:
        data=json.loads(manifest.read_text())
        if data['status']!='starting': break
        page.wait_for_timeout(100)
    assert data['status']=='submitted', data
    assert data['worker_mode']=='sdk'
    expect(page.locator('#sdk-input')).to_have_value('')
    assert not errors, errors
    page.close()


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary',type=Path,default=BINARY)
    p.add_argument('--sources',default='codex,claude,grok,agy,opencode')
    a=p.parse_args()
    with tempfile.TemporaryDirectory(prefix='sd-start-preference-') as tmp, sync_playwright() as pw:
        browser=pw.chromium.launch(headless=True)
        for source in a.sources.split(','):
            root=Path(tmp)/source;root.mkdir()
            with provider(source,root,a.binary,report=True) as (base,peer,extra):
                exercise(browser,base,source,root,'node')
                hr=root/'hub';hr.mkdir()
                hub=Hub(a.binary.resolve().with_name('sessiondock-hub'),hr,[SimpleNamespace(name='Worker',token=TOKEN,port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':',1)[1]))])
                hub.start()
                try: exercise(browser,f'http://127.0.0.1:{hub.port}',source,root,'hub')
                finally: hub.stop()
        browser.close()
    print('PASS report SDK preference, five providers node/Hub')

if __name__=='__main__': main()
