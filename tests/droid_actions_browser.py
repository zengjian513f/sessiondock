#!/usr/bin/env python3
"""Droid native session actions through real Node/Hub Chromium interfaces."""
import argparse
import json
from contextlib import ExitStack
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import sync_playwright, expect
from claude_sdk_browser import open_sdk, send, wait_snapshot, receipt
from droid_sdk_fixtures import DroidFixture
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import node_env, free_port, TOKEN
from sdk_usage_fixtures import reveal, close_more, assert_inline_usage


def calls(fixture, method):
    return [frame for frame in fixture.frames() if frame.get('method') == method]


def repeat(page, base, response, *, ok=True, changes=None):
    body = {**response.request.post_data_json, **(changes or {})}
    result = page.request.post(base + '/api/native-sdk/' + response.url.rsplit('/', 1)[1], data=body)
    assert result.ok == ok, result.text()
    return receipt(result.json())


def capture(page, out, name):
    if out:
        out.mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(out / (name + '.png')), full_page=True)


def confirm(page, label):
    page.locator('dialog.app-popup[open]').get_by_role('button', name=label, exact=True).click()


def wait_selected(page, uid):
    try:
        page.wait_for_function('uid => S.sel === uid && NativeSdkView.uid === uid && NativeSdkView.connected', arg=uid)
    except Exception:
        print('selected diagnostic', page.evaluate('({selected:S.sel,uid:NativeSdkView.uid,connected:NativeSdkView.connected,openFailed:NativeSdkView.openFailed,notice:NativeSdkView.notice?.textContent})'), flush=True)
        page.screenshot(path='/tmp/sessiondock-droid-actions-failure.png', full_page=True)
        raise
    expect(page.locator('#sdk-send')).to_be_enabled()


def check(page, base, fixture, out, hub_mode):
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base, wait_until='networkidle')
    row = page.locator('#side .item[data-uid^="droid:"]').first
    expect(row).to_be_visible()
    uid = row.get_attribute('data-uid')
    assert ('~' in uid) == hub_mode, uid
    open_sdk(page, fixture, expect, uid=uid)
    expect(page.locator('.sdk-history')).to_contain_text('earlier Droid answer')

    # Select the native modes and await the server-confirmed settings.
    reveal(page, '#native-sdk-mode').select_option('spec')
    wait_snapshot(page, uid, lambda raw: raw.get('settings', {}).get('interactionMode') == 'spec')
    reveal(page, '#native-sdk-autonomy_level').select_option('medium')
    wait_snapshot(page, uid, lambda raw: raw.get('settings', {}).get('autonomyLevel') == 'medium')
    expect(page.locator('#native-sdk-mode')).to_have_value('spec')
    expect(page.locator('#native-sdk-autonomy_level')).to_have_value('medium')
    settings = calls(fixture, 'droid.update_session_settings')
    assert settings[-2]['params'] == {'interactionMode':'spec'}, settings
    assert settings[-1]['params'] == {'autonomyLevel':'medium'}, settings
    (fixture.root / 'settings-fail').touch()
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/settings')) as settings_failed:
        reveal(page, '#native-sdk-autonomy_level').select_option('high')
    assert not settings_failed.value.ok
    expect(page.locator('#native-sdk-autonomy_level')).to_have_value('medium')
    (fixture.root / 'settings-fail').unlink()
    close_more(page)

    # Rename via the real composer dialog, then reload the native read model.
    send(page, '/rename')
    dialog = page.get_by_role('dialog', name='重命名会话', exact=True)
    dialog.get_by_role('textbox', name='会话名称').fill('Droid actions renamed')
    capture(page, out, 'rename-wide-light')
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/rename')) as renamed:
        dialog.get_by_role('button', name='保存', exact=True).click()
    assert renamed.value.ok, renamed.value.text()
    expect(page.locator('.dhead h2')).to_have_text('Droid actions renamed')
    expect(page.locator('#sdk-input')).to_have_value('')
    expect(page.locator('#sdk-send')).not_to_have_attribute('aria-busy', 'true')
    repeat(page, base, renamed.value)
    assert len(calls(fixture, 'droid.rename_session')) == 1
    page.reload(wait_until='networkidle')
    open_sdk(page, fixture, expect, uid=uid)
    expect(page.locator('.dhead h2')).to_have_text('Droid actions renamed')
    expect(page.locator('#sdk-input')).to_have_value('')
    assert_inline_usage(page)
    capture(page, out, 'settings-wide-light')

    # A pending successful rename clears only its own command, never a newer draft.
    hold = fixture.root / 'rename-hold'; hold.touch()
    with page.expect_request(lambda request: request.url.endswith('/api/native-sdk/rename')):
        send(page, '/rename Droid delayed renamed')
    page.locator('#sdk-input').fill('keep newer draft')
    hold.unlink()
    expect(page.locator('.dhead h2')).to_have_text('Droid delayed renamed')
    expect(page.locator('#sdk-input')).to_have_value('keep newer draft')
    expect(page.locator('#sdk-send')).not_to_have_attribute('aria-busy', 'true')
    page.reload(wait_until='networkidle')
    open_sdk(page, fixture, expect, uid=uid)
    expect(page.locator('#sdk-input')).to_have_value('keep newer draft')
    page.locator('#sdk-input').fill('')

    for width, theme in [(1280,'dark'), (390,'light'), (390,'dark')]:
        page.set_viewport_size({'width':width,'height':900})
        page.emulate_media(color_scheme=theme)
        open_sdk(page, fixture, expect, uid=uid)
        reveal(page, '#native-sdk-mode')
        expect(page.locator('#native-sdk-autonomy_level')).to_have_value('medium')
        capture(page, out, f'settings-{width}-{theme}')
        close_more(page)
    page.set_viewport_size({'width':1280,'height':900})
    page.emulate_media(color_scheme='light')
    open_sdk(page, fixture, expect, uid=uid)

    # Cancellation makes no RPC; a real fork navigates to the returned identity.
    reveal(page, '[data-sdk-action="fork"]').click()
    confirm(page, '取消')
    assert not calls(fixture, 'droid.fork_session')
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/fork')) as forked:
        reveal(page, '[data-sdk-action="fork"]').click()
        confirm(page, '分支')
    assert forked.value.ok, forked.value.text()
    fork = receipt(forked.value.json())
    assert fork['uid'] != uid and fork['session_id'] != fixture.sid, fork
    assert ('~' in fork['uid']) == hub_mode, fork
    wait_selected(page, fork['uid'])
    expect(page.locator('.sdk-history')).to_contain_text('earlier Droid answer')
    assert repeat(page, base, forked.value)['uid'] == fork['uid']
    repeat(page, base, forked.value, ok=False, changes={'title':'different duplicate payload'})
    assert len(calls(fixture, 'droid.fork_session')) == 1
    send(page, 'continue fork')
    expect(page.locator('.native-sdk-pane')).to_contain_text('Droid final continue fork')
    wait_snapshot(page, fork['uid'], lambda raw: not raw['running'])
    assert 'continue fork' not in fixture.path.read_text(), 'fork input reached the source history'
    capture(page, out, 'fork-wide-light')

    # Ordinary native compaction stays on this session and refreshes history.
    reveal(page, '[data-sdk-action="compact"]').click()
    confirm(page, '取消')
    assert not calls(fixture, 'droid.compact_session')
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/compact')) as compacted:
        reveal(page, '[data-sdk-action="compact"]').click()
        confirm(page, '压缩')
    assert compacted.value.ok, compacted.value.text()
    compact = receipt(compacted.value.json())
    assert compact.get('uid', fork['uid']) == fork['uid'], compact
    compact['uid'] = fork['uid']
    wait_selected(page, compact['uid'])
    assert repeat(page, base, compacted.value).get('uid', compact['uid']) == compact['uid']
    repeat(page, base, compacted.value, ok=False, changes={'custom_instructions':'different duplicate payload'})
    assert len(calls(fixture, 'droid.compact_session')) == 1
    wait_snapshot(page, compact['uid'], lambda raw: (raw.get('context_usage') or {}).get('used_tokens') == 8000)
    send(page, 'continue compact')
    expect(page.locator('.native-sdk-pane')).to_contain_text('Droid final continue compact')
    wait_snapshot(page, compact['uid'], lambda raw: not raw['running'])
    capture(page, out, 'compact-wide-light')

    # A native new-session reply is also supported, without silently retaining the old UID.
    (fixture.root / 'compact-new-session').touch()
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/compact')) as handed:
        reveal(page, '[data-sdk-action="compact"]').click()
        confirm(page, '压缩')
    assert handed.value.ok, handed.value.text()
    continued = receipt(handed.value.json())
    assert continued['uid'] != compact['uid'], continued
    wait_selected(page, continued['uid'])
    expect(page.locator('.sdk-history')).to_contain_text('Earlier conversation preserved as a compact summary')
    assert repeat(page, base, handed.value)['uid'] == continued['uid']
    repeat(page, base, handed.value, ok=False, changes={'custom_instructions':'different duplicate payload'})
    assert len(calls(fixture, 'droid.compact_session')) == 2
    send(page, 'continue new compact')
    expect(page.locator('.native-sdk-pane')).to_contain_text('Droid final continue new compact')
    wait_snapshot(page, continued['uid'], lambda raw: not raw['running'])
    if not page.locator(f'#side .item[data-uid="{compact["uid"]}"]').count():
        page.locator('#a-fork-chain').click()
        page.locator(f'#fork-chain-menu .chain-row[data-uid="{compact["uid"]}"] .chain-toggle[data-visible="1"]').click()
        page.keyboard.press('Escape')
    open_sdk(page, fixture, expect, uid=compact['uid'])
    send(page, 'return to compact source')
    expect(page.locator('.native-sdk-pane')).to_contain_text('Droid final return to compact source')
    wait_snapshot(page, compact['uid'], lambda raw: not raw['running'])
    child_path = fixture.path.with_name(continued['session_id'] + '.jsonl')
    assert 'return to compact source' not in child_path.read_text(), 'source reopening reused the new session transport'
    assert 'return to compact source' in fixture.path.with_name(fork['session_id'] + '.jsonl').read_text()
    open_sdk(page, fixture, expect, uid=continued['uid'])
    compact = continued
    capture(page, out, 'compact-new-session-wide-light')

    # Native failures stay visible and retries of the same HTTP mutation do not dispatch twice.
    (fixture.root / 'fork-fail').touch()
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/fork')) as rejected:
        reveal(page, '[data-sdk-action="fork"]').click()
        confirm(page, '分支')
    assert not rejected.value.ok
    assert page.evaluate('S.sel') == compact['uid']
    expect(page.locator('#sdk-status-menu')).to_contain_text('Synthetic fork failure')
    repeat(page, base, rejected.value, ok=False)
    assert len(calls(fixture, 'droid.fork_session')) == 2
    capture(page, out, 'native-failure-wide-light')
    (fixture.root / 'fork-fail').unlink()

    # Native message rewind creates a branch before the selected message, without touching files.
    source_path = fixture.path.with_name(continued['session_id'] + '.jsonl')
    original_rows = [json.loads(line) for line in source_path.read_text().splitlines()]
    selected = next(row for row in original_rows if row.get('message', {}).get('content') == [{'type':'text','text':'continue new compact'}])
    bubble = page.locator('.sdk-history .msg[data-role="user"], .sdk-live .msg[data-role="user"]').filter(has_text='continue new compact')
    with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/fork')) as rewound:
        bubble.get_by_role('button', name='从此处之前分支', exact=True).click()
        capture(page, out, 'message-fork-confirm-wide-light')
        confirm(page, '分支')
    assert rewound.value.ok, rewound.value.text()
    partial = receipt(rewound.value.json())
    wait_selected(page, partial['uid'])
    expect(page.locator('#sdk-input')).to_have_value('continue new compact')
    expect(page.locator('.sdk-history')).to_contain_text('Earlier conversation preserved as a compact summary')
    expect(page.locator('.sdk-msgs')).not_to_contain_text('continue new compact')
    native = calls(fixture, 'droid.execute_rewind')
    assert len(native) == 1, native
    params = native[0]['params']
    assert params['sessionId'] == continued['session_id'] and params['messageId'] == selected['id'], params
    assert params['filesToRestore'] == [] and params['filesToDelete'] == [], params
    assert repeat(page, base, rewound.value)['uid'] == partial['uid']
    repeat(page, base, rewound.value, ok=False, changes={'before':'different-message'})
    assert len(calls(fixture, 'droid.execute_rewind')) == 1
    child_rows = [json.loads(line) for line in fixture.path.with_name(partial['session_id'] + '.jsonl').read_text().splitlines()]
    boundary = original_rows.index(selected)
    assert child_rows[1:] == original_rows[1:boundary]
    assert child_rows[0]['parent'] == continued['session_id']
    assert child_rows[0]['forkedAtMessageId'] == original_rows[boundary - 1]['id']
    assert [json.loads(line) for line in source_path.read_text().splitlines()] == original_rows
    send(page, 'continue message fork')
    expect(page.locator('.native-sdk-pane')).to_contain_text('Droid final continue message fork')
    wait_snapshot(page, partial['uid'], lambda raw: not raw['running'])
    capture(page, out, 'message-fork-wide-light')
    assert not any(frame.get('params', {}).get('text', '').startswith('/') for frame in fixture.frames())
    assert not errors, errors


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-droid-actions-browser'))
    args = parser.parse_args()
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        try:
            for hub_mode in (False, True):
                with tempfile.TemporaryDirectory(prefix='sessiondock-droid-actions-') as directory, ExitStack() as stack:
                    root = Path(directory)
                    fixture = DroidFixture(root)
                    port = free_port()
                    extra = node_env(root, port, '127.0.0.0/8') if hub_mode else None
                    base = stack.enter_context(fixture.server(BINARY, extra))
                    if hub_mode:
                        hub_root = root / 'hub'; hub_root.mkdir()
                        hub = Hub(BINARY.with_name('sessiondock-hub'), hub_root,
                                  [SimpleNamespace(name='DroidActionsNode', token=TOKEN, port=port)])
                        hub.start(); stack.callback(hub.stop)
                        base = f'http://127.0.0.1:{hub.port}'
                    context = browser.new_context(viewport={'width':1280, 'height':900}, color_scheme='light')
                    try:
                        page = context.new_page()
                        check(page, base, fixture, args.screenshots / ('hub' if hub_mode else 'node'), hub_mode)
                    finally:
                        context.close()
        finally:
            browser.close()
    print('Droid actions Chromium Node + Hub: PASS')


if __name__ == '__main__':
    main()
