#!/usr/bin/env python3
"""Chromium: configurable inactivity sleep, full-page shade, quiet transport and Resume."""

import json
from pathlib import Path
import sys
import tempfile
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright, expect
from history_fixtures import REPO, BINARY, Corpus, claude_row, isolated_server
from private_hosts import private_hosts
from send_browser import SETTINGS, initialize, create_claude
from draft_sync_browser import wait_server_text


def settings(page):
    if not page.locator('#settings').is_visible():
        page.locator('#header-more-btn').click()
    page.locator('#settings').click()
    page.locator('[data-tab="features"]').click()


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-page-sleep-') as tmp, private_hosts(Path(tmp)), sync_playwright() as pw:
        root = Path(tmp)
        for name in ['host', 'work', 'work/claude-area', 'ledger', 'state', 'bin', 'home', 'claude', 'codex', 'grok']:
            (root / name).mkdir(mode=0o700)
        corpus = Corpus(root)
        wrapper = root / 'bin/fake-claude'
        wrapper.write_text(f'#!/bin/sh\nexec {sys.executable} {REPO / "tests/fake_claude_cli.py"} "$@"\n')
        wrapper.chmod(0o700)
        config = root / 'launcher.json'
        config.touch(mode=0o600)
        config.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
            'host_dir': str(root / 'host'), 'adapters': [], 'profiles': [{
                'id': 'claude-cli-v1', 'source': 'claude', 'executable': str(wrapper),
                'args': ['--settings', SETTINGS], 'new_args': ['--session-id', '{session_id}'],
                'resume_args': ['--resume', '{sid}'], 'env': {'PATH': '/usr/bin:/bin',
                    'HOME': str(root / 'home'), 'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
                    'SESSIONDOCK_TEST_CLAUDE_ROOT': str(root / 'claude')}}]}))
        initialize('--initialize-lifecycle', root / 'ledger')
        browser = pw.chromium.launch(headless=True)
        try:
            with isolated_server(corpus, BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                    launcher_config=config, state_dir=root / 'state') as (base, _):
                context = browser.new_context(viewport={'width':1280, 'height':900}, service_workers='block')
                page = context.new_page(); errors = []; requests = []; sockets = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.on('request', lambda r: requests.append(urlsplit(r.url).path))
                page.on('websocket', lambda ws: sockets.append(ws))
                page.clock.install()
                page.goto(base, wait_until='domcontentloaded')
                page.wait_for_function('T.listLoaded && uiEventsReady')
                receipt = create_claude(page, base, root / 'work', open_terminal=False)
                try:
                    uid = 'tmux:' + receipt['name']
                    page.wait_for_function('!composerDraft().loading')
                    page.fill('#cinput', 'Keep this draft while sleeping')
                    wait_server_text(context, base, uid, 'Keep this draft while sleeping')
                    page.locator('#a-term').click()
                    page.wait_for_function('T.ws?.readyState===WebSocket.OPEN')
                    settings(page)
                    expect(page.locator('#setting-sleep')).to_have_value('60')
                    page.locator('#settings-dialog .modal-close').click()
                    page.clock.fast_forward(59 * 60000)
                    assert not page.evaluate('SessionDockSleep.sleeping')
                    # Real keyboard input restarts the deadline; output/polls do not.
                    page.keyboard.press('Shift')
                    page.clock.fast_forward(59 * 60000)
                    assert not page.evaluate('SessionDockSleep.sleeping')
                    page.clock.fast_forward(61000)
                    dialog = page.locator('#page-sleep-dialog')
                    expect(dialog).to_be_visible()
                    assert page.evaluate('SessionDockNetwork.paused && !uiEvents && !_es && [...T.views.values()].every(v=>!v.ws)')
                    assert sockets and all(ws.is_closed() for ws in sockets)
                    for theme, shade in [('light','rgba(15, 23, 42, 0.24)'), ('dark','rgba(0, 0, 0, 0.52)')]:
                        page.evaluate('theme=>applyTheme(theme)', theme)
                        assert dialog.evaluate("e=>getComputedStyle(e,'::backdrop').backgroundColor") == shade
                    card = dialog.bounding_box()
                    assert abs(card['x'] + card['width']/2 - 640) < 2
                    assert abs(card['y'] + card['height']/2 - 450) < 2
                    page.wait_for_timeout(500); requests.clear()
                    page.clock.fast_forward(65000)
                    page.mouse.move(20, 20)
                    page.keyboard.press('Escape')
                    page.mouse.click(20, 20)
                    page.evaluate("dispatchEvent(new Event('focus')); document.dispatchEvent(new Event('visibilitychange'))")
                    page.wait_for_timeout(600)
                    expect(dialog).to_be_visible()
                    assert not [p for p in requests if '/api/' in p], requests
                    # Private fixture host continues running and server-side additions wait for Resume.
                    rows = context.request.get(base + '/api/term/list').json()
                    assert any(r['name'] == receipt['name'] and r['running'] for r in rows['pending'] + rows['sessions'])
                    corpus.put('while-asleep', 'claude', [claude_row('while-asleep','user','u0',None,'Appeared while asleep')], [])
                    page.get_by_role('button', name='Resume', exact=True).click()
                    expect(dialog).not_to_be_visible()
                    page.wait_for_function('uiEventsReady && T.ws?.readyState===WebSocket.OPEN && S.sessions.some(s=>s.sid==="while-asleep")')
                    expect(page.locator('#cinput')).to_have_value('Keep this draft while sleeping')
                    assert any('/api/sessions' == p for p in requests)
                    print('PASS default 1h, keyboard resets deadline, shade in both themes, HTTP/SSE/WebSocket quiet, Resume catches up and keeps draft/host', flush=True)

                    # Setting persists and sleep is modal above an already open settings dialog.
                    settings(page)
                    page.locator('#setting-sleep').select_option('5')
                    page.reload(wait_until='domcontentloaded')
                    page.wait_for_function('typeof SessionDockSleep !== "undefined" && T.listLoaded')
                    settings(page)
                    expect(page.locator('#setting-sleep')).to_have_value('5')
                    page.clock.fast_forward(4 * 60000)
                    assert not page.evaluate('SessionDockSleep.sleeping')
                    page.clock.fast_forward(61000)
                    expect(dialog).to_be_visible()
                    assert page.locator('#settings-dialog').evaluate('e=>e.open')
                    assert page.evaluate("document.elementFromPoint(20,20).id === 'page-sleep-dialog'")
                    page.keyboard.press('Escape')
                    expect(dialog).to_be_visible()
                    page.get_by_role('button', name='Resume', exact=True).click()
                    expect(page.locator('#settings-dialog')).to_be_visible()
                    page.locator('#setting-sleep').select_option('0')
                    page.locator('#settings-dialog .modal-close').click()
                    page.clock.fast_forward(4 * 3600000)
                    assert not page.evaluate('SessionDockSleep.sleeping')
                    settings(page)
                    page.locator('#setting-sleep').select_option('5')
                    page.locator('#settings-dialog .modal-close').click()
                    # A claim already in flight must not attach after sleep begins.
                    claims = []
                    page.route('**/api/term/claim', lambda route: claims.append(route))
                    page.evaluate('reconnectTerm()')
                    page.wait_for_timeout(500)
                    assert claims, 'expected an in-flight terminal claim'
                    # Simulate OS sleep: wall clock advances without running timers.
                    page.clock.set_system_time(page.evaluate('Date.now() + 6 * 60000'))
                    page.evaluate("dispatchEvent(new Event('focus'))")
                    expect(dialog).to_be_visible()
                    for route in claims: route.continue_()
                    page.unroute('**/api/term/claim')
                    page.wait_for_timeout(500)
                    assert page.evaluate('[...T.views.values()].every(v=>!v.ws)')
                    page.evaluate("markStaleBuild('synthetic-new-build')")
                    page.get_by_role('button', name='Resume', exact=True).click()
                    assert page.evaluate('SessionDockNetwork.paused && SessionDockNetwork.reason === "stale"')
                    expect(page.locator('.version-stale')).to_be_visible()
                    assert not errors, errors
                    print('PASS Features preference persists, 5min/disabled, top-layer modal and explicit Resume, suspended-clock catch-up, stale-build pause retained', flush=True)
                finally:
                    context.request.post(base + '/api/term/kill', data={'record_id':receipt['record_id'], 'instance_id':receipt['instance_id']})
                    context.close()
        finally:
            browser.close()


if __name__ == '__main__':
    main()
