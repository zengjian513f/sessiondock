#!/usr/bin/env python3
"""Console capability priorities, repeated clicks and claim uncertainty.

The built frontend uses a private Hub, actual composed UI/services and held claim HTTP.
No real CLI or user history is opened.
"""
import os
import re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect
from popups import on_popup  # noqa: E402

def main():
    """Exercise the composed page and real claim timeout over private HTTP."""
    from browser_runtime import scoped_frontend
    if not scoped_frontend():
        # Drives the Vue runtime directly; the legacy console has its own suites.
        print('SKIP hub_console_availability_browser: requires the Vue build', flush=True)
        return
    from contextlib import ExitStack, suppress
    import tempfile
    from history_parity import BINARY
    from hub_http_suite import FakeNode, Hub, scoped

    with tempfile.TemporaryDirectory(prefix='sessiondock-console-availability-') as temporary, ExitStack() as stack:
        first = FakeNode('a' * 32, 'Lyra fixture')
        stack.callback(first.stop)
        second = FakeNode('b' * 32, 'Other fixture')
        stack.callback(second.stop)
        second_row = second.state()['row']
        second_row.update(uid='codex:same-file-hash', source='codex')
        second.set(row=second_row)
        # FakeNode's default name-based token contains spaces in these display
        # names. Use explicit private hexadecimal credentials for registration.
        for node in (first, second):
            node.token = node.nid * 2
            node.set(token=node.token)
        hub = Hub(BINARY.resolve().with_name('sessiondock-hub'), Path(temporary), [first, second])
        hub.start()
        stack.callback(hub.stop)
        uid = scoped(first.nid, first.state()['row']['uid'])
        uid2 = scoped(second.nid, second_row['uid'])
        ready = {'enabled': True, 'sources': {'claude': True}, 'resume_sources': {'claude': True}}
        capabilities = {first.nid: ready, second.nid: ready}
        terminal_rows = []
        claim_routes = []
        with sync_playwright() as p, ExitStack() as browser_stack:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = p.chromium.launch(**launch)
            browser_stack.callback(browser.close)
            page = browser.new_page(viewport={'width': 1400, 'height': 900})
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            def terminal_list(route):
                response = route.fetch()
                data = response.json()
                # API fault inputs, decoded by the actual terminal/list service.
                data.update(capabilities=capabilities, sessions=terminal_rows,
                            resume_sources={'claude': True}, enabled=True)
                for field in ('list_delta', 'list_unchanged', 'list_version'):
                    data.pop(field, None)
                route.fulfill(response=response, json=data)
            page.route('**/api/term/list', terminal_list)
            page.route('**/api/term/claim', lambda route: claim_routes.append(route))
            page.goto(f'http://127.0.0.1:{hub.port}', wait_until='networkidle')
            page.wait_for_function('window.SessionDockRuntime?.terminal.state.listLoaded')
            page.locator(f'#side .item[data-uid="{uid}"] .t').click()
            page.wait_for_function('uid => window.SessionDockRuntime.core.state.selection.sel === uid', arg=uid)
            button = page.locator('#a-term')
            def apply(capability, *, ended=False, top_level=False):
                capabilities[first.nid] = capability
                page.evaluate('''({nid,uid,capability,ended,top_level}) => {
                    const runtime = window.SessionDockRuntime;
                    runtime.core.nodes.applyNodeState({capabilities:{...runtime.core.state.nodes.capabilities,[nid]:capability}});
                    runtime.terminal.state.resume_sources = {claude:top_level};
                    runtime.terminal.state.ended.clear();
                    if(ended)runtime.terminal.state.ended.set(uid,{reason:'已退出且不可恢复'});
                }''', {'nid':first.nid, 'uid':uid, 'capability':capability, 'ended':ended, 'top_level':top_level})
            for ended in [False, True]:
                apply(ready, ended=ended)
                expect(button).to_have_attribute('data-unavailable', 'false')
            for capability in [
                {'enabled': True, 'sources': {'claude': True}, 'resume_sources': {'claude': False}},
                {'enabled': True, 'sources': {'claude': True}},
                {'enabled': False, 'unavailable_reason': 'fixture offline', 'sources': {'claude': True}, 'resume_sources': {'claude': True}},
                {'enabled': True, 'sources': {'claude': False}, 'resume_sources': {'claude': True}},
            ]:
                apply(capability, top_level=True)
                expect(button).to_have_attribute('data-unavailable', 'true')
                expect(button).to_have_attribute('aria-label', re.compile('控制台不可用'))
            apply({'enabled': True, 'sources': {'claude': True}, 'resume_sources': {'claude': False}}, ended=True, top_level=True)
            expect(button).to_have_attribute('data-unavailable', 'true')
            expect(button).to_have_attribute('aria-label', re.compile('已退出且不可恢复'))
            apply(ready)
            capabilities[second.nid] = {'enabled':True,'sources':{'claude':True},'resume_sources':{'claude':False}}
            page.locator(f'#side .item[data-uid="{uid2}"] .t').click()
            expect(button).to_have_attribute('data-unavailable', 'true')
            page.locator(f'#side .item[data-uid="{uid}"] .t').click()
            expect(button).to_have_attribute('data-unavailable', 'false')
            page.evaluate('window.SessionDockRuntime.core.state.selection.agent="child-agent"')
            expect(button).to_have_attribute('data-unavailable', 'true')
            expect(button).to_have_attribute('aria-label', re.compile('子代理没有独立控制台'))
            page.evaluate('window.SessionDockRuntime.core.state.selection.agent=null')
            apply(ready)
            dialogs = []
            on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.dismiss()))
            capabilities[second.nid] = {'enabled':True,'sources':{'codex':False},'resume_sources':{'codex':False}}
            page.evaluate('capabilities => window.SessionDockRuntime.core.nodes.applyNodeState({capabilities})', capabilities)
            page.locator(f'#side .item[data-uid="{uid2}"] .t').click()
            expect(button).to_have_attribute('data-unavailable', 'true')
            expect(button).to_have_attribute('aria-label', re.compile('Other fixture：未配置可用的 Codex 启动命令'))
            button.click()
            expect(page.locator('dialog.app-popup[open]')).to_have_count(0)
            assert len(dialogs) == 1 and 'CLI 安装和启动器配置' in dialogs[0], dialogs
            assert len(claim_routes) == 0
            dialogs.clear()
            page.locator(f'#side .item[data-uid="{uid}"] .t').click()
            # Bind a real observed instance through the API, then hold claims.
            terminal_rows.append({'name':first.nid+'~fixture-pane','uid':uid,'current_uid':uid,
                                  'instance_id':'fixture-instance','source':'claude','backend':'ptyhost'})
            page.evaluate('() => window.SessionDockRuntime.terminal.loadTermList()')
            page.wait_for_function('uid => window.SessionDockRuntime.terminal.state.list.some(row => row.uid === uid)', arg=uid)
            with page.expect_request('**/api/term/claim'):
                button.click()
            page.wait_for_function('uid => window.SessionDockRuntime.core.state.console.busy.has(uid)', arg=uid)
            assert len(claim_routes) == 1
            claim = claim_routes[0].request.post_data_json
            assert claim['uid'] == uid and claim['instance_id'] == 'fixture-instance', claim
            assert claim['name'] == first.nid + '~fixture-pane' and not claim.get('force'), claim
            button.click()
            button.click()
            assert len(claim_routes) == 1
            assert not dialogs, dialogs
            expect(page.locator('#console-toast')).to_contain_text('正在打开控制台')
            # Actual post timeout aborts the held HTTP request; no injected rejection.
            page.wait_for_function('uid => !window.SessionDockRuntime.core.state.console.busy.has(uid)', arg=uid)
            expect(button).to_have_attribute('data-unavailable', 'false')
            assert page.evaluate('() => [...window.SessionDockRuntime.terminal.state.views.values()].every(view => !view.ws)')
            expect(page.locator('#console-toast')).to_contain_text('服务端可能已取得控制权')
            assert not dialogs, dialogs
            with page.expect_request('**/api/term/claim'):
                button.click()
            page.wait_for_function('uid => window.SessionDockRuntime.core.state.console.busy.has(uid)', arg=uid)
            assert len(claim_routes) == 2
            retry = claim_routes[1].request.post_data_json
            assert retry['uid'] == uid and retry['instance_id'] == claim['instance_id'], retry
            assert retry['page'] == claim['page'] and not retry.get('force'), retry
            page.wait_for_function('uid => !window.SessionDockRuntime.core.state.console.busy.has(uid)', arg=uid)
            assert not dialogs, dialogs
            assert not errors, errors
            for route in claim_routes:
                with suppress(Exception):
                    route.abort()
            print('PASS scoped hub console: capability priorities, repeated clicks, actual HTTP claim timeout, uncertainty and explicit retry')


if __name__ == '__main__':
    main()
