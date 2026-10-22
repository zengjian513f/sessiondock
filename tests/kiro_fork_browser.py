#!/usr/bin/env python3
"""Kiro rewind via native message-scoped fork: node/Hub, history/live, capability gating."""
from __future__ import annotations

import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from codex_sdk_browser import server
from history_fixtures import BINARY
from hub_fixtures import Hub, free_port
from kiro_sdk_browser import api, api_raw
from kiro_sdk_fixtures import KiroFixture
from node_auth_fixtures import TOKEN, node_env
from sdkhost_fixtures import stop_private_sdkhosts
from send_browser import initialize


def exercise(browser, base, fixture, label, width, theme, screenshots, baseline=False):
    context = browser.new_context(viewport={"width": width, "height": 900}, color_scheme=theme,
                                  service_workers="block")
    page = context.new_page()
    page.set_default_timeout(20000)
    page.goto(base, wait_until="domcontentloaded")
    page.wait_for_function("async sid => { await loadSessions(true); return S.sessions.some(s => s.sid === sid); }", arg=fixture.sid)
    uid = page.evaluate("sid => S.sessions.find(s => s.sid === sid).uid", fixture.sid)
    if page.evaluate('uid => !!S.sessions.find(s=>s.uid===uid)?.fork_parent', uid):
        page.evaluate('uid => setForkParentVisibility([uid], true)', uid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.locator('#sdk-input').click()
    page.wait_for_function("SdkView.connected && !SdkView.readOnly")
    if not baseline:
        no_identity = page.locator('.sdk-live .msg[data-role="user"]').filter(has_text='审查第 42 轮：')
        expect(no_identity).to_have_count(1)
        expect(no_identity.locator('.sdk-bubble-toggle')).to_have_count(0)
    # Older turns are projected history; newer turns arrive in the live snapshot.
    turn = 4 if theme == 'light' else 43
    bubble = page.locator('.sdk-history .msg[data-role="user"], .sdk-live .msg[data-role="user"]').filter(has_text=f'审查第 {turn:02d} 轮：')
    expect(bubble).to_have_count(1)
    bubble.locator('.sdk-bubble-toggle').click()
    menu = page.locator('.sdk-bubble-menu:popover-open')
    if screenshots:
        page.screenshot(path=str(screenshots / f'{label}-{width}-{theme}-menu.png'), full_page=True)
    if baseline:
        expect(menu).to_contain_text('从此处之前分支')
        with page.expect_response(lambda r: r.url.endswith('/api/native-sdk/fork')) as failed:
            menu.get_by_role('button', name='从此处之前分支', exact=True).click()
        assert 'kiro_sdk_fork_position_unavailable' in failed.value.text(), failed.value.text()
        print('REPRODUCED: message branch is offered but backend rejects its position', flush=True)
        context.close()
        return
    expect(menu.get_by_role('button')).to_have_text(['回退', '分支'])
    action = '回退' if width == 390 else '分支'
    if action == '回退':
        page.route('**/api/native-sdk/fork', lambda route: route.fulfill(status=503, json={'error': 'synthetic fork failure'}))
        menu.get_by_role('button', name=action, exact=True).click()
        page.wait_for_function("document.querySelector('#sdk-status-menu').textContent.includes('synthetic fork failure')")
        assert page.evaluate('uid => S.sel===uid && !sessionHidden(S.sessions.find(s=>s.uid===uid))', uid)
        page.unroute('**/api/native-sdk/fork')
        bubble.locator('.sdk-bubble-toggle').click()
    native_before = (fixture.v3_dir / 'messages.jsonl').read_bytes()
    with page.expect_response(lambda r: r.url.endswith('/api/native-sdk/fork')) as fork:
        menu.get_by_role('button', name=action, exact=True).click()
    assert fork.value.ok, fork.value.text()
    result = fork.value.json()
    new_uid = result['uid']
    assert new_uid != uid and ('~' in new_uid) == ('~' in uid), result
    page.wait_for_function('uid => S.sel === uid && SdkView.uid === uid && SdkView.connected && !SdkView.readOnly', arg=new_uid)
    expect(page.locator('.sdk-live')).to_contain_text(f'第 {turn:02d} 轮审查完成')
    expect(page.locator('#sdk-input')).to_have_value('')
    expect(page.locator('.native-history-warning:visible')).to_have_count(0)
    page.wait_for_function('turn => !SdkView.items.some(i => i.text?.includes(`审查第 ${String(turn+1).padStart(2,"0")} 轮：`))', arg=turn)
    try:
        page.wait_for_function('([uid, visible]) => { const row=S.sessions.find(s=>s.uid===uid); return row?.fork_parent && !!row.fork_parent_visible===visible; }', arg=[uid, action == '分支'])
    except Exception:
        print('VISIBILITY DIAGNOSTICS', page.evaluate('S.sessions'), flush=True)
        raise
    request = fork.value.request.post_data_json
    assert 'hide_source' not in request
    assert request['messageId'] == f'user-{turn:02d}', request
    params = fixture.received('session/fork')[-1]['params']
    assert params['_meta']['kiro'] == {'messageId': f'user-{turn:02d}', 'createdReason': 'rewind'}, params
    assert 'messageId' not in params and 'before' not in params
    native_fork = fixture.sessions / 'bucket_v3' / result['session_id'] / 'messages.jsonl'
    rows = [json.loads(line) for line in native_fork.read_text().splitlines()]
    assert len(rows) == (turn + 1) * 2 and rows[-1]['id'] == f'answer-{turn:02d}'
    assert (fixture.v3_dir / 'messages.jsonl').read_bytes() == native_before
    forks = len(fixture.received('session/fork'))
    assert api(page, 'fork', request)['uid'] == new_uid
    assert len(fixture.received('session/fork')) == forks, 'Retry duplicated the branch'
    if screenshots:
        page.screenshot(path=str(screenshots / f'{label}-{width}-{theme}-fork.png'), full_page=True)
    page.reload(wait_until='domcontentloaded')
    try:
        page.wait_for_function('uid => SdkView.uid === uid && SdkView.connected', arg=new_uid)
    except Exception:
        print('RELOAD DIAGNOSTICS', page.evaluate('''() => ({url:location.href, selected:S.sel,
          uid:SdkView.uid, connected:SdkView.connected, readOnly:SdkView.readOnly,
          live:[...S.liveSdk], row:S.sessions.find(s=>s.uid===S.sel),
          text:document.querySelector('#detail')?.innerText})'''), flush=True)
        raise
    expect(page.locator('.sdk-live')).to_contain_text(f'第 {turn:02d} 轮审查完成')
    page.wait_for_function('([uid, visible]) => { const row=S.sessions.find(s=>s.uid===uid); return row?.fork_parent && !!row.fork_parent_visible===visible; }', arg=[uid, action == '分支'])
    if action == '回退':
        page.locator('#a-fork-chain').click()
        toggle = page.locator(f'#fork-chain-menu .chain-row[data-uid="{uid}"] .chain-toggle')
        expect(toggle).to_have_text('显示')
        toggle.click()
        expect(toggle).to_have_text('隐藏')
        toggle.click()
        expect(toggle).to_have_text('显示')
        page.locator('#a-fork-chain').click()
    assert not fixture.received('session/prompt'), 'Fork/load unexpectedly sent a prompt'
    print(f'PASS {label} {width} {theme}: native ID, retained turn, source preserved, retry, reload', flush=True)
    context.close()


def capability_gating(browser, base, fixture):
    page = browser.new_page()
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('async sid => { await loadSessions(true); return S.sessions.some(s => s.sid === sid); }', arg=fixture.sid)
    uid = page.evaluate('sid => S.sessions.find(s => s.sid === sid).uid', fixture.sid)
    api(page, 'stop', {'uid': uid})
    marker = fixture.sessions / 'no-fork-message-id'
    marker.touch()
    if page.evaluate('uid => !!S.sessions.find(s=>s.uid===uid)?.fork_parent', uid):
        page.evaluate('uid => setForkParentVisibility([uid], true)', uid)
    page.locator(f'#side .item[data-uid="{uid}"]').click()
    page.locator('#sdk-input').click()
    page.wait_for_function('SdkView.connected && SdkView.capabilities.fork_at_message === false')
    expect(page.locator('.sdk-bubble-toggle')).to_have_count(0)
    assert page.evaluate('SdkView.capabilities.fork === true')
    forks = len(fixture.received('session/fork'))
    denied = api_raw(page, 'fork', {'uid': uid, 'messageId': 'user-00', 'request_id': 'not-advertised'})
    assert 'kiro_sdk_fork_position_unavailable' in json.dumps(denied), denied
    assert len(fixture.received('session/fork')) == forks
    full = api(page, 'fork', {'uid': uid, 'request_id': 'full-without-position-support'})
    assert full['uid'] != uid
    marker.unlink()
    bad = api_raw(page, 'fork', {'uid': uid, 'messageId': 'unknown-native-id', 'request_id': 'unknown-position'})
    assert 'MESSAGE_NOT_FOUND' in json.dumps(bad), bad
    assert not fixture.received('session/prompt')
    print('PASS missing position capability: hidden per-turn action, backend refusal, whole-session fork retained; invalid ID surfaces native error', flush=True)
    page.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    parser.add_argument('--baseline', action='store_true')
    parser.add_argument('--only', choices=['node', 'hub'])
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-kfork-') as temporary:
        root = Path(temporary)
        for name in ('claude', 'codex', 'grok', 'state'):
            (root / name).mkdir()
        fixture = KiroFixture(root, args.binary.resolve().with_name('ptyhost'))
        rows = []
        for turn in range(45):
            for kind, mid, text in [('user', 'user', f'审查第 {turn:02d} 轮：核对会话历史、工具调用与恢复边界。'),
                                    ('assistant', 'answer', f'第 {turn:02d} 轮审查完成，保留此前已确认的上下文。')]:
                rows.append({'id': None if kind == 'user' and turn == 42 else f'{mid}-{turn:02d}', 'timestamp': '2026-10-22T00:00:00Z',
                             'message': {'kind': kind, 'data': {'content': [{'kind': 'text', 'data': text}]}}})
        (fixture.v3_dir / 'messages.jsonl').write_text(''.join(json.dumps(r, ensure_ascii=False) + '\n' for r in rows))
        initialize('--initialize-lifecycle', root / 'ledger', args.binary.resolve())
        port = free_port()
        env = {**fixture.env, **node_env(root, port, '127.0.0.0/8'),
               'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_LIFECYCLE_DIR': str(root / 'ledger'),
               'SESSIONDOCK_STATE_DIR': str(root / 'state'), 'SESSIONDOCK_PTYHOST_DIR': str(fixture.host_dir)}
        try:
            with ExitStack() as stack:
                base = stack.enter_context(server(SimpleNamespace(root=root), args.binary, 'off', env))
                hub_root = root / 'hub'
                hub_root.mkdir()
                hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                          [SimpleNamespace(name='KiroNode', token=TOKEN, port=port)])
                hub.start()
                stack.callback(hub.stop)
                with sync_playwright() as p:
                    browser = p.chromium.launch(headless=True)
                    try:
                        for label, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
                            if args.only and args.only != label:
                                continue
                            for width, theme in [(1280, 'light'), (390, 'dark'), (1280, 'dark'), (390, 'light')]:
                                exercise(browser, url, fixture, label, width, theme, args.screenshots, args.baseline)
                                if args.baseline:
                                    return
                        capability_gating(browser, base, fixture)
                    finally:
                        browser.close()
        finally:
            stop_private_sdkhosts(root)
    print('Kiro message fork browser: PASS')


if __name__ == '__main__':
    main()
