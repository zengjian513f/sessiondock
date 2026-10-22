#!/usr/bin/env python3
"""Six SDKs fold completed history/replay while the current turn keeps streaming."""
import argparse
import copy
from pathlib import Path
import tempfile
from types import SimpleNamespace

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
                     'result': {'role': 'tool_result',
                                'text': '\n'.join(f'检查结果 {line}' for line in range(18))}}])
            history.append({'id': f'final-{turn}', 'role': 'assistant', 'turn_id': identity,
                            'text': f'第 {turn + 1} 轮最终结论：检查已完成。', 'phase': 'final'})
        # One historical round precedes the native snapshot; the snapshot itself
        # replays two completed rounds and an unfinished tail (as Kiro does).
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
            data.update(messages=history, reset=True, start=0, end=len(history),
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
            if screenshots: page.screenshot(path=str(screenshots / f'{name}-{state}.png'), full_page=True)

        def toggle():
            button = page.locator('#a-turns')
            if not button.is_visible(): page.locator('#a-more').click()
            button.click()
            page.keyboard.press('Escape')

        capture('initial')
        if not baseline:
            tools = page.locator('.sdk-pane .tool-disclosable')
            expect(tools.last.locator('.tool-toggle')).to_have_attribute('aria-expanded', 'true')
            expect(tools.last.locator('.tool-out')).to_contain_text('检查结果 17')
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
            expect(tool.locator('.tool-out')).not_to_be_visible()
            tool.get_by_role('button', name='展开工具调用', exact=True).click()
            expect(tool.locator('.tool-out')).to_contain_text('检查结果 17')
            page.evaluate("window.turnProbe = document.querySelector('[data-sdk-item=\"think-3-7\"]')")
            native[-1]['result']['text'] += '\n后台工具更新'
            native.append({'id': 'new-tool', 'role': 'tool', 'turn_id': 'turn-3',
                           'name': 'new-tool', 'text': 'new arguments',
                           'result': {'role': 'tool_result', 'text': '最新工具完整结果'}})
            native[-3]['text'] += '\n当前回合持续生成'
        else:
            raw['items'][0]['text'] += '\n当前回合持续生成'
        events.publish()
        expect(tail).to_contain_text('当前回合持续生成')
        expect(replay.locator('.turn-process-body')).to_be_visible()
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
        context.close()
        print(f'PASS {name}: header/per-turn folding, replay, streaming, completion, reload', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='kiro,codex,claude,grok,agy,opencode')
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
                    if args.baseline: continue
                    hub_root = root / 'hub'; hub_root.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                              [SimpleNamespace(name='FoldNode', token=TOKEN,
                               port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try:
                        exercise(browser, f'http://127.0.0.1:{hub.port}', source, events, args.screenshots, 'hub')
                    finally: hub.stop()
        finally: browser.close()


if __name__ == '__main__':
    main()
