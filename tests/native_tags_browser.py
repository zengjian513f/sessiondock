#!/usr/bin/env python3
"""Audited native tag families, exercised through real Chromium and API/search.

Synthetic histories only, no model CLI. Every positive has a literal/malformed
control; image delimiters must never become file-read authority.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
from urllib.parse import urlencode

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, claude_row, codex_row, encoded, get_json, isolated_server

PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAIAAAADCAIAAAA2iEnWAAAAE0lEQVR4nGP8z8DAwMDAxIBMAQAUQAEF3SN5DgAAAABJRU5ErkJggg=='
PLUGINS = '<recommended_plugins>\nHere is a list of plugins that are available but not installed.\nPluginInjectionSentinel\n</recommended_plugins>'
FORK = '<fork-boilerplate>\nYou are a worker fork. ForkInjectionSentinel\n</fork-boilerplate>'
SKILL = '<skill_information>\n<skills_referenced><skill name="demo"/></skills_referenced>\n<skill name="demo">SkillInjectionSentinel</skill>\n</skill_information>'


def build(root):
    corpus, cases = Corpus(root), {}
    for source in ('claude', 'codex', 'grok'):
        (root / source).mkdir()

    def put(sid, source, rows, expected, forbidden=(), role='user'):
        if source == 'grok':
            path = root / 'grok/project-tags' / sid
            path.mkdir(parents=True)
            (path / 'summary.json').write_text(json.dumps({'info': {'id': sid, 'cwd': '/synthetic/tags'}, 'generated_title': sid}))
            (path / 'chat_history.jsonl').write_bytes(b''.join(map(encoded, rows)))
            corpus.paths[sid] = path
        else:
            corpus.put(sid, source, rows, [])
        path = corpus.paths[sid]
        uid = source + ':' + hashlib.sha1(str(path).encode()).hexdigest()[:16]
        cases[sid] = dict(uid=uid, expected=expected, forbidden=forbidden, role=role)

    def cx(sid, contents, expected, forbidden=()):
        rows = [codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/tags'})]
        for content in contents:
            rows.append(codex_row('response_item', {'type': 'message', 'role': 'user', 'content': content}))
        put(sid, 'codex', rows, expected, forbidden)

    cx('plugins', [PLUGINS, 'ActualPluginQuestion'], 'ActualPluginQuestion', ['recommended_plugins', 'PluginInjectionSentinel'])
    cx('plugins-literal', ['Discuss ' + PLUGINS], 'Discuss ' + PLUGINS)
    cx('plugins-broken', [PLUGINS.removesuffix('</recommended_plugins>')], PLUGINS.removesuffix('</recommended_plugins>'))
    image_open = '<image name=[Image #1] path="/synthetic/not-authorized.png">'
    cx('image', [[{'type': 'input_text', 'text': image_open},
                  {'type': 'input_image', 'image_url': 'data:image/png;base64,' + PNG},
                  {'type': 'input_text', 'text': '</image>'},
                  {'type': 'input_text', 'text': 'ActualImageQuestion'}]], 'ActualImageQuestion', ['<image', '</image>', '/synthetic/not-authorized.png'])
    cx('image-literal', [image_open + 'text only</image>'], image_open + 'text only</image>')
    cx('user-markup', ['<task>Keep actual task</task>\n<div>Keep HTML</div>\nVec<T>'], '<task>Keep actual task</task>\n<div>Keep HTML</div>\nVec<T>')
    rows = [codex_row('session_meta', {'id': 'codex-internal', 'cwd': '/synthetic/tags'}),
            codex_row('response_item', {'type': 'message', 'role': 'developer', 'content': '<permissions>HiddenDeveloper</permissions><collaboration_mode>HiddenMode</collaboration_mode>'}),
            codex_row('response_item', {'type': 'message', 'role': 'user', 'content': '<codex_internal_context source="goal"><objective>HiddenGoal</objective></codex_internal_context>', 'internal_chat_message_metadata_passthrough': {'content_item_kinds': ['goal.internal_context']}}),
            codex_row('response_item', {'type': 'message', 'role': 'user', 'content': 'ActualInternalQuestion'})]
    put('codex-internal', 'codex', rows, 'ActualInternalQuestion', ['HiddenDeveloper', 'HiddenMode', 'HiddenGoal'])

    sid = 'parent'
    put(sid, 'claude', [claude_row(sid, 'user', 'u0', None, 'ActualParentQuestion')], 'ActualParentQuestion')
    side = corpus.paths[sid].with_suffix('') / 'subagents/agent-child.jsonl'
    side.parent.mkdir(parents=True)
    side.write_bytes(encoded(claude_row(sid, 'user', 'c0', None, [
        {'type': 'text', 'text': 'ActualChildDirective'}, {'type': 'text', 'text': FORK}], isSidechain=True, agentId='child')))
    # A main user quoting even the complete boilerplate remains a user input.
    put('fork-literal', 'claude', [claude_row('fork-literal', 'user', 'u0', None, FORK)], FORK)
    put('claude-internal', 'claude', [
        claude_row('claude-internal', 'user', 'u0', None, '<agent-message>HiddenPeer</agent-message>', isMeta=True),
        claude_row('claude-internal', 'user', 'u1', 'u0', '<skill-format>HiddenSkillFormat</skill-format>', isMeta=True),
        claude_row('claude-internal', 'user', 'u2', 'u1', 'ActualClaudeQuestion')], 'ActualClaudeQuestion', ['HiddenPeer', 'HiddenSkillFormat'])
    notify = '<task-notification>\n<task-id>monitor-1</task-id><summary>Monitor event: "Synthetic monitor"</summary><event>MonitorPayloadSentinel\n&lt;literal&gt; survives</event><result>ResultPayloadSentinel</result><note>NotePayloadSentinel</note><usage><subagent_tokens>123</subagent_tokens><tool_uses>4</tool_uses><duration_ms>500</duration_ms></usage><worktree><worktreePath>/synthetic/worktree</worktreePath><worktreeBranch>topic</worktreeBranch></worktree></task-notification>'
    put('notification', 'claude', [claude_row('notification', 'user', 'u0', None, notify)], '监控事件 · Synthetic monitor', ['<event>', '<usage>', '<worktree>'], 'event')
    unknown = '<task-notification>\n<summary>Unknown notification</summary><extension>KeepUnknownNotice</extension></task-notification>'
    put('notification-unknown', 'claude', [claude_row('notification-unknown', 'user', 'u0', None, unknown)], 'Unknown notification', role='event')

    def tool(sid, source, name, raw, expected, forbidden=(), error=False):
        if source == 'claude':
            rows = [claude_row(sid, 'user', 'u0', None, sid + ' question'),
                    claude_row(sid, 'assistant', 'a0', 'u0', [{'type': 'tool_use', 'id': 'call0', 'name': name, 'input': {}}]),
                    claude_row(sid, 'user', 'u1', 'a0', [{'type': 'tool_result', 'tool_use_id': 'call0', 'content': raw, 'is_error': error}]),
                    claude_row(sid, 'assistant', 'a1', 'u1', sid + ' complete')]
        else:
            rows = [{'type': 'user', 'content': sid + ' question'},
                    {'type': 'assistant', 'content': '', 'tool_calls': [{'id': 'call0', 'name': name, 'arguments': '{}'}]},
                    {'type': 'tool_result', 'tool_call_id': 'call0', 'content': raw},
                    {'type': 'assistant', 'content': sid + ' complete'}]
        put(sid, source, rows, expected, forbidden, 'tool_result')
        cases[sid]['error'] = error

    tool('tool-error', 'claude', 'Bash', '<tool_use_error>ErrorBodySentinel</tool_use_error>', 'ErrorBodySentinel', ['tool_use_error'], True)
    tool('persisted', 'claude', 'Bash', '<persisted-output>\nOutput too large. Full output saved to: /synthetic/output.txt\nPreview: PersistedBodySentinel\n</persisted-output>', 'Output too large. Full output saved to: /synthetic/output.txt\nPreview: PersistedBodySentinel', ['persisted-output'])
    tool('retrieval', 'claude', 'TaskOutput', '<retrieval_status>timeout</retrieval_status>\n<task_id>task-1</task_id><task_type>local_bash</task_type><status>running</status><output>RetrievalBodySentinel</output>', '获取状态：timeout\n任务：task-1\n任务类型：local_bash\n任务状态：running\n输出：RetrievalBodySentinel', ['<retrieval_status>', '<task_id>'])
    tool('reminder', 'claude', 'Read', 'ActualReadOutput\n<system-reminder>\nReminderBodySentinel\n</system-reminder>', 'ActualReadOutput\n系统提示：\nReminderBodySentinel', ['system-reminder'])
    tool('reminder-before', 'claude', 'Read', '<system-reminder>BeforeReadSentinel</system-reminder>\n1\tActualFileContents', '系统提示：\nBeforeReadSentinel\n1\tActualFileContents', ['system-reminder'])
    tool('reminder-combined', 'claude', 'Bash', '<system-reminder>BeforeOutput</system-reminder>\n<persisted-output>Output too large. CombinedPreview</persisted-output>\n<system-reminder>AfterOutput</system-reminder>', '系统提示：\nBeforeOutput\nOutput too large. CombinedPreview\n系统提示：\nAfterOutput', ['system-reminder', 'persisted-output'])
    tool('workspace', 'grok', 'grep', '<workspace_result workspace_path="/synthetic/workspace">\nWorkspaceBodySentinel <T>\n</workspace_result>', '工作区：/synthetic/workspace\nWorkspaceBodySentinel <T>', ['workspace_result'])
    tool('background', 'grok', 'bash', '<task-id>task-2</task-id><task-type>bash</task-type><output-file>/synthetic/task.log</output-file><status>running</status><summary>BackgroundBodySentinel</summary>', '任务：task-2\n任务类型：bash\n输出文件：/synthetic/task.log\n任务状态：running\n摘要：BackgroundBodySentinel', ['<task-id>', '<status>'])
    for sid, source, name, raw in [
        ('tool-literal', 'claude', 'Bash', 'Example: <tool_use_error>KeepLiteralTool</tool_use_error>'),
        ('tool-broken', 'claude', 'Bash', '<persisted-output>KeepBrokenTool'),
        ('tool-unknown', 'claude', 'TaskOutput', '<retrieval_status>timeout</retrieval_status><extension>KeepUnknownTool</extension>'),
        ('reminder-literal', 'claude', 'Read', 'Example <system-reminder>KeepLiteralReminder</system-reminder>'),
        ('workspace-literal', 'grok', 'bash', 'Example <workspace_result workspace_path="/synthetic">KeepLiteralWorkspace</workspace_result>'),
        ('background-unknown', 'grok', 'bash', '<task-id>id</task-id><task-type>bash</task-type><extension>KeepUnknownBackground</extension>'),
    ]:
        tool(sid, source, name, raw, raw)
    grok_query = '<user_query>\nActualSkillQuestion\n</user_query>\n' + SKILL
    put('skill', 'grok', [{'type': 'user', 'content': grok_query}], 'ActualSkillQuestion', ['skill_information', 'skills_referenced', 'SkillInjectionSentinel', 'user_query'])
    put('skill-literal-body', 'grok', [{'type': 'user', 'content': '<user_query>\n<system-reminder>ActualLiteralQuestion</system-reminder>\n</user_query>\n' + SKILL}], '<system-reminder>ActualLiteralQuestion</system-reminder>', ['skill_information', 'SkillInjectionSentinel'])
    put('skill-unknown', 'grok', [{'type': 'user', 'content': grok_query + '\nUnknown suffix'}], grok_query + '\nUnknown suffix')
    put('grok-internal', 'grok', [
        {'type': 'system', 'content': '<work_policy>HiddenWorkPolicy</work_policy>'},
        {'type': 'user', 'content': '<user_info>HiddenUserInfo</user_info><rules>HiddenRules</rules>'},
        {'type': 'user', 'synthetic_reason': 'system_reminder', 'content': '<system-reminder><monitor-event>HiddenMonitor</monitor-event></system-reminder>'},
        {'type': 'user', 'content': '<user_query>\nActualGrokQuestion\n</user_query>'}], 'ActualGrokQuestion', ['HiddenWorkPolicy', 'HiddenUserInfo', 'HiddenRules', 'HiddenMonitor'])
    return corpus, cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-native-tags-') as temporary:
        corpus, cases = build(Path(temporary))
        originals = {p: p.read_bytes() for p in corpus.root.rglob('*') if p.is_file()}
        with isolated_server(corpus, args.binary) as (base, opener), sync_playwright() as pw:
            options = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**options)
            try:
                context = browser.new_context(viewport={'width': 1400, 'height': 1000}, service_workers='block')
                context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda e: errors.append(str(e)))
                page.goto(base, wait_until='networkidle')
                for sid, case in cases.items():
                    data = get_json(opener, base, '/api/messages/' + case['uid'])
                    messages = [m for m in data['messages'] if m.get('role') == case['role']]
                    assert any(m['text'].strip() == case['expected'].strip() for m in messages), (sid, messages)
                    window = get_json(opener, base, '/api/messages/' + case['uid'] + '?window=1')
                    assert [(m['role'], m['text'], m.get('details')) for m in window['messages']] == [(m['role'], m['text'], m.get('details')) for m in data['messages']], (sid, 'window projection')
                    public = json.dumps(data['messages'], ensure_ascii=False)
                    for forbidden in case['forbidden']:
                        assert forbidden not in public, (sid, forbidden)
                    page.locator(f'#side .item[data-uid="{case["uid"]}"]').click()
                    if case['role'] == 'tool_result':
                        expect(page.locator('#msgs')).to_contain_text(sid + ' complete')
                        for toggle in page.locator('#msgs .turn-process.folded .fold-toggle, #msgs .grp.folded .fold-toggle').all():
                            toggle.click()
                        expect(page.locator('#msgs pre.tool-out')).to_contain_text(case['expected'])
                        assert messages[0]['error'] == case['error'], sid
                        if case['error']:
                            expect(page.locator('#msgs .tool-status.err')).to_be_visible()
                    else:
                        expect(page.locator('#msgs')).to_contain_text(case['expected'])
                    for forbidden in case['forbidden']:
                        expect(page.locator('#msgs')).not_to_contain_text(forbidden)
                    if sid == 'notification':
                        page.locator('#msgs .event-details summary').click()
                        for text in ['MonitorPayloadSentinel', '<literal> survives', 'ResultPayloadSentinel', 'NotePayloadSentinel', 'Token：123', '工具调用次数：4', '耗时（毫秒）：500', '工作树：/synthetic/worktree', '分支：topic']:
                            expect(page.locator('#msgs .event-detail-body')).to_contain_text(text)
                    if sid == 'notification-unknown':
                        page.locator('#msgs .event-details summary').click()
                        expect(page.locator('#msgs .event-detail-body')).to_contain_text('<extension>KeepUnknownNotice</extension>')
                    if sid == 'image':
                        expect(page.locator('#msgs img')).to_have_count(1)
                        expect(page.locator('#msgs img')).to_be_visible()
                        page.wait_for_function("document.querySelector('#msgs img')?.naturalWidth === 2")
                        assert messages[0]['media'] and data['meta']['title'] == 'ActualImageQuestion'
                        expect(page.locator(f'#side .item[data-uid="{case["uid"]}"]')).to_contain_text('ActualImageQuestion')
                    if sid == 'plugins':
                        assert data['meta']['title'] == 'ActualPluginQuestion'
                        expect(page.locator(f'#side .item[data-uid="{case["uid"]}"]')).to_contain_text('ActualPluginQuestion')
                    if sid == 'parent':
                        page.locator('#a-view-switch').click()
                        page.locator('#session-view-menu button[data-agent="child"]').click()
                        expect(page.locator('#msgs')).to_contain_text('ActualChildDirective')
                        expect(page.locator('#msgs')).not_to_contain_text('ForkInjectionSentinel')
                    print('PASS native tags browser:', sid, flush=True)
                # The same projection must apply to a real SSE append, not
                # just a freshly loaded history response.
                skill_case = cases['skill']
                page.locator(f'#side .item[data-uid="{skill_case["uid"]}"]').click()
                expect(page.locator('#msgs')).to_contain_text('ActualSkillQuestion')
                page.wait_for_function('_es && _es.readyState === EventSource.OPEN')
                skill_path = corpus.paths['skill'] / 'chat_history.jsonl'
                with skill_path.open('ab') as stream:
                    stream.write(encoded({'type': 'user', 'content': '<user_query>\nLiveSkillQuestion\n</user_query>\n' + SKILL}))
                originals[skill_path] = skill_path.read_bytes()
                expect(page.locator('#msgs')).to_contain_text('LiveSkillQuestion', timeout=10000)
                expect(page.locator('#msgs')).not_to_contain_text('SkillInjectionSentinel')
                # Search through the actual search input, including hidden
                # injection text: it must not reappear through cached bodies.
                for query, hit in [('ActualSkillQuestion', True), ('ActualImageQuestion', True), ('ActualPluginQuestion', True), ('PluginInjectionSentinel', False), ('SkillInjectionSentinel', False)]:
                    old = page.locator('#stat').get_attribute('data-seq') or ''
                    page.locator('#q').fill(query)
                    page.locator('#q').press('Enter')
                    page.wait_for_function("old => String(document.querySelector('#stat').dataset.seq || '') !== old", arg=old)
                    expect(page.locator('#search-progress')).not_to_be_visible()
                    results = get_json(opener, base, '/api/search?' + urlencode({'q': query}))['results']
                    # Literal controls intentionally contain the same sentinel:
                    # only the normalized positive fixture must disappear.
                    target = cases['plugins' if query == 'PluginInjectionSentinel' else 'skill']['uid']
                    if hit:
                        target = cases[{'ActualSkillQuestion': 'skill', 'ActualImageQuestion': 'image', 'ActualPluginQuestion': 'plugins'}[query]]['uid']
                        assert any(r.get('uid') == target for r in results), query
                        expect(page.locator(f'#side .item[data-uid="{target}"] .snip')).to_contain_text(query)
                    else:
                        assert all(r.get('uid') != target for r in results), (query, results)
                        expect(page.locator(f'#side .item[data-uid="{target}"]')).to_have_count(0)
                assert not errors, errors
            finally:
                browser.close()
        assert all(p.read_bytes() == data for p, data in originals.items()), 'native records were modified'
    print(f'PASS {len(cases)} native tag browser cases, subagent switch, details disclosure, native images, list titles, search, live append, read-only histories')


if __name__ == '__main__':
    main()
