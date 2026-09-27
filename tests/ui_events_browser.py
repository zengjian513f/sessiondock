#!/usr/bin/env python3
"""Idle pages use UI events; inactive conversation bodies load only on selection."""
import argparse
from contextlib import ExitStack
from pathlib import Path
import tempfile
from types import SimpleNamespace
from urllib.parse import unquote, urlsplit

from playwright.sync_api import sync_playwright
from history_parity import BINARY, Corpus, claude_row, encoded, isolated_server
from hub_http_suite import Hub, free_port
from node_auth_suite import node_env, TOKEN


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-ui-events-') as tmp, sync_playwright() as pw:
        with ExitStack() as browsers:
            browser = pw.chromium.launch(headless=True)
            browsers.callback(browser.close)
            for hub_mode in (False, True):
                root = Path(tmp) / ('hub-page' if hub_mode else 'node-page')
                data = Corpus(root / 'node')
                for source in ('claude', 'codex', 'grok'):
                    (data.root / source).mkdir(parents=True)
                for i in range(2):
                    sid = f'events-{i}'
                    data.put(sid, 'claude', [
                        claude_row(sid, 'user', 'u0', None, f'Events session {i}'),
                        claude_row(sid, 'assistant', 'a0', 'u0', 'Initial reply'),
                    ], [])
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
                    context = browser.new_context(); stack.callback(context.close)
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
                    # Real user navigation populates one cached but inactive history.
                    page.locator(f'#side .item[data-uid="{inactive}"] .t').click()
                    page.wait_for_function('uid=>S.sel===uid && cache.has(uid)', arg=inactive)
                    page.locator(f'#side .item[data-uid="{selected}"] .t').click()
                    page.wait_for_function('uid=>S.sel===uid && cache.has(uid)', arg=selected)
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
                    for n in (1, 2):
                        with data.paths['events-0'].open('ab') as stream:
                            stream.write(encoded(claude_row('events-0', 'assistant', f'a{n}', f'a{n-1}', f'Event-driven reply {n}')))
                        page.wait_for_function('arg=>S.unread.get(arg.uid)?.count===arg.n',
                            arg={'uid': inactive, 'n': n}, timeout=30000)
                        assert page.evaluate('uid=>cache.get(uid).end', inactive) == old_end
                    assert not any('/api/messages/' + inactive in path for _, path in requests), requests
                    assert not any(method == 'GET' and path == '/api/sessions' for method, path in requests), (
                        requests, page.evaluate('window.uiEventPackets'))
                    append_summaries = sum('/api/sessions/unread' in path for _, path in requests)
                    assert append_summaries == 2, requests
                    page.locator(f'#side .item[data-uid="{inactive}"] .t').click()
                    page.wait_for_function("document.querySelector('#msgs')?.innerText.includes('Event-driven reply 2')", timeout=15000)
                    assert any('/api/messages/' + inactive in path for _, path in requests), requests
                    assert page.evaluate('uid=>!S.unread.has(uid)', inactive)
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
                    assert not errors, errors
                    print(f'PASS UI events {"hub" if hub_mode else "node"}: 10s idle={idle_count} requests, '
                          f'list/live/term polling=0 ({idle_paths}); two inactive appends={append_summaries} summaries, '
                          'body/list GET=0; click loads latest; new session and reconnect discovered', flush=True)


if __name__ == '__main__':
    main()
