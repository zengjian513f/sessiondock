#!/usr/bin/env python3
"""Report workers bind the guarded Codex status identity with a shared writer.

Two identical titles, delayed native publication, current and pre-upgrade
rename receipts, a node restart and real terminal/history clicks. Private fake
CLI, app-server and /proc only; no production records or models.
"""
import argparse
import json
import os
from pathlib import Path
import sys
import tempfile
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright
from bug_report_node_browser import open_report, wait_drafts
from browser_runtime import wait_for_async
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, REPO, Corpus, codex_message, codex_row, isolated_server
from private_hosts import private_hosts
from send_browser import initialize, xterm_includes
from spawned_by_fixtures import proc_pid

TITLE = 'bug report会出现两个一模一样的会话。比如当前会话就有另一个一样标题的'


def run(browser, root, binary, screenshots):
    for name in ('work', 'host', 'state', 'ledger', 'audit', 'reports', 'home',
                 'codex', 'claude', 'grok', 'staged', 'proc'):
        (root / name).mkdir(mode=0o700)
    corpus = Corpus(root)
    # A same-title session cannot be used to infer this launch's identity.
    decoy = '11111111-2222-4333-8444-555555555555'
    corpus.put('decoy', 'codex', [codex_row('session_meta', {'id': decoy,
        'cwd': str(root / 'work')}), codex_message('user', 'BUG: ' + TITLE)], [])
    proc = root / 'proc'
    (proc / 'stat').write_text('btime 1700000000\n')
    proc_pid(proc, 400, 'codex', ['codex', 'app-server'], 1)
    socket = root / 'app.sock'
    config = root / 'launcher.json'
    config.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
        'host_dir': str(root / 'host'), 'profiles': [{'id': 'codex-cli-v1', 'source': 'codex',
        'executable': sys.executable, 'args': [str(REPO / 'tests/fake_codex_cli.py'), '--reply'],
        'env': {'PATH': os.environ['PATH'], 'HOME': str(root / 'home'), 'TERM': 'xterm-256color',
            'SESSIONDOCK_TEST_CODEX_ROOT': str(root / 'staged'),
            'SESSIONDOCK_TEST_STATUS_EPHEMERAL': '1',
            'SESSIONDOCK_TEST_STATUS_LOG': str(root / 'status.log'),
            'SESSIONDOCK_TEST_SUBMISSIONS': str(root / 'submissions.jsonl')}}]}))
    config.chmod(0o600)
    initialize('--initialize-lifecycle', root / 'ledger')
    env = {'SESSIONDOCK_PROC_ROOT': str(proc), 'SESSIONDOCK_CODEX_APP_SERVER': str(socket),
        'SESSIONDOCK_BUG_REPORT_DIR': str(root / 'reports'), 'SESSIONDOCK_BUG_REPORT_WORKER': 'true',
        'SESSIONDOCK_BUG_REPORT_REPO': str(root / 'work')}

    def node():
        return isolated_server(corpus, binary, state_dir=root / 'state', host_dir=root / 'host',
            lifecycle_dir=root / 'ledger', launcher_config=config, audit_dir=root / 'audit',
            extra_env=env, file_roots=(root / 'work',), file_write_roots=(root / 'work',))

    def manifest(report):
        return json.loads((root / 'reports' / report['report_id'] / 'manifest.json').read_text())

    def publish(sid):
        path = root / 'codex' / f'rollout-{sid}.jsonl'
        (root / 'staged' / path.name).rename(path)
        (proc / '400/fd' / sid).symlink_to(path)
        return path

    with private_hosts(root), FakeAppServer(socket, corpus.paths['decoy'], decoy) as fake:
        with node() as (base, _):
            page = browser.new_page(viewport={'width': 1280, 'height': 900}, color_scheme='light')
            page.goto(base, wait_until='networkidle')
            reports = []
            for i in range(2):
                print(f'report {i + 1}: submit through dialog; hold native publication', flush=True)
                open_report(page)
                page.locator('input[name="bug-report-source"][value="codex"]').check()
                page.locator('#bug-report-description').fill(TITLE)
                wait_drafts(page)
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/bug-report') as sent:
                    page.locator('#bug-report-go').click()
                assert sent.value.status == 202, sent.value.text()
                report = sent.value.json()
                page.expose_function(f'submitted{i}', lambda r=report: manifest(r)['status'] == 'submitted')
                wait_for_async(page, f'async () => await submitted{i}()', timeout=25000)
                key = f'report-rename:{report["worker"]["record_id"]}'
                store = json.loads((root / 'state/conversations/conversation-ledger.json').read_text())
                receipt = next((r for r in store['requests'].values() if r['id'] == key), None)
                assert receipt, (key, list(store['requests']), report)
                sid = receipt['result']['sid']
                assert receipt['phase'] == 'sent' and sid != decoy
                reports.append((report, sid))
                # No native rollout is visible yet: exact SID must wait for it.
                page.wait_for_timeout(1800)
                ledger = json.loads((root / 'ledger/lifecycle-ledger.json').read_text())
                assert ledger['records'][report['worker']['record_id']]['binding'] is None
                if i == 0:
                    fake.thread_id, fake.rollout = sid, publish(sid)
                    corpus.paths[sid] = fake.rollout
                    uid = corpus.uid(sid)
                    page.wait_for_function("uid => document.querySelector(`#side .item[data-uid=\"${uid}\"]`)", arg=uid)
                    page.wait_for_function("uid => !document.querySelector(`#side .item[data-uid=\"${uid}\"]`)",
                        arg='tmux:' + report['worker']['name'], timeout=20000)
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    page.locator('#a-chat').click()
                    expect(page.locator('#msgs')).to_contain_text(TITLE)
                    if screenshots:
                        page.screenshot(path=str(screenshots / 'current-wide-light.png'))
                    page.locator('#a-term').click()
                    xterm_includes(page, '处理 sessiondock 缺陷报告')
                    page.locator('#a-chat').click()
            page.close()
        # Simulate exactly the old successful rename result on private data.
        report, sid = reports[1]
        ledger_path = root / 'state/conversations/conversation-ledger.json'
        old = json.loads(ledger_path.read_text())
        for receipt in old['requests'].values():
            if receipt['id'] == f'report-rename:{report["worker"]["record_id"]}':
                receipt['result'].pop('sid')
        ledger_path.write_text(json.dumps(old))
        fake.thread_id, fake.rollout = sid, publish(sid)
        print('restart: recover legacy receipt after its status card disappeared', flush=True)
        with node() as (base, _):
            page = browser.new_page(viewport={'width': 390, 'height': 844}, color_scheme='dark')
            page.goto(base, wait_until='networkidle')
            for report, sid in reports:
                pending = 'tmux:' + report['worker']['name']
                page.wait_for_function("uid => !document.querySelector(`#side .item[data-uid=\"${uid}\"]`)",
                    arg=pending, timeout=20000)
            # Three identical titles are real distinct native sessions: decoy + two reports.
            expect(page.locator('#side .item').filter(has_text=TITLE)).to_have_count(3)
            ledger = json.loads((root / 'ledger/lifecycle-ledger.json').read_text())
            for report, sid in reports:
                binding = ledger['records'][report['worker']['record_id']]['binding']
                assert binding['state'] == 'confirmed' and binding['method'] == 'process', binding
                evidence = 'guarded Codex /status' if sid == reports[0][1] else 'completed guarded SEND'
                assert binding['evidence'].startswith(evidence), binding
                assert binding['spec']['sid'] == sid, binding
            if screenshots:
                page.screenshot(path=str(screenshots / 'recovered-narrow-dark.png'))
            commands = [json.loads(line)['command'] for line in
                (root / 'submissions.jsonl.commands').read_text().splitlines()]
            assert len(commands) == 2 and all(command.startswith('/rename ') for command in commands), commands
            assert (root / 'status.log').read_text().splitlines() == ['/status', '/status']
            assert len((root / 'submissions.jsonl').read_text().splitlines()) == 2
            page.close()
    print('PASS report status binding: same-title isolation, shared daemon writer, delayed native '
          'history, current/old receipts, restart, terminal; no duplicate SEND/status', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-report-identity-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            run(browser, Path(tmp), args.binary.resolve(), args.screenshots)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
