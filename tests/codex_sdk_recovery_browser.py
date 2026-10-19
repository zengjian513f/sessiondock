#!/usr/bin/env python3
"""BUG-20261018-030340-bc6f60: SDK resumes after an independent CLI exits.

Private PTY, fake CLI, scripted native writer-lock rejection, synthetic history.
Chromium stops the CLI through the page and keeps the failed SDK view open.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD, build
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import BINARY, REPO, isolated_server
from popups import on_popup
from private_hosts import private_hosts


class WriterAppServer(FakeAppServer):
    async def _handle(self, writer, message):
        if message.get('method') == 'thread/resume' and not self.exited.exists():
            await self.send(writer, {'id': message['id'], 'error': {
                'code': -32600, 'message': 'thread already has an active writer'}})
            return
        await super()._handle(writer, message)


CLI = '''import os, sys, tty
from pathlib import Path
if sys.argv[1:] == ['--version']:
    print('fake-codex 1.0')
    raise SystemExit
tty.setraw(0)
os.write(1, b'INDEPENDENT_WRITER_READY\\r\\n')
while os.read(0, 1) != b'\\x04':
    pass
Path(os.environ['WRITER_EXITED']).touch()
'''


def check(browser, binary, screenshots, width, theme):
    with tempfile.TemporaryDirectory(prefix='sd-sdk-recovery-') as tmp:
        root = Path(tmp)
        corpus = build(root)
        for name in ['work', 'host', 'ledger', 'state', 'proc']:
            (root / name).mkdir(mode=0o700)
        (root / 'proc/stat').write_text('btime 1700000000\n')
        cli = root / 'cli.py'
        cli.write_text(CLI)
        exited = root / 'exited'
        config = root / 'launcher.json'
        config.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
            'host_dir': str(root / 'host'), 'profiles': [{'id': 'codex-cli', 'source': 'codex',
                'executable': sys.executable, 'args': [str(cli)],
                'env': {'PATH': os.environ['PATH'], 'WRITER_EXITED': str(exited)}}]}))
        config.chmod(0o600)
        # The native history's launch directory must exist in this fixture.
        path = corpus.paths['sdk']
        path.write_text(path.read_text().replace('/synthetic/sdk', str(root / 'work')))
        subprocess.run([str(binary.resolve()), '--initialize-lifecycle', str(root / 'ledger')],
                       check=True, capture_output=True, timeout=15)
        with private_hosts(root), WriterAppServer(root / 'app.sock', path, THREAD) as fake:
            fake.exited = exited
            with isolated_server(corpus, binary, state_dir=root / 'state', host_dir=root / 'host',
                    lifecycle_dir=root / 'ledger', launcher_config=config,
                    extra_env={'SESSIONDOCK_PROC_ROOT': str(root / 'proc'),
                               'SESSIONDOCK_CODEX_APP_SERVER': str(root / 'app.sock')}) as (base, _):
                context = browser.new_context(viewport={'width': width, 'height': 900},
                                              color_scheme=theme, service_workers='block')
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                on_popup(page, lambda dialog: dialog.accept())
                try:
                    page.goto(base, wait_until='networkidle')
                    uid = corpus.uid('sdk')
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator('#sdk-status-menu')).to_contain_text('独立运行')
                    # Start the fixture TUI through the real launch path.
                    page.locator('#a-term').click()
                    page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
                    page.wait_for_function("Array.from({length:T.term.buffer.active.length}, (_,i)=>"
                        "T.term.buffer.active.getLine(i)?.translateToString()||'').join('\\n').includes('INDEPENDENT_WRITER_READY')")
                    page.locator('#a-sdk').click()
                    expect(page.locator('#sdk-status-menu')).to_contain_text('独立运行')
                    expect(page.locator('.sdk-history')).to_contain_text('earlier synthetic answer')
                    draft = '关闭旧会话后保留这条未发送草稿 / keep unsent draft'
                    page.locator('#sdk-input').fill(draft)
                    if screenshots:
                        page.screenshot(path=str(screenshots / f'busy-{width}-{theme}.png'))
                    # Clicking the already-selected SDK must retry immediately.
                    before = len(fake.methods('thread/resume'))
                    page.locator('#a-sdk').click()
                    fake.wait_for(lambda: len(fake.methods('thread/resume')) > before)
                    expect(page.locator('#sdk-input')).to_have_value(draft)
                    if width == 1280:
                        before = len(fake.methods('thread/resume'))
                        page.locator(f'#side .item[data-uid="{uid}"]').click()
                        fake.wait_for(lambda: len(fake.methods('thread/resume')) > before)
                        expect(page.locator('#sdk-input')).to_have_value(draft)
                    action = page.locator('#a-session-toggle')
                    if not action.is_visible():
                        page.locator('#a-more').click()
                    expect(action).to_have_attribute('aria-label', '停止会话')
                    with page.expect_response(lambda r: '/api/session/stop' in r.url) as response:
                        action.click()
                    result = response.value.json()
                    assert result['ok'] and result['stopped'], result
                    assert exited.exists(), 'fake CLI did not exit on Ctrl-D'
                    # No SDK toggle or reload: background open retry must recover.
                    expect(page.locator('#sdk-model')).to_have_value('fake-sol', timeout=12000)
                    expect(page.locator('.sdk-gauge[data-gauge="context"] b')).to_have_text('20%')
                    expect(page.locator('#sdk-status-menu')).not_to_contain_text('无法打开')
                    expect(page.locator('#sdk-input')).to_have_value(draft)
                    expect(page.locator('.sdk-history')).to_contain_text('earlier synthetic answer')
                    assert not fake.methods('turn/start'), 'recovery sent the draft'
                    if screenshots:
                        page.screenshot(path=str(screenshots / f'recovered-{width}-{theme}.png'))
                    page.locator('#sdk-input').fill('approve after writer exit')
                    page.locator('#sdk-send').click()
                    expect(page.locator('.sdk-request')).to_contain_text('需要批准执行命令')
                    page.locator('.sdk-request').get_by_role('button', name='批准', exact=True).click()
                    expect(page.locator('.sdk-history')).to_contain_text('All done after approval')
                    assert len(fake.methods('turn/start')) == 1
                    # Healthy SDK selection must reuse its subscription.
                    before = len(fake.methods('thread/resume'))
                    page.locator('#a-sdk').click()
                    assert len(fake.methods('thread/resume')) == before
                    # Leaving a busy view cancels its scheduled retry.
                    page.locator('#a-chat').click()
                    opens = []
                    def busy_open(route):
                        opens.append(route.request)
                        route.fulfill(status=409, json={'error': 'Synthetic active writer',
                                                      'code': 'codex_sdk_thread_busy'})
                    page.route('**/api/codex-sdk/open', busy_open)
                    page.locator('#a-sdk').click()
                    expect(page.locator('#sdk-status-menu')).to_contain_text('Synthetic active writer')
                    page.locator('#a-chat').click()
                    page.wait_for_timeout(1400)  # Beyond the cancelled first retry's 1s deadline.
                    assert len(opens) == 1, 'closed view retried open'
                    assert not errors, errors
                finally:
                    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for width, theme in [(1280, 'dark'), (390, 'light'), (1280, 'light'), (390, 'dark')]:
                print(f'checking writer-exit recovery {width}px {theme}', flush=True)
                check(browser, args.binary, args.screenshots, width, theme)
        finally:
            browser.close()
    print('PASS writer exit → SDK recovery; manual retry; draft preserved; one explicit send')


if __name__ == '__main__':
    main()
