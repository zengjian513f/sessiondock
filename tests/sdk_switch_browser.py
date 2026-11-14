#!/usr/bin/env python3
"""Repeated active-session switches retain the visible preview through discovery."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import re
import tempfile
import threading
import time
from types import SimpleNamespace
from urllib.parse import parse_qs, unquote, urlsplit

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_selection_fixtures import snapshots
from sdk_start_stop_fixtures import provider
from sdkhost_fixtures import stop_private_sdkhosts


def history(index, count):
    rows = []
    for turn in range(count // 20):
        tid = f'switch-{index}-{turn}'
        rows.append({'id': tid, 'role': 'user', 'turn_id': tid,
                     'text': f'会话 {index} 第 {turn} 轮检查'})
        for step in range(18):
            rows.append({'id': f'{tid}-{step}', 'role': 'tool', 'turn_id': tid,
                         'name': 'exec_command', 'summary': f'检查 {step}',
                         'text': '{"command":"inspect synthetic fixture"}',
                         'result': {'role': 'tool_result', 'text': '合成输出\n' * 50}})
        rows.append({'id': f'{tid}-final', 'role': 'assistant', 'turn_id': tid,
                     'phase': 'final', 'text': f'会话 {index} 的历史结论 {turn}\n\n'
                     '| 检查项 | 结果 |\n| --- | --- |\n' + '| 合成数据 | 保留完整记录 |\n' * 12})
        if turn == 0:
            rows[-1]['text'] += '\n公式 $x^2 + 1$\n\n```python\nprint("retained history")\n```'
    return rows


def exercise(browser, base, streams, shots, width, theme):
    context = browser.new_context(viewport={'width': width, 'height': 900},
                                  color_scheme=theme, service_workers='block')
    page = context.new_page(); page.set_default_timeout(15000)
    errors, opens, held, timings, background = [], [], [], [], []
    state = {'hold': False}
    page.on('pageerror', lambda e: errors.append(str(e)))
    page.on('request', lambda r: opens.append(r.url) if r.method == 'POST' and r.url.endswith('/open') else None)
    try:
        catalog = page.request.get(base+'/api/sessions').json()['sessions']
        uids = [row['uid'] for row in catalog if row['source'] in ('codex', 'agy')]
        assert len(uids) == 4, uids
        data = {uid: history(i, n) for i, (uid, n) in enumerate(zip(uids, (360, 40, 240, 560)))}
        packets = {}
        for i, uid in enumerate(uids):
            raw = {'items': [], 'requests': [], 'running': False, 'input_queue': [], 'capabilities': {}}
            if uid.startswith('codex:'):
                raw = {'items': [], 'turn': None, 'requests': [],
                       'summary': {'settings': {}, 'queue': [], 'status': {'type': 'idle'}}}
            streams[i].packet = {'type': 'snapshot', 'seq': 0, 'raw': raw}
            packets[uid] = streams[i].url
        page.route(re.compile(r'/api/live(?:\?|$)'), lambda r: r.fulfill(json={
            'known': True, 'uids': uids, 'sdk_uids': uids, 'tmux_uids': [], 'working_uids': []}))
        def runtime(route):
            if state['hold']: held.append(route)
            else: route.fulfill(json={'known': True, 'sdk': True, 'tmux': False, 'running': True, 'external': False})
        page.route('**/api/session/runtime?*', runtime)
        def messages(route):
            response = route.fetch(); body = response.json()
            uid = unquote(urlsplit(route.request.url).path.rsplit('/', 1)[-1])
            if body.get('reset'):
                body.update(messages=data[uid], partial=None, message_total=len(data[uid]), activity={'state': 'idle'})
            route.fulfill(response=response, json=body)
        page.route('**/api/messages/*?*', messages)
        for api in ('codex-sdk', 'native-sdk'):
            page.route(f'**/api/{api}/events?*', lambda r: r.continue_(
                url=packets[parse_qs(urlsplit(r.request.url).query)['uid'][0]]))
            for name in ('models', 'limits', 'usage', 'agents'):
                page.route(f'**/api/{api}/{name}*', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('S.liveSdk.size === 4 && T.listLoaded')
        page.locator('#livecount').click()
        # Keep inventory refreshes slow across repeated clicks, as on a fleet
        # with an offline node. Runtime ownership must stay independently fresh.
        page.route(re.compile(r'/api/(?:term/list|live)\?force=1(?:&|$)'), lambda r: background.append(r))
        page.evaluate('''() => {
          window.switchTasks=[];
          window.savedHistories = new Map();
          new PerformanceObserver(l=>{for(const x of l.getEntries())switchTasks.push(x.duration)}).observe({type:'longtask'});
        }''')
        profiler = context.new_cdp_session(page)
        profiler.send('Performance.enable')
        def layout_counts():
            return {x['name']: x['value'] for x in profiler.send('Performance.getMetrics')['metrics']
                    if x['name'] in ('LayoutCount', 'RecalcStyleCount')}
        before_layout = layout_counts()
        def select(uid):
            if page.locator('#detail .mobile-back').is_visible(): page.locator('#detail .mobile-back').click()
            began = time.monotonic()
            page.locator(f'#side .item[data-uid="{uid}"]').first.click()
            timings.append(round((time.monotonic()-began)*1000))
        for lap in range(2):
            for i, uid in enumerate(uids):
                select(uid)
                page.wait_for_function('SdkView.connected && !SdkView.discovering')
                expect(page.locator('.sdk-history')).to_contain_text(f'会话 {i} 的历史结论')
                expect(page.locator('#sdk-send')).to_be_enabled()
                if lap == 0:
                    page.evaluate('uid=>savedHistories.set(uid, SdkView.history.firstChild)', uid)
                else:
                    assert page.evaluate('uid=>savedHistories.get(uid)===SdkView.history.firstChild', uid), 'warm switch rebuilt unchanged history'
                    expect(page.locator('.sdk-history .katex').first).to_be_attached()
                    expect(page.locator('.sdk-history code.code-block[data-syntax-done]').first).to_be_attached()
        metrics = {'click_ms': timings.copy(), 'long_tasks_ms': page.evaluate('switchTasks'),
                   'layout': {key: value-before_layout[key] for key, value in layout_counts().items()}}
        (shots/f'{width}-{theme}-switches.json').write_text(json.dumps(metrics))
        print(f'SWITCHES {width}/{theme}: {metrics}', flush=True)
        # Count actual Chromium layouts, not wall time. Before batching header
        # measurements/status disclosure, the desktop run used >450 layouts.
        assert metrics['layout']['LayoutCount'] < 400, metrics
        term_refreshes = [r for r in background if '/term/list' in r.request.url]
        live_refreshes = [r for r in background if '/live?' in r.request.url]
        assert len(term_refreshes) == 1, f'duplicate pending fleet refreshes: {len(term_refreshes)}'
        assert len(live_refreshes) == 4, f'duplicate pending node refreshes: {len(live_refreshes)}'
        for route in background:
            if '/term/list' in route.request.url: route.fulfill(response=route.fetch())
            else: route.fulfill(json={'known': True, 'uids': uids, 'sdk_uids': uids, 'tmux_uids': [], 'working_uids': []})
        background.clear()
        page.unroute(re.compile(r'/api/(?:term/list|live)\?force=1(?:&|$)'))
        # Warm re-entry paints immediately while ownership is unknown. The
        # exact visible editor, its focus, selection and draft must survive.
        state['hold'] = True
        select(uids[0])
        expect(page.locator('.sdk-history')).to_contain_text('会话 0 的历史结论')
        expect(page.locator('#sdk-send')).to_be_disabled()
        page.locator('#sdk-input').fill('正在切换时编辑的草稿')
        page.locator('#sdk-input').press('ArrowLeft')
        page.evaluate('window.previewInput=SdkView.input; window.previewHistory=SdkView.history.firstChild')
        page.screenshot(path=str(shots/f'{width}-{theme}-pending.png'), full_page=True)
        state['hold'] = False
        for route in held:
            route.fulfill(json={'known': True, 'sdk': True, 'tmux': False, 'running': True, 'external': False})
        held.clear()
        page.wait_for_function('SdkView.connected && !SdkView.discovering')
        assert page.evaluate('previewInput === SdkView.input'), 'ownership discovery replaced the visible editor'
        assert page.evaluate('previewHistory === SdkView.history.firstChild'), 'ownership discovery rebuilt visible history'
        expect(page.locator('#sdk-input')).to_be_focused()
        expect(page.locator('#sdk-input')).to_have_value('正在切换时编辑的草稿')
        assert page.locator('#sdk-input').evaluate('e=>e.selectionStart') == len('正在切换时编辑的草稿') - 1
        page.screenshot(path=str(shots/f'{width}-{theme}-ready.png'), full_page=True)
        # Do not wait for previous ownership replies before choosing another row.
        state['hold'] = True
        for uid in uids[1:]: select(uid)
        state['hold'] = False
        for route in reversed(held):
            route.fulfill(json={'known': True, 'sdk': True, 'tmux': False, 'running': True, 'external': False})
        held.clear()
        page.wait_for_function('uid=>SdkView.uid===uid && SdkView.connected', arg=uids[-1])
        expect(page.locator('.sdk-history')).to_contain_text('会话 3 的历史结论')
        expect(page.locator('#sdk-input')).to_have_value('')
        # Receive text/new items while switching without waiting for discovery.
        page.evaluate('''() => {
          window.hiddenComposerReads = 0;
          const height = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollHeight');
          Object.defineProperty(document.querySelector('#cinput'), 'scrollHeight', {
            get() { if (SessionContentView.kind) hiddenComposerReads++; return height.get.call(this); }
          });
        }''')
        for i, uid in enumerate(uids):
            raw = streams[i].packet['raw']
            if uid.startswith('codex:'):
                raw['turn'] = {'id': f'live-{i}', 'status': 'inProgress'}
                raw['items'] = [{'id': f'live-tool-{j}', 'type': 'commandExecution',
                                 'command': f'check {j}', 'status': 'completed',
                                 'aggregatedOutput': '检查结果\n' * 50} for j in range(40)]
                raw['items'].append({'id': 'live-answer', 'type': 'agentMessage', 'text': f'实时会话 {i}: '})
            else:
                raw['running'] = True
                raw['items'] = [{'id': 'live-answer', 'role': 'assistant', 'turn_id': f'live-{i}',
                                 'text': f'实时会话 {i}: '}]
            streams[i].publish()
        stopped = threading.Event()
        def push():
            tick = 0
            while not stopped.wait(.03):
                tick += 1
                for i, uid in enumerate(uids):
                    raw = streams[i].packet['raw']
                    text = f'增量{tick} '
                    if tick % 20 == 0:
                        item = {'id': f'live-answer-{tick}', 'text': f'实时会话 {i} 新消息{tick}: '}
                        item.update({'type': 'agentMessage'} if uid.startswith('codex:')
                                    else {'role': 'assistant', 'turn_id': f'live-{i}'})
                        raw['items'].append(item)
                        event = {'type': 'event', 'method': 'item/started', 'raw': {'item': item}}
                    else:
                        raw['items'][-1]['text'] += text
                        event = {'type': 'event', 'method': 'item/agentMessage/delta',
                                 'raw': {'itemId': raw['items'][-1]['id'], 'delta': text}}
                    streams[i].publish(event if uid.startswith('codex:') else None)
        worker = threading.Thread(target=push, daemon=True); worker.start()
        try:
            for lap in range(3):
                for uid in uids:
                    select(uid)
                    page.wait_for_timeout(60)
            for i, uid in enumerate(uids):
                select(uid)
                expect(page.locator('.sdk-live')).to_contain_text(f'实时会话 {i}')
                expect(page.locator('.sdk-live')).to_contain_text('增量')
                expect(page.locator('#sdk-input')).to_have_value('正在切换时编辑的草稿' if i == 0 else '')
        finally:
            stopped.set(); worker.join()
        for i, uid in enumerate(uids):
            select(uid)
            expect(page.locator('.sdk-live')).to_contain_text(streams[i].packet['raw']['items'][-1]['text'])
        reads = page.evaluate('hiddenComposerReads')
        print(f'STREAM SWITCHES {width}/{theme}: hidden composer height reads={reads}', flush=True)
        assert reads == 0, f'hidden ordinary composer forced layout during SDK updates: {reads}'
        page.screenshot(path=str(shots/f'{width}-{theme}-stream.png'), full_page=True)
        # The ordinary conversation is built lazily when actually selected.
        page.locator('#a-chat').click()
        expect(page.locator('#msgs:visible')).to_contain_text('会话 3 的历史结论')
        editor = page.locator('#cinput')
        editor.fill('切回普通对话后继续编辑\n' * 12)
        page.wait_for_function('document.querySelector("#cinput").getBoundingClientRect().height >= 170')
        editor.fill('保留跨视图草稿')
        page.wait_for_function('document.querySelector("#cinput").getBoundingClientRect().height < 70')
        page.screenshot(path=str(shots/f'{width}-{theme}-chat.png'), full_page=True)
        page.locator('#a-sdk').click()
        page.wait_for_function('SdkView.connected')
        expect(page.locator('.sdk-history')).to_contain_text('会话 3 的历史结论')
        expect(page.locator('#sdk-input')).to_have_value('保留跨视图草稿')
        page.locator('#sdk-input').fill('')
        page.wait_for_function('''() => {
          const draft = composerDrafts.get(composerDraftOwner(S.sel));
          return draft.text === '' && draft.editVersion === draft.savedVersion;
        }''')
        if width == 1440:
            # The runtime request's own timeout must still resolve discovery
            # through the existing unknown-owner fallback, unlike an obsolete
            # reply from a previously selected session.
            state['hold'] = True
            select(uids[0])
            page.wait_for_function('SdkView.discovering')
            page.wait_for_function('SdkView.selection?.mode !== "loading" && !SdkView.discovering', timeout=15000)
            expect(page.locator('.sdk-history:visible, #msgs:visible')).to_contain_text('会话 0 的历史结论')
        assert not opens, opens
        assert not errors, errors
        metrics = {'click_ms': timings, 'long_tasks_ms': page.evaluate('switchTasks')}
        (shots/f'{width}-{theme}.json').write_text(json.dumps(metrics))
        print(f'PASS {width}/{theme}: repeated four-session switches, preview/focus/draft retained, '
              f'late replies isolated; click_ms={timings}', flush=True)
    finally:
        for route in held: route.abort()
        for route in background: route.abort()
        page.unroute_all(behavior='ignoreErrors')
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path, default=Path(tempfile.gettempdir())/'sd-sdk-switch')
    args = parser.parse_args(); args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-sdk-switch-') as tmp, ExitStack() as stack, sync_playwright() as pw:
        root = Path(tmp); nodes=[]; streams=[]
        for i, source in enumerate(('codex', 'codex', 'codex', 'agy')):
            node = root/str(i); node.mkdir()
            _, _, env = stack.enter_context(provider(source, node, args.binary))
            stack.callback(stop_private_sdkhosts, node)
            nodes.append(SimpleNamespace(name=f'Switch{i}', token=TOKEN,
                         port=int(env['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1])))
            streams.append(stack.enter_context(snapshots()))
        hub_root = root/'hub'; hub_root.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root, nodes); hub.start()
        browser = pw.chromium.launch(headless=True)
        try:
            for width, theme in ((1440, 'light'), (390, 'dark')):
                exercise(browser, f'http://127.0.0.1:{hub.port}', streams, args.screenshots, width, theme)
        finally:
            browser.close(); hub.stop()


if __name__ == '__main__': main()
