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
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'codex-cli-v1', 'source': 'codex',
                'executable': str(Path(sys.executable).resolve()),
                'args': [str(REPO / 'tests/fake_codex_cli.py'), '--model', 'gpt-5.6-luna'],
                'new_args': [], 'resume_args': ['resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'),
                    'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                    'SESSIONDOCK_TEST_ANIMATED_PADDING': '1',
                    'SESSIONDOCK_TEST_FOOTERLESS_PASTE': '1',
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
                page.wait_for_function("composerDraft()?.inputStatus?.state === 'ready'", timeout=15000)
                page.locator('#cinput').fill('report task\n\nfirst paragraph\n\n最后一段\n')
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/session/conversation/send', timeout=15000) as multiline:
                    page.locator('#cinput').press('Enter')
                assert multiline.value.status == 200, multiline.value.text()
                page.locator('#a-term').click()
                xterm_includes(page, '> report task')
                page.locator('#a-term').click()
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
                # Submit an actual report through the dialog. The worker must
                # consume its whole task once without a manual terminal Enter.
                if not page.locator('#report-bug').is_visible():
                    page.locator('#header-more-btn').click()
                page.locator('#report-bug').click()
                page.locator('#bug-report-description').fill('多段落任务没有提交')
                with page.expect_response(lambda r: urlsplit(r.url).path == '/api/bug-report') as report:
                    page.locator('#bug-report-go').click()
                assert report.value.status == 202, report.value.text()
                worker = report.value.json()['worker']
                bundle = Path(report.value.json()['path'])
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    manifest = json.loads((bundle / 'manifest.json').read_text())
                    if manifest['status'] in ('submitted', 'failed'):
                        break
                    page.wait_for_timeout(100)
                assert manifest['status'] == 'submitted', manifest
                submissions = [json.loads(line)['text'] for line in (root / 'submissions.jsonl').read_text().splitlines()]
                assert len(submissions) == 4, submissions
                assert submissions[-1] == (bundle / 'worker-prompt.md').read_text()
                assert not dialogs and not errors, (dialogs, errors)
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
    print('PASS Codex browser: text plus two images, then text plus .txt; each published and submitted with Enter')


if __name__ == '__main__':
    main()
