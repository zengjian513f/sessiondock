"""Explicit SDK start/stop: five providers, real isolated bridges and Chromium."""
from contextlib import contextmanager
import json
from types import SimpleNamespace
from unittest.mock import patch

from agy_sdk_fixtures import AgyFixture
from claude_sdk_fixtures import ClaudeFixture
from codex_sdk_browser import build, server
from codex_sdk_fixtures import FakeAppServer
from codex_sdk_actions_browser import environment
from grok_sdk_fixtures import GrokFixture
from history_fixtures import REPO, Corpus, isolated_server
from node_auth_fixtures import node_env
from hub_fixtures import free_port
from opencode_sdk_fixtures import FakeOpenCode
from send_browser import initialize


@contextmanager
def provider(source, root, binary, *, cli=False):
    extra = node_env(root, free_port(), '127.0.0.0/8')
    if cli:
        (root/'menu-host').mkdir()
        from terminal_browser import SHELL_SCRIPT
        launcher = root/'menu-cli-launcher.json'
        launcher.write_text(json.dumps({'schema':2, 'host_binary':str(REPO/'target/debug/ptyhost'),
            'host_dir':str(root/'menu-host'), 'profiles':[{'id':'menu-'+source, 'source':source,
            'executable':'/bin/sh', 'args':['-c', SHELL_SCRIPT]}]}))
        initialize('--initialize-lifecycle', root/'menu-ledger', binary.resolve())
        extra.update(SESSIONDOCK_LAUNCHER_CONFIG=str(launcher), SESSIONDOCK_PTYHOST_DIR=str(root/'menu-host'),
            SESSIONDOCK_LIFECYCLE_DIR=str(root/'menu-ledger'), SESSIONDOCK_NATIVE_SDKS='off',
            SESSIONDOCK_CODEX_APP_SERVER='off')
    if source == 'claude':
        peer = ClaudeFixture(root)
        with peer.server(binary, extra) as base:
            yield base, peer, extra
    elif source == 'codex':
        corpus = build(root)
        if cli:
            work = root/'work'; work.mkdir()
            path = corpus.paths['sdk']
            path.write_text(path.read_text().replace('/synthetic/sdk', str(work)))
        with StartDaemon(root/'app.sock', corpus.paths['sdk'], '01a1sdk0-0000-7000-8000-000000000001') as peer:
            with server(corpus, binary, root/'app.sock', {**environment(root, binary), **extra}) as base:
                yield base, peer, extra
    elif source == 'grok':
        for name in ('claude', 'codex', 'grok', 'state'):
            (root/name).mkdir()
        peer = GrokFixture(root)
        initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
        with server(SimpleNamespace(root=root), binary, 'off', {
            **peer.env, 'SESSIONDOCK_NATIVE_SDKS': 'auto',
            'SESSIONDOCK_LIFECYCLE_DIR': str(root/'ledger'),
            'SESSIONDOCK_PTYHOST_DIR': str(root/'unused-host'),
            'SESSIONDOCK_STATE_DIR': str(root/'state'), **extra,
        }) as base:
            yield base, peer, extra
    elif source == 'agy':
        peer = AgyFixture(root, REPO/'target/debug/ptyhost')
        with patch.dict('os.environ', peer.env):
            initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
            with isolated_server(Corpus(root), binary, host_dir=root/'host', lifecycle_dir=root/'ledger',
                    state_dir=root/'state', launcher_config=peer.launcher,
                    extra_env={**peer.extra_env, **extra}) as (base, _):
                yield base, peer, extra
    else:
        for name in ('claude', 'codex', 'grok', 'host', 'state', 'proc'):
            (root/name).mkdir()
        (root/'proc/stat').write_text('btime 1700000000\n')
        with FakeOpenCode(root) as peer:
            launcher = root/'launcher.json'
            launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO/'target/debug/ptyhost'),
                'host_dir': str(root/'host'), 'profiles': [peer.profile()]}))
            initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
            with isolated_server(Corpus(root), binary, host_dir=root/'host', state_dir=root/'state',
                    lifecycle_dir=root/'ledger', launcher_config=launcher, extra_env={
                        'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_OPENCODE_DB': str(peer.db),
                        'SESSIONDOCK_OPENCODE_ROOT': str(root/'mirror'),
                        'SESSIONDOCK_PROC_ROOT': str(root/'proc'), **extra}) as (base, _):
                yield base, peer, extra


class StartDaemon(FakeAppServer):
    async def _handle(self, writer, message):
        if message.get('method') == 'thread/loaded/list':
            await self.send(writer, {'id': message['id'], 'result': {'data': []}})
            return
        await super()._handle(writer, message)


def exercise(browser, base, source, screenshots, label):
    from playwright.sync_api import expect
    from popups import on_popup
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        print(f'{label} {source} {width} {theme}', flush=True)
        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme,
                                      service_workers='block')
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
        before = page.evaluate('S.sessions.map(row => [row.uid, row.updated])')
        order = page.locator('#side .item').evaluate_all('(rows) => rows.map(row => row.dataset.uid)')
        row.click()
        expect(page.locator('.sdk-history')).to_be_visible()
        expect(page.locator('#sdk-composer')).to_be_hidden()
        expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label', '启动会话')
        assert not opens, opens
        assert not page.evaluate('uid => S.liveSdk.has(uid)', uid)
        page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-cold.png'))
        # SDK view switches and reloads also remain read-only.
        page.locator('#a-chat').click()
        page.locator('#a-sdk').click()
        expect(page.locator('#sdk-composer')).to_be_hidden()
        page.reload(wait_until='networkidle')
        expect(page.locator('.sdk-history')).to_be_visible()
        expect(page.locator('#sdk-composer')).to_be_hidden()
        assert not opens, opens
        after = page.evaluate('S.sessions.map(row => [row.uid, row.updated])')
        assert after == before, {'before':before, 'after':after, 'opens':opens}
        assert page.locator('#side .item').evaluate_all('(rows) => rows.map(row => row.dataset.uid)') == order

        def power():
            button = page.locator('#a-session-toggle')
            if not button.is_visible(): page.locator('#a-more').click()
            button.click()

        # Choose SDK explicitly, even when the previous view was ordinary chat.
        page.locator('#a-chat').click()
        if page.locator('#detail .mobile-back').is_visible(): page.locator('#detail .mobile-back').click()
        row.click(button='right')
        page.locator('#item-menu [data-act="stop"]').click()
        expect(page.locator('#session-start-menu')).to_be_visible()
        page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-start-menu.png'))
        page.locator('#session-start-menu [data-start-mode="sdk"]').click()
        expect(page.locator('#session-start-menu')).to_be_hidden()
        expect(page.locator('#sdk-input')).to_be_visible()
        page.wait_for_function('SdkView.connected')
        expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label', '停止会话')
        assert len(opens) == 1, opens
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
        # A second page attaches to the live SDK; stopping must also close its stream.
        second = context.new_page()
        second.goto(page.url, wait_until='networkidle')
        second.wait_for_function('SdkView.connected')
        power()
        expect(page.locator('#sdk-composer')).to_be_hidden()
        expect(second.locator('#sdk-composer')).to_be_hidden()
        expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label', '启动会话')
        assert uid not in page.request.get(base+'/api/live?force=1').json().get('sdk_uids', [])
        count = len(opens)
        page.reload(wait_until='networkidle')
        expect(page.locator('#sdk-composer')).to_be_hidden()
        assert len(opens) == count, opens
        page.screenshot(path=str(screenshots/f'{label}-{source}-{width}-stopped.png'))
        power()
        page.wait_for_function('SdkView.connected')
        expect(page.locator('#sdk-input')).to_be_visible()
        if label == 'node' and width == 1280:
            text = {'codex':'ask stop-test', 'claude':'hold', 'grok':'wait-for-cancel',
                    'agy':'hold:stop-test', 'opencode':'slow stop-test'}[source]
            page.locator('#sdk-input').fill(text)
            page.locator('#sdk-send').click()
            expect(page.locator('#sdk-stop')).to_be_visible()
            page.locator('#sdk-input').fill('unsent draft after stop')
        power()
        expect(page.locator('#sdk-composer')).to_be_hidden()
        assert uid not in page.request.get(base+'/api/live?force=1').json().get('sdk_uids', [])
        if label == 'node' and width == 1280:
            power()
            page.wait_for_function('SdkView.connected')
            expect(page.locator('#sdk-input')).to_have_value('unsent draft after stop')
            power()
            expect(page.locator('#sdk-composer')).to_be_hidden()
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
        expect(page.locator('#sdk-composer')).to_be_hidden()
        page.locator('#a-session-toggle').click()
        page.wait_for_function('SdkView.connected')
        expect(page.locator('#a-session-toggle')).to_have_attribute('aria-label','停止会话')
        page.screenshot(path=str(screenshots/'codex-child-started.png'))
        page.locator('#a-session-toggle').click()
        expect(page.locator('#sdk-composer')).to_be_hidden()
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
