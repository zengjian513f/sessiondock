#!/usr/bin/env python3
"""Native model evidence survives SDK opening without changing CLI defaults."""
import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from agy_model_browser import pictures, selected
from history_fixtures import BINARY, REPO
from native_sdk_queue_browser import open_page, send
from native_sdk_queue_fixtures import NativeQueueFixture
from sdk_ui_fixtures import start_sdk


def open_other(browser, base, sid):
    context = browser.new_context(viewport={'width': 1912, 'height': 900}, service_workers='block')
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=sid)
    uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', sid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.locator('#a-sdk').click()
    # An idle SDK view is read-only with a click-to-start input; start it
    # explicitly like the other SDK suites.
    start_sdk(page)
    expect(page.locator('#sdk-input')).to_be_enabled()
    return context, page, uid


def codex(browser, root, binary, screenshots):
    from codex_sdk_browser import build, server, THREAD
    from codex_sdk_fixtures import FakeAppServer
    root.mkdir()
    corpus = build(root)
    with corpus.paths['sdk'].open('a') as transcript:
        transcript.write(json.dumps({'type': 'turn_context', 'payload': {'model': 'fake-sol'}}) + '\n')
    socket = root / 'app-server.sock'
    with FakeAppServer(socket, corpus.paths['sdk'], THREAD) as native, server(corpus, binary, socket) as base:
        context, page, _ = open_other(browser, base, THREAD)
        try:
            expect(page.locator('#sdk-model')).to_have_value('fake-sol')
            expect(page.locator('.dhead')).to_contain_text('fake-sol')
            native.set_settings('fake-private-model', 'xhigh')
            expect(page.locator('#sdk-model')).to_have_value('fake-private-model')
            page.locator('#sdk-model').select_option('fake-luna')
            page.wait_for_function('CodexSdkView.summary?.settings?.model === "fake-luna"')
            assert any(row['params'].get('model') == 'fake-luna'
                       for row in native.methods('thread/settings/update'))
            page.reload(wait_until='domcontentloaded')
            expect(page.locator('#sdk-model')).to_have_value('fake-luna')
            assert not native.methods('turn/start')
            if screenshots:
                screenshots.mkdir(parents=True, exist_ok=True)
                page.screenshot(path=str(screenshots / 'codex-confirmed.png'), full_page=True)
        finally:
            context.close()


def opencode(browser, root, binary, host_binary, screenshots):
    from opencode_sdk_fixtures import FakeOpenCode, SID
    from history_fixtures import Corpus, isolated_server
    from send_browser import initialize
    for name in ('claude', 'codex', 'grok', 'host', 'state', 'proc'):
        (root / name).mkdir(parents=True)
    (root / 'proc/stat').write_text('btime 1700000000\n')
    with FakeOpenCode(root) as native:
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(host_binary.resolve(strict=True)),
            'host_dir': str(root / 'host'), 'profiles': [native.profile()]}))
        initialize('--initialize-lifecycle', root / 'ledger', binary)
        env = {'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_OPENCODE_DB': str(native.db),
               'SESSIONDOCK_OPENCODE_ROOT': str(root / 'mirror'), 'SESSIONDOCK_PROC_ROOT': str(root / 'proc')}
        with isolated_server(Corpus(root), binary, host_dir=root / 'host', state_dir=root / 'state',
                             lifecycle_dir=root / 'ledger', launcher_config=launcher, extra_env=env) as (base, _):
            context, page, uid = open_other(browser, base, SID)
            try:
                expect(page.locator('#sdk-model')).to_have_value('synthetic/fake-opencode')
                expect(page.locator('.dhead')).to_contain_text('fake-opencode')
                with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/settings')) as response:
                    page.locator('#sdk-effort').select_option('high')
                assert response.value.ok, response.value.text()
                expect(page.locator('#sdk-effort')).to_be_enabled()
                expect(page.locator('#sdk-effort')).to_have_value('high')
                page.reload(wait_until='domcontentloaded')
                expect(page.locator('#sdk-model')).to_have_value('synthetic/fake-opencode')
                expect(page.locator('#sdk-effort')).to_have_value('high')
                assert not any(path.endswith('/prompt') for _, path, _ in native.received)
                pictures(page, uid, screenshots, 'opencode-confirmed')
            finally:
                context.close()


def exercise(browser, fixture, screenshots, *, same_model=False):
    source = fixture.source
    if source == 'claude':
        rows = [json.loads(line) for line in fixture.peer.path.read_text().splitlines()]
        rows[-1]['message']['model'] = 'fake-claude' if same_model else 'previous-claude'
        # API errors and sidechains are not evidence of the main session model.
        error = {**rows[-1], 'uuid': 'synthetic-error', 'message': {**rows[-1]['message'], 'model': '<synthetic>'}}
        sidechain = {**rows[-1], 'uuid': 'sidechain', 'isSidechain': True, 'message': {**rows[-1]['message'], 'model': 'other-agent'}}
        fixture.peer.path.write_text(''.join(json.dumps(row) + '\n' for row in [*rows, error, sidechain]))
        initial = rows[-1]['message']['model']
        label, current = ('Synthetic Claude' if same_model else initial) + '（上次使用）', 'fake-claude'
    else:
        fixture.peer.capability_marker.write_text(json.dumps({'image': True, 'omit_model_option': True}))
        initial, label, current = 'fake-grok', 'Fake Grok', 'fake-grok'
    with fixture.server() as base:
        context, page, uid = open_page(browser, base, fixture)
        try:
            selected(page, label, initial)
            expect(page.locator('.dhead')).to_contain_text(initial)
            expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_contain_text(initial)
            assert not fixture.inputs(), 'opening the picker must not send a prompt'
            if source == 'claude':
                argv = fixture.peer.frames('spawn')[-1]['args']
                assert not any(arg.startswith('--model') for arg in argv), argv
            pictures(page, uid, screenshots, source + '-restored')
            # A missing catalog must retain the independently observed model.
            page.route('**/api/native-sdk/models', lambda route: route.fulfill(status=503, json={'error': 'fixture catalog unavailable'}))
            page.reload(wait_until='domcontentloaded')
            expect(page.locator('#sdk-input')).to_be_enabled()
            selected(page, initial + ('（上次使用）' if source == 'claude' else ''), initial)
            page.unroute('**/api/native-sdk/models')
            page.reload(wait_until='domcontentloaded')
            expect(page.locator('#sdk-input')).to_be_enabled()
            send(page, 'model evidence probe')
            page.wait_for_function('NativeSdkView.raw && !NativeSdkView.raw.running')
            expect(page.locator('#sdk-model')).to_have_value(current)
            if source == 'grok':
                if not page.locator('#sdk-model').is_visible():
                    page.locator('#sdk-more').click()
                page.locator('#sdk-model').select_option('fake-other')
                selected(page, 'Fake Other', 'fake-other')
                current = 'fake-other'
            else:
                selected(page, 'Synthetic Claude', current)
            pictures(page, uid, screenshots, source + '-confirmed')
            pid = fixture.native_pid()
        finally:
            context.close()
    assert fixture.alive(pid)
    with fixture.server() as base:
        context, page, _ = open_page(browser, base, fixture)
        try:
            expect(page.locator('#sdk-model')).to_have_value(current)
            assert fixture.native_pid() == pid
            assert len(fixture.inputs()) == 1, 'reconnecting must not replay a prompt'
        finally:
            context.close()


def unknown_claude(browser, fixture):
    rows = [json.loads(line) for line in fixture.peer.path.read_text().splitlines()]
    rows[-1]['message']['model'] = '<synthetic>'
    fixture.peer.path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
    with fixture.server() as base:
        context, page, _ = open_page(browser, base, fixture)
        try:
            selected(page, '选择模型')
            assert not fixture.inputs()
            send(page, 'resolve previously unknown model')
            selected(page, 'Synthetic Claude', 'fake-claude')
        finally:
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--host-binary', type=Path, default=REPO / 'target/debug/ptyhost')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-model-') as temp, sync_playwright() as pw:
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        for name, source in [('claude', 'claude'), ('grok', 'grok'), ('same', 'claude'), ('unknown', 'claude')]:
            fixture = NativeQueueFixture(Path(temp) / name, source, args.binary.resolve(strict=True), args.host_binary)
            try:
                if name == 'unknown':
                    unknown_claude(browser, fixture)
                else:
                    exercise(browser, fixture, args.screenshots if name != 'same' else None, same_model=name == 'same')
            finally:
                fixture.close()
        codex(browser, Path(temp) / 'codex', args.binary.resolve(strict=True), args.screenshots)
        opencode(browser, Path(temp) / 'opencode', args.binary.resolve(strict=True), args.host_binary, args.screenshots)
        browser.close()
    print('PASS models: Claude history/runtime/unknown, Grok model state/config, Codex native settings, OpenCode provider/model; catalog failure, reconnect, no override or replay')


if __name__ == '__main__':
    main()
