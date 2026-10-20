"""Explicit SDK start/stop: five providers, real isolated bridges and Chromium."""
from types import SimpleNamespace
from codex_sdk_browser import build, server
from sdk_start_stop_fixtures import provider

def exercise(browser, base, source, screenshots, label):
    from playwright.sync_api import expect
    from popups import on_popup
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        print(f'{label} {source} {width} {theme}', flush=True)
        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme,
                                      service_workers='block', has_touch=width < 720)
        page = context.new_page()
        page.set_default_timeout(15000)
        on_popup(page, lambda dialog: dialog.accept())
        errors, opens = [], []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.on('request', lambda request: opens.append(request.url)
                if request.method == 'POST' and request.url.split('?')[0].endswith('/open') else None)
        # Keep a neighbouring row so sorting is checked as well as timestamps.
        def sessions(route):
            # Force a full inventory so signature-only poll replies cannot
            # bypass the synthetic neighbouring row while mirrors warm up.
            response = route.fetch(url=route.request.url.split('?')[0]+'?force=1')
            data = response.json()
            rows = data.get('sessions', [])
            if rows:
                comparison = dict(rows[0], uid=rows[0]['uid']+'-comparison',
                    title='Neighbouring session: clicking history must not reorder it',
                    sid='comparison-session', updated='2026-09-10T10:00:00Z')
                data['sessions'] = [*rows, comparison]
            route.fulfill(response=response, json=data)
        import re
        page.route(re.compile(r'/api/sessions(?:\?|$)'), sessions)
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded')
        page.wait_for_function("S.sessions.some(row => row.uid.endsWith('-comparison'))")
        row = page.locator(f'#side .item[data-uid^="{source}:"]:not([data-uid$="-comparison"])').first
        if not row.is_visible(): page.locator(f'#chips button[data-source="{source}"]').click()
        uid = row.get_attribute('data-uid')
        from session_start_fixtures import choose_mode
        def set_mode(mode):
            return_to_session = page.locator('#detail .mobile-back').is_visible()
            if return_to_session: page.locator('#detail .mobile-back').click()
            choose_mode(page, source, mode)
            page.locator('#settings-dialog .modal-close').click()
            if return_to_session: row.click()
        set_mode('cli')
        row.click()
        expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
        assert not opens
        set_mode('sdk')
        page.reload(wait_until='networkidle')
        expect(page.locator('#a-sdk')).to_have_attribute('aria-pressed', 'true')
        expect(page.locator('#sdk-composer')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        set_mode('cli')
        page.reload(wait_until='networkidle')
        expect(page.locator('#a-sdk')).to_have_attribute('aria-pressed', 'true')
        page.locator('#a-chat').click()
        set_mode('sdk')
        page.reload(wait_until='networkidle')
        expect(page.locator('#a-chat')).to_have_attribute('aria-pressed', 'true')
        assert not opens
        page.locator('#a-sdk').click()
        if page.locator('#detail .mobile-back').is_visible(): page.locator('#detail .mobile-back').click()
        before = page.evaluate('S.sessions.map(row => [row.uid, row.updated])')
        order = page.locator('#side .item').evaluate_all('(rows) => rows.map(row => row.dataset.uid)')
        row.click()
        expect(page.locator('.sdk-history')).to_be_visible()
        expect(page.locator('#sdk-composer')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label', '启动会话')
        page.locator('#sdk-input').fill('draft before explicit start')
        page.locator('#sdk-input').press('Enter')
        expect(page.locator('#sdk-input')).to_have_value('draft before explicit start' + ('\n' if width < 720 else ''))
        assert not opens, opens
        assert not page.evaluate('uid => S.liveSdk.has(uid)', uid)
        page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-cold.png'))
        # SDK view switches and reloads also remain read-only.
        page.locator('#a-chat').click()
        page.locator('#a-sdk').click()
        expect(page.locator('#sdk-composer')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        page.reload(wait_until='networkidle')
        expect(page.locator('.sdk-history')).to_be_visible()
        expect(page.locator('#sdk-composer')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        assert not opens, opens
        after = page.evaluate('S.sessions.map(row => [row.uid, row.updated])')
        assert after == before, {'before':before, 'after':after, 'opens':opens}
        assert page.locator('#side .item').evaluate_all('(rows) => rows.map(row => row.dataset.uid)') == order
        draft = f'draft for input click {label} {source} {width}'
        with page.expect_response(lambda response: response.request.method == 'POST'
                                  and response.url.endswith('/api/session/conversation')):
            page.locator('#sdk-input').fill(draft)
        assert not opens, opens

        def connected():
            try:
                page.wait_for_function('SdkView.connected')
            except Exception:
                print('Connection state:', errors, page.evaluate('({uid:S.sel, kind:SessionContentView.kind, readOnly:SdkView.readOnly, connected:SdkView.connected, live:[...S.liveSdk], status:SdkView.notice?.title, failed:SdkView.openFailed, busy:[...ConsoleUI.busy]})'), flush=True)
                page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-connection-failed.png'))
                raise

        def start(repeat=True):
            editor = page.locator('#sdk-input')
            # A real pointer/touch click starts, even when the editor already
            # has programmatic focus. Repeated clicks must not duplicate opens.
            (editor.tap if width < 720 else editor.click)()
            if repeat: editor.click()

        def stop():
            button = page.locator('#a-session-toggle')
            if not button.is_visible(): page.locator('#a-more').click()
            button.click()
            page.wait_for_function('SdkView.readOnly === true && !ConsoleUI.busy.has(S.sel)')
            expect(button).to_have_attribute('aria-label', '启动会话')

        expect(page.locator('#sdk-composer .sdk-status')).to_contain_text('未运行')
        expect(page.locator('.footer-session-toggle')).to_have_count(0)
        if label == 'node' and width == 1280 and source == 'codex':
            endpoint = '**/api/codex-sdk/open'
            page.route(endpoint, lambda route: route.fulfill(status=503, json={'error':'Synthetic startup failure'}), times=1)
            start(repeat=False)
            expect(page.locator('#sdk-composer .sdk-status')).to_contain_text('无法打开')
            expect(page.locator('#sdk-send')).to_be_disabled()
            page.wait_for_function('!ConsoleUI.busy.has(S.sel)')
            page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-failed.png'))
            opens.clear()
        start()

        expect(page.locator('#sdk-input')).to_be_visible()
        connected()
        expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label', '停止会话')
        assert len(opens) == 1, opens
        expect(page.locator('#sdk-input')).to_be_focused()
        expect(page.locator('#sdk-input')).to_have_value(draft)
        if source in ('claude', 'grok', 'agy'):
            expect(page.locator('#a-term')).to_have_attribute('aria-disabled', 'true')
            expect(page.locator('#a-term')).to_have_attribute('aria-description', '此会话正在 SDK 模式运行，请先停止会话，再启动 CLI。')
            page.locator('#a-term').click(force=True)
            expect(page.locator('#a-sdk')).to_have_attribute('aria-pressed', 'true')
            for force in (False, True):
                response = page.request.post(base+'/api/term/takeover', data={'uid':uid, 'force':force})
                assert response.status == 409, response.text()
                assert 'sdk_session_busy' in response.text(), response.text()
            expect(page.locator('#sdk-input')).to_be_visible()
        page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-started.png'))
        # Reload may render history before discovery reports the running SDK.
        # Deliver that observation later, without a second session click/start.
        with page.expect_response(lambda response: response.request.method == 'POST'
                                  and response.url.endswith('/api/session/conversation')):
            page.locator('#sdk-input').fill('draft survives delayed SDK discovery')
        live_route = re.compile(r'/api/live(?:\?|$)')
        page.route(live_route, lambda route: route.fulfill(json={
            'uids': [], 'sdk_uids': [], 'tmux_uids': [], 'working_uids': []}))
        page.reload(wait_until='networkidle')
        expect(page.locator('.sdk-history')).to_be_visible()
        expect(page.locator('#sdk-composer')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        count = len(opens)
        page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-discovery-pending.png'))
        page.unroute(live_route)
        page.evaluate('refreshLive(true)')
        expect(page.locator('#sdk-input')).to_be_visible()
        connected()
        expect(page.locator('#sdk-input')).to_have_value('draft survives delayed SDK discovery')
        assert len(opens) == count + 1, opens
        page.evaluate('refreshLive(true)')
        assert len(opens) == count + 1, opens
        page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-reconnected.png'))
        # A second page attaches to the live SDK; stopping must also close its stream.
        second = context.new_page()
        second.on('pageerror', lambda error: errors.append(str(error)))
        second.goto(page.url, wait_until='networkidle')
        second.locator('#a-sdk').click()
        try:
            second.wait_for_function('SdkView.connected')
        except Exception:
            print('Second page:', second.url, errors, second.evaluate('({uid:S.sel, agent:S.agent, kind:SessionContentView.kind, readOnly:SdkView.readOnly, connected:SdkView.connected, live:[...S.liveSdk], status:SdkView.notice?.textContent})'), flush=True)
            second.screenshot(path=str(screenshots/f'{label}-{source}-{width}-second-failed.png'))
            raise
        running_snapshot = page.request.get(base+'/api/live?force=1').json()
        stop()
        expect(page.locator('#sdk-composer')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        expect(second.locator('#sdk-composer')).to_be_visible()
        expect(second.locator('#sdk-send')).to_be_disabled()
        expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label', '启动会话')
        assert uid not in page.request.get(base+'/api/live?force=1').json().get('sdk_uids', [])
        # A late pre-stop observation must not reconnect a stopped view.
        stopped_opens = len(opens)
        page.route(live_route, lambda route: route.fulfill(json=running_snapshot))
        page.evaluate('refreshLive(true)')
        expect(page.locator('#sdk-composer')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        assert len(opens) == stopped_opens, opens
        page.unroute(live_route)
        page.evaluate('refreshLive(true)')
        count = len(opens)
        page.reload(wait_until='networkidle')
        expect(page.locator('#sdk-composer')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        assert len(opens) == count, opens
        page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-stopped.png'))
        start()
        connected()
        expect(page.locator('#sdk-input')).to_be_visible()
        if label == 'node' and width == 1280:
            text = {'codex':'ask stop-test', 'claude':'hold', 'grok':'wait-for-cancel',
                    'agy':'hold:stop-test', 'opencode':'slow stop-test'}[source]
            page.locator('#sdk-input').fill(text)
            page.locator('#sdk-send').click()
            expect(page.locator('#sdk-stop')).to_be_visible()
            page.locator('#sdk-input').fill('unsent draft after stop')
        stop()
        expect(page.locator('#sdk-composer')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        assert uid not in page.request.get(base+'/api/live?force=1').json().get('sdk_uids', [])
        if label == 'node' and width == 1280:
            start()
            connected()
            expect(page.locator('#sdk-input')).to_have_value('unsent draft after stop')
            stop()
            expect(page.locator('#sdk-composer')).to_be_visible()
            expect(page.locator('#sdk-send')).to_be_disabled()
        assert not errors, errors
        context.close()


def child_check(browser, root, binary, screenshots):
    from urllib.parse import urlencode
    from playwright.sync_api import expect
    from codex_sdk_subagent_browser import add_children, SubagentServer, CHILD, THREAD
    from popups import on_popup
    class ChildDaemon(SubagentServer):
        async def _handle(self, writer, message):
            if message.get('method') in ('thread/loaded/list', 'turn/interrupt'):
                await self.send(writer, {'id':message['id'], 'result':{'data':[]} if message['method']=='thread/loaded/list' else {}})
                return
            await super()._handle(writer, message)
    corpus = build(root)
    add_children(corpus)
    with ChildDaemon(root/'app.sock', corpus) as fake, server(corpus, binary, root/'app.sock') as base:
        page = browser.new_page(viewport={'width':1280,'height':900})
        on_popup(page, lambda dialog: dialog.accept())
        page.goto(base+'/?'+urlencode({'sid':'codex:'+THREAD+'/agent:'+CHILD}), wait_until='networkidle')
        expect(page.locator('.sdk-history')).to_contain_text('Child history sentinel '+CHILD)
        expect(page.locator('#sdk-composer')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        page.locator('#a-chat').click()
        expect(page.locator('#composer')).to_be_visible()
        expect(page.locator('#cinput')).to_be_disabled()
        expect(page.locator('#cinput')).to_have_value('')
        expect(page.locator('#csend')).to_be_disabled()
        page.screenshot(path=str(screenshots/'codex-child-conversation.png'))
        page.locator('#a-sdk').click()
        page.locator('#sdk-input').click()
        page.wait_for_function('SdkView.connected')
        expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label','停止会话')
        page.screenshot(path=str(screenshots/'codex-child-started.png'))
        page.locator('#a-session-toggle').click()
        expect(page.locator('#sdk-composer')).to_be_visible()
        expect(page.locator('#sdk-send')).to_be_disabled()
        assert [m['params']['threadId'] for m in fake.methods('thread/resume')] == [CHILD]
        assert [m['params']['threadId'] for m in fake.methods('turn/interrupt')] == [CHILD]
        assert [m['params']['threadId'] for m in fake.methods('thread/unsubscribe')] == [CHILD]
        page.close()
    print('PASS child start/stop leaves parent and sibling untouched', flush=True)


def main():
    import argparse
    import tempfile
    from pathlib import Path
    from playwright.sync_api import sync_playwright
    from history_fixtures import BINARY
    from hub_fixtures import Hub
    from node_auth_fixtures import TOKEN
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path(tempfile.gettempdir())/'sessiondock-sdk-start-stop')
    parser.add_argument('--sources', default='codex,claude,grok,agy,opencode')
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-start-stop-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(tmp)/source; root.mkdir()
                with provider(source, root, args.binary) as (base, peer, extra):
                    exercise(browser, base, source, args.screenshots, 'node')
                    hub_root = root/'hub'; hub_root.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                        [SimpleNamespace(name='SdkNode', token=TOKEN,
                         port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try: exercise(browser, f'http://127.0.0.1:{hub.port}', source, args.screenshots, 'hub')
                    finally: hub.stop()
            if 'codex' in args.sources.split(','):
                root = Path(tmp)/'child'; root.mkdir()
                child_check(browser, root, args.binary, args.screenshots)
        finally: browser.close()
    print('PASS explicit SDK start/stop: five sources, node/Hub, cold/reload/view switch, two tabs, stop/restart')


if __name__ == '__main__':
    main()
