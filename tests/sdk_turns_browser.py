#!/usr/bin/env python3
"""Six SDKs fold completed history/replay while the current turn keeps streaming."""
import argparse
import copy
from datetime import datetime, timedelta, timezone
from pathlib import Path
import re
import tempfile
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from sdk_mode_fixtures import provider
from sdk_selection_fixtures import snapshots


def exercise(browser, base, source, events, screenshots, label, baseline=False):
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        context = browser.new_context(viewport={'width': width, 'height': 900},
                                      color_scheme=theme, service_workers='block')
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        history = []
        for turn in range(4):
            identity = f'user-{turn}' if source == 'kiro' else f'turn-{turn}'
            history.append({'id': f'user-{turn}', 'role': 'user', 'turn_id': identity,
                'native_id': identity, 'prompt_index': turn, 'native': {'step_index': turn},
                'text': f'第 {turn + 1} 轮：检查会话历史、工具输出与最终结论。'})
            for index in range(8):
                history.extend([
                    {'id': f'think-{turn}-{index}', 'role': 'thinking', 'turn_id': identity,
                     'text': f'第 {turn + 1} 轮分析 {index + 1}：逐项核对输入、执行过程和结果。\n' * 4},
                    {'id': f'tool-{turn}-{index}', 'role': 'tool', 'turn_id': identity,
                     'name': 'exec_command', 'text': '{"command": "check fixture"}', 'input': {'command': f'check-step-{index}'},
                     'summary': f'$ check-step-{index}',
                     'result': {'role': 'tool_result', 'error': index == 0,
                                'text': '\n'.join(f'检查结果 {line}' for line in range(18))}}])
            history.append({'id': f'final-{turn}', 'role': 'assistant', 'turn_id': identity,
                            'text': f'第 {turn + 1} 轮最终结论：检查已完成。', 'phase': 'final'})
        # One historical round precedes the native snapshot; the snapshot itself
        # replays two completed rounds and an unfinished tail (as Kiro does).
        for index, item in enumerate(history):
            item['ts'] = (datetime(2026, 11, 5, tzinfo=timezone.utc)
                          + timedelta(minutes=6 * index)).isoformat()
        native = copy.deepcopy(history[18:-1])
        if source == 'kiro':
            # ACP replay has item IDs rather than history's normalized turn IDs.
            for item in native:
                for key in ('turn_id', 'native_id', 'prompt_index', 'native'):
                    item.pop(key, None)
        history = history[:-1]
        raw = {'items': native, 'requests': [], 'running': True, 'input_queue': [],
               'model': 'synthetic-model', 'capabilities': {'interrupt': True}}
        if source == 'codex':
            raw = {'items': [{'id': 'live-answer', 'type': 'agentMessage', 'text': '当前回合持续生成'},
                             {'id': 'live-tool', 'type': 'commandExecution', 'command': 'check fixture',
                              'status': 'completed', 'aggregatedOutput': '\n'.join(f'检查结果 {line}' for line in range(18))}],
                   'turn': {'id': 'turn-3', 'status': 'inProgress'}, 'requests': [],
                   'summary': {'settings': {}, 'queue': [], 'status': {'type': 'active'}}}
        events.packet = {'type': 'snapshot', 'seq': 0, 'raw': raw}
        api = 'codex-sdk' if source == 'codex' else 'native-sdk'
        context.route(f'**/api/{api}/open', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
        context.route(f'**/api/{api}/events?*', lambda r: r.continue_(url=events.url))
        for endpoint in ('models', 'limits', 'usage', 'agents'):
            context.route(f'**/api/{api}/{endpoint}*', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))

        def messages(route):
            response = route.fetch()
            data = response.json()
            start = parse_qs(urlsplit(route.request.url).query).get('start', [''])[0]
            current = start == str(len(history))
            data.update(messages=[] if current else history, reset=not current,
                        start=len(history) if current else 0, end=len(history),
                        message_total=len(history), partial=None, activity={'state': 'working'})
            route.fulfill(response=response, json=data)
        context.route('**/api/messages/**', messages)
        context.route('**/api/watch?*', lambda r: r.fulfill(
            content_type='text/event-stream', body=': synthetic history\n\n'))
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible(): page.locator(f'#chips button[data-source="{source}"]').click()
        row.click()
        page.locator('#a-sdk').click()
        expect(page.locator('.sdk-history')).to_contain_text('第 1 轮最终结论')
        power = page.locator('#a-session-toggle')
        if page.evaluate('SdkView.readOnly'):
            if not power.is_visible(): page.locator('#a-more').click()
            power.click()
        expect(page.locator('#sdk-stop')).to_be_visible()
        name = f'{label}-{source}-{width}-{theme}'

        def capture(state):
            if not baseline:
                expect(page.locator('.message-time-divider')).to_have_count(0)
            if screenshots: page.screenshot(path=str(screenshots / f'{name}-{state}.png'), full_page=True)

        def toggle():
            button = page.locator('#a-turns')
            if not button.is_visible(): page.locator('#a-more').click()
            button.click()
            page.keyboard.press('Escape')

        capture('initial')
        if not baseline:
            tools = page.locator('.sdk-pane .tool-entry')
            expect(tools.last.locator('.tool-toggle')).to_have_attribute('aria-expanded', 'false')
            expect(tools.last.locator('.tool-out')).to_contain_text('检查结果 7')
            if source != 'codex':
                expect(tools.first.locator('.tool-toggle')).to_have_attribute('aria-expanded', 'false')
        groups = page.locator('.sdk-pane .turn-process')
        if baseline:
            toggle()
            capture('expanded')
            print('BASELINE', name, 'process groups', groups.count(), flush=True)
            context.close()
            return
        expect(groups).to_have_count(3)
        expect(page.locator('.sdk-pane .turn-process.has-error')).to_have_count(0)
        expect(groups.first.locator('.turn-stats')).to_contain_text('调用报错 1')
        assert groups.first.locator('.turn-preview').evaluate(
            "node => { const probe = document.createElement('span'); probe.style.border = '1px solid var(--border)'; node.append(probe); const same = getComputedStyle(node).borderTopColor === getComputedStyle(probe).borderTopColor; probe.remove(); return same; }")

        expect(page.locator('.sdk-pane .turn-process:not(.folded)')).to_have_count(0)
        toggle()
        expect(page.locator('.sdk-pane .turn-process:not(.folded)')).to_have_count(3)
        replay = groups.nth(1)
        page.locator('.sdk-msgs').hover()
        page.mouse.wheel(0, -100000)
        replay.locator('.fold-toggle').first.hover()
        capture('expanded')
        toggle()
        expect(page.locator('.sdk-pane .turn-process:not(.folded)')).to_have_count(0)
        replay.hover()
        capture('collapsed')
        # A per-turn choice and expanded tool survive streaming snapshots, and
        # the active tail remains visible regardless of the global preference.
        replay.locator('.fold-toggle').first.click()
        expect(replay.locator('.turn-process-body')).to_be_visible()
        tail = page.locator('[data-sdk-item="live-answer"]' if source == 'codex'
                            else '[data-sdk-item="think-3-7"]')
        expect(tail).to_be_visible()
        if source != 'codex':
            tool = page.locator('[data-sdk-item="tool-1-0"]')
            tool.locator('xpath=ancestor::div[contains(@class,"sdk-tool-group")]').locator(':scope > .group-preview .fold-toggle').click()
            expect(tool.locator('.tool-out')).to_be_visible()
            tool.get_by_text('展开全文', exact=False).click()
            tool.get_by_role('button', name='展开原始参数', exact=True).click()
            expect(tool.locator('.tool-out')).to_contain_text('检查结果 7')
            page.evaluate("window.turnProbe = document.querySelector('[data-sdk-item=\"think-3-7\"]')")
            native[-1]['result']['text'] += '\n后台工具更新'
            native.append({'id': 'new-tool', 'role': 'tool', 'turn_id': 'turn-3',
                           'name': 'new-tool', 'text': 'new arguments', 'status': 'in_progress',
                           'result': {'role': 'tool_result', 'text': '最新工具完整结果'}})
            native[-3]['text'] += '\n当前回合持续生成'
        else:
            raw['items'][0]['text'] += '\n当前回合持续生成'
        page.evaluate('window.historyProbe = document.querySelector(".sdk-history .msg")')
        events.publish()
        expect(tail).to_contain_text('当前回合持续生成')
        expect(replay.locator('.turn-process-body')).to_be_visible()
        assert page.evaluate('historyProbe === document.querySelector(".sdk-history .msg")'), name
        if source != 'codex':
            expect(tool.locator('.tool-args')).to_be_visible()
            expect(page.locator('[data-sdk-item="tool-3-7"] .tool-toggle')).to_have_attribute('aria-expanded', 'false')
            expect(page.locator('[data-sdk-item="new-tool"] .tool-out')).to_be_visible()
            capture('tool-expanded')
            assert page.evaluate("window.turnProbe === document.querySelector('[data-sdk-item=\"think-3-7\"]')")
        history.append({'id': 'final-3', 'role': 'assistant', 'turn_id': 'turn-3',
                        'text': '第 4 轮最终结论：当前回合已经完成。', 'phase': 'final'})
        if source == 'codex':
            events.packet = {'method': 'turn/completed', 'seq': events.packet['seq'],
                             'raw': {'turn': {'id': 'turn-3', 'status': 'completed'}}}
        else:
            native.append(history[-1])
            raw['running'] = False
            raw['last_turn'] = {'status': 'completed'}
        events.publish()
        expect(page.locator('.sdk-msgs')).to_contain_text('第 4 轮最终结论')
        expect(groups).to_have_count(4)
        assert page.evaluate('historyProbe === document.querySelector(".sdk-history .msg")'), name
        expect(replay.locator('.turn-process-body')).to_be_visible()
        toggle()  # expand all, including the newly completed tail
        expect(page.locator('.sdk-pane .turn-process:not(.folded)')).to_have_count(4)
        toggle()
        expect(page.locator('.sdk-pane .turn-process:not(.folded)')).to_have_count(0)
        groups.last.hover()
        capture('completed')
        # Reload preserves the global choice when reopening the SDK history.
        raw.pop('turn', None)
        if source == 'codex': raw['items'] = []
        events.packet = {'type': 'snapshot', 'seq': 0, 'raw': raw}
        page.reload(wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        page.locator('#a-sdk').click()
        expect(page.locator('.sdk-msgs')).to_contain_text('第 4 轮最终结论')
        expect(groups).to_have_count(4)
        expect(page.locator('.sdk-pane .turn-process:not(.folded)')).to_have_count(0)
        assert not errors, errors
        context.unroute_all(behavior='wait')
        context.close()
        print(f'PASS {name}: header/per-turn folding, replay, streaming, completion, reload', flush=True)


def exercise_connecting_active_tail(browser, base, screenshots, label):
    """BUG-20261110-162848-105d2c: an active Native SDK session with an unfinished
    tail turn (and optional-check tool errors) must stay expanded while connecting,
    keep Agy live tool raw args folded behind 1-line summaries, and fold cleanly
    only after the turn completes."""
    with snapshots(replay=False) as delayed:
        for width, theme in [(1280, 'light'), (390, 'dark')]:
            context = browser.new_context(viewport={'width': width, 'height': 900},
                                          color_scheme=theme, service_workers='block')
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            t0 = datetime(2026, 11, 10, 16, 10, tzinfo=timezone.utc)
            history = [
                {'id': 'hist-user-0', 'role': 'user', 'turn_id': 'turn-0',
                 'native_id': 'agy:sid:step:0', 'native': {'step_index': 0},
                 'text': '上一轮已完成请求：核对配置与运行状态。', 'ts': t0.isoformat()},
                {'id': 'hist-think-0', 'role': 'thinking', 'turn_id': 'turn-0',
                 'text': '先检查运行目录与服务状态。', 'ts': (t0 + timedelta(seconds=10)).isoformat()},
                {'id': 'hist-tool-0', 'role': 'tool', 'turn_id': 'turn-0',
                 'name': 'run_command', 'text': '{"CommandLine": "hostname"}',
                 'summary': '$ hostname',
                 'result': {'role': 'tool_result', 'text': 'lyra'},
                 'ts': (t0 + timedelta(seconds=15)).isoformat()},
                {'id': 'hist-final-0', 'role': 'assistant', 'turn_id': 'turn-0',
                 'text': '上一轮最终结论：环境检查通过。', 'phase': 'final',
                 'ts': (t0 + timedelta(seconds=25)).isoformat()},
                {'id': 'active-user', 'role': 'user', 'turn_id': 'turn-1',
                 'native_id': 'agy:sid:step:4', 'native': {'step_index': 4},
                 'text': '正在处理的当前回合：检查可选规则并定位缺陷根因。',
                 'ts': (t0 + timedelta(minutes=2)).isoformat()},
                {'id': 'active-think-1', 'role': 'thinking', 'turn_id': 'turn-1',
                 'text': '先按顺序读取本机可选规则文件，不存在则跳过。',
                 'ts': (t0 + timedelta(minutes=2, seconds=5)).isoformat()},
                {'id': 'active-tool-err', 'role': 'tool', 'turn_id': 'turn-1',
                 'name': 'read_file', 'text': '{"Path": "/lyra-share/agent-rules/agents/agy.md"}',
                 'summary': '读 /lyra-share/agent-rules/agents/agy.md',
                 'result': {'role': 'tool_result',
                            'text': 'Error reading file: /lyra-share/agent-rules/agents/agy.md does not exist.',
                            'error': True},
                 'ts': (t0 + timedelta(minutes=2, seconds=8)).isoformat()},
                {'id': 'active-think-2', 'role': 'thinking', 'turn_id': 'turn-1',
                 'text': '继续分析 NativeSdkView.paintHistory 的尾回合折叠条件。',
                 'ts': (t0 + timedelta(minutes=2, seconds=20)).isoformat()},
                {'id': 'active-tool-ok', 'role': 'tool', 'turn_id': 'turn-1',
                 'name': 'run_command', 'text': '{"CommandLine": "rg -n paintHistory web/"}',
                 'summary': '$ rg -n paintHistory web/',
                 'result': {'role': 'tool_result', 'text': 'web/native-sdk.js:438:  paintHistory() {'},
                 'ts': (t0 + timedelta(minutes=2, seconds=30)).isoformat()},
            ]
            live_items = [
                {'id': 'agy:request:req-1', 'role': 'user', 'content': history[4]['text'],
                 'request_id': 'req-1', 'native_id': 'agy:sid:step:4',
                 'native': {'step_index': 4}, 'status': 'accepted'},
                {'id': 'agy:sid:step:5:thinking', 'role': 'thinking',
                 'content': history[5]['text'], 'request_id': 'req-1',
                 'native_type': 'PLANNER_RESPONSE'},
                {'id': 'agy:sid:step:5', 'role': 'tool', 'native_type': 'tool',
                 'request_id': 'req-1', 'status': 'ERROR', 'name': 'read_file',
                 'input': {'Path': '/lyra-share/agent-rules/agents/agy.md'},
                 'error': True,
                 'result': '{"type":"TOOL_ERROR","message":"Error reading file: /lyra-share/agent-rules/agents/agy.md does not exist."}',
                 'content': '{"name":"read_file"}'},
                {'id': 'agy:sid:step:6', 'role': 'system', 'native_type': 'checkpoint',
                 'request_id': 'req-1', 'status': 'DONE',
                 'content': '{"conversation_id":"sid","step_index":6,"step_type":"checkpoint"}'},
                {'id': 'agy:sid:step:7:thinking', 'role': 'thinking',
                 'content': history[7]['text'], 'request_id': 'req-1',
                 'native_type': 'PLANNER_RESPONSE'},
                {'id': 'agy:sid:step:7', 'role': 'tool', 'native_type': 'tool',
                 'request_id': 'req-1', 'status': 'ACTIVE', 'name': 'run_command',
                 'input': {'CommandLine': 'rg -n paintHistory web/', 'WaitMsBeforeAsync': 5000},
                 'content': '{"name":"run_command"}'},
            ]
            raw = {'items': live_items, 'requests': [], 'running': True, 'input_queue': [],
                   'model': 'synthetic-agy', 'capabilities': {'interrupt': True}}
            delayed.packet = {'type': 'snapshot', 'seq': 0, 'raw': raw}
            context.route('**/api/session/runtime?*', lambda r: r.fulfill(
                json={'known': True, 'live': True, 'sdk': True, 'tmux': False, 'external': False}))
            context.route('**/api/native-sdk/open', lambda r: r.fulfill(
                json={'ok': True, 'capabilities': {'interrupt': True}, 'raw': {}}))
            context.route('**/api/native-sdk/events?*', lambda r: r.continue_(url=delayed.url))
            for endpoint in ('models', 'limits', 'usage', 'agents'):
                context.route(f'**/api/native-sdk/{endpoint}*', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))

            def messages(route):
                response = route.fetch()
                data = response.json()
                start = parse_qs(urlsplit(route.request.url).query).get('start', [''])[0]
                current = start == str(len(history))
                data.update(messages=[] if current else history, reset=not current,
                            start=len(history) if current else 0, end=len(history),
                            message_total=len(history), partial=None, activity=None)
                route.fulfill(response=response, json=data)
            context.route('**/api/messages/**', messages)
            context.route('**/api/watch?*', lambda r: r.fulfill(
                content_type='text/event-stream', body=': synthetic history\n\n'))

            page.goto(base, wait_until='domcontentloaded')
            page.wait_for_function('T.listLoaded')
            row = page.locator('#side .item[data-uid^="agy:"]').first
            if not row.is_visible(): page.locator('#chips button[data-source="agy"]').click()
            row.click()
            name = f'{label}-agy-connecting-{width}-{theme}'

            # 1) Connecting before first SSE snapshot: completed turn-0 is folded,
            #    while unfinished turn-1 is ALREADY expanded (never folded into a
            #    red-bordered .turn-process.has-error pill).
            expect(page.locator('.native-sdk-pane .sdk-status')).to_contain_text('载入中')
            expect(page.locator('.sdk-history')).to_contain_text('上一轮最终结论')
            expect(page.locator('.sdk-history')).to_contain_text('继续分析 NativeSdkView.paintHistory')
            expect(page.locator('.sdk-pane .turn-process')).to_have_count(1)
            expect(page.locator('.sdk-pane .turn-process.has-error')).to_have_count(0)
            assert not page.evaluate('NativeSdkView.connected')
            if screenshots:
                page.screenshot(path=str(screenshots / f'{name}-1-connecting.png'), full_page=True)

            # 2) Live SSE snapshot arrives (running=True): active turn seamlessly
            #    moves to .sdk-live, remains expanded, hides checkpoint JSON, and
            #    folds Agy tool raw parameters behind 1-line summaries.
            delayed.publish()
            expect(page.locator('#sdk-stop')).to_be_visible()
            expect(page.locator('.native-sdk-pane .sdk-status')).to_contain_text('处理中')
            expect(page.locator('.sdk-pane .turn-process')).to_have_count(1)
            expect(page.locator('.sdk-pane .turn-process.has-error')).to_have_count(0)
            expect(page.locator('.sdk-live')).not_to_contain_text('"step_type"')
            active_cmd = page.locator('[data-sdk-item="agy:sid:step:7"]')
            expect(active_cmd).to_contain_text('$ rg -n paintHistory web/')
            expect(active_cmd).to_contain_text('运行中…')
            expect(active_cmd.locator('.tool-toggle')).to_have_attribute('aria-expanded', 'false')
            err_read = page.locator('[data-sdk-item="agy:sid:step:5"]')
            # Unpack the grouped tool container if grouped so we can inspect/toggle raw args.
            if not err_read.is_visible():
                err_read.locator('xpath=ancestor::div[contains(@class,"sdk-tool-group")]').locator(':scope > .group-preview .fold-toggle').click()
            expect(err_read).to_contain_text('读 /lyra-share/agent-rules/agents/agy.md')
            expect(err_read.locator('.tool-out.err')).to_contain_text('does not exist')
            expect(err_read.locator('.tool-out.err')).not_to_contain_text('TOOL_ERROR')
            expect(err_read.locator('.tool-toggle')).to_have_attribute('aria-expanded', 'false')
            err_read.get_by_role('button', name='展开原始参数', exact=True).click()
            expect(err_read.locator('.tool-args')).to_be_visible()
            expect(err_read.locator('.tool-args')).to_contain_text('/lyra-share/agent-rules/agents/agy.md')
            err_read.get_by_role('button', name='收起原始参数', exact=True).click()
            expect(err_read.locator('.tool-toggle')).to_have_attribute('aria-expanded', 'false')
            if screenshots:
                page.screenshot(path=str(screenshots / f'{name}-2-streaming.png'), full_page=True)

            # 3) Turn finishes (phase='final', running=False): turn-1 now folds
            #    cleanly above its final conclusion, and toggles open/closed freely.
            live_items[-1]['status'] = 'DONE'
            live_items[-1]['output'] = 'web/native-sdk.js:438:  paintHistory() {'
            live_items[-1]['result'] = 'web/native-sdk.js:438:  paintHistory() {'
            live_items.append({
                'id': 'agy:sid:step:8', 'role': 'assistant', 'native_type': 'agent_response',
                'request_id': 'req-1', 'status': 'DONE', 'phase': 'final',
                'content': '当前回合最终结论：连接阶段不再把未完成尾回合误折成红框胶囊。',
            })
            raw['running'] = False
            raw['last_turn'] = {'status': 'completed'}
            delayed.publish()
            expect(page.locator('.sdk-live')).to_contain_text('当前回合最终结论：连接阶段不再把未完成尾回合误折成红框胶囊。')
            expect(page.locator('.sdk-pane .turn-process')).to_have_count(2)
            expect(page.locator('.sdk-pane .turn-process.folded')).to_have_count(2)
            tail_group = page.locator('.sdk-live .turn-process')
            expect(tail_group).not_to_have_class(re.compile(r'\bhas-error\b'))
            expect(tail_group.locator('.turn-stats')).to_contain_text('调用报错 1')
            if screenshots:
                page.screenshot(path=str(screenshots / f'{name}-3-completed-folded.png'), full_page=True)
            tail_group.locator('.fold-toggle').first.click()
            expect(tail_group.locator('.turn-process-body')).to_be_visible()
            if screenshots:
                page.screenshot(path=str(screenshots / f'{name}-4-completed-expanded.png'), full_page=True)
            tail_group.locator('.fold-toggle').first.click()
            expect(page.locator('.sdk-pane .turn-process.folded')).to_have_count(2)
            assert not errors, errors
            context.unroute_all(behavior='wait')
            context.close()
            print(f'PASS {name}: connecting tail stays expanded, Agy live tools summarized, completed turn folds/toggles', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='kiro,codex,claude,grok,agy,opencode,codebuddy,droid')
    parser.add_argument('--screenshots', type=Path)
    parser.add_argument('--baseline', action='store_true')
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-turns-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(tmp) / source; root.mkdir()
                with provider(source, root, args.binary) as (base, _, extra), snapshots() as events:
                    exercise(browser, base, source, events, args.screenshots, 'node', args.baseline)
                    if not args.baseline and source == 'agy':
                        exercise_connecting_active_tail(browser, base, args.screenshots, 'node')
                    if args.baseline: continue
                    hub_root = root / 'hub'; hub_root.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                              [SimpleNamespace(name='FoldNode', token=TOKEN,
                               port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', source, events, args.screenshots, 'hub')
                        if source == 'agy':
                            exercise_connecting_active_tail(browser, f'http://127.0.0.1:{hub.port}', args.screenshots, 'hub')
                    finally: hub.stop()
        finally: browser.close()


if __name__ == '__main__':
    main()
