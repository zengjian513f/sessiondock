#!/usr/bin/env python3
"""Agy persisted history survives delayed SDK replay, pagination and live writes."""
import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from agy_history_browser import record, write_transcript
from agy_sdk_browser import send as send_agy
from agy_sdk_fixtures import SID
from claude_sdk_browser import open_sdk, send, snapshot, wait_snapshot
from history_fixtures import BINARY, encoded
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_mode_fixtures import provider
from sdk_selection_fixtures import snapshots
from sdkhost_fixtures import stop_private_sdkhosts


def expand_turns(page):
    while page.locator('.sdk-msgs .turn-process.folded').count():
        page.locator('.sdk-msgs .turn-process.folded > .turn-toolbar .fold-toggle').first.click()


def exercise(browser, base, peer, label, screenshots):
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        records = [record(0, 'USER_INPUT', '首条输入：核对完整历史。')]
        records[0]['content'] += '\n<USER_SETTINGS_CHANGE>保留模型设置说明</USER_SETTINGS_CHANGE>'
        for index in range(1, 521):
            if index % 2:
                value = record(index, 'PLANNER_RESPONSE', '', thinking=f'思考步骤 {index}')
                value['tool_calls'] = [{'name': 'read_file', 'args': {'AbsolutePath': f'/synthetic/item-{index}.txt'}}]
            else:
                value = record(index, 'GENERIC', f'完整工具结果 {index}：实时通知没有这段正文。' + '\n' + 'native output\n' * 800)
            records.append(value)
        records.extend([record(521, 'PLANNER_RESPONSE', '第一轮的完整结论。'),
                        record(522, 'USER_INPUT', '相同追问'),
                        record(523, 'PLANNER_RESPONSE', '后续结论：连接后仍须保留。'),
                        record(524, 'USER_INPUT', '相同追问'),
                        record(525, 'PLANNER_RESPONSE', '最新结论：不能回退到旧回复。')])
        path = write_transcript(peer.native, SID, records)
        raw = {'running': True, 'items': [], 'requests': [], 'capabilities': {}}
        def item(role, index, content):
            return {'id': f'agy:{SID}:step:{index}', 'role': role, 'content': content,
                    'native': {'step_index': index}, 'status': 'DONE'}
        # Replay is intentionally sparse and behind the already persisted history.
        raw['items'] = [item('user', 0, '首条输入：核对完整历史。'),
                        item('tool', 520, '稀疏工具通知'),
                        item('assistant', 521, '第一轮的完整结论。'),
                        item('user', 522, '相同追问'), item('user', 524, '相同追问')]
        with snapshots(replay=False) as events:
            events.packet = {'type': 'snapshot', 'seq': 0, 'raw': raw}
            context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme)
            page = context.new_page(); errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            context.route('**/api/native-sdk/open', lambda route: route.fulfill(json={'ok': True, 'raw': {}}))
            context.route('**/api/native-sdk/events?*', lambda route: route.continue_(url=events.url))
            for endpoint in ('models', 'usage', 'agents'):
                context.route(f'**/api/native-sdk/{endpoint}*', lambda route: route.fulfill(json={'ok': True, 'raw': {}}))
            page.wait_for_timeout(1100)
            page.request.get(base + '/api/sessions?force=1')
            page.goto(base); page.wait_for_function('T.listLoaded')
            page.locator('#side .item[data-uid^="agy:"]').first.click()
            page.locator('#a-sdk').click()
            expect(page.locator('.sdk-history')).to_contain_text('最新结论：不能回退到旧回复。', timeout=20000)
            expect(page.locator('.sdk-history .history-gap')).to_have_count(1)
            page.wait_for_function('NativeSdkView.historyData.messages[0].native_step_index === 0')
            page.locator('.sdk-history .fold-toggle').first.click()
            before = page.locator('.sdk-history').text_content()
            page.evaluate('window.joinFirst = NativeSdkView.history.querySelector("[data-role=user]")')
            if page.evaluate('SdkView.readOnly'):
                if not page.locator('#a-session-toggle').is_visible(): page.locator('#a-more').click()
                page.locator('#a-session-toggle').click()
            # Exercise a genuinely delayed first snapshot, as in the report.
            page.wait_for_timeout(3100)
            assert page.locator('.sdk-history').text_content() == before
            events.publish()
            page.wait_for_function('NativeSdkView.connected')
            assert page.locator('.sdk-history').text_content() == before
            assert page.evaluate('window.joinFirst === NativeSdkView.history.querySelector("[data-role=user]")')
            expect(page.locator('.sdk-live [data-role=user]')).to_have_count(0)
            expect(page.locator('.sdk-history [data-role=user]')).to_have_count(3)
            expect(page.locator('.sdk-history')).to_contain_text('保留模型设置说明')
            assert page.evaluate('''() => NativeSdkView.history.querySelector('[data-role=user]').compareDocumentPosition(NativeSdkView.history.querySelector('.history-gap')) & Node.DOCUMENT_POSITION_FOLLOWING''')
            # Step-less bridge notices belong to the preceding native step,
            # including a head notice before the pagination gap. An SSE replay
            # alone (no history change) must invalidate the history rendering.
            def notice(name):
                return {'id': 'agy:error:' + name, 'role': 'system', 'error': True,
                        'content': name, 'received_at': '2026-10-04T00:00:00Z'}
            raw['items'].insert(1, notice('headnotice'))
            raw['items'].insert(-1, notice('cancelnotice'))
            events.publish()
            expect(page.locator('.sdk-history')).to_contain_text('cancelnotice')
            expect(page.locator('.sdk-live')).not_to_contain_text('cancelnotice')
            expand_turns(page)
            assert page.evaluate('''() => {
              const rows = [...NativeSdkView.history.querySelectorAll('[data-role=system]')];
              const head = rows.find(node => node.textContent.includes('headnotice'));
              const cancel = rows.find(node => node.textContent.includes('cancelnotice'));
              const users = NativeSdkView.history.querySelectorAll('[data-role=user]');
              return !!(head.compareDocumentPosition(NativeSdkView.history.querySelector('.history-gap')) & Node.DOCUMENT_POSITION_FOLLOWING)
                && !!(users[1].compareDocumentPosition(cancel) & Node.DOCUMENT_POSITION_FOLLOWING)
                && !!(cancel.compareDocumentPosition(users[2]) & Node.DOCUMENT_POSITION_FOLLOWING);
            }''')
            # Fill the middle via the real history pagination endpoint.
            page.locator('.sdk-history .history-gap-load').click()
            expect(page.locator('.sdk-history .history-gap')).to_have_count(0, timeout=30000)
            if page.locator('.sdk-history .turn-process.folded').count():
                page.locator('.sdk-history .turn-process.folded .fold-toggle').first.click()
            page.locator('.sdk-history .grp .fold-toggle').first.click()
            page.locator('.sdk-history .tool-toggle').first.click()
            expect(page.locator('.sdk-history')).to_contain_text('完整工具结果 2：')
            expect(page.locator('.sdk-history .tool-toggle').first).to_have_attribute('aria-expanded', 'true')
            raw['items'] += [item('assistant', 526, '尚未落盘的实时回复')]
            raw['items'][-1]['status'] = 'RUNNING'
            raw['items'] += [{'id': 'pending-user', 'role': 'user', 'content': '尚未回显的输入', 'status': 'queued'}]
            events.publish()
            expect(page.locator('.sdk-live')).to_contain_text('尚未落盘的实时回复')
            expect(page.locator('.sdk-live')).to_contain_text('尚未回显的输入')
            raw['items'][-2]['content'] += '，继续增长。'
            events.publish()
            expect(page.locator('.sdk-live')).to_contain_text('继续增长。')
            # Native writes can arrive without another SDK event. Use the actual
            # history poll path; the stale replay must not overwrite the answer.
            with path.open('ab') as output:
                output.write(encoded(record(526, 'PLANNER_RESPONSE', '完整落盘后的实时回复。')))
            page.evaluate('NativeSdkView.refreshHistory(NativeSdkView.generation, true)')
            expect(page.locator('.sdk-history')).to_contain_text('完整落盘后的实时回复。', timeout=30000)
            expect(page.locator('.sdk-live')).not_to_contain_text('尚未落盘的实时回复')
            expect(page.locator('.sdk-live')).to_contain_text('尚未回显的输入')
            events.publish()
            expect(page.locator('.sdk-history')).to_contain_text('完整落盘后的实时回复。')
            if screenshots:
                page.locator('.sdk-msgs').evaluate('box => box.scrollTop = box.scrollHeight')
                page.screenshot(path=str(screenshots / f'{label}-{width}-{theme}-joined.png'), full_page=True)
            page.reload(); page.wait_for_function('T.listLoaded')
            if not page.locator('.sdk-pane').count(): page.locator('#a-sdk').click()
            expect(page.locator('.sdk-history')).to_contain_text('完整落盘后的实时回复。')
            if page.evaluate('SdkView.readOnly'):
                if not page.locator('#a-session-toggle').is_visible(): page.locator('#a-more').click()
                page.locator('#a-session-toggle').click()
            page.wait_for_timeout(300); events.publish()
            page.wait_for_function('NativeSdkView.connected')
            expect(page.locator('.sdk-history')).to_contain_text('最新结论：不能回退到旧回复。', timeout=20000)
            expect(page.locator('.sdk-history [data-role=user]')).to_have_count(3)
            assert not errors, errors
            context.close()
            print(f'PASS {label} {width} {theme}: delayed connect, sparse replay, stable first input, native tools, gap fill, live append, history catchup, reload', flush=True)


def exercise_transport(browser, base, peer, screenshots):
    (peer.root / 'native-echo-store').touch()
    context = browser.new_context(viewport={'width': 1280, 'height': 900})
    page = context.new_page()
    page.goto(base); page.wait_for_function('T.listLoaded')
    uid = page.locator('#side .item[data-uid^="agy:"]').first.get_attribute('data-uid')
    text = 'verify persisted join ' + ('hub' if '~' in uid else 'node')
    opens = []
    page.on('request', lambda request: opens.append(request.url) if request.url.endswith('/api/native-sdk/open') else None)
    open_sdk(page, peer, expect, uid=uid)
    # Subscription observes an attached runtime. It must not repeat the full
    # native writer scan from /open (nor grant permission to send).
    peer.occupy()
    try:
        assert snapshot(page, uid)['type'] == 'snapshot'
        blocked = page.request.post(base + '/api/native-sdk/open', data={'uid': uid})
        assert blocked.status == 409, blocked.text()
    finally: peer.unoccupy()
    send(page, text)
    expect(page.locator('.sdk-msgs')).to_contain_text('Synthetic Agy reply: ' + text)
    wait_snapshot(page, uid, lambda raw: not raw['running'])
    page.evaluate('NativeSdkView.refreshHistory(NativeSdkView.generation, true)')
    expect(page.locator('.sdk-history')).to_contain_text('Synthetic Agy reply: ' + text)
    expect(page.locator('.sdk-live [data-role=user]')).to_have_count(0)
    opens.clear()
    page.reload(); page.wait_for_function('T.listLoaded')
    open_sdk(page, peer, expect, uid=uid)
    expect(page.locator('.sdk-msgs')).to_contain_text('Synthetic Agy reply: ' + text)
    assert not opens, ('re-entry must subscribe without reopening', opens)
    assert len(peer.inputs(text)) == 1
    # Reproduce the report through the real bridge: cancel a turn, then send
    # new work and let native history advance past the bridge-only notice.
    expand_turns(page)
    notices = page.locator('.sdk-msgs [data-role=system]').filter(has_text='agy_sdk_interrupted:')
    notice_count = notices.count() + 1
    send(page, 'hold:join-cancel')
    expect(page.locator('#sdk-stop')).to_be_visible()
    page.locator('#sdk-stop').click()
    wait_snapshot(page, uid, lambda raw: not raw['running'])
    notice = notices.last
    expect(notices).to_have_count(notice_count)
    for index in range(2):
        followup = f'after cancellation {text} {index}'
        send(page, followup)
        wait_snapshot(page, uid, lambda raw: not raw['running'])
        page.evaluate('NativeSdkView.refreshHistory(NativeSdkView.generation, true)')
        answer = page.locator('.sdk-history [data-role=assistant]').filter(has_text='Synthetic Agy reply: ' + followup)
        expect(answer).to_have_count(1)
        expand_turns(page)
        expect(notices).to_have_count(notice_count)
        user = page.locator('.sdk-history [data-role=user]').filter(has_text=followup)
        expect(user).to_have_count(1)
        assert notice.evaluate('(node, next) => !!(node.compareDocumentPosition(next) & Node.DOCUMENT_POSITION_FOLLOWING)', user.element_handle()), page.evaluate('''() => ({
          raw: NativeSdkView.raw.items.map(m => [m.id, m.role, nativeSdkAgyStep(m)]),
          history: NativeSdkView.historyData.messages.map(m => [m.role, m.native_step_index, m.text]),
          notices: [...nativeSdkAgyHistoryJoin(NativeSdkView).notices.values()]
        })''')
        assert notice.evaluate('(node, next) => !!(node.compareDocumentPosition(next) & Node.DOCUMENT_POSITION_FOLLOWING)', answer.element_handle())
    page.reload(); page.wait_for_function('T.listLoaded')
    open_sdk(page, peer, expect, uid=uid)
    expand_turns(page)
    expect(notices).to_have_count(notice_count)
    expect(answer).to_have_count(1)
    assert notice.evaluate('(node, next) => !!(node.compareDocumentPosition(next) & Node.DOCUMENT_POSITION_FOLLOWING)', answer.element_handle())
    # BUG-20261115-234238-df94ad: native DB/transcript already contain a
    # steered user step while headless output waits for an older background
    # tool. Confirm consumption by clientMessageId before releasing NDJSON.
    send(page, 'hold:delayed-echo-' + text.split()[-1])
    wait_snapshot(page, uid, lambda raw: raw['running'])
    echo_gate = peer.root / 'steer-echo-delay'
    echo_gate.touch()
    repeated = '相同文字的独立引导 ' + text.split()[-1]
    ids = []
    for count in (1, 2):
        page.locator('#sdk-input').fill(repeated)
        with page.expect_response(lambda response: response.url.endswith('/api/native-sdk/steer')) as response:
            page.locator('#sdk-input').press('Control+Enter')
        assert response.value.ok, response.value.text()
        request_id = response.value.json()['request_id']
        ids.append(request_id)
        page.evaluate('NativeSdkView.refreshHistory(NativeSdkView.generation, true)')
        expect(page.locator('.sdk-history [data-role=user]').filter(has_text=repeated)).to_have_count(count)
        expect(page.locator('.sdk-msgs [data-role=user]').filter(has_text=repeated)).to_have_count(count)
        wait_snapshot(page, uid, lambda raw: any(item.get('request_id') == request_id and item.get('native_echo') for item in raw['items']))
        expect(page.locator('.sdk-live [data-role=user]').filter(has_text=repeated)).to_have_count(0)
    assert len(set(ids)) == 2
    assert len(peer.steers(repeated)) == 2
    page.reload(); page.wait_for_function('T.listLoaded')
    open_sdk(page, peer, expect, uid=uid)
    expect(page.locator('.sdk-msgs [data-role=user]').filter(has_text=repeated)).to_have_count(2)
    echo_gate.unlink()
    wait_snapshot(page, uid, lambda raw: sum(item.get('role') == 'user' and item.get('content') == repeated for item in raw['items']) == 2
                  and all(any(item.get('request_id') == ident and item.get('native', {}).get('conversation_id') for item in raw['items']) for ident in ids))
    expect(page.locator('.sdk-msgs [data-role=user]').filter(has_text=repeated)).to_have_count(2)
    peer.release('delayed-echo-' + text.split()[-1])
    wait_snapshot(page, uid, lambda raw: not raw['running'])
    # Whitespace-only chunks must preserve indentation for later text, allow
    # transcript hydration, and leave room for a result-only native reply.
    for prompt, expected in [
        ('__WHITESPACE_REPLY__', ['\n\n    分块回复保留 Markdown 缩进', '从原生记录补齐正文。']),
        ('__WHITESPACE_RESULT__', ['最终回执中的回复补齐正文。']),
    ]:
        request = send_agy(page, prompt)['request_id']
        final = wait_snapshot(page, uid, lambda raw: (raw.get('last_turn') or {}).get('id') == request)
        raw = final['raw']
        assert raw['last_turn']['status'] == 'completed', raw['last_turn']
        replies = [item['content'] for item in raw['items'] if item['role'] == 'assistant'
                   and (item.get('request_id') == request or item['id'] == f'agy:result:{request}')]
        assert replies == expected, replies
        for reply in expected:
            expect(page.locator('.sdk-msgs')).to_contain_text(reply.strip())
    if screenshots:
        for width, theme in [(1280, 'light'), (390, 'dark')]:
            page.set_viewport_size({'width': width, 'height': 900})
            page.emulate_media(color_scheme=theme)
            page.evaluate('() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))')
            if not page.locator('.sdk-pane').is_visible():
                page.locator(f'#side .item[data-uid="{uid}"]').click()
            expect(page.locator('.sdk-pane')).to_be_visible()
            page.locator('.sdk-msgs').evaluate('box => box.scrollTop = box.scrollHeight')
            page.screenshot(path=str(screenshots / f'cancel-{text.split()[-1]}-{width}-{theme}.png'), full_page=True)
    context.close()
    print(text, 'PASS fake CLI transport: open, send, cancel, reconnect, delayed native echoes and identical independent steers', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-agy-history-join-') as tmp, sync_playwright() as pw:
        root = Path(tmp)
        with provider('agy', root, args.binary) as (base, peer, extra):
            browser = pw.chromium.launch(headless=True)
            try:
                exercise(browser, base, peer, 'node', args.screenshots)
                hub_root = root / 'hub'; hub_root.mkdir()
                hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                          [SimpleNamespace(name='JoinNode', token=TOKEN,
                           port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                hub.start()
                try: exercise(browser, f'http://127.0.0.1:{hub.port}', peer, 'hub', args.screenshots)
                finally: hub.stop()
            finally: browser.close()
        transport_root = root / 'transport'; transport_root.mkdir()
        with provider('agy', transport_root, args.binary) as (base, peer, extra):
            browser = pw.chromium.launch(headless=True)
            try:
                exercise_transport(browser, base, peer, args.screenshots)
                hub_root = transport_root / 'hub'; hub_root.mkdir()
                hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                          [SimpleNamespace(name='TransportNode', token=TOKEN,
                           port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                hub.start()
                try: exercise_transport(browser, f'http://127.0.0.1:{hub.port}', peer, args.screenshots)
                finally: hub.stop()
            finally:
                browser.close()
                stop_private_sdkhosts(transport_root)


if __name__ == '__main__':
    main()
