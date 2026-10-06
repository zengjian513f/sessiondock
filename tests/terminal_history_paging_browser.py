#!/usr/bin/env python3
"""Main console pages older host scrollback beyond the 2000-row snapshot.

A synthetic shell prints 5000 numbered rows before the page opens its console,
so the attach snapshot carries only the newest 2000. Real scrollbar drags,
wheel input and scrollbar keys reach the oldest loaded row; the console fetches
`GET /api/term/grid/history` under its own lease, prepends each page without
moving the visible rows, and ends with rows 1..5000 in order with no gap or
duplicate at any seam. Live input still works afterwards, idle pages issue no
history requests, a page reload pages again from a fresh snapshot, and a
recording replay never requests history. A stale lease token answered with the
server's real 409 stops paging quietly for that connection (no retry, no error,
audit event `terminal.history_page_failed`), and paging resumes once the
network returns and the console reconnects with a fresh snapshot. A host started with a small
`--history` keeps streaming every scrolled row to the attached page after its
history is full, without a reconnect.
"""

import json
import os
from pathlib import Path
import re
import tempfile
import time
import uuid

from playwright.sync_api import sync_playwright

from history_parity import BINARY, REPO, Corpus, codex_message, codex_row, isolated_server
from private_hosts import private_hosts
from draft_sync_browser import SHELL as REPLAY_SHELL, initialize
import terminal_input_browser as fixture

ROWS = 5000
SHELL = f"""stty -echo
seq -f 'PAGE_ROW_%05g' 1 {ROWS}
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
  case "$command" in
    ping) printf 'PAGE_PING_OK\\n' ;;
    more) seq -f 'PAGE_LIVE_%03g' 1 30 ;;
  esac
done
"""

TERM = '[...T.views.values()][0].term'
NUMBERS = """() => { const t = %s, b = t.buffer.active, out = [];
  for (let i = 0; i < b.length; i++) {
    const m = /^PAGE_ROW_(\\d{5})$/.exec(b.getLine(i)?.translateToString(true) || '');
    if (m) out.push(Number(m[1]));
  }
  return out; }""" % TERM
TOP_TEXT = f"() => {{ const b = {TERM}.buffer.active; return b.getLine(b.viewportY)?.translateToString(true) || ''; }}"


def launch_browser(pw):
    options = {'headless': True}
    if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
        options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
    return pw.chromium.launch(**options)


def track(context, base, started=None):
    context.route('**/*', lambda r: r.continue_() if r.request.url.startswith(base + '/') else r.abort())
    requests = []
    if started is not None:
        context.on('request', lambda r: started.append(r.url) if '/api/term/grid/history' in r.url else None)

    def on_response(response):
        if '/api/term/grid/history' in response.url:
            requests.append({'url': response.url, 'status': response.status})
    context.on('response', on_response)
    return requests


def wait_requests(page, requests, count, timeout=10):
    deadline = time.monotonic() + timeout
    while len(requests) < count:
        assert time.monotonic() < deadline, ('history request missing', requests)
        page.wait_for_timeout(50)


def settle(page):
    # A page in flight finishes and the chained check runs before asserting.
    page.wait_for_function(f'!currentTermViewObject()?.historyLoading', timeout=10000)
    page.wait_for_timeout(150)
    page.wait_for_function(f'!currentTermViewObject()?.historyLoading', timeout=10000)


def breaks(numbers):
    return [(numbers[i], n) for i, n in enumerate(numbers[1:]) if n != numbers[i] + 1]


def assert_contiguous(page, first_expected=None):
    numbers = page.evaluate(NUMBERS)
    assert numbers, 'no numbered rows'
    assert numbers == list(range(numbers[0], numbers[0] + len(numbers))), \
        ('gap or duplicate', breaks(numbers)[:5])
    assert numbers[-1] == ROWS, numbers[-5:]
    if first_expected is not None:
        assert numbers[0] == first_expected, numbers[:5]
    return numbers


def live_paging(pw, root, corpus, uid):
    with fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid), \
         isolated_server(corpus, BINARY, host_dir=root / 'host', audit_dir=root / 'audit') as (base, _):
        browser = launch_browser(pw)
        context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
        started = []
        requests = track(context, base, started)
        audits = []

        def on_audit(request):
            if request.method == 'POST' and '/api/audit/browser' in request.url:
                try:
                    audits.extend(request.post_data_json.get('events', []))
                except (TypeError, ValueError, AttributeError):
                    pass  # A beacon body Playwright cannot decode.
        context.on('request', on_audit)
        errors, dialogs, sockets = [], [], []
        page = context.new_page()
        page.on('pageerror', lambda e: errors.append(str(e)))
        page.on('dialog', lambda d: (dialogs.append(d.message), d.dismiss()))
        page.on('websocket', lambda ws: sockets.append(ws.url))
        try:
            page.goto(base, wait_until='networkidle')
            fixture.open_console(page, uid)
            fixture.xterm_contains(page, f'PAGE_ROW_{ROWS:05d}')
            model = page.evaluate(f'(() => {{ const m = {TERM}.model; return {{older: m.historyOlder, '
                                  f'paging: m.historyPaging, loaded: m.scrollback.length}}; }})()')
            assert model['paging'] and model['older'] > 0 and model['loaded'] <= 2000, model
            first = assert_contiguous(page)[0]
            # The snapshot carries 2000 history rows plus the visible screen.
            assert 1 < first <= ROWS - 2000 and first == model['older'] + 1, (first, model)
            page.wait_for_timeout(800)
            assert not requests, ('idle console fetched history', requests)

            bar = page.get_by_role('scrollbar', name='终端历史')
            thumb = page.locator('.grid-scrollbar-thumb')
            bar.wait_for(state='visible')
            page.evaluate(f'window.pagingBytes = []; {TERM}.onData(d => pagingBytes.push(d))')

            # 1. Scrollbar drag to the top: one page is prepended and the row
            #    that was at the top stays at the top.
            track_box, thumb_box = bar.bounding_box(), thumb.bounding_box()
            x = thumb_box['x'] + thumb_box['width'] / 2
            page.mouse.move(x, thumb_box['y'] + thumb_box['height'] / 2)
            page.mouse.down()
            page.mouse.move(x, track_box['y'] + 1, steps=12)
            page.mouse.up()
            wait_requests(page, requests, 1)
            settle(page)
            assert page.evaluate(TOP_TEXT) == f'PAGE_ROW_{first:05d}', page.evaluate(TOP_TEXT)
            top = page.evaluate(f'{TERM}.buffer.active.viewportY')
            assert top == 500, top
            second = assert_contiguous(page)[0]
            assert second == first - 500, (first, second)
            print('PASS scrollbar drag pages 500 older rows; viewport stays on the same row', flush=True)

            # 2. Real wheel input back up to the oldest loaded row. Stop as soon
            #    as the page request starts and remember the row on top then.
            page.locator('.grid-canvas').hover()
            seen = len(started)
            for _ in range(400):
                if len(started) > seen:
                    break
                page.mouse.wheel(0, -120)
            mark = page.evaluate(f'(() => {{ const b = {TERM}.buffer.active; return {{y: b.viewportY,'
                                 f' text: b.getLine(b.viewportY)?.translateToString(true) || ""}}; }})()')
            wait_requests(page, requests, len(started))
            settle(page)
            third = assert_contiguous(page)[0]
            assert third == second - 500, (second, third)
            assert page.evaluate(TOP_TEXT) == mark['text'], (page.evaluate(TOP_TEXT), mark)
            row = int(re.search(r'(\d{5})', mark['text']).group(1))
            # Paging starts within 40 rows of the oldest loaded row.
            assert second <= row <= second + 40, (mark, second)
            print('PASS wheel reaching the oldest row pages again without a jump', flush=True)

            # 3. Scrollbar keys: Home repeatedly until the host has nothing older.
            for _ in range(10):
                if page.evaluate(f'{TERM}.model.historyOlder') == 0:
                    break
                seen = len(requests)
                bar.focus()
                bar.press('Home')
                wait_requests(page, requests, seen + 1)
                settle(page)
            assert page.evaluate(f'{TERM}.model.historyOlder') == 0
            assert_contiguous(page, first_expected=1)
            assert page.evaluate(f'{TERM}.buffer.active.getLine(0).translateToString(true)') == 'PAGE_ROW_00001'
            for item in requests:
                assert item['status'] == 200, item
                query = dict(part.split('=', 1) for part in item['url'].split('?', 1)[1].split('&'))
                assert query['token'] and query['instance_id'] and query['uid'], query
                # 500 rows plus the seam overlap and slack, far below the
                # host's 2000-row page cap.
                assert 0 <= int(query['from']) < int(query['to']) <= ROWS, query
                assert int(query['to']) - int(query['from']) <= 500 + 4 + 64, query
            count = len(requests)
            bar.press('Home')
            page.mouse.wheel(0, -600)
            page.wait_for_timeout(500)
            assert len(requests) == count, 'fetched after the host history was exhausted'
            assert not page.evaluate('pagingBytes'), 'history scrolling reached the PTY'
            print(f'PASS keyboard paging reaches row 1; rows 1..{ROWS} contiguous after {count} pages', flush=True)

            # 4. Live input and output still work after paging; seams stay intact.
            bar.press('End')
            page.wait_for_function(f'{TERM}.buffer.active.viewportY === {TERM}.buffer.active.baseY')
            keyboard = page.locator('#termpane .xterm-helper-textarea')
            keyboard.press_sequentially('more')
            keyboard.press('Enter')
            fixture.xterm_contains(page, 'PAGE_LIVE_030')
            keyboard.press_sequentially('ping')
            keyboard.press('Enter')
            fixture.xterm_contains(page, 'PAGE_PING_OK')
            assert_contiguous(page, first_expected=1)
            page.wait_for_timeout(600)
            assert len(requests) == count, 'live output caused history requests'
            print('PASS live input/output after paging; no idle history requests', flush=True)

            # 5. Reload: a fresh connection pages again from its own snapshot.
            page.reload(wait_until='networkidle')
            fixture.open_console(page, uid, history=False)
            fixture.xterm_contains(page, 'PAGE_PING_OK')
            assert page.evaluate(f'{TERM}.model.historyPaging')
            seen = len(requests)
            page.get_by_role('scrollbar', name='终端历史').press('Home')
            wait_requests(page, requests, seen + 1)
            settle(page)
            assert requests[-1]['status'] == 200, requests[-1]
            numbers = assert_contiguous(page)
            assert numbers[0] < ROWS - 2000, numbers[0]
            print('PASS reload reconnects and pages from the new snapshot', flush=True)

            # 6. A narrower window reflows both sides; paging re-aligns first.
            #    Rows the host moves between its history and screen while it
            #    reflows are aligned with its history tail, so there is no seam
            #    at the newest edge either (terminal_reflow_browser covers more).
            cols = page.evaluate(f'{TERM}.cols')
            page.set_viewport_size({'width': 900, 'height': 900})
            page.wait_for_function(f'cols => {TERM}.cols !== cols && {TERM}.model.historyResync', arg=cols)
            page.wait_for_function(f'!{TERM}.model.reflowPending && !{TERM}.model.tailSync'
                                   ' && !currentTermViewObject()?.historyTailLoading', timeout=10000)
            page.wait_for_timeout(300)
            numbers = page.evaluate(NUMBERS)
            seams = breaks(numbers)
            assert not seams, ('seam at the newest edge after a width change', seams[:5])
            loaded = len(numbers)
            for _ in range(3):
                seen = len(requests)
                page.get_by_role('scrollbar', name='终端历史').press('Home')
                wait_requests(page, requests, seen + 1)
                settle(page)
                if len(page.evaluate(NUMBERS)) > loaded:
                    break
            assert all(item['status'] == 200 for item in requests), requests
            paged = page.evaluate(NUMBERS)
            assert len(paged) > loaded and page.evaluate(f'{TERM}.model.historyPaging'), (loaded, len(paged))
            assert paged[len(paged) - loaded:] == numbers, 'paging changed already loaded rows'
            assert breaks(paged) == seams, (breaks(paged), seams)
            assert paged[len(paged) - loaded - 1] + 1 == numbers[0], 'seam after reflow'
            print('PASS width change re-aligns with the host and keeps paging contiguous rows', flush=True)

            # 7. A stale lease token: the server answers one page request with
            #    its real 409. Paging stops quietly for this connection (no
            #    retry, no error shown, no reconnect) and the audit event is
            #    recorded; a reconnect resumes from a fresh snapshot.
            # The first connection's lease ended with the reload in step 5.
            expired = re.search(r'[?&]token=([^&]+)', requests[0]['url']).group(1)
            stale = []

            def stale_token_once(route):
                if stale:
                    route.continue_()
                    return
                stale.append(route.request.url)
                assert 'token=' + expired not in route.request.url
                route.continue_(url=re.sub(r'([?&]token=)[^&]*', lambda m: m.group(1) + expired, route.request.url))
            page.route('**/api/term/grid/history?*', stale_token_once)
            bar = page.get_by_role('scrollbar', name='终端历史')
            assert page.evaluate(f'{TERM}.model.historyPaging && {TERM}.model.historyOlder > 0')
            page.evaluate('window.staleSocket = T.ws')
            before, opened, seen = page.evaluate(NUMBERS), len(sockets), len(requests)
            page.locator('.grid-canvas').hover()
            for _ in range(400):
                if stale:
                    break
                page.mouse.wheel(0, -120)
            wait_requests(page, requests, seen + 1)
            settle(page)
            assert len(stale) == 1 and requests[seen]['status'] == 409, (stale, requests[seen:])
            assert 'token=' + expired in requests[seen]['url'], requests[seen]
            assert page.evaluate(f'{TERM}.model.historyPaging') is False
            assert page.evaluate(NUMBERS) == before, 'a refused page changed the loaded rows'
            # Quiet: more wheel input and scrollbar Home do not fetch, the socket
            # stays open and the console shows no error for this connection.
            for _ in range(20):
                page.mouse.wheel(0, -600)
            bar.focus()
            bar.press('Home')
            page.wait_for_timeout(600)
            assert len(requests) == seen + 1, ('paging retried after 409', requests[seen:])
            assert len(sockets) == opened and page.evaluate('T.ws === window.staleSocket && T.ws.readyState === 1')
            assert not page.evaluate('ConsoleUI.errors.get(S.sel) || ""'), page.evaluate('ConsoleUI.errors.get(S.sel)')
            assert not dialogs and not errors, (dialogs, errors)
            deadline = time.monotonic() + 15
            while not any(event['event'] == 'terminal.history_page_failed' for event in audits):
                assert time.monotonic() < deadline, ('audit event missing', [event['event'] for event in audits])
                page.wait_for_timeout(100)
            failed = [event for event in audits if event['event'] == 'terminal.history_page_failed']
            assert len(failed) == 1, failed
            assert failed[0]['data']['status'] == 409 and failed[0]['data']['name'], failed
            assert failed[0]['data']['error'], failed  # the server's error code
            assert failed[0]['severity'] == 'info' and failed[0]['connection_id'], failed
            print(f"PASS stale token 409 stops paging quietly without retry; audit {failed[0]['data']}", flush=True)

            # The network drops and returns: the console reconnects with a
            # fresh snapshot and paging resumes under the new lease.
            context.set_offline(True)
            page.wait_for_timeout(300)
            context.set_offline(False)
            page.wait_for_function('T.ws && T.ws !== window.staleSocket && T.ws.readyState === WebSocket.OPEN',
                                   timeout=15000)
            page.wait_for_function(f'{TERM}.model.historyPaging && {TERM}.model.historyOlder > 0', timeout=10000)
            assert len(sockets) > opened, sockets
            fresh = page.evaluate(NUMBERS)
            assert fresh and not breaks(fresh) and fresh[-1] == ROWS, (fresh[:3], fresh[-3:], breaks(fresh)[:5])
            seen = len(requests)
            page.get_by_role('scrollbar', name='终端历史').press('Home')
            wait_requests(page, requests, seen + 1)
            settle(page)
            assert requests[seen]['status'] == 200 and 'token=' + expired not in requests[seen]['url'], requests[seen:]
            resumed = assert_contiguous(page)
            assert resumed[0] < fresh[0] and resumed[len(resumed) - len(fresh):] == fresh, (resumed[0], fresh[0])
            assert len(stale) == 1, stale
            print('PASS reconnect after the stale token pages again from the fresh snapshot', flush=True)
            assert not errors and not dialogs, (errors, dialogs)
        finally:
            context.close()
            browser.close()


CAP_HISTORY = 60
CAP_ROWS = 600
# Bursts of 15 rows with a pause stay below one capture's worth of a full
# history, so every scrolled row is expected live.
CAP_SHELL = f"""stty -echo
printf 'RS_SHELL_READY\\n'
while IFS= read -r command; do
  case "$command" in
    flood)
      i=1
      while [ "$i" -le {CAP_ROWS} ]; do
        printf 'CAP_ROW_%05d\\n' "$i"
        if [ $((i % 15)) -eq 0 ]; then sleep 0.03; fi
        i=$((i + 1))
      done
      printf 'CAP_FLOOD_DONE\\n' ;;
    ping) printf 'CAP_PING_OK\\n' ;;
  esac
done
"""
CAP_NUMBERS = NUMBERS.replace('PAGE_ROW_', 'CAP_ROW_')


def full_history_live(pw, root, corpus, uid):
    """A host whose `--history` is full keeps streaming scrolled rows live."""
    fixture.SHELL = CAP_SHELL
    with fixture.host(root, 'synthetic-' + uuid.uuid4().hex, uid, history=CAP_HISTORY), \
         isolated_server(corpus, BINARY, host_dir=root / 'host') as (base, _):
        browser = launch_browser(pw)
        context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
        requests = track(context, base)
        errors = []
        sockets = []
        page = context.new_page()
        page.on('pageerror', lambda e: errors.append(str(e)))
        page.on('websocket', lambda ws: sockets.append(ws.url))
        try:
            page.goto(base, wait_until='networkidle')
            fixture.open_console(page, uid)
            opened = len(sockets)
            keyboard = page.locator('#termpane .xterm-helper-textarea')
            keyboard.press_sequentially('flood')
            keyboard.press('Enter')
            fixture.xterm_contains(page, 'CAP_FLOOD_DONE', timeout=30000)
            numbers = page.evaluate(CAP_NUMBERS)
            assert numbers == list(range(1, CAP_ROWS + 1)), \
                ('rows missing after the host history filled', len(numbers), breaks(numbers)[:5], numbers[-3:])
            model = page.evaluate(f'(() => {{ const m = {TERM}.model; return {{base: m.historyBase, '
                                  f'appended: m.historyAppended, loaded: m.scrollback.length}}; }})()')
            # Rows beyond the host cap arrived through diff.scrolled; the host
            # total estimate follows history_total instead of growing past it.
            assert model['appended'] > 5 * CAP_HISTORY and model['loaded'] >= model['appended'], model
            assert model['base'] + model['appended'] == CAP_HISTORY, model
            assert len(sockets) == opened, ('console reconnected', sockets)
            keyboard.press_sequentially('ping')
            keyboard.press('Enter')
            fixture.xterm_contains(page, 'CAP_PING_OK')
            assert page.evaluate(CAP_NUMBERS) == numbers
            assert not requests, ('full history fetched host history', requests)
            assert not errors, errors
            print(f'PASS rows 1..{CAP_ROWS} stream live past a {CAP_HISTORY}-row host history', flush=True)
        finally:
            context.close()
            browser.close()


def replay_never_pages(pw, root):
    cfg = root / 'launcher.json'
    cfg.touch(mode=0o600)
    cfg.write_text(json.dumps({
        'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'), 'host_dir': str(root / 'host'),
        'adapters': [{'id': 'synthetic-shell-v1', 'source': 'shell', 'executable': str(Path('/bin/sh').resolve()),
                      'args': ['-c', REPLAY_SHELL], 'env': {'PATH': '/usr/bin:/bin', 'TERM': 'xterm-256color'}}],
        'profiles': []}))
    initialize('--initialize-lifecycle', root / 'ledger')
    with isolated_server(Corpus(root), BINARY, host_dir=root / 'host', lifecycle_dir=root / 'ledger',
                         launcher_config=cfg, state_dir=root / 'state') as (base, _):
        browser = launch_browser(pw)
        context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
        requests = track(context, base)
        errors = []
        page = context.new_page()
        page.on('pageerror', lambda e: errors.append(str(e)))
        try:
            page.goto(base, wait_until='networkidle')
            shell = context.request.post(base + '/api/term/create', data={
                'source': 'shell', 'cwd': str(root / 'work'), 'request_id': 'history-paging-replay'}).json()
            page.evaluate('info => openPendingSession(info)', shell)
            page.wait_for_function('T.ws?.readyState === WebSocket.OPEN', timeout=10000)
            keyboard = page.locator('#termpane .xterm-helper-textarea')
            for _ in range(60):
                keyboard.press_sequentially('x')
                keyboard.press('Enter')
            fixture.xterm_contains(page, 'RS_UNKNOWN')
            keyboard.press_sequentially('quit')
            keyboard.press('Enter')
            page.wait_for_function("document.querySelector('#termpane').classList.contains('replay')"
                                   " && [...T.views.values()].some(v => v.replay && v.ws?.readyState === 1)",
                                   timeout=15000)
            page.wait_for_function('currentTermViewObject().term.model.scrollback.length > 0', timeout=15000)
            # Even if a replay model claimed older host rows, the console must not ask.
            page.evaluate('(() => { const m = currentTermViewObject().term.model;'
                          ' m.historyOlder = 500; m.historyPaging = true; })()')
            page.locator('.grid-canvas').hover()
            for _ in range(40):
                page.mouse.wheel(0, -300)
            bar = page.get_by_role('scrollbar', name='终端历史')
            if bar.is_visible() and bar.get_attribute('aria-disabled') != 'true':
                bar.press('Home')
            page.wait_for_timeout(600)
            assert page.evaluate('currentTermViewObject().term.buffer.active.viewportY') == 0
            assert not requests, ('replay requested host history', requests)
            assert not errors, errors
            print('PASS recording replay never requests host history', flush=True)
        finally:
            context.close()
            browser.close()


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-history-paging-') as tmp, \
            private_hosts(Path(tmp), hosts=('live/host', 'replay/host', 'capped/host')), sync_playwright() as pw:
        root = Path(tmp)
        live = root / 'live'
        replay = root / 'replay'
        capped = root / 'capped'
        for parent, names in ((live, ['host', 'work', 'audit', 'claude', 'codex', 'grok']),
                              (capped, ['host', 'work', 'claude', 'codex', 'grok']),
                              (replay, ['host', 'work', 'ledger', 'state', 'claude', 'codex', 'grok'])):
            parent.mkdir(mode=0o700)
            for name in names:
                (parent / name).mkdir(mode=0o700)
        corpus = Corpus(live)
        sid = 'synthetic-native-sid'
        corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(live / 'work')}),
                                  codex_message('user', 'Synthetic history paging')], [])
        fixture.SHELL = SHELL
        live_paging(pw, live, corpus, corpus.uid(sid))
        replay_never_pages(pw, replay)
        capped_corpus = Corpus(capped)
        capped_corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': str(capped / 'work')}),
                                         codex_message('user', 'Synthetic full host history')], [])
        full_history_live(pw, capped, capped_corpus, capped_corpus.uid(sid))
        print('PASS terminal_history_paging_browser', flush=True)


if __name__ == '__main__':
    main()
