#!/usr/bin/env python3
"""Claude editable queue pops must not remain pending SENDs (BUG-20261014-151630-58b85b)."""
import argparse
from contextlib import ExitStack
from datetime import datetime, timezone, timedelta
import json
import os
from pathlib import Path
import sys
import tempfile
import uuid

from playwright.sync_api import expect, sync_playwright

from history_fixtures import REPO, BINARY, Corpus, isolated_server
from private_hosts import private_hosts
from send_browser import SETTINGS, create_claude, initialize, wait_history


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--before', action='store_true')
    args = parser.parse_args()
    screenshots = REPO / 'target/validation/claude-returned'
    screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-claude-returned-') as temporary, private_hosts(Path(temporary)):
        root = Path(temporary)
        for name in ['host', 'work/claude-area', 'ledger', 'state', 'home', 'claude', 'codex', 'grok']:
            (root / name).mkdir(parents=True, mode=0o700)
        fake = root / 'fake.py'
        fake.write_text('''import sys
sys.path.insert(0, %r)
from fake_claude_cli import Fake, parse
class QueueFake(Fake):
    def submit(self):
        if self.buffer.startswith('Queue:'):
            text = self.buffer
            self.buffer = ''
            self.collapsed_paste = None
            self.append([self.queue_operation('enqueue', text)])
            self.render()
        else:
            super().submit()
QueueFake(parse(sys.argv[1:])).run()
''' % str(REPO / 'tests'))
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
            'host_dir': str(root / 'host'), 'adapters': [], 'profiles': [{
                'id': 'claude-cli-v1', 'source': 'claude', 'executable': sys.executable,
                'args': [str(fake), '--settings', SETTINGS, '--reply'],
                'new_args': ['--session-id', '{session_id}'], 'resume_args': ['--resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'), 'TERM': 'xterm-256color',
                        'LANG': 'C.UTF-8', 'SESSIONDOCK_TEST_CLAUDE_ROOT': str(root / 'claude')}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger', args.binary)
        def server():
            return isolated_server(Corpus(root), args.binary, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=launcher, state_dir=root / 'state', file_roots=(root / 'work',),
                file_write_roots=(root / 'work',))

        with ExitStack() as servers, sync_playwright() as pw:
            base, _ = servers.enter_context(server())
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            context = browser.new_context(service_workers='block', viewport={'width': 1280, 'height': 900})
            context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
            page = context.new_page()
            errors, sends = [], []
            page.on('pageerror', lambda e: errors.append(str(e)))
            page.on('request', lambda r: sends.append(r.post_data_json) if r.url.endswith('/conversation/send') else None)
            try:
                page.goto(base, wait_until='networkidle')
                receipt = create_claude(page, base, root / 'work', open_terminal=False)
                page.wait_for_function('composerUid && !composerDraft().loading && takenOver(composerUid)')
                native = root / 'claude/project-history' / (receipt['declared_sid'] + '.jsonl')

                def send(text):
                    page.locator('#cinput').fill(text)
                    expect(page.locator('#csend')).to_be_enabled()
                    with page.expect_response(lambda r: r.url.endswith('/conversation/send')) as response:
                        page.locator('#csend').click()
                    assert response.value.status == 200, response.value.text()
                    return response.value.json()['request_id']

                def append(operation, text, *, age=0):
                    timestamp = (datetime.now(timezone.utc) - timedelta(seconds=age)).isoformat()
                    row = {'type': 'queue-operation', 'operation': operation, 'content': text,
                           'sessionId': receipt['declared_sid'], 'timestamp': timestamp}
                    with native.open('a') as stream:
                        stream.write(json.dumps(row) + '\n')

                def bubble(request):
                    return page.locator(f'.queued-send[data-request-id="{request}"]')

                send('Bootstrap queue return observation')
                wait_history(page, 'OK: Bootstrap queue return observation')
                text = 'Queue: 默认是只显示高亮部分；保留这条输入，不要误报已处理。'
                request = send(text)
                expect(bubble(request).locator('.queued-send-state')).to_have_text('已进入 CLI 队列，当前步骤结束后处理')
                append('popAll', text)
                if args.before:
                    page.wait_for_timeout(1500)
                    page.screenshot(path=str(screenshots / 'before.png'))
                    return
                returned = '已从 CLI 队列退回编辑区，未确认处理，请到终端查看'
                expect(bubble(request).locator('.queued-send-state')).to_have_text(returned)
                assert not page.locator('#msgs .msg[data-role=user]:not(.queued-send)').filter(has_text=text).count()
                for width, theme in [(1280, 'light'), (675, 'light'), (1280, 'dark'), (675, 'dark')]:
                    page.set_viewport_size({'width': width, 'height': 900 if width == 1280 else 542})
                    page.evaluate('(theme) => { document.documentElement.dataset.theme = theme; }', theme)
                    if not bubble(request).is_visible():
                        page.locator('#side .item').filter(has_text='Bootstrap queue return observation').click()
                    expect(bubble(request)).to_be_visible()
                    page.screenshot(path=str(screenshots / f'{width}-{theme}.png'))
                page.set_viewport_size({'width': 1280, 'height': 900})
                page.reload(wait_until='networkidle')
                expect(bubble(request).locator('.queued-send-state')).to_have_text(returned)
                expect(bubble(request).locator('button')).to_have_attribute('title', '仅关闭此发送提示，不取消或重发 CLI 输入')
                bubble(request).locator('button').click()
                expect(bubble(request)).to_have_count(0)
                print('PASS popAll preserves text as returned, survives reload, and dismisses without resend', flush=True)

                # Historical pop, missing content and ordinary drain operations
                # are not evidence that this new SEND was returned for editing.
                second = send(text)
                expect(bubble(second).locator('.queued-send-state')).to_have_text('已进入 CLI 队列，当前步骤结束后处理')
                append('popOne', text, age=60)
                append('popAll', '')
                append('dequeue', text)
                append('remove', text)
                page.reload(wait_until='networkidle')
                expect(bubble(second).locator('.queued-send-state')).to_have_text('已进入 CLI 队列，当前步骤结束后处理')
                append('popOne', text)
                expect(bubble(second).locator('.queued-send-state')).to_have_text(returned)
                print('PASS popOne, old/empty pop and ordinary drain boundaries', flush=True)

                # A real user echo still wins; pop is never forged into history.
                rows = [json.loads(line) for line in native.read_text().splitlines()]
                parent = next(row['uuid'] for row in reversed(rows) if row.get('uuid'))
                with native.open('a') as stream:
                    stream.write(json.dumps({'type': 'user', 'uuid': str(uuid.uuid4()), 'parentUuid': parent,
                        'sessionId': receipt['declared_sid'], 'cwd': str(root / 'work/claude-area'),
                        'timestamp': datetime.now(timezone.utc).isoformat(),
                        'message': {'role': 'user', 'content': text}}) + '\n')
                expect(bubble(second)).to_have_count(0)
                wait_history(page, text)
                assert len(sends) == 3, sends
                print('PASS later native user echo settles returned SEND without duplicate delivery', flush=True)

                # Stop only the private web service before the pop: its ledger
                # remains an old queued receipt. Bury the evidence in an older
                # history page, then reopen through the ordinary session list.
                old_text = 'Queue: 历史排队回执在升级后应按原生退回证据自动恢复'
                old_request = send(old_text)
                expect(bubble(old_request).locator('.queued-send-state')).to_have_text('已进入 CLI 队列，当前步骤结束后处理')
                uid = page.evaluate('S.sel')
                page.goto('about:blank')
                servers.close()
                rows = [json.loads(line) for line in native.read_text().splitlines()]
                parent = next(row['uuid'] for row in reversed(rows) if row.get('uuid'))
                with native.open('a') as stream:
                    for index in range(700):
                        if index == 100:
                            stream.write(json.dumps({'type': 'queue-operation', 'operation': 'popAll',
                                'content': old_text, 'sessionId': receipt['declared_sid'],
                                'timestamp': datetime.now(timezone.utc).isoformat()}) + '\n')
                        current = str(uuid.uuid4())
                        role = 'user' if index % 2 == 0 else 'assistant'
                        stream.write(json.dumps({'type': role, 'uuid': current, 'parentUuid': parent,
                            'sessionId': receipt['declared_sid'], 'timestamp': datetime.now(timezone.utc).isoformat(),
                            'message': {'role': role, 'content': 'Later history item %d' % index}}) + '\n')
                        parent = current
                base, _ = servers.enter_context(server())
                page.set_viewport_size({'width': 1280, 'height': 900})
                page.goto(base, wait_until='networkidle')
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                expect(bubble(old_request).locator('.queued-send-state')).to_have_text(returned)
                wire = context.request.get(base + '/api/messages/' + uid + '?window=1').json()
                assert wire.get('partial'), 'fixture must exercise paginated history'
                assert not any(m.get('operation') == 'popAll' and m.get('text') == old_text for m in wire['messages'])
                assert len(sends) == 4, sends
                assert not errors, errors
                print('PASS restart repairs old queued receipt even when pop evidence is outside the opening window', flush=True)
            finally:
                context.close()
                browser.close()


if __name__ == '__main__':
    main()
