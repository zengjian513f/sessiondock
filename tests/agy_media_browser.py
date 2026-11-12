#!/usr/bin/env python3
"""Agy typed JSON/audio attachments through node/Hub chat and SDK history.

Synthetic native records and files only; no real CLI, account or production data.
"""
from __future__ import annotations

import argparse
import base64
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
from urllib.parse import urlencode

sys.dont_write_bytecode = True
from playwright.sync_api import expect, sync_playwright
from agy_history_browser import private_environment, record, write_transcript
from agy_sdk_fixtures import AgyFixture, SID, TITLE
from history_fixtures import BINARY, Corpus, encoded, isolated_server
from hub_fixtures import Hub
from media_browser import PNG
from node_auth_fixtures import TOKEN, free_port, node_env
from send_browser import initialize

# 0.15 seconds of synthetic mono silence, MPEG layer III at 8 kHz / 8 kbps.
SILENCE = ('SUQzBAAAAAAAI1RTU0UAAAAPAAADTGF2ZjYwLjE2LjEwMAAAAAAAAAAAAAAA'
           '/+MYxAAAAANIAAAAAExBTUUzLjEwMFVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVMQU1FMy4xMDBVVVVVVVVVVVVV'
           '/+MYxDsAAANIAAAAAFVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV'
           '/+MYxHYAAANIAAAAAFVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV'
           '/+MYxLEAAANIAAAAAFVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV'
           '/+MYxMQAAANIAAAAAFVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVV')


def run(args):
    args.screenshots.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-media-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        fixture = AgyFixture(root, binary.with_name('ptyhost'))
        files = fixture.native / 'brain' / SID / '.user_uploaded'
        files.mkdir(parents=True)
        document = files / ('项目 配置 ' + '长文件名' * 12 + '.json')
        document.write_text('{"project": "agy attachment regression"}\n')
        audio = files / '语音 100% sample.mp3'
        audio.write_bytes(base64.b64decode(SILENCE))
        image = files / '截图.png'
        image.write_bytes(base64.b64decode(PNG))
        missing = files / '缺失附件.json'
        user = record(0, 'USER_INPUT', '检查 JSON 附件和音频结果 agymediauser')
        user['media'] = [{'mime_type': 'application/json', 'uri': str(document)},
                         {'mime_type': 'image/png', 'uri': image.as_uri()}]
        values = [user]
        for index in range(1, 9):
            result = record(index, 'GENERIC', '' if index == 1 else f'音频结果 {index}')
            result['media'] = [{'mime_type': 'audio/mpeg', 'uri': audio.as_uri()}]
            values.append(result)
        values.append(record(9, 'PLANNER_RESPONSE', '附件处理完成 agymediafinal'))
        path = write_transcript(fixture.native, SID, values)
        # Another selected session does not inherit the attachment references.
        corpus = Corpus(root)
        from history_fixtures import claude_row
        corpus.put('unrelated', 'claude', [claude_row('unrelated', 'user', 'u', None, '无附件会话')], [])
        node_port = free_port()
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        try:
            with ExitStack() as stack:
                stack.enter_context(private_environment(root))
                initialize('--initialize-lifecycle', root / 'ledger', binary)
                base, _ = stack.enter_context(isolated_server(
                    corpus, binary, state_dir=root / 'state', host_dir=root / 'host',
                    lifecycle_dir=root / 'ledger',
                    launcher_config=fixture.launcher,
                    extra_env={**fixture.extra_env, **node_env(root, node_port, '127.0.0.0/8')}))
                hub_root = root / 'hub'
                hub_root.mkdir()
                hub = Hub(binary.with_name('sessiondock-hub'), hub_root,
                          [SimpleNamespace(name='AgyMediaNode', token=TOKEN, port=node_port)])
                hub.start()
                stack.callback(hub.stop)
                for label, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
                    for width, theme in [(1280, 'light'), (1280, 'dark'), (472, 'light'), (472, 'dark')]:
                        context = browser.new_context(viewport={'width': width, 'height': 900},
                                                      color_scheme=theme, service_workers='block')
                        context.route('**/*', lambda route: route.continue_()
                                      if route.request.url.startswith(url + '/') else route.abort())
                        page = context.new_page()
                        page.set_default_timeout(15000)
                        errors = []
                        page.on('pageerror', lambda error: errors.append(str(error)))
                        page.goto(url)
                        page.locator('#side .item').filter(has_text=TITLE).click()
                        uid = page.evaluate('S.sel')
                        for mode in ('chat', 'sdk'):
                            page.locator('#a-' + mode).click()
                            surface = '#msgs' if mode == 'chat' else '.native-sdk-pane .sdk-history'
                            expect(page.locator(surface)).to_contain_text('agymediafinal')
                            # SDK uses the same process/tool disclosures in its history pane.
                            for selector in ('.turn-process.folded > .turn-toolbar .fold-toggle',
                                             '.grp.folded > .group-preview > .fold-toggle'):
                                buttons = page.locator(surface + ' ' + selector)
                                while buttons.count():
                                    buttons.first.click()
                            page.screenshot(path=str(args.screenshots / f'{label}-{width}-{theme}-{mode}-full.png'))
                            expect(page.locator(surface + ' .native-file')).to_have_count(9)
                            expect(page.locator('.native-history-warning')).to_have_count(0)
                            expect(page.locator(surface + ' .media-link img')).to_have_count(1)
                            img = page.locator(surface + ' .media-link img')
                            img.scroll_into_view_if_needed()
                            expect(img).to_have_js_property('naturalWidth', 2)
                            detail = page.request.get(url + '/api/messages/' + uid).json()
                            assert detail['meta']['migration_warnings'] == [], detail['meta']
                            assert detail['messages'][0]['text'] == '检查 JSON 附件和音频结果 agymediauser'
                            assert len([m for m in detail['messages'] if m.get('native_files')]) == 9
                            for name, kind in [(document.name, 'json'), (audio.name, 'audio')]:
                                link = page.locator(surface + ' .native-file a').filter(has_text=name).first
                                link.scroll_into_view_if_needed()
                                page.screenshot(path=str(args.screenshots / f'{label}-{width}-{theme}-{mode}-{kind}.png'))
                                with context.expect_page() as opened:
                                    link.click()
                                preview = opened.value
                                if kind == 'json':
                                    expect(preview.locator('#file-content pre')).to_contain_text('agy attachment regression')
                                else:
                                    player = preview.locator('audio')
                                    expect(player).to_be_visible()
                                    player.evaluate('(audio) => { audio.loop = true; }')
                                    player.click(position={'x': 25, 'y': 25})
                                    expect(player).to_have_js_property('paused', False)
                                    expect(player).to_have_js_property('readyState', 4)
                                preview.screenshot(path=str(args.screenshots / f'{label}-{width}-{theme}-{mode}-{kind}-preview.png'))
                                with preview.expect_download() as download:
                                    preview.locator('#file-download').click()
                                assert download.value.failure() is None
                                assert download.value.suggested_filename == name
                                preview.close()
                            page.reload()
                            expect(page.locator(surface)).to_contain_text('agymediafinal')
                        assert not errors, errors
                        assert page.evaluate('document.documentElement.scrollWidth <= innerWidth')
                        context.close()
                        print(f'PASS Agy JSON/audio/image: {label} {width} {theme}; chat/SDK, reload, preview/play/download', flush=True)
                # Append and same-message rewrite must update typed files without
                # changing the original user text or requiring a new selection.
                context = browser.new_context(service_workers='block')
                page = context.new_page()
                page.goto(base)
                page.locator('#side .item').filter(has_text=TITLE).click()
                page.locator('#a-chat').click()
                next_record = record(10, 'USER_INPUT', '')
                next_record['media'] = [{'mime_type': 'application/json', 'uri': missing.as_uri()}]
                with path.open('ab') as stream:
                    stream.write(encoded(next_record))
                link = page.locator('#msgs .native-file a').filter(has_text=missing.name)
                expect(link).to_be_visible()
                with context.expect_page() as opened:
                    link.click()
                expect(opened.value.locator('#file-retry')).to_be_visible()
                expect(opened.value.locator('#file-content')).to_contain_text('不存在')
                opened.value.close()
                next_record['media'][0]['uri'] = document.as_uri()
                write_transcript(fixture.native, SID, values + [next_record])
                expect(link).to_have_count(0)
                expect(page.locator('#msgs .msg[data-role=user]').last.locator('.native-file a')).to_contain_text(document.name)
                unrelated = corpus.uid('unrelated')
                response = page.request.get(base + '/api/session/file?' + urlencode({'uid': unrelated, 'ref': str(document)}))
                assert response.status == 404 and response.json()['code'] == 'file_not_referenced', response.text()
                malformed = record(11, 'GENERIC', '无法解析的附件元数据仍需提示')
                malformed['media'] = [{'mime_type': 'audio/mpeg'}]
                with path.open('ab') as stream:
                    stream.write(encoded(malformed))
                expect(page.locator('.native-history-warning')).to_contain_text('Agy 媒体类型：audio/mpeg')
                context.close()
                print('PASS append, attachment-only message, URI rewrite, missing file and unrelated-session denial', flush=True)
        finally:
            browser.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-agy-media-review'))
    run(parser.parse_args())
