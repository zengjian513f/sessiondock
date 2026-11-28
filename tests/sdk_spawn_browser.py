#!/usr/bin/env python3
"""Direct local SDK create callers keep their display parent across retries and detach."""
import argparse
import http.client
import json
from pathlib import Path
import tempfile
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright
from agy_sdk_browser import select_sdk, send
from agy_sdk_fixtures import SID as AGY_SID
from codex_sdk_browser import THREAD, build as codex_corpus
from history_fixtures import BINARY, Corpus, codex_row, codex_message
from new_session_model_browser import choose_model, open_dialog
from sdk_start_stop_fixtures import StartDaemon, provider
from spawned_by_fixtures import proc_pid


def hosted_caller(page, fixture, proc):
    uid = select_sdk(page)
    text = 'Dispatch a review from this SDK session'
    send(page, text)
    expect(page.locator('.native-sdk-pane .msgs')).to_contain_text('Synthetic Agy reply: ' + text)
    agy_pid = fixture.inputs(text)[0]['pid']
    agy_status = next(status for status in (fixture.root/'state/sdkhost').glob('*/status.json')
                      if json.loads(status.read_text())['pid'] == agy_pid)
    assert json.loads(agy_status.read_text())['exit'] is None
    # Preserve the fixture's actual native process incarnation while modeling
    # the tool subprocess that owns the create request's socket.
    stat = Path(f'/proc/{agy_pid}/stat').read_text()
    host_pid = int(stat.rsplit(')', 1)[1].split()[1])
    for pid, name in ((agy_pid, 'agy'), (host_pid, 'sdkhost')):
        path = proc/str(pid)
        path.mkdir()
        (path/'stat').write_text(Path(f'/proc/{pid}/stat').read_text())
        (path/'cmdline').write_bytes(name.encode() + b'\0')
    proc_pid(proc, 200, 'python', ['python', 'dispatch.py'], agy_pid, fds={4:'socket:[900]'})
    return uid


def prepare_create(page, source, cwd):
    open_dialog(page)
    page.locator(f'input[name=new-source][value={source}]').check()
    page.locator('#new-cwd').fill(str(cwd))
    if source == 'opencode':
        choose_model(page, 'Synthetic OpenCode')


def exercise(browser, base, source, peer, root, proc, parent_uid, args, *, agy=None):
    page = browser.new_page(viewport={'width':1280,'height':900})
    page.set_default_timeout(20000)
    page.goto(base, wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    parent_sid = THREAD
    if agy:
        parent_uid = hosted_caller(page, agy, proc)
        parent_sid = AGY_SID
    parent_source = parent_uid.split(':', 1)[0]
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
    prepare_create(page, source, getattr(peer, 'work', root))
    with page.expect_response(lambda r: r.url.endswith(f'/api/{endpoint}/create')) as created:
        page.locator('#new-session-go').click()
    assert created.value.ok, created.value.text()
    receipt = created.value.json()
    attached = receipt['session'].get('nest_parent') or {}
    assert (attached.get('source'), attached.get('sid')) == (parent_source, parent_sid), receipt['session']
    expect(page.locator('#new-session-dialog')).to_be_hidden()
    page.wait_for_function('uid=>S.sel===uid',arg=receipt['uid'])
    expect(page.locator('#sdk-input')).to_be_visible()
    uid = page.evaluate('S.sel')
    page.wait_for_function('''async uid=>{await loadSessions(true);
        return sidebarSessions().find(r=>r.uid===uid)?.nest_parent?.sid===%s}''' % json.dumps(parent_sid), arg=uid)
    before = len(peer.methods('thread/start')) if source == 'codex' else None
    status, raw = forward(captured[-1])
    assert status == 200 and json.loads(raw)['uid'] == uid
    if source == 'codex':
        assert len(peer.methods('thread/start')) == before
    page.reload(wait_until='domcontentloaded')
    page.wait_for_function('T.listLoaded')
    page.locator('#nest-toggle').click()
    if page.evaluate('source=>S.off.has(source)', parent_source):
        page.locator(f'#chips button[data-source={parent_source}]').click()
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
        prefix = f'agy-to-{source}' if agy else source
        page.screenshot(path=str(args.screenshots/f'{prefix}-sdk-spawn-nested.png'),full_page=True)
        page.set_viewport_size({'width':390,'height':844})
        page.emulate_media(color_scheme='dark')
        if page.locator('#detail .mobile-back').is_visible():
            page.locator('#detail .mobile-back').click()
        page.screenshot(path=str(args.screenshots/f'{prefix}-sdk-spawn-mobile.png'),full_page=True)
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
    prepare_create(page, source, getattr(peer, 'work', root))
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
        prepare_create(page, source, root)
        with page.expect_response(lambda r: r.url.endswith('/api/codex-sdk/create')) as created:
            page.locator('#new-session-go').click()
        assert created.value.ok, created.value.text()
        daemon_uid = created.value.json()['uid']
        expect(page.locator('#new-session-dialog')).to_be_hidden()
        page.wait_for_function('uid=>sidebarSessions().some(r=>r.uid===uid)',arg=daemon_uid)
        assert not page.evaluate('uid=>sidebarSessions().find(r=>r.uid===uid).nest_parent',daemon_uid)
    page.close()
    print(f'PASS {parent_source} → {source}: caller socket, click create, nested sidebar, retry/reload, explicit detach, ordinary create', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='codex,kiro,claude,grok,agy,opencode,codebuddy,agy-to-codex')
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
                if source == 'agy-to-codex':
                    corpus = codex_corpus(root)
                    socket = root/'app.sock'
                    with StartDaemon(socket, corpus.paths['sdk'], THREAD) as peer:
                        with provider('agy', root, args.binary, extra_env={
                            'SESSIONDOCK_PROC_ROOT': str(proc),
                            'SESSIONDOCK_CODEX_APP_SERVER': str(socket),
                        }) as (base, agy, _):
                            exercise(browser, base, 'codex', peer, root, proc, None, args, agy=agy)
                    continue
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
