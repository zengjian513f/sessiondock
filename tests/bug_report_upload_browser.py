#!/usr/bin/env python3
"""Real Hub/node report staging: recover a lost reply without duplicate uploads or launches."""
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

from browser_runtime import js
from playwright.sync_api import sync_playwright, expect

from history_parity import REPO, Corpus, isolated_server
from hub_http_suite import Hub, free_port
from hub_send_browser import prepare, cleanup_hosts
from bug_report_node_browser import open_report
from hub_fake_node import PNG


def run(browser, root, config):
    node = SimpleNamespace(nid='a' * 32, name='FixtureNode', port=free_port(), token='f' * 64)
    for name, value in [('node-token', node.token), ('node-id', node.nid)]:
        path = root / name
        path.touch(mode=0o600)
        path.write_text(value)
    env = {
        'SESSIONDOCK_NODE_BIND': f'127.0.0.1:{node.port}',
        'SESSIONDOCK_NODE_TOKEN_FILE': str(root / 'node-token'),
        'SESSIONDOCK_NODE_ID_FILE': str(root / 'node-id'),
        'SESSIONDOCK_NODE_PEERS': '127.0.0.0/8',
        'SESSIONDOCK_BUG_REPORT_DIR': str(root / 'reports'),
        'SESSIONDOCK_BUG_REPORT_REPO': str(root / 'work'),
    }
    with isolated_server(Corpus(root), REPO / 'target/release/sessiondock',
            state_dir=root / 'state', host_dir=root / 'host', lifecycle_dir=root / 'ledger',
            launcher_config=config, audit_dir=root / 'audit',
            extra_env=env):
        hub = Hub(REPO / 'target/release/sessiondock-hub', root / 'hub', [node])
        hub.start()
        context = browser.new_context(viewport={'width': 608, 'height': 761}, service_workers='block')
        page = context.new_page()
        attempts, errors = [], []
        fail_replies = 1
        stalled_reads = []

        def stall_draft(route):
            # Leave the first GET unanswered until the real browser timeout.
            # Recovery uses the real Hub/node, not a fabricated draft response.
            if not stalled_reads:
                stalled_reads.append(route)
                return
            route.continue_()

        def upload(route):
            nonlocal fail_replies
            if route.request.method != 'POST':
                return route.continue_()
            query = parse_qs(urlsplit(route.request.url).query)
            attempts.append((query['uid'][0], query['id'][0]))
            # Every attempt reaches the real Hub and authenticated Rust node.
            # Lose only the successful reply, after bytes are already durable.
            response = route.fetch()
            assert response.status == 200, response.text()
            if fail_replies:
                fail_replies -= 1
                route.fulfill(status=502, content_type='application/json',
                    body='{"error":"机器连接中断；写操作可能已执行，请核对目标机器状态"}')
            else:
                route.fulfill(response=response)

        context.route('**/api/session/conversation/attachment?*', upload)
        page.route('**/api/session/conversation?*', stall_draft)
        page.on('pageerror', lambda error: errors.append(str(error)))
        try:
            page.goto(f'http://127.0.0.1:{hub.port}', wait_until='networkidle')
            page.evaluate(js(r"""() => {
                window.draftReadAudit = [];
                const original = browserAuditEvent;
                browserAuditEvent = (event, data, content, fields) => {
                    if (data?.url?.startsWith('api/session/conversation?'))
                        draftReadAudit.push({event, data, content, fields});
                    return original(event, data, content, fields);
                };
            }""", r"""() => {
                window.draftReadAudit = [];
                const original = runtime.core.audit.browserAuditEvent;
                runtime.core.audit.browserAuditEvent = (event, data, content, fields) => {
                    if (data?.url?.startsWith('api/session/conversation?'))
                        draftReadAudit.push({event, data, content, fields});
                    return original(event, data, content, fields);
                };
            }"""))
            open_report(page)
            page.wait_for_function(js('bugReportDraftObject().loadFailed', 'runtime.launch.bugReportDraftObject().loadFailed'), timeout=16000)
            stalled_reads[0].abort()
            expect(page.locator('#bug-report-items')).to_contain_text('草稿读取超时')
            page.fill('#bug-report-description', 'Recover report attachment reply')
            page.wait_for_function(js('!bugReportDraftObject().loadFailed && !bugReportDraftObject().storageError && bugReportDraftObject().savedVersion === bugReportDraftObject().editVersion', '!runtime.launch.bugReportDraftObject().loadFailed && !runtime.launch.bugReportDraftObject().storageError && runtime.launch.bugReportDraftObject().savedVersion === runtime.launch.bugReportDraftObject().editVersion'))
            expect(page.locator('#bug-report-description')).to_have_value('Recover report attachment reply')
            audit = page.evaluate('draftReadAudit')
            failure = next(e for e in audit if e['event'] == 'http.request.failed')
            assert failure['data']['phase'] == 'headers' and failure['data']['status'] is None, failure
            assert failure['data']['timeout_ms'] == 12000 and 'TimeoutError' in failure['data']['error'], failure
            assert any(e['event'] == 'http.response.received' for e in audit), audit
            assert all(e['content'] is None for e in audit), audit
            assert failure['fields']['traceId'] and failure['fields']['uid'].startswith('report:'), failure
            assert not list((root / 'reports').glob('BUG-*'))
            print('PASS draft read timeout: audited phase, retained input, automatic recovery without worker creation', flush=True)
            # Same size class as the reported phone image; valid PNG with padding.
            payload = PNG + b'\0' * (831 * 1024 - len(PNG))
            page.locator('#bug-report-file').set_input_files(
                {'name': 'capture.png', 'mimeType': 'image/png', 'buffer': payload})
            page.wait_for_function(js('bugReportDraftObject().attachments[0]?.uploaded?.upload_id && !bugReportDraftObject().attachments[0].staging', 'runtime.launch.bugReportDraftObject().attachments[0]?.uploaded?.upload_id && !runtime.launch.bugReportDraftObject().attachments[0].staging'))
            assert len(attempts) == 2 and attempts[0] == attempts[1], attempts
            assert not page.locator('#bug-report-items').get_by_role('button', name='重试', exact=True).count()
            print('PASS lost upload reply: real bytes saved; retry reuses draft/upload ID', flush=True)

            fail_replies = 2
            page.locator('#bug-report-file').set_input_files(
                {'name': 'second.png', 'mimeType': 'image/png', 'buffer': PNG})
            retry = page.locator('#bug-report-items').get_by_role('button', name='重试', exact=True)
            expect(retry).to_be_visible()
            assert len(attempts) == 4 and attempts[2] == attempts[3], attempts
            expect(page.locator('#bug-report-description')).to_have_value('Recover report attachment reply')
            retry.click()
            page.wait_for_function(js('bugReportDraftObject().attachments.every(a => a.uploaded?.upload_id && !a.staging)', 'runtime.launch.bugReportDraftObject().attachments.every(a => a.uploaded?.upload_id && !a.staging)'))
            assert len(attempts) == 5 and attempts[4] == attempts[2], attempts
            print('PASS persistent interruption: bounded retry, retained text/file, manual recovery', flush=True)

            # The worker CLI runs with the model and effort picked in the dialog.
            page.locator('#bug-report-model').click()
            page.locator('#bug-report-model-options [role=option]', has_text='Opus').click()
            page.locator('#bug-report-effort').select_option('high')
            with page.expect_response(lambda r: urlsplit(r.url).path == '/api/bug-report') as submitted:
                page.locator('#bug-report-go').click()
            assert submitted.value.status == 202, submitted.value.text()
            assert json.loads(submitted.value.request.post_data)['model'] == 'opus'
            ledger = json.loads((root / 'ledger/lifecycle-ledger.json').read_text())
            specs = [r['spec'] for r in ledger['records'].values() if r['request_id'].startswith('bug-report-')]
            assert [(spec.get('model'), spec.get('effort')) for spec in specs] == [('opus', 'high')], specs
            sid = submitted.value.json()['worker']['sid']
            argv = next(cmd for cmd in (p.read_bytes().rstrip(b'\0').split(b'\0') for p in Path('/proc').glob('[0-9]*/cmdline')
                                        if p.exists() and sid.encode() in p.read_bytes()) if b'--effort' in cmd)
            assert argv[-4:] == [b'--model', b'opus', b'--effort', b'high'], argv
            expect(page.locator('#bug-report-dialog')).not_to_be_visible()
            bundles = list((root / 'reports').glob('BUG-*'))
            assert len(bundles) == 1, bundles
            # The worker brief defers validation to AGENTS.md: no unit-test step.
            brief = (bundles[0] / 'worker-prompt.md').read_text()
            assert 'AGENTS.md' in brief and 'cargo test' not in brief, brief
            files = sorted((root / 'work/sessiondock_attachments').rglob('*.png'))
            assert len(files) == 2, files
            assert {p.name: p.read_bytes() for p in files} == {'capture.png': payload, 'second.png': PNG}
            assert not errors, errors
            print('PASS report submit: one bundle, AGENTS.md-only worker brief, two exact attachments, one worker response with the chosen model/effort', flush=True)
        finally:
            context.close()
            hub.stop()


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-report-upload-') as tmp:
        root = Path(tmp)
        config = prepare(root)
        with sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            try:
                run(browser, root, config)
            finally:
                browser.close()
                cleanup_hosts(root)


if __name__ == '__main__':
    main()
