#!/usr/bin/env python3
"""OpenCode sessions end to end, as a user drives them.

The server mirrors an OpenCode 2 style SQLite store (`project`, `session_v2`,
`session_message`) into files and lists, renders and searches its sessions.
A new session is created in OpenCode under a server-assigned id before its
TUI starts with `--session`, so the page switches straight to the native row.
Covered: the five-source picker on desktop and phone, a seeded session with
reasoning, tool calls, an image and failures, body search, launch, composer
SEND with its native echo, input checks, stop and resume, portrait/dark
icons, the report dialog and irreversible delete. Private fake CLI,
loopback server, temporary directories only.
"""
import base64
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import time
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from history_parity import BINARY, REPO, Corpus, isolated_server
from send_browser import initialize

sys.path.insert(0, str(REPO / 'tests'))
import fake_opencode_composer as fake  # noqa: E402
from popups import on_popup  # noqa: E402

# 1x1 PNG, as OpenCode stores a pasted image inline.
PIXEL = base64.b64encode(bytes.fromhex(
    '89504e470d0a1a0a0000000d4948445200000001000000010806000000'
    '1f15c4890000000d49444154789c6360f8cfc0f01f0005000201ff6c2d5e2b0000000049454e44ae426082')).decode()
SEEDED = 'ses_0f0000000000seededSessionA'


def seed(db, work):
    """One finished session with the kinds of rows OpenCode 2 writes."""
    connection = sqlite3.connect(db)
    connection.executescript(fake.SCHEMA)
    t = 1790500000000
    connection.execute("INSERT INTO project VALUES ('fakeproject', ?, ?, ?, '[]')", (str(work), t, t))
    connection.execute(
        "INSERT INTO session_v2 (id, project_id, slug, directory, title, version, agent, model,"
        " time_created, time_updated) VALUES (?, 'fakeproject', 'seeded', ?, '已有的 OpenCode 会话',"
        " '2.0.18', 'build', ?, ?, ?)", (SEEDED, str(work), json.dumps(fake.MODEL), t, t + 50))
    rows = [
        ('user', {'time': {'created': t}, 'text': '看看这张图 [Image 1]', 'agents': [],
                  'files': [{'data': PIXEL, 'mime': 'image/png', 'source': {'type': 'inline'},
                             'name': 'clipboard'}]}),
        ('assistant', {'time': {'created': t + 1, 'completed': t + 2}, 'agent': 'build',
                       'model': fake.MODEL, 'finish': 'tool-calls', 'content': [
                           {'type': 'reasoning', 'text': '先列一下目录'},
                           {'type': 'tool', 'id': 'call_1', 'name': 'shell', 'state': {
                               'status': 'completed', 'input': {'command': 'ls'},
                               'content': [{'type': 'text', 'text': 'README.md'}]}},
                           {'type': 'tool', 'id': 'call_2', 'name': 'read', 'state': {
                               'status': 'error', 'input': {'path': '/nope'}, 'error': 'No such file'}}]}),
        ('assistant', {'time': {'created': t + 3, 'completed': t + 4}, 'agent': 'build',
                       'model': fake.MODEL, 'finish': 'stop',
                       'content': [{'type': 'text', 'text': '目录里有 zebracorn 文件。'}]}),
        ('idle', {'time': {'created': t + 5}, 'outcome': 'succeeded'}),
        ('user', {'time': {'created': t + 6}, 'text': '再来一次', 'files': [], 'agents': []}),
        ('assistant', {'time': {'created': t + 7}, 'agent': 'build', 'model': fake.MODEL,
                       'finish': 'error', 'content': [],
                       'error': {'type': 'provider.auth', 'message': 'Key limit exceeded'}}),
        ('idle', {'time': {'created': t + 8}, 'outcome': 'failed'}),
        ('user', {'time': {'created': t + 9}, 'text': '第三次', 'files': [], 'agents': []}),
        ('idle', {'time': {'created': t + 10}, 'outcome': 'failed'}),
        ('agent-switched', {'time': {'created': t + 11}, 'agent': 'build'}),
        ('location-switched', {'time': {'created': t + 12}, 'directory': str(work)}),
        ('user', {'time': {'created': t + 13}, 'text': '列八十种水果', 'files': [], 'agents': []}),
        ('assistant', {'time': {'created': t + 14, 'completed': t + 15}, 'agent': 'build',
                       'model': fake.MODEL, 'finish': 'error',
                       'error': {'type': 'aborted', 'message': 'The operation was aborted.'},
                       'content': [{'type': 'reasoning', 'text': '逐个列出'},
                                   {'type': 'text', 'text': '苹果、香蕉、樱桃，写到这里'}]}),
        ('idle', {'time': {'created': t + 16}, 'outcome': 'interrupted'}),
        # One reasoning part plus the reply: the reasoning still folds into the turn process.
        ('user', {'time': {'created': t + 17}, 'text': '能说话吗', 'files': [], 'agents': []}),
        ('assistant', {'time': {'created': t + 18, 'completed': t + 19}, 'agent': 'build',
                       'model': fake.MODEL, 'finish': 'stop',
                       'content': [{'type': 'reasoning', 'text': '单段思考 quokkaridge'},
                                   {'type': 'text', 'text': '能，链路是通的。'}]}),
        ('idle', {'time': {'created': t + 20}, 'outcome': 'succeeded'}),
    ]
    for seq, (kind, data) in enumerate(rows, 1):
        connection.execute('INSERT INTO session_message VALUES (?, ?, ?, ?, ?, ?, ?)',
                           (f'msg_seed_{seq}', SEEDED, kind, seq, t + seq, t + seq, json.dumps(data)))
    connection.commit()
    connection.close()


def legacy_pending_record(ledger, work):
    """A finished OpenCode launch from before pre-creation (`new_pending`) must still load."""
    path = ledger / 'lifecycle-ledger.json'
    data = json.loads(path.read_text())
    data['revision'] = 3
    data['records']['bcc0f47478d75ded04a7ca2994a646f7'] = {
        'record_id': 'bcc0f47478d75ded04a7ca2994a646f7', 'request_id': '28aa3bc1-92dd-40a8-9d8f-e00bc58967e1',
        'spec': {'source': 'opencode', 'adapter_id': 'opencode-cli-v1', 'cwd': str(work),
                 'launch': {'kind': 'new_pending'}},
        'launch_id': '766ff9293ff3340de955e2c01934da11', 'instance_id': '540f4d8ab1094a9ee68605db972b4d6d',
        'host_name': 'sessiondock-0183e7c024d811f75511c4b87498cdcc', 'revision': 3, 'state': 'exited',
        'failure': None, 'cancel_requested': True, 'binding': None, 'session_id': None,
        'created_at': 1790576084, 'finished_at': 1790578497, 'discarded': True}
    path.write_text(json.dumps(data))


def picker_rows(page):
    """Distinct top offsets of the new-session source buttons."""
    return page.evaluate("""() => [...new Set([...document.querySelectorAll(
        '#new-session-form .new-source label')].map(l => Math.round(l.getBoundingClientRect().top)))]""")


def wait_screen(page, text):
    """Wait for text on the live console screen (the renderer paints a canvas)."""
    page.wait_for_function("""text => { const b = T.term?.buffer?.active; if (!b) return false;
        for (let i = 0; i < b.length; i++) if ((b.getLine(i)?.translateToString(true) || '').includes(text)) return true;
        return false; }""", arg=text, timeout=15000)


def rows_of(page, base):
    return [row for row in page.request.get(base + '/api/sessions?force=1').json()['sessions']
            if row['source'] == 'opencode']


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-opencode-') as temporary:
        root = Path(temporary).resolve()
        for name in ('host', 'work', 'ledger', 'delivery', 'state', 'home', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        screen = root / 'screen'
        db = root / 'opencode.db'
        seed(db, root / 'work')
        env = {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'), 'TERM': 'xterm-256color',
               'LANG': 'C.UTF-8', 'SESSIONDOCK_TEST_SCREEN': str(screen),
               'SESSIONDOCK_TEST_OPENCODE_DB': str(db)}
        executable = str(Path(sys.executable).resolve())
        profiles = [{'id': 'opencode-cli-v1', 'source': 'opencode', 'executable': executable,
                     'args': [str(REPO / 'tests/fake_opencode_composer.py')],
                     'resume_args': ['--session', '{sid}'], 'env': env}]
        # The other CLIs only need to exist so the picker shows all five sources.
        profiles += [{'id': f'{source}-cli-v1', 'source': source, 'executable': executable,
                      'args': ['-c', 'import time; time.sleep(60)'], 'env': env}
                     for source in ('claude', 'codex', 'grok')]
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
                                        'host_dir': str(root / 'host'), 'adapters': [], 'profiles': profiles}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger')
        legacy_pending_record(root / 'ledger', root / 'work')
        with isolated_server(Corpus(root), BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=launcher, state_dir=root / 'state',
                trash_dir=root / 'trash', audit_dir=root / 'audit',
                file_roots=(root / 'work',), file_write_roots=(root / 'work',),
                extra_env={'SESSIONDOCK_OPENCODE_DB': str(db),
                           'SESSIONDOCK_OPENCODE_ROOT': str(root / 'mirror'),
                           'SESSIONDOCK_BUG_REPORT_DIR': str(root / 'reports'),
                           'SESSIONDOCK_BUG_REPORT_REPO': str(root / 'work')}) as (base, _), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(viewport={'width': 1280, 'height': 860}, service_workers='block')
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page()
            errors, dialogs = [], []
            page.on('pageerror', lambda error: errors.append(str(error)))
            on_popup(page, lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
            shots = os.environ.get('SESSIONDOCK_TEST_SHOTS')
            shot = (lambda name: page.screenshot(path=os.path.join(shots, f'opencode-{name}.png'))) if shots else (lambda name: None)
            try:
                page.goto(base, wait_until='networkidle')
                # ---- The mirrored seed session: listed, rendered, searchable.
                page.wait_for_function("() => S.sessions.some(row => row.source === 'opencode')", timeout=20000)
                seeded = next(row for row in rows_of(page, base) if row['sid'] == SEEDED)
                assert seeded['title'] == '已有的 OpenCode 会话' and seeded['model'] == 'fake-model', seeded
                assert seeded['cwd'] == str(root / 'work') and seeded['supported'], seeded
                # Agent/location switches are bookkeeping, not unknown records.
                assert 'OpenCode 记录类型' not in json.dumps(seeded, ensure_ascii=False), seeded
                item = page.locator(f'#side .item[data-uid="{seeded["uid"]}"]')
                expect(item.locator('use[href="#i-opencode"]')).to_have_count(1)
                item.click()
                msgs = page.locator('#msgs')
                expect(msgs).to_contain_text('目录里有 zebracorn 文件。')
                expect(msgs).to_contain_text('[OpenCode 错误] Key limit exceeded')
                expect(msgs).to_contain_text('[OpenCode] 本轮失败，没有产生回复')
                # An Esc-interrupted turn keeps its partial reply, marked interrupted.
                interrupted = page.locator('#msgs .msg').filter(has_text='苹果、香蕉、樱桃，写到这里')
                expect(interrupted).to_have_count(1)
                expect(interrupted.locator('.native-message-state')).to_have_text('已中断')
                # A failure the assistant row explained is not repeated by its idle marker.
                assert page.locator('#msgs .msg').filter(has_text='本轮失败').count() == 1
                expect(page.locator('#msgs .msg[data-role=user]').first).to_contain_text('看看这张图')
                expect(page.locator('#msgs .msg[data-role=user] img')).to_have_count(1)
                roles = page.evaluate("[...new Set([...document.querySelectorAll('#msgs .msg')].map(m => m.dataset.role))]")
                assert {'user', 'assistant'} <= set(roles) and ('process' in roles or 'toolgroup' in roles), roles
                # A lone reasoning block is folded with its turn, not left on the main line.
                expect(page.locator('#msgs > .msg[data-role=assistant]').filter(has_text='能，链路是通的。')).to_have_count(1)
                expect(page.locator('#msgs > .msg[data-role=thinking]')).to_have_count(0)
                lone = page.locator('#msgs > .turn-process').filter(has_text='1 段思考').last
                expect(lone).to_have_class('msg turn-process folded')
                expect(lone.locator('.turn-process-body')).to_be_hidden()
                lone.locator('.fold-toggle').click()
                expect(lone.locator('.msg[data-role=thinking]')).to_contain_text('单段思考 quokkaridge')
                expect(page.locator('#detail .msgs')).to_be_visible()
                shot('seeded')
                old = page.locator('#stat').get_attribute('data-seq') or ''
                page.locator('#q').fill('zebracorn')
                page.locator('#q').press('Enter')
                page.wait_for_function("old => String(document.querySelector('#stat').dataset.seq || '') !== old", arg=old)
                expect(page.locator(f'#side .item[data-uid="{seeded["uid"]}"]')).to_be_visible()
                expect(page.locator(f'#side .item[data-uid="{seeded["uid"]}"] .m')).to_contain_text('命中')
                page.locator('#q').fill('')
                page.locator('#q').press('Enter')

                # ---- Picker: five sources, one labelled row on desktop, icons on a phone.
                page.locator('#new-session').click()
                labels = page.locator('#new-session-form .new-source label')
                expect(labels).to_have_count(5)
                assert len(picker_rows(page)) == 1, picker_rows(page)
                for label in labels.all():
                    scroll, client = label.locator('span').evaluate('e => [e.scrollWidth, e.clientWidth]')
                    assert scroll <= client, (scroll, client)
                page.set_viewport_size({'width': 390, 'height': 844})
                assert len(picker_rows(page)) == 1, picker_rows(page)
                spans = page.evaluate("""() => [...document.querySelectorAll('#new-session-form .new-source label > span')]
                    .map(e => { const r = e.getBoundingClientRect(); return [r.left, r.right]; })""")
                assert all(b[0] >= a[1] - 0.5 for a, b in zip(spans, spans[1:])), spans
                page.set_viewport_size({'width': 1280, 'height': 860})
                labels.filter(has=page.locator('input[value="opencode"]')).click()
                page.locator('#new-cwd').fill(str(root / 'work'))
                with page.expect_response(lambda response: urlsplit(response.url).path == '/api/term/create') as created:
                    page.locator('#new-session-go').click()
                assert created.value.status == 200, created.value.text()
                receipt = created.value.json()
                sid = receipt['declared_sid']
                assert receipt['launch_kind'] == 'new_assigned' and sid.startswith('ses_') and receipt['running'], receipt

                # ---- The session exists in OpenCode before its TUI: the page
                # moves to the native row at once.
                page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=sid, timeout=20000)
                uid = next(row['uid'] for row in rows_of(page, base) if row['sid'] == sid)
                page.wait_for_function('uid => S.sel === uid', arg=uid, timeout=20000)
                expect(page.locator('#detail .meta-source')).to_have_text('OpenCode')
                expect(page.locator('#composer')).to_be_visible()
                page.wait_for_function("() => composerDraft()?.inputStatus?.state === 'ready'", timeout=20000)

                def send(text, echo, reply):
                    page.locator('#cinput').fill(text)
                    with page.expect_response(lambda response: urlsplit(response.url).path == '/api/session/conversation/send',
                                              timeout=20000) as sent:
                        page.locator('#csend').click()
                    assert sent.value.status == 200 and sent.value.json()['state'] == 'sent', sent.value.text()
                    expect(page.locator('#cinput')).to_have_value('')
                    expect(page.locator('#msgs .msg[data-role=assistant]').filter(has_text=reply)).to_have_count(1, timeout=20000)
                    expect(page.locator('#msgs .msg[data-role=user]').filter(has_text=echo)).to_have_count(1)
                    # The native echo settles the send button.
                    expect(page.locator('#csend')).to_have_attribute('aria-busy', 'false', timeout=15000)

                send('hello opencode', 'hello opencode', 'echo: hello opencode')
                trace = screen.with_suffix('.trace')
                assert b'\x1b[200~hello opencode\x1b[201~' in trace.read_bytes()
                send('first line\nsecond line\nthird line', 'third line', 'echo: first line')
                expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_contain_text('hello opencode')
                shot('conversation')

                # ---- The server refuses SEND while a palette covers the prompt.
                screen.write_text('palette')
                page.locator('#cinput').fill('blocked while the palette is open')
                page.wait_for_function("() => composerDraft()?.inputStatus?.code === 'cli_not_ready'", timeout=15000)
                expect(page.locator('#csend')).to_be_disabled()
                refused = page.evaluate("""async () => { const name = takenOver(S.sel);
                    try { return await post('api/session/conversation/send', {uid: S.sel, name,
                        text: 'blocked while the palette is open', request_id: crypto.randomUUID(),
                        lease: termSendLease(name).lease || null}); }
                    catch (error) { return {thrown: String(error)}; } }""")
                assert '未识别到 CLI 可输入的消息编辑区' in str(refused), refused
                assert b'blocked while' not in trace.read_bytes()
                # ---- OpenCode's question form and permission prompt are CLI questions.
                for dialog in ('question', 'permission'):
                    screen.write_text(dialog)
                    page.wait_for_function("() => composerDraft()?.inputStatus?.code === 'cli_question'", timeout=15000)
                    expect(page.locator('#csend')).to_be_disabled()
                    expect(page.locator('#composer-input-status')).to_contain_text('检测到终端选择界面')
                    shot(dialog)
                    refused = page.evaluate("""async () => { const name = takenOver(S.sel);
                        try { return await post('api/session/conversation/send', {uid: S.sel, name,
                            text: 'blocked while the palette is open', request_id: crypto.randomUUID(),
                            lease: termSendLease(name).lease || null}); }
                        catch (error) { return {thrown: String(error)}; } }""")
                    assert 'CLI 正在等待选择' in str(refused), refused
                    assert b'blocked while' not in trace.read_bytes()
                screen.write_text('composer')
                page.wait_for_function("() => composerDraft()?.inputStatus?.state === 'ready'", timeout=15000)
                expect(page.locator('#csend')).to_be_enabled()
                page.locator('#cinput').fill('')

                # ---- Theme keeps source icons legible.
                row = page.locator(f'#side .item[data-uid="{uid}"]')
                page.evaluate("document.documentElement.dataset.theme = 'dark'")
                assert row.locator('.source-icon').evaluate('e => getComputedStyle(e).color') == 'rgb(170, 178, 191)'
                page.evaluate("document.documentElement.dataset.theme = 'light'")

                # ---- Report a problem: the ordinary dialog, report CLIs only.
                page.locator('.dhead-actions [data-report-bug]').click()
                expect(page.locator('#bug-report-dialog')).to_be_visible()
                assert page.evaluate("[...document.querySelectorAll('#bug-report-source input')].map(i => i.value)") \
                    == ['claude', 'codex', 'grok', 'opencode']
                page.keyboard.press('Escape')
                expect(page.locator('#bug-report-dialog')).to_be_hidden()

                # ---- An older empty catalog snapshot must not classify the
                # populated conversation as an unused pre-created launch. Keep
                # watch disconnected for this snapshot; HTTP and host evidence
                # still come from the private running server.
                def old_cursor(route):
                    response = route.fetch()
                    data = response.json()
                    rows = data.get('sessions', []) if 'sessions' in data else [data.get('meta', {})]
                    for row in rows:
                        if row.get('uid') == uid:
                            row['cursor'] = {**row.get('cursor', {}), 'end': 0}
                    route.fulfill(response=response, json=data)
                def disconnected_watch(route):
                    route.abort()
                page.route('**/api/sessions?*', old_cursor)
                page.route('**/api/messages/**', old_cursor)
                page.route('**/api/watch?*', disconnected_watch)
                page.reload(wait_until='domcontentloaded')
                page.wait_for_function('T.listLoaded')
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.wait_for_function('uid => S.sel === uid && cache.get(viewKey(uid,null))?.end > 0 && takenOver(uid)', arg=uid)
                expect(page.locator('#msgs')).to_contain_text('echo: hello opencode')
                assert page.evaluate('uid => cache.get(viewKey(uid,null)).meta.cursor.end', uid) == 0

                # ---- Stop, then resume from the native row with --session.
                action = page.locator('#a-session-action')
                if not action.is_visible():
                    page.locator('#a-more').click()
                expect(action).to_have_attribute('aria-label', '停止会话')
                page.unroute('**/api/sessions?*', old_cursor)
                page.unroute('**/api/messages/**', old_cursor)
                page.unroute('**/api/watch?*', disconnected_watch)
                action.click()
                page.wait_for_function("() => document.querySelector('#a-session-action')?.getAttribute('aria-label') === '删除会话'", timeout=30000)
                page.wait_for_function('uid => !S.live.has(uid) && !takenOver(uid)', arg=uid, timeout=30000)
                shot('stopped')
                with page.expect_response(lambda response: urlsplit(response.url).path == '/api/term/takeover', timeout=30000) as resumed:
                    page.locator('#a-term').click()
                assert resumed.value.json()['launch_kind'] == 'resume', resumed.value.text()
                assert resumed.value.json()['declared_sid'] == sid, resumed.value.text()
                page.wait_for_function('T.ws?.readyState === WebSocket.OPEN', timeout=20000)
                wait_screen(page, 'sent: hello opencode')
                shot('resumed')

                # ---- Delete: stop, confirm the irreversible OpenCode delete.
                page.locator('#a-term').click()
                action = page.locator('#a-session-action')
                if not action.is_visible():
                    page.locator('#a-more').click()
                if action.get_attribute('aria-label') == '停止会话':
                    action.click()
                    page.wait_for_function("() => document.querySelector('#a-session-action')?.getAttribute('aria-label') === '删除会话'", timeout=30000)
                    if not action.is_visible():
                        page.locator('#a-more').click()
                with page.expect_response(lambda response: response.request.method == 'DELETE', timeout=30000) as removed:
                    action.click()
                assert removed.value.status == 200 and removed.value.json()['entry_id'] == '', removed.value.text()
                assert any('OpenCode 会话会从 OpenCode 直接删除' in message and '无法恢复' in message for message in dialogs), dialogs
                expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_count(0)
                expect(page.locator('#detail')).to_contain_text('会话已从 OpenCode 删除')
                with sqlite3.connect(db) as connection:
                    assert connection.execute('SELECT count(*) FROM session_v2 WHERE id = ?', (sid,)).fetchone()[0] == 0
                    assert connection.execute('SELECT count(*) FROM session_message WHERE session_id = ?', (sid,)).fetchone()[0] == 0
                assert not (root / 'mirror' / 'fakeproject' / sid).exists()

                # ---- A report handled by OpenCode: the worker session is pre-created
                #      in the repository and receives the report prompt through SEND.
                page.locator('#report-bug').click()
                expect(page.locator('#bug-report-dialog')).to_be_visible()
                page.locator('#bug-report-form label:has(input[value="opencode"])').click()
                page.fill('#bug-report-description', 'OpenCode 处理这份报告')
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/bug-report', timeout=60000) as reported:
                    page.locator('#bug-report-go').click()
                assert reported.value.status == 202, reported.value.text()
                report = reported.value.json()
                worker = report['worker']
                assert worker['source'] == 'opencode' and worker['sid'] and worker['cwd'] == str(root / 'work'), worker
                deadline = time.monotonic() + 45
                while True:
                    with sqlite3.connect(db) as connection:
                        texts = [json.loads(data).get('text', '') for (data,) in connection.execute(
                            "SELECT data FROM session_message WHERE session_id = ? AND type = 'user'", (worker['sid'],))]
                    if any(report['report_id'] in text for text in texts) or time.monotonic() > deadline:
                        break
                    time.sleep(0.3)
                assert any(report['report_id'] in text for text in texts), texts
                assert not errors, errors
            finally:
                context.close()
                browser.close()
    print('PASS opencode browser: mirrored seed (image, tools, failures) listed/rendered/searched, five-source picker, '
          'pre-created launch lands on the native row, SEND with native echo, input checks, stop+resume, '
          'icons, report dialog, irreversible delete, OpenCode report worker receives the prompt')


if __name__ == '__main__':
    main()
