#!/usr/bin/env python3
"""Chromium idle request budget: a selected, untouched page stays quiet.

Node and Hub pages select a synthetic conversation, reach steady state, then
count every request for an idle window. A third page takes over a free fake
Grok terminal and checks that the composer input probe backs off while the
screen is unchanged and returns to fast checks on output and typing.
Loopback servers and private temporary directories only.
"""

import argparse
from collections import Counter
from contextlib import ExitStack
import json
from pathlib import Path
import sys
import tempfile
import time
from types import SimpleNamespace
from urllib.parse import unquote, urlsplit

from playwright.sync_api import sync_playwright
from history_parity import REPO, BINARY, Corpus, claude_row, isolated_server
from hub_http_suite import Hub, free_port
from node_auth_suite import node_env, TOKEN
from send_browser import initialize

IDLE_SECONDS = 15
CHECK = '/api/session/conversation/check'
# Upper bounds for one idle window. Measured before the reduction: node 2
# (api/groups 2), Hub 6 (api/groups 2, transfers 3, audit 1) and a ready
# composer 10 CHECKs; after it: node 0, Hub 1 (audit), composer 3. Totals keep
# slack for the 30 s build check, 20 s backup reconcile, 60 s shell-env check
# and an audit flush landing in the same window.
BUDGET = {
    'node': {'/api/groups': 0, '/api/session/transfers': 0, 'total': 4},
    'hub': {'/api/groups': 0, '/api/session/transfers': 1, 'total': 5},
    'composer': {CHECK: 5},
}


def watch(page):
    requests = []
    page.on('request', lambda request: requests.append(
        (request.method, unquote(urlsplit(request.url).path), time.monotonic())))
    return requests


def idle_window(page, requests, seconds=IDLE_SECONDS):
    requests.clear()
    page.wait_for_timeout(seconds * 1000)
    return Counter(path for _, path, _ in requests), list(requests)


def report(label, counts):
    total = sum(counts.values())
    paths = ', '.join(f'{path}={count}' for path, count in sorted(counts.items())) or 'none'
    print(f'{label}: {IDLE_SECONDS}s idle total={total} ({paths})', flush=True)
    return total


def assert_budget(label, counts):
    total = sum(counts.values())
    for path, limit in BUDGET[label].items():
        seen = total if path == 'total' else counts.get(path, 0)
        assert seen <= limit, (label, path, seen, limit, dict(counts))


def list_page(browser, tmp, args, hub_mode, report_only):
    label = 'hub' if hub_mode else 'node'
    root = Path(tmp) / f'{label}-page'
    data = Corpus(root / 'node')
    for source in ('claude', 'codex', 'grok'):
        (data.root / source).mkdir(parents=True)
    (data.root / 'state').mkdir(mode=0o700)
    for i in range(2):
        sid = f'idle-{i}'
        data.put(sid, 'claude', [
            claude_row(sid, 'user', 'u0', None, f'Idle session {i}'),
            claude_row(sid, 'assistant', 'a0', 'u0', 'Initial reply'),
        ], [])
    nid = 'b' * 32
    ids = data.root / 'ids'; ids.mkdir(); (ids / 'node-id').write_text(nid + '\n')
    node = SimpleNamespace(name='idle', nid=nid, port=free_port(), token=TOKEN)
    with ExitStack() as stack:
        base, _ = stack.enter_context(isolated_server(data, args.binary, state_dir=data.root / 'state',
            extra_env=node_env(data.root, node.port, '127.0.0.0/8')))
        if hub_mode:
            hubroot = root / 'hub'; hubroot.mkdir()
            hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [node])
            hub.start(); stack.callback(hub.stop)
            base = f'http://127.0.0.1:{hub.port}'
        context = browser.new_context(service_workers='block'); stack.callback(context.close)
        page = context.new_page(); errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        requests = watch(page)
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('uiEventsReady && S.sessions.length===2 && T.listLoaded && !uiEventApplying',
                               timeout=30000)
        page.wait_for_function('SessionDockGroups.available', timeout=15000)
        uid = data.uid('idle-1')
        if hub_mode:
            uid = uid.replace(':', f':{nid}~', 1)
        page.locator(f'#side .item[data-uid="{uid}"] .t').click()
        page.wait_for_function('uid=>S.sel===uid && cache.has(uid)', arg=uid)
        page.wait_for_timeout(3000)
        counts, _ = idle_window(page, requests)
        report(label, counts)
        if not report_only:
            assert_budget(label, counts)
            # Event-driven refresh: a group created by another client reaches
            # this idle page through the UI event channel, not a timer.
            created = f'idle-group-{label}'
            response = context.request.post(base + '/api/groups', data={'create_groups': [created]})
            assert response.ok, response.text()
            page.wait_for_function('name=>SessionDockGroups.names.includes(name)', arg=created, timeout=10000)
        assert not errors, errors


def composer_page(browser, tmp, binary, report_only):
    root = Path(tmp) / 'composer'
    for name in ['host', 'work', 'ledger', 'state', 'home', 'claude', 'codex', 'grok']:
        (root / name).mkdir(mode=0o700, parents=True)
    screen = root / 'screen'
    screen.write_text('composer')
    launcher = root / 'launcher.json'
    launcher.write_text(json.dumps({'schema': 2,
        'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
        'adapters': [], 'profiles': [{'id': 'grok-cli-v1', 'source': 'grok',
            'executable': str(Path(sys.executable).resolve()),
            'args': [str(REPO / 'tests/fake_grok_composer.py')],
            'new_args': ['--session-id', '{session_id}'], 'resume_args': ['--resume', '{sid}'],
            'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                'TERM': 'xterm-256color', 'LANG': 'C.UTF-8', 'SESSIONDOCK_TEST_SCREEN': str(screen)}}]}))
    launcher.chmod(0o600)
    initialize('--initialize-lifecycle', root / 'ledger', binary)
    with isolated_server(Corpus(root), binary, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
            launcher_config=launcher, state_dir=root / 'state',
            file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _):
        context = browser.new_context(service_workers='block')
        try:
            page = context.new_page(); errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            requests = watch(page)
            page.goto(base, wait_until='domcontentloaded')
            page.wait_for_function('uiEventsReady && T.listLoaded', timeout=30000)
            page.locator('#new-session').click()
            page.locator('input[name="new-source"][value="grok"]').check()
            page.locator('#new-cwd').fill(str(root / 'work'))
            with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/create') as created:
                page.locator('#new-session-go').click()
            assert created.value.status == 200, created.value.text()
            page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'", timeout=20000)
            page.wait_for_timeout(3000)
            counts, _ = idle_window(page, requests)
            report('composer', counts)
            if report_only:
                return
            assert_budget('composer', counts)
            assert page.evaluate("composerDraft()?.inputStatus?.state") == 'ready'
            # Typing returns to fast checks immediately.
            requests.clear(); started = time.monotonic()
            page.locator('#cinput').press_sequentially('hi')
            deadline = started + 2.5
            while time.monotonic() < deadline and not any(p == CHECK and t > started for _, p, t in requests):
                page.wait_for_timeout(100)
            assert any(p == CHECK and t > started for _, p, t in requests), 'typing did not resume fast input checks'
            page.locator('#cinput').fill('')
            # A changed screen is noticed promptly, not after a long backoff.
            idle_window(page, requests, 10)
            started = time.monotonic()
            screen.write_text('login')
            page.wait_for_function("composerDraft()?.inputStatus?.state !== 'ready'", timeout=8000)
            noticed = time.monotonic() - started
            screen.write_text('composer')
            page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'", timeout=8000)
            print(f'composer: typing resumed checks; screen change noticed in {noticed:.1f}s', flush=True)
            assert not errors, errors
        finally:
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--report-only', action='store_true', help='print counts without asserting the budget')
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix='sessiondock-idle-requests-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for hub_mode in (False, True):
                list_page(browser, tmp, args, hub_mode, args.report_only)
            composer_page(browser, tmp, args.binary, args.report_only)
        finally:
            browser.close()
    print('PASS idle request budget' + (' (report only)' if args.report_only else ''), flush=True)


if __name__ == '__main__':
    main()
