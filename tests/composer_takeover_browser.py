"""Explicit footer takeover: real private external processes, CLI host and fake SDK."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import uuid

from playwright.sync_api import sync_playwright, expect
from codex_sdk_browser import build, server, THREAD
from codex_sdk_actions_browser import environment
from sdk_start_stop_fixtures import StartDaemon
from history_fixtures import BINARY, REPO
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from private_hosts import private_hosts
from session_start_fixtures import choose_mode
from terminal_browser import SHELL_SCRIPT


def external_process(sid, cwd):
    # The private Python sleeper has Codex's argv identity but runs no real CLI.
    return subprocess.Popen(['codex', '-c', 'import time; time.sleep(180)', '--session-id', sid],
                            executable=sys.executable, cwd=cwd,
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def exercise(browser, base, mode, label, root, sid, daemon, shots):
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        print('takeover', label, mode, width, flush=True)
        child = external_process(sid, root/'work')
        unrelated = external_process(str(uuid.uuid4()), root/'work')
        context = browser.new_context(viewport={'width':width, 'height':900}, color_scheme=theme,
                                      service_workers='block', has_touch=width < 720)
        page = context.new_page(); page.set_default_timeout(20000)
        mutations, errors = [], []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.on('request', lambda request: mutations.append((request.url.rsplit('/',1)[-1], request.post_data_json))
                if request.method == 'POST' and any(request.url.endswith('/'+path) for path in
                    ('stop','takeover','open','turn')) else None)
        try:
            page.goto(base, wait_until='networkidle')
            page.wait_for_function('T.listLoaded')
            choose_mode(page, 'codex', 'cli' if mode == 'sdk' else 'sdk')
            page.locator('#settings-dialog .modal-close').click()
            card = page.locator('#side .item[data-uid^="codex:"]').first
            uid = card.get_attribute('data-uid')
            page.evaluate('refreshLive(true)')
            card.click()
            page.locator('#a-sdk' if mode == 'sdk' else '#a-chat').click()
            footer = page.locator('#sdk-composer' if mode == 'sdk' else '#conversation-status')
            editor = page.locator('#sdk-input' if mode == 'sdk' else '#cinput')
            expect(footer.locator('.sdk-status')).to_have_text('正在外部CLI运行')
            action = footer.locator('.composer-run-action')
            expect(action).to_have_text('接管')
            expect(action).to_be_enabled()
            draft = f'保留当前草稿，接管后不自动发送 {mode} {label} {width}'
            editor.click(); editor.fill(draft); editor.click()
            expect(editor).to_have_value(draft)
            assert not mutations, mutations
            page.screenshot(path=str(shots/f'{label}-{mode}-{width}-external.png'))
            action.click()
            dialog = page.locator('dialog.app-popup[open]')
            expect(dialog).to_contain_text(f'以 {mode.upper()} 模式重新启动')
            expect(action).to_be_disabled()
            assert not mutations and child.poll() is None and unrelated.poll() is None
            page.screenshot(path=str(shots/f'{label}-{mode}-{width}-confirm.png'))
            dialog.locator('[data-popup-action="cancel"]').click()
            expect(action).to_be_enabled()
            assert not mutations and child.poll() is None
            expect(editor).to_have_value(draft)
            if mode == 'sdk' and label == 'node' and width == 1280:
                for response in ({'error':'Synthetic stop failure'}, {'ok':True,'stopped':False,'stage':'uncertain'}):
                    page.route('**/api/session/stop', lambda route, *, response=response:
                               route.fulfill(status=503 if 'error' in response else 200,json=response),times=1)
                    action.click(); dialog.locator('[data-popup-action="ok"]').click()
                    expect(dialog).to_contain_text('接管失败')
                    assert child.poll() is None
                    assert not any(path == 'open' for path, _ in mutations), mutations
                    expect(editor).to_have_value(draft)
                    dialog.locator('[data-popup-action="ok"]').click()
                    expect(action).to_be_enabled()
                mutations.clear()
            resumes = len(daemon.methods('thread/resume'))
            action.click(); dialog.locator('[data-popup-action="ok"]').click()
            page.wait_for_function('!ConsoleUI.busy.has(S.sel)')
            if mode == 'sdk':
                page.wait_for_function('SdkView.connected && !SdkView.readOnly')
                expect(page.locator('#a-sdk')).to_have_attribute('aria-pressed','true')
                assert [path for path, _ in mutations] == ['stop','open'], mutations
                assert len(daemon.methods('thread/resume')) == resumes+1
            else:
                page.wait_for_function('!!takenOver(S.sel)')
                expect(page.locator('#a-chat')).to_have_attribute('aria-pressed','true')
                assert [path for path, _ in mutations] == ['takeover'], mutations
                assert mutations[0][1]['force'] is True
                assert len(daemon.methods('thread/resume')) == resumes
            child.wait(timeout=8)
            assert unrelated.poll() is None, 'takeover must not stop an unrelated native SID'
            assert all(body['uid'] == uid for _, body in mutations)
            expect(editor).to_have_value(draft)
            expect(editor).to_be_focused()
            expect(action).to_be_hidden()
            page.screenshot(path=str(shots/f'{label}-{mode}-{width}-managed.png'))
            assert not errors, errors
            # Explicitly stop only this private fixture before the next case.
            endpoint = '/api/codex-sdk/stop' if mode == 'sdk' else '/api/session/stop'
            result = page.request.post(base+endpoint,data={'uid':uid})
            assert result.ok, result.text()
        finally:
            context.close()
            for process in (child, unrelated):
                if process.poll() is None: process.terminate()
                process.wait(timeout=8)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=BINARY)
    parser.add_argument('--screenshots',type=Path,default=Path(tempfile.gettempdir())/'sessiondock-composer-takeover')
    args=parser.parse_args(); args.screenshots.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-footer-takeover-') as tmp, sync_playwright() as pw:
        root=Path(tmp); (root/'work').mkdir()
        corpus=build(root); sid=str(uuid.uuid4())
        native=corpus.paths['sdk']
        native.write_text(native.read_text().replace(THREAD,sid).replace('/synthetic/sdk',str(root/'work')))
        extra=environment(root,args.binary)
        config=root/'launcher.json'
        profile=json.loads(config.read_text())
        profile['profiles']=[{'id':'footer-cli','source':'codex','executable':'/bin/sh','args':['-c',SHELL_SCRIPT]}]
        config.write_text(json.dumps(profile))
        extra.update(node_env(root,free_port(),'127.0.0.0/8'))
        extra['SESSIONDOCK_PROC_ROOT']='/proc'
        with private_hosts(root), StartDaemon(root/'app.sock',native,sid) as daemon:
            with server(corpus,args.binary,root/'app.sock',extra) as base:
                hub_root=root/'hub';hub_root.mkdir()
                hub=Hub(args.binary.resolve().with_name('sessiondock-hub'),hub_root,
                    [SimpleNamespace(name='FooterNode',token=TOKEN,port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':',1)[1]))])
                hub.start();browser=pw.chromium.launch(headless=True)
                try:
                    for label,url in [('node',base),('hub',f'http://127.0.0.1:{hub.port}')]:
                        for mode in ('sdk','cli'): exercise(browser,url,mode,label,root,sid,daemon,args.screenshots)
                finally: browser.close();hub.stop()
    print('PASS footer takeover: node/Hub, CLI/SDK, cancel, stop failure/uncertainty, exact process stop and preserved draft')

if __name__=='__main__':main()
