#!/usr/bin/env python3
"""Report dialog, native rename and first task with real Codex, isolated home and Luna low only.

Operator-only (*_real): run directly or with run_validation --include-real.
Requires the existing controlled login; never changes everyday defaults.
Checks native user text and turn model/effort, not only the PTY write ACK.
"""
import argparse
import hashlib
import json
import os
import shutil
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright

from history_fixtures import REPO, BINARY, Corpus, isolated_server
from private_hosts import private_hosts
from send_browser import initialize

MODEL = 'gpt-5.6-luna'
DESCRIPTION = '原生命名后发送报告验收\nSynthetic test only. Reply OK without using tools.'


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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    binary = parser.parse_args().binary.resolve(strict=True)
    codex = shutil.which('codex')
    assert codex, 'codex executable required'
    real_home = Path(os.environ.get('CODEX_HOME', Path.home() / '.codex'))
    config = real_home / 'config.toml'
    before = hashlib.sha256(config.read_bytes()).hexdigest() if config.exists() else None
    assert (real_home / 'auth.json').is_file(), 'existing login required'
    try:
        with tempfile.TemporaryDirectory(prefix='sessiondock-codex-report-') as tmp, private_hosts(Path(tmp)):
            root = Path(tmp)
            for name in ('host', 'work', 'ledger', 'delivery', 'state', 'home', 'claude', 'codex', 'grok', 'reports', 'audit'):
                (root / name).mkdir(mode=0o700)
            (root / 'work/AGENTS.md').write_text(
                'This is an isolated automated acceptance test. Every bug report is synthetic. '
                'Do not inspect files, execute tools, fix bugs, commit, push or deploy. '
                'Reply only OK to the report task. This instruction applies to every report.\n')
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
            with isolated_server(Corpus(root), binary, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                    launcher_config=launcher, state_dir=root / 'state',
                    audit_dir=root / 'audit', extra_env={
                        'SESSIONDOCK_CODEX_ROOT': str(home / 'sessions'),
                        'SESSIONDOCK_CODEX_INDEX': str(home / 'session_index.jsonl'),
                        'SESSIONDOCK_BUG_REPORT_DIR': str(root / 'reports'),
                        'SESSIONDOCK_BUG_REPORT_REPO': str(root / 'work')},
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
                    if not page.locator('#report-bug').is_visible():
                        page.locator('#header-more-btn').click()
                    page.locator('#report-bug').click()
                    page.locator('#bug-report-description').fill(DESCRIPTION)
                    page.locator('input[name="bug-report-source"][value="codex"]').check()
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/bug-report') as created:
                        page.locator('#bug-report-go').click()
                    assert created.value.status == 202, created.value.text()
                    report = created.value.json()
                    receipt = report['worker']
                    title = 'BUG: ' + DESCRIPTION.splitlines()[0]
                    assert receipt['title'] == title, receipt
                    bundle = Path(report['path'])
                    prompt = (bundle / 'worker-prompt.md').read_text().strip()
                    page.locator('#bug-report-toast').get_by_role('button', name='打开', exact=True).click()
                    deadline = time.monotonic() + 60
                    while time.monotonic() < deadline:
                        rows = native_rows(home)
                        contexts = [r['payload'] for r in rows if r.get('type') == 'turn_context']
                        assert all(r.get('model') == MODEL and r.get('effort') == 'low' for r in contexts), contexts
                        users = [r['payload'] for r in rows if r.get('type') == 'response_item'
                                 and r['payload'].get('type') == 'message' and r['payload'].get('role') == 'user']
                        matches = [r for r in users if any(c.get('text', '').strip() == prompt for c in r.get('content', []))]
                        replies = [r for r in rows if r.get('type') == 'response_item'
                                   and r['payload'].get('role') == 'assistant']
                        if contexts and matches and replies:
                            break
                        page.wait_for_timeout(200)
                    assert contexts and len(matches) == 1 and replies, ('native submission/reply missing or duplicated', json.loads((bundle / 'manifest.json').read_text()), rows[-3:])
                    assert any(c.get('text', '').strip() == 'OK' for r in replies for c in r['payload'].get('content', []))
                    manifest = json.loads((bundle / 'manifest.json').read_text())
                    assert manifest['status'] == 'submitted', manifest
                    sid = next(r['payload']['id'] for r in rows if r.get('type') == 'session_meta')
                    names = [json.loads(line) for line in (home / 'session_index.jsonl').read_text().splitlines()]
                    assert [r for r in names if r['id'] == sid][-1]['thread_name'] == title, names
                    assert not any(c.get('text', '').startswith(('/rename ', '/status'))
                                   for r in users for c in r.get('content', [])), users
                    page.wait_for_function("title => [...document.querySelectorAll('#side .item')].some(n => n.textContent.includes(title))", arg=title)
                    print('PASS real Codex report browser: dialog → native rename → exact first task → reply OK; name retained; Luna low')
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
