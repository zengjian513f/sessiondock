#!/usr/bin/env python3
"""Direct local SDK create callers keep their display parent across retries and detach."""
import argparse
import http.client
import json
from pathlib import Path
import tempfile
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD
from history_fixtures import BINARY, Corpus, codex_row, codex_message
from new_session_model_browser import open_dialog
from sdk_start_stop_fixtures import provider
from spawned_by_fixtures import proc_pid


def exercise(browser, base, source, peer, root, proc, parent_uid, args):
    page = browser.new_page(viewport={'width':1280,'height':900})
    page.set_default_timeout(20000)
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    port = urlsplit(base).port
    endpoint = 'codex-sdk' if source == 'codex' else 'native-sdk'
    captured = []
    identified = True

    def forward(body):
        # Real HTTP peer port paired with this request's synthetic /proc FD.
        connection = http.client.HTTPConnection('127.0.0.1', port, timeout=30)
        connection.connect()
        local = connection.sock.getsockname()[1]
        (proc/'net/tcp').write_text('header\n' + (
            f' 0: 0100007F:{local:04X} 0100007F:{port:04X} 01 0 0 0 0 0 900\n' if identified else ''))
        connection.request('POST',f'/api/{endpoint}/create',json.dumps(body),{'Content-Type':'application/json'})
        response = connection.getresponse()
        status, raw = response.status, response.read()
        connection.close()
        return status, raw

    def intercept(route):
        body = route.request.post_data_json
        captured.append(body)
        status, raw = forward(body)
        route.fulfill(status=status, content_type='application/json', body=raw)

    page.route(f'**/api/{endpoint}/create', intercept)
    open_dialog(page)
    page.locator(f'input[name=new-source][value={source}]').check()
    page.locator('#new-cwd').fill(str(getattr(peer, 'work', root)))
    with page.expect_response(lambda r: r.url.endswith(f'/api/{endpoint}/create')) as created:
        page.locator('#new-session-go').click()
    assert created.value.ok, created.value.text()
    expect(page.locator('#new-session-dialog')).to_be_hidden()
    page.wait_for_function('uid=>S.sel===uid',arg=created.value.json()['uid'])
    expect(page.locator('#sdk-input')).to_be_visible()
    uid = page.evaluate('S.sel')
    page.wait_for_function('''async uid=>{await loadSessions(true);
        return sidebarSessions().find(r=>r.uid===uid)?.nest_parent?.sid===%s}''' % json.dumps(THREAD), arg=uid)
    before = len(peer.methods('thread/start')) if source == 'codex' else None
    status, raw = forward(captured[-1])
    assert status == 200 and json.loads(raw)['uid'] == uid
    if source == 'codex':
        assert len(peer.methods('thread/start')) == before
    page.reload(wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    page.locator('#nest-toggle').click()
    if source != 'codex' and page.evaluate("S.off.has('codex')"):
        page.locator('#chips button[data-source=codex]').click()
    row = page.locator(f'#side .item[data-uid="{uid}"]')
    parent = page.locator(f'#side .item[data-uid="{parent_uid}"]')
    if not row.is_visible():
        parent.locator('.nest-caret').click()
    expect(row).to_be_visible()
    expect(row).to_have_attribute('data-depth','1')
    row.click()
    expect(page.locator('#sdk-input')).to_be_visible()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(args.screenshots/f'{source}-sdk-spawn-nested.png'),full_page=True)
        page.set_viewport_size({'width':390,'height':844})
        page.emulate_media(color_scheme='dark')
        if page.locator('#detail .mobile-back').is_visible():
            page.locator('#detail .mobile-back').click()
        page.screenshot(path=str(args.screenshots/f'{source}-sdk-spawn-mobile.png'),full_page=True)
        page.set_viewport_size({'width':1280,'height':900})
        page.emulate_media(color_scheme='light')
    row.click(button='right')
    with page.expect_response(lambda r: r.url.endswith('/api/session/nest')) as detached:
        page.locator('#item-menu [data-act="detach"]').click()
    assert detached.value.ok, detached.value.text()
    page.wait_for_function('uid=>{const r=sidebarSessions().find(r=>r.uid===uid);return r && !r.nest_parent}',arg=uid)
    assert forward(captured[-1])[0] == 200
    page.reload(wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    page.wait_for_function('uid=>sidebarSessions().some(r=>r.uid===uid)',arg=uid)
    assert not page.evaluate('uid=>sidebarSessions().find(r=>r.uid===uid).nest_parent',uid)
    # The same SDK daemon must not make ordinary browser creates children.
    identified = False
    open_dialog(page)
    page.locator(f'input[name=new-source][value={source}]').check()
    page.locator('#new-cwd').fill(str(getattr(peer, 'work', root)))
    with page.expect_response(lambda r: r.url.endswith(f'/api/{endpoint}/create')) as created:
        page.locator('#new-session-go').click()
    assert created.value.ok, created.value.text()
    expect(page.locator('#new-session-dialog')).to_be_hidden()
    page.wait_for_function('uid=>S.sel===uid',arg=created.value.json()['uid'])
    expect(page.locator('#sdk-input')).to_be_visible()
    other = page.evaluate('S.sel')
    page.wait_for_function('uid=>sidebarSessions().some(r=>r.uid===uid)',arg=other)
    assert other != uid and not page.evaluate('uid=>sidebarSessions().find(r=>r.uid===uid).nest_parent',other)
    identified = True
    assert forward(captured[-1])[0] == 200
    page.reload(wait_until='domcontentloaded')
    page.wait_for_function('uid=>sidebarSessions().some(r=>r.uid===uid)',arg=other)
    assert not page.evaluate('uid=>sidebarSessions().find(r=>r.uid===uid).nest_parent',other)
    if source == 'codex':
        # A daemon's inherited environment cannot identify its HTTP clients.
        (proc/'200/cmdline').write_bytes(b'codex\0app-server\0')
        open_dialog(page)
        page.locator('input[name=new-source][value=codex]').check()
        page.locator('#new-cwd').fill(str(root))
        with page.expect_response(lambda r: r.url.endswith('/api/codex-sdk/create')) as created:
            page.locator('#new-session-go').click()
        assert created.value.ok, created.value.text()
        daemon_uid = created.value.json()['uid']
        expect(page.locator('#new-session-dialog')).to_be_hidden()
        page.wait_for_function('uid=>sidebarSessions().some(r=>r.uid===uid)',arg=daemon_uid)
        assert not page.evaluate('uid=>sidebarSessions().find(r=>r.uid===uid).nest_parent',daemon_uid)
    page.close()
    print(f'PASS {source}: caller socket, click create, nested sidebar, retry/reload, explicit detach, ordinary create', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='codex,kiro,claude,grok,agy,opencode,codebuddy')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sd-sdk-spawn-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(tmp)/source; root.mkdir()
                proc = root/'request-proc'; proc.mkdir()
                (proc/'stat').write_text('btime 1700000000\n')
                (proc/'net').mkdir()
                (proc/'net/tcp').write_text('header\n')
                proc_pid(proc, 200, 'python', ['python','dispatch.py'], 1,
                         env=[('CODEX_THREAD_ID',THREAD)], fds={4:'socket:[900]'})
                with provider(source, root, args.binary, extra_env={'SESSIONDOCK_PROC_ROOT':str(proc),'SESSIONDOCK_CODEX_ROOT':str(root/'codex')}) as (base,peer,_):
                    corpus = Corpus(root)
                    if source == 'codex':
                        # provider already has this native parent (same path as build()).
                        # Do not recreate the directories: take the fake daemon's original rollout.
                        corpus.paths['sdk'] = peer.rollout
                        parent_uid = corpus.uid('sdk')
                    else:
                        corpus.put('caller', 'codex', [codex_row('session_meta', {
                            'id':THREAD,'cwd':str(root),'timestamp':'2026-09-11T10:00:00Z'}),
                            codex_message('user','Parent dispatching a complete review task')], [])
                        parent_uid = corpus.uid('caller')
                    exercise(browser,base,source,peer,root,proc,parent_uid,args)
        finally:
            browser.close()


if __name__ == '__main__':
    main()
