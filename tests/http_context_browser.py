#!/usr/bin/env python3
"""Real insecure HTTP origin: new launch and recoverable tree deletion.

Chromium alone maps a reserved .test hostname to loopback. No hosts/proxy edits,
API action shortcuts, crypto injection, production data or paid CLI calls.
"""
import argparse
from contextlib import contextmanager
import os
import subprocess
import urllib.request
import json
from pathlib import Path
import re
import sys
import tempfile
import time
from playwright.sync_api import expect, sync_playwright
from browser_evidence_fixtures import run_with_evidence
from history_fixtures import BINARY, REPO
from frontend_paths import frontend_dir
from hub_fixtures import free_port
from session_clone_browser import prepare
from new_session_model_browser import open_dialog, pick_source
from private_hosts import private_hosts
from send_browser import initialize


@contextmanager
def http_server(corpus, binary, launcher, log):
    root = corpus.root
    port = free_port()
    base = f'http://127.0.0.1:{port}'
    env = {k: v for k, v in os.environ.items() if not k.startswith('SESSIONDOCK_')}
    env.update({'SESSIONDOCK_BIND': f'127.0.0.1:{port}',
        'SESSIONDOCK_PUBLIC_HOSTS': f'sessiondock-http.test:{port}',
        'SESSIONDOCK_WEB_DIR': str(frontend_dir()), 'SESSIONDOCK_LOG_REQUESTS': 'all'})
    for key, path in {'STATE_DIR': 'state', 'PTYHOST_DIR': 'host', 'LIFECYCLE_DIR': 'ledger',
            'TRASH_DIR': 'trash', 'AUDIT_DIR': 'audit', 'PROC_ROOT': 'proc',
            'CLAUDE_ROOT': 'claude', 'CODEX_ROOT': 'codex', 'GROK_ROOT': 'grok',
            'OPENCODE_ROOT': 'opencode', 'OPENCODE_DB': 'opencode/opencode.db',
            'AGY_ROOT': 'agy', 'AGY_HOME': 'agy-home', 'LAUNCHER_CONFIG': launcher.name}.items():
        env['SESSIONDOCK_' + key] = str(root / path)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with log.open('wb') as stream:
        process = subprocess.Popen([str(binary.resolve())], cwd=REPO, env=env, stdout=stream, stderr=stream)
        try:
            for _ in range(150):
                assert process.poll() is None, log.read_text()
                try:
                    with opener.open(base+'/api/health', timeout=.2):
                        break
                except OSError:
                    time.sleep(.05)
            else:
                raise AssertionError('private server health timeout')
            yield base
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait(timeout=5)
                raise AssertionError('private server shutdown timeout')


def rows(path):
    return [json.loads(s) for s in path.read_text().splitlines() if s.strip()] if path.exists() else []


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--output', type=Path, default=Path('target/http-context'))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    results = []
    # Only this test process and its browser/driver children bypass ambient proxies.
    for key in list(os.environ):
        if key.lower() in ('http_proxy', 'https_proxy', 'all_proxy'):
            os.environ.pop(key)
    os.environ['NO_PROXY'] = os.environ['no_proxy'] = '*'
    with tempfile.TemporaryDirectory(prefix='sessiondock-http-') as tmp, private_hosts(Path(tmp)), sync_playwright() as pw:
        root = Path(tmp)
        corpus = prepare(root)
        for name in ('home', 'host', 'ledger'):
            (root / name).mkdir(exist_ok=True)
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(args.binary.resolve().with_name('ptyhost')),
            'host_dir': str(root / 'host'), 'adapters': [], 'profiles': [{
                'id': 'opencode-cli-v1', 'source': 'opencode', 'executable': str(Path(sys.executable).resolve()),
                'args': [str(REPO / 'tests/fake_model_cli.py')], 'env': {'PATH': '/usr/bin:/bin',
                'HOME': str(root / 'home'), 'TERM': 'xterm-256color', 'SESSIONDOCK_TEST_MODELS': 'fixture/model',
                'SESSIONDOCK_TEST_ARGV_LOG': str(root / 'argv.jsonl')}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger', args.binary)
        log = root / 'server.log'
        browser = pw.chromium.launch(headless=True, args=[
            '--host-resolver-rules=MAP sessiondock-http.test 127.0.0.1', '--no-proxy-server'])
        try:
            with http_server(corpus, args.binary, launcher, log) as base:
                ids = []
                for secure, width in ((False, 1280), (False, 390), (True, 1280)):
                    context = browser.new_context(viewport={'width': width, 'height': 900}, service_workers='block')
                    page = context.new_page()
                    errors = []
                    page.on('pageerror', lambda e: errors.append(str(e)))
                    origin = base if secure else base.replace('127.0.0.1', 'sessiondock-http.test')
                    response = page.goto(origin, wait_until='networkidle')
                    assert response.ok and response.server_addr()['ipAddress'] == '127.0.0.1'
                    environment = page.evaluate('({secure:isSecureContext, uuid:typeof crypto.randomUUID, random:typeof crypto.getRandomValues})')
                    page.screenshot(path=str(args.output/f'initial-{secure}-{width}.png'))
                    print('ORIGIN', origin, environment, flush=True)
                    assert environment == {'secure': secure, 'uuid': 'function' if secure else 'undefined', 'random': 'function'}, environment
                    if width == 1280 and not secure:
                        open_dialog(page); pick_source(page, 'opencode')
                        page.locator('#new-cwd').fill(str(root/'workspace'))
                        with page.expect_response('**/api/term/create') as created:
                            page.locator('#new-session-go').click()
                        launch = created.value.json()
                        assert created.value.ok and launch.get('state') != 'failed', launch
                        expect(page.locator('#new-session-dialog')).to_be_hidden()
                        assert list((root/'host').glob('*.json'))
                        assert any(a[:2] == ['api', 'session.create'] for a in rows(root/'argv.jsonl'))
                        # The lifecycle ledger must persist the returned launch identity.
                        persisted = '\n'.join(p.read_text(errors='replace') for p in (root/'ledger').rglob('*.json'))
                        assert launch['launch_id'] in persisted, launch
                        page.reload(wait_until='networkidle')
                    uid = corpus.uid('a')
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    def preview():
                        page.locator('#a-more').click()
                        page.locator('#a-delete-tree').click()
                        dialog = page.locator('#delete-tree-dialog')
                        expect(dialog.locator('.tree-confirm')).to_be_enabled(timeout=20000)
                        return dialog
                    before = {key: p.exists() for key, p in corpus.paths.items()}
                    dialog = preview()
                    page.screenshot(path=str(args.output/f'preview-{secure}-{width}.png'))
                    dialog.locator('.tree-close').click()
                    assert before == {key: p.exists() for key, p in corpus.paths.items()}
                    dialog = preview()
                    started = time.monotonic()
                    try:
                        with page.expect_request('**/api/session/tree/delete', timeout=10000) as outgoing:
                            dialog.locator('.tree-confirm').click()
                    except Exception:
                        (args.output/'browser-errors.json').write_text(json.dumps({'environment': environment, 'errors': errors}))
                        raise
                    request = outgoing.value
                    body = request.post_data_json
                    rid = body['request_id']
                    assert re.fullmatch(r'[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}', rid), rid
                    assert rid not in ids
                    ids.append(rid)
                    expect(dialog.locator('.transfer-progress-label')).to_have_text('整棵会话树已移入回收站', timeout=20000)
                    elapsed = (time.monotonic()-started)*1000
                    receipt = json.loads((root/'trash'/f'.tree-delete-{rid}.json').read_text())
                    assert receipt['phase'] == 'complete', receipt
                    for key, path in corpus.paths.items():
                        assert path.exists() == (key == 'unrelated'), (key, path)
                    manifests = list((root/'trash').glob('*/manifest.json'))
                    assert len(manifests) == 1
                    manifest = json.loads(manifests[0].read_text())
                    assert len(manifest['sessions']) > 1
                    headers = request.all_headers()
                    trace, page_id = headers['x-sessiondock-trace'], headers['x-sessiondock-page']
                    dialog.locator('.tree-close').click()
                    page.reload(wait_until='networkidle')
                    if width == 390: page.locator('#header-more-btn').click()
                    page.locator('#trash').click()
                    item = page.locator('.trash-item')
                    expect(item).to_have_count(1)
                    item.get_by_role('button', name='恢复整棵树', exact=True).click()
                    expect(item).to_have_count(0, timeout=20000)
                    assert all(p.exists() for p in corpus.paths.values())
                    page.screenshot(path=str(args.output/f'restored-{secure}-{width}.png'))
                    # Normal browser batching, no internal flush helper.
                    page.wait_for_timeout(5500)
                    http = [r for r in rows(log) if r.get('event') == 'http.request' and r.get('trace') == trace]
                    assert any(r.get('page') == page_id and r.get('status') == 200 for r in http), http
                    audits = [r for p in (root/'audit').rglob('*.jsonl') for r in rows(p)]
                    assert page_id in json.dumps(audits), 'browser page ID absent from persisted audit'
                    joined = [r for r in audits if r.get('trace_id') == trace and r.get('request_id') == rid]
                    assert any(r.get('event') == 'browser.tree.request.completed' for r in joined), joined
                    assert not errors, errors
                    results.append({'environment': environment, 'width': width, 'request_id': rid,
                        'trace': trace, 'page': page_id, 'delete_ms': elapsed, 'receipt_phase': receipt['phase'],
                        'manifest_sessions': len(manifest['sessions']), 'http': http,
                        'audit_events': len(audits), 'audit_bytes': sum(p.stat().st_size for p in (root/'audit').rglob('*.jsonl'))})
                    context.close()
        finally:
            browser.close()
        (args.output/'server.jsonl').write_text(log.read_text())
    (args.output/'result.json').write_text(json.dumps({'scenarios': results,
        'scope': 'real insecure HTTP and secure loopback; new fake-provider launch, cancel/delete/refresh/restore; durable receipts and log correlation'}, indent=2))
    print('PASS real insecure HTTP new session/tree deletion, durable receipts, audit/HTTP correlation; secure loopback regression')


if __name__ == '__main__':
    run_with_evidence(main, 'target/http-context')
