#!/usr/bin/env python3
"""Operator real Agy test (`run_validation --real-only` or direct): native approvals/questions and commands.

Private HOME, temporary files, loopback synthetic model, no account or paid
requests. Reuse the real-browser lifecycle/isolation harness; all answers use
Chromium controls or its native terminal keyboard.
"""
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import re
import threading
from urllib.parse import urlsplit

from playwright.sync_api import expect
import agy_real_browser as harness
from agy_tools_real_browser import MODEL, WIRE_MODEL


@contextmanager
def gateway(root, holds):
    settings = root / 'home/.antigravity/settings.json'
    settings.parent.mkdir(mode=0o700)
    settings.write_text(json.dumps({'models': {'list': [{'modelId': MODEL,
        'displayName': 'Synthetic loopback fixture', 'toolFormatterType': 'TOOL_FORMATTER_TYPE_NONE'}]}}))
    records, errors, emitted = [], [], set()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_CONNECT(self):
            self.send_error(403, 'external network forbidden')

        def do_GET(self):
            self.send_error(403, 'catalog explicitly configured')

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            records.append(request)
            (root / f'gateway-{len(records)}.json').write_text(json.dumps(request, indent=2))
            if request.get('model') != WIRE_MODEL or self.path != '/v1/chat/completions':
                errors.append('unexpected model/destination')
                self.send_error(403)
                return
            main = any('<identity>' in str(m.get('content', '')) for m in request['messages'])
            matches = [text for m in request['messages'] if m['role'] == 'user'
                       for text in re.findall(r'<USER_REQUEST>\s*([\s\S]*?)\s*</USER_REQUEST>', str(m.get('content', '')))]
            marker = matches[-1] if matches else ''
            delta, finish = {'role': 'assistant', 'content': 'DONE ' + marker if main else 'Fixture'}, 'stop'
            if main and marker.startswith('fixture-') and marker not in emitted:
                emitted.add(marker)
                args = {'toolSummary': 'Temporary fixture', 'toolAction': 'Testing native interaction'}
                if marker in ('fixture-questions', 'fixture-writein'):
                    name = 'ask_question'
                    args['questions'] = [{'question': 'Choose a color', 'options': ['Red', 'Blue'], 'is_multi_select': False}]
                    if marker == 'fixture-questions':
                        args['questions'].append({'question': 'Choose features',
                            'options': ['Logs', 'Metrics', 'Tracing'], 'is_multi_select': True})
                elif marker == 'fixture-file':
                    name = 'write_to_file'
                    args.update(TargetFile=str(root / 'work/safe.txt'), Overwrite=False,
                                CodeContent='PRIVATE_FIXTURE\n', Description='Temporary fixture')
                else:
                    name = 'run_command'
                    prefix = 'echo' if 'session-allow' in marker else '/usr/bin/printf' if 'persist-allow' in marker else 'printf'
                    args.update(CommandLine=prefix + ' ' + marker, Cwd=str(root / 'work'), WaitMsBeforeAsync=1000)
                assert name in [t['function']['name'] for t in request['tools']]
                delta = {'role': 'assistant', 'tool_calls': [{'index': 0, 'id': marker,
                    'type': 'function', 'function': {'name': name, 'arguments': json.dumps(args)}}]}
                finish = 'tool_calls'
            print('GATEWAY', request['model'], marker, finish, flush=True)
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            for part, reason in ((delta, None), ({}, finish)):
                self.wfile.write(('data: ' + json.dumps({'id': 'local-fixture', 'object': 'chat.completion.chunk',
                    'model': WIRE_MODEL, 'choices': [{'index': 0, 'delta': part, 'finish_reason': reason}]}) + '\n\n').encode())
            self.wfile.write(b'data: [DONE]\n\n')

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}', records, errors
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def run(browser, base, root, agy, records, evidence, holds):
    context = browser.new_context(viewport={'width': 1280, 'height': 1000}, service_workers='block')
    context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(20000)
    errors = []
    page.on('pageerror', lambda e: errors.append(str(e)))
    card = page.locator('#composer-question')

    def capture(label):
        # Read the actual host frame, including styles; no production host.
        import subprocess
        result = subprocess.run([str(harness.REPO / 'target/debug/ptyhost'), '--dir', str(root / 'host'),
                                 'capture', name], capture_output=True, timeout=5, check=True)
        (root / (label + '.capture')).write_bytes(result.stdout)

    def option(label):
        card.locator('[data-question-option]').filter(has_text=label).click()

    def prompt(text):
        expect(card).to_contain_text(text)
        expect(page.locator('#csend')).to_be_disabled()
        checked = page.request.post(base + '/api/session/conversation/check', data={
            'uid': page.evaluate('S.sel'), '_build': page.evaluate('BUILD_ID')})
        assert checked.status == 409 and checked.json()['code'] == 'cli_question', checked.text()

    try:
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded && T.sources.agy === true')
        harness.open_picker(page)
        page.locator('label:has(input[name="new-source"][value="agy"])').click()
        # The picker lists reasoning variants as one model plus an effort.
        page.locator('#new-model').click()
        page.locator('#new-model-options [data-model-option][title="gemini-3.1-pro"]').click()
        page.locator('#new-effort').select_option('low')
        page.locator('#new-cwd').fill(str(root / 'work'))
        with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/create') as created:
            page.locator('#new-session-go').click()
        name = created.value.json()['name']
        prompt('Do you trust')
        option('Yes, I trust')
        harness.ready(page)

        harness.send(page, 'hello')
        page.wait_for_function("S.sel?.startsWith('agy:')")
        harness.history(page, 'DONE hello')
        for marker, choice in [('fixture-once', 'Yes, run command'), ('fixture-deny', 'No, cancel'),
                               ('fixture-cancel', None), ('fixture-amend', 'amend')]:
            harness.send(page, marker)
            prompt('Run this command?')
            capture(marker)
            page.locator('#cinput').fill('Keep this draft')
            if choice == 'amend':
                card.locator('[data-question-action]').filter(has_text='Amend').click()
                harness.terminal(page)
                harness.wait_screen(page, 'Yes, and tell Antigravity CLI what to do next')
                capture('amend-text')
                # This is an approval plus instruction, not command editing.
                harness.terminal(page, 'Continue with the fixture', key='Enter')
            elif choice:
                option(choice)
            else:
                card.locator('.question-cancel').click()
            harness.ready(page)
            expect(page.locator('#cinput')).to_have_value('Keep this draft')
            page.locator('#cinput').fill('')
            evidence.pass_(marker + ': native menu answer/cancel, CHECK blocks ordinary SEND and retains draft')

        harness.send(page, 'fixture-file')
        prompt('Allow creation of this file?')
        capture('file-permission')
        option('Yes, allow creation')
        harness.history(page, 'DONE fixture-file')
        assert (root / 'work/safe.txt').read_text() == 'PRIVATE_FIXTURE\n'
        evidence.pass_('native create-file approval writes only the temporary fixture')

        harness.send(page, 'fixture-questions')
        prompt('Choose a color')
        capture('question-single')
        option('Blue')
        prompt('Choose features')
        capture('question-multi')
        option('Logs')
        expect(card.locator('[data-question-option]').filter(has_text='Logs')).to_have_class(re.compile('selected'))
        option('Metrics')
        card.locator('[data-question-action]').filter(has_text='Previous question').click()
        prompt('Choose a color')
        option('Blue')
        prompt('Choose features')
        card.locator('[data-question-action]').filter(has_text='Submit All').click()
        harness.history(page, 'DONE fixture-questions')
        assert any('Logs, Metrics' in str(r['messages']) for r in records)
        evidence.pass_('native single/multiple questions, toggle, previous page and submit all reach model')

        harness.send(page, 'fixture-writein')
        prompt('Choose a color')
        option('Write-in...')
        expect(card.locator('.question-text-input')).to_be_visible()
        capture('writein')
        card.locator('.question-text-input').fill('Custom answer')
        card.locator('.question-text-form .question-submit').click()
        harness.history(page, 'DONE fixture-writein')
        evidence.pass_('native write-in subpage and submission')

        for scope, label in [('session-allow', 'in this conversation'), ('persist-allow', 'Persist to settings.json')]:
            marker = 'fixture-' + scope
            harness.send(page, marker)
            prompt('Run this command?')
            capture(scope)
            option(label)
            harness.history(page, 'DONE ' + marker)
            harness.send(page, marker + '-again')
            harness.history(page, 'DONE ' + marker + '-again')
            evidence.pass_(scope + ': explicit native scope choice permits the next harmless command')
        persisted = root / 'home/.gemini/antigravity-cli/settings.json'
        assert '/usr/bin/printf' in persisted.read_text(), persisted
        assert 'echo' not in persisted.read_text(), 'Conversation-only permission must not persist'

        native = root / 'home/.gemini/antigravity-cli/brain'
        transcript = next(native.glob('*/.system_generated/logs/transcript_full.jsonl'))
        for command, title in [('/model', 'Switch Model'), ('/permissions', 'Permission Config Editor'),
                               ('/resume', 'Conversations'), ('/help', 'Quick Reference'), ('/settings', 'Settings')]:
            before = transcript.read_bytes()
            harness.send(page, command)
            harness.terminal(page)
            harness.wait_screen(page, title)
            capture(command[1:])
            assert transcript.read_bytes() == before, command
            harness.terminal(page, key='Escape')
            harness.ready(page)
            checked = page.request.post(base + '/api/session/conversation/check', data={
                'uid': page.evaluate('S.sel'), '_build': page.evaluate('BUILD_ID')}).json()
            assert checked['cli']['queued'] == [], (command, checked)
            evidence.pass_(command + ': native menu opens/closes without phantom echo queue')
        assert records and all(r['model'] == WIRE_MODEL for r in records)
        assert not errors, errors
    except Exception as error:
        evidence.fail('interactions', error, page)
    finally:
        context.close()


if __name__ == '__main__':
    harness.MODELS = (MODEL,)
    harness.gateway = gateway
    harness.run = run
    harness.main()
