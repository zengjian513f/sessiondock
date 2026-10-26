#!/usr/bin/env python3
"""Start a saved Claude session from SDK view, then type through its real PTY.

Uses synthetic history, a fake CLI, private hosts and an isolated node/Hub.
The SDK open fails synthetically so it cannot create a competing native writer.
"""
import argparse
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
from urllib.parse import urlsplit
import uuid

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, REPO, Corpus, claude_row, isolated_server
from hub_fixtures import Hub, free_port, scoped
from private_hosts import private_hosts
from send_browser import initialize, xterm_includes


def exercise(browser, base, corpus, sid, node, width, theme, output):
    label = f'{"hub" if node else "node"}-{width}-{theme}'
    print(f'Running {label}', flush=True)
    context = browser.new_context(viewport={'width': width, 'height': 900},
                                  color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    context.route('**/api/native-sdk/open', lambda route: route.fulfill(status=409, json={
        'error': 'Synthetic SDK unavailable', 'code': 'synthetic_sdk_unavailable'}))
    page = context.new_page()
    errors, launches, claims = [], [], []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.on('request', lambda request: claims.append(request.post_data_json)
            if urlsplit(request.url).path == '/api/term/claim' else None)
    page.on('response', lambda response: launches.append(response.json())
            if urlsplit(response.url).path == '/api/term/takeover' else None)
    uid = scoped(node.nid, corpus.uid(sid)) if node else corpus.uid(sid)
    try:
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        page.locator(f'#side .item[data-uid="{uid}"]').click()
        page.locator('#sdk-composer .composer-run-action').click()
        expect(page.locator('#sdk-status-menu')).to_contain_text('Synthetic SDK unavailable')
        expect(page.locator('#session-content-view')).to_be_visible()
        page.screenshot(path=str(output / f'{label}-before.png'))
        if page.locator('#detail .mobile-back').is_visible(): page.locator('#detail .mobile-back').click()
        page.locator(f'#side .item[data-uid="{uid}"]').click(button='right')
        start = page.locator('#item-menu [data-act="start-cli"]')
        expect(start).to_have_text('启动CLI模式')
        start.click()
        try:
            expect(page.locator('#session-content-view')).to_have_count(0, timeout=12000)
            expect(page.locator('#termpane')).to_be_visible()
            page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
            xterm_includes(page, '❯' if width < 600 else f'FAKE_CLAUDE_READY sid=[{sid}]')
        except Exception:
            page.screenshot(path=str(output / f'{label}-failed.png'))
            print('Failed start state:', page.evaluate('({view:SessionContentView.kind, '
                  'selected:S.sel, terminal:T.name, visible:!$("#termpane").classList.contains("hidden"), '
                  'socket:T.ws?.readyState})'), 'launches:', launches, 'claims:', claims, flush=True)
            raise
        assert len(launches) == 1 and launches[0].get('ok'), launches
        assert len(claims) == 1 and claims[0]['uid'] == uid, claims
        text = f'terminal connected {label}'
        editor = page.locator('#termpane .xterm-helper-textarea')
        editor.press_sequentially(text)
        editor.press('Enter')
        page.locator('#a-chat').click()
        expect(page.locator('#msgs')).to_contain_text('OK: ' + text, timeout=15000)
        rows = [json.loads(line) for line in corpus.paths[sid].read_text().splitlines()]
        assert sum(row.get('message', {}).get('content') == text for row in rows) == 1
        page.locator('#a-term').click()
        expect(page.locator('#termpane')).to_be_visible()
        page.screenshot(path=str(output / f'{label}-connected.png'))
        # A later discovery refresh must preserve the user's terminal choice.
        page.evaluate('loadTermList()')
        expect(page.locator('#termpane')).to_be_visible()
        page.wait_for_function('T.ws?.readyState === WebSocket.OPEN')
        expect(page.locator('#session-content-view')).to_have_count(0)
        assert len(launches) == 1, launches
        assert not errors, errors
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path, default=REPO / 'target/debug/sessiondock-hub')
    parser.add_argument('--screenshots', type=Path, default=REPO / 'target/session-start-sdk')
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.hub_binary = args.hub_binary.resolve(strict=True)
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-session-start-sdk-') as temporary, \
            private_hosts(Path(temporary)), sync_playwright() as pw:
        root = Path(temporary)
        for name in ('host', 'work', 'home', 'claude', 'codex', 'grok', 'ledger', 'state', 'hub'):
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        cases = [(mode, width, theme, str(uuid.uuid4())) for mode in (False, True)
                 for width, theme in ((1280, 'light'), (390, 'dark'), (1280, 'dark'), (390, 'light'))]
        for _, _, _, sid in cases:
            corpus.put(sid, 'claude', [claude_row(sid, 'user', 'seed-user', None,
                '恢复研究会话 / Resume saved conversation with SDK unavailable', cwd=str(root / 'work'))], [])
        launcher = root / 'launcher.json'
        launcher.write_text(json.dumps({'schema': 2,
            'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
            'profiles': [{'id': 'claude-cli-v1', 'source': 'claude',
                'executable': str(Path(sys.executable).resolve()),
                'args': [str(REPO / 'tests/fake_claude_cli.py'), '--reply'],
                'new_args': ['--session-id', '{session_id}'], 'resume_args': ['--resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'), 'TERM': 'xterm-256color',
                        'LANG': 'C.UTF-8', 'SESSIONDOCK_TEST_CLAUDE_ROOT': str(root / 'claude')}}]}))
        launcher.chmod(0o600)
        initialize('--initialize-lifecycle', root / 'ledger', args.binary)
        node = SimpleNamespace(nid='a' * 32, name='StartNode', token='f' * 64, port=free_port())
        extra = {'SESSIONDOCK_NATIVE_SDKS': 'on', 'SESSIONDOCK_NODE_BIND': f'127.0.0.1:{node.port}',
                 'SESSIONDOCK_NODE_PEERS': '127.0.0.0/8'}
        for key, value in (('TOKEN', node.token), ('ID', node.nid)):
            path = root / f'node-{key.lower()}'
            path.write_text(value)
            path.chmod(0o600)
            extra[f'SESSIONDOCK_NODE_{key}_FILE'] = str(path)
        with isolated_server(corpus, args.binary, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                state_dir=root / 'state', launcher_config=launcher, extra_env=extra) as (base, _):
            hub = Hub(args.hub_binary, root / 'hub', [node])
            hub.start()
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            try:
                for hub_mode, width, theme, sid in cases:
                    exercise(browser, f'http://127.0.0.1:{hub.port}' if hub_mode else base,
                             corpus, sid, node if hub_mode else None, width, theme, args.screenshots)
            finally:
                browser.close()
                hub.stop()
    print('PASS SDK view → start session → visible connected PTY, typed input, history and discovery refresh; node/Hub, desktop/mobile, light/dark', flush=True)


if __name__ == '__main__':
    main()
