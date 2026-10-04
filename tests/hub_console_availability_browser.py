#!/usr/bin/env python3
"""Console capability priorities, repeated clicks and claim uncertainty.

Legacy uses its existing isolated extracted-function page. The selected ESM
artifact uses a private Hub, actual composed UI/services and held claim HTTP.
No real CLI or user history is opened.
"""
import os
from browser_runtime import scoped_frontend
import re
from pathlib import Path
from playwright.sync_api import sync_playwright, expect
from popups import on_popup  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
if not scoped_frontend():
    SOURCE = (ROOT / 'legacy-web/nodes.js').read_text()
    FUNCTIONS = '\n'.join([
        SOURCE[SOURCE.index('function nodeOf('):SOURCE.index('function nodeSelected(')],
        SOURCE[SOURCE.index('function consoleUnavailableReason('):SOURCE.index('function showConsoleToast(')],
        SOURCE[SOURCE.index('function paintConsoleAvailability('):SOURCE.index('function consoleButtonMarkup(')],
        SOURCE[SOURCE.index('function bindConsoleButton('):SOURCE.index('function applyNodeState(')],
    ])
    TERM_SOURCE = (ROOT / 'legacy-web/term.js').read_text()
    FUNCTIONS += '\n' + TERM_SOURCE[TERM_SOURCE.index('async function claimTermOwnership('):TERM_SOURCE.index('// 抢占方的描述')]
    FUNCTIONS += '\n' + TERM_SOURCE[TERM_SOURCE.index('function describeTermTaker('):TERM_SOURCE.index('function handleTermRevoked(')]


def main():
    if scoped_frontend():
        return scoped_main()
    with sync_playwright() as p:
        launch = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = p.chromium.launch(**launch)
        try:
            page = browser.new_page()
            page.set_content('<button id="a-term">接管会话</button>')
            page.add_script_tag(path=str(ROOT / 'legacy-web/popup.js'))
            page.add_script_tag(content='''
                const HUB_MODE = true, nid = 'a'.repeat(32), uid = `claude:${nid}~synthetic`;
                const nid2 = 'b'.repeat(32), uid2 = `codex:${nid2}~synthetic`;
                const SessionDockCapabilities = {config:{backend:'rust'}, allows:()=>true};
                const T = {enabled:true,listLoaded:true,listError:'',resume_sources:{},ended:new Map(),pending:[]};
                const Nodes = {list:[{id:nid,name:'Lyra fixture'},{id:nid2,name:'Other fixture'}],errors:new Map(),capabilities:{}};
                const ConsoleUI = {errors:new Map(),busy:new Set()}, SOURCES = {claude:{name:'Claude'},codex:{name:'Codex'}};
                const TERM_PAGE_ID='fixture-page', TERM_CLAIM_TIMEOUT_MS=5000;
                let rejectClaim, claimCount=0, opened=false, toast='';
                async function post(){claimCount++; return new Promise((_resolve,reject)=>{rejectClaim=reject})}
                async function takeover(){opened=!!await claimTermOwnership('fixture-pane',uid)}
                function Terminal(){} function FitAddon(){}
                function renderTakeoverBtn(){paintConsoleAvailability(document.querySelector('#a-term'),uid)}
                function sessionTermMeta(uid){return {source:uid.split(':')[0]}}
                function linkedTermSession(){return null} function showConsoleToast(reason){toast=reason}
            ''' + FUNCTIONS)
            button = page.locator('#a-term')
            def apply(capability, *, ended=False, top_level=False):
                page.evaluate('''({capability,ended,top_level})=>{
                    Nodes.capabilities[nid]=capability;
                    T.resume_sources={claude:top_level};
                    T.ended=ended?new Map([[uid,{reason:'已退出且不可恢复'}]]):new Map();
                    paintConsoleAvailability(document.querySelector('#a-term'),uid);
                }''', {'capability': capability, 'ended': ended, 'top_level': top_level})
            ready = {'enabled': True, 'sources': {'claude': True}, 'resume_sources': {'claude': True}}
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
            page.evaluate("Nodes.capabilities[nid2]={enabled:true,sources:{claude:true},resume_sources:{claude:false}}; paintConsoleAvailability(document.querySelector('#a-term'),uid2)")
            expect(button).to_have_attribute('data-unavailable', 'true')
            page.evaluate("paintConsoleAvailability(document.querySelector('#a-term'),uid)")
            expect(button).to_have_attribute('data-unavailable', 'false')
            page.evaluate("paintConsoleAvailability(document.querySelector('#a-term'),uid,'child-agent')")
            expect(button).to_have_attribute('data-unavailable', 'true')
            expect(button).to_have_attribute('aria-label', re.compile('子代理没有独立控制台'))
            apply(ready)
            dialogs = []
            on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.dismiss()))
            # BUG-20261003-103542-d86b9f: the node reads Desktop history but
            # has no Codex launcher profile. Explain that first, before the
            # generic unlinked-instance message; never send a takeover.
            page.evaluate("Nodes.capabilities[nid2]={enabled:true,sources:{codex:false},resume_sources:{codex:false}}; bindConsoleButton(document.querySelector('#a-term'),uid2)")
            expect(button).to_have_attribute('data-unavailable', 'true')
            expect(button).to_have_attribute('aria-label', re.compile('Other fixture：未配置可用的 Codex 启动命令'))
            button.click()
            expect(page.locator('dialog.app-popup[open]')).to_have_count(0)
            assert len(dialogs) == 1 and 'CLI 安装和启动器配置' in dialogs[0], dialogs
            assert page.evaluate('claimCount') == 0
            dialogs.clear()
            page.evaluate("bindConsoleButton(document.querySelector('#a-term'),uid)")
            button.click()
            page.wait_for_function('ConsoleUI.busy.has(uid) && claimCount===1')
            button.click()
            button.click()
            assert page.evaluate('claimCount') == 1
            assert not dialogs, dialogs
            assert '正在打开控制台' in page.evaluate('toast')
            page.evaluate("const error=new Error('fixture deadline'); error.name='TimeoutError'; rejectClaim(error)")
            page.wait_for_function('!ConsoleUI.busy.has(uid)')
            expect(button).to_have_attribute('data-unavailable', 'false')
            assert not page.evaluate('opened')
            assert '服务端可能已取得控制权' in page.evaluate('toast')
            assert not dialogs, dialogs
            # The user can start a fresh non-force claim after the failed attempt.
            button.click()
            page.wait_for_function('claimCount===2')
            page.evaluate("const error=new Error('fixture deadline'); error.name='TimeoutError'; rejectClaim(error)")
            page.wait_for_function('!ConsoleUI.busy.has(uid)')
            print('PASS delayed claim: repeated clicks never block with a dialog or duplicate requests; timeout clears busy, preserves uncertainty and allows explicit retry')
            print('PASS hub console button: per-node resume enables unlinked/exited sessions; global flags cannot enable unresumable, unknown, offline, missing-CLI or child views')
        finally:
            browser.close()


def scoped_main():
    """Exercise the composed page and real claim timeout over private HTTP."""
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
