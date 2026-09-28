#!/usr/bin/env python3
"""OpenCode launch, console-first page and composer SEND, as a user drives them.

OpenCode is an AI CLI source whose history SessionDock does not read yet. The
page therefore leads with its console, keeps the CLI send path (input checks,
bracketed paste, Enter) and must not wait for a native echo that cannot
arrive. Also checks the five-source picker on desktop and phone, dark theme
and portrait icons. Private fake CLI, loopback server, temporary dirs only.
"""
import json
import os
import re
from pathlib import Path
import sys
import tempfile
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, REPO, Corpus, isolated_server
from send_browser import initialize


def picker_rows(page):
    """Distinct top offsets of the new-session source buttons."""
    return page.evaluate("""() => [...new Set([...document.querySelectorAll(
        '#new-session-form .new-source label')].map(l => Math.round(l.getBoundingClientRect().top)))]""")


def wait_screen(page, text):
    """Wait for text on the live console screen (the renderer paints a canvas)."""
    page.wait_for_function("""text => { const b = T.term?.buffer?.active; if (!b) return false;
        for (let i = 0; i < b.length; i++) if ((b.getLine(i)?.translateToString(true) || '').includes(text)) return true;
        return false; }""", arg=text, timeout=15000)


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-opencode-') as temporary:
        root = Path(temporary).resolve()
        for name in ('host', 'work', 'ledger', 'delivery', 'state', 'home', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        screen = root / 'screen'
        fake = {'executable': str(Path(sys.executable).resolve()),
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'), 'TERM': 'xterm-256color',
                        'LANG': 'C.UTF-8', 'SESSIONDOCK_TEST_SCREEN': str(screen)}}
        profiles = [{'id': 'opencode-cli-v1', 'source': 'opencode',
                     'args': [str(REPO / 'tests/fake_opencode_composer.py')],
                     'resume_args': ['--session', '{sid}'], **fake}]
        # The other CLIs only need to exist so the picker shows all five sources.
        profiles += [{'id': f'{source}-cli-v1', 'source': source, 'args': ['-c', 'import time; time.sleep(60)'],
                      **fake} for source in ('claude', 'codex', 'grok')]
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
                                        'host_dir': str(root / 'host'), 'adapters': [], 'profiles': profiles}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger')
        initialize('--initialize-delivery', root / 'delivery')
        with isolated_server(Corpus(root), BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=launcher, delivery_dir=root / 'delivery', state_dir=root / 'state',
                file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(viewport={'width': 1280, 'height': 860}, service_workers='block')
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page()
            errors, dialogs = [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.on('dialog', lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
            try:
                page.goto(base, wait_until='networkidle')
                # ---- Picker: five sources, one row with labels on desktop,
                # one row of icons on a phone, OpenCode selectable.
                page.locator('#new-session').click()
                labels = page.locator('#new-session-form .new-source label')
                expect(labels).to_have_count(5)
                assert len(picker_rows(page)) == 1, picker_rows(page)
                expect(labels.filter(has_text='OpenCode')).to_be_visible()
                for label in labels.all():
                    scroll, client = label.locator('span').evaluate('e => [e.scrollWidth, e.clientWidth]')
                    assert scroll <= client, (scroll, client)  # no clipped label
                page.set_viewport_size({'width': 390, 'height': 844})
                assert len(picker_rows(page)) == 1, picker_rows(page)
                spans = page.evaluate("""() => [...document.querySelectorAll('#new-session-form .new-source label > span')]
                    .map(e => { const r = e.getBoundingClientRect(); return [r.left, r.right]; })""")
                assert all(b[0] >= a[1] - 0.5 for a, b in zip(spans, spans[1:])), spans
                for label in labels.all():
                    box = label.bounding_box()
                    assert box and box['x'] >= 0 and box['x'] + box['width'] <= 390, box
                expect(labels.filter(has=page.locator('input[value="opencode"]')).locator('.src-label')).to_be_hidden()
                page.set_viewport_size({'width': 1280, 'height': 860})
                labels.filter(has=page.locator('input[value="opencode"]')).click()
                expect(page.locator('input[name="new-source"][value="opencode"]')).to_be_checked()
                page.locator('#new-cwd').fill(str(root / 'work'))
                with page.expect_response(lambda response: urlsplit(response.url).path == '/api/term/create') as created:
                    page.locator('#new-session-go').click()
                assert created.value.status == 200, created.value.text()
                receipt = created.value.json()
                assert receipt['source'] == 'opencode' and receipt['launch_kind'] == 'new_pending', receipt
                uid = 'tmux:' + receipt['name']
                row = page.locator(f'#side .item[data-uid="{uid}"]')

                # ---- The console leads the page; the composer stays below it.
                expect(row).to_have_count(1)
                expect(row.locator('use[href="#i-opencode"]')).to_have_count(1)
                expect(row).to_contain_text('在控制台查看回复')
                expect(page.locator('#detail .meta-source')).to_have_text('OpenCode')
                expect(page.locator('#termpane')).to_be_visible()
                expect(page.locator('#right')).to_have_class(re.compile(r'\bterminal-first\b'))
                assert 'shell-session' not in page.locator('#right').get_attribute('class')
                expect(page.locator('#composer')).to_be_visible()
                expect(page.locator('#detail .msgs')).to_be_hidden()
                pane, composer = page.locator('#termpane').bounding_box(), page.locator('#composer').bounding_box()
                assert pane['y'] + pane['height'] <= composer['y'] + 1 and pane['height'] > 300, (pane, composer)
                page.wait_for_function('T.ws?.readyState === WebSocket.OPEN', timeout=15000)
                wait_screen(page, 'Build · Fake-Model Free')

                if os.environ.get('SESSIONDOCK_TEST_SHOTS'):
                    page.screenshot(path=os.path.join(os.environ['SESSIONDOCK_TEST_SHOTS'], 'opencode-desktop.png'))
                # ---- SEND: ready on the recognized prompt, blocked on a palette.
                page.wait_for_function("() => composerDraft()?.inputStatus?.state === 'ready'", timeout=15000)
                expect(page.locator('#csend')).to_be_enabled()
                expect(page.locator('#cesc')).to_be_visible()
                screen.write_text('palette')
                page.locator('#cinput').fill('blocked while the palette is open')
                page.wait_for_function("() => composerDraft()?.inputStatus?.code === 'cli_not_ready'", timeout=15000)
                expect(page.locator('#composer-input-status')).to_contain_text('请先在上方终端关闭菜单或对话框')
                expect(page.locator('#csend')).to_be_disabled()
                # The server refuses SEND on its own screen check, whatever the page does.
                refused = page.evaluate("""async () => { const name = takenOver(S.sel);
                    try { return await post('api/session/conversation/send', {uid: S.sel, name,
                        text: 'blocked while the palette is open', request_id: crypto.randomUUID(),
                        lease: termSendLease(name).lease || null}); }
                    catch (error) { return {thrown: String(error)}; } }""")
                assert '未识别到 CLI 可输入的消息编辑区' in str(refused), refused
                # The notice sits in the composer, never over the console.
                status, pane = page.locator('#composer-input-status').bounding_box(), page.locator('#termpane').bounding_box()
                assert status['y'] >= pane['y'] + pane['height'] - 1, (status, pane)
                if os.environ.get('SESSIONDOCK_TEST_SHOTS'):
                    page.screenshot(path=os.path.join(os.environ['SESSIONDOCK_TEST_SHOTS'], 'opencode-blocked.png'))
                trace = screen.with_suffix('.trace')
                assert not trace.exists() or b'blocked while' not in trace.read_bytes()
                screen.write_text('composer')
                page.wait_for_function("() => composerDraft()?.inputStatus?.state === 'ready'", timeout=15000)

                def send(text):
                    page.locator('#cinput').fill(text)
                    with page.expect_response(lambda response: urlsplit(response.url).path == '/api/session/conversation/send',
                                              timeout=20000) as sent:
                        page.locator('#csend').click()
                    assert sent.value.status == 200, sent.value.text()
                    body = sent.value.json()
                    assert body['state'] == 'sent' and body['echo_hash'] is None, body
                    expect(page.locator('#cinput')).to_have_value('')
                    # No native echo can arrive: the button must not wait for one.
                    expect(page.locator('#csend')).to_have_attribute('aria-busy', 'false')
                    if '\n' not in text:
                        wait_screen(page, 'sent: ' + text)

                send('hello opencode')
                data = trace.read_bytes()
                assert b'\x1b[200~hello opencode\x1b[201~' in data and data.endswith(b'\r'), data[-120:]
                send('first line\nsecond line\nthird line')
                wait_screen(page, 'sent: [Pasted ~3 lines]')
                assert not dialogs, dialogs

                # ---- Theme and portrait icons stay legible.
                page.evaluate("document.documentElement.dataset.theme = 'dark'")
                color = row.locator('.source-icon').evaluate('e => getComputedStyle(e).color')
                assert color == 'rgb(170, 178, 191)', color
                page.evaluate("document.documentElement.dataset.toolIcons = 'boss'")
                use = row.locator('.source-icon > use')
                assert use.evaluate('e => getComputedStyle(e).visibility') == 'visible'
                page.evaluate("delete document.documentElement.dataset.toolIcons; document.documentElement.dataset.theme = 'light'")

                # ---- Phone: console first, composer reachable.
                page.set_viewport_size({'width': 390, 'height': 844})
                page.wait_for_timeout(300)
                if not page.locator('#composer').is_visible():
                    row.click()  # the phone shows the list first; a tap opens the session
                expect(page.locator('#termpane')).to_be_visible()
                page.wait_for_function('T.ws?.readyState === WebSocket.OPEN', timeout=15000)
                pane, composer = page.locator('#termpane').bounding_box(), page.locator('#composer').bounding_box()
                assert pane['y'] + pane['height'] <= composer['y'] + 1 and pane['height'] > 300, (pane, composer)
                if os.environ.get('SESSIONDOCK_TEST_SHOTS'):
                    page.screenshot(path=os.path.join(os.environ['SESSIONDOCK_TEST_SHOTS'], 'opencode-phone.png'))
                expect(page.locator('#composer')).to_be_visible()
                page.set_viewport_size({'width': 1280, 'height': 860})

                # ---- Reporting a problem from an OpenCode session opens the
                # ordinary dialog; the handling CLI stays one of the report CLIs.
                page.locator('.dhead-actions [data-report-bug]').click()
                expect(page.locator('#bug-report-dialog')).to_be_visible()
                assert page.evaluate("[...document.querySelectorAll('#bug-report-source input')].map(i => i.value)") \
                    == ['claude', 'codex', 'grok']
                if os.environ.get('SESSIONDOCK_TEST_SHOTS'):
                    page.screenshot(path=os.path.join(os.environ['SESSIONDOCK_TEST_SHOTS'], 'opencode-report.png'))
                page.keyboard.press('Escape')
                expect(page.locator('#bug-report-dialog')).to_be_hidden()

                # ---- Delete: stops the CLI and removes the row.
                if os.environ.get('SESSIONDOCK_TEST_SHOTS'):
                    page.wait_for_timeout(500)
                    page.screenshot(path=os.path.join(os.environ['SESSIONDOCK_TEST_SHOTS'], 'opencode-back-desktop.png'))
                action = page.locator('#a-session-action')
                if not action.is_visible():
                    page.locator('#a-more').click()
                expect(action).to_have_attribute('aria-label', '删除会话')
                with page.expect_response(lambda response: urlsplit(response.url).path == '/api/term/discard',
                                          timeout=30000) as discarded:
                    action.click()
                assert discarded.value.status == 200, discarded.value.text()
                expect(row).to_have_count(0)
                assert not errors, errors
            finally:
                context.close()
                browser.close()
    print('PASS opencode browser: five-source picker (desktop labels, phone icons), OpenCode launch, '
          'console-first page, input checks, paste+Enter SEND without echo wait, dark/portrait icons, delete')


if __name__ == '__main__':
    main()
