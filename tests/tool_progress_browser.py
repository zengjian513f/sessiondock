#!/usr/bin/env python3
"""Continuous tool runs keep the latest result visible, over ordinary and SDK SSE."""
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


def exercise(browser, base, source, events, watch, shots, label, mode):
    for width, theme in [(1280, 'light'), (390, 'dark')]:
        context = browser.new_context(viewport={'width': width, 'height': 900}, color_scheme=theme,
                                      service_workers='block')
        page = context.new_page()
        page.set_default_timeout(10000)
        errors = []
        page.on('pageerror', lambda e: errors.append(str(e)))
        output = '\n'.join(f'输出第 {i} 行：检查工具分组与执行结果。' for i in range(18))
        commands = [
            "cat /tmp/tool-push.log; git status --short; rg -n 'lock|flock' deploy/deploy.py | head -15",
            'timeout 420 python3 deploy/deploy.py deploy --web-only --targets node-a,node-b,hub --test none',
            'tail -45 /tmp/tool-deploy.log', 'git diff --check', 'python3 tests/check_docs_links.py',
            'python3 tests/tool_progress_browser.py --sources codex,claude,grok,agy,opencode,kiro',
        ]
        items = [{'id': 'prompt', 'role': 'user', 'turn_id': 'turn', 'text': '检查连续工具调用。'}]
        for i in range(6):
            items.append({'id': f'tool-{i}', 'call_id': f'call-{i}', 'role': 'tool', 'turn_id': 'turn',
                          'name': 'exec_command', 'summary': '$ ' + commands[i],
                          'text': commands[i],
                          'status': 'in_progress' if i == 5 else 'completed',
                          'result': {'role': 'tool_result', 'text': output}})
        history = copy.deepcopy(items) if mode == 'chat' else []
        raw = {'items': items, 'running': True, 'requests': [], 'input_queue': [],
               'model': 'synthetic-model', 'capabilities': {'interrupt': True}}
        if source == 'codex':
            raw = {'items': [{'id': f'tool-{i}', 'type': 'commandExecution', 'command': commands[i],
                             'status': 'inProgress' if i == 5 else 'completed', 'aggregatedOutput': output}
                            for i in range(6)],
                   'turn': {'id': 'turn', 'status': 'inProgress'}, 'requests': [],
                   'summary': {'settings': {}, 'queue': [], 'status': {'type': 'active'}}}
        events.packet = {'type': 'snapshot', 'seq': 0, 'raw': raw}
        watch.packet = {'seq': 0}
        api = 'codex-sdk' if source == 'codex' else 'native-sdk'
        context.route(f'**/api/{api}/open', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
        context.route(f'**/api/{api}/events?*', lambda r: r.continue_(url=events.url))
        for endpoint in ('models', 'limits', 'usage', 'agents'):
            context.route(f'**/api/{api}/{endpoint}*', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
        def messages(route):
            response = route.fetch()
            data = response.json()
            data.update(messages=copy.deepcopy(history), reset=True, start=0, end=len(history),
                        message_total=len(history), partial=None, activity={'state': 'working'})
            watch.packet = {**data, 'seq': watch.packet['seq']}
            route.fulfill(response=response, json=data)
        context.route('**/api/messages/**', messages)
        context.route('**/api/watch?*', lambda r: r.continue_(url=watch.url))
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        row = page.locator(f'#side .item[data-uid^="{source}:"]').first
        if not row.is_visible(): page.locator(f'#chips button[data-source="{source}"]').click()
        row.click()
        page.locator('#a-sdk' if mode == 'sdk' else '#a-chat').click()
        if mode == 'sdk':
            if page.evaluate('SdkView.readOnly'):
                power = page.locator('#a-session-toggle')
                if not power.is_visible(): page.locator('#a-more').click()
                power.click()
            scope = '.sdk-live'
        else:
            scope = '#msgs'
        group = page.locator(scope + ' .grp').last
        expect(group).to_be_visible()
        expect(group.locator('.group-count')).to_have_text('🔧 ×5')
        expect(group.locator('.group-rest')).to_have_text('… 另有 2 项')
        expect(group.locator('.tool-active-tail .tool-out')).to_be_visible()
        expect(group.locator('.tool-active-tail .tool-out')).to_contain_text('输出第 7 行')
        expect(group.locator('.tool-out:visible')).to_have_count(1)
        name = f'{label}-{source}-{mode}-{width}-{theme}'
        def capture(state):
            if shots: page.screenshot(path=str(shots / f'{name}-{state}.png'), full_page=True)
        capture('running')
        stable = [group.element_handle(), group.locator('.group-preview').element_handle(),
                  group.locator('.group-outline code').first.element_handle()] if mode == 'sdk' else []
        def unchanged_summary():
            for handle, selector in zip(stable, ['', ' .group-preview', ' .group-outline code']):
                assert handle.evaluate('(node, selector) => node.isConnected && '
                    'document.querySelector(selector) === node', scope + ' .grp' + selector), \
                    'stream update replaced the group or an unchanged highlighted summary'

        # Completion must leave the last call outside the collapsed summary.
        if mode == 'chat':
            history[-1]['status'] = 'completed'
            watch.packet.update(messages=copy.deepcopy(history), end=len(history) + 1)
            watch.publish()
        else:
            raw['items'][-1]['status'] = 'completed'
            events.publish()
        expect(group.locator('.group-count')).to_have_text('🔧 ×5')
        expect(group.locator('.tool-active-tail')).to_have_count(1)
        expect(group.locator('.tool-out:visible')).to_have_count(1)
        unchanged_summary()
        capture('completed')
        # Mouse and keyboard inspection remain available after automatic folding.
        group.locator(':scope > .group-preview .fold-toggle').click()
        first = group.locator('.tool-entry').first
        expect(group.locator('.tool-out:visible')).to_have_count(6)
        expect(first.locator('.tool-out')).to_be_visible()
        first.get_by_text('展开全文', exact=False).click()
        first.locator('.tool-toggle').click()
        expect(first.locator('.tool-args')).to_be_visible()
        expect(first.locator('.tool-out')).to_contain_text('输出第 17 行')
        if mode == 'sdk':
            target = raw['items'][0] if source == 'codex' else raw['items'][1]
            if source == 'codex': target['aggregatedOutput'] += '\n手动查看保持展开'
            else: target['result']['text'] += '\n手动查看保持展开'
            events.publish()
            expect(first.locator('.tool-out')).to_contain_text('手动查看保持展开')
            expect(first.locator('.tool-out')).to_be_visible()
        first.locator('.tool-toggle').press('Enter')
        expect(first.locator('.tool-args')).not_to_be_visible()
        expect(first.locator('.tool-out')).to_be_visible()
        capture('manual')
        group.locator(':scope > .disclosure').click()
        expect(group.locator('.tool-out:visible')).to_have_count(1)
        # A new running call appears below the existing six-item summary.
        new = {'id': 'tool-6', 'call_id': 'call-6', 'role': 'tool', 'turn_id': 'turn',
               'name': 'exec_command', 'summary': '$ next-step', 'text': 'next-step',
               'status': 'in_progress', 'result': {'role': 'tool_result', 'text': '新的执行输出'}}
        if mode == 'chat':
            history.append(new)
            watch.packet.update(messages=copy.deepcopy(history), end=len(history) + 2)
            watch.publish()
        else:
            raw['items'].append({'id': 'tool-6', 'type': 'commandExecution', 'command': 'next-step',
                'status': 'inProgress', 'aggregatedOutput': '新的执行输出'} if source == 'codex' else new)
            events.publish()
        expect(group.locator('.group-count')).to_have_text('🔧 ×6')
        expect(group.locator('.tool-active-tail .tool-out')).to_have_text('新的执行输出')
        unchanged_summary()
        capture('next')
        # Any later message closes the exposed tool, even while it is running.
        following_role = 'assistant' if width == 1280 else ('user' if mode == 'sdk' else 'thinking')
        following = {'id': 'following', 'role': following_role, 'turn_id': 'turn',
                     'text': '工具后面的新消息', 'phase': 'commentary'}
        if mode == 'chat':
            history.append(following)
            watch.packet.update(messages=copy.deepcopy(history), end=len(history) + 3)
            watch.publish()
        else:
            codex_following = {'id': 'following', 'type': 'agentMessage', 'text': following['text']}
            if following_role == 'user':
                codex_following = {'id': 'following', 'type': 'userMessage',
                                   'content': [{'type': 'text', 'text': following['text']}]}
            raw['items'].append(codex_following if source == 'codex' else following)
            events.publish()
        expect(page.locator(scope)).to_contain_text(following['text'])
        expect(group.locator('.group-count')).to_have_text('🔧 ×7')
        expect(group.locator('.tool-active-tail')).to_have_count(0)
        expect(group.locator('.tool-out:visible')).to_have_count(0)
        unchanged_summary()
        capture('following-message')
        if following_role != 'user':
            group.locator(':scope > .group-preview .fold-toggle').click()
            expect(group.locator('.tool-out:visible')).to_have_count(7)
            capture('following-expanded')
        # A standalone command uses the same group disclosure after any later message.
        single = {'id': 'single-tool', 'call_id': 'single-call', 'role': 'tool', 'turn_id': 'single-turn',
                  'name': 'exec_command', 'summary': '$ python status.py', 'text': 'python status.py',
                  'status': 'completed', 'result': {'role': 'tool_result', 'text': output}}
        after_single = {'id': 'after-single', 'role': 'assistant', 'turn_id': 'single-turn',
                        'text': '单次检查完成，继续核对结果。', 'phase': 'commentary'}
        def publish_single(followed):
            if mode == 'chat': page.wait_for_function('_es && _es.readyState === EventSource.OPEN')
            added = [single, after_single] if followed else [single]
            if mode == 'chat':
                history[:] = [item for item in history if item['id'] not in ('single-tool', 'after-single')] + added
                watch.packet.update(messages=copy.deepcopy(history), end=len(history) + 10)
                watch.publish()
            else:
                if source == 'codex':
                    added = [{'id': 'single-tool', 'type': 'commandExecution', 'command': 'python status.py',
                              'status': 'completed', 'aggregatedOutput': output}]
                    if followed: added.append({'id': 'after-single', 'type': 'agentMessage', 'text': after_single['text']})
                raw['items'][:] = [item for item in raw['items'] if item['id'] not in ('single-tool', 'after-single')] + added
                events.publish()
        publish_single(False)
        single_group = page.locator(scope + ' .grp').last
        expect(single_group.locator('.tool-active-tail .tool-out')).to_be_visible()
        expect(single_group.locator(':scope > .group-preview')).to_be_hidden()
        publish_single(True)
        expect(page.locator(scope)).to_contain_text(after_single['text'])
        expect(single_group.locator('.group-count')).to_have_text('🔧 ×1')
        expect(single_group.locator('.tool-out:visible')).to_have_count(0)
        expect(single_group.locator(':scope > .group-preview')).to_be_visible()
        capture('single-collapsed')
        single_group.locator(':scope > .group-preview .fold-toggle').click()
        expect(single_group.locator('.tool-out')).to_be_visible()
        single_group.get_by_text('展开全文', exact=False).click()
        expect(single_group.locator('.tool-out')).to_contain_text('输出第 17 行')
        capture('single-expanded')
        single_group.locator(':scope > .disclosure').click()
        expect(single_group.locator('.tool-out:visible')).to_have_count(0)
        assert not errors, errors
        for handle in stable: handle.dispose()
        context.unroute_all(behavior='wait')
        context.close()
        print(f'PASS {name}: running tail, completion, summary, manual inspection, next call', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default='codex,claude,grok,agy,opencode,kiro')
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots: args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sd-tool-progress-') as tmp, sync_playwright() as pw:
        browser = pw.chromium.launch(headless=True)
        try:
            for source in args.sources.split(','):
                root = Path(tmp) / source; root.mkdir()
                with provider(source, root, args.binary) as (base, _, extra), snapshots() as events, snapshots('message', replay=False) as watch:
                    hub_root = root / 'hub'; hub_root.mkdir()
                    hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                              [SimpleNamespace(name='ToolNode', token=TOKEN,
                               port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                    hub.start()
                    try:
                        for label, url in [('node', base), ('hub', f'http://127.0.0.1:{hub.port}')]:
                            for mode in (['sdk'] if source == 'kiro' else ['chat', 'sdk']):
                                exercise(browser, url, source, events, watch, args.screenshots, label, mode)
                    finally: hub.stop()
        finally: browser.close()


if __name__ == '__main__':
    main()
