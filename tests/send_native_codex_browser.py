#!/usr/bin/env python3
"""Conversation SEND against a resumed native Codex session, using a private fake CLI."""
from browser_runtime import js
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import time
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright
from history_parity import REPO, BINARY, Corpus, codex_message, codex_row, isolated_server
from send_browser import initialize, xterm_includes
from hub_send_browser import cleanup_hosts
from popups import on_popup  # noqa: E402

CODEX_SID = "6a7b8c9d-0e1f-4a2b-9c3d-4e5f6a7b8c9d"


def resume_codex(page, uid):
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.wait_for_function(js("uid => S.sel === uid", 'uid => runtime.core.state.selection.sel === uid'), arg=uid, timeout=20000)
    expect(page.locator("#msgs")).to_contain_text("Synthetic codex prompt")
    with page.expect_response(lambda response: urlsplit(response.url).path == "/api/term/takeover") as taken:
        page.locator("#a-term").click()
    resumed = taken.value.json()
    assert taken.value.status == 200 and resumed["launch_kind"] == "resume", resumed
    expect(page.locator("#termpane")).to_be_visible()
    page.wait_for_function(js("T.ws?.readyState === WebSocket.OPEN", 'runtime.terminal.state.ws?.readyState === WebSocket.OPEN'))
    xterm_includes(page, "FAKE_CODEX_TUI sid=[%s]" % CODEX_SID)
    page.wait_for_function(js("uid => (T.list || []).some(row => row.uid === uid && row.instance_id)", 'uid => (runtime.terminal.state.list || []).some(row => row.uid === uid && row.instance_id)'), arg=uid, timeout=15000)
    return resumed


def user_records(path):
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    return [row["payload"] for row in rows
            if row.get("type") == "response_item" and row["payload"].get("type") == "message"
            and row["payload"].get("role") == "user"]


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-send-') as temporary:
        root = Path(temporary).resolve()
        for name in ('host', 'work', 'ledger', 'delivery', 'state', 'home', 'claude', 'codex', 'grok'):
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        rollout = corpus.put(CODEX_SID, 'codex', [
            codex_row('session_meta', {'id': CODEX_SID, 'cwd': str(root / 'work')}),
            codex_message('user', 'Synthetic codex prompt')], [])
        uid = corpus.uid(CODEX_SID)
        # Load historical command sends, plus escaped input whose trimmed
        # text looks like a command. Receipt digests prove the distinction.
        conversations = root / 'state' / 'conversations'
        conversations.mkdir(mode=0o700)
        ledger = conversations / 'conversation-ledger.json'
        legacy_rows = [{'request_id': request_id, 'text': text,
                        'echo_hash': hashlib.sha256(json.dumps(text).encode()).hexdigest(),
                        'sent_at': time.time(), 'state': state, 'cli_queued_at': None}
                       for request_id, text, state in (
                           ('old-model', '/model', 'queued'),
                           ('old-lost-model', '/model', 'lost'),
                           ('old-status', '/status', 'queued'),
                           ('old-rename', '/rename New title', 'queued'),
                           ('old-plan', '/plan Original task', 'queued'),
                           ('escaped-status', '/status', 'lost'),
                           ('ordinary-model-text', '/model is mentioned here', 'lost'))]
        legacy_key = 'launch:legacy-model-menu'
        requests = {}
        for row in legacy_rows:
            original = ' /status' if row['request_id'] == 'escaped-status' else row['text']
            payload = {'text': original, 'attachments': [], 'quotes': []}
            # serde_json preserves insertion order for submission fingerprints.
            digest = hashlib.sha256(json.dumps(payload, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
            requests[legacy_key + '\0' + row['request_id']] = {'key': legacy_key, 'id': row['request_id'],
                'payload': digest, 'attachments': [], 'phase': 'sent', 'result': {'ok': True, 'state': 'sent'}}
        ledger.write_text(json.dumps({'aliases': {uid: legacy_key}, 'queued': {legacy_key: legacy_rows}, 'requests': requests}))
        launcher = root / 'launcher.json'
        launcher.touch(mode=0o600)
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'codex-cli-v1', 'source': 'codex',
                'executable': str(Path(sys.executable).resolve()),
                'args': [str(REPO / 'tests/fake_codex_cli.py'), '--model', 'gpt-5.6-luna', '--reply'],
                'new_args': [], 'resume_args': ['resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                    'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                    'SESSIONDOCK_TEST_QUOTED_READY': '1',
                    'SESSIONDOCK_TEST_QUEUE_FILE': str(root / 'queue'),
                    'SESSIONDOCK_TEST_BUSY_WARNING': '1',
                    'SESSIONDOCK_TEST_QUEUE_INTERRUPT': '1',
                    'SESSIONDOCK_TEST_COMMAND_DISPATCH': '1',
                    'SESSIONDOCK_TEST_COMMAND_LOG': str(root / 'command-log'),
                    'SESSIONDOCK_TEST_CODEX_ROOT': str(root / 'codex')}}]}))
        initialize('--initialize-lifecycle', root / 'ledger')
        try:
            with isolated_server(corpus, BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                    launcher_config=launcher, state_dir=root / 'state',
                    file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), sync_playwright() as pw:
                options = {'headless': True}
                if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                    options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
                browser = pw.chromium.launch(**options)
                context = browser.new_context(service_workers='block')
                context.route('**/*', lambda route: route.continue_()
                    if route.request.url.startswith(base + '/') else route.abort())
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                on_popup(page, lambda dialog: (errors.append(dialog.message), dialog.accept()))
                page.goto(base, wait_until='domcontentloaded')
                resume_codex(page, uid)
                xterm_includes(page, 'Context 32% used · Ready')
                xterm_includes(page, 'Context 27% used · Working')
                page.locator('#a-term').click()
                page.wait_for_function(js("() => composerDraft()?.inputStatus?.state === 'ready'", "() => runtime.composer.composerDraft()?.inputStatus?.state === 'ready'"))
                build = context.request.get(base + '/api/meta').json()['build']
                checked = context.request.post(base + '/api/session/conversation/check', data={'uid': uid, '_build': build}).json()
                survivors = ['escaped-status', 'ordinary-model-text']
                assert [r['request_id'] for r in checked['cli']['queued']] == survivors, (checked, json.loads(ledger.read_text()))
                persisted = json.loads(ledger.read_text())
                assert [r['request_id'] for rows in persisted['queued'].values() for r in rows] == survivors, persisted['queued']
                for request_id in survivors:
                    dismissed = context.request.post(base + '/api/session/conversation/queued/dismiss',
                        data={'uid': uid, 'request_id': request_id, '_build': build})
                    assert dismissed.status == 200, dismissed.text()
                # Exercise the actual composer -> SEND -> local CLI menu path.
                before_model = rollout.read_bytes()
                page.locator('#cinput').fill('/model  ')
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as sent:
                    page.locator('#csend').click()
                assert sent.value.status == 200 and sent.value.json()['state'] == 'sent', sent.value.text()
                model_request = sent.value.request.post_data_json
                expect(page.locator('#cinput')).to_have_value('')
                expect(page.locator('#queued-sends .queued-send')).to_have_count(0)
                page.locator('#a-term').click()
                xterm_includes(page, 'Select Model and Effort')
                page.reload(wait_until='domcontentloaded')
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.wait_for_function(js("() => composerDraft()?.inputStatus?.state === 'blocked'", "() => runtime.composer.composerDraft()?.inputStatus?.state === 'blocked'"))
                checked = context.request.post(base + '/api/session/conversation/check', data={'uid': uid, '_build': build}).json()
                assert checked['cli']['queued'] == [] and checked['input']['state'] == 'blocked', checked
                expect(page.locator('#queued-sends .queued-send')).to_have_count(0)
                assert rollout.read_bytes() == before_model, 'Local /model must not create native history'
                replay = context.request.post(base + '/api/session/conversation/send', data=model_request)
                assert replay.status == 200 and replay.json()['state'] == 'sent', replay.text()
                # Reload preserves the selected terminal/conversation mode.
                if not page.locator('#termpane').is_visible():
                    page.locator('#a-term').click()
                page.wait_for_function(js('() => T.ws?.readyState === WebSocket.OPEN', '() => runtime.terminal.state.ws?.readyState === WebSocket.OPEN'))
                xterm_includes(page, 'Select Model and Effort')
                page.locator('#termpane .xterm-helper-textarea').press('Escape')
                page.wait_for_function(js("() => composerDraft()?.inputStatus?.state === 'ready'", "() => runtime.composer.composerDraft()?.inputStatus?.state === 'ready'"))
                page.locator('#a-term').click()
                # Audit every known built-in and each opted-in argument form
                # by typing and submitting through the real composer.
                manifest = json.loads((REPO / 'tests/fixtures/codex_send_commands.json').read_text())
                commands = ['/' + row['name'] for row in manifest['commands'] if row['name'] != 'model']
                commands += ['/' + row['name'] + ' argument text' for row in manifest['commands'] if row['inline_args']]
                commands += ['/fast', '/goooal Task', '/model\nAdditional text']
                for command in commands:
                    page.wait_for_function(js("() => composerDraft()?.inputStatus?.state === 'ready'", "() => runtime.composer.composerDraft()?.inputStatus?.state === 'ready'"))
                    page.locator('#cinput').fill(command)
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as sent:
                        page.locator('#csend').click()
                    assert sent.value.status == 200, (command, sent.value.text())
                    expect(page.locator('#cinput')).to_have_value('')
                    checked = context.request.post(base + '/api/session/conversation/check', data={'uid': uid, '_build': build}).json()
                    assert checked['cli']['queued'] == [], (command, checked)
                    expect(page.locator('#queued-sends .queued-send')).to_have_count(0)
                assert any(r['content'][0]['text'] == 'argument text' for r in user_records(rollout)), 'Inline /plan must submit transformed input'
                dispatched = [json.loads(line)['text'] for line in (root / 'command-log').read_text().splitlines()]
                assert dispatched == [command for command in commands
                    if command != '/status' and not command.startswith('/rename ')], dispatched
                print(f'PASS Codex command sends: {len(commands) + 1} bare/inline/alias/multiline forms', flush=True)
                native_count = len(user_records(rollout))
                for number in range(2):
                    page.wait_for_function(js("uid => composerUid === uid && composerDraft()?.inputStatus?.state === 'ready'", "uid => runtime.composer.composerUid === uid && runtime.composer.composerDraft()?.inputStatus?.state === 'ready'"), arg=uid)
                    text = f'native conversation send {number}'
                    page.locator('#cinput').fill(text)
                    page.evaluate(js('async () => await composerDraftWrites', 'async () => await runtime.composer.composerDraftWrites'))
                    draft = context.request.get(base + '/api/session/conversation', params={'uid': uid}).json()['draft']
                    assert draft['value']['session']['cwd'] == str(root / 'work'), draft
                    started = time.monotonic()
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as sent:
                        page.locator('#csend').click()
                    assert sent.value.status == 200, sent.value.text()
                    body = sent.value.request.post_data_json
                    assert body['uid'] == uid and sent.value.json()['state'] == 'sent', body
                    page.wait_for_function(js('() => !composerSending', '() => !runtime.composer.composerSending'))
                    expect(page.locator('#cinput')).to_have_value('')
                    print(f'native Codex click-to-clear: {(time.monotonic() - started)*1000:.0f} ms', flush=True)
                    page.locator('#a-term').click()
                    xterm_includes(page, '> ' + text)
                    page.locator('#a-term').click()
                    users = [r['content'][0]['text'] for r in user_records(rollout)]
                    assert users.count(text) == 1, users
                    # Replaying a completed request only reads its receipt.
                    replay = context.request.post(base + '/api/session/conversation/send', data=body)
                    assert replay.status == 200 and replay.json()['state'] == 'sent', replay.text()
                    assert len(user_records(rollout)) == native_count + number + 1
                # Codex's busy queue is TUI-only: no native record until released.
                queue_file = root / 'queue'
                # Non-inline command arguments and paths are ordinary input.
                # Hold their native echoes so an incorrect classifier cannot
                # be hidden by an immediately retired queued row.
                literal_inputs = ['/model is mentioned here', '/tmp/file', ' /model']
                for text in literal_inputs:
                    queue_file.write_text('quoted')
                    page.wait_for_function(js("() => composerDraft()?.inputStatus?.state === 'ready'", "() => runtime.composer.composerDraft()?.inputStatus?.state === 'ready'"))
                    page.locator('#cinput').fill(text)
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as sent:
                        page.locator('#csend').click()
                    assert sent.value.status == 200, sent.value.text()
                    expect(page.locator('#queued-sends .queued-send')).to_have_count(1)
                    checked = context.request.post(base + '/api/session/conversation/check', data={'uid': uid, '_build': build}).json()
                    assert checked['cli']['queued'][0]['text'] == text, checked
                    queue_file.unlink()
                    expect(page.locator('#queued-sends .queued-send')).to_have_count(0, timeout=15000)
                    assert [r['content'][0]['text'] for r in user_records(rollout)].count(text.strip()) == 1
                native_count += len(literal_inputs)
                queue_file.write_text('quoted')
                queued_text = ' /status\n第二行保留完整正文'
                for _ in range(2):
                    page.wait_for_function(js("() => composerDraft()?.inputStatus?.state === 'ready'", "() => runtime.composer.composerDraft()?.inputStatus?.state === 'ready'"))
                    page.locator('#cinput').fill(queued_text)
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as sent:
                        page.locator('#csend').click()
                    assert sent.value.status == 200, sent.value.text()
                    expect(page.locator('#cinput')).to_have_value('')
                bubbles = page.locator('#queued-sends .queued-send')
                expect(bubbles).to_have_count(2)
                checked = context.request.post(base + '/api/session/conversation/check', data={'uid': uid, '_build': context.request.get(base + '/api/meta').json()['build']}).json()
                assert all(r['cli_queued_at'] is None for r in checked['cli']['queued']), checked
                assert len(user_records(rollout)) == native_count + 2
                queue_file.write_text('one')
                expect(page.locator('#queued-sends [data-cli-queued="1"]')).to_have_count(1, timeout=15000)
                for _ in range(3):
                    checked = context.request.post(base + '/api/session/conversation/check', data={'uid': uid, '_build': context.request.get(base + '/api/meta').json()['build']}).json()
                    assert sum(r['cli_queued_at'] is not None for r in checked['cli']['queued']) == 1, checked
                queue_file.write_text('all')
                expect(page.locator('#queued-sends [data-cli-queued="1"]')).to_have_count(2, timeout=15000)
                expect(bubbles.first.locator('.queued-send-state')).to_have_text('已进入 CLI 队列，当前步骤结束后处理')
                # Persisted evidence survives reopening the page; the fake CLI
                # still has not written either queued prompt into JSONL.
                page.reload(wait_until='domcontentloaded')
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                expect(page.locator('#queued-sends [data-cli-queued="1"]')).to_have_count(2, timeout=15000)
                assert len(user_records(rollout)) == native_count + 2
                queue_file.unlink()
                expect(page.locator('#queued-sends .queued-send')).to_have_count(0, timeout=15000)
                assert [r['content'][0]['text'] for r in user_records(rollout)].count(queued_text.strip()) == 2
                # BUG-20260930-172002-bc4cf4: Esc consumes a TUI-only
                # queue as a steer, then Esc aborts before any native echo.
                queue_file.write_text('all')
                text = 'queued steer interrupted before native user record'
                page.locator('#cinput').fill(text)
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as sent:
                    page.locator('#csend').click()
                assert sent.value.status == 200, sent.value.text()
                submission = sent.value.request.post_data_json
                bubble = page.locator('#queued-sends .queued-send').filter(has_text=text)
                expect(bubble).to_have_attribute('data-cli-queued', '1', timeout=15000)
                # An idle screen without the queue is insufficient evidence
                # when no native abort followed this send.
                queue_file.write_text('idle-hidden')
                time.sleep(2)
                checked = context.request.post(base + '/api/session/conversation/check', data={'uid': uid, '_build': build}).json()
                assert checked['cli']['instance']['busy'] is False, checked
                assert checked['cli']['queued'][0]['state'] == 'queued', checked
                queue_file.write_text('all')
                page.locator('#cesc').click()
                # The first abort starts the steer: a working turn with an
                # off-screen queue must still retain its confirmation.
                time.sleep(2)
                expect(bubble).to_have_attribute('data-state', 'queued')
                page.locator('#cesc').click()
                expect(bubble).to_have_attribute('data-state', 'interrupted', timeout=15000)
                expect(bubble.locator('.queued-send-state')).to_contain_text('CLI 已中断，未确认处理')
                assert text not in [r['content'][0]['text'] for r in user_records(rollout)]
                # Persisted status survives reload and does not resend.
                page.reload(wait_until='domcontentloaded')
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                expect(bubble).to_have_attribute('data-state', 'interrupted', timeout=15000)
                replay = context.request.post(base + '/api/session/conversation/send', data=submission)
                assert replay.status == 200, replay.text()
                expect(bubble).to_have_attribute('data-state', 'interrupted')
                persisted = json.loads(ledger.read_text())
                assert any(r['state'] == 'interrupted' and r['text'] == text
                           for rows in persisted['queued'].values() for r in rows)
                # A send after the abort is a new queue, not an interrupted
                # receipt. A later echo also overrides interrupted evidence.
                page.locator('#cinput').fill('new input after interrupt')
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as sent:
                    page.locator('#csend').click()
                assert sent.value.status == 200, sent.value.text()
                new_bubble = page.locator('#queued-sends .queued-send').filter(has_text='new input after interrupt')
                expect(new_bubble).to_have_attribute('data-cli-queued', '1', timeout=15000)
                expect(new_bubble).to_have_attribute('data-state', 'queued')
                queue_file.write_text('idle-hidden')
                time.sleep(2)
                checked = context.request.post(base + '/api/session/conversation/check', data={'uid': uid, '_build': build}).json()
                assert checked['cli']['instance']['busy'] is False, checked
                assert next(r for r in checked['cli']['queued'] if r['text'] == 'new input after interrupt')['state'] == 'queued', checked
                queue_file.write_text('all')
                late_echo = codex_message('user', text)
                late_echo['timestamp'] = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())
                with rollout.open('a') as stream:
                    stream.write(json.dumps(late_echo) + '\n')
                expect(bubble).to_have_count(0, timeout=15000)
                queue_file.unlink()
                expect(new_bubble).to_have_count(0, timeout=15000)
                assert not errors, errors
                context.close()
                browser.close()
        finally:
            cleanup_hosts(root)
    print('PASS native Codex browser: built-in commands, resumed identity, TUI queues, duplicate/wrapped sends, native retirement, interrupted steer without echo, idle/old-abort negatives, persisted status, replay without resend, late echo')


if __name__ == '__main__':
    main()
