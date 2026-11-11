#!/usr/bin/env python3
"""Authenticated loopback WS: refusal, recovery, restart and shared TUI argv.

Synthetic CLI/rollouts only. The native Windows companion suite checks the
managed listener; this suite exercises the same HTTP/browser/TUI connection
path on a development host with Chromium.
"""
import argparse
import asyncio
import json
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from codex_sdk_actions_browser import environment
from codex_sdk_browser import THREAD, LoadedAppServer, build, server, open_sdk
from history_fixtures import BINARY
from lifecycle_cli_browser import FAKE_CODEX, xterm_includes
from popups import on_popup
from private_hosts import private_hosts
from sdk_ui_fixtures import start_sdk

TOKEN = 'synthetic-private-codex-capability'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sd-codex-ws-') as temporary:
        root = Path(temporary)
        corpus = build(root)
        work = root / 'work'; work.mkdir()
        corpus.paths['sdk'].write_text(corpus.paths['sdk'].read_text().replace('/synthetic/sdk', str(work)))
        env = environment(root, args.binary)
        cli = root / 'fake-codex'
        cli.write_text(FAKE_CODEX.replace('service-env-tool\n', ''))
        cli.chmod(0o700)
        config = root / 'launcher.json'
        settings = json.loads(config.read_text())
        settings['profiles'][0].update(executable=str(cli), args=[], resume_args=['resume', '{sid}'])
        config.write_text(json.dumps(settings))
        env['SESSIONDOCK_CODEX_APP_SERVER_TOKEN'] = TOKEN
        with private_hosts(root, hosts=('host',)), LoadedAppServer('ws://127.0.0.1:0', corpus.paths['sdk'], THREAD, token=TOKEN) as fake, sync_playwright() as pw:
            browser = pw.chromium.launch(headless=True)
            try:
                wrong = {**env, 'SESSIONDOCK_CODEX_APP_SERVER_TOKEN': 'wrong-synthetic-token'}
                with server(corpus, args.binary, fake.socket_path, wrong) as base:
                    page = browser.new_page()
                    page.goto(base, wait_until='domcontentloaded')
                    page.locator(f'#side .item[data-uid="{corpus.uid("sdk")}"]').click()
                    page.get_by_title('启动 SDK会话', exact=True).click()
                    expect(page.locator('#sdk-status-menu')).to_contain_text('握手失败')
                    assert fake.rejected and not fake.methods('thread/resume')
                    page.close()
                # The same private state can reconnect with the correct token;
                # a restarted web service reattaches, without another process.
                for restart in range(2):
                    with server(corpus, args.binary, fake.socket_path, env) as base:
                        page = browser.new_page()
                        on_popup(page, lambda dialog: dialog.accept())
                        page.goto(base, wait_until='domcontentloaded')
                        open_sdk(page, corpus, expect)
                        start_sdk(page)
                        page.wait_for_function('CodexSdkView.connected')
                        assert fake.methods('thread/resume')
                        if restart == 0:
                            # Drop every bridge connection, then open a fresh page.
                            # No accepted input may be replayed by reconnect.
                            async def disconnect():
                                for writer in list(fake.clients):
                                    writer.close()
                            asyncio.run_coroutine_threadsafe(disconnect(), fake.loop).result(5)
                            page.reload(wait_until='domcontentloaded')
                            page.wait_for_function('CodexSdkView.connected')
                            assert not fake.methods('turn/start')
                        else:
                            page.locator('#a-term').click()
                            expect(page.locator('#termpane')).to_be_visible()
                            xterm_includes(page, '--remote')
                            xterm_includes(page, fake.socket_path)
                            xterm_includes(page, '--remote-auth-token-env')
                            xterm_includes(page, 'SESSIONDOCK_CODEX_APP_SERVER_TOKEN')
                            xterm_includes(page, THREAD)
                            xterm_includes(page, 'RS_SHELL_READY')
                            terminal = page.locator('#termpane .xterm-helper-textarea:visible')
                            terminal.press_sequentially('same listener input')
                            terminal.press('Enter')
                            xterm_includes(page, 'RS_INPUT_OK')
                        page.close()
            finally:
                browser.close()
    print('PASS Codex WS: auth refusal/recovery, browser reconnect without replay, service restart, shared terminal endpoint/token argv')


if __name__ == '__main__':
    main()
