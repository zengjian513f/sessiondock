#!/usr/bin/env python3
"""Use an existing Codex child subscription without resuming the parent or a sibling."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import urlencode

from playwright.sync_api import expect, sync_playwright

from codex_sdk_subagent_browser import CHILD, NESTED, SIBLING, THREAD, SubagentServer, add_children, build, server
from history_fixtures import BINARY, batch35_agent_meta, codex_message, codex_row
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env


NATIVE_CHILD = '0199a100-0000-7000-8000-000000000005'


def exercise_native_child(browser, base, fake, screenshot):
    context = browser.new_context(viewport={'width': 390, 'height': 850}, color_scheme='dark', service_workers='block')
    page = context.new_page()
    page.set_default_timeout(15000)
    before = len(fake.methods('thread/resume'))
    page.goto(base + '/?' + urlencode({'sid': f'codex:{THREAD}/agent:{NATIVE_CHILD}'}))
    expect(page.locator('.sdk-history')).to_contain_text('Native child sends its own progress')
    uid = page.evaluate('S.sel')
    page.wait_for_function('key => S.liveSdkChildren.has(key) && !S.liveSdk.has(key)', arg=f'{uid}:{NATIVE_CHILD}')
    expect(page.locator('#dlive')).to_have_attribute('title', '子代理运行中')
    expect(page.locator('#sdk-composer .sdk-status [role=status]')).to_have_text('生成回复')
    expect(page.locator('#sdk-composer .composer-run-action')).to_have_text('连接')
    page.locator('#sdk-composer .sdk-status').click()
    expect(page.locator('#sdk-status-menu:popover-open')).to_contain_text('字符回复')
    if screenshot:
        page.screenshot(path=str(screenshot))
    assert len(fake.methods('thread/resume')) == before
    context.close()


def exercise(browser, base, fake, child, width, theme, screenshot):
    context = browser.new_context(viewport={'width': width, 'height': 850},
                                  color_scheme=theme, service_workers='block')
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    page = context.new_page()
    page.set_default_timeout(15000)
    url = base + '/?' + urlencode({'sid': f'codex:{THREAD}/agent:{child}'})
    page.goto(url)
    expect(page.locator('.sdk-history')).to_contain_text('Child history sentinel ' + child)
    before = len(fake.methods('thread/resume'))
    page.locator('#sdk-composer .composer-run-action').click()
    page.wait_for_function('SdkView.connected && !ConsoleUI.busy.has(S.sel)')
    assert [m['params']['threadId'] for m in fake.methods('thread/resume')[before:]] == [child]
    page.locator('#sdk-input').fill('hold child status observation')
    page.locator('#sdk-send').click()
    expect(page.locator('#sdk-composer .sdk-status [role=status]')).to_have_text('执行工具')
    expect(page.locator('#dlive')).to_have_class('item-status visible sdk turn-working')
    uid = page.evaluate('S.sel')
    page.wait_for_function('({uid, child}) => S.liveSdk.has(uid + ":" + child)', arg={'uid': uid, 'child': child})

    # Another page gets the exact child stream while the root stays unopened.
    reader = context.new_page()
    opens = []
    reader.on('request', lambda request: opens.append(request.url)
              if request.method == 'POST' and request.url.endswith('/api/codex-sdk/open') else None)
    with reader.expect_request(lambda request: '/api/codex-sdk/events?' in request.url and 'observe=1' in request.url):
        reader.goto(url)
    expect(reader.locator('#sdk-composer .sdk-status [role=status]')).to_have_text('执行工具')
    expect(reader.locator('#dlive')).to_have_class('item-status visible sdk turn-working')
    reader.locator('#sdk-composer .sdk-status').click()
    expect(reader.locator('#sdk-status-menu:popover-open')).to_contain_text('正在运行 1 项工具')
    if screenshot:
        reader.screenshot(path=str(screenshot))
    reader.keyboard.press('Escape')
    reader.locator('#a-view-switch').click()
    expect(reader.locator(f'#session-view-menu [data-agent="{child}"] .view-live.sdk')).to_be_visible()
    expect(reader.locator(f'#session-view-menu [data-agent="{SIBLING}"] .view-live')).to_have_count(0)
    blocked = reader.request.get(base + '/api/codex-sdk/events?' + urlencode(
        {'uid': uid, 'agent': SIBLING, 'observe': 1}))
    assert blocked.status == 503, blocked.text()
    reader.locator(f'#session-view-menu [data-agent="{SIBLING}"]').click()
    expect(reader.locator('.sdk-history')).to_contain_text('Child history sentinel ' + SIBLING)
    expect(reader.locator('#dlive')).to_be_hidden()
    assert not opens, opens
    assert [m['params']['threadId'] for m in fake.methods('thread/resume')[before:]] == [child]
    context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-codex-child-observe-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        corpus = build(root)
        add_children(corpus)
        stamp = datetime.now(timezone.utc).isoformat()
        corpus.put(NATIVE_CHILD, 'codex', [batch35_agent_meta(NATIVE_CHILD, THREAD), *[
            {**row, 'timestamp': stamp} for row in (
                codex_row('event_msg', {'type': 'task_started', 'turn_id': 'native-child-turn'}),
                codex_message('user', 'Work on the delegated task'),
                codex_message('assistant', 'Native child sends its own progress'),
            )]], [], parent='sdk')
        port = free_port()
        with SubagentServer(root/'app.sock', corpus) as fake, server(
            corpus, args.binary, root/'app.sock', node_env(root, port, '127.0.0.0/8')
        ) as node:
            fake.paths[NATIVE_CHILD] = corpus.paths[NATIVE_CHILD]
            fake.loaded.add(NATIVE_CHILD)
            hub_root = root/'hub'
            hub_root.mkdir()
            hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                      [SimpleNamespace(name='SdkNode', token=TOKEN, port=port)])
            hub.start()
            try:
                browser = pw.chromium.launch(headless=True)
                for name, base, child, width, theme in (
                    ('node', node, CHILD, 1280, 'light'),
                    ('hub', f'http://127.0.0.1:{hub.port}', NESTED, 390, 'dark'),
                ):
                    exercise_native_child(browser, base, fake,
                                          args.screenshots / f'codex-native-child-{name}.png' if args.screenshots else None)
                    exercise(browser, base, fake, child, width, theme,
                             args.screenshots / f'codex-child-{name}.png' if args.screenshots else None)
                    print(f'PASS Codex child observation via {name}', flush=True)
                browser.close()
            finally:
                hub.stop()


if __name__ == '__main__':
    main()
