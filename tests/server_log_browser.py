#!/usr/bin/env python3
"""The server's structured log, on a node and on the Hub.

With `SESSIONDOCK_LOG_REQUESTS=all`, opening the page and a conversation logs
one JSON line per `/api` request (method, path, status, ms), an ApiError
answer carries its `code` at level error, the page's trace id is kept, and
neither query strings nor request bodies reach the log (secret markers sent in
both are absent). Every stderr line is a JSON object with ts/level/event,
starting with `server.listening`. In the default `errors` mode ordinary 200
requests are not logged. The Hub logs `hub.listening` and its requests the
same way.
"""
import json
import tempfile
from pathlib import Path
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from history_fixtures import BINARY, Corpus, batch35_meta, codex_message, isolated_server
from hub_fixtures import Hub, free_port

SECRET = 'SECRET-MARKER-7f3a'


def lines(text):
    rows = [json.loads(line) for line in text.splitlines() if line.strip()]
    for row in rows:
        assert {'ts', 'level', 'event'} <= row.keys(), row
    assert SECRET not in text, 'a query string or body reached the log'
    return rows


def drive(browser, base, uid):
    page = browser.new_page()
    errors = []
    page.on('pageerror', lambda error: errors.append(str(error)))
    page.goto(base + '/?' + 'secret=' + SECRET)
    page.locator(f'#side .item[data-uid="{uid}"] .t').click()
    expect(page.locator('#msgs')).to_contain_text('Logged conversation')
    status = page.evaluate("""async secret => {
        const failed = await fetch('api/term/final?id=bad&token=' + secret,
            {headers: {'X-SessionDock-Trace': 'trace-log-fixture'}});
        await fetch('api/session/star', {method: 'POST', headers: {'Content-Type': 'application/json'},
            body: JSON.stringify({uid: 'codex:missing', starred: true, note: secret})});
        return failed.status;
    }""", SECRET)
    assert not errors, errors
    page.close()
    return status


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-server-log-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        corpus = Corpus(root)
        corpus.put('logged', 'codex', [batch35_meta('logged-session'),
                   codex_message('user', 'Logged conversation')], ['Logged conversation'])
        uid = corpus.uid('logged')
        browser = pw.chromium.launch(headless=True)
        try:
            node = SimpleNamespace(nid='b' * 32, name='LogNode', port=free_port(), token='e' * 64)
            token, identity = root / 'node-token', root / 'node-id'
            for path, value in [(token, node.token), (identity, node.nid)]:
                path.touch(mode=0o600)
                path.write_text(value)
            env = {'SESSIONDOCK_LOG_REQUESTS': 'all', 'SESSIONDOCK_NODE_BIND': f'127.0.0.1:{node.port}',
                   'SESSIONDOCK_NODE_TOKEN_FILE': str(token), 'SESSIONDOCK_NODE_ID_FILE': str(identity),
                   'SESSIONDOCK_NODE_PEERS': '127.0.0.0/8'}
            log = root / 'node.log'
            with isolated_server(corpus, BINARY, extra_env=env, log_path=log) as (base, _):
                status = drive(browser, base, uid)
                (root / 'hub').mkdir()
                hub = Hub(BINARY.with_name('sessiondock-hub'), root / 'hub', [node])
                hub.env['SESSIONDOCK_LOG_REQUESTS'] = 'all'
                hub.start()
                try:
                    hub_page = browser.new_page()
                    hub_page.goto(f'http://127.0.0.1:{hub.port}/?secret={SECRET}')
                    scoped = uid.replace(':', ':' + node.nid + '~', 1)
                    hub_page.locator(f'#side .item[data-uid="{scoped}"] .t').click()
                    expect(hub_page.locator('#msgs')).to_contain_text('Logged conversation')
                    hub_page.close()
                finally:
                    hub.stop()
                hub_text = hub.process.stderr.read()
                rows = lines(log.read_text())
            assert status == 501, status
            assert rows[0]['event'] == 'server.listening' and rows[0]['requests'] == 'all', rows[0]
            requests = [row for row in rows if row['event'] == 'http.request']
            assert all(set(row) >= {'method', 'path', 'status', 'ms'} and '?' not in row['path']
                       for row in requests), requests[:3]
            assert any(row['path'] == '/api/sessions' and row['status'] == 200 and row['level'] == 'info'
                       for row in requests), [row['path'] for row in requests]
            failed = [row for row in requests if row['path'] == '/api/term/final']
            assert failed and failed[0]['level'] == 'error' and failed[0]['code'] == 'terminal_disabled' \
                and failed[0]['trace'] == 'trace-log-fixture', failed
            assert any(row['path'] == '/api/session/star' for row in requests)
            print('PASS node: one JSON line per request, error code and trace kept, no query or body', flush=True)

            hub_rows = lines(hub_text)
            assert hub_rows[0]['event'] == 'hub.listening' and hub_rows[0]['nodes'] == 1, hub_rows[0]
            hub_requests = [row for row in hub_rows if row['event'] == 'http.request']
            assert any(row['path'] == '/api/sessions' and row['status'] == 200 for row in hub_requests), hub_rows[:5]
            print('PASS hub: structured startup line and request lines, no query string', flush=True)

            quiet = root / 'quiet.log'
            with isolated_server(corpus, BINARY, log_path=quiet) as (base, _):
                drive(browser, base, uid)
            quiet_rows = lines(quiet.read_text())
            assert quiet_rows[0]['requests'] == 'errors', quiet_rows[0]
            logged = [row for row in quiet_rows if row['event'] == 'http.request']
            assert any(row['path'] == '/api/term/final' for row in logged), logged
            assert all(row['status'] >= 500 or row['ms'] >= 2000 for row in logged), logged
            print('PASS default mode logs only server errors and slow requests', flush=True)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
