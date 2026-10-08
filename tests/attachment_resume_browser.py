#!/usr/bin/env python3
"""Select a multi-chunk file, lose replies, truncate a chunk, restart and resume.

The browser selects/sends/removes attachments through the real composer. Only
private synthetic bytes and a fake CLI are used, directly and through the Hub.
"""
import argparse
from pathlib import Path
import socket
import tempfile
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit, urlencode
from playwright.sync_api import sync_playwright, expect
from composer_editor_browser import fixture
from frontend_framework_browser import launch_chromium
from history_fixtures import BINARY, Corpus, isolated_server
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from network_fault_fixtures import StalledResponses
from private_hosts import private_hosts
from popups import on_popup
from send_browser import create_claude


def run(browser, binary, hub_mode):
    with tempfile.TemporaryDirectory(prefix='sessiondock-upload-resume-') as tmp, private_hosts(Path(tmp)):
        root = Path(tmp)
        launcher = fixture(root, binary)
        options = dict(host_dir=root/'host', lifecycle_dir=root/'ledger', launcher_config=launcher,
                       state_dir=root/'state', file_roots=(root/'work',), file_write_roots=(root/'work',))
        nid = 'a' * 32
        node = SimpleNamespace(name='upload-fixture', nid=nid, port=free_port(), token=TOKEN)
        if hub_mode:
            ids = root/'ids'; ids.mkdir(); (ids/'node-id').write_text(nid+'\n')
            options['extra_env'] = node_env(root, node.port, '127.0.0.0/8')
        server = isolated_server(Corpus(root), binary, **options)
        base, _ = server.__enter__()
        hub = None
        context = browser.new_context(viewport={'width':390 if hub_mode else 1280, 'height':900})
        try:
            if hub_mode:
                hubroot = root/'hub'; hubroot.mkdir()
                hub = Hub(binary.resolve().with_name('sessiondock-hub'), hubroot, [node])
                hub.start(); base = f'http://127.0.0.1:{hub.port}'
            destination = {'base':base}
            page = context.new_page()
            on_popup(page, lambda dialog: dialog.accept())
            page.goto(base, wait_until='networkidle')
            create_claude(page, base, root/'work', open_terminal=False)
            expect(page.locator('#cinput')).to_be_enabled()
            chunks = []
            restarted = []
            def forward(route):
                nonlocal server
                parsed = urlsplit(route.request.url)
                target = destination['base'] + parsed.path + ('?'+parsed.query if parsed.query else '')
                if parsed.path.endswith(('/api/watch', '/api/events')):
                    route.continue_(); return
                is_upload = parsed.path.endswith('/conversation/attachment') and route.request.method == 'POST'
                if is_upload:
                    query = parse_qs(parsed.query)
                    offset = int(query['offset'][0])
                    body = route.request.post_data_buffer
                    chunks.append((offset, len(body)))
                    if len(chunks) == 2:
                        # Forward only part of this browser upload, with its real
                        # Content-Length. The server must not acknowledge it.
                        where = urlsplit(destination['base'])
                        path = parsed.path + '?' + parsed.query
                        with socket.create_connection((where.hostname, where.port), timeout=5) as conn:
                            conn.sendall((f'POST {path} HTTP/1.1\r\nHost: {where.netloc}\r\n'
                                f'Content-Type: application/octet-stream\r\nContent-Length: {len(body)}\r\n'
                                'Connection: close\r\n\r\n').encode() + body[:65536])
                        route.abort('connectionreset'); return
                    if len(chunks) == 3 and not hub_mode:
                        server.__exit__(None, None, None)
                        server = isolated_server(Corpus(root), binary, **options)
                        destination['base'], _ = server.__enter__()
                        restarted.append(True)
                        target = destination['base'] + parsed.path + '?' + parsed.query
                # The fixture restarted on a new loopback port. Like the real
                # same-origin proxy, rewrite Origin for that private upstream.
                headers = {**route.request.headers, 'origin':destination['base']}
                response = route.fetch(url=target, headers=headers)
                if is_upload:
                    assert response.ok, response.text()
                    if len(chunks) == 1:
                        # The first chunk landed; only its reply is lost.
                        route.abort('connectionreset'); return
                route.fulfill(response=response)
            page.route('**/api/**', forward)
            payload = bytes(range(256)) * (12288 + 17)
            page.locator('#cadd').click()
            with page.expect_file_chooser() as chooser:
                page.locator('#attach-menu [data-attach=file]').click()
            chooser.value.set_files({'name':'resumable.bin', 'mimeType':'application/octet-stream', 'buffer':payload})
            page.wait_for_function('composerDraft()?.attachments[0]?.uploaded?.upload_id && !composerDraft().attachments[0].staging', timeout=45000)
            assert chunks == [(0,2097152),(2097152,len(payload)-2097152),(2097152,len(payload)-2097152)], chunks
            assert bool(restarted) is not hub_mode
            item = page.evaluate('({uid:composerUid,id:composerDraft().attachments[0].uploaded.upload_id})')
            result = context.request.get(destination['base'] + '/api/session/conversation/attachment?' + urlencode(item))
            assert result.ok and result.body() == payload
            with StalledResponses() as faults:
                # A complete upload with a stalled response body must release
                # its lane and discover the receipt without another POST.
                page.evaluate('COMPOSER_UPLOAD_STALL_MS = 800')
                posts = []
                def stalled_reply(route):
                    posts.append(route.request.url)
                    parsed = urlsplit(route.request.url)
                    target = destination['base'] + parsed.path + '?' + parsed.query
                    response = route.fetch(url=target, headers={**route.request.headers, 'origin':destination['base']})
                    assert response.ok, response.text()
                    faults.redirect(route, 'body', response.body())
                page.route('**/api/session/conversation/attachment?*', stalled_reply)
                page.locator('#cadd').click()
                with page.expect_file_chooser() as chooser:
                    page.locator('#attach-menu [data-attach=file]').click()
                chooser.value.set_files({'name':'stalled-ack.bin', 'mimeType':'application/octet-stream', 'buffer':b'ack bytes'})
                page.wait_for_function('composerDraft()?.attachments[1]?.uploaded?.upload_id && !composerDraft().attachments[1].staging', timeout=10000)
                assert len(posts) == 1
                faults.release(); page.unroute('**/api/session/conversation/attachment?*', stalled_reply)
                with page.expect_response(lambda r: r.url.endswith('/api/session/conversation/attachment/discard')):
                    page.locator('#compose-items .draft-card').nth(1).locator('.draft-remove').click()
                page.evaluate('COMPOSER_UPLOAD_STALL_MS = 30000')
            with StalledResponses() as faults:
                cancelled_chunks = []
                def cancel_chunk(route):
                    parsed = urlsplit(route.request.url)
                    offset = int(parse_qs(parsed.query)['offset'][0])
                    cancelled_chunks.append(offset)
                    if len(cancelled_chunks) == 2:
                        faults.redirect(route, 'body', b'{"ok":true}')
                        return
                    target = destination['base'] + parsed.path + '?' + parsed.query
                    response = route.fetch(url=target, headers={**route.request.headers, 'origin':destination['base']})
                    assert response.ok, response.text()
                    route.fulfill(response=response)
                page.route('**/api/session/conversation/attachment?*', cancel_chunk)
                page.locator('#cadd').click()
                with page.expect_file_chooser() as chooser:
                    page.locator('#attach-menu [data-attach=file]').click()
                chooser.value.set_files({'name':'cancel-resume.bin', 'mimeType':'application/octet-stream', 'buffer':payload})
                expect(page.locator('#compose-items .draft-card').nth(1)).to_contain_text('100%', timeout=10000)
                page.locator('#compose-items .draft-card').nth(1).locator('[title="取消上传"]').click()
                expect(page.locator('#compose-items .draft-card').nth(1)).to_contain_text('上传已取消')
                page.wait_for_timeout(3500)
                assert cancelled_chunks == [0,2097152], cancelled_chunks
                assert page.evaluate('composerDraft().attachments[1].uploadRetryAt === undefined')
                page.locator('#compose-items .draft-card').nth(1).locator('.draft-retry').click()
                page.wait_for_function('composerDraft()?.attachments[1]?.uploaded?.upload_id && !composerDraft().attachments[1].staging', timeout=10000)
                assert cancelled_chunks == [0,2097152,2097152], cancelled_chunks
                faults.release(); page.unroute('**/api/session/conversation/attachment?*', cancel_chunk)
                with page.expect_response(lambda r: r.url.endswith('/api/session/conversation/attachment/discard')):
                    page.locator('#compose-items .draft-card').nth(1).locator('.draft-remove').click()
            # Normal SEND still publishes the exact complete file only once.
            page.locator('#cinput').fill('Send the resumed synthetic attachment')
            page.locator('#csend').click()
            expect(page.locator('#cinput')).to_have_value('', timeout=20000)
            published = list((root/'work/claude-area/sessiondock_attachments').glob('*/resumable.bin'))
            assert len(published) == 1 and published[0].read_bytes() == payload
            assert not list((root/'state/conversations/conversation-uploads').iterdir())
            print('PASS', 'Hub' if hub_mode else 'node/restart', 'lost ACK, interrupted chunk, stalled body, cancel/manual resume, exact publication', flush=True)
        finally:
            context.close()
            if hub: hub.stop()
            server.__exit__(None, None, None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with sync_playwright() as pw:
        browser = launch_chromium(pw)
        try:
            for hub_mode in (False, True): run(browser, args.binary, hub_mode)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
