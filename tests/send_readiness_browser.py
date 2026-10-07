#!/usr/bin/env python3
"""Grok readiness, post-paste refusal, browser composer focus, and keyboard inset.
Only a free fake CLI and loopback server in private temporary directories.
"""
import json
import os
from pathlib import Path
import re
import sys
import tempfile
from urllib.parse import urlsplit

from browser_runtime import wait_for_async
from playwright.sync_api import expect, sync_playwright
from history_fixtures import REPO, BINARY, Corpus, isolated_server
from private_hosts import private_hosts
from send_browser import initialize, xterm_includes
from popups import on_popup  # noqa: E402


def composer_layout(page):
    return page.evaluate("""() => ['#detail', '#composer', '#cinput'].map(selector => {
        const node=document.querySelector(selector), rect=node.getBoundingClientRect();
        return [rect.top, rect.bottom, node.scrollTop];
    })""")


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-readiness-') as temporary, private_hosts(Path(temporary)):
        root = Path(temporary)
        for name in ['host', 'work', 'ledger', 'delivery', 'state', 'home', 'claude', 'codex', 'grok']:
            (root / name).mkdir(mode=0o700)
        screen = root / 'screen'
        screen.write_text('login')
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'grok-cli-v1', 'source': 'grok',
                'executable': str(Path(sys.executable).resolve()),
                'args': [str(REPO / 'tests/fake_grok_composer.py')],
                'new_args': ['--session-id', '{session_id}'], 'resume_args': ['--resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                    'TERM': 'xterm-256color', 'LANG': 'C.UTF-8', 'SESSIONDOCK_TEST_SCREEN': str(screen)}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger')
        with isolated_server(Corpus(root), BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=launcher, state_dir=root / 'state',
                file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(service_workers='block')
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page()
            errors, dialogs = [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
            receipt = None
            try:
                page.goto(base, wait_until='networkidle')
                page.locator('#new-session').click()
                page.locator('input[name="new-source"][value="grok"]').check()
                page.locator('#new-cwd').fill(str(root / 'work'))
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/create') as created:
                    page.locator('#new-session-go').click()
                assert created.value.status == 200, created.value.text()
                receipt = created.value.json()
                uid = 'tmux:' + receipt['name']
                build = context.request.get(base + '/api/meta').json()['build']

                def post(route, **fields):
                    return context.request.post(base + '/api/session/conversation/' + route, data={'uid': uid, '_build': build, **fields})

                def wait_code(code):
                    wait_for_async(page, r"""async ({uid, code, build}) => {
                        const r=await fetch('api/session/conversation/check', {method:'POST',
                            headers:{'Content-Type':'application/json'},body:JSON.stringify({uid,_build:build})});
                        const d=await r.json(); return code ? d.code===code : d.ok===true;
                    }""", arg={'uid': uid, 'code': code, 'build': build}, timeout=10000)

                wait_code('cli_not_ready')
                status = post('check').json()
                assert status['input']['state'] == 'unknown' and status['input']['code'] == 'cli_not_ready', status
                assert isinstance(status['draft_revision'], int), status
                page.locator('#cinput').fill('keep this message')
                page.evaluate('async () => await composerDraftWrites')
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'unknown'")
                expect(page.locator('#csend')).to_be_disabled()
                expect(page.locator('#composer-input-status')).to_contain_text('PTY')
                expect(page.locator('#composer-input-status')).to_be_visible()
                expect(page.locator('#dlive')).not_to_have_class(re.compile(r'\binput-attention\b'))
                page.evaluate(r"""() => {
                    const item = document.querySelector('#side .item.sel');
                    S.unread.set(item.dataset.uid, {count:2});
                    paintItemStatus(item);
                }""")
                marker = page.locator('#side .item.sel > .ico > .item-status')
                expect(marker).to_have_text('2')
                expect(marker).to_have_attribute('title', re.compile('2 条新内容'))
                page.evaluate(r"""() => {
                    const item = document.querySelector('#side .item.sel');
                    S.unread.delete(item.dataset.uid);
                    paintItemStatus(item);
                }""")
                expect(marker).not_to_have_text('2')
                expect(page.locator('#composer-input-status')).not_to_have_class(re.compile(r'\binput-attention\b'))
                # No submission is needed to show or retain the reason. Enter
                # follows the disabled button and leaves the draft editable.
                trace = screen.with_suffix('.trace')
                page.locator('#cinput').press('Enter')
                page.wait_for_timeout(1800)
                assert not dialogs, dialogs
                assert not trace.exists()
                expect(page.locator('#composer-input-status')).to_be_visible()
                expect(page.locator('#cinput')).to_have_value('keep this message')
                # A proxy failure belongs to SessionDock, not the native CLI.
                page.route('**/api/session/conversation/check', lambda route:
                    route.fulfill(status=502, content_type='text/html', body='Bad Gateway'))
                expect(page.locator('#composer-input-status')).to_have_text('SessionDock 请求失败（HTTP 502），请稍后重试', timeout=10000)
                expect(page.locator('#csend')).to_be_disabled()
                expect(page.locator('#cinput')).to_have_value('keep this message')
                page.unroute('**/api/session/conversation/check')
                expect(page.locator('#composer-input-status')).to_have_text('暂未识别到终端消息编辑区，请切换到 PTY（终端）查看；输入已保留', timeout=10000)
                # A stalled CHECK must time out and let later polls recover,
                # without the user submitting or reloading the conversation.
                page.evaluate(r"""() => {
                    const original=window.fetch;
                    window.restoreInputChecks=() => {window.fetch=original;};
                    window.fetch=(url, options) => {
                        if (!String(url).endsWith('/api/session/conversation/check'))
                            return original(url, options);
                        return new Promise((resolve, reject) => {
                            options?.signal?.addEventListener('abort', () =>
                                reject(new DOMException('Aborted', 'AbortError')), {once:true});
                        });
                    };
                }""")
                expect(page.locator('#composer-input-status')).to_contain_text('检查超时', timeout=12000)
                expect(page.locator('#csend')).to_be_disabled()
                page.evaluate('restoreInputChecks()')
                expect(page.locator('#composer-input-status')).to_contain_text('PTY', timeout=10000)
                # Temporary non-ready states and a genuinely busy process are
                # normal progress, not a yellow exclamation mark.
                transitions = page.evaluate(r"""() => {
                    const draft=composerDraft(), previous=draft.cli;
                    const results=[];
                    for (const code of ['input_check_pending','cli_starting','cli_catching_up','cli_pasting']) {
                        updateComposerInputStatus(composerUid, {input:{state:code === 'input_check_pending'
                            ? 'unknown' : 'starting',code,message:'progress'}});
                        results.push([code,document.querySelector('#dlive').classList.contains('input-attention'),
                            document.querySelector('#composer-input-status').classList.contains('input-attention')]);
                    }
                    applyCliState(composerUid, {...previous,instance:{running:true,busy:true},
                        input:{state:'unknown',code:'cli_not_ready',message:'processing'}});
                    results.push(['working',document.querySelector('#dlive').classList.contains('input-attention'),
                        document.querySelector('#composer-input-status').classList.contains('input-attention')]);
                    applyCliState(composerUid, previous);
                    return results;
                }""")
                assert all(not header and not composer for _, header, composer in transitions), transitions
                ordinary_states = page.evaluate(r"""() => {
                    const draft=composerDraft(), previous=draft.cli, results=[];
                    for (const [state,code,running,expected] of [
                        ['blocked','cli_input_pending',true,''],
                        ['blocked','cli_input_returned',true,''],
                        ['unknown','cli_not_ready',true,''],
                        ['unknown','input_check_failed',true,''],
                        ['blocked','new_unknown_block',true,''],
                        ['blocked','cli_not_ready',true,''],
                        ['unknown','cli_input_pending',true,''],
                        ['blocked','cli_input_pending',false,'']]) {
                        applyCliState(composerUid, {...previous,instance:{running,busy:false},
                            input:{state,code,message:'synthetic native state'}});
                        const attention=sessionInputAttention(composerUid);
                        const shown=document.querySelector('#composer-input-status').classList.contains('input-attention');
                        results.push({state,code,running,expected,attention,shown});
                    }
                    applyCliState(composerUid, previous);
                    return results;
                }""")
                assert all(row['attention'] == row['expected'] and row['shown'] == bool(row['expected'])
                           for row in ordinary_states), ordinary_states
                assert not dialogs, dialogs
                # Exercise focus through actual CHECK polling and keyboard
                # input, not only synchronous DOM changes in one JS turn.
                screen.write_text('custom')
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'")
                expect(page.locator('#csend')).to_be_enabled()
                expect(page.locator('#composer-input-status')).to_be_hidden()
                expect(page.locator('#dlive')).not_to_have_class(re.compile(r'\binput-attention\b'))
                editor = page.locator('#cinput')
                editor.click()
                editor.press('Home')
                editor.press('Shift+ArrowRight')
                editor.press('Shift+ArrowRight')
                # Native menu arrival and departure flow through ordinary CHECK
                # polling. No presentation refresh or product-state injection.
                menus = json.loads((REPO / 'tests/fixtures/cli_menus_grok.json').read_text())
                screen.write_text(next(menu['screen'] for menu in menus if menu['name'] == 'approval_bash'))
                expect(page.locator('#composer-input-status')).to_contain_text('选择', timeout=10000)
                expect(page.locator('#composer-input-status')).to_be_visible()
                expect(page.locator('#csend')).to_be_disabled()
                expect(editor).to_be_enabled()
                expect(editor).to_be_focused()
                expect(editor).to_have_value('keep this message')
                assert editor.evaluate('el => el.value.slice(el.selectionStart, el.selectionEnd)') == 'ke'
                editor.press('Enter')
                expect(editor).to_have_value('keep this message')
                assert not trace.exists(), 'blocked composer submitted terminal input'
                screen.write_text('custom')
                expect(page.locator('#composer-input-status')).to_be_hidden(timeout=10000)
                expect(page.locator('#csend')).to_be_enabled()
                expect(editor).to_be_focused()
                expect(editor).to_have_value('keep this message')
                assert editor.evaluate('el => el.value.slice(el.selectionStart, el.selectionEnd)') == 'ke'
                editor.press('End')
                screen.write_text('login')
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'unknown'")
                expect(page.locator('#csend')).to_be_disabled()
                expect(page.locator('#composer-input-status')).to_be_visible()
                assert page.evaluate("document.activeElement?.id === 'cinput'"), 'polling stole composer focus'
                page.keyboard.type('!')
                expect(page.locator('#cinput')).to_have_value('keep this message!')
                page.locator('#cinput').fill('keep this message')
                page.evaluate("addComposerQuote('quoted words')")
                page.wait_for_function("document.activeElement?.matches('#compose-items .draft-quote textarea')")
                page.wait_for_timeout(350)
                assert page.evaluate("document.activeElement?.matches('#compose-items .draft-quote textarea')"), 'quote lost focus during draft save'
                page.keyboard.type('!')
                assert page.evaluate('composerDraft().quotes[0].text') == 'quoted words!'
                stable = page.evaluate(r"""() => {
                    const quote=document.activeElement, input=document.querySelector('#cinput');
                    const composer=document.querySelector('#composer');
                    const status=document.querySelector('#composer-input-status');
                    quote.setSelectionRange(3, 3);
                    updateComposerInputStatus(composerUid, {ok:true,
                        input:{state:'ready',code:'',message:''}});
                    const before=[composer.getBoundingClientRect().top,
                        input.getBoundingClientRect().top];
                    const quoteRetained=document.activeElement===quote && quote.isConnected
                        && quote.selectionStart===3;
                    input.focus(); input.setSelectionRange(4, 4);
                    updateComposerInputStatus(composerUid, {ok:false,
                        input:{state:'unknown',code:'test_long_notice',message:'PTY '+ '长提示'.repeat(90)}});
                    const bubble=status.getBoundingClientRect();
                    const style=getComputedStyle(status);
                    const shown=[composer.getBoundingClientRect().top,
                        input.getBoundingClientRect().top];
                    updateComposerInputStatus(composerUid, {ok:true,
                        input:{state:'ready',code:'',message:''}});
                    const hidden=[composer.getBoundingClientRect().top,
                        input.getBoundingClientRect().top];
                    return {quoteRetained, inputFocused:document.activeElement===input,
                        selection:input.selectionStart, inside:composer.contains(status),
                        bubbleHeight:bubble.height, border:style.borderTopWidth,
                        radius:style.borderTopLeftRadius, background:style.backgroundColor,
                        before, shown, hidden, bubbleBottom:bubble.bottom};
                }""")
                assert stable['quoteRetained'] and stable['inputFocused'] and stable['selection'] == 4, stable
                assert stable['inside'] and stable['bubbleHeight'] > 18, stable
                assert stable['before'] == stable['shown'] == stable['hidden'], stable
                assert stable['bubbleBottom'] < stable['shown'][0], stable
                assert (stable['border'], stable['radius'], stable['background']) == (
                    '1px', '6px', 'color(srgb 1 1 1 / 0.82)'), stable
                # Exercise the real settings controls and composer in both
                # themes, including the report's narrow keyboard-sized viewport.
                for width, height in [(1280, 720), (390, 844), (424, 259)]:
                    for theme in ['dark', 'light']:
                        page.set_viewport_size({'width': 1280, 'height': 720})
                        page.locator('#settings').click()
                        page.locator('#setting-theme').select_option(theme)
                        page.keyboard.press('Escape')
                        page.set_viewport_size({'width': width, 'height': height})
                        if width < 600:
                            page.evaluate('showMobileDetail()')
                        page.locator('#cinput').fill('keep this message')
                        page.wait_for_function("composerDraft()?.inputStatus?.state === 'unknown'")
                        expect(page.locator('#composer-input-status')).to_be_visible()
                        expect(page.locator('#composer-input-status')).to_have_css('backdrop-filter', 'blur(6px)')
                        background = page.locator('#composer-input-status').evaluate('el => getComputedStyle(el).backgroundColor')
                        assert background.endswith('/ 0.82)'), background
                        bounds = page.locator('#composer-input-status').bounding_box()
                        assert bounds and bounds['x'] >= 0 and bounds['y'] >= 0, bounds
                        assert bounds['x'] + bounds['width'] <= width, bounds
                        composer = page.locator('#composer').bounding_box()
                        assert bounds['y'] + bounds['height'] < composer['y'], (bounds, composer)
                        # The notice contributes no empty row to the composer.
                        items = page.locator('#compose-items').bounding_box()
                        assert items['y'] - composer['y'] < 12, (items, composer)
                        inset = 8 if width < 600 else 18
                        assert abs(bounds['x'] - composer['x'] - inset) < 2, (bounds, composer)
                        assert abs(bounds['width'] - composer['width'] + 2 * inset) < 2, (bounds, composer)
                        expect(page.locator('#composer-input-status')).not_to_contain_text('SessionDock')
                        capture = os.environ.get('SESSIONDOCK_TEST_SCREENSHOTS')
                        if capture:
                            Path(capture).mkdir(parents=True, exist_ok=True)
                            page.screenshot(path=str(Path(capture) / f'composer-{width}-{height}-{theme}.png'))
                        # Real CHECK transitions must leave the conversation and
                        # editor in place, including in a keyboard-sized viewport.
                        before = composer_layout(page)
                        screen.write_text('custom')
                        expect(page.locator('#composer-input-status')).to_be_hidden(timeout=10000)
                        assert composer_layout(page) == before, (before, composer_layout(page))
                        if capture:
                            page.screenshot(path=str(Path(capture) / f'composer-{width}-{height}-{theme}-ready.png'))
                        screen.write_text('login')
                        expect(page.locator('#composer-input-status')).to_be_visible(timeout=10000)
                        assert composer_layout(page) == before, (before, composer_layout(page))
                        page.locator('#cinput').press('End')
                        page.locator('#cinput').press('!')
                        expect(page.locator('#cinput')).to_have_value('keep this message!')
                        page.locator('#cinput').fill('keep this message')
                        print(f'PASS floating composer notice {width}x{height} {theme}: stable layout, no reserved row', flush=True)
                page.set_viewport_size({'width': 390, 'height': 844})
                page.evaluate('showMobileDetail()')
                if page.locator('#termpane').is_visible():
                    page.locator('#a-term').click()
                expect(page.locator('#cinput')).to_be_visible()
                mobile = page.evaluate(r"""() => {
                    const input=document.querySelector('#cinput');
                    const composer=document.querySelector('#composer');
                    input.focus(); input.setSelectionRange(2, 2);
                    updateComposerInputStatus(composerUid, {ok:true,
                        input:{state:'ready',code:'',message:''}});
                    const before=[composer.getBoundingClientRect().top,
                        input.getBoundingClientRect().top];
                    updateComposerInputStatus(composerUid, {ok:false,
                        input:{state:'unknown',code:'test_long_notice',message:'PTY '+ '长提示'.repeat(90)}});
                    const status=document.querySelector('#composer-input-status');
                    const bubble=status.getBoundingClientRect();
                    status.scrollTop=status.scrollHeight;
                    const scrollable=status.scrollTop>0;
                    const shown=[composer.getBoundingClientRect().top,
                        input.getBoundingClientRect().top];
                    updateComposerInputStatus(composerUid, {ok:true,
                        input:{state:'ready',code:'',message:''}});
                    const hidden=[composer.getBoundingClientRect().top,
                        input.getBoundingClientRect().top];
                    return {focused:document.activeElement===input, selection:input.selectionStart,
                        active:document.activeElement?.id, disabled:input.disabled,
                        visible:input.getClientRects().length>0,
                        inside:bubble.height>0 && composer.contains(document.querySelector('#composer-input-status')),
                        before, shown, hidden, scrollable, bubbleBottom:bubble.bottom};
                }""")
                assert mobile['focused'] and mobile['selection'] == 2, mobile
                assert mobile['inside'], mobile
                assert mobile['before'] == mobile['shown'] == mobile['hidden'], mobile
                assert mobile['scrollable'], mobile
                assert mobile['bubbleBottom'] < mobile['shown'][0], mobile
                page.set_viewport_size({'width': 1280, 'height': 720})
                page.evaluate('removeComposerQuote(composerDraft().quotes[0].id)')
                page.evaluate('async () => await composerDraftWrites')
                # Repeated keyboard attempts keep the inline reason, without
                # a modal or terminal writes.
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'unknown'")
                page.locator('#cinput').press('Enter')
                page.wait_for_function('() => !composerSending')
                assert not dialogs, dialogs
                expect(page.locator('#composer-input-status')).to_be_visible()
                assert page.evaluate("document.activeElement?.id === 'cinput'"), 'failed SEND stole composer focus'
                expect(page.locator('#cinput')).to_have_value('keep this message')
                refused = post('send', text='keep this message', request_id='login-refusal')
                assert refused.status == 409 and refused.json()['code'] == 'cli_not_ready', refused.text()
                assert 'PTY' in refused.json()['error']
                trace = screen.with_suffix('.trace')
                assert not trace.exists(), 'login received terminal input'
                draft = context.request.get(base + '/api/session/conversation', params={'uid': uid}).json()['draft']
                assert draft['value']['text'] == 'keep this message'
                screen.write_text('An unfamiliar nonempty startup screen')
                page.wait_for_timeout(100)
                refused = post('send', text='keep this message', request_id='unknown-refusal')
                assert refused.status == 409 and refused.json()['code'] == 'cli_not_ready', refused.text()
                assert not trace.exists()

                # Recovery permits exactly one paste and Enter and clears the draft.
                screen.write_text('custom')
                wait_code(None)
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'")
                # A stale question card remains display data; PTY readiness wins.
                page.evaluate(r"""() => {
                    const key=viewKey(composerUid), entry=cache.get(key) || {};
                    entry.prompt={id:'stale-question',questions:[{question:'Old question',options:['Yes','No']}]};
                    cache.set(key,entry); syncComposerSendState();
                }""")
                expect(page.locator('#csend')).to_be_enabled()
                with page.expect_response(lambda r: urlsplit(r.url).path.endswith('/conversation/send')) as sent:
                    page.locator('#csend').click()
                assert sent.value.status == 200, sent.value.text()
                expect(page.locator('#cinput')).to_have_value('')
                page.wait_for_timeout(100)
                assert trace.read_bytes() == b'\x1b[200~keep this message\x1b[201~\r', trace.read_bytes()

                # Soft keyboard shrinks the visual viewport; PTY rows and
                # conversation SEND must not follow that inset.
                screen.write_text('custom')
                wait_code(None)
                page.set_viewport_size({'width': 608, 'height': 788})
                page.evaluate('showMobileDetail()')
                page.locator('#a-term').click()
                expect(page.locator('#termpane')).to_be_visible()
                name = receipt['name']
                # A list refresh must not let the old question above undo the
                # user's explicit switch to the terminal. Force the poll here
                # instead of depending on its timer racing this assertion.
                page.evaluate('async () => await loadTermList()')
                expect(page.locator('#termpane')).to_be_visible()
                # A genuinely new question still reveals the conversation;
                # switching back acknowledges that ID, not all future prompts.
                page.evaluate(r"""async () => {
                    const entry=cache.get(viewKey(composerUid));
                    entry.prompt={id:'new-question',questions:[{question:'New question',options:['Yes','No']}]};
                    await loadTermList();
                }""")
                expect(page.locator('#termpane')).to_be_hidden()
                page.locator('#a-term').click()
                page.evaluate('async () => await loadTermList()')
                expect(page.locator('#termpane')).to_be_visible()
                page.wait_for_function(r"""name => {
                    const v = T.views.get(name);
                    return !!(v && v.term && v.lastResizeKey && v.ws && v.ws.readyState === 1);
                }""", arg=name)
                before = page.evaluate(r"""name => {
                    T.name = name;
                    const v = T.views.get(name);
                    syncTermAliases(v);
                    v.ws._resizeSpy = [];
                    const orig = v.ws.send.bind(v.ws);
                    v.ws.send = function(data) {
                        try {
                            const msg = typeof data === 'string' ? JSON.parse(data) : null;
                            if (msg && msg.t === 'resize') v.ws._resizeSpy.push(msg);
                        } catch {}
                        return orig(data);
                    };
                    return {cols: v.term.cols, rows: v.term.rows, key: v.lastResizeKey, name};
                }""", name)
                assert before['cols'] >= 60 and before['rows'] > 10, before
                full = page.evaluate(r"""async () => {
                    const data = await probeComposerInput(composerUid);
                    return {data, input: composerDraft()?.inputStatus || null};
                }""")
                assert full['input']['state'] == 'ready', full
                page.set_viewport_size({'width': 608, 'height': 484})
                page.wait_for_function(r"""before => {
                    const v = T.views.get(before.name);
                    return visualKeyboardOpen()
                        && v && v.term.cols === before.cols
                        && v.term.rows === before.rows
                        && v.lastResizeKey === before.key
                        && v.ws._resizeSpy.length === 0;
                }""", arg=before)
                page.locator('#a-term').click()
                expect(page.locator('#composer')).to_be_visible()
                expect(page.locator('#cinput')).to_be_visible()
                probed = page.evaluate(r"""async () => {
                    const data = await probeComposerInput(composerUid);
                    return {data, input: composerDraft()?.inputStatus || null};
                }""")
                assert probed['input']['state'] == 'ready', probed
                page.set_viewport_size({'width': 608, 'height': 788})
                page.wait_for_function('() => !visualKeyboardOpen()')
                page.set_viewport_size({'width': 1280, 'height': 720})
                page.evaluate(r"""() => {
                    showMobileList();
                    if (typeof suspendTerm === 'function') suspendTerm();
                    for (const view of T.views.values()) view.inputLease = null;
                }""")
                wait_code(None)
                expect(page.locator('#csend')).to_be_enabled()

                # Transient post-paste redraw lasts longer than the old 600ms delay.
                trace.unlink()
                screen.with_suffix('.block').write_text('wait')
                response = post('send', text='wait through redraw', request_id='transient-paste')
                assert response.status == 200, response.text()
                page.wait_for_timeout(100)
                assert trace.read_bytes() == b'\x1b[200~wait through redraw\x1b[201~\r', trace.read_bytes()
                screen.with_suffix('.block').write_text('')

                # The UI can change after paste: SEND must recheck before Enter.
                trace.unlink()
                screen.with_suffix('.block').touch()
                page.locator('#cinput').fill('blocked from the send button')
                page.evaluate('async () => await composerDraftWrites')
                with page.expect_response(lambda r: urlsplit(r.url).path.endswith('/conversation/send')) as blocked_click:
                    page.locator('#csend').click()
                assert blocked_click.value.status == 409, blocked_click.value.text()
                page.wait_for_function('() => !composerSending')
                assert page.evaluate("document.activeElement?.id === 'cinput'"), 'failed button SEND stole composer focus'
                page.keyboard.type('!')
                expect(page.locator('#cinput')).to_have_value('blocked from the send button!')
                assert trace.read_bytes() == b'\x1b[200~blocked from the send button\x1b[201~', trace.read_bytes()
                trace.unlink()
                screen.write_text('custom')
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'")
                page.locator('#cinput').fill('blocked from keyboard button')
                page.evaluate('async () => await composerDraftWrites')
                page.locator('#csend').focus()
                with page.expect_response(lambda r: urlsplit(r.url).path.endswith('/conversation/send')) as blocked_key:
                    page.locator('#csend').press('Enter')
                assert blocked_key.value.status == 409, blocked_key.value.text()
                page.wait_for_function('() => !composerSending')
                assert page.evaluate("document.activeElement?.id === 'cinput'"), page.evaluate(r"""() => ({
                    active:document.activeElement?.id, tag:document.activeElement?.tagName,
                    disabled:document.querySelector('#cinput').disabled,
                    composerUid, selected:S.sel, shown:!document.querySelector('#composer').classList.contains('hidden')
                })""")
                page.keyboard.type('!')
                expect(page.locator('#cinput')).to_have_value('blocked from keyboard button!')
                assert trace.read_bytes() == b'\x1b[200~blocked from keyboard button\x1b[201~', trace.read_bytes()
                trace.unlink()
                screen.write_text('custom')
                wait_code(None)  # Wait for the fake CLI's redraw, not filesystem state.
                response = post('send', text='blocked after paste', request_id='changed-screen')
                assert response.status == 409 and response.json()['code'] == 'cli_not_ready', response.text()
                assert trace.read_bytes() == b'\x1b[200~blocked after paste\x1b[201~', trace.read_bytes()
                replay = post('send', text='blocked after paste', request_id='changed-screen')
                assert replay.status == 409 and replay.json()['code'] == 'send_result_unknown', replay.text()
                assert trace.read_bytes() == b'\x1b[200~blocked after paste\x1b[201~'
                # Persistent startup stays bounded and does not write any bytes.
                screen.with_suffix('.block').unlink()
                trace.unlink()
                screen.write_text('')
                wait_code('cli_starting')
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'starting'")
                expect(page.locator('#dlive')).not_to_have_class(re.compile(r'\binput-attention\b'))
                expect(page.locator('#composer-input-status')).not_to_have_class(re.compile(r'\binput-attention\b'))
                response = post('send', text='still starting', request_id='startup-timeout')
                assert response.status == 409 and response.json()['code'] == 'cli_starting', response.text()
                assert not trace.exists()
                screen.write_text('login')
                page.locator('#a-term').click()
                xterm_includes(page, 'Waiting for approval...')
                expect(page.locator('#composer-input-status')).to_be_hidden()
                # Conversation CHECK/SEND leave another page's PTY lease intact.
                # Only an explicit terminal open asks before revoking it.
                other = browser.new_context(service_workers='block')
                other.route('**/*', lambda route: route.continue_()
                    if route.request.url.startswith(base + '/') else route.abort())
                page_two = other.new_page()
                claims, other_dialogs = [], []
                page_two.on('request', lambda request: claims.append(request.post_data_json)
                    if urlsplit(request.url).path == '/api/term/claim' else None)
                on_popup(page_two, lambda dialog: (other_dialogs.append(dialog.message), dialog.accept()))
                try:
                    page_two.goto(base, wait_until='networkidle')
                    page_two.evaluate('async receipt => {await loadTermList();await openPendingSession(receipt)}', receipt)
                    page_two.wait_for_function('uid => composerUid === uid', arg=uid)
                    page_two.locator('#cinput').fill('message without takeover')
                    page_two.wait_for_function("composerDraft()?.inputStatus?.code === 'cli_not_ready'", timeout=10000)
                    expect(page_two.locator('#composer-input-status .btn')).to_have_count(0)
                    assert not other_dialogs, other_dialogs
                    owner_token = page.evaluate('name => T.views.get(name)?.inputLease?.token', receipt['name'])
                    assert owner_token
                    screen.write_text('custom')
                    page_two.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'", timeout=10000)
                    with page_two.expect_response(lambda r: urlsplit(r.url).path.endswith('/conversation/send')) as sent:
                        page_two.locator('#csend').click()
                    assert sent.value.status == 200, sent.value.text()
                    expect(page_two.locator('#cinput')).to_have_value('')
                    assert trace.read_bytes() == b'\x1b[200~message without takeover\x1b[201~\r', trace.read_bytes()
                    assert not other_dialogs and not any(c.get('force') for c in claims), (other_dialogs, claims)
                    assert page.evaluate('name => T.views.get(name)?.inputLease?.token', receipt['name']) == owner_token
                    assert page.evaluate('T.ws?.readyState === WebSocket.OPEN')
                    page_two.locator('#cinput').fill('draft after takeover')
                    page_two.locator('#a-term').click()
                    page_two.wait_for_function('name => T.views.get(name)?.inputLease?.token && T.ws?.readyState === WebSocket.OPEN',
                        arg=receipt['name'], timeout=15000)
                    force_claims = [claim for claim in claims if claim.get('force') is True]
                    assert len(force_claims) == 1 and force_claims[0]['instance_id'] == receipt['instance_id'], claims
                    assert len(other_dialogs) == 1 and '抢占' in other_dialogs[0], other_dialogs
                    expect(page_two.locator('#cinput')).to_have_value('draft after takeover')
                    page_two.locator('#a-term').click()
                    page_two.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'", timeout=10000)
                    # Pure terminal CSS hides the composer via its parent.
                    checks=[]
                    page_two.on('request', lambda request: checks.append(request.url)
                        if urlsplit(request.url).path == '/api/session/conversation/check' else None)
                    page_two.locator('#a-term').click()
                    expect(page_two.locator('#cinput')).not_to_be_visible()
                    page_two.wait_for_function('!composerInputProbeBusy')
                    before=len(checks)
                    page_two.wait_for_timeout(6200)
                    assert len(checks)==before, f'hidden composer sent {len(checks)-before} CHECKs'
                    with page_two.expect_request(lambda request:
                            urlsplit(request.url).path == '/api/session/conversation/check', timeout=1400):
                        page_two.locator('#a-term').click()
                    expect(page_two.locator('#cinput')).to_be_visible()
                    page_two.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'")
                    print('PASS hidden composer: 0 CHECKs over 6.2s, immediate check on return')
                    stale = page_two.evaluate(r"""async () => {
                        const status=document.querySelector('#composer-input-status');
                        updateComposerInputStatus(composerUid, {ok:false,
                            input:{state:'blocked',code:'cli_not_ready',message:'请切换到 PTY'}});
                        await Promise.resolve();
                        const before=getComputedStyle(status).display;
                        markStaleBuild('new-build');
                        await Promise.resolve();
                        return {before, after:getComputedStyle(status).display,
                            banner:!!document.querySelector('.version-stale'),
                            sendDisabled:document.querySelector('#csend').disabled};
                    }""")
                    assert stale['before'] != 'none' and stale['after'] == 'none', stale
                    assert stale['banner'] and stale['sendDisabled'], stale
                finally:
                    other.close()
                assert not errors, errors
            finally:
                if receipt:
                    context.request.post(base + '/api/term/kill', data={'record_id': receipt['record_id'], 'instance_id': receipt['instance_id']})
                browser.close()
    print('PASS send_readiness_browser: refusal/recovery, mouse and keyboard focus retention, post-paste recheck, no replay, keyboard inset keeps PTY size')


if __name__ == '__main__':
    main()
