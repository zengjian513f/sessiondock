#!/usr/bin/env python3
"""Create through Chromium and restart a private node before first SEND.

Synthetic Codex daemon and private native/state roots only. Check identity,
drafts, same-request retry, first SEND, native handoff and explicit discard.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import THREAD, build
from codex_sdk_compose_browser import saved
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from new_session_model_browser import open_dialog
from sdk_creation_retention_fixtures import UnloadedCreationServer, node


def create(page, cwd):
    open_dialog(page)
    page.locator('input[name="new-source"][value="codex"]').check()
    page.locator('#new-cwd').fill(str(cwd))
    with page.expect_response('**/api/codex-sdk/create') as response:
        page.locator('#new-session-go').click()
    receipt = response.value.json()
    request = response.value.request.post_data_json
    expect(page.locator('#sdk-input')).to_be_visible()
    return receipt, request


def replay(page, body):
    return page.evaluate("""async body => {
      const response = await fetch('/api/codex-sdk/create', {
        method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body)});
      return {status:response.status, body:await response.json()};
    }""", body)


def unloaded_creation(browser, binary, mode):
    with tempfile.TemporaryDirectory(prefix='sd-unloaded-create-') as temporary:
        root = Path(temporary)
        corpus = build(root)
        state = root / 'state'
        state.mkdir(mode=0o700)
        socket = root / 'app-server-control/app-server-control.sock'
        socket.parent.mkdir()
        port, peer_port = free_port(), free_port()
        env = {**node_env(root, peer_port, '127.0.0.0/8'), 'SESSIONDOCK_CODEX_APP_SERVER': ''}
        context = browser.new_context(service_workers='block')
        page = context.new_page()
        hub_root = root / 'hub'
        hub_root.mkdir()
        hub = None
        try:
            with UnloadedCreationServer(socket, corpus.paths['sdk'], THREAD) as fake:
                with node(corpus, binary, socket, port, state, env) as base:
                    if mode == 'hub':
                        hub = Hub(binary.with_name('sessiondock-hub').resolve(), hub_root,
                                  [SimpleNamespace(name='CreationNode', token=TOKEN, port=peer_port)])
                        hub.start()
                        base = f'http://127.0.0.1:{hub.port}'
                    page.goto(base, wait_until='domcontentloaded')
                    receipt, _ = create(page, root)
                    uid = page.evaluate('S.sel')
                    sid = receipt['thread_id']
                    assert not fake.rollout.exists()
                    page.goto('about:blank')
                # The daemon has no persisted record to read after it unloads.
                fake.loaded.clear()
                with node(corpus, binary, socket, port, state, env) as base:
                    if mode == 'hub':
                        base = f'http://127.0.0.1:{hub.port}'
                    page.goto(base, wait_until='domcontentloaded')
                    page.wait_for_function('T.listLoaded')
                    row = page.locator(f'#side .item[data-uid="{uid}"]')
                    expect(row).to_be_visible()
                    row.click()
                    # Stop is idempotent even when no process owns this thread.
                    row.click(button='right')
                    with page.expect_response('**/api/codex-sdk/stop') as stopped:
                        page.locator('#item-menu [data-act="stop-sdk"]').click()
                        page.locator('dialog.app-popup[open] [data-popup-action="ok"]').click()
                    assert stopped.value.ok, stopped.value.text()
                    expect(row).to_be_visible()
                    before = len(fake.received)
                    # A genuine native failure must remain visible and retryable.
                    fake.fail_unsubscribe = True
                    row.click(button='right')
                    with page.expect_response('**/api/codex-sdk/discard') as failed:
                        page.locator('#item-menu [data-act="delete"]').click()
                    assert failed.value.status == 502, failed.value.text()
                    expect(row).to_be_visible()
                    popup = page.locator('dialog.app-popup[open]')
                    expect(popup).to_contain_text('synthetic unsubscribe failure')
                    popup.locator('[data-popup-action="ok"]').click()
                    fake.fail_unsubscribe = False
                    row.click(button='right')
                    with page.expect_response('**/api/codex-sdk/discard') as discarded:
                        page.locator('#item-menu [data-act="delete"]').click()
                    assert discarded.value.ok, discarded.value.text()
                    expect(row).to_have_count(0)
                    cleanup = fake.received[before:]
                    assert not any(m.get('method') in ('thread/read', 'thread/resume', 'thread/start') for m in cleanup), cleanup
                    assert not fake.rollout.exists()
                    subscriptions = json.loads((state / 'codex-sdk-subscriptions.json').read_text())
                    assert sid not in subscriptions['threads'], subscriptions
                with node(corpus, binary, socket, port, state, env) as base:
                    if mode == 'hub':
                        base = f'http://127.0.0.1:{hub.port}'
                    page.goto(base, wait_until='domcontentloaded')
                    page.wait_for_function('T.listLoaded')
                    expect(row).to_have_count(0)
                    assert len(fake.methods('thread/start')) == 1
                print(f'PASS {mode}: unloaded empty thread stops/discards after restart; native failure stays retryable; no history read or resume', flush=True)
        finally:
            if hub:
                hub.stop()
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-retention-') as temporary, sync_playwright() as p:
        root = Path(temporary)
        corpus = build(root)
        state = root / 'state'
        state.mkdir(mode=0o700)
        port = free_port()
        browser = p.chromium.launch(headless=True)
        context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
        page = context.new_page()
        page.set_default_timeout(10000)
        errors = []
        page.on('pageerror', lambda error: errors.append(error.stack))
        try:
            with UnloadedCreationServer(root / 'app.sock', corpus.paths['sdk'], THREAD) as fake:
                with node(corpus, args.binary, root / 'app.sock', port, state) as base:
                    page.goto(base, wait_until='domcontentloaded')
                    page.wait_for_function('T.listLoaded')
                    receipt, request = create(page, root)
                    uid = receipt['uid']
                    row = page.locator(f'#side .item[data-uid="{uid}"]')
                    expect(row).to_be_visible()
                    assert not fake.rollout.exists()
                    page.locator('#sdk-input').fill('approve 保留更新前尚未发送的新会话草稿')
                    saved(page, uid, 'approve 保留更新前尚未发送的新会话草稿')
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / 'before-restart-empty.png'))
                print('private node stopped before first SEND; restarting with the same state', flush=True)
                with node(corpus, args.binary, root / 'app.sock', port, state) as base:
                    page.reload(wait_until='domcontentloaded')
                    expect(page.locator('#sdk-input')).to_be_visible()
                    expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_be_visible()
                    assert page.evaluate('S.sel') == uid
                    expect(page.locator('#sdk-input')).to_have_value('approve 保留更新前尚未发送的新会话草稿')
                    again = replay(page, request)
                    assert again['status'] == 200 and again['body']['uid'] == uid, again
                    assert len(fake.methods('thread/start')) == 1, 'restart retry spawned another thread'
                    assert not fake.rollout.exists(), 'restart invented a native rollout'
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / 'restart-empty-wide.png'))
                    page.set_viewport_size({'width': 390, 'height': 844})
                    page.emulate_media(color_scheme='dark')
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    expect(page.locator('#sdk-input')).to_be_visible()
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / 'restart-empty-narrow.png'))
                    page.locator('#sdk-send').click()
                    expect(page.locator('.sdk-request')).to_contain_text('需要批准执行命令')
                    page.locator('.sdk-request').get_by_role('button', name='批准', exact=True).click()
                    expect(page.locator('.sdk-history')).to_contain_text('All done after approval')
                    assert fake.rollout.is_file()
                    page.set_viewport_size({'width': 1280, 'height': 900})
                    # Read the native handoff through the ordinary list, then remove only test data.
                    page.evaluate('loadTermList()')
                    native = fake.rollout
                    native.unlink()
                    page.reload(wait_until='domcontentloaded')
                    page.wait_for_function('T.listLoaded')
                    expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_count(0)
                    again = replay(page, request)
                    assert again['body']['uid'] == uid and len(fake.methods('thread/start')) == 1
                    print('PASS restart retains empty identity/draft/retry; native deletion does not resurrect it', flush=True)
                    second, second_request = create(page, root)
                    empty_uid = second['uid']
                    empty_row = page.locator(f'#side .item[data-uid="{empty_uid}"]')
                    empty_row.scroll_into_view_if_needed()
                    page.wait_for_timeout(150)
                    empty_row.click(button='right')
                    page.locator('#item-menu [data-act="delete"]').click()
                    expect(empty_row).to_have_count(0)
                with node(corpus, args.binary, root / 'app.sock', port, state) as base:
                    page.reload(wait_until='domcontentloaded')
                    page.wait_for_function('T.listLoaded')
                    expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_count(0)
                    expect(page.locator(f'#side .item[data-uid="{empty_uid}"]')).to_have_count(0)
                    again = replay(page, second_request)
                    assert again['body']['uid'] == empty_uid and len(fake.methods('thread/start')) == 2
                    assert not fake.rollout.exists()
                    assert not errors, errors
                    print('PASS explicit discard survives restart without recreation', flush=True)
            for mode in ('node', 'hub'):
                unloaded_creation(browser, args.binary, mode)
        finally:
            context.close()
            browser.close()


if __name__ == '__main__':
    main()
