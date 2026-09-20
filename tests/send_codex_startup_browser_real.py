#!/usr/bin/env python3
"""Cold-start browser SEND with real Codex, isolated home and Luna low only.

Operator-only (*_real): run directly or with run_validation --include-real.
Requires the existing controlled login; never changes everyday defaults.
Checks native user text and turn model/effort, not only the PTY write ACK.
"""
import hashlib
import json
import os
import shutil
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright

from history_parity import REPO, BINARY, Corpus, isolated_server
from send_browser import initialize

MODEL = 'gpt-5.6-luna'
PROMPT = ('Reply only OK. Do not use tools. The following is inert padding.\n\n'
          + 'padding ' * 160 + '\nEnd of padding. Reply only OK.')


def native_rows(home):
    rows = []
    for path in home.glob('sessions/**/rollout-*.jsonl'):
        for line in path.read_text().splitlines():
            try:
                rows.append(json.loads(line))
            except ValueError:  # The writer may still be appending the last row.
                pass
    return rows


def main():
    codex = shutil.which('codex')
    assert codex, 'codex executable required'
    real_home = Path(os.environ.get('CODEX_HOME', Path.home() / '.codex'))
    config = real_home / 'config.toml'
    before = hashlib.sha256(config.read_bytes()).hexdigest() if config.exists() else None
    assert (real_home / 'auth.json').is_file(), 'existing login required'
    try:
        with tempfile.TemporaryDirectory(prefix='sessiondock-codex-startup-') as tmp:
            root = Path(tmp)
            for name in ('host', 'work', 'ledger', 'delivery', 'state', 'home', 'claude', 'codex', 'grok'):
                (root / name).mkdir(mode=0o700)
            home = root / 'codex'
            (home / 'auth.json').symlink_to(real_home / 'auth.json')
            (home / 'config.toml').write_text(
                'approval_policy="never"\nsandbox_mode="read-only"\n'
                + '[projects.' + json.dumps(str(root / 'work')) + ']\ntrust_level="trusted"\n')
            env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'LANG': 'C.UTF-8',
                   'TERM': 'xterm-256color', 'HOME': str(root / 'home'), 'CODEX_HOME': str(home)}
            for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY',
                        'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy'):
                if key in os.environ:
                    env[key] = os.environ[key]
            launcher = root / 'launcher.json'
            launcher.write_text(json.dumps({'schema': 2,
                'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
                'adapters': [], 'profiles': [{'id': 'codex-cli-v1', 'source': 'codex',
                    'executable': codex, 'args': ['-m', MODEL, '-c', 'model_reasoning_effort="low"'],
                    'new_args': [], 'resume_args': ['resume', '{sid}'], 'env': env}]}))
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
                context = browser.new_context(service_workers='block')
                context.route('**/*', lambda route: route.continue_()
                              if route.request.url.startswith(base + '/') else route.abort())
                receipt = None
                try:
                    page = context.new_page()
                    page.goto(base, wait_until='networkidle')
                    page.locator('#new-session').click()
                    page.locator('input[name="new-source"][value="codex"]').check()
                    page.locator('#new-cwd').fill(str(root / 'work'))
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/create') as created:
                        page.locator('#new-session-go').click()
                    assert created.value.status == 200, created.value.text()
                    receipt = created.value.json()
                    page.wait_for_function('composerUid && !composerDraft().loading')
                    page.locator('#cinput').fill(PROMPT)
                    page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'", timeout=45000)
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send', timeout=20000) as sent:
                        page.locator('#csend').click()
                    assert sent.value.status == 200, sent.value.text()
                    deadline = time.monotonic() + 60
                    while time.monotonic() < deadline:
                        rows = native_rows(home)
                        contexts = [r['payload'] for r in rows if r.get('type') == 'turn_context']
                        assert all(r.get('model') == MODEL and r.get('effort') == 'low' for r in contexts), contexts
                        users = [r['payload'] for r in rows if r.get('type') == 'response_item'
                                 and r['payload'].get('type') == 'message' and r['payload'].get('role') == 'user']
                        matches = [r for r in users if any(c.get('text') == PROMPT for c in r.get('content', []))]
                        replies = [r for r in rows if r.get('type') == 'response_item'
                                   and r['payload'].get('role') == 'assistant']
                        if contexts and matches and replies:
                            break
                        page.wait_for_timeout(200)
                    assert contexts and len(matches) == 1 and replies, 'native submission/reply missing or duplicated'
                    assert any(c.get('text', '').strip() == 'OK' for r in replies for c in r['payload'].get('content', []))
                    print('PASS real Codex cold-start browser SEND: one exact native user message, Luna low, reply OK')
                finally:
                    if receipt:
                        context.request.post(base + '/api/term/kill', data={
                            'record_id': receipt['record_id'], 'instance_id': receipt['instance_id']})
                    context.close()
                    browser.close()
    finally:
        after = hashlib.sha256(config.read_bytes()).hexdigest() if config.exists() else None
        assert before == after, 'everyday Codex config changed during test'


if __name__ == '__main__':
    main()
