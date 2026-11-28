#!/usr/bin/env python3
"""Live progress across the seven native SDK views on a node and through Hub."""
import argparse
from contextlib import closing
from datetime import datetime, timedelta, timezone
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright

from history_fixtures import BINARY, REPO
from hub_fixtures import Hub
from node_auth_fixtures import TOKEN
from opencode_sdk_fixtures import SID as OPENCODE_PARENT
from sdk_mode_fixtures import provider
from sdk_selection_fixtures import snapshots
from sdk_ui_fixtures import start_sdk


SOURCES = ('claude', 'codebuddy', 'grok', 'opencode', 'kiro', 'droid', 'agy')
LIVE_REPLIES = {
    'claude': ('progress-hold', 'streaming Claude progress'),
    'codebuddy': ('progress-hold', 'streaming CodeBuddy progress'),
    'grok': ('wait-for-cancel', 'Grok streaming '),
    'opencode': ('slow progress', 'partial native reply'),
    'kiro': ('wait-for-cancel', 'Partial response before stop'),
    'droid': ('hold', 'Droid working'),
    'agy': ('hold:progress', 'Agy running progress '),
}


def exercise(browser, base, source, label, screenshots):
    for width, theme in ((1280, 'light'), (390, 'dark')):
        with snapshots() as events:
            context = browser.new_context(viewport={'width': width, 'height': 760},
                                          color_scheme=theme, service_workers='block')
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            context.route('**/api/native-sdk/open', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
            context.route('**/api/native-sdk/events?*', lambda r: r.continue_(url=events.url))
            for endpoint in ('models', 'usage', 'agents'):
                context.route(f'**/api/native-sdk/{endpoint}*', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))

            raw = {'running': False, 'items': [], 'requests': [], 'capabilities': {'interrupt': True}}
            events.packet = {'type': 'snapshot', 'seq': 0, 'raw': raw}
            page.goto(base, wait_until='domcontentloaded')
            page.wait_for_function('T.listLoaded')
            page.locator(f'#side .item[data-uid^="{source}:"]').first.click()
            if not page.locator('#sdk-input').is_visible():
                page.locator('#a-sdk').click()
            start_sdk(page)

            status = page.locator('.native-sdk-pane .sdk-status [role=status]')
            metric = page.locator('.native-sdk-pane .sdk-turn-metric')
            detail = page.locator('#sdk-status-menu')
            prior = [
                {'id': 'old-prompt', 'role': 'user', 'text': '上一轮'},
                {'id': 'old-reply', 'role': 'assistant', 'text': '旧回复不计入本轮'},
            ]
            observed = (datetime.now(timezone.utc) - timedelta(seconds=12)).isoformat()
            prompt = {'id': 'prompt', 'role': 'user', 'request_id': 'round', 'ts': observed,
                      'text': '检查测试覆盖范围。'}
            queued = {'id': 'next', 'role': 'user', 'request_id': 'next', 'status': 'queued',
                      'delivery_state': 'queued', 'text': '排队消息'}
            start = {'request_id': 'round'} if source in ('claude', 'codebuddy', 'droid', 'agy') else {}
            def update(*items, streaming=None, queued_input=False, **state):
                raw.update(running=True, items=[*prior, prompt, *items, *([queued] if queued_input else [])],
                           progress={**start, 'stream_item': streaming} if source == 'kiro' else start,
                           requests=[])
                raw.update(state)
                events.publish()

            def capture(name):
                if screenshots:
                    page.screenshot(path=str(screenshots / f'{label}-{source}-{width}-{theme}-{name}.png'))

            def show_details(text):
                status.click()
                expect(detail).to_be_visible()
                expect(detail).to_contain_text(text)
                expect(detail).to_contain_text('本轮已用时')
                page.keyboard.press('Escape')

            update()
            expect(status).to_have_text('处理中')
            if source == 'agy':
                # Agy can send only DONE tool steps for minutes before its next
                # planner response. Replay has their receipts but no prompt ts.
                prompt.pop('ts')
                prompt['received_at'] = (datetime.now(timezone.utc) - timedelta(days=1)).isoformat()
                done = [{'id': f'tool-{i}', 'request_id': 'round', 'role': 'tool', 'status': 'DONE',
                         'name': name, 'received_at': observed} for i, name in enumerate(
                             ['read_file'] * 22 + ['run_command'])]
                prior.append({**done[-1], 'id': 'previous-tool', 'request_id': 'previous'})
                update(*done, queued_input=True)
                expect(status).to_have_text('处理中')
                expect(metric).to_have_text('· 已结束 23 项工具')
                if width == 390:
                    expect(metric).to_be_hidden()
                else:
                    expect(metric).to_be_visible()
                capture('between-tools')
                status.click()
                expect(detail).to_contain_text('已结束 23 项工具 · 最近一项 run_command')
                expect(detail).to_contain_text('本轮已用时')
                expect(detail).not_to_contain_text('小时')
                expect(detail).not_to_contain_text('正在运行')
                expect(detail).not_to_have_text(detail.text_content(), timeout=4000)
                capture('between-tools-detail')
                page.keyboard.press('Escape')
                page.reload(wait_until='domcontentloaded')
                if page.locator('#a-sdk').is_visible() and not page.locator('#sdk-input').is_visible():
                    page.locator('#a-sdk').click()
                start_sdk(page)
                expect(metric).to_have_text('· 已结束 23 项工具')
                update(*done, {'id': 'done-thought', 'role': 'thinking', 'content': '核对完毕'},
                       {'id': 'done-reply', 'role': 'assistant', 'status': 'DONE', 'content': '已读取'})
                status.click()
                expect(detail).to_contain_text('4 字符思考记录')
                expect(detail).to_contain_text('3 字符回复')
                page.keyboard.press('Escape')
                update()
                expect(metric).to_be_hidden()
                prompt['ts'] = observed
            thought = {'id': 'thought', 'role': 'thinking', 'text': '先核对历史。', 'ts': observed}
            if source == 'grok':
                thought['native_content'] = [{'type': 'text', 'text': thought['text']}]
            elif source != 'kiro':
                thought['sdk_streaming'] = True
            update(thought, streaming='thought')
            expect(status).to_have_text('思考中')
            thought_row = page.locator('[data-sdk-item="thought"]')
            expect(thought_row).to_be_visible()
            expect(thought_row.locator('.mb')).to_be_hidden()
            thought_row.get_by_role('button', name='展开思考记录').click()
            expect(thought_row.locator('.mb')).to_have_text(thought['text'])
            show_details(f'{len(thought["text"])} 字符思考记录')
            capture('thinking')
            thought['text'] += '再核对测试。'
            update(thought, streaming='thought')
            expect(thought_row.locator('.mb')).to_be_visible()
            expect(thought_row.locator('.mb')).to_have_text(thought['text'])

            tool = {'id': 'tool', 'role': 'tool', 'name': 'test_lookup', 'text': '{}', 'ts': observed}
            if source in ('claude', 'codebuddy'):
                tool['native_block'] = {'type': 'tool_use'}
            elif source == 'droid':
                tool['native_type'] = 'tool_call'
            else:
                tool['status'] = 'ACTIVE' if source == 'agy' else 'in_progress'
            update(thought, tool)
            expect(status).to_have_text('执行工具')
            show_details('正在运行 1 项工具 · test_lookup')
            update(thought, tool, requests=[{'key': 'ask', 'type': 'approval', 'title': '批准测试'}])
            expect(status).to_have_text('待批准')
            expect(metric).to_be_hidden()
            tool.update(status='completed', result={'role': 'tool_result', 'text': '完成'})

            reply = {'id': 'reply', 'role': 'assistant', 'text': '你好🌱', 'ts': observed}
            if source == 'grok':
                reply['native_content'] = [{'type': 'text', 'text': reply['text']}]
            elif source == 'agy':
                reply.update(status='ACTIVE', native_type='agent_response')
            elif source != 'kiro':
                reply['sdk_streaming'] = True
            update(thought, tool, reply, streaming='reply', queued_input=True)
            expect(status).to_have_text('生成回复')
            expect(metric).to_have_text('· 3 字符')
            if width == 390:
                expect(metric).to_be_hidden()
            else:
                expect(metric).to_be_visible()
            show_details('本轮已收到 3 字符回复')
            capture('reply')

            page.reload(wait_until='domcontentloaded')
            if page.locator('#a-sdk').is_visible() and not page.locator('#sdk-input').is_visible():
                page.locator('#a-sdk').click()
            start_sdk(page)
            expect(status).to_have_text('生成回复')
            expect(metric).to_have_text('· 3 字符')
            update(thought, tool, reply, running=False, last_turn={'status': 'completed'})
            expect(status).to_have_text('就绪')
            expect(metric).to_be_hidden()
            if source == 'grok':
                update(running=False, native_tasks=['subagent_id:child-a', 'subagent_id:child-b'])
                expect(status).to_have_text('后台运行')
                status.click()
                expect(detail).to_contain_text('子代理运行中 · child-a')
                expect(detail).to_contain_text('子代理运行中 · child-b')
                update(running=False, native_tasks=['subagent_id:child-b'])
                expect(detail).not_to_contain_text('child-a')
                expect(detail).to_contain_text('child-b')
                capture('child-background')
                page.keyboard.press('Escape')
                update(running=False, native_tasks=[])
                expect(status).to_have_text('就绪')
            update(thought, tool, reply, progress=None)
            expect(status).to_have_text('处理中')
            expect(metric).to_be_hidden()
            assert not errors, errors
            context.close()
            print(f'PASS {label} {source} {width}/{theme}: phase, counts, approval, reload, completion', flush=True)


def exercise_large_tool_run(browser, base, label, screenshots):
    with snapshots() as events:
        page = browser.new_page(viewport={'width': 1280, 'height': 900}, color_scheme='light',
                                service_workers='block')
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.route('**/api/native-sdk/open', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))
        page.route('**/api/native-sdk/events?*', lambda r: r.continue_(url=events.url))
        for endpoint in ('models', 'usage', 'agents'):
            page.route(f'**/api/native-sdk/{endpoint}*', lambda r: r.fulfill(json={'ok': True, 'raw': {}}))

        def tool(index):
            return {'id': f'large-read-{index}', 'role': 'tool', 'name': 'Read', 'status': 'completed',
                    'summary': f'Read record-{index}.md',
                    'input': {'file_path': f'/synthetic/record-{index}.md'},
                    'result': {'role': 'tool_result', 'text': f'Output for synthetic record {index}'}}

        initial = [{'role': 'user', 'id': 'large-prompt', 'text': 'Read the synthetic records'},
                   *(tool(index) for index in range(920))]
        raw = {'running': True, 'requests': [], 'items': initial, 'capabilities': {'interrupt': True}}
        events.packet = {'type': 'snapshot', 'seq': 0, 'snapshot_version': 0, 'raw': raw}
        page.goto(base, wait_until='domcontentloaded')
        page.locator('#side .item[data-uid^="droid:"]').first.click()
        if not page.locator('#sdk-input').is_visible():
            page.locator('#a-sdk').click()
        start_sdk(page)

        live = page.locator('.sdk-live')
        group = live.locator(':scope > .sdk-tool-group')
        expect(group.locator(':scope > [data-sdk-item]')).to_have_count(920)
        expect(group.locator(':scope > .tool-msg')).to_have_count(1)
        folded_nodes = live.locator('*').count()
        tail_args = group.locator('[data-sdk-item="large-read-919"] .tool-toggle')
        tail_args.click()
        expect(tail_args).to_have_attribute('aria-expanded', 'true')

        def splice(version, start, count, values):
            events.publish({'type': 'snapshot_delta', 'seq': version, 'snapshot_base': version - 1,
                            'snapshot_version': version, 'raw': {'kind': 'object', 'remove': [],
                                'fields': [['items', {'kind': 'splice', 'start': start,
                                                     'delete': count, 'values': values}]]}})

        splice(1, len(initial), 0, [tool(920)])
        expect(group.locator('.tool-active-tail')).to_contain_text('synthetic record 920')
        expect(group.locator(':scope > .tool-msg')).to_have_count(1)
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-droid-large-folded.png'))
        first = group.locator('[data-sdk-item="large-read-0"]')
        group.locator('.group-preview .fold-toggle').click()
        expect(first).to_contain_text('synthetic record 0')
        expect(tail_args).to_have_attribute('aria-expanded', 'true')
        expect(group.locator(':scope > .tool-msg')).to_have_count(921)
        open_nodes = live.locator('*').count()
        draft = page.locator('#sdk-input')
        draft.click()
        draft.press('ControlOrMeta+a')
        draft.press_sequentially('draft kept while tools update')
        page.evaluate("""() => {
          const live = NativeSdkView.live;
          const group = live.querySelector('.sdk-tool-group');
          const first = group.querySelector('[data-sdk-item="large-read-0"]');
          window.detached = 0;
          window.observeLargeRun = new MutationObserver(records => {
            for (const record of records)
              for (const node of record.removedNodes)
                if (node === group || node === first) window.detached++;
          });
          observeLargeRun.observe(live, {childList:true});
          observeLargeRun.observe(group, {childList:true});
        }""")

        splice(2, len(initial), 1, [tool(921), {'role': 'thinking', 'id': 'between-reads',
                                               'text': 'Next batch of synthetic reads'},
                                    tool(922), tool(923), tool(924)])
        expect(live.locator(':scope > .sdk-tool-group')).to_have_count(2)
        expect(live.locator('[data-sdk-item="large-read-920"]')).to_have_count(0)
        expect(live.locator('[data-sdk-item="between-reads"]')).to_contain_text('Next batch')
        expect(first).to_be_visible()
        expect(draft).to_be_focused()
        expect(draft).to_have_value('draft kept while tools update')
        assert page.evaluate('window.detached') == 0, 'retained tool group or first tool was detached'
        page.evaluate('observeLargeRun.disconnect()')

        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-droid-large-wide.png'))
        splice(3, len(initial) + 1, 1, [])
        expect(group.locator(':scope > [data-sdk-item]')).to_have_count(924)
        page.set_viewport_size({'width': 390, 'height': 900})
        page.emulate_media(color_scheme='dark')
        page.evaluate('showMobileDetail()')
        group.locator('.group-preview .fold-toggle').click()
        expect(first).to_be_hidden()
        if screenshots:
            page.screenshot(path=str(screenshots / f'{label}-droid-large-narrow.png'))
        assert not errors, errors
        page.close()
        print(f'PASS {label} Droid: 920-tool run retained through append, split and rejoin '
              f'({folded_nodes} folded / {open_nodes} expanded live DOM elements)', flush=True)


def native_reply(browser, base, source, sid=None, screenshot=None):
    context = browser.new_context(viewport={'width': 1280, 'height': 760}, service_workers='block')
    if sid:
        context.add_init_script("localStorage.setItem('sessiondock.childMode', JSON.stringify('flat'))")
    page = context.new_page()
    try:
        page.goto(base, wait_until='domcontentloaded')
        page.wait_for_function('T.listLoaded')
        if sid:
            page.wait_for_function('sid => S.sessions.some(row => row.sid === sid)', arg=sid)
            uid = page.evaluate('sid => S.sessions.find(row => row.sid === sid).uid', sid)
            page.locator(f'#side .item[data-uid="{uid}"]').click()
        else:
            page.locator(f'#side .item[data-uid^="{source}:"]').first.click()
        if not page.locator('#sdk-input').is_visible():
            page.locator('#a-sdk').click()
        start_sdk(page)
        prompt, reply = LIVE_REPLIES[source]
        page.locator('#sdk-input').fill(prompt)
        page.locator('#sdk-send').click()
        expect(page.locator('.sdk-live')).to_contain_text(reply.strip(), timeout=15000)
        expect(page.locator('.native-sdk-pane .sdk-status [role=status]')).to_have_text('生成回复')
        expect(page.locator('.native-sdk-pane .sdk-turn-metric')).to_have_text(f'· {len(reply)} 字符')
        if sid:
            assert page.evaluate('NativeSdkView.raw.session_id') == sid
        if screenshot:
            page.locator('.native-sdk-pane .sdk-status').click()
            expect(page.locator('#sdk-status-menu:popover-open')).to_contain_text(f'{len(reply)} 字符回复')
            page.screenshot(path=str(screenshot))
            page.keyboard.press('Escape')
        page.locator('#sdk-stop').click()
        expect(page.locator('#sdk-stop')).to_be_hidden(timeout=15000)
        print(f'PASS {source}: browser SEND through native protocol, reply count and stop', flush=True)
    finally:
        context.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--sources', default=','.join(SOURCES))
    parser.add_argument('--screenshots', type=Path)
    args = parser.parse_args()
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    hub_binary = args.binary.resolve().with_name('sessiondock-hub')
    if not hub_binary.is_file():
        hub_binary = REPO / 'target/debug/sessiondock-hub'
    with tempfile.TemporaryDirectory(prefix='sd-sdk-progress-') as tmp, sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        for source in args.sources.split(','):
            root = Path(tmp) / source
            root.mkdir()
            with provider(source, root, args.binary) as (base, peer, extra):
                exercise(browser, base, source, 'node', args.screenshots)
                if source == 'droid':
                    exercise_large_tool_run(browser, base, 'node', args.screenshots)
                hub_root = root / 'hub'
                hub_root.mkdir()
                hub = Hub(hub_binary, hub_root, [
                    SimpleNamespace(name='ProgressNode', token=TOKEN,
                                    port=int(extra['SESSIONDOCK_NODE_BIND'].rsplit(':', 1)[1]))])
                hub.start()
                try:
                    hub_base = f'http://127.0.0.1:{hub.port}'
                    exercise(browser, hub_base, source, 'hub', args.screenshots)
                    if source == 'droid':
                        exercise_large_tool_run(browser, hub_base, 'hub', args.screenshots)
                    if source == 'opencode':
                        child = 'ses_sessiondock_sdk_child'
                        peer.create(child, str(peer.cwd))
                        with closing(sqlite3.connect(peer.db)) as connection, connection:
                            connection.execute('UPDATE session_v2 SET parent_id=? WHERE id=?',
                                               (OPENCODE_PARENT, child))
                        native_reply(browser, hub_base, source, child, args.screenshots / 'hub-opencode-child.png'
                                     if args.screenshots else None)
                finally:
                    hub.stop()
                native_reply(browser, base, source, OPENCODE_PARENT if source == 'opencode' else None)
        browser.close()


if __name__ == '__main__':
    main()
