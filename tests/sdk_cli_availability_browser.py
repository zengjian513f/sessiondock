#!/usr/bin/env python3
"""Five configured SDKs obey installation probes on node and Hub pages.

Private roots and synthetic version-only CLIs; no real model or account.
Exercise absent files, wrappers returning 126/127, and repaired installations.
All native bridges are configured, including the independent Codex SDK path.
"""
import argparse
from contextlib import closing
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from fake_opencode_composer import SCHEMA
from history_fixtures import BINARY, REPO, Corpus, claude_row, isolated_server
from hub_fixtures import Hub, free_port, scoped
from lifecycle_unavailable_cli_browser import XTERM
from new_session_model_browser import open_dialog
from node_auth_fixtures import TOKEN, node_env
from private_hosts import private_hosts

SOURCES = ('claude', 'codex', 'grok', 'agy', 'opencode')
STATES = ('absent', 'exit126', 'exit127', 'repaired')


def exercise(browser, base, root, uid, screenshots, state, width, theme, node_id=None):
    label = f'{state}-{"hub" if node_id else "node"}-{width}-{theme}'
    print('Running ' + label, flush=True)
    context = browser.new_context(viewport={'width': width, 'height': 900},
                                  color_scheme=theme, service_workers='block')
    page = context.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    try:
        page.goto(base, wait_until='networkidle')
        page.wait_for_function('T.listLoaded')
        row_uid = scoped(node_id, uid) if node_id else uid
        page.locator(f'#side .item[data-uid="{row_uid}"]').click()
        # Preserve history access when every configured AI CLI is missing.
        if state != 'repaired':
            expect(page.locator('#msgs')).to_contain_text('Preserved history without installed CLIs')
        if page.locator('#detail .mobile-back').is_visible():
            page.locator('#detail .mobile-back').click()
        open_dialog(page)
        if node_id:
            page.locator('#new-node').select_option(node_id)
        available = state == 'repaired'
        # Exact keys prove the four native bridges were actually configured;
        # absence of a bridge cannot accidentally make this regression pass.
        cap = page.evaluate('''() => { const cap = newNodeCapabilities();
            return {native_sdks:cap.native_sdks, codex_sdk:cap.codex_sdk, sources:cap.sources}; }''')
        assert set(cap['native_sdks']) == set(SOURCES) - {'codex'}, cap
        assert cap['codex_sdk'] is available, cap
        for source in SOURCES:
            radio = page.locator(f'input[name="new-source"][value="{source}"]')
            assert cap['sources'][source] is available, (source, cap)
            if source != 'codex':
                assert cap['native_sdks'][source] is available, (source, cap)
            if available:
                expect(radio).to_be_enabled()
            else:
                expect(radio).to_be_disabled()
                box = radio.locator('..').bounding_box()
                page.mouse.click(box['x'] + box['width'] / 2, box['y'] + box['height'] / 2)
                expect(radio).not_to_be_checked()
                endpoint = 'codex-sdk' if source == 'codex' else 'native-sdk'
                refused = context.request.post(base + f'/api/{endpoint}/create', data={
                    'source': source, 'cwd': str(root / 'work'), 'request_id': label + '-' + source,
                    **({'_node': node_id} if node_id else {})})
                assert refused.status == 501, (source, refused.text())
                code = 'codex_sdk_disabled' if source == 'codex' else 'native_sdk_disabled'
                assert refused.json()['code'] == code, (source, refused.text())
        page.screenshot(path=str(screenshots / (label + '.png')))
        # The independent shell remains usable even when every AI CLI is gone.
        page.locator('input[name="new-source"][value="shell"]').locator('..').click()
        page.locator('#new-cwd').fill(str(root / 'work'))
        with page.expect_response(lambda response: response.url.endswith('/api/term/create')) as created:
            page.locator('#new-session-go').click()
        assert created.value.status == 200, created.value.text()
        expect(page.locator('#new-session-dialog')).to_be_hidden()
        page.locator('#a-term').click()
        page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
        keyboard = page.locator('#termpane .xterm-helper-textarea')
        keyboard.press_sequentially('printf SDK_; printf AVAILABILITY_OK')
        keyboard.press('Enter')
        page.wait_for_function('(' + XTERM + ")().includes('SDK_AVAILABILITY_OK')")
        assert not errors, errors
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=REPO / 'target/sdk-cli-availability')
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-all-sdk-clis-') as temporary, \
            private_hosts(Path(temporary)), sync_playwright() as pw:
        root = Path(temporary)
        for name in ('host', 'ledger', 'bin', 'home', 'work', 'state', 'claude', 'codex',
                     'grok-home/sessions', 'agy', 'opencode', 'proc', 'home/.gemini/antigravity-cli'):
            (root / name).mkdir(mode=0o700, parents=True, exist_ok=True)
        (root / 'proc/stat').write_text('btime 1700000000\n')
        with closing(sqlite3.connect(root / 'opencode.db')) as db:
            db.executescript(SCHEMA)
        env = {'HOME': str(root / 'home'), 'PATH': '/usr/bin:/bin',
               'GROK_HOME': str(root / 'grok-home'), 'TERM': 'xterm-256color'}
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(binary.parent / 'ptyhost'),
            'host_dir': str(root / 'host'), 'profiles': [
                {'id': source + '-cli', 'source': source, 'executable': str(root / 'bin' / source),
                 'env': env} for source in SOURCES], 'adapters': [
                {'id': 'shell', 'source': 'shell', 'executable': '/bin/sh', 'args': ['-i'], 'env': env}]}))
        subprocess.run([str(binary), '--initialize-lifecycle', str(root / 'ledger')],
                       check=True, capture_output=True, timeout=15)
        corpus = Corpus(root)
        corpus.put('all-clis-missing', 'claude', [claude_row('all-clis-missing', 'user', 'u1', None,
            'Preserved history without installed CLIs')], [])
        node = SimpleNamespace(port=free_port(), name='AllSdkNode', token=TOKEN)
        network = node_env(root, node.port, '127.0.0.0/8')
        network.update(SESSIONDOCK_NATIVE_SDKS='on',
            SESSIONDOCK_CODEX_APP_SERVER=str(root / 'absent-app.sock'),
            SESSIONDOCK_GROK_ROOT=str(root / 'grok-home/sessions'),
            SESSIONDOCK_AGY_HOME=str(root / 'home/.gemini/antigravity-cli'),
            SESSIONDOCK_AGY_ROOT=str(root / 'agy'), SESSIONDOCK_OPENCODE_DB=str(root / 'opencode.db'),
            SESSIONDOCK_OPENCODE_ROOT=str(root / 'opencode'), SESSIONDOCK_PROC_ROOT=str(root / 'proc'))
        options = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**options)
        try:
            for state in STATES:
                if state != 'absent':
                    for source in SOURCES:
                        cli = root / 'bin' / source
                        cli.write_text('#!/bin/sh\n' + (
                            f'case "$1" in --version) echo "{source} 1.2.3"; exit 0;; esac\nexit 2\n'
                            if state == 'repaired' else 'exit ' + state.removeprefix('exit') + '\n'))
                        cli.chmod(0o700)
                with isolated_server(corpus, binary, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                        launcher_config=launcher, state_dir=root / 'state', extra_env=network) as (base, _):
                    node.nid = (root / 'ids/node-id').read_text().strip()
                    hub_root = root / ('hub-' + state)
                    hub_root.mkdir(mode=0o700)
                    hub = Hub(binary.with_name('sessiondock-hub'), hub_root, [node])
                    hub.start()
                    try:
                        for hub_mode, url in ((False, base), (True, f'http://127.0.0.1:{hub.port}')):
                            for width, theme in ((1280, 'light'), (390, 'dark'), (1280, 'dark'), (390, 'light')):
                                exercise(browser, url, root, corpus.uid('all-clis-missing'), args.screenshots,
                                         state, width, theme, node.nid if hub_mode else None)
                    finally:
                        hub.stop()
        finally:
            browser.close()
    print('PASS all five SDKs: absent files, wrapper 126/127, repaired CLI probes, disabled picker and '
          'direct create refusal, preserved history and working shell; node/Hub, restart, desktop/mobile, light/dark')


if __name__ == '__main__':
    main()
