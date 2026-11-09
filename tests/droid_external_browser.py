#!/usr/bin/env python3
"""Droid external green state and confirmed takeover, using private fake processes."""
import argparse
from contextlib import ExitStack
from datetime import datetime, timezone, timedelta
import json
import os
import re
import subprocess
import tempfile
import uuid
from pathlib import Path
from types import SimpleNamespace

from playwright.sync_api import sync_playwright, expect
from droid_sdk_fixtures import DroidFixture, native_records, native_uid
from history_fixtures import BINARY
from private_hosts import private_hosts
from codebuddy_cli_browser import wait_start
from node_auth_fixtures import node_env, free_port, TOKEN
from hub_fixtures import Hub


def check(root, through_hub, screenshots):
    f = DroidFixture(root)
    f.path.unlink()
    f.sid = str(uuid.uuid4())
    f.path = f.path.with_name(f.sid+'.jsonl')
    f.uid = native_uid(f.path)
    config = json.loads(f.launcher.read_text())
    config['host_binary'] = str(BINARY.with_name('ptyhost'))
    f.launcher.write_text(json.dumps(config))
    # argv0 is Droid, with neither a session ID nor a held native file. These
    # are sleep processes, not real CLIs; only this fixture's process is killed.
    external = subprocess.Popen(['droid', '300'], executable='/bin/sleep', cwd=f.cwd,
                                env={**os.environ, 'HOME':str(f.home), 'FACTORY_HOME_OVERRIDE':str(f.home)})
    other = subprocess.Popen(['claude', '300'], executable='/bin/sleep', cwd=f.cwd)
    rows = native_records(f.sid, f.cwd, [('external Droid prompt', 'external Droid answer')])
    clock = datetime.now(timezone.utc)
    now = (clock + timedelta(minutes=11) if through_hub else clock).isoformat()
    if through_hub:
        logs = f.home/'.factory/logs'; logs.mkdir()
        def entry(time, sid):
            return '['+time.isoformat()+'] INFO: [Session] Settings provenance | Context: '+json.dumps({
                'process.pid':external.pid,'sessionId':sid,'tags':{'clientMode':'tui','sessionId':sid}})+'\n'
        # A valid explicit identity works even when the first message is delayed.
        # A stale entry for a reused PID cannot replace it, even later in the file.
        (logs/'droid-log-single.log').write_text(entry(clock,f.sid)+entry(clock-timedelta(hours=1),'stale-session'))
    for row in rows[1:]: row['timestamp'] = now
    f.path.write_text(''.join(json.dumps(r)+'\n' for r in rows))
    old = f.path.with_name('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa.jsonl')
    old.write_text(''.join(json.dumps(r)+'\n' for r in native_records(old.stem, f.cwd, [('old prompt', 'old answer')])))
    try:
        with ExitStack() as stack:
            stack.enter_context(private_hosts(f.root/'host'))
            extra = {'SESSIONDOCK_PROC_ROOT':'/proc'}
            port = free_port()
            if through_hub: extra.update(node_env(f.root, port, '127.0.0.0/8'))
            base = stack.enter_context(f.server(BINARY, extra))
            uid, old_uid = f.uid, native_uid(old)
            if through_hub:
                hubroot = f.root/'hub'; hubroot.mkdir()
                hub = Hub(BINARY.with_name('sessiondock-hub'), hubroot, [SimpleNamespace(name='DroidNode', token=TOKEN, port=port)])
                hub.start(); stack.callback(hub.stop)
                base = f'http://127.0.0.1:{hub.port}'
            with sync_playwright() as pw:
                browser = pw.chromium.launch(headless=True)
                page = browser.new_page(viewport={'width':390 if through_hub else 1280, 'height':900},
                                        color_scheme='dark' if through_hub else 'light')
                page.goto(base, wait_until='networkidle')
                if through_hub:
                    uid = page.locator(f'#side .item[data-uid$="{f.uid.split(":",1)[1]}"]').get_attribute('data-uid')
                    old_uid = page.locator(f'#side .item[data-uid$="{native_uid(old).split(":",1)[1]}"]').get_attribute('data-uid')
                page.wait_for_function('uid => S.live.has(uid)', arg=uid)
                assert page.evaluate('uid => !S.liveTmux.has(uid) && !S.liveSdk.has(uid)', uid)
                assert page.evaluate('uid => !S.live.has(uid)', old_uid)
                item = page.locator(f'#side .item[data-uid="{uid}"]')
                item.click()
                expect(page.locator('#dlive')).to_have_class(re.compile(r'\bvisible\b'))
                expect(page.locator('#dlive')).not_to_have_class(re.compile(r'\b(tmux|sdk)\b'))
                if screenshots: page.screenshot(path=str(screenshots/f'green-{through_hub}.png'), full_page=True)
                # Opening the SDK cannot create a second writer or claim blue.
                page.locator('#a-sdk').click()
                run = page.locator('#sdk-composer .composer-run-action')
                expect(run).to_be_visible()
                expect(run).to_have_text('接管')
                run.click()
                page.locator('dialog.app-popup[open]').get_by_role('button', name='取消', exact=True).click()
                expect(page.locator('#a-term')).to_be_enabled()
                opened = page.request.post(base+'/api/native-sdk/open', data={'uid':uid})
                assert not opened.ok and 'busy' in opened.text(), opened.text()
                assert not f.frames('spawn')
                assert external.poll() is None
                assert page.evaluate('!NativeSdkView.connected')
                # CLI cancel leaves the external process intact; confirmation
                # must terminate it before starting the managed resume.
                with page.expect_response(lambda r:'/api/term/takeover' in r.url) as taken: page.locator('#a-term').click()
                receipt = taken.value.json()
                assert receipt.get('needs_confirm') and receipt['pids']==[external.pid], receipt
                dialog = page.locator('dialog.app-popup[open]')
                expect(dialog).to_contain_text('会先停止外部运行')
                if screenshots: page.screenshot(path=str(screenshots/f'confirm-{through_hub}.png'), full_page=True)
                dialog.get_by_role('button', name='取消', exact=True).click()
                assert external.poll() is None and not f.frames('cli_start')
                page.locator('#a-term').click()
                with page.expect_response(lambda r:'/api/term/takeover' in r.url and r.request.post_data_json.get('force') is True) as taken:
                    dialog.get_by_role('button', name='确定', exact=True).click()
                receipt = taken.value.json()
                assert receipt.get('action')=='killed', receipt
                external.wait(timeout=8)
                assert other.poll() is None
                wait_start(f, lambda r:'--resume' in r['args'])
                expect(page.locator('#termpane')).to_be_visible()
                page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
                page.wait_for_function('uid => S.liveTmux.has(uid)', arg=uid)
                if screenshots: page.screenshot(path=str(screenshots/f'hosted-{through_hub}.png'), full_page=True)
                browser.close()
    finally:
        for child in (external, other):
            if child.poll() is None: child.terminate()
            child.wait(timeout=8)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    for hub in (False, True):
        with tempfile.TemporaryDirectory(prefix='sd-droid-external-') as root: check(root, hub, args.screenshots)
    print('PASS Droid external green, SDK conflict, cancel, terminate/resume and blue; node and Hub')


if __name__=='__main__': main()
