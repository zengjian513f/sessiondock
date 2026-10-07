#!/usr/bin/env python3
"""Browser SEND from a new real Grok session, isolated GROK_HOME, grok-4.6 low only.

Operator-only (*_real): run directly or with run_validation --include-real.
Requires the existing controlled Grok login, which is linked, never read or
copied; never changes everyday defaults. The model and effort are chosen in
the new-session picker as a user would, and the suite checks what Grok itself
recorded (summary model/effort, one native user query, a reply), not only the
PTY write ACK.
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

from grok_files_real_fixtures import MODEL, EFFORT
from history_fixtures import REPO, BINARY, Corpus, isolated_server
from private_hosts import private_hosts
from send_browser import initialize

PROMPT = 'Reply only OK. Do not use tools.'


def grok_binary():
    found = shutil.which('grok')
    if found:
        return found
    for path in (Path.home() / '.local/bin/grok', Path.home() / '.grok/bin/grok'):
        if path.is_file():
            return str(path)
    raise AssertionError('grok executable required')


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None


def text_of(content):
    if isinstance(content, str):
        return content
    return ''.join(part.get('text', '') for part in content or [] if isinstance(part, dict))


def native(sessions):
    """(summary, chat rows) of the one session Grok wrote, or (None, [])."""
    for summary in sessions.glob('*/*/summary.json'):
        rows = []
        chat = summary.with_name('chat_history.jsonl')
        if chat.exists():
            for line in chat.read_text().splitlines():
                try:
                    rows.append(json.loads(line))
                except ValueError:  # The writer may still be appending the last row.
                    pass
        try:
            return json.loads(summary.read_text()), rows
        except ValueError:
            return None, []
    return None, []


def main():
    grok = grok_binary()
    real = Path(os.environ.get('GROK_HOME', Path.home() / '.grok'))
    config = real / 'config.toml'
    before = digest(config)
    assert (real / 'auth.json').is_file(), 'existing controlled Grok login required'
    assert (real / 'models_cache.json').is_file(), 'Grok model cache required (run grok once)'
    try:
        with tempfile.TemporaryDirectory(prefix='sessiondock-grok-send-') as tmp, private_hosts(Path(tmp)):
            root = Path(tmp)
            for name in ('host', 'work', 'ledger', 'delivery', 'state', 'home', 'claude', 'codex', 'grok'):
                (root / name).mkdir(mode=0o700)
            home = root / 'native'
            home.mkdir(mode=0o700)
            (home / 'sessions').symlink_to(root / 'grok')
            (home / 'auth.json').symlink_to((real / 'auth.json').resolve())
            # The picker reads Grok's own model cache; a private copy keeps a
            # refresh by the CLI out of the everyday home.
            shutil.copyfile(real / 'models_cache.json', home / 'models_cache.json')
            (home / 'trusted_folders.toml').write_text(
                f'[folders.{json.dumps(str(root / "work"))}]\ntrusted = true\ndecided_at = {int(time.time())}\n')
            env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'LANG': 'C.UTF-8',
                   'TERM': 'xterm-256color', 'HOME': str(root / 'home'), 'GROK_HOME': str(home)}
            for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY',
                        'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy'):
                if key in os.environ:
                    env[key] = os.environ[key]
            launcher = root / 'launcher.json'
            launcher.write_text(json.dumps({'schema': 2,
                'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
                'adapters': [], 'profiles': [{'id': 'grok-cli-v1', 'source': 'grok',
                    'executable': grok, 'args': [], 'new_args': [],
                    'resume_args': ['--resume', '{sid}'], 'env': env}]}))
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
                context.route('**/*', lambda route: route.continue_()
                              if route.request.url.startswith(base + '/') else route.abort())
                receipt = None
                try:
                    page = context.new_page()
                    page.goto(base, wait_until='networkidle')
                    page.locator('#new-session').click()
                    page.locator('input[name="new-source"][value="grok"]').check()
                    page.locator('#new-cwd').fill(str(root / 'work'))
                    # Pick the cheap model and low effort in the picker as a user would.
                    page.locator('#new-model').click()
                    page.locator(f'#new-model-options [data-model-option][title="{MODEL}"]').click()
                    page.wait_for_function("effort => [...document.querySelectorAll('#new-effort option')]"
                                           ".some(o => o.value === effort && !o.disabled)", arg=EFFORT)
                    page.locator('#new-effort').select_option(EFFORT)
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/create') as created:
                        page.locator('#new-session-go').click()
                    assert created.value.status == 200, created.value.text()
                    receipt = created.value.json()
                    page.wait_for_function('composerUid && !composerDraft().loading')
                    page.locator('#cinput').fill(PROMPT)
                    page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'", timeout=60000)
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send',
                                              timeout=20000) as sent:
                        page.locator('#csend').click()
                    assert sent.value.status == 200, sent.value.text()
                    deadline = time.monotonic() + 90
                    summary, rows, queries, replies = None, [], [], []
                    while time.monotonic() < deadline:
                        summary, rows = native(root / 'grok')
                        queries = [r for r in rows if r.get('type') == 'user' and 'prompt_index' in r]
                        replies = [r for r in rows if r.get('type') == 'assistant' and text_of(r.get('content')).strip()]
                        if summary and queries and replies:
                            break
                        page.wait_for_timeout(250)
                    assert summary, 'Grok wrote no session summary'
                    assert (summary.get('current_model_id'), summary.get('reasoning_effort')) == (MODEL, EFFORT), \
                        (summary.get('current_model_id'), summary.get('reasoning_effort'))
                    matches = [r for r in queries if PROMPT in text_of(r.get('content'))]
                    assert len(queries) == 1 and len(matches) == 1, \
                        ('native query missing or duplicated', [text_of(r.get('content'))[:120] for r in queries])
                    assert replies and any('OK' in text_of(r.get('content')) for r in replies), \
                        [text_of(r.get('content'))[:120] for r in replies]
                    page.wait_for_function("text => [...document.querySelectorAll('#msgs .msg[data-role=user]')]"
                                           ".some(m => m.textContent.includes(text))", arg=PROMPT, timeout=30000)
                    print(f'PASS real Grok browser SEND: one native query, {MODEL} {EFFORT}, reply OK', flush=True)
                finally:
                    if receipt:
                        context.request.post(base + '/api/term/kill', data={
                            'record_id': receipt['record_id'], 'instance_id': receipt['instance_id']})
                    context.close()
                    browser.close()
    finally:
        assert digest(config) == before, 'everyday Grok config changed during test'


if __name__ == '__main__':
    main()
