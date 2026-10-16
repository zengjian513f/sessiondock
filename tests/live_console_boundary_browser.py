#!/usr/bin/env python3
"""A headless child of a managed CLI must not borrow its console or bypass takeover."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import uuid

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, REPO, Corpus, codex_row, codex_message, get_json, isolated_server
from host_identity import host
from spawned_by_fixtures import proc_pid, START


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sd-console-boundary-') as tmp:
        root = Path(tmp)
        for name in ['host', 'work', 'state', 'ledger']:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        parent, child = 'synthetic-native-sid', '0bbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
        for sid, title in [(parent, 'Managed parent CLI console'),
                           (child, '独立执行会话 · Codex exec child without a console')]:
            corpus.put(sid, 'codex', [codex_row('session_meta', {
                'id': sid, 'cwd': str(root / 'work'), 'source': 'exec' if sid == child else 'cli'}),
                codex_message('user', title)], [])
        parent_uid, child_uid = corpus.uid(parent), corpus.uid(child)
        config = root / 'launcher.json'
        config.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'adapters': [], 'profiles': [{'id': 'codex-cli-v1', 'source': 'codex',
                'executable': '/bin/sh', 'args': ['-c', 'exit 99'],
                'new_args': [], 'resume_args': ['{sid}'], 'env': {'PATH': '/usr/bin:/bin'}}]}))
        config.chmod(0o600)
        subprocess.run([str(args.binary.resolve()), '--initialize-lifecycle', str(root / 'ledger')],
                       check=True, capture_output=True)
        instance = 'synthetic-' + uuid.uuid4().hex
        with host(root, instance, uid=parent_uid) as (process, record), sync_playwright() as pw:
            proc = root / 'proc'; proc.mkdir()
            (proc / 'stat').write_text('btime 1700000000\n')
            # Runtime identity verification uses the real private host. Discovery
            # uses this synthetic tree whose root is the host's actual child PID.
            parent_pid = record['pid']
            START[parent_pid] = 10000
            proc_pid(proc, parent_pid, 'codex', ['codex'], 1,
                     fds={3: corpus.paths[parent]})
            proc_pid(proc, 200, 'python', ['python', 'dispatch.py'], parent_pid)
            proc_pid(proc, 400, 'codex', ['codex', 'exec'], 200,
                     fds={3: corpus.paths[child]})
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**launch)
            try:
                with isolated_server(corpus, args.binary, state_dir=root / 'state',
                        host_dir=root / 'host', lifecycle_dir=root / 'ledger', launcher_config=config,
                        extra_env={'SESSIONDOCK_PROC_ROOT': str(proc)}) as (base, opener):
                    for width, theme in [(1280, 'light'), (390, 'dark')]:
                        context = browser.new_context(viewport={'width': width, 'height': 900})
                        page = context.new_page()
                        page.add_init_script("localStorage.setItem('sessiondock.theme', JSON.stringify('" + theme + "'))")
                        page.goto(base, wait_until='networkidle')
                        item = page.locator(f'#side .item[data-uid="{child_uid}"]')
                        expect(item).to_be_visible()
                        item.click()
                        expect(page.locator('#msgs')).to_contain_text('Codex exec child without a console')
                        live = get_json(opener, base, '/api/live?force=1')
                        assert child_uid in live['uids'], live
                        if args.screenshots:
                            args.screenshots.mkdir(parents=True, exist_ok=True)
                            page.screenshot(path=str(args.screenshots / f'child-{width}.png'))
                        assert child_uid not in live['tmux_uids'], live
                        assert parent_uid in live['tmux_uids'], live
                        assert 'live-tmux' not in (item.get_attribute('class') or '').split()
                        with page.expect_response(lambda r: '/api/term/takeover' in r.url) as response:
                            page.locator('#a-term').click()
                        body = response.value.json()
                        assert body.get('needs_confirm') is True and body['pids'] == [400], body
                        popup = page.locator('dialog.app-popup[open]')
                        expect(popup).to_contain_text('无法直接接入')
                        if args.screenshots:
                            page.screenshot(path=str(args.screenshots / f'confirm-{width}.png'))
                        popup.get_by_role('button', name='取消', exact=True).click()
                        assert get_json(opener, base, '/api/term/list')['pending'] == []
                        assert process.poll() is None
                        context.close()
                    # Parent remains attachable through the existing real host.
                    page = browser.new_page(viewport={'width': 1280, 'height': 900})
                    page.goto(base, wait_until='networkidle')
                    page.locator(f'#side .item[data-uid="{parent_uid}"]').click()
                    page.locator('#a-term').click()
                    page.wait_for_function('T.ws && T.ws.readyState === WebSocket.OPEN')
                    expect(page.locator('#termpane')).to_be_visible()
                    page.close()
            finally:
                browser.close()
    print('PASS managed CLI boundary: child live without console; takeover asks, cancel preserves parent; parent attaches')


if __name__ == '__main__':
    main()
