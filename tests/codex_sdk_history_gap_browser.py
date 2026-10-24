#!/usr/bin/env python3
"""Join a full active Codex replay to real paginated history without moving old input."""
import argparse
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from codex_sdk_browser import THREAD, build, server
from codex_sdk_status_browser import StatusDaemon
from history_fixtures import BINARY, codex_row, encoded
from hub_fixtures import Hub, free_port
from node_auth_fixtures import TOKEN, node_env
from sdk_selection_fixtures import snapshots

OLD = '不是，你要进行系统复核。这是很早的输入，必须留在历史缺口里面。'
TURN = 'long-active-turn'


def corpus_rows():
    rows = [codex_row('session_meta', {'id': THREAD, 'cwd': '/synthetic/sdk-gap'})]
    items = []
    def message(role, text, identity, turn):
        rows.append(codex_row('response_item', {'type': 'message', 'role': role,
            'id': identity, 'turn_id': turn, 'phase': 'commentary',
            'content': [{'type': 'input_text' if role == 'user' else 'output_text', 'text': text}]}))
    for i in range(6): message('user' if i == 0 else 'assistant', f'最早一轮记录 {i}', f'early-{i}', 'early-turn')
    message('user', OLD, 'native-user', TURN)
    items.append({'id': 'sdk-user', 'type': 'userMessage', 'content': [{'type': 'text', 'text': OLD}]})
    for i in range(450):
        rows.extend([
            codex_row('response_item', {'type': 'function_call', 'id': f'native-tool-{i}',
                'turn_id': TURN, 'name': 'exec', 'call_id': f'call-{i}', 'arguments': '{"command":"check"}'}),
            codex_row('response_item', {'type': 'function_call_output', 'turn_id': TURN,
                'call_id': f'call-{i}', 'output': f'检查结果 {i}\n' + 'synthetic result\n' * 500})])
        items.append({'id': f'exec-{i}', 'type': 'commandExecution', 'command': f'check-step-{i}',
                      'status': 'completed', 'aggregatedOutput': f'检查结果 {i}\n' * 20})
        if i % 20 == 0:
            # Repeated prose is intentional: identity, not text, locates overlap.
            message('assistant', '继续逐项复核，尚未完成。', f'answer-{i}', TURN)
            items.append({'id': f'answer-{i}', 'type': 'agentMessage', 'text': '继续逐项复核，尚未完成。'})
    message('assistant', '当前尾部进度', 'tail-answer', TURN)
    items.append({'id': 'tail-answer', 'type': 'agentMessage', 'text': '当前尾部进度'})
    rows.append(codex_row('event_msg', {'type': 'task_started', 'turn_id': TURN}))
    return rows, items


def exercise(browser, base, events, items, shots, label):
    for width, theme in [(1440, 'dark'), (390, 'light')]:
        context = browser.new_context(viewport={'width': width, 'height': 960}, color_scheme=theme,
                                      service_workers='block')
        page = context.new_page()
        page.set_default_timeout(20000)
        errors = []
        page.on('pageerror', lambda e: errors.append(str(e)))
        raw = {'turn': {'id': TURN, 'status': 'inProgress'}, 'items': items,
               'requests': [], 'summary': {'settings': {}, 'queue': [], 'status': {'type': 'active'}}}
        events.packet = {'type': 'snapshot', 'seq': 0, 'raw': raw}
        context.route('**/api/codex-sdk/open', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
        context.route('**/api/codex-sdk/events?*', lambda r: r.continue_(url=events.url))
        for endpoint in ('models', 'limits'):
            context.route(f'**/api/codex-sdk/{endpoint}*', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        page.locator('#side .item[data-uid^="codex:"]').first.click()
        if page.locator('.sdk-pane').count() == 0: page.locator('#a-sdk').click()
        expect(page.locator('.sdk-msgs')).to_contain_text('当前尾部进度')
        page.wait_for_function('CodexSdkView.turn !== null')
        expect(page.locator('.sdk-history .history-gap-load')).to_have_count(1)
        expect(page.locator('.sdk-msgs')).not_to_contain_text(OLD)
        expect(page.locator('[data-sdk-item="tail-answer"]')).to_have_count(1)
        checkpoint = page.evaluate('({end:CodexSdkView.historyData.end,head:CodexSdkView.historyData.head})')
        # The native prefix before the first shared ID stays ahead of the replay.
        assert page.evaluate('CodexSdkView.historyData.partial.omitted > 0')
        assert page.evaluate('CodexSdkView.replayOmitted.has("sdk-user")')
        page.locator('.sdk-msgs').hover(); page.mouse.wheel(0, -100000)
        if shots: page.screenshot(path=str(shots/f'{label}-{width}-{theme}-gap.png'), full_page=True)
        # Streaming does not resurrect hidden replay items, even if their body changes.
        events.packet = {'seq': 1, 'method': 'item/agentMessage/delta',
                         'raw': {'itemId': 'tail-answer', 'delta': '，实时更新仍可见'}}
        events.publish()
        expect(page.locator('[data-sdk-item="tail-answer"]')).to_contain_text('实时更新仍可见')
        expect(page.locator('.sdk-msgs')).not_to_contain_text(OLD)
        # Legacy records/snapshots without shared IDs retain native history;
        # a later identifiable snapshot restores the live suffix in order.
        events.packet = {'type': 'snapshot', 'seq': 2, 'raw': {**raw,
            'items': [{**item, 'id': 'unmatched-' + item['id']} for item in items]}}
        events.publish()
        expect(page.locator('.sdk-live .msg')).to_have_count(0)
        expect(page.locator('.sdk-history')).to_contain_text('当前尾部进度')
        expect(page.locator('.sdk-msgs')).not_to_contain_text(OLD)
        events.packet = {'type': 'snapshot', 'seq': 3, 'raw': raw}
        events.publish()
        expect(page.locator('[data-sdk-item="tail-answer"]')).to_have_count(1)
        page.locator('.sdk-history .history-gap-load').click()
        expect(page.locator('.sdk-history .history-gap-load')).to_have_count(0)
        expect(page.locator('.sdk-msgs .msg[data-role="user"]').filter(has_text=OLD)).to_have_count(1)
        assert page.evaluate("""() => !!(document.querySelector('[data-sdk-item="sdk-user"]')
            .compareDocumentPosition(document.querySelector('[data-sdk-item="tail-answer"]'))
            & Node.DOCUMENT_POSITION_FOLLOWING)""")
        assert page.evaluate('({end:CodexSdkView.historyData.end,head:CodexSdkView.historyData.head})') == checkpoint
        page.locator('.sdk-msgs').hover(); page.mouse.wheel(0, -100000)
        if shots: page.screenshot(path=str(shots/f'{label}-{width}-{theme}-loaded.png'), full_page=True)
        events.packet = {'type': 'snapshot', 'seq': 0, 'raw': raw}
        page.reload(wait_until='domcontentloaded')
        expect(page.locator('[data-sdk-item="tail-answer"]')).to_have_count(1)
        expect(page.locator('.sdk-msgs')).not_to_contain_text(OLD)
        assert not errors, errors
        context.close()
        print(f'PASS {label} {width} {theme}: gap/replay ordering, repeated prose, stream, load, checkpoint, reload', flush=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--binary', type=Path, default=BINARY)
    ap.add_argument('--screenshots', type=Path)
    args = ap.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-codex-gap-') as tmp, sync_playwright() as pw:
        root = Path(tmp); corpus = build(root)
        rows, items = corpus_rows()
        corpus.paths['sdk'].write_bytes(b''.join(encoded(row) for row in rows))
        extra = node_env(root, free_port(), '127.0.0.0/8')
        with StatusDaemon(root/'app.sock', corpus.paths['sdk'], THREAD), server(corpus, args.binary, root/'app.sock', extra) as base, snapshots() as events:
            browser = pw.chromium.launch(headless=True)
            try:
                exercise(browser, base, events, items, args.screenshots, 'node')
                hub_root = root/'hub'; hub_root.mkdir()
                hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                          [SimpleNamespace(name='GapNode', token=TOKEN, port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':',1)[1]))])
                hub.start()
                try: exercise(browser, f'http://127.0.0.1:{hub.port}', events, items, args.screenshots, 'hub')
                finally: hub.stop()
            finally: browser.close()


if __name__ == '__main__': main()
