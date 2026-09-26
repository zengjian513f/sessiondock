#!/usr/bin/env python3
"""Browser send of text plus two PNG attachments to an isolated fake Codex PTY."""
import base64
import json
import os
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from history_parity import REPO, BINARY, Corpus, isolated_server
from media_browser import PNG
from send_browser import initialize, xterm_includes


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-codex-images-') as temporary:
        root = Path(temporary).resolve()
        for name in ('host', 'work', 'ledger', 'delivery', 'state', 'home', 'claude', 'codex', 'grok', 'audit', 'reports'):
            (root / name).mkdir(mode=0o700)
        # Model the update trap: without the launch override the CLI exits
        # before rendering an editor or creating a native session.
        cli = root / 'codex.py'
        cli.write_text("import sys, runpy\n"
            "assert any(sys.argv[i] in ('-c', '--config') and sys.argv[i+1] == "
            "'check_for_update_on_startup=false' for i in range(1, len(sys.argv)-1)), "
            "'startup updater would exit this session'\n"
            + "runpy.run_path(" + repr(str(REPO / 'tests/fake_codex_cli.py')) + ", run_name='__main__')\n")
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'codex-cli-v1', 'source': 'codex',
                'executable': str(Path(sys.executable).resolve()),
                'args': [str(cli), '--model', 'gpt-5.6-luna'],
                'new_args': [], 'resume_args': ['resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                    'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                    'SESSIONDOCK_TEST_ANIMATED_PADDING': '1',
                    'SESSIONDOCK_TEST_STARTUP_DELAY': '5',
                    'SESSIONDOCK_TEST_COLLAPSED_PASTE': '1',
                    'SESSIONDOCK_TEST_FOOTERLESS_PASTE': '1',
                    'SESSIONDOCK_TEST_FOOTER_PASTE_FILE': str(root / 'footer-paste'),
                    'SESSIONDOCK_TEST_SCROLLED_PASTE': '1',
                    'SESSIONDOCK_TEST_SUBMISSIONS': str(root / 'submissions.jsonl'),
                    'SESSIONDOCK_TEST_CODEX_ROOT': str(root / 'codex')}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger')
        initialize('--initialize-delivery', root / 'delivery')
        with isolated_server(Corpus(root), BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                launcher_config=launcher, delivery_dir=root / 'delivery', state_dir=root / 'state',
                audit_dir=root / 'audit', extra_env={
                    'SESSIONDOCK_BUG_REPORT_DIR': str(root / 'reports'),
                    'SESSIONDOCK_BUG_REPORT_REPO': str(root / 'work')},
                file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            receipt = None
            worker = None
            context = None
            try:
                context = browser.new_context(service_workers='block')
                context.route('**/*', lambda route: route.continue_()
                    if route.request.url.startswith(base + '/') else route.abort())
                page = context.new_page()
                errors, dialogs = [], []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.on('dialog', lambda dialog: (dialogs.append(dialog.message), dialog.accept()))
                page.goto(base, wait_until='networkidle')
                page.locator('#new-session').click()
                page.locator('input[name="new-source"][value="codex"]').check()
                page.locator('#new-cwd').fill(str(root / 'work'))
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/term/create') as created:
                    page.locator('#new-session-go').click()
                assert created.value.status == 200, created.value.text()
                receipt = created.value.json()
                assert receipt['running'], receipt
                page.wait_for_function("composerUid && !composerDraft().loading")
                page.wait_for_function("composerDraft()?.inputStatus?.code === 'cli_starting'", timeout=4000)
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'", timeout=15000)
                # An already-open pre-fix Codex can still exit during update.
                # Empty pending sessions must retain a clickable recovery path.
                stopped = context.request.post(base + '/api/term/kill', data={
                    'record_id': receipt['record_id'], 'instance_id': receipt['instance_id']})
                assert stopped.status == 200, stopped.text()
                restart = page.get_by_role('button', name='重新启动', exact=True)
                expect(restart).to_be_visible(timeout=15000)
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/restart') as restarted:
                    restart.click()
                assert restarted.value.status == 200, restarted.value.text()
                receipt = restarted.value.json()
                assert receipt['source'] == 'codex' and receipt['cwd'] == str(root / 'work'), receipt
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'", timeout=15000)
                page.locator('#cinput').fill('report task\n\n│ >_ OpenAI Codex (quoted text)\n│ model: loading\n\n最后一段\n')
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send', timeout=15000) as multiline:
                    page.locator('#cinput').press('Enter')
                assert multiline.value.status == 200, multiline.value.text()
                page.locator('#a-term').click()
                xterm_includes(page, '> report task')
                page.locator('#a-term').click()
                # Count the critical path and hold the post-send draft save.
                # A slow cleanup must not keep the successful send spinning.
                page.locator('#cinput').fill('latency regression')
                page.evaluate('async () => await composerDraftWrites')
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'")
                page.evaluate('''() => {
                    window.sendTrace = [];
                    window.sendFetch = window.fetch;
                    window.releaseCleanup = null;
                    window.fetch = async (url, options) => {
                        const path = new URL(url, location.href).pathname;
                        if ((composerSending || sendTrace.includes('send')) && options?.method === 'POST'
                            && path.includes('/conversation')) {
                            sendTrace.push(path.split('/').pop());
                            if (path.endsWith('/conversation') && sendTrace.includes('send') && !window.releaseCleanup)
                                await new Promise(resolve => { window.releaseCleanup = resolve; });
                        }
                        return sendFetch(url, options);
                    };
                    window.sendStarted = performance.now();
                }''')
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as fast_sent:
                    page.locator('#csend').click()
                assert fast_sent.value.status == 200, fast_sent.value.text()
                page.wait_for_function('() => !composerSending', timeout=2000)
                expect(page.locator('#cinput')).to_have_value('')
                trace = page.evaluate('sendTrace')
                assert trace[:2] == ['conversation', 'send'], trace
                print('Codex click-to-clear ms:', round(page.evaluate('performance.now() - sendStarted')), flush=True)
                page.wait_for_function('() => !!window.releaseCleanup')
                page.locator('#cinput').fill('edit while cleanup is pending')
                page.evaluate('''() => { window.fetch = sendFetch; releaseCleanup?.(); }''')
                page.evaluate('async () => await composerDraftWrites')
                expect(page.locator('#cinput')).to_have_value('edit while cleanup is pending')
                saved = context.request.get(base + '/api/session/conversation',
                    params={'uid': page.evaluate('composerUid')}).json()['draft']
                assert saved['value']['text'] == 'edit while cleanup is pending', saved
                page.locator('#cinput').fill('')
                png = base64.b64decode(PNG)
                page.locator('#cadd').click()
                with page.expect_file_chooser() as chooser:
                    page.locator('#attach-menu [data-attach="image"]').click()
                chooser.value.set_files([
                    {'name': 'one.png', 'mimeType': 'image/png', 'buffer': png},
                    {'name': 'two.png', 'mimeType': 'image/png', 'buffer': png},
                ])
                expect(page.locator('#compose-items .draft-card')).to_have_count(2)
                page.wait_for_function("composerDraft().attachments.every(a => a.uploaded?.upload_id && !a.staging)", timeout=15000)
                page.locator('#cinput').fill('Please inspect both images')
                page.evaluate('async () => await composerDraftWrites')
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send', timeout=30000) as sent:
                    page.locator('#csend').click()
                assert sent.value.status == 200 and sent.value.json()['state'] == 'sent', sent.value.text()
                payload = sent.value.request.post_data_json
                assert payload['text'] == 'Please inspect both images' and len(payload['attachments']) == 2, payload
                page.wait_for_function('() => !composerSending')
                expect(page.locator('#cinput')).to_have_value('')
                assert not dialogs and not errors, (dialogs, errors)
                published = list((root / 'work/sessiondock_attachments').glob('*/*.png'))
                assert {path.name for path in published} == {'one.png', 'two.png'}, published
                assert all(path.read_bytes() == png for path in published), published
                page.locator('#a-term').click()
                xterm_includes(page, '> Please inspect both images')
                xterm_includes(page, '附件2:')
                page.locator('#a-term').click()
                expect(page.locator('#composer')).to_be_visible()
                page.locator('#cadd').click()
                with page.expect_file_chooser() as chooser:
                    page.locator('#attach-menu [data-attach="file"]').click()
                chooser.value.set_files([{'name': 'notes.txt', 'mimeType': 'text/plain',
                    'buffer': b'text attachment bytes'}])
                page.wait_for_function("composerDraft().attachments.length === 1 && composerDraft().attachments[0].uploaded?.upload_id")
                page.locator('#cinput').fill('Please read the text file')
                page.evaluate('async () => await composerDraftWrites')
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send', timeout=30000) as text_sent:
                    page.locator('#csend').click()
                assert text_sent.value.status == 200 and text_sent.value.json()['state'] == 'sent', text_sent.value.text()
                page.wait_for_function('() => !composerSending')
                note_paths = list((root / 'work/sessiondock_attachments').glob('*/notes.txt'))
                assert len(note_paths) == 1 and note_paths[0].read_bytes() == b'text attachment bytes', note_paths
                page.locator('#a-term').click()
                xterm_includes(page, '> Please read the text file')
                (root / 'footer-paste').touch()
                for index, description in enumerate(['没回车\n' * 8, '多段落任务没有提交\n' + '这是用于覆盖长文本折叠占位符的诊断描述。' * 20, '更新退出后保留报告']):
                    # Submit an actual report through the dialog. The worker must
                    # consume its whole task once without a manual terminal Enter.
                    if not page.locator('#report-bug').is_visible():
                        page.locator('#header-more-btn').click()
                    page.locator('#report-bug').click()
                    page.locator('#bug-report-description').fill(description)
                    with page.expect_response(lambda r: urlsplit(r.url).path == '/api/bug-report') as report:
                        page.locator('#bug-report-go').click()
                    assert report.value.status == 202, report.value.text()
                    worker = report.value.json()['worker']
                    bundle = Path(report.value.json()['path'])
                    if index == 2:
                        stopped = context.request.post(base + '/api/term/kill', data={
                            'record_id': worker['record_id'], 'instance_id': worker['instance_id']})
                        assert stopped.status == 200, stopped.text()
                        page.locator('#bug-report-toast').get_by_role('button', name='打开', exact=True).click()
                        expect(page.get_by_role('button', name='重新启动', exact=True)).to_be_visible(timeout=15000)
                        expect(page.locator('#cinput')).to_have_value(description)
                        # Reload exercises recovery from server-owned report input.
                        page.reload(wait_until='networkidle')
                        page.locator('#side .item').filter(has_text='处理 ' + report.value.json()['report_id']).click()
                        expect(page.locator('#cinput')).to_have_value(description)
                        with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/restart') as recovered:
                            page.get_by_role('button', name='重新启动', exact=True).click()
                        assert recovered.value.status == 200, recovered.value.text()
                        worker = recovered.value.json()
                        page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'", timeout=15000)
                        with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send') as resent:
                            page.locator('#csend').click()
                        assert resent.value.status == 200, resent.value.text()
                    deadline = time.monotonic() + 15
                    while time.monotonic() < deadline:
                        manifest = json.loads((bundle / 'manifest.json').read_text())
                        if manifest['status'] in ('submitted', 'failed'):
                            break
                        page.wait_for_timeout(100)
                    assert manifest['status'] == 'submitted', manifest
                    if index == 0:
                        assert (root / 'footer-paste.scrolled').exists(), 'exercise a genuinely clipped prompt marker'
                    submissions = [json.loads(line)['text'] for line in (root / 'submissions.jsonl').read_text().splitlines()]
                    assert len(submissions) == 5 + index, submissions
                    worker_prompt = (bundle / 'worker-prompt.md').read_text()
                    assert (len(worker_prompt) > 1000) == (index == 1), 'cover expanded and collapsed reports'
                    assert submissions[-1] == worker_prompt
                    assert '随后立即 push' in worker_prompt
                    assert 'python3 deploy/deploy.py deploy --all' in worker_prompt
                    assert '无需再次确认' in worker_prompt
                    assert '不要 push' not in worker_prompt and '不要部署' not in worker_prompt
                    assert not dialogs and not errors, (dialogs, errors)
                    context.request.post(base + '/api/term/kill', data={
                        'record_id': worker['record_id'], 'instance_id': worker['instance_id']})
                    worker = None
            finally:
                if worker and context:
                    context.request.post(base + '/api/term/kill', data={
                        'record_id': worker['record_id'], 'instance_id': worker['instance_id']})
                if receipt and context:
                    context.request.post(base + '/api/term/kill', data={
                        'record_id': receipt['record_id'], 'instance_id': receipt['instance_id']})
                if context:
                    context.close()
                browser.close()
    print('PASS Codex browser: startup update override, empty-session restart, report exit/reload/restart, startup wait, attachments, expanded/collapsed report; exactly one submission each')


if __name__ == '__main__':
    main()
