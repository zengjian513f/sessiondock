#!/usr/bin/env python3
"""Chromium: large list deltas, node/hub parity, cache recovery and paused pages."""

import argparse
from contextlib import ExitStack
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import tempfile
import threading
import time
from types import SimpleNamespace
from urllib.parse import urlsplit

from playwright.sync_api import sync_playwright
from history_fixtures import BINARY, Corpus, claude_row, encoded, isolated_server
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env


class Tap(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        self.forward()

    def do_POST(self):
        self.forward()

    def forward(self):
        conn = http.client.HTTPConnection('127.0.0.1', self.server.node_port, timeout=15)
        body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
        headers = dict(self.headers)
        headers['Host'] = f'127.0.0.1:{self.server.node_port}'
        terminal = urlsplit(self.path).path == '/api/term/list'
        if terminal:
            headers = {k: v for k, v in headers.items() if k.lower() != 'x-sessiondock-list'}
        conn.request(self.command, self.path, body, headers)
        response = conn.getresponse(); raw = response.read()
        if terminal and response.status == 200:
            data = json.loads(raw); data['sessions'] = self.server.terminal_rows
            raw = json.dumps(data).encode()
        if urlsplit(self.path).path in ('/api/sessions', '/api/term/list'):
            self.server.reads.append((urlsplit(self.path).path, len(raw), json.loads(raw)))
        self.send_response(response.status)
        self.send_header('Content-Type', response.getheader('Content-Type', 'application/json'))
        self.send_header('Content-Length', str(len(raw)))
        try:
            self.end_headers(); self.wfile.write(raw)
        except (BrokenPipeError, ConnectionResetError):
            pass  # Browser pause/reload and hub restart intentionally abort reads.
        finally:
            conn.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    count = 1200
    with tempfile.TemporaryDirectory(prefix='sessiondock-list-delta-') as tmp, sync_playwright() as pw, ExitStack() as stack:
        browser = pw.chromium.launch(headless=True); stack.callback(browser.close)
        for hub_mode in (False, True):
            root = Path(tmp) / ('hub' if hub_mode else 'node')
            corpus = Corpus(root / 'data')
            for source in ('claude', 'codex', 'grok'):
                (corpus.root / source).mkdir(parents=True)
            for i in range(count):
                sid = f'bulk-{i:04d}'
                corpus.put(sid, 'claude', [claude_row(sid, 'user', 'u0', None, f'Bulk session {i:04d}'),
                    claude_row(sid, 'assistant', 'a0', 'u0', 'Synthetic reply')], [])
            agents = corpus.paths['bulk-0000'].with_suffix('') / 'subagents'; agents.mkdir(parents=True)
            for i in range(200):
                agent = f'worker-{i:03d}'
                path = agents / f'agent-{agent}.jsonl'
                path.write_bytes(encoded(claude_row('bulk-0000', 'user', 'u0', None,
                    f'Synthetic worker {i}', isSidechain=True, agentId=agent)))
                path.with_suffix('.meta.json').write_text(json.dumps({'description':f'Synthetic worker {i}', 'agentType':'reviewer'}))
            ids = corpus.root / 'ids'; ids.mkdir(); nid = 'a' * 32
            (corpus.root / 'state').mkdir(mode=0o700)
            (ids / 'node-id').write_text(nid + '\n')
            node_port = free_port()
            with ExitStack() as mode:
                base, _ = mode.enter_context(isolated_server(corpus, args.binary, state_dir=corpus.root / 'state',
                    extra_env=node_env(corpus.root, node_port, '127.0.0.0/8')))
                tap = None
                if hub_mode:
                    tap = ThreadingHTTPServer(('127.0.0.1', 0), Tap)
                    tap.node_port = node_port; tap.reads = []
                    tap.terminal_rows = [{'name':f'fixture-{i}', 'uid':corpus.uid('bulk-0000'),
                        'sid':'bulk-0000', 'source':'claude', 'cwd':f'/synthetic/terminal-{i}', 'running':False}
                        for i in range(120)]
                    worker = threading.Thread(target=tap.serve_forever, daemon=True); worker.start()
                    mode.callback(tap.server_close); mode.callback(tap.shutdown)
                    hubroot = root / 'hub'; hubroot.mkdir()
                    node = SimpleNamespace(nid=nid, name='synthetic', port=tap.server_port, token=TOKEN)
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [node])
                    hub.start(); mode.callback(hub.stop)
                    base = f'http://127.0.0.1:{hub.port}'
                context = browser.new_context(); mode.callback(context.close)
                page = context.new_page(); records = []; errors = []; network = []
                page.on('pageerror', lambda error: errors.append(str(error)))
                page.on('request', lambda request: network.append((request.method, urlsplit(request.url).path)))
                def finished(request):
                    path = urlsplit(request.url).path
                    if path in ('/api/sessions', '/api/term/list'):
                        response = request.response()
                        if response and response.ok:
                            raw = response.body()
                            records.append((path, len(raw), json.loads(raw)))
                page.on('requestfinished', finished)
                page.goto(base, wait_until='domcontentloaded')
                page.wait_for_function('n=>S.sessions.length===n && uiEventsReady && T.listLoaded && !uiEventApplying', arg=count, timeout=45000)
                initial = max(size for path, size, _ in records if path == '/api/sessions')
                uid = corpus.uid('bulk-0000')
                if hub_mode: uid = uid.replace(':', f':{nid}~', 1)
                # A real user action changes one row among a batch-sized list.
                # 1200 rows are windowed: scroll the oldest row into the sidebar first.
                page.evaluate('uid=>sidebarRowNode(uid,null,true)?.scrollIntoView({block:"center"})', uid)
                page.locator(f'#side .star-toggle[data-star-uid="{uid}"]').click()
                page.wait_for_function('uid=>S.sessions.find(s=>s.uid===uid)?.starred', arg=uid)
                # The local optimistic star precedes the Hub's next catalog
                # snapshot. Wait for that actual wire delta before measuring it.
                deadline = time.monotonic() + 15
                changes = []
                while not changes and time.monotonic() < deadline:
                    page.evaluate('pollSessions()')
                    page.wait_for_timeout(250)
                    changes = [size for path, size, data in records if path == '/api/sessions'
                        and data.get('list_delta', {}).get('collections', {}).get('sessions', {}).get('upsert')]
                assert changes and max(changes) < initial / 20, (initial, changes)
                assert page.evaluate('uid=>S.sessions.find(s=>s.uid===uid).agent_items.length', uid) == 200
                (agents / 'agent-worker-000.meta.json').write_text(json.dumps({'description':'Changed worker title', 'agentType':'reviewer'}))
                page.wait_for_function('uid=>S.sessions.find(s=>s.uid===uid)?.agent_items.some(a=>a.title==="Changed worker title")', arg=uid, timeout=30000)
                child_changes = [(size, item['agents']) for path, size, data in records if path == '/api/sessions'
                    for item in data.get('list_delta', {}).get('collections', {}).get('sessions', {}).get('upsert', []) if 'agents' in item]
                assert child_changes and child_changes[-1][0] < initial / 50, child_changes[-1:]
                assert len(child_changes[-1][1]['upsert']) == 1
                print(f'PASS 200 subagents: one changed child={child_changes[-1][0]} B', flush=True)
                # Cursor growth moves a row without re-sending the other 1199 rows.
                with corpus.paths['bulk-0001'].open('ab') as stream:
                    stream.write(encoded(claude_row('bulk-0001', 'assistant', 'a1', 'a0', 'Appended reply',
                        timestamp='2026-09-12T12:00:00Z')))
                corpus.paths['bulk-0002'].unlink()
                corpus.put('bulk-new', 'claude', [claude_row('bulk-new', 'user', 'u0', None, 'Newly discovered bulk row')], [])
                page.wait_for_function('S.sessions.some(s=>s.title.includes("Newly discovered bulk row"))', timeout=30000)
                removed = corpus.uid('bulk-0002')
                if hub_mode: removed = removed.replace(':', f':{nid}~', 1)
                assert page.evaluate('uid=>!S.sessions.some(s=>s.uid===uid)', removed)
                assert page.evaluate('new Set(S.sessions.map(s=>s.uid)).size') == count
                actual = context.request.get(base + '/api/sessions').json()['sessions']
                page.evaluate('pollSessions()')
                assert page.evaluate('S.sessions.map(s=>s.uid)') == [row['uid'] for row in actual]
                # No-change terminal reads have no sessions/pending payload.
                page.evaluate('loadTermList()'); page.wait_for_timeout(100)
                terminal = [data for path, _, data in records if path == '/api/term/list' and 'list_delta' in data]
                assert terminal and terminal[-1]['list_unchanged'], terminal
                assert all(not patch['upsert'] for patch in terminal[-1]['list_delta']['collections'].values())
                if hub_mode:
                    term_full = max(size for path, size, data in records if path == '/api/term/list' and 'list_delta' not in data)
                    term_idle = [size for path, size, data in records if path == '/api/term/list' and data.get('list_unchanged')][-1]
                    assert term_idle < term_full / 10, (term_full, term_idle)
                    tap.terminal_rows[0]['cwd'] = '/synthetic/changed-terminal'
                    page.evaluate('loadTermList()')
                    page.wait_for_function('T.list.some(row=>row.cwd==="/synthetic/changed-terminal")')
                    term_changes = [data for path, _, data in records if path == '/api/term/list'
                        and data.get('list_delta', {}).get('collections', {}).get('sessions', {}).get('upsert')]
                    assert len(term_changes[-1]['list_delta']['collections']['sessions']['upsert']) == 1
                    print(f'PASS terminal list: {term_full} B full, {term_idle} B unchanged; changed host reaches UI', flush=True)
                unknown = context.request.get(base + '/api/sessions', headers={'X-SessionDock-List': 'evicted'}).json()
                assert len(unknown['sessions']) == count and 'list_delta' not in unknown
                scoped = context.request.get(base + '/api/sessions?debug_run=unknown',
                    headers={'X-SessionDock-List': unknown['list_version']}).json()
                assert len(scoped['sessions']) == count and 'list_delta' not in scoped
                assert scoped['sig'] == unknown['sig']
                if hub_mode:
                    upstream = [(size, data) for path, size, data in tap.reads if path == '/api/sessions' and 'list_delta' in data]
                    assert upstream and min(size for size, _ in upstream) < initial / 100, upstream[-1:]
                    hub.stop(); hub.start()
                    page.evaluate('pollSessions()')
                    assert page.evaluate('S.sessions.length') == count
                print(f'PASS {"hub" if hub_mode else "node"}: {count} rows, full={initial} B, one-row delta={max(changes)} B; '
                    'delete/reorder, unchanged terminal, missing revision and ignored obsolete query', flush=True)
                # New build freezes background HTTP/SSE, while draft saves remain possible.
                page.route('**/api/meta', lambda route: route.fulfill(json={'build':'new-fixture-build'}))
                page.evaluate('checkServerBuild()')
                page.locator('.version-stale').wait_for()
                page.wait_for_timeout(300); network.clear()
                page.evaluate('Promise.all([pollSessions(), pollLive(), loadTermList()])')
                page.wait_for_timeout(8500)
                assert not [r for r in network if r[1].startswith('/api/')], network
                saves = []
                def save(route):
                    saves.append(route.request.post_data_json); route.fulfill(json={'ok':True})
                page.route('**/api/session/conversation', save)
                assert page.evaluate("fetch(appUrl('api/session/conversation'),{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({value:{text:'unsaved draft'}})}).then(r=>r.ok)")
                assert saves[0]['value']['text'] == 'unsaved draft'
                page.unroute('**/api/meta')
                with page.expect_navigation(wait_until='domcontentloaded'):
                    page.locator('.version-stale [data-act="reload"]').click()
                page.wait_for_function('!SessionDockNetwork.paused && S.sessions.length>0 && uiEventsReady', timeout=30000)
                # Follow the same redirect an expired proxy login returns.
                page.route('**/__auth/login', lambda route: route.fulfill(content_type='text/html', body='<p>Login</p>'))
                page.route('**/api/meta', lambda route: route.fulfill(status=302, headers={'Location':'/__auth/login'}))
                page.evaluate('checkServerBuild()')
                page.locator('.login-expired').wait_for()
                page.wait_for_timeout(300); network.clear()
                page.evaluate('Promise.all([pollSessions(), pollLive(), loadTermList()])')
                page.wait_for_timeout(8500)
                assert not [r for r in network if r[1].startswith('/api/')], network
                assert not errors, errors
                print('PASS stale build and expired login: background requests stop; stale-page draft save and reload work', flush=True)


if __name__ == '__main__':
    main()
