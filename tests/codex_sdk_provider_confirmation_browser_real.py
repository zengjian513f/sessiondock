#!/usr/bin/env python3
"""Real provider confirmation and migration, no model input or credentials, private homes."""
from contextlib import ExitStack
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import tempfile
import time

from playwright.sync_api import expect, sync_playwright
import codex_provider_migrate_browser_real as fixture
from codex_sdk_actions_browser import environment
from codex_sdk_browser import server
from history_fixtures import BINARY, Corpus
from new_session_model_browser import open_dialog
from sdk_ui_fixtures import start_sdk


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', type=Path, default=Path(shutil.which('codex') or 'codex'))
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sd-pr-') as tmp, sync_playwright() as pw:
        root = Path(tmp); home = root/'codex'; home.mkdir(); cwd = root/'work'; cwd.mkdir()
        for name in ('claude','grok'): (root/name).mkdir()
        (home/'config.toml').write_text('''model_provider="openai_catalog_cache"
model="gpt-5.6-luna"
model_reasoning_effort="low"
[model_providers.openai_catalog_cache]
name="Synthetic cache"
wire_api="responses"
base_url="http://127.0.0.1:9/v1"
''')
        original = fixture.SID
        first = fixture.seed(args.codex, home, cwd)
        fixture.SID = '01a11d3f-7510-7d20-8f67-546a6975ebea'
        second = fixture.seed(args.codex, home, cwd, 'openai_catalog_cache', 'cache provider history')
        wrapper = root/'bin'; wrapper.mkdir(); pidlog = root/'pids'
        (wrapper/'codex').write_text('#!/bin/sh\necho $$ >> '+shlex.quote(str(pidlog))+'\nexec '+shlex.quote(str(args.codex.resolve()))+' -c \'model="gpt-5.6-luna"\' -c \'model_reasoning_effort="low"\' "$@"\n')
        (wrapper/'codex').chmod(0o700)
        env = {**environment(root,args.binary), 'HOME':str(root), 'CODEX_HOME':str(home),
               'PATH':str(wrapper)+os.pathsep+os.environ['PATH'],
               'SESSIONDOCK_CODEX_APP_SERVER':'', 'SESSIONDOCK_CODEX_ROOT':str(home/'sessions')}
        corpus = Corpus(root)
        browser = pw.chromium.launch(headless=True)
        stack = ExitStack()
        shared_socket = home/'app-server-control/app-server-control.sock'
        shared_socket.parent.mkdir()
        shared = stack.enter_context(fixture.native(args.codex, home, cwd, 'openai_catalog_cache', shared_socket))
        deadline=time.monotonic()+15
        while not shared_socket.exists():
            assert shared.poll() is None and time.monotonic()<deadline
            time.sleep(.05)
        try:
            for run in range(2):
                with server(corpus,args.binary,root/'unused.sock',env) as base:
                    page = browser.new_page(viewport={'width':1280,'height':900})
                    page.goto(base,wait_until='networkidle')
                    page.locator('#side .item').filter(has_text='cache provider history').first.click()
                    if run==0:
                        page.locator('.sdk-pane .composer-run-action').click()
                        popup=page.locator('dialog.app-popup[open]')
                        expect(popup).to_contain_text('openai_catalog_cache')
                        if args.screenshots:
                            args.screenshots.mkdir(parents=True,exist_ok=True)
                            page.screenshot(path=str(args.screenshots/'native-provider-prompt.png'),full_page=True)
                        popup.get_by_role('button',name='否，不启动',exact=True).click()
                        assert not page.evaluate('CodexSdkView.connected')
                        import sqlite3
                        db=sqlite3.connect(home/'state_5.sqlite')
                        assert db.execute('select model_provider from threads where id=?',(fixture.SID,)).fetchone()[0]=='openai_catalog_cache'
                        page.locator('.sdk-pane .composer-run-action').click()
                        popup.get_by_role('button',name='是，修改并启动',exact=True).click()
                        page.wait_for_function('CodexSdkView.connected')
                        assert db.execute('select model_provider from threads where id=?',(fixture.SID,)).fetchone()[0]=='openai'
                        history=sqlite3.connect(home/'thread_history_1.sqlite')
                        offsets=history.execute('select rollout_byte_offset,rollout_end_byte_offset from thread_turns where thread_id=?',(fixture.SID,)).fetchall()
                        assert len(offsets)==2
                        with second.open('rb') as f:
                            for start,end in offsets:
                                for offset in (start,end):
                                    if offset:
                                        f.seek(offset-1);assert f.read(1)==b'\n'
                        history.close();db.close()
                    else:
                        start_sdk(page)
                    summary=page.evaluate('CodexSdkView.summary')
                    assert summary['resume']['modelProvider']=='openai',summary
                    assert summary['settings']['model']==fixture.MODEL,summary
                    assert summary['settings']['effort']=='low',summary
                    expect(page.locator('.sdk-history')).to_contain_text('preserved answer 1')
                    page.reload(wait_until='networkidle');start_sdk(page)
                    expect(page.locator('dialog.app-popup[open]')).to_have_count(0)
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots/f'native-provider-ready-{run}.png'),full_page=True)
                    if run==0:
                        open_dialog(page)
                        page.locator('input[name="new-source"][value="codex"]').check()
                        page.locator('#new-model').click()
                        page.locator('[data-model-option][title="gpt-5.6-luna"]').click()
                        page.locator('#new-effort').select_option('low')
                        page.locator('#new-cwd').fill(str(cwd))
                        page.locator('#new-session-go').click()
                        expect(page.locator('#new-session-dialog')).to_be_hidden(timeout=20000)
                        page.wait_for_function('CodexSdkView.connected')
                        summary=page.evaluate('CodexSdkView.summary')
                        assert summary['resume']['modelProvider']=='openai',summary
                        assert summary['settings']['model']==fixture.MODEL,summary
                    page.close()
            assert len(pidlog.read_text().splitlines()) == 1, 'service restart spawned a duplicate provider process'
            assert shared.poll() is None, 'shared daemon was stopped'
            print('PASS native provider confirmation: No unchanged, Yes migrated and resumed, indexed offsets preserved, default openai create, reload/restart; no model input')
        finally:
            browser.close()
            stack.close()
            if pidlog.exists():
                for raw in pidlog.read_text().splitlines():
                    try: os.kill(int(raw),signal.SIGTERM)
                    except ProcessLookupError: pass

if __name__=='__main__': main()
