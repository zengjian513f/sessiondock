#!/usr/bin/env python3
"""Composer boundary acceptance against the CURRENT checkout's compiled Vue.

Run on a POSIX machine with Playwright Chromium already installed:
  python3 tests/composer_boundaries_browser.py --repo /path/to/current-checkout

--repo can explicitly select a checkout when running from a separate worktree. Its
tests/composer_editor_browser.py fixture/type_text and send_browser.py session
helper are imported, and its web/dist-migration is served by default. Optional
--binary and --web-dir select already-built artifacts; this suite builds nothing.
Requires that checkout's target/debug/ptyhost, fake_claude_cli.py, and the helper
modules' dependencies. PLAYWRIGHT_CHROMIUM_EXECUTABLE is supported.

Only private loopback fixtures, temporary homes and a Python fake CLI are used.
File inputs are reached through the attachment menu and real file chooser. The
512 MiB + 1 byte file is sparse; an upload route aborts any guard regression
before it reaches the server. A blocked upload is a FAILURE, never evidence of
client rejection. No application state/function or synthetic unload event is
injected. Browser Back triggers native beforeunload after real user activation.

The frozen behavior baseline limits a draft to 12 files and 512 MiB per file.
All cases run independently so a failed boundary cannot hide unload coverage.
"""

import argparse
import os
import sys
import tempfile
import traceback
from pathlib import Path
from urllib.parse import parse_qs, urlsplit


MAX_FILE_BYTES = 512 * 1024 * 1024
EXPECTED_FILE_LIMIT = 12
ATTACHMENT_PATH = '/api/session/conversation/attachment'
DRAFT_PATH = '/api/session/conversation'
INVALID_NAMES = {'boundary-empty.txt', 'boundary-oversize.bin'}
CASES = ('empty-file', 'oversize-file', 'file-limit', 'beforeunload')


def choose_files(page, paths):
    """Exercise the same menu/file chooser as composer_editor_browser."""
    from playwright.sync_api import expect

    expect(page.locator('#cadd')).to_be_enabled()
    page.locator('#cadd').click()
    menu = page.locator('#attach-menu')
    expect(menu).to_be_visible()
    with page.expect_file_chooser() as chooser:
        menu.locator('[data-attach=file]').click()
    assert chooser.value.is_multiple(), 'attachment chooser must allow multiple files'
    chooser.value.set_files([str(path) for path in paths])
    expect(menu).to_be_hidden()


def accept_alert(page, expected=None):
    from playwright.sync_api import expect

    popup = page.locator('dialog.app-popup[open]')
    expect(popup).to_be_visible()
    expect(popup).to_have_attribute('data-popup-type', 'alert')
    message = popup.get_attribute('data-message')
    if expected is not None:
        assert message == expected, (message, expected)
    popup.locator('[data-popup-action=ok]').click()
    expect(popup).to_be_hidden()
    return message


def rejection(page, root, name, attempts):
    from composer_editor_browser import type_text
    from playwright.sync_api import expect

    editor = page.locator('#cinput')
    retained = f'draft retained after rejecting {name}'
    type_text(editor, retained)
    choose_files(page, [root / 'samples' / name])
    accept_alert(page, f'「{name}」为空或超过 512 MB')
    expect(page.locator('#compose-items .draft-card')).to_have_count(0)
    expect(editor).to_have_value(retained)
    assert not attempts, f'rejected file reached upload transport: {attempts}'

    # A tiny accepted file is the control: a disconnected/non-working chooser
    # cannot satisfy this test merely by showing no cards or upload requests.
    tiny = root / 'samples' / 'boundary-control.txt'
    with page.expect_response(lambda response:
            urlsplit(response.url).path == ATTACHMENT_PATH
            and response.request.method == 'POST') as uploaded:
        choose_files(page, [tiny])
    assert uploaded.value.status == 200, uploaded.value.text()
    card = page.locator('#compose-items .draft-card').filter(has_text=tiny.name)
    expect(card).to_have_class('draft-card ready')
    expect(editor).to_have_value(retained)
    assert attempts == [tiny.name], f'invalid bytes must never be uploaded: {attempts}'
    print(f'PASS {name}: client alert, no invalid card/upload, draft retained, tiny control uploads',
          flush=True)


def file_limit(page, root, attempts):
    from composer_editor_browser import type_text
    from playwright.sync_api import expect

    text = 'draft retained at attachment count boundary'
    editor = page.locator('#cinput')
    type_text(editor, text)
    files = [root / 'samples' / f'boundary-count-{index:02d}.txt'
             for index in range(1, EXPECTED_FILE_LIMIT + 2)]
    choose_files(page, files)
    message = accept_alert(page)
    cards = page.locator('#compose-items .draft-card')
    actual = cards.locator('.draft-info b').all_text_contents()
    expected = [path.name for path in files[:EXPECTED_FILE_LIMIT]]
    assert actual == expected, (
        f'12-file acceptance mismatch: retained {len(actual)} of 13 selected files; '
        f'product alert={message!r}; expected exactly the first 12 files')
    assert message == '一次最多添加 12 个附件', message
    expect(editor).to_have_value(text)
    expect(cards.locator('.draft-info b')).to_have_text(expected)
    expect(page.locator('#compose-items .draft-card.ready')).to_have_count(
        EXPECTED_FILE_LIMIT, timeout=30000)
    assert len(attempts) == EXPECTED_FILE_LIMIT and set(attempts) == set(expected), attempts

    # Another chooser action at capacity must reject without disturbing any
    # existing card. Reuse the overflow file; it has never entered the draft.
    choose_files(page, [files[-1]])
    accept_alert(page, '一次最多添加 12 个附件')
    expect(cards.locator('.draft-info b')).to_have_text(expected)
    expect(editor).to_have_value(text)
    assert len(attempts) == EXPECTED_FILE_LIMIT, attempts
    print('PASS file-limit: first 12 accepted/uploaded, file 13 rejected, full draft retained',
          flush=True)


def cancel_back(page, editor, text, native_dialogs, card_name=None):
    """Real browser history navigation, not dispatchEvent or the product handler."""
    from playwright.sync_api import expect

    current_url = page.url
    before = len(native_dialogs)
    cdp = page.context.new_cdp_session(page)
    try:
        history = cdp.send('Page.getNavigationHistory')
        assert history['currentIndex'] > 0, history
        target = history['entries'][history['currentIndex'] - 1]['id']
        # CDP issues the real Back navigation without waiting for a document
        # load which intentionally never occurs when beforeunload is cancelled.
        with page.expect_event('dialog', timeout=10000) as event:
            cdp.send('Page.navigateToHistoryEntry', {'entryId': target})
    finally:
        cdp.detach()
    assert event.value.type == 'beforeunload', event.value.type
    assert native_dialogs[before:] == ['beforeunload'], native_dialogs
    assert page.url == current_url, (page.url, current_url)
    expect(editor).to_be_visible()
    expect(editor).to_have_value(text)
    if card_name:
        expect(page.locator('#compose-items .draft-card').filter(
            has_text=card_name)).to_be_visible()


def beforeunload(page, root, native_dialogs):
    from composer_editor_browser import type_text
    from playwright.sync_api import expect

    editor = page.locator('#cinput')
    text = 'unsaved boundary draft survives cancelled browser Back'

    def fail_save(route):
        if route.request.method == 'POST':
            route.fulfill(status=503, json={'error': 'synthetic boundary save outage'})
        else:
            route.continue_()

    # A real failed draft save makes the text dirty. Only the network is
    # intercepted; the application decides whether an unload warning is needed.
    page.route('**/api/session/conversation', fail_save)
    type_text(editor, text)
    expect(page.locator('.draft-save-error')).to_contain_text('synthetic boundary save outage')
    cancel_back(page, editor, text, native_dialogs)
    with page.expect_response(lambda response:
            urlsplit(response.url).path == DRAFT_PATH
            and response.request.method == 'POST'
            and response.request.post_data_json['value']['text'] == text
            and response.status == 200, timeout=20000) as saved:
        page.unroute('**/api/session/conversation', fail_save)
    assert saved.value.status == 200, saved.value.text()
    expect(page.locator('.draft-save-error')).to_have_count(0)
    expect(editor).to_have_value(text)

    # Persisted metadata cannot replace browser-only bytes. Fail just a tiny
    # attachment upload; the real failed card must retain its File and warn.
    pending = root / 'samples' / 'boundary-pending.txt'

    def fail_upload(route):
        if route.request.method == 'POST':
            route.fulfill(status=422, json={'error': 'synthetic boundary upload outage'})
        else:
            route.continue_()

    page.route('**/api/session/conversation/attachment?*', fail_upload)
    choose_files(page, [pending])
    card = page.locator('#compose-items .draft-card').filter(has_text=pending.name)
    expect(card).to_have_class('draft-card failed')
    expect(card).to_contain_text('synthetic boundary upload outage')
    cancel_back(page, editor, text, native_dialogs, pending.name)

    # After cancellation the same card's actual Retry button must still upload
    # the original bytes. This proves retention beyond merely visible metadata.
    page.unroute('**/api/session/conversation/attachment?*', fail_upload)
    with page.expect_response(lambda response:
            urlsplit(response.url).path == ATTACHMENT_PATH
            and response.request.method == 'POST') as uploaded:
        card.locator('.draft-retry').click()
    assert uploaded.value.status == 200, uploaded.value.status
    # Read the private staged bytes through the normal HTTP API. Use the UID
    # and upload id from the actual request, never application state.
    query = parse_qs(urlsplit(uploaded.value.url).query)
    from urllib.parse import urlencode
    staged = page.context.request.get(
        page.url.split('?', 1)[0].rstrip('/') + ATTACHMENT_PATH + '?' + urlencode({
            'uid': query['uid'][0], 'id': query['id'][0]}))
    assert staged.status == 200 and staged.body() == pending.read_bytes(), (
        f'retried attachment did not retain its original bytes: HTTP {staged.status}')
    expect(card).to_have_class('draft-card ready')
    expect(editor).to_have_value(text)

    # Drain the real save queue with one final user edit and its real 200 reply.
    # Then the same Back navigation must succeed without a stale warning.
    with page.expect_response(lambda response:
            urlsplit(response.url).path == DRAFT_PATH
            and response.request.method == 'POST'
            and response.request.post_data_json['value']['text'] == text + '!'
            and response.status == 200, timeout=20000):
        editor.click()
        editor.press('End')
        editor.press_sequentially('!')
    expect(page.locator('.draft-save-error')).to_have_count(0)
    expect(editor).to_have_value(text + '!')
    before = len(native_dialogs)
    page.go_back(wait_until='domcontentloaded', timeout=10000)
    assert urlsplit(page.url).path == '/composer-boundary-navigation', page.url
    assert len(native_dialogs) == before, f'fully saved draft still warned: {native_dialogs}'
    print('PASS beforeunload: native Back cancelled for unsaved text and local file; '
          'draft/File retained; Retry uploads; saved draft leaves without warning', flush=True)


def exercise(browser, base, root, case):
    from playwright.sync_api import expect
    from send_browser import create_claude

    context = browser.new_context(viewport={'width': 1280, 'height': 900},
                                  service_workers='block')
    attempts, sends, errors, native_dialogs = [], [], [], []
    receipt = None

    def network(route):
        request = route.request
        path = urlsplit(request.url).path
        if not request.url.startswith(base + '/'):
            route.abort()
        elif path == '/composer-boundary-navigation':
            route.fulfill(content_type='text/html', body='<p>Private navigation destination</p>')
        elif path == ATTACHMENT_PATH and request.method == 'POST':
            name = parse_qs(urlsplit(request.url).query).get('name', [''])[0]
            if name in INVALID_NAMES:
                route.abort()  # Safety net only: assertions must fail for this request.
            else:
                route.continue_()
        elif path == '/api/session/conversation/send' and request.method == 'POST':
            route.abort()  # These cases never authorize a SEND, even to the fake CLI.
        else:
            route.continue_()

    def track(request):
        path = urlsplit(request.url).path
        if request.method == 'POST' and path == ATTACHMENT_PATH:
            attempts.append(parse_qs(urlsplit(request.url).query).get('name', [''])[0])
        elif request.method == 'POST' and path == '/api/session/conversation/send':
            sends.append(request.url)

    context.route('**/*', network)
    context.on('request', track)
    page = context.new_page()
    page.set_default_timeout(10000)
    page.on('pageerror', lambda error: errors.append(str(error)))

    def dismiss_native(dialog):
        native_dialogs.append(dialog.type)
        dialog.dismiss()  # Native beforeunload cancellation, not an app popup.

    page.on('dialog', dismiss_native)
    try:
        # Establish a genuine same-origin history entry for Browser Back.
        page.goto(base + '/composer-boundary-navigation', wait_until='domcontentloaded')
        page.goto(base, wait_until='networkidle')
        expect(page.locator('script[type=module][src]')).to_have_count(1)
        receipt = create_claude(page, base, root / 'work', open_terminal=False)
        expect(page.locator('#cinput')).to_be_enabled()
        expect(page.locator('#cadd')).to_be_enabled()
        if case == 'empty-file':
            rejection(page, root, 'boundary-empty.txt', attempts)
        elif case == 'oversize-file':
            rejection(page, root, 'boundary-oversize.bin', attempts)
        elif case == 'file-limit':
            file_limit(page, root, attempts)
        else:
            beforeunload(page, root, native_dialogs)
        assert not errors, errors
        assert not sends, f'boundary operations unexpectedly attempted SEND: {sends}'
        assert not set(attempts).intersection(INVALID_NAMES), attempts
        assert all(kind == 'beforeunload' for kind in native_dialogs), native_dialogs
        if case != 'beforeunload':
            assert not native_dialogs, native_dialogs
    finally:
        try:
            if receipt:
                killed = context.request.post(base + '/api/term/kill', data={
                    'record_id': receipt['record_id'], 'instance_id': receipt['instance_id']})
                assert killed.ok, f'private fake CLI cleanup failed: {killed.status} {killed.text()}'
        finally:
            context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parents[1],
                        help='current main checkout, providing helpers, fake CLI and built Vue')
    parser.add_argument('--binary', type=Path, help='already-built sessiondock binary')
    parser.add_argument('--web-dir', type=Path, help='already-built Vue directory (default: REPO/web/dist-migration)')
    parser.add_argument('--only', choices=CASES, action='append', help='run selected cases; repeatable')
    args = parser.parse_args()
    if os.name != 'posix':
        parser.error('private fake PTY fixture requires POSIX')
    repo = args.repo.resolve(strict=True)
    for helper in ('composer_editor_browser.py', 'send_browser.py', 'history_parity.py'):
        if not (repo / 'tests' / helper).is_file():
            parser.error(f'current checkout helper missing: {helper}')
    sys.path.insert(0, str(repo / 'tests'))
    web_dir = Path(args.web_dir or os.environ.get('SESSIONDOCK_TEST_WEB_DIR') or repo / 'web/dist-migration').resolve(strict=True)
    os.environ['SESSIONDOCK_TEST_WEB_DIR'] = str(web_dir)

    from composer_editor_browser import fixture
    from frontend_paths import entry_asset, frontend_html, html_assets
    from history_parity import REPO, Corpus, isolated_server
    from playwright.sync_api import sync_playwright

    assert REPO.resolve() == repo, f'helpers imported from wrong checkout: {REPO}'
    if not frontend_html().modules:
        # The production frontend is plain classic scripts; these boundaries are the Vue composer's.
        print(f'SKIP composer_boundaries_browser: {web_dir} has no Vue module entry', flush=True)
        return
    entry_asset()
    html_assets(web_dir)
    binary = (args.binary or repo / 'target/debug/sessiondock').resolve(strict=True)
    (repo / 'target/debug/ptyhost').resolve(strict=True)
    print(f'Composer boundaries: helpers={repo}; Vue={web_dir}; binary={binary}', flush=True)
    failures = []
    with tempfile.TemporaryDirectory(prefix='sessiondock-composer-boundaries-') as temporary:
        root = Path(temporary).resolve()
        launcher = fixture(root, binary)
        samples = root / 'samples'
        samples.mkdir(mode=0o700)
        (samples / 'boundary-empty.txt').touch(mode=0o600)
        sparse = samples / 'boundary-oversize.bin'
        with sparse.open('wb') as stream:
            stream.truncate(MAX_FILE_BYTES + 1)
        sparse.chmod(0o600)
        assert sparse.stat().st_size == MAX_FILE_BYTES + 1
        assert sparse.stat().st_blocks * 512 < 1024 * 1024, 'oversize fixture must remain sparse'
        for name in ('boundary-control.txt', 'boundary-pending.txt',
                     *(f'boundary-count-{index:02d}.txt' for index in range(1, EXPECTED_FILE_LIMIT + 2))):
            path = samples / name
            path.touch(mode=0o600)
            path.write_bytes(f'synthetic boundary bytes: {name}\n'.encode())
        with isolated_server(Corpus(root), binary, host_dir=root / 'host',
                lifecycle_dir=root / 'ledger', launcher_config=launcher, state_dir=root / 'state',
                file_roots=(root / 'work',), file_write_roots=(root / 'work',)) as (base, _), \
                sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            try:
                for case in args.only or CASES:
                    print(f'RUN {case}', flush=True)
                    try:
                        exercise(browser, base, root, case)
                    except Exception as error:
                        failures.append(case)
                        print(f'FAIL {case}: {error}', file=sys.stderr, flush=True)
                        traceback.print_exc()
            finally:
                browser.close()
    if failures:
        raise SystemExit('Composer boundary acceptance failed: ' + ', '.join(failures))
    print('PASS composer boundary acceptance', flush=True)


if __name__ == '__main__':
    main()
