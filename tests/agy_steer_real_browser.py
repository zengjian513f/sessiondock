#!/usr/bin/env python3
"""Operator-only real Agy steering via Chromium; private HOME and loopback model.

No credentials, paid models, everyday settings, or production sessions. Verifies
that real Agy 1.5.x SendUserCascadeMessage (NEXT_INVOCATION) steers an active
stream-json turn via Ctrl+Enter and queue_steer without restarting the runtime
or losing the explicit CLI --model selection.
"""
from contextlib import ExitStack, contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import argparse
import json
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import threading
from urllib.parse import quote

from playwright.sync_api import expect, sync_playwright
from agy_real_browser import installed_agy
from agy_sdk_browser import idle, send, watch
from agy_tools_real_browser import MODEL, WIRE_MODEL
from claude_sdk_browser import open_sdk
from history_fixtures import BINARY, REPO, Corpus, isolated_server
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize


@contextmanager
def gateway(root: Path):
    state = {'sid': None, 'stage': 0, 'errors': [], 'models': [], 'seen_user_texts': []}
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_): pass
        def do_CONNECT(self): self.send_error(403)
        def do_GET(self): self.send_error(403)
        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            messages = request.get('messages', [])
            system = str([m for m in messages if m['role'] == 'system'])
            match = re.search(r'Conversation ID: ([a-f0-9-]+)', system)
            sid = match[1] if match else None
            tools = {t['function']['name']: t['function'] for t in request.get('tools', [])}
            delta, finish = {'role': 'assistant', 'content': 'PRIVATE_STEER_TITLE'}, 'stop'
            if sid and sid == state['sid']:
                state['models'].append(request.get('model'))
                if request.get('model') not in (MODEL, WIRE_MODEL):
                    state['errors'].append(f"unexpected wire model: {request.get('model')}")
                    self.send_error(403)
                    return
                user_blob = '\n'.join(str(m.get('content', '')) for m in messages if m.get('role') == 'user')
                state['seen_user_texts'].append(user_blob)
                stage = state['stage']
                state['stage'] += 1
                if stage == 0:
                    assert 'run_command' in tools, list(tools)
                    args = {
                        'CommandLine': 'python3 ' + shlex.quote(str(root / 'work/hold_tool.py')),
                        'WaitMsBeforeAsync': 10000,
                        'toolSummary': 'Hold active tool for real steer test',
                    }
                    delta = {'role': 'assistant', 'tool_calls': [{'index': 0, 'id': 'steer_tool_0',
                        'type': 'function', 'function': {'name': 'run_command', 'arguments': json.dumps(args)}}]}
                    finish = 'tool_calls'
                else:
                    delta = {'role': 'assistant', 'content': 'REAL_AGY_STEERED_CONTINUATION_COMPLETE'}
            try:
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.end_headers()
                for part, reason in ((delta, None), ({}, finish)):
                    chunk = {'id': 'private-steer-fixture', 'object': 'chat.completion.chunk', 'model': request.get('model', WIRE_MODEL),
                        'choices': [{'index': 0, 'delta': part, 'finish_reason': reason}]}
                    self.wfile.write(('data: ' + json.dumps(chunk) + '\n\n').encode())
                self.wfile.write(b'data: [DONE]\n\n')
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}', state
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--agy', type=Path, default=installed_agy())
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-steer-real-') as tmp:
        root = Path(tmp).resolve()
        for name in ('home', 'work', 'state', 'host', 'ledger', 'mirror', 'claude', 'codex', 'grok', 'config', 'data', 'cache', 'tmp', 'proc'):
            (root / name).mkdir(mode=0o700)
        native = root / 'home/.gemini/antigravity-cli'
        onboarding = native / 'cache/onboarding.json'
        onboarding.parent.mkdir(parents=True)
        onboarding.write_text(json.dumps({'consumerOnboardingComplete': True, 'enterpriseOnboardingComplete': True, 'onboardingComplete': True}))
        settings = root / 'home/.antigravity/settings.json'
        settings.parent.mkdir()
        settings.write_text(json.dumps({
            'models': {
                'list': [
                    {'modelId': MODEL, 'displayName': 'Private fixture', 'toolFormatterType': 'TOOL_FORMATTER_TYPE_NONE'},
                ],
            },
        }))
        (root / 'work/hold_tool.py').write_text(
            "import time\nfrom pathlib import Path\np = Path(__file__).parent\n"
            "(p / 'tool-running').write_text('1')\nend = time.monotonic() + 30\n"
            "while time.monotonic() < end and not (p / 'release-tool').exists():\n    time.sleep(0.03)\n"
            "print('tool_released_ok')\n"
        )
        with ExitStack() as cleanup, gateway(root) as (url, state):
            cleanup.callback(stop_private_sdkhosts, root)
            cleanup.callback(lambda: (root / 'work/release-tool').touch())
            env = {
                'HOME': str(root / 'home'), 'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8', 'TERM': 'dumb',
                'AGY_CLI_DISABLE_AUTO_UPDATE': '1', 'AGY_LLM_GATEWAY_URL': url + '/v1',
                'AGY_LLM_GATEWAY_MODELS': MODEL,
                'AGY_LLM_GATEWAY_API_KEY': 'synthetic-local-fixture',
                'AGY_LLM_GATEWAY_WIRE_PROTOCOL': 'openai',
                'HTTP_PROXY': url, 'HTTPS_PROXY': url, 'ALL_PROXY': url, 'NO_PROXY': '127.0.0.1,localhost,::1',
            }
            for key, folder in [('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'), ('XDG_CACHE_HOME', 'cache'), ('XDG_STATE_HOME', 'state'), ('TMPDIR', 'tmp')]:
                env[key] = str(root / folder)
            argv = [str(args.agy), '--model', MODEL, '--input-format', 'stream-json', '--output-format', 'stream-json']
            seed = subprocess.run(argv, input='', capture_output=True, text=True, cwd=root / 'work', env=env, timeout=20)
            assert seed.returncode == 0, seed.stderr
            init = json.loads(seed.stdout.splitlines()[0])
            sid = init['conversation_id']
            state['sid'] = sid
            assert init['init']['model'] == MODEL
            launcher = root / 'launcher.json'
            launcher.write_text(json.dumps({
                'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
                'host_dir': str(root / 'host'), 'adapters': [],
                'profiles': [{'id': 'agy-steer-real', 'source': 'agy', 'executable': str(args.agy),
                    'args': ['--model', MODEL, '--dangerously-skip-permissions'], 'env': env}],
            }))
            initialize('--initialize-lifecycle', root / 'ledger', args.binary)
            extra = {'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_AGY_HOME': str(native), 'SESSIONDOCK_AGY_ROOT': str(root / 'mirror')}
            with isolated_server(Corpus(root), args.binary, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                    state_dir=root / 'state', launcher_config=launcher, extra_env=extra) as (base, _), sync_playwright() as pw:
                browser = pw.chromium.launch(headless=True)
                page = browser.new_page(viewport={'width': 1280, 'height': 900})
                page.set_default_timeout(25000)
                try:
                    page.goto(base, wait_until='domcontentloaded')
                    page.wait_for_function('sid => S.sessions.some(r => r.sid === sid)', arg=sid)
                    uid = page.evaluate('sid => S.sessions.find(r => r.sid === sid).uid', sid)
                    open_sdk(page, None, expect, uid=uid)
                    watch(page, uid)
                    send(page, 'Start real Agy steer turn')
                    page.wait_for_function("() => NativeSdkView.running")
                    deadline = 0
                    while not (root / 'work/tool-running').exists():
                        page.wait_for_timeout(50)
                        deadline += 50
                        assert deadline < 15000, 'real Agy tool never started'
                    hosts = list((root / 'state/sdkhost').glob('*/status.json'))
                    pid = json.loads(hosts[0].read_text())['pid']

                    # Ctrl hold switches #sdk-send from 排队 to 引导; Ctrl+Enter steers immediately.
                    expect(page.locator('#sdk-send')).to_have_text('排队')
                    page.keyboard.down('Control')
                    expect(page.locator('#sdk-send')).to_have_text('引导')
                    page.keyboard.up('Control')
                    page.locator('#sdk-input').fill('Direct real Ctrl+Enter steer')
                    with page.expect_response(lambda r: r.url.endswith('/api/native-sdk/steer')) as direct_resp:
                        page.locator('#sdk-input').press('Control+Enter')
                    assert direct_resp.value.ok, direct_resp.value.text()
                    expect(page.locator('#sdk-input')).to_have_value('')

                    # Queue a message then promote it via 立即引导 (queue_steer).
                    send(page, 'Queued real steer via promote')
                    queued = page.locator('.sdk-queued').filter(has_text='Queued real steer via promote')
                    expect(queued).to_be_visible()
                    with page.expect_response(lambda r: r.url.endswith('/api/native-sdk/queue_steer')) as promote_resp:
                        queued.get_by_role('button', name='立即引导', exact=True).click()
                    assert promote_resp.value.ok, promote_resp.value.text()
                    expect(queued).to_have_count(0)

                    # Release active tool so Agy enters next invocation within the SAME turn.
                    (root / 'work/release-tool').touch()
                    expect(page.locator('.sdk-live')).to_contain_text('REAL_AGY_STEERED_CONTINUATION_COMPLETE')
                    idle(page)
                    assert page.evaluate('NativeSdkView.raw.last_turn.status') == 'completed'
                    assert json.loads(hosts[0].read_text())['pid'] == pid
                    assert not state['errors'], state['errors']
                    assert state['models'] and set(state['models']) <= {MODEL, WIRE_MODEL}, state['models']
                    assert any('Direct real Ctrl+Enter steer' in blob and 'Queued real steer via promote' in blob for blob in state['seen_user_texts']), state['seen_user_texts']

                    # Reload and verify history/live native_id deduplication and /api/messages native transcript order.
                    page.reload(wait_until='networkidle')
                    open_sdk(page, None, expect, uid=uid)
                    expect(page.locator('.native-sdk-pane .msgs').get_by_text('Direct real Ctrl+Enter steer', exact=True)).to_have_count(1)
                    expect(page.locator('.native-sdk-pane .msgs').get_by_text('Queued real steer via promote', exact=True)).to_have_count(1)
                    msgs_text = page.locator('.native-sdk-pane .msgs').inner_text()
                    assert msgs_text.index('Start real Agy steer turn') < msgs_text.index('Direct real Ctrl+Enter steer') < msgs_text.index('Queued real steer via promote') < msgs_text.index('REAL_AGY_STEERED_CONTINUATION_COMPLETE')
                    api_msgs = page.request.get(f"{base}/api/messages/{quote(uid, safe='')}").json()
                    raw_history = json.dumps(api_msgs.get('messages', []), ensure_ascii=False)
                    assert raw_history.index('Start real Agy steer turn') < raw_history.index('Direct real Ctrl+Enter steer') < raw_history.index('Queued real steer via promote') < raw_history.index('REAL_AGY_STEERED_CONTINUATION_COMPLETE')
                    page.request.post(base + '/api/native-sdk/stop', data={'uid': uid})
                    print('PASS real Agy steer: Ctrl+Enter + queue_steer continued in same turn, same PID/SID, explicit --model preserved', flush=True)
                finally:
                    browser.close()


if __name__ == '__main__':
    main()
