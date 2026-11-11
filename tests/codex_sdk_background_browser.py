#!/usr/bin/env python3
"""Codex SDK background work agrees with the sidebar across node/Hub.

Real HTTP/SSE and Chromium; synthetic proc, native records and app-server only.
"""
import argparse
from pathlib import Path
import re
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD, build, server
from history_fixtures import BINARY, codex_row, codex_message
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from spawned_by_fixtures import proc_pid


from codex_sdk_status_browser import StatusDaemon


def exercise(browser, base, fake, width, theme, screenshots, label):
    print(label, 'SDK ready, local/remote background, completion, editable draft', flush=True)
    context = browser.new_context(viewport={'width': width, 'height': 900},
                                  color_scheme=theme, service_workers='block')
    page = context.new_page()
    page.set_default_timeout(15000)
    errors = []
    page.on('pageerror', lambda error: errors.append(error.stack))
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded && S.sessions.length === 2')
    uid = page.evaluate('S.sessions.find(r => r.sid === "' + THREAD + '").uid')
    independent = page.evaluate('S.sessions.find(r => r.sid === "external-cli-thread").uid')
    card = page.locator(f'#side .item[data-uid="{uid}"]')
    external = page.locator(f'#side .item[data-uid="{independent}"]')
    marker = card.locator(':scope > .ico > .item-status')
    # The daemon already owns this thread, before this browser ever opens it.
    expect(marker).to_have_class(re.compile(r'\bsdk\b'))
    expect(external.locator(':scope > .ico > .item-status')).not_to_have_class(re.compile(r'\bsdk\b'))
    if screenshots:
        page.screenshot(path=str(screenshots / (label + '-before-click.png')))
    # A slow/partial fleet read can lack SDK ownership while this exact
    # thread's independent SDK stream has already delivered its snapshot.
    background = {'kind': ''}
    def stale_live(route):
        response = route.fetch()
        data = response.json()
        data['sdk_uids'] = []
        data['working_uids'] = [uid] if background['kind'] == 'local' else []
        if background['kind'] == 'remote':
            row = page.evaluate('uid => S.sessions.find(r => r.uid === uid)', uid)
            data['remote_working'] = [{'node_id': row.get('node_id', ''),
                                       'source': 'codex', 'sid': THREAD}]
        route.fulfill(response=response, json=data)
    page.route('**/api/live*', stale_live)
    card.click()
    expect(page.locator('#sdk-model')).to_have_value('fake-sol')
    page.wait_for_function('CodexSdkView.connected')
    expect(page.locator('#dlive')).to_have_class(re.compile(r'\bsdk\b'))
    expect(marker).to_have_class(re.compile(r'\bsdk\b'))
    expect(marker).to_have_class(re.compile(r'\bvisible\b'))
    expect(page.locator('#dlive')).to_have_attribute('title', re.compile('受管 SDK'))
    data = page.request.get(base + '/api/live').json()
    assert uid in data['sdk_uids'] and uid in data['uids'], data
    assert uid not in data['tmux_uids'], 'SDK must not claim a terminal'
    # Use the existing blue token, including dark mode; no DOM mutation.
    assert marker.evaluate("e => getComputedStyle(e).backgroundColor") == page.locator('#dlive').evaluate(
        "e => { const c = getComputedStyle(e); return c.backgroundColor; }")
    if screenshots:
        page.screenshot(path=str(screenshots / (label + '-sdk.png')))
    # A completed native turn can still own local or cross-node work.
    # Fleet-only changes must update both the marker and SDK footer.
    for kind in ('local', 'remote'):
        background['kind'] = kind
        page.evaluate('refreshLive(true)')
        expect(marker).to_have_class(re.compile(r'\bturn-working\b'))
        expect(page.locator('#sdk-composer .sdk-status')).to_contain_text('后台运行')
        expect(page.locator('#sdk-send')).to_have_text('发送')
        page.locator('#sdk-input').fill('background draft')
        expect(page.locator('#sdk-send')).to_be_enabled()
        page.locator('#sdk-composer .sdk-status').click()
        expect(page.locator('#sdk-status-menu')).to_contain_text('主回合已结束')
        page.keyboard.press('Escape')
        if screenshots:
            page.screenshot(path=str(screenshots / (label + '-' + kind + '-background.png')))
        background['kind'] = ''
        page.evaluate('refreshLive(true)')
        expect(marker).not_to_have_class(re.compile(r'\bturn-working\b'))
        expect(page.locator('#sdk-composer .sdk-status')).to_contain_text('就绪')
        expect(page.locator('#sdk-input')).to_have_value('background draft')
    page.locator('#sdk-input').fill('')
    assert not errors, errors
    context.close()
    fake.disconnect()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--hub-binary', type=Path)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    expect.set_options(timeout=20000)
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-status-') as tmp:
        root = Path(tmp)
        corpus = build(root)
        corpus.put('sdk', 'codex', [codex_row('session_meta', {'id': THREAD, 'cwd': str(root)}),
                   codex_message('user', 'SDK 状态回归：已完成的历史仍属于受管会话 Managed SDK'),
                   codex_row('event_msg', {'type': 'task_complete'})], [])
        corpus.put('external', 'codex', [codex_row('session_meta', {
                   'id': 'external-cli-thread', 'cwd': str(root)}),
                   codex_message('user', 'Independent CLI 独立进程')], [])
        proc = root / 'proc'
        proc.mkdir()
        (proc / 'stat').write_text('btime 1700000000\n')
        proc_pid(proc, 400, 'codex', ['codex', 'app-server'], 1, fds={3: corpus.paths['sdk']})
        proc_pid(proc, 500, 'codex', ['codex', 'resume', 'external-cli-thread'], 1,
                 fds={3: corpus.paths['external']})
        port = free_port()
        env = node_env(root, port, '127.0.0.0/8')
        env['SESSIONDOCK_PROC_ROOT'] = str(proc)
        with StatusDaemon(root / 'app.sock', corpus.paths['sdk'], THREAD) as fake:
            with server(corpus, args.binary, root / 'app.sock', env) as base:
                hub_root = root / 'hub'
                hub_root.mkdir()
                hub = Hub((args.hub_binary or args.binary.with_name('sessiondock-hub')).resolve(),
                          hub_root, [SimpleNamespace(name='SdkNode', token=TOKEN, port=port)])
                hub.start()
                try:
                    with sync_playwright() as pw:
                        browser = pw.chromium.launch(headless=True)
                        try:
                            for mode, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
                                for width, theme in [(1280, 'light'), (390, 'dark'), (1280, 'dark'), (390, 'light')]:
                                    exercise(browser, url, fake, width, theme, args.screenshots,
                                             f'{mode}-{width}-{theme}')
                        finally:
                            browser.close()
                finally:
                    hub.stop()
    print('PASS SDK background status: node/Hub, local/remote work, completion, draft, widths/themes')


if __name__ == '__main__':
    main()
