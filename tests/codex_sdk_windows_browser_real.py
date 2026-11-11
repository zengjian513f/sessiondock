#!/usr/bin/env python3
"""Windows managed Codex listeners through real Chromium and native CLI.

Start codex_sdk_windows_fixtures.py on the target in a temporary home, expose
only its loopback HTTP port through SSH, and supply that URL and a JSON argv
for the fixture controller. No model input is sent; native settings must be
Luna/low. All Rust artifacts are built on the development build machine.
"""
import argparse
import json
import os
import subprocess

from playwright.sync_api import expect, sync_playwright
from new_session_model_browser import open_dialog
from sdk_ui_fixtures import start_sdk
from popups import on_popup


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', default=os.environ.get('SESSIONDOCK_CODEX_WINDOWS_BASE'))
    parser.add_argument('--driver', default=os.environ.get('SESSIONDOCK_CODEX_WINDOWS_DRIVER'), help='JSON argv prefix for the Windows fixture controller')
    parser.add_argument('--root', default=os.environ.get('SESSIONDOCK_CODEX_WINDOWS_ROOT'))
    args = parser.parse_args()
    if not all((args.base, args.driver, args.root)):
        print('SKIP native Windows Codex: requires a private Windows fixture and SSH loopback URL (--base/--driver/--root)')
        return
    driver = json.loads(args.driver)

    def control(mode):
        result = subprocess.run(driver + [mode, '--root', args.root], capture_output=True, text=True, timeout=90)
        assert result.returncode == 0, result.stderr + result.stdout
        return json.loads(result.stdout)

    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            page = browser.new_page(viewport={'width': 1280, 'height': 900})
            on_popup(page, lambda dialog: dialog.accept())
            for run in range(2):
                page.goto(args.base, wait_until='domcontentloaded')
                for label, provider in [('Windows primary provider fixture', 'openai'),
                                        ('Windows secondary provider fixture', 'openai_catalog_cache')]:
                    page.locator('#side .item').filter(has_text=label).first.click()
                    start_sdk(page)
                    page.wait_for_function('CodexSdkView.connected', timeout=60000)
                    summary = page.evaluate('CodexSdkView.summary')
                    assert summary['resume']['modelProvider'] == provider, summary
                    assert summary['settings']['model'] == 'gpt-6-luna', summary
                    assert summary['settings']['effort'] == 'low', summary
                    expect(page.locator('.sdk-history')).to_contain_text('Windows synthetic preserved answer')
                    page.reload(wait_until='domcontentloaded')
                    page.wait_for_function('CodexSdkView.connected', timeout=30000)
                snapshot = control('snapshot')['listeners']
                assert {r['provider'] for r in snapshot} == {'openai', 'openai_catalog_cache'}, snapshot
                assert all(r['url'].startswith('ws://127.0.0.1:') and r['pid'] for r in snapshot)
                if run == 0:
                    original = {r['provider']: (r['pid'], r['started'], r['url']) for r in snapshot}
                    control('restart')
                else:
                    assert {r['provider']: (r['pid'], r['started'], r['url']) for r in snapshot} == original
            assert control('snapshot')['private_acl'], 'token files grant access outside current user'
            page.close()
            control('kill-listener')
            # The subscription worker alone must not resurrect a dead daemon.
            page = browser.new_page()
            page.wait_for_timeout(5500)
            after_death = control('snapshot')['listeners']
            assert next(r for r in after_death if r['provider'] == 'openai')['pid'] == original['openai'][0]
            page.goto(args.base, wait_until='domcontentloaded')
            page.locator('#side .item').filter(has_text='Windows primary provider fixture').first.click()
            start_sdk(page)
            restarted = next(r for r in control('snapshot')['listeners'] if r['provider'] == 'openai')
            assert (restarted['pid'], restarted['started']) != original['openai'][:2]
            original['openai'] = (restarted['pid'], restarted['started'], restarted['url'])
            on_popup(page, lambda dialog: dialog.accept())
            # Creation uses the same provider listener. No SEND is performed.
            open_dialog(page)
            page.locator('input[name="new-source"][value="codex"]').check()
            page.locator('#new-model').click()
            page.locator('[data-model-option][title="gpt-6-luna"]').click()
            page.locator('#new-effort').select_option('low')
            page.locator('#new-cwd').fill(args.root.replace('\\', '/') + '/work')
            page.locator('#new-session-go').click()
            expect(page.locator('#new-session-dialog')).to_be_hidden(timeout=30000)
            page.wait_for_function('CodexSdkView.connected')
            summary = page.evaluate('CodexSdkView.summary')
            assert summary['settings']['model'] == 'gpt-6-luna', summary
            assert summary['settings']['effort'] == 'low', summary
            assert summary['resume']['modelProvider'] == 'openai', summary
            assert len(control('snapshot')['listeners']) == 2
            page.locator('#side .item').filter(has_text='Windows primary provider fixture').first.click()
            start_sdk(page)
            page.locator('#a-term').click()
            expect(page.locator('#termpane')).to_be_visible(timeout=30000)
            page.wait_for_function('T.ws?.readyState === WebSocket.OPEN', timeout=30000)
            terminal = control('snapshot')['terminals']
            assert len(terminal) == 1, terminal
            argv = terminal[0]['argv']
            assert argv[argv.index('--remote') + 1] == original['openai'][2], argv
            assert argv[argv.index('--remote-auth-token-env') + 1] == 'SESSIONDOCK_CODEX_APP_SERVER_TOKEN', argv
            assert '01a11d3f-7510-7d20-8f67-546a6975ebef' in argv, argv
            print('PASS native Windows Codex: auto listeners, provider split, history resume, browser reload, service restart with same process/endpoint, dead listener replacement only on explicit start, private token ACL, Luna/low create and shared TUI; no inference')
        finally:
            browser.close()
            control('stop')


if __name__ == '__main__': main()
