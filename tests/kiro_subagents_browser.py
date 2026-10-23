#!/usr/bin/env python3
"""Kiro multiplexed subagents: list, isolated history, restart and legacy projection upgrade."""
from __future__ import annotations
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sys
import tempfile
import time
from types import SimpleNamespace
from kiro_sdk_fixtures import FakeKiroAcp, KiroFixture


class SubagentPeer(FakeKiroAcp):
    @staticmethod
    def history_updates(directory):
        updates = FakeKiroAcp.history_updates(directory)
        if not (directory.parent.parent / 'replay-subtasks').exists():
            return updates
        # Real Kiro replay supplies ownership on start, but only toolCallId on end.
        for aid in ['review-a', 'review-b', 'review-c']:
            updates.append({'sessionUpdate': 'tool_call', 'toolCallId': 'invoke_subagent_' + aid,
                            'title': aid, 'status': 'in_progress',
                            '_meta': {'kiro': {'kind': 'agent-subtask', 'agentSubtaskId': aid, 'replay': True}}})
        for aid, status in [('review-a', 'completed'), ('review-b', 'failed')]:
            updates.append({'sessionUpdate': 'tool_call_update', 'toolCallId': 'invoke_subagent_' + aid,
                            'status': status, 'rawOutput': 'Historical result ' + aid,
                            '_meta': {'kiro': {'replay': True}}})
        updates.append({'sessionUpdate': 'session_info_update', '_meta': {'kiro': {
            'kind': 'turn_end', 'stopReason': 'cancelled', 'replay': True}}})
        return updates

    def run_prompt(self, message):
        text = ' '.join(b.get('text', '') for b in message['params'].get('prompt', []))
        if text == 'Crash with an unfinished subagent':
            self.update('tool_call', toolCallId='invoke_subagent_crash', title='Unfinished child',
                        status='in_progress', _meta={'kiro': {'kind': 'agent-subtask', 'agentSubtaskId': 'crash'}})
            self.update('agent_message_chunk', content={'type': 'text', 'text': 'Child output before crash'},
                        _meta={'kiro': {'agentSubtaskId': 'crash'}})
            time.sleep(.5)
            print('FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory', file=sys.stderr, flush=True)
            os._exit(1)
        if text != 'Review with two parallel subagents':
            return super().run_prompt(message)
        for aid, title in [('review-a', '历史与搜索审查'), ('review-b', 'SDK 与生命周期审查')]:
            self.update('tool_call', toolCallId='invoke_subagent_' + aid,
                        title='Sub-agent: context-gatherer', status='in_progress',
                        rawInput={'name': 'context-gatherer', 'explanation': title},
                        _meta={'kiro': {'kind': 'agent-subtask', 'agentSubtaskId': aid}})
        for part in ['独立审查开始。', '并行审查完成。']:
            for aid in ['review-a', 'review-b']:
                self.update('agent_message_chunk', content={'type': 'text', 'text': aid + ':' + part},
                            _meta={'kiro': {'agentSubtaskId': aid, 'replayId': 'same-message-id'}})
        self.update('agent_message_chunk', content={'type': 'text', 'text': '主代理等待审查结果。'})
        for aid in ['review-a', 'review-b']:
            self.update('tool_call', toolCallId='same-tool-id', title='Read File', status='completed',
                        rawOutput=aid + ' 独立工具结果', _meta={'kiro': {'agentSubtaskId': aid}})
        gate = self.sessions_root / 'finish-subtasks'
        deadline = time.monotonic() + 120
        while not gate.exists() and time.monotonic() < deadline:
            time.sleep(.05)
        for aid in ['review-a', 'review-b']:
            self.update('tool_call_update', toolCallId='invoke_subagent_' + aid,
                        status='completed' if aid == 'review-a' else 'failed',
                        rawOutput='审查结果已返回' if aid == 'review-a' else '合成失败原因',
                        _meta={'kiro': {'replay': False}})
        self.reply(message, {'stopReason': 'end_turn'})


def fake():
    if 'acp' in sys.argv:
        peer = SubagentPeer()
        for line in sys.stdin:
            if line.strip():
                peer.handle(json.loads(line))
    else:
        from kiro_sdk_fixtures import main
        sys.argv.remove('--fake-subagents')
        main()


def run(args):
    from playwright.sync_api import expect, sync_playwright
    from codex_sdk_browser import server
    from kiro_sdk_browser import watch
    from sdk_ui_fixtures import start_sdk
    from sdkhost_fixtures import stop_private_sdkhosts
    from send_browser import initialize
    from agent_menu_browser import open_menu
    with tempfile.TemporaryDirectory(prefix='sessiondock-kiro-subagents-') as temporary:
        root = Path(temporary)
        for name in ('claude', 'codex', 'grok', 'state', 'trash'):
            (root / name).mkdir()
        fixture = KiroFixture(root, args.binary.resolve().with_name('ptyhost'))
        config = json.loads(fixture.launcher.read_text())
        config['profiles'][0]['args'] = [str(Path(__file__).resolve()), '--fake-subagents']
        fixture.launcher.write_text(json.dumps(config))
        initialize('--initialize-lifecycle', root / 'ledger', args.binary.resolve())
        env = {**fixture.env, 'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_STATE_DIR': str(root / 'state'),
               'SESSIONDOCK_LIFECYCLE_DIR': str(root / 'ledger'), 'SESSIONDOCK_PTYHOST_DIR': str(fixture.host_dir)}
        if args.hub:
            from hub_fixtures import free_port
            from node_auth_fixtures import TOKEN, node_env
            node_port = free_port()
            env.update(node_env(root, node_port, '127.0.0.0/8'))
        stack = ExitStack()
        try:
            with sync_playwright() as pw:
                browser = pw.chromium.launch(headless=True)
                page = browser.new_page(viewport={'width': 1400, 'height': 950})
                page.set_default_timeout(20000)
                def boot():
                    base = stack.enter_context(server(SimpleNamespace(root=root), args.binary, 'off', env))
                    if args.hub:
                        from hub_fixtures import Hub
                        hub_root = Path(tempfile.mkdtemp(prefix='hub-', dir=root))
                        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hub_root,
                                  [SimpleNamespace(name='Kiro', token=TOKEN, port=node_port)])
                        hub.start()
                        stack.callback(hub.stop)
                        base = f'http://127.0.0.1:{hub.port}'
                    page.goto(base, wait_until='domcontentloaded')
                    page.wait_for_function('T.listLoaded')
                    page.wait_for_function('async sid => { await loadSessions(true); return S.sessions.some(r => r.sid === sid); }', arg=fixture.sid)
                    uid = page.evaluate('sid => S.sessions.find(r => r.sid === sid).uid', fixture.sid)
                    page.locator(f'#side .item[data-uid="{uid}"]').click()
                    return uid
                def refresh_agents(active):
                    page.wait_for_function('''async ({uid, active}) => {
                        await loadSessions(true);
                        const row = S.sessions.find(r => r.uid === uid);
                        return row?.agent_items?.length === 2 && row.agent_items.every(a => a.active === active);
                    }''', arg={'uid': uid, 'active': active}, polling=300)
                def verify_views():
                    open_menu(page)
                    for aid, other in [('review-a', 'review-b'), ('review-b', 'review-a')]:
                        page.locator(f'#session-view-menu button[data-agent="{aid}"]').click()
                        page.wait_for_function("() => document.querySelector('#msgs')?.innerText.includes('独立审查开始。') || document.querySelector('#msgs button[aria-label=\"展开本轮过程\"]')")
                        if '独立审查开始。' not in page.locator('#msgs').inner_text():
                            page.get_by_role('button', name='展开本轮过程', exact=True).click()
                        expect(page.locator('#msgs')).to_contain_text(aid + ':独立审查开始。')
                        expect(page.locator('#msgs')).to_contain_text(aid + ':并行审查完成。')
                        expect(page.locator('#msgs')).not_to_contain_text(other + ':')
                        expect(page.locator('#msgs')).not_to_contain_text('主代理等待审查结果。')
                        expect(page.locator('#sdk-input')).not_to_be_visible()
                        open_menu(page)
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / 'kiro-subagents-desktop.png'), full_page=True)
                    page.locator('#session-view-menu button[data-agent=""]').click()
                    expect(page.locator('#msgs')).not_to_contain_text('review-a:独立审查开始。')
                uid = boot()
                page.locator('#a-sdk').click()
                start_sdk(page)
                watch(page, uid)
                page.locator('#sdk-input').fill('Review with two parallel subagents')
                page.locator('#sdk-send').click()
                refresh_agents(True)
                verify_views()
                before = len(fixture.received('session/prompt'))
                loads = len(fixture.received('session/load'))
                page.goto('about:blank')
                stack.close(); stack = ExitStack()
                uid = boot()
                refresh_agents(True)
                verify_views()
                # Downgrade only the private fixture checkpoint to the old flattened
                # representation, then exercise the automatic read-only upgrade.
                page.goto('about:blank')
                stack.close(); stack = ExitStack()
                checkpoint = next(p for p in (root / 'state/sdk-adapters/kiro').glob('*.json')
                                  if json.loads(p.read_text()).get('uid') == ('kiro:' + uid.split('~')[-1] if '~' in uid else uid))
                state = json.loads(checkpoint.read_text())
                for task in state.pop('subtasks').values():
                    state['items'].extend(task['state']['items'])
                state.pop('transcript_version')
                checkpoint.write_text(json.dumps(state))
                uid = boot()
                refresh_agents(True)
                verify_views()
                from kiro_sdk_browser import api_raw
                for method in ('open', 'stop', 'discard'):
                    denied = api_raw(page, method, {'uid': uid, 'agent': 'review-a'})
                    assert denied['status'] >= 400, denied
                (fixture.sessions / 'finish-subtasks').touch()
                refresh_agents(False)
                verify_views()
                assert len(fixture.received('session/prompt')) == before
                assert len(fixture.received('session/load')) == loads
                page.set_viewport_size({'width': 390, 'height': 844})
                page.locator(f'#side .item[data-uid=\"{uid}\"]').click()
                open_menu(page)
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / 'kiro-subagents-mobile.png'), full_page=True)
                page.emulate_media(color_scheme='dark')
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / 'kiro-subagents-mobile-dark.png'), full_page=True)
                page.set_viewport_size({'width': 1400, 'height': 950})
                page.emulate_media(color_scheme='light')
                page.locator('#session-view-menu button[data-agent=""]').click()
                page.locator('#sdk-input').fill('Crash with an unfinished subagent')
                page.locator('#sdk-send').click()
                page.wait_for_function("""async uid => {
                    await loadSessions(true);
                    const row = S.sessions.find(r => r.uid === uid);
                    return row?.agent_items?.length === 3 && row.agent_items.every(a => !a.active);
                }""", arg=uid)
                expect(page.locator('.native-sdk-pane')).to_contain_text('heap out of memory')
                page.reload(wait_until='domcontentloaded')
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                expect(page.locator('#side')).not_to_contain_text('运行中')
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / 'kiro-subagents-crash.png'), full_page=True)
                (fixture.sessions / 'replay-subtasks').touch()
                start_sdk(page)
                watch(page, uid)
                page.wait_for_function("""async uid => {
                    await loadSessions(true);
                    const row = S.sessions.find(r => r.uid === uid);
                    return row?.agent_items?.filter(a => a.id.startsWith('review-')).length === 3
                        && row.agent_items.every(a => !a.active);
                }""", arg=uid)
                # Exercise upgrade of an already persisted v1 replay checkpoint.
                page.goto('about:blank')
                stack.close(); stack = ExitStack()
                for checkpoint in (root / 'state/sdk-adapters/kiro').glob('*.json'):
                    state = json.loads(checkpoint.read_text())
                    if 'review-c' not in state.get('subtasks', {}):
                        continue
                    state['transcript_version'] = 1
                    for task in state['subtasks'].values():
                        task.pop('tool_call_id', None)
                        task['status'] = 'in_progress'
                    checkpoint.write_text(json.dumps(state))
                uid = boot()
                page.wait_for_function("""async uid => {
                    await loadSessions(true);
                    const row = S.sessions.find(r => r.uid === uid);
                    return row?.agent_items?.some(a => a.id === 'review-c') && row.agent_items.every(a => !a.active);
                }""", arg=uid)
                page.reload(wait_until='domcontentloaded')
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                open_menu(page)
                expect(page.locator('#session-view-menu')).to_contain_text('review-c')
                expect(page.locator('#session-view-menu')).not_to_contain_text('运行中')
                expect(page.locator('#side')).not_to_contain_text('运行中')
                for checkpoint in (root / 'state/sdk-adapters/kiro').glob('*.json'):
                    state = json.loads(checkpoint.read_text())
                    if 'review-c' in state.get('subtasks', {}):
                        assert {k: v['status'] for k, v in state['subtasks'].items()} == {
                            'review-a': 'completed', 'review-b': 'failed', 'review-c': 'interrupted'}
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / 'kiro-subagents-replay.png'), full_page=True)
                browser.close()
        finally:
            stack.close()
            stop_private_sdkhosts(root)
    print('Kiro subagents browser: PASS')


def main():
    if '--fake-subagents' in sys.argv:
        return fake()
    from history_fixtures import BINARY
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--screenshots', type=Path)
    parser.add_argument('--hub', action='store_true')
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    run(args)


if __name__ == '__main__':
    main()
