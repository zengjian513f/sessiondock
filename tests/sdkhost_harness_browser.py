#!/usr/bin/env python3
"""Codex/OpenCode host continuity through production UI and private synthetic peers."""
from __future__ import annotations
import argparse
import asyncio
import json
from pathlib import Path
import tempfile
import time
import threading
import base64
import sys
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD, build, server
from codex_sdk_actions_browser import environment
from codex_sdk_fixtures import FakeAppServer
from frontend_paths import frontend_dir
from history_fixtures import BINARY, Corpus, isolated_server
from opencode_sdk_fixtures import FakeOpenCode, SID
from sdkhost_fixtures import stop_private_sdkhosts
from sdk_ui_fixtures import start_sdk
from send_browser import initialize


def hosts(root, prefix):
    return {p.parent.name: json.loads(p.read_text()) for p in (root / 'state/sdkhost').glob('*/status.json')
            if p.parent.name.startswith(prefix)}


def open_sid(page, base, sid):
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('sid => S.sessions.some(s=>s.sid===sid)', arg=sid)
    uid = page.evaluate('sid => S.sessions.find(s=>s.sid===sid).uid', sid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    start_sdk(page)
    expect(page.locator('#sdk-input')).to_be_enabled()
    return uid


def send(page, text):
    page.locator('#sdk-input').fill(text)
    page.locator('#sdk-send').click()


def codex(browser, binary, screenshots):
    with tempfile.TemporaryDirectory(prefix='sd-hosted-codex-') as tmp:
        root = Path(tmp); corpus = build(root); env = environment(root, binary)
        try:
            with FakeAppServer(root / 'native.sock', corpus.paths['sdk'], THREAD) as native:
                page = browser.new_page(); page.set_default_timeout(30000)
                with server(corpus, binary, root / 'native.sock', env) as base:
                    open_sid(page, base, THREAD)
                    send(page, 'approve across service restart')
                    expect(page.locator('.sdk-request')).to_contain_text('Synthetic approval reason')
                    original = hosts(root, 'codex-channel-'); assert len(original) == 1, original
                    original_client = list(native.clients); assert len(original_client) == 1
                    initialize_count = len(native.methods('initialize'))
                    turn_count = len(native.methods('turn/start'))
                # The same native WebSocket must stay subscribed while no HTTP service exists.
                assert native.clients == original_client, 'service shutdown closed native subscription'
                asyncio.run_coroutine_threadsafe(native.notify('item/agentMessage/delta', {
                    'turnId': native.turn, 'itemId': 'offline-item', 'delta': 'output while node offline'}), native.loop).result(5)
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    raw = b''.join(p.read_bytes() for p in (root / 'state/sdkhost').glob('codex-channel-*/stdout'))
                    if b'output while node offline' in raw: break
                    time.sleep(.05)
                assert b'output while node offline' in raw, 'offline output was not retained by sdkhost'
                with server(corpus, binary, root / 'native.sock', env) as base:
                    open_sid(page, base, THREAD)
                    card = page.locator('.sdk-request')
                    expect(card).to_contain_text('Synthetic approval reason')
                    assert {k:v['pid'] for k,v in hosts(root, 'codex-channel-').items()} == {k:v['pid'] for k,v in original.items()}
                    assert native.clients == original_client
                    assert len(native.methods('initialize')) == initialize_count, 'reinitialized existing native connection'
                    assert len(native.methods('turn/start')) == turn_count, 'replayed user input'
                    card.get_by_role('button', name='批准', exact=True).click()
                    expect(page.locator('#detail')).to_contain_text('All done after approval', timeout=30000)
                    page.reload(wait_until='domcontentloaded')
                    page.wait_for_function('CodexSdkView.connected')
                    expect(page.locator('.sdk-request')).to_have_count(0)
                    assert len(native.responses()) == 1, native.responses()
                    if screenshots: page.screenshot(path=str(screenshots / 'codex-host-recovery.png'), full_page=True)
                page.close()
                print('PASS Codex: same hosted socket/PID, offline output, initialize once, pending approval recovered and answered once', flush=True)
        finally: stop_private_sdkhosts(root)


def codex_listener(browser, binary):
    with tempfile.TemporaryDirectory(prefix='sd-hosted-listener-') as tmp:
        root = Path(tmp); corpus = build(root); env = environment(root, binary)
        script = root/'listener.py'
        script.write_text('''import sys, json, threading
from pathlib import Path
sys.path.insert(0, ''' + repr(str(Path(__file__).parent.resolve())) + ''')
from codex_sdk_fixtures import FakeAppServer
if sys.argv[1:] == ['--version']:
    print('synthetic codex listener'); raise SystemExit
args = sys.argv[1:]
if args[:1] == ['-c']:
    assert args[1] == 'model_provider="openai"', args
    args = args[2:]
assert args[:2] == ['app-server','--listen'], args
socket = args[2].removeprefix('unix://')
class Peer(FakeAppServer):
    async def _handle(self, writer, message):
        with open(''' + repr(str(root/'received.jsonl')) + ''', 'a') as output:
            output.write(json.dumps(message)+'\\n')
        await super()._handle(writer, message)
with Peer(socket, Path(''' + repr(str(corpus.paths['sdk'])) + '), ' + repr(THREAD) + '''):
    threading.Event().wait()
''')
        launcher = root/'launcher.json'
        config = json.loads(launcher.read_text())
        config['profiles'][0].update(executable=sys.executable, args=[str(script)])
        launcher.write_text(json.dumps(config))
        env['SESSIONDOCK_CODEX_APP_SERVER'] = ''
        page = browser.new_page(); page.set_default_timeout(30000)
        try:
            with server(corpus, binary, '', env) as base:
                open_sid(page, base, THREAD)
                send(page, 'approve hosted listener')
                expect(page.locator('.sdk-request')).to_contain_text('Synthetic approval reason')
                original = hosts(root, 'codex-listener-'); assert len(original)==1, original
            with server(corpus, binary, '', env) as base:
                open_sid(page, base, THREAD)
                expect(page.locator('.sdk-request')).to_contain_text('Synthetic approval reason')
                assert {k:v['pid'] for k,v in hosts(root, 'codex-listener-').items()} == {k:v['pid'] for k,v in original.items()}
                page.locator('.sdk-request').get_by_role('button', name='批准', exact=True).click()
                expect(page.locator('#detail')).to_contain_text('All done after approval', timeout=30000)
                frames = [json.loads(line) for line in (root/'received.jsonl').read_text().splitlines()]
                assert sum(frame.get('method')=='initialize' for frame in frames)==1
                assert sum(frame.get('method')=='turn/start' for frame in frames)==1
            print('PASS Codex auto-start: sdkhost owns listener and channel; same native PID across service restart', flush=True)
        except Exception:
            print('Listener UI:', page.locator('.sdk-status').all_text_contents(), flush=True)
            for name in ('host.log','stderr'):
                for path in (root/'state/sdkhost').glob('*/'+name):
                    print(path.parent.name, path.read_text(errors='replace')[-6000:], flush=True)
            raise
        finally:
            page.close()
            stop_private_sdkhosts(root)


class DelayedOpenCode(FakeOpenCode):
    def __init__(self, root):
        super().__init__(root)
        self.call_started = threading.Event()
        self.call_release = threading.Event()
    def handle(self, method, path, body):
        result = super().handle(method, path, body)
        if method == 'POST' and path.endswith('/prompt') and body.get('text') == 'delayed-host-call':
            self.call_started.set()
            assert self.call_release.wait(15), 'test did not release delayed native response'
        return result


def opencode(browser, binary, screenshots):
    with tempfile.TemporaryDirectory(prefix='sd-hosted-opencode-') as tmp:
        root = Path(tmp)
        for name in ('claude','codex','grok','host','state','proc'): (root/name).mkdir(mode=0o700)
        (root/'proc/stat').write_text('btime 1700000000\n')
        try:
            with DelayedOpenCode(root) as native:
                launcher = root/'launcher.json'
                launcher.write_text(json.dumps({'schema':2,'host_binary':str(binary.with_name('ptyhost').resolve()),
                    'host_dir':str(root/'host'),'profiles':[native.profile()]}))
                initialize('--initialize-lifecycle', root/'ledger', binary)
                env = {'SESSIONDOCK_NATIVE_SDKS':'auto','SESSIONDOCK_OPENCODE_DB':str(native.db),
                    'SESSIONDOCK_OPENCODE_ROOT':str(root/'mirror'),'SESSIONDOCK_PROC_ROOT':str(root/'proc')}
                def service(): return isolated_server(Corpus(root), binary, host_dir=root/'host', state_dir=root/'state',
                    lifecycle_dir=root/'ledger', launcher_config=launcher, extra_env=env)
                page = browser.new_page(); page.set_default_timeout(30000)
                with service() as (base, _):
                    open_sid(page, base, SID); send(page, 'approval')
                    expect(page.locator('.sdk-request')).to_contain_text('native approval survives reload', timeout=30000)
                    original = hosts(root, 'command-channel-'); assert len(original) == 1, original
                    prompts = [r for r in native.received if r[0]=='POST' and r[1].endswith('/prompt')]
                    assert len(prompts) == 1
                assert native.permissions[SID], 'shutdown answered or canceled native permission'
                with service() as (base, _):
                    open_sid(page, base, SID)
                    expect(page.locator('.sdk-request')).to_contain_text('native approval survives reload', timeout=30000)
                    assert {k:v['pid'] for k,v in hosts(root, 'command-channel-').items()} == {k:v['pid'] for k,v in original.items()}
                    page.locator('.sdk-request').get_by_role('button', name='拒绝', exact=True).click()
                    page.wait_for_function('!NativeSdkView.raw?.requests?.length')
                    assert len([r for r in native.received if r[0]=='POST' and r[1].endswith('/prompt')]) == 1
                    if screenshots: page.screenshot(path=str(screenshots / 'opencode-host-recovery.png'), full_page=True)
                    page.wait_for_function('NativeSdkView.raw?.running === false')
                    send(page, 'delayed-host-call')
                    assert native.call_started.wait(5), 'native prompt did not start'
                # The HTTP service is gone while the machine client is waiting
                # for its response. Completing the response must still be captured.
                native.call_release.set()
                records = [json.loads(p.read_text()) for p in (root/'state/sdk-adapters/commands').rglob('call.json')]
                call = next(row for row in records if 'delayed-host-call' in json.dumps(row['frame']))
                output = root/'state/sdkhost'/call['host']/'stdout'
                deadline = time.monotonic()+10
                while time.monotonic()<deadline:
                    rows = [json.loads(line) for line in output.read_text().splitlines() if line.endswith('}')]
                    found = next((row for row in rows if row['id']==call['id']), None)
                    if found: break
                    time.sleep(.05)
                assert found and found['result']['success'], found
                assert json.loads(base64.b64decode(found['result']['stdout'])) is not None
                with service() as (base, _):
                    open_sid(page, base, SID)
                    expect(page.locator('#detail')).to_contain_text('native reply completed: delayed-host-call', timeout=30000)
                    assert len([r for r in native.received if r[0]=='POST' and r[1].endswith('/prompt')]) == 2
                    assert {k:v['pid'] for k,v in hosts(root, 'command-channel-').items()} == {k:v['pid'] for k,v in original.items()}
                page.close()
                print('PASS OpenCode: same command host, in-flight API response captured while offline, native permission recovered, prompts delivered once', flush=True)
        finally: stop_private_sdkhosts(root)


def ephemeral(browser, binary, source):
    from sdk_platform_fixtures import Worker
    from hub_fixtures import free_port
    from popups import on_popup
    args = SimpleNamespace(source=source, binary=binary, port=free_port(), web=frontend_dir(),
        driver=binary.parent/'examples/platform_fixture')
    worker = Worker(args)
    scratch = worker.root/'temporary'; scratch.mkdir(mode=0o700)
    worker.env.pop('SESSIONDOCK_STATE_DIR')
    worker.env.update(TMPDIR=str(scratch), TMP=str(scratch), TEMP=str(scratch))
    if source == 'kiro':
        # Kiro's app-owned catalog needs an explicit projection location when
        # no global state directory is configured; this is not host storage.
        worker.env['SESSIONDOCK_KIRO_PROJECTION_ROOT'] = str(worker.root/'kiro-projection')
    page = browser.new_page(); page.set_default_timeout(30000)
    on_popup(page, lambda dialog: dialog.accept())
    try:
        info = worker.start()
        open_sid(page, info['base'], info['sid'])
        send(page, 'platform first input')
        page.wait_for_function('NativeSdkView.raw?.running === false && (NativeSdkView.raw.items || []).some(i=>JSON.stringify(i).includes("platform first input"))')
        directories = list(scratch.glob('sessiondock-sdk-*'))
        assert len(directories)==1, directories
        worker.state = directories[0]
        statuses = [json.loads(p.read_text()) for p in worker.state.glob('sdkhost/*/status.json')]
        assert any(s['exit'] is None for s in statuses), (source, statuses)
        stop = page.locator('#a-session-toggle')
        if not stop.is_visible(): page.locator('#a-more').click()
        with page.expect_response(lambda r:r.url.endswith('/api/native-sdk/stop')) as response: stop.click()
        assert response.value.ok, response.value.text()
        print('PASS',source,'without configured state: sdkhost owns native runtime; UI send and explicit stop',flush=True)
    finally:
        page.close()
        directories = list(scratch.glob('sessiondock-sdk-*'))
        if directories: worker.state = directories[0]
        worker.stop_service()
        if directories: stop_private_sdkhosts(worker.root, host_dir=worker.state/'sdkhost')
        worker.args.driver = None
        worker.cleanup()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    parser.add_argument('--only', choices=('persistent','ephemeral'))
    args = parser.parse_args(); args.binary = args.binary.resolve()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            if args.only != 'ephemeral':
                codex(browser, args.binary, args.screenshots)
                codex_listener(browser, args.binary)
                opencode(browser, args.binary, args.screenshots)
            if args.only != 'persistent':
                for source in ('claude','codebuddy','droid','grok','agy','kiro'):
                    ephemeral(browser, args.binary, source)
        finally: browser.close()


if __name__ == '__main__': main()
