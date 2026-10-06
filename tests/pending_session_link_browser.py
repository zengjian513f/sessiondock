#!/usr/bin/env python3
"""Pending launch links survive refresh, sharing and browser navigation on node/Hub."""
import json
import tempfile
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import parse_qs, urlencode, urlsplit

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, batch35_meta, codex_message, isolated_server
from hub_fixtures import Hub, free_port
from hub_send_browser import prepare
from pending_create_discard_browser import STAY
from private_hosts import private_hosts


def selected(page, uid):
    page.wait_for_function("uid => S.sel === uid", arg=uid)
    expect(page.locator('#detail .new-session-wait')).to_be_attached()
    expect(page.locator('#detail h2')).to_be_visible()
    expect(page.locator('#composer')).to_be_visible()


def create(page, root, source):
    if page.viewport_size['width'] == 390 and page.locator('#detail .mobile-back').is_visible():
        page.locator('#detail .mobile-back').click()
    if not page.locator('#new-session').is_visible():
        page.locator('#header-more-btn').click()
    page.locator('#new-session').click()
    page.locator(f'#new-session-form label:has(input[value="{source}"])').click()
    page.locator('#new-cwd').fill(str(root / 'work'))
    with page.expect_response(lambda response: urlsplit(response.url).path == '/api/term/create') as created:
        page.locator('#new-session-go').click()
    receipt = created.value.json()
    assert created.value.status == 200 and receipt['running'], receipt
    selected(page, 'tmux:' + receipt['name'])
    return receipt


def check(browser, base, root, node_id, width):
    context = browser.new_context(viewport={'width': width, 'height': 900}, service_workers='block')
    context.route('**/*', lambda route: route.continue_()
                  if route.request.url.startswith(base + '/') else route.abort())
    errors = []
    page = context.new_page()
    page.on('pageerror', lambda error: errors.append(str(error)))
    old_link = base + '/?' + urlencode({'sid': 'codex:old-link-fixture', 'keep': 'yes'})
    page.goto(old_link)
    expect(page.locator('#msgs')).to_contain_text('Older conversation')
    old_uid = page.evaluate('S.sel')
    receipt = create(page, root, 'codex')
    uid = 'tmux:' + receipt['name']
    selected(page, uid)
    link = page.url
    query = parse_qs(urlsplit(link).query)
    assert query['sid'] == ['tmux:' + receipt['name'].split('~')[-1]], query
    assert query['keep'] == ['yes'], query
    if node_id:
        assert query['node'] == [node_id], query
    page.locator('#cinput').fill('Unsent link fixture draft')
    page.wait_for_function("""() => {
        const draft = composerDrafts.get(S.sel);
        return draft?.text === 'Unsent link fixture draft'
            && draft.savedVersion > 0 && draft.savedVersion === draft.editVersion;
    }""")
    page.reload()
    selected(page, uid)
    expect(page.locator('#cinput')).to_have_value('Unsent link fixture draft')
    # Opening another row then navigating both ways must preserve history.
    if width == 390:
        page.locator('#detail .mobile-back').click()
    page.locator(f'#side .item[data-uid="{old_uid}"] .t').click()
    expect(page.locator('#msgs')).to_contain_text('Older conversation')
    page.go_back()
    selected(page, uid)
    page.go_forward()
    expect(page.locator('#msgs')).to_contain_text('Older conversation')
    # Clicking the pending row must also replace the previously selected SID.
    if width == 390:
        page.locator('#detail .mobile-back').click()
    page.locator(f'#side .item[data-uid="{uid}"] .t').click()
    selected(page, uid)
    assert page.url == link
    if width == 390:
        page.locator('#detail .mobile-back').click()
        page.reload()
        selected(page, uid)  # The explicit link wins over saved list mode.
        page.locator('#detail .mobile-back').click()
        page.goto(base, wait_until='networkidle')
        assert page.evaluate('S.sel') is None
        page.goto(link)
        selected(page, uid)
    # A fresh browser has no local selection/draft. Hold its terminal inventory
    # after the native catalog arrives, then release it to exercise boot ordering.
    fresh = browser.new_context(viewport={'width': width, 'height': 900}, service_workers='block')
    fresh.route('**/*', lambda route: route.continue_()
                if route.request.url.startswith(base + '/') else route.abort())
    other = fresh.new_page()
    other.on('pageerror', lambda error: errors.append(str(error)))
    held = []
    other.route('**/api/term/list', lambda route: held.append(route))
    other.goto(link, wait_until='domcontentloaded')
    other.wait_for_function('S.sessions.length > 0')
    other.wait_for_function('typeof T !== "undefined" && T.listRequest !== null')
    assert other.evaluate('S.sel') is None
    for route in held:
        route.continue_()
    other.unroute('**/api/term/list')
    selected(other, uid)
    # Existing scoped UID links remain accepted too.
    scoped_link = base + '/?' + urlencode({'sid': uid, **({'node': node_id} if node_id else {})})
    other.goto(scoped_link)
    selected(other, uid)
    # A missing launch link must never fall back to the last saved conversation.
    other.goto(old_link)
    expect(other.locator('#msgs')).to_contain_text('Older conversation')
    missing = base + '/?' + urlencode({'sid': 'tmux:sessiondock-missing', **({'node': node_id} if node_id else {})})
    other.goto(missing)
    expect(other.locator('#session-stop-notice')).to_contain_text('暂时无法找到此启动记录')
    assert other.evaluate('S.sel') is None
    # A delayed link lookup cannot undo a later explicit sidebar selection.
    held.clear()
    other.route('**/api/term/list', lambda route: held.append(route))
    other.goto(link, wait_until='domcontentloaded')
    other.wait_for_function('S.sessions.length > 0')
    other.wait_for_function('typeof T !== "undefined" && T.listRequest !== null')
    if width == 390:
        other.evaluate('showMobileList()')
    other.locator(f'#side .item[data-uid="{old_uid}"] .t').click()
    expect(other.locator('#msgs')).to_contain_text('Older conversation')
    for route in held:
        route.continue_()
    other.unroute('**/api/term/list')
    other.wait_for_function('T.listLoaded && !T.listRequest')
    assert other.evaluate('S.sel') == old_uid
    assert not errors, errors
    fresh.close()
    context.close()
    print('PASS pending links', 'hub' if node_id else 'node', width,
          'create, click, reload, fresh browser, delayed inventory, Back/Forward, missing, navigation race', flush=True)


def check_binding(browser, base, root):
    notice = root / 'notice'
    notice.unlink(missing_ok=True)
    context = browser.new_context(service_workers='block')
    page = context.new_page()
    page.goto(base)
    receipt = create(page, root, 'claude')
    pending_link = page.url
    assert parse_qs(urlsplit(pending_link).query)['sid'][0].startswith('tmux:')
    history_length = page.evaluate('history.length')
    notice.write_text('Synthetic startup notice')
    expect(page.locator('#msgs')).to_contain_text('Synthetic startup notice', timeout=20000)
    native = page.evaluate('S.sel')
    assert native.startswith('claude:')
    assert page.evaluate('history.length') == history_length
    assert parse_qs(urlsplit(page.url).query)['sid'] == ['claude:' + receipt['declared_sid']]
    fresh = browser.new_context(service_workers='block')
    other = fresh.new_page()
    other.goto(pending_link)
    expect(other.locator('#msgs')).to_contain_text('Synthetic startup notice', timeout=20000)
    assert other.evaluate('S.sel') == native
    assert parse_qs(urlsplit(other.url).query)['sid'] == ['claude:' + receipt['declared_sid']]
    fresh.close()
    context.close()
    print('PASS retained launch link follows native binding without extra history entry', flush=True)


def main():
    with tempfile.TemporaryDirectory(prefix='sessiondock-pending-links-') as temporary:
        root = Path(temporary)
        with private_hosts(root):
            config = prepare(root)
            stay = root / 'bin/stay'
            stay.write_text(STAY)
            stay.chmod(0o700)
            value = json.loads(config.read_text())
            value['profiles'][0]['env']['SESSIONDOCK_TEST_CLAUDE_NOTICE'] = str(root / 'notice')
            value['profiles'].append({'id': 'codex-cli-v1', 'source': 'codex',
                'executable': str(stay), 'args': [], 'resume_args': ['resume', '{sid}'],
                'env': {'PATH': '/usr/bin:/bin', 'HOME': str(root / 'home'), 'TERM': 'xterm-256color'}})
            config.write_text(json.dumps(value))
            corpus = Corpus(root)
            corpus.put('old', 'codex', [batch35_meta('old-link-fixture'),
                       codex_message('user', 'Older conversation')], ['Older conversation'])
            node = SimpleNamespace(nid='a' * 32, name='FixtureNode', port=free_port(), token='f' * 64)
            token, identity = root / 'node-token', root / 'node-id'
            for path, value in [(token, node.token), (identity, node.nid)]:
                path.touch(mode=0o600)
                path.write_text(value)
            env = {'SESSIONDOCK_NODE_BIND': f'127.0.0.1:{node.port}',
                   'SESSIONDOCK_NODE_TOKEN_FILE': str(token), 'SESSIONDOCK_NODE_ID_FILE': str(identity),
                   'SESSIONDOCK_NODE_PEERS': '127.0.0.0/8'}
            with isolated_server(corpus, BINARY, state_dir=root / 'state', host_dir=root / 'host',
                                 lifecycle_dir=root / 'ledger', launcher_config=config, extra_env=env) as (local, _):
                hub = Hub(BINARY.with_name('sessiondock-hub'), root / 'hub', [node])
                hub.start()
                try:
                    with sync_playwright() as pw:
                        browser = pw.chromium.launch(headless=True)
                        try:
                            for base, node_id in [(local, ''), (f'http://127.0.0.1:{hub.port}', node.nid)]:
                                for width in (1280, 390):
                                    check(browser, base, root, node_id, width)
                                check_binding(browser, base, root)
                        finally:
                            browser.close()
                finally:
                    hub.stop()


if __name__ == '__main__':
    main()
