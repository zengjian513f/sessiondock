#!/usr/bin/env python3
"""Cancelled SDK receipts retain server drafts after lost HTTP acknowledgments.

Chromium clicks SEND and cancel against a private node/Hub and fake app-server.
No real CLI, production directories or external requests are used.
"""
from __future__ import annotations

import argparse
import base64
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, build, server
from codex_sdk_compose_browser import attach, saved
from codex_sdk_queue_browser import QueueServer, open_page
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from media_browser import PNG
from node_auth_fixtures import TOKEN, node_env


def exercise(browser, base, fake, width, theme, screenshots, label):
    context = browser.new_context(viewport={'width': width, 'height': 900 if width > 500 else 844},
                                  color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(15000)
    errors, submissions = [], []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.on('request', lambda request: submissions.append(request.post_data_json)
            if request.method == 'POST' and urlsplit(request.url).path.endswith('/api/codex-sdk/turn') else None)
    try:
        page.goto(base, wait_until='domcontentloaded')
        page.locator('#side .item').filter(has_text='earlier synthetic question').first.click()
        uid = page.evaluate('S.sel')
        box, send = page.locator('#sdk-input'), page.locator('#sdk-send')
        expect(box).to_be_enabled()
        box.fill('Hold this synthetic turn while checking cancellation recovery')
        send.click()
        expect(box).to_have_value('')
        expect(send).to_have_text('排队')

        text = '撤销后保留完整草稿，等待用户明确重发 · realistic-project/analysis-report ' + label
        quote = '保留引用：do not silently replace this research context or submit an empty prompt.'
        box.fill(text)
        attach(page, 'image', '屏幕截图-long-research-context.png', 'image/png', base64.b64decode(PNG))
        attach(page, 'file', 'analysis-input-研究笔记.txt', 'text/plain', b'private cancellation fixture\n')
        page.locator('#sdk-add').click()
        page.locator('#sdk-attach-menu [data-attach="quote"]').click()
        page.get_by_role('textbox', name='引用文字').fill(quote)
        saved(page, uid, text)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(3)
        attachment_metadata = """() => composerDraft(S.sel).attachments.map(item => ({
            id:item.id, kind:item.kind, name:item.file.name, upload_id:item.uploaded.upload_id}))"""
        original_files = page.evaluate(attachment_metadata)
        assert [item['kind'] for item in original_files] == ['image', 'file']

        def lose_reply(route):
            response = route.fetch()
            assert response.ok, response.text()
            route.abort('failed')

        page.route('**/api/codex-sdk/turn', lose_reply, times=1)
        send.click()
        expect(page.locator('.sdk-queued')).to_contain_text(text)
        expect(page.locator('.sdk-status')).to_contain_text('发送失败')
        expect(box).to_have_value(text)
        cancelled_id = submissions[-1]['request_id']
        original = submissions[-1]
        saved(page, uid, text)

        # The server-owned draft must retain the uncertain ID after reload.
        page.reload(wait_until='domcontentloaded')
        expect(box).to_have_value(text)
        expect(page.get_by_role('textbox', name='引用文字')).to_have_value(quote)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(3)
        # SEND already published the image and removes its staged bytes. The
        # shared composer intentionally displays its kind icon after reload.
        assert page.evaluate(attachment_metadata) == original_files
        page.wait_for_function('id => composerDraft(S.sel).sdkSubmission?.id === id', arg=cancelled_id)
        page.route('**/api/codex-sdk/queue', lose_reply, times=1)
        page.locator('.sdk-queued [data-queue-action="cancel"]').click()
        expect(page.locator('.sdk-queued')).to_have_count(0)
        expect(page.locator('.sdk-status')).to_contain_text('队列操作失败')
        expect(box).to_have_value(text)
        page.reload(wait_until='domcontentloaded')
        expect(box).to_have_value(text)
        expect(page.locator('.sdk-queued')).to_have_count(0)
        page.wait_for_function('id => composerDraft(S.sel).sdkSubmission?.id === id', arg=cancelled_id)
        assert len(fake.methods('turn/start')) == 1 and not fake.methods('turn/steer')

        # A successful HTTP response reporting a cancelled receipt cannot
        # consume any text, staged file or quote, or replay the request.
        with page.expect_response('**/api/codex-sdk/turn') as cancelled:
            send.click()
        assert cancelled.value.json()['raw']['state'] == 'cancelled'
        expect(page.locator('.sdk-status')).to_contain_text('该消息已撤销，草稿已保留')
        expect(box).to_have_value(text)
        expect(page.get_by_role('textbox', name='引用文字')).to_have_value(quote)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(3)
        assert page.evaluate(attachment_metadata) == original_files
        assert submissions[-1]['request_id'] == cancelled_id
        assert submissions[-1]['attachments'] == original['attachments']
        assert submissions[-1]['quotes'] == original['quotes']
        assert len(submissions) == 3, submissions
        saved(page, uid, text)
        page.wait_for_function('!composerDraft(S.sel).sdkSubmission')
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-cancelled-draft.png'))
        page.locator('.sdk-status').click()
        expect(page.locator('#sdk-status-menu')).to_be_visible()
        expect(page.locator('#sdk-status-menu')).to_contain_text('该消息已撤销，草稿已保留；请再次发送。')
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-cancelled-notice.png'))
        page.keyboard.press('Escape')

        # Persist the cleared identity across another reload, then let only
        # this explicit user click create and consume a new submission.
        page.reload(wait_until='domcontentloaded')
        expect(box).to_have_value(text)
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(3)
        assert len(submissions) == 3
        with page.expect_response('**/api/codex-sdk/turn') as resent:
            send.click()
        assert resent.value.json()['raw']['state'] == 'queued'
        assert submissions[-1]['request_id'] != cancelled_id
        assert submissions[-1]['attachments'] == original['attachments']
        assert submissions[-1]['quotes'] == original['quotes']
        expect(box).to_have_value('')
        expect(page.locator('#sdk-compose-items .draft-card')).to_have_count(0)
        expect(page.locator('.sdk-queued')).to_have_count(1)
        saved(page, uid, '')
        page.reload(wait_until='domcontentloaded')
        expect(box).to_have_value('')
        expect(page.locator('.sdk-queued')).to_have_count(1)
        assert len(fake.methods('turn/start')) == 1 and not fake.methods('turn/steer')
        fake.complete()
        expect(page.locator('.sdk-live')).to_contain_text(text)
        expect(page.locator('.sdk-queued')).to_have_count(0)
        native = fake.methods('turn/start')[-1]['params']['input']
        prompt = '\n'.join(item.get('text', '') for item in native)
        assert text in prompt and quote in prompt and 'analysis-input-研究笔记.txt' in prompt
        assert all(item['type'] == 'text' for item in native), native
        assert 'sessiondock_attachments' in prompt and '.png' in prompt, prompt
        assert len(submissions) == 4 and len(fake.methods('turn/start')) == 2
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-resent.png'))
        assert not errors, errors
        print(f'PASS {label}: lost submit/cancel replies, server-draft reload, cancelled receipt retained, '
              'explicit fresh-ID resend, text/files/image/quote preserved, no automatic replay', flush=True)
    except BaseException:
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-failure.png'))
        raise
    finally:
        context.close()


def legacy_attachment_queue(browser, binary):
    """Retry a persisted pre-change image entry using only its published path text."""
    with tempfile.TemporaryDirectory(prefix='sd-sdk-legacy-attachment-') as temporary:
        root = Path(temporary)
        corpus = build(root)
        env = environment(root, binary)
        image = root / 'sessiondock_attachments' / '1' / 'legacy-image.png'
        image.parent.mkdir(parents=True)
        image.write_bytes(base64.b64decode(PNG))
        text = 'Review the saved attachment\n附件1: ./sessiondock_attachments/1/legacy-image.png'
        (root / 'state' / 'codex-sdk-queue.json').write_text(json.dumps({THREAD: [{
            'id': 'legacy-image', 'text': text,
            'input': [{'type': 'text', 'text': text}, {'type': 'localImage', 'path': str(image)}],
            'model': None, 'effort': None, 'state': 'failed', 'error': 'Previous service stopped',
        }]}))
        with QueueServer(root / 'app.sock', corpus.paths['sdk'], THREAD) as fake:
            with server(corpus, binary, root / 'app.sock', env) as base:
                context, page = open_page(browser, base, 390, 'dark')
                row = page.locator('.sdk-queued').filter(has_text='legacy-image.png')
                expect(row).to_be_visible()
                row.locator('[data-queue-action="retry"]').click()
                expect(page.locator('.sdk-live')).to_contain_text('Review the saved attachment')
                native = fake.methods('turn/start')[-1]['params']['input']
                assert native == [{'type': 'text', 'text': text}], native
                assert image.is_file()
                fake.complete()
                expect(page.locator('.sdk-queued')).to_have_count(0)
                context.close()
        print('PASS old queued image retry retains attachment path and sends text only', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    hub_binary = args.hub_binary or args.binary.with_name('sessiondock-hub')
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            legacy_attachment_queue(browser, args.binary)
            for mode, width, theme in [('node', 1280, 'light'), ('node', 390, 'dark'),
                                       ('hub', 1280, 'dark'), ('hub', 390, 'light')]:
                with tempfile.TemporaryDirectory(prefix='sd-sdk-cancel-') as temporary:
                    root = Path(temporary)
                    corpus = build(root)
                    work = root / 'work-研究-project'
                    work.mkdir()
                    rows = [json.loads(line) for line in corpus.paths['sdk'].read_text().splitlines()]
                    rows[0]['payload']['cwd'] = str(work)
                    corpus.paths['sdk'].write_text(''.join(json.dumps(row, ensure_ascii=False) + '\n' for row in rows))
                    port = free_port()
                    env = {**environment(root, args.binary), **node_env(root, port, '127.0.0.0/8')}
                    with QueueServer(root / 'app.sock', corpus.paths['sdk'], THREAD) as fake:
                        with server(corpus, args.binary, root / 'app.sock', env) as base:
                            hub = None
                            if mode == 'hub':
                                hub_root = root / 'hub'
                                hub_root.mkdir()
                                hub = Hub(hub_binary.resolve(), hub_root,
                                          [SimpleNamespace(name='CancelNode', token=TOKEN, port=port)])
                                hub.start()
                            try:
                                exercise(browser, f'http://127.0.0.1:{hub.port}' if hub else base,
                                         fake, width, theme, args.screenshots, f'{mode}-{width}-{theme}')
                            finally:
                                if hub:
                                    hub.stop()
        finally:
            browser.close()


if __name__ == '__main__':
    main()
