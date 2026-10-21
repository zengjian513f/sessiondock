#!/usr/bin/env python3
"""Idle pages use UI events; inactive conversation bodies load only on selection."""

import argparse
from contextlib import ExitStack
from datetime import datetime
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import unquote, urlsplit

from playwright.sync_api import sync_playwright
from browser_runtime import choose_toolbar_option
from history_fixtures import (BINARY, Corpus, batch35_agent_meta, claude_row, codex_row,
                              codex_message, encoded, isolated_server)
from hub_fixtures import Hub, free_port
from node_auth_fixtures import node_env, TOKEN


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path('/tmp/sessiondock-ui-events'))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-ui-events-') as tmp, sync_playwright() as pw:
        with ExitStack() as browsers:
            browser = pw.chromium.launch(headless=True)
            browsers.callback(browser.close)
            for source, hub_mode in (('codex', False), ('codex', True), ('claude', False), ('claude', True)):
                label = f'{source}-{"hub" if hub_mode else "node"}'
                root = Path(tmp) / label
                data = Corpus(root / 'node')
                for provider in ('claude', 'codex', 'grok'):
                    (data.root / provider).mkdir(parents=True)
                def timestamp(day):
                    return f'2026-09-{day:02d}T10:00:00.000Z'
                def set_mtime(sid, stamp):
                    seconds = datetime.fromisoformat(stamp.replace('Z', '+00:00')).timestamp()
                    os.utime(data.paths[sid], (seconds, seconds))
                for i in range(2):
                    sid = f'events-{i}'
                    rows = [
                        claude_row(sid, 'user', 'u0', None, f'Events session {i}'),
                        claude_row(sid, 'assistant', 'a0', 'u0', 'Initial reply'),
                    ] if source == 'claude' else [
                        codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/history',
                                                  'timestamp': timestamp(11 + i)}),
                        codex_row('event_msg', {'type': 'task_started'}),
                        codex_message('user', f'Events session {i}'),
                        codex_message('assistant', 'Initial reply'),
                    ]
                    data.put(sid, source, [{**row, 'timestamp': timestamp(11 + i)} for row in rows], [])
                    set_mtime(sid, timestamp(11 + i))
                if source == 'codex':
                    data.put('events-child', 'codex', [
                        batch35_agent_meta('events-child', 'events-0', timestamp(10)),
                        {**codex_row('event_msg', {'type': 'task_started'}), 'timestamp': timestamp(10)},
                        {**codex_message('assistant', 'Child initial reply'), 'timestamp': timestamp(10)},
                    ], [], parent='events-0')
                nid = 'a' * 32
                ids = data.root / 'ids'; ids.mkdir(); (ids / 'node-id').write_text(nid + '\n')
                node = SimpleNamespace(name='events', nid=nid, port=free_port(), token=TOKEN)
                with ExitStack() as stack:
                    base, _ = stack.enter_context(isolated_server(data, args.binary,
                        extra_env=node_env(data.root, node.port, '127.0.0.0/8')))
                    if hub_mode:
                        hubroot = root / 'hub'; hubroot.mkdir()
                        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [node])
                        hub.start(); stack.callback(hub.stop)
                        base = f'http://127.0.0.1:{hub.port}'
                    context = browser.new_context(viewport={'width': 390 if hub_mode else 1280, 'height': 900},
                                                  color_scheme='dark' if hub_mode else 'light')
                    stack.callback(context.close)
                    page = context.new_page(); errors = []; requests = []
                    page.on('pageerror', lambda error: errors.append(str(error)))
                    page.on('request', lambda request: requests.append((request.method, unquote(urlsplit(request.url).path))))
                    # Record wire notifications without changing the page's subscription.
                    page.add_init_script('''window.uiEventPackets=[];
                        const NativeEventSource=window.EventSource;
                        window.EventSource=class extends NativeEventSource {
                          constructor(url,options){super(url,options);
                            if(String(url).includes('api/events')) this.addEventListener('change',
                              e=>window.uiEventPackets.push(JSON.parse(e.data)));}
                        };''')
                    page.goto(base, wait_until='domcontentloaded')
                    page.wait_for_function('uiEventsReady && S.sessions.length===2 && T.listLoaded && !uiEventApplying', timeout=30000)
                    def uid_for(sid):
                        uid = data.uid(sid)
                        return uid.replace(':', f':{nid}~', 1) if hub_mode else uid
                    inactive = uid_for('events-0'); selected = uid_for('events-1')
                    def show_list():
                        back = page.locator('#detail .mobile-back')
                        if back.is_visible():
                            back.click()
                    def order():
                        return page.locator('#side .item[data-uid]').evaluate_all(
                            'rows=>rows.map(row=>row.dataset.uid)')
                    assert order() == [selected, inactive], order()
                    # Real user navigation populates one cached but inactive history.
                    page.locator(f'#side .item[data-uid="{inactive}"] .t').click()
                    page.wait_for_function('uid=>S.sel===uid && cache.has(uid)', arg=inactive)
                    show_list()
                    page.locator(f'#side .item[data-uid="{selected}"] .t').click()
                    page.wait_for_function('uid=>S.sel===uid && cache.has(uid)', arg=selected)
                    show_list()
                    page.wait_for_timeout(3000)
                    requests.clear(); page.evaluate('window.uiEventPackets=[]')
                    old_end = page.evaluate('uid=>cache.get(uid).end', inactive)
                    page.wait_for_timeout(10000)
                    periodic_paths = {'/api/live', '/api/term/list', '/api/sessions'}
                    periodic = [(m, p) for m, p in requests if m == 'GET' and p in periodic_paths]
                    assert not periodic, (periodic, page.evaluate('window.uiEventPackets'))
                    idle_count = len(requests)
                    idle_paths = list(requests)
                    requests.clear(); page.evaluate('window.uiEventPackets=[]')
                    page.screenshot(path=str(args.screenshots / f'{label}-before.png'), full_page=True)
                    for n in (1, 2):
                        stamp = timestamp(12 + n)
                        reply = (claude_row('events-0', 'assistant', f'a{n}', f'a{n-1}', f'Event-driven reply {n}')
                                 if source == 'claude' else codex_message('assistant', f'Event-driven reply {n}'))
                        with data.paths['events-0'].open('ab') as stream:
                            stream.write(encoded({**reply, 'timestamp': stamp}))
                        set_mtime('events-0', stamp)
                        page.wait_for_function('arg=>S.unread.get(arg.uid)?.count===arg.n',
                            arg={'uid': inactive, 'n': n}, timeout=30000)
                        assert page.evaluate('uid=>cache.get(uid).end', inactive) == old_end
                        row = page.evaluate('uid=>S.sessions.find(s=>s.uid===uid)', inactive)
                        page.screenshot(path=str(args.screenshots / f'{label}-append-{n}.png'), full_page=True)
                        assert row['updated'] == stamp, (row['updated'], stamp, page.evaluate('window.uiEventPackets'))
                        assert row['size'] == data.paths['events-0'].stat().st_size, row
                        if source == 'codex':
                            assert row['turn'] == 'working', row
                        assert order() == [inactive, selected], order()
                    assert not any('/api/messages/' + inactive in path for _, path in requests), requests
                    assert not any(method == 'GET' and path == '/api/sessions' for method, path in requests), (
                        requests, page.evaluate('window.uiEventPackets'))
                    append_summaries = sum('/api/sessions/unread' in path for _, path in requests)
                    assert append_summaries == 2, requests
                    page.locator(f'#side .item[data-uid="{inactive}"] .t').click()
                    page.wait_for_function("document.querySelector('#msgs')?.innerText.includes('Event-driven reply 2')", timeout=15000)
                    assert any('/api/messages/' + inactive in path for _, path in requests), requests
                    assert page.evaluate('uid=>!S.unread.has(uid)', inactive)
                    show_list()
                    assert order() == [inactive, selected], order()
                    page.screenshot(path=str(args.screenshots / f'{label}-after-click.png'), full_page=True)
                    if source == 'codex':
                        # An agent's activity also updates its owner's nested
                        # position before either history is opened.
                        choose_toolbar_option(page, 'nest', 'nested')
                        page.locator(f'#side .item[data-uid="{selected}"] .t').click()
                        show_list()
                        with data.paths['events-1'].open('ab') as stream:
                            stream.write(encoded({**codex_message('assistant', 'Neighbour advances'),
                                                  'timestamp': timestamp(15)}))
                        page.wait_for_function('uid=>S.sessions.find(s=>s.uid===uid).updated=== "2026-09-15T10:00:00.000Z"',
                                               arg=selected)
                        assert order() == [selected, inactive], order()
                        requests.clear()
                        with data.paths['events-child'].open('ab') as stream:
                            stream.write(encoded({**codex_message('assistant', 'Child advances'),
                                                  'timestamp': timestamp(16)}))
                        page.wait_for_function('uid=>S.sessions.find(s=>s.uid===uid).agent_items[0].updated=== "2026-09-16T10:00:00.000Z"',
                                               arg=inactive)
                        assert order() == [inactive, selected], order()
                        assert not any('/api/messages/' in path or (method == 'GET' and path == '/api/sessions')
                                       for method, path in requests), requests
                        page.screenshot(path=str(args.screenshots / f'{label}-child.png'), full_page=True)
                        # Search keeps its match set, but its existing rows must
                        # receive the same activity metadata as the main list.
                        search = page.locator('#q')
                        search.fill('Events session')
                        search.press('Enter')
                        page.wait_for_function('S.results?.length===2')
                        requests.clear()
                        with data.paths['events-1'].open('ab') as stream:
                            stream.write(encoded({**codex_message('assistant', 'Search result advances'),
                                                  'timestamp': timestamp(17)}))
                        page.wait_for_function('uid=>S.results?.find(s=>s.uid===uid)?.updated=== "2026-09-17T10:00:00.000Z"',
                                               arg=selected)
                        assert order()[0] == selected, order()
                        assert not any(method == 'GET' and path == '/api/sessions' for method, path in requests), requests
                        search.fill('')
                        search.press('Enter')
                        page.wait_for_function('S.results===null && !S.term')
                    # A genuine list change must invalidate membership, unlike appends.
                    requests.clear()
                    failed_lists = []
                    def fail_one_list(route):
                        if urlsplit(route.request.url).path == '/api/sessions' and not failed_lists:
                            failed_lists.append(route.request.url)
                            route.fulfill(status=503, json={'error':'temporary list failure'})
                        else:
                            route.continue_()
                    page.route('**/api/sessions*', fail_one_list)
                    data.put('events-new', 'claude', [claude_row('events-new', 'user', 'new-u', None, 'New event-discovered session')], [])
                    page.wait_for_function('uid=>S.sessions.some(s=>s.uid===uid)', arg=uid_for('events-new'), timeout=30000)
                    assert len(failed_lists) == 1, failed_lists
                    assert sum(method == 'GET' and path == '/api/sessions' for method, path in requests) >= 2, requests
                    page.unroute('**/api/sessions*', fail_one_list)
                    page.locator(f'#side .item[data-uid="{uid_for("events-new")}"] .t').click()
                    page.wait_for_function("document.querySelector('#msgs')?.innerText.includes('New event-discovered session')")
                    show_list()
                    # A finite SSE response closes the transport. Chromium's
                    # offline emulation leaves existing SSE sockets alive, so use
                    # an actual EOF and let EventSource.onerror trigger recovery.
                    disconnected = []
                    def eof_once(route):
                        if not disconnected:
                            disconnected.append(True)
                            route.fulfill(status=200, content_type='text/event-stream', body=': test disconnect\n\n')
                        else:
                            route.continue_()
                    page.route('**/api/events', eof_once)
                    requests.clear()
                    page.reload(wait_until='domcontentloaded')
                    page.wait_for_function('S.sessions.length===3 && !uiEventsReady && uiEventsRetry!==0')
                    assert disconnected
                    data.put('events-reconnect', 'claude', [claude_row('events-reconnect', 'user', 'reconnect-u', None, 'Created while disconnected')], [])
                    page.wait_for_function('uiEventsReady && !uiEventApplying', timeout=30000)
                    page.wait_for_function('uid=>S.sessions.some(s=>s.uid===uid)', arg=uid_for('events-reconnect'), timeout=30000)
                    assert sum(path == '/api/events' for _, path in requests) >= 2, requests
                    page.unroute('**/api/events', eof_once)
                    # Keep this page's subscription alive while another page
                    # connects and reloads. The shared observer's old baseline
                    # cannot stand in for an observation after each subscriber.
                    # No session changes are needed to trigger that baseline.
                    newcomer = context.new_page()
                    newcomer.add_init_script('''window.uiEventPackets=[];
                        const NativeEventSource=window.EventSource;
                        window.EventSource=class extends NativeEventSource {
                          constructor(url,options){super(url,options);
                            if(String(url).includes('api/events')) this.addEventListener('change',
                              e=>window.uiEventPackets.push(JSON.parse(e.data)));}
                        };''')
                    for reconnect in (False, True):
                        if reconnect:
                            newcomer.reload(wait_until='domcontentloaded')
                        else:
                            newcomer.goto(base, wait_until='domcontentloaded')
                        newcomer.wait_for_function(
                            'uiEventPackets.filter(packet=>packet.initial).length>=2 && uiEventsReady && !uiEventApplying',
                            timeout=15000)
                        assert page.evaluate('uiEventsReady && uiEvents?.readyState===EventSource.OPEN')
                        if newcomer.locator('#detail .mobile-back').is_visible():
                            newcomer.locator('#detail .mobile-back').click()
                        newcomer.locator(f'#side .item[data-uid="{uid_for("events-reconnect")}"] .t').click()
                        newcomer.wait_for_function("document.querySelector('#msgs')?.innerText.includes('Created while disconnected')")
                    newcomer.close()
                    assert not errors, errors
                    print(f'PASS UI events {label}: 10s idle={idle_count} requests, '
                          f'list/live/term polling=0 ({idle_paths}); two inactive appends={append_summaries} summaries, '
                          'body/list GET=0; activity/order updated before click; click keeps order and loads latest; '
                          'new session and reconnect discovered', flush=True)


if __name__ == '__main__':
    main()
