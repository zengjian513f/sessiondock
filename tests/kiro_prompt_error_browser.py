#!/usr/bin/env python3
"""Kiro upstream connection errors stay failures, including legacy recovery and Hub."""
from __future__ import annotations

import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import re
import sqlite3
import sys
import tempfile
from types import SimpleNamespace

from kiro_sdk_fixtures import FakeKiroAcp, KiroFixture

ERROR = {
    'code': -32000,
    'message': 'Your connection was interrupted. Please try again in a moment.',
    'data': {'errorType': 'ServerConnectionResetError', 'retryErrorType': 'TRANSIENT'},
}


class ErrorPeer(FakeKiroAcp):
    def run_prompt(self, message):
        text = ' '.join(block.get('text', '') for block in message['params'].get('prompt', []))
        if text == 'CONNECTION_RESET':
            self.append_v3_message('user', text)
            self.update('tool_call', toolCallId=f'work-{message["id"]}', title='Completed work',
                        kind='edit', status='completed', rawOutput={'output': 'Changes applied'})
            self.update('agent_message_chunk', content={'type': 'text', 'text': 'Partial reply before connection reset'})
            self.send({'jsonrpc': '2.0', 'id': message['id'], 'error': ERROR})
            return
        super().run_prompt(message)


def main():
    if '--fake-prompt-error' in sys.argv:
        if 'acp' in sys.argv:
            peer = ErrorPeer()
            for line in sys.stdin:
                if line.strip():
                    peer.handle(json.loads(line))
        else:
            from kiro_sdk_fixtures import main as fixture_main
            sys.argv.remove('--fake-prompt-error')
            fixture_main()
        return

    from playwright.sync_api import expect, sync_playwright
    from codex_sdk_browser import server
    from history_fixtures import BINARY
    from hub_fixtures import Hub, free_port
    from node_auth_fixtures import TOKEN, node_env
    from sdk_ui_fixtures import start_sdk
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-kiro-prompt-error-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        for name in ('claude', 'codex', 'grok', 'state', 'trash'):
            (root / name).mkdir()
        fixture = KiroFixture(root)
        config = json.loads(fixture.launcher.read_text())
        config['profiles'][0]['args'] = [str(Path(__file__).resolve()), '--fake-prompt-error']
        fixture.launcher.write_text(json.dumps(config))
        initialize('--initialize-lifecycle', root / 'ledger', args.binary.resolve())
        port = free_port()
        env = {**fixture.env, **node_env(root, port, '127.0.0.0/8'),
               'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_STATE_DIR': str(root / 'state'),
               'SESSIONDOCK_LIFECYCLE_DIR': str(root / 'ledger'),
               'SESSIONDOCK_TRASH_DIR': str(root / 'trash'), 'SESSIONDOCK_PTYHOST_DIR': str(fixture.host_dir)}
        browser = pw.chromium.launch(headless=True)
        node = ExitStack()
        hub = None
        try:
            base = node.enter_context(server(SimpleNamespace(root=root), args.binary, 'off', env))

            def exercise(base, label, width, theme, start=False, send=True):
                page = browser.new_page(viewport={'width': width, 'height': 900}, color_scheme=theme)
                page.set_default_timeout(20000)
                page.goto(base, wait_until='domcontentloaded')
                page.wait_for_function('T.listLoaded')
                uid = page.wait_for_function(
                    'async sid => {await loadSessions(true); return S.sessions.find(s=>s.sid===sid)?.uid}',
                    arg=fixture.sid).json_value()
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                page.locator('#a-sdk').click()
                if start:
                    start_sdk(page)
                page.wait_for_function('NativeSdkView.connected && !NativeSdkView.readOnly')
                if send:
                    page.locator('#sdk-input').fill('CONNECTION_RESET')
                    page.locator('#sdk-send').click()
                page.wait_for_function('!NativeSdkView.running && NativeSdkView.raw?.error')
                status = page.locator('.sdk-status-row > .sdk-status:visible')
                if send:
                    expect(status).to_have_attribute('title', re.compile('^正在重试：.*' + re.escape(ERROR['message'])))
                else:
                    expect(status).to_have_attribute('title', '错误：' + ERROR['message'])
                raw = page.evaluate('NativeSdkView.raw')
                assert raw['last_turn']['status'] == 'failed', raw['last_turn']
                assert raw['last_turn']['native_error'] == ERROR, raw['last_turn']
                assert raw['last_turn']['turn_id'] is not None
                assert 'stopReason' not in raw['last_turn']
                assert raw['error_kind'] == 'turn' and not raw['cancel_requested']
                assert (raw.get('retry') or {}).get('delay_seconds') == (15 if send else None)
                expect(page.locator('.sdk-live')).to_contain_text('Partial reply before connection reset')
                if send:
                    expect(page.locator('#sdk-stop')).to_be_visible()
                else:
                    expect(page.locator('#sdk-stop')).to_be_hidden()
                expect(page.locator('.sdk-queue [data-queue-id]')).to_have_count(0)
                expect(page.locator('#sdk-send')).to_be_enabled()
                if send:
                    expect(page.locator('#sdk-input')).to_have_value('')
                page.locator('#sdk-input').fill('保留草稿，等待我确认后继续')
                from codex_sdk_compose_browser import saved
                saved(page, uid, '保留草稿，等待我确认后继续')
                if args.screenshots:
                    status.click()
                    page.screenshot(path=str(args.screenshots / f'{label}-{width}-{theme}.png'), full_page=True)
                    page.keyboard.press('Escape')
                count = len(fixture.received('session/prompt'))
                page.reload(wait_until='domcontentloaded')
                page.wait_for_function('NativeSdkView.raw?.last_turn?.status === "failed"')
                expect(page.locator('#sdk-input')).to_have_value('保留草稿，等待我确认后继续')
                assert len(fixture.received('session/prompt')) == count
                assert not fixture.received('session/cancel')
                page.close()
                print('PASS', label, width, theme, 'native error, partial work, draft and no replay', flush=True)

            exercise(base, 'node', 1280, 'light', start=True)
            prompts = len(fixture.received('session/prompt'))
            loads = len(fixture.received('session/load'))
            node.close()
            # Old builds inferred cancellation solely from the error prose.
            for path in (root / 'state/sdk-adapters/kiro').glob('*.sqlite'):
                with sqlite3.connect(path) as db:
                    state = json.loads(db.execute('SELECT state FROM checkpoint WHERE id=1').fetchone()[0])
                    state['last_turn'] = {'status': 'interrupted', 'stopReason': 'cancelled'}
                    state['error_kind'] = 'interrupted'
                    db.execute('UPDATE checkpoint SET state=? WHERE id=1', (json.dumps(state),))
            base = node.enter_context(server(SimpleNamespace(root=root), args.binary, 'off', env))
            exercise(base, 'legacy-restored', 390, 'dark', send=False)
            assert len(fixture.received('session/prompt')) == prompts
            assert len(fixture.received('session/load')) == loads
            hub_root = root / 'hub'
            hub_root.mkdir()
            hub = Hub(args.binary.with_name('sessiondock-hub').resolve(), hub_root,
                      [SimpleNamespace(name='ErrorNode', token=TOKEN, port=port)])
            hub.start()
            for width, theme in ((1280, 'dark'), (390, 'light')):
                exercise(f'http://127.0.0.1:{hub.port}', 'hub', width, theme)
        finally:
            if hub:
                hub.stop()
            node.close()
            browser.close()
            stop_private_sdkhosts(root)


if __name__ == '__main__':
    main()
