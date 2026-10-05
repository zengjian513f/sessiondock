#!/usr/bin/env python3
"""Agy read-only native history through the selected UI in real Chromium.

Build target/debug/sessiondock first, then run this script with --binary PATH.
Only synthetic conversation_summaries.db and transcript_full.jsonl are used;
No CLI, account, or conversation binary schema is used. Media and raw tool
fixtures follow the CLI's documented full-transcript fields; they do not
claim a real model/tool round trip.
All runtime files are under /tmp/sessiondock-agy-history-*. The server uses
Corpus/isolated_server with private HOME, native root, mirror and loopback.
"""
from __future__ import annotations

import argparse
from contextlib import closing, contextmanager
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import re
import sqlite3
import sys
import tempfile
import time
from urllib.parse import urlencode, urlsplit

# Running directly must not emit __pycache__ files into the shared checkout.
sys.dont_write_bytecode = True

from playwright.sync_api import expect, sync_playwright


from frontend_paths import frontend_dir
from history_parity import BINARY, Corpus, codex_row, codex_message, encoded, get_json, isolated_server


SEEDED = 'a6000000-0000-4000-8000-000000000001'
PAGES = 'a6000000-0000-4000-8000-000000000002'
DECOY = 'a6000000-0000-4000-8000-000000000003'
ORPHAN = 'a6000000-0000-4000-8000-000000000004'
INDEX_ONLY = 'a6000000-0000-4000-8000-000000000006'
TITLE = '已有的 Agy 中文会话'
USER = '请阅读原生历史，保留正文中的 <USER_REQUEST>literal</USER_REQUEST>。'
ANSWER = '原生正文 agyzebracorn REWRITE_A'
THINKING = '先核对完整投影 agyquokkaridge'
SCHEMA = '''CREATE TABLE conversation_summaries (
    conversation_id TEXT, title TEXT, last_modified_time TEXT,
    workspace_uris TEXT, parent_conversation_id TEXT, status TEXT,
    step_count INTEGER, agent_name TEXT
)'''
SYSTEM_INTRO = ('The following is a <SYSTEM_MESSAGE> not actually sent by the user. '
                'It is provided by the system as important information to pay attention to.\n\n')
TAG_USER = '另一条输入：保留 <SYSTEM_MESSAGE>用户自己的示例</SYSTEM_MESSAGE> 和 <String>。'
SETTINGS = '模型设置已更新 agysettingsnotice'
UNKNOWN_TAG = '未定义附加标签的正文 agyextratag <String>'
SYSTEM_NOTICE = '[Notice] 后台任务因服务重启停止 agysystemnotice'
TASK_NOTICE = 'Task id "fixture/task-46" finished with result:\n\nOutput: agytasknotice <String>'


def system_record(index, text, sender='system', priority='MESSAGE_PRIORITY_LOW'):
    return record(index, 'SYSTEM_MESSAGE', SYSTEM_INTRO + '<SYSTEM_MESSAGE>\n'
                  + f'[Message] timestamp=2026-10-04T00:00:00Z sender={sender} priority={priority} content={text}'
                  + '\n</SYSTEM_MESSAGE>')


def record(index, kind, text, *, thinking=None):
    timestamp = (datetime(2026, 10, 4, tzinfo=timezone.utc)
                 + timedelta(seconds=index)).isoformat().replace('+00:00', 'Z')
    if kind == 'USER_INPUT':
        text = f'<USER_REQUEST>\n{text}\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\n{timestamp}\n</ADDITIONAL_METADATA>'
    value = {'type': kind, 'created_at': timestamp, 'step_index': index,
             'source': 'USER_EXPLICIT' if kind == 'USER_INPUT' else 'MODEL',
             'status': 'DONE', 'content': text}
    # These fields follow the complete transcript's documented schema.
    if thinking is not None:
        value['thinking'] = thinking
    return value


def transcript(native, sid):
    return native / 'brain' / sid / '.system_generated/logs/transcript_full.jsonl'


def write_transcript(native, sid, records):
    path = transcript(native, sid)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    path.write_bytes(b''.join(encoded(value) for value in records))
    return path


def seed_catalog(db, work, *, title=TITLE):
    with closing(sqlite3.connect(db)) as connection, connection:
        connection.execute(SCHEMA)
        for sid, label, steps in ((SEEDED, title, 2), (PAGES, 'Agy 分页历史', 480),
                                  (DECOY, '另一条 Agy 会话', 2),
                                  (INDEX_ONLY, '只有索引的 Agy 会话', 1)):
            connection.execute('INSERT INTO conversation_summaries VALUES (?, ?, ?, ?, ?, ?, ?, ?)',
                               (sid, label, '2026-10-04T00:08:00Z',
                                json.dumps([work.as_uri()]), '', 'DONE', steps, 'synthetic-agent'))


def execute(db, sql, parameters=()):
    with closing(sqlite3.connect(db, timeout=10)) as connection, connection:
        connection.execute(sql, parameters)


def native_snapshot(native):
    """Compare original bytes and mtimes directly, without introducing hashes."""
    return {str(path.relative_to(native)): (path.read_bytes(), path.stat().st_mtime_ns)
            for path in native.rglob('*') if path.is_file()}


def assert_native(native, expected, stage):
    actual = native_snapshot(native)
    assert actual.keys() == expected.keys(), f'{stage}: ordinary read changed native file set'
    for name, value in expected.items():
        assert actual[name] == value, f'{stage}: ordinary read changed native bytes/mtime: {name}'


def wait_for(predicate, description, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.1)
    raise AssertionError(f'timed out: {description}')


def mirror_stamps(directory):
    return {name: (directory.joinpath(name).stat().st_mtime_ns,
                   directory.joinpath(name).read_bytes())
            for name in ('summary.json', 'messages.jsonl')}


@contextmanager
def private_temp(root):
    """Keep Chromium profiles, driver output and Python server logs in our root."""
    previous_env, previous_cache = os.environ.get('TMPDIR'), tempfile.tempdir
    os.environ['TMPDIR'] = str(root)
    tempfile.tempdir = str(root)
    try:
        yield
    finally:
        tempfile.tempdir = previous_cache
        if previous_env is None:
            os.environ.pop('TMPDIR', None)
        else:
            os.environ['TMPDIR'] = previous_env


@contextmanager
def private_environment(root):
    # isolated_server intentionally accepts only SESSIONDOCK_* extra_env.
    # Scope HOME here so all inherited defaults are private too. Keep the selected frontend.
    values = {'HOME': str(root / 'home'), 'XDG_CONFIG_HOME': str(root / 'home/config'),
              'XDG_DATA_HOME': str(root / 'home/data'), 'XDG_CACHE_HOME': str(root / 'home/cache'),
              'PATH': '/usr/bin:/bin', 'SESSIONDOCK_TEST_WEB_DIR': str(frontend_dir())}
    previous = {key: os.environ.get(key) for key in values}
    os.environ.update(values)
    try:
        yield
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def messages(opener, base, uid, **query):
    return get_json(opener, base, '/api/messages/' + uid + ('?' + urlencode(query) if query else ''))


def checkpoint(snapshot):
    return {'start': snapshot['end'], 'head': snapshot['version']['head'],
            'anchor': snapshot['anchor'], 'append': 1}


def progress(text):
    print('PASS ' + text, flush=True)


def run(binary):
    print('Frontend: ' + ('legacy')
          + ' (' + str(frontend_dir()) + ')', flush=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-agy-history-', dir='/tmp') as temporary:
        root = Path(temporary).resolve()
        for name in ('home', 'work/中文 工作目录', 'host', 'state', 'claude', 'codex', 'grok', 'native', 'proc', 'trash'):
            (root / name).mkdir(parents=True, mode=0o700)
        work, native, mirror = root / 'work/中文 工作目录', root / 'native', root / 'mirror'
        pixel = work / 'agy 图片.png'
        pixel.write_bytes(bytes.fromhex('89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360f8cfc0f01f0005000201ff6c2d5e2b0000000049454e44ae426082'))
        db = native / 'conversation_summaries.db'
        seed_catalog(db, work)
        seed_records = [record(0, 'USER_INPUT', USER),
                        record(1, 'PLANNER_RESPONSE', ANSWER, thinking=THINKING)]
        seed_records[0]['media'] = [{'mime_type': 'image/png', 'uri': pixel.as_uri()}]
        path = write_transcript(native, SEEDED, seed_records)
        raw_call = record(2, 'PLANNER_RESPONSE', '')
        raw_call['tool_calls'] = [{'name': 'fixture_read', 'arguments': {'path': 'fixture.txt'}}]
        error = record(4, 'ERROR_MESSAGE', 'Error: model output must contain either output text or tool calls')
        error['error'] = 'model output must contain either output text or tool calls'
        tagged_user = record(0, 'USER_INPUT', TAG_USER)
        tagged_user['content'] += ('\n<USER_SETTINGS_CHANGE>\n' + SETTINGS + '\n</USER_SETTINGS_CHANGE>'
                                  '\n<EXTRA_NATIVE_TAG>\n' + UNKNOWN_TAG + '\n</EXTRA_NATIVE_TAG>')
        write_transcript(native, DECOY, [tagged_user,
                                       record(1, 'PLANNER_RESPONSE', '另一个回答'), raw_call,
                                       record(3, 'UNKNOWN_NATIVE_STEP', '完整工具结果 agyrawresult'), error,
                                       system_record(5, SYSTEM_NOTICE),
                                       system_record(6, TASK_NOTICE, 'fixture/task-46', 'MESSAGE_PRIORITY_HIGH'),
                                       record(7, 'SYSTEM_MESSAGE', '纯文本系统消息 agysystemplain'),
                                       record(8, 'SYSTEM_MESSAGE', '<SYSTEM_MESSAGE>\n未闭合的原始内容 agysystembroken'),
                                       record(9, 'SYSTEM_MESSAGE', '')])
        page_records = [record(index, 'USER_INPUT' if index % 2 == 0 else 'PLANNER_RESPONSE',
                               f'AGY PAGE {index:04d}') for index in range(480)]
        page_path = write_transcript(native, PAGES, page_records)
        directory = mirror / 'cli' / SEEDED
        expected_native = native_snapshot(native)
        corpus = Corpus(root)
        mixed_sid = 'a6000000-0000-4000-8000-000000000005'
        mixed_path = corpus.put(mixed_sid, 'codex', [codex_row('session_meta', {
            'id': mixed_sid, 'cwd': str(work), 'timestamp': '2026-10-04T00:00:00Z'}),
            codex_message('user', '混合组父会话')], ['混合组父会话'])
        mixed_before = mixed_path.read_bytes()
        extra_env = {'SESSIONDOCK_AGY_HOME': str(native), 'SESSIONDOCK_AGY_ROOT': str(mirror),
                     'SESSIONDOCK_PROC_ROOT': str(root / 'proc'),
                     'SESSIONDOCK_HISTORY_PAGE_EVENTS': '80'}

        def server():
            return isolated_server(corpus, binary, host_dir=root / 'host',
                                   state_dir=root / 'state', file_roots=(work,),
                                   trash_dir=root / 'trash', extra_env=extra_env)

        with private_temp(root), sync_playwright() as playwright:
            options = {'headless': True, 'env': {
                **os.environ, 'HOME': str(root / 'home'),
                'XDG_CONFIG_HOME': str(root / 'home/config'),
                'XDG_DATA_HOME': str(root / 'home/data'),
                'XDG_CACHE_HOME': str(root / 'home/cache')}}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                options['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            # Resolve the installed Chromium before changing HOME.
            browser = playwright.chromium.launch(**options)
            context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
            allowed_base = ['']
            context.route('**/*', lambda route: route.continue_()
                          if allowed_base[0] and route.request.url.startswith(allowed_base[0] + '/')
                          else route.abort())
            page = context.new_page()
            page.set_default_timeout(20000)
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))

            def row(sid):
                page.wait_for_function('sid => S.sessions.some(r => r.sid === sid && r.source === "agy")',
                    arg=sid)
                uid = page.evaluate('sid => S.sessions.find(r => r.sid === sid && r.source === "agy").uid', sid)
                return uid, page.locator(f'#side .item[data-uid="{uid}"]')

            def open_row(sid, marker):
                uid, item = row(sid)
                expect(item).to_be_visible()
                item.click()
                expect(page.locator('#msgs')).to_contain_text(marker)
                expect(page.locator('#detail .meta-source')).to_have_text('Agy')
                return uid

            def search(text, uid, excluded_uid):
                old = page.locator('#stat').get_attribute('data-seq') or ''
                page.locator('#q').fill(text)
                page.locator('#q').press('Enter')
                page.wait_for_function('old => String(document.querySelector("#stat").dataset.seq || "") !== old',
                                       arg=old)
                item = page.locator(f'#side .item[data-uid="{uid}"]')
                expect(item).to_be_visible()
                expect(item.locator('.m')).to_contain_text('命中')
                expect(page.locator(f'#side .item[data-uid="{excluded_uid}"]')).to_have_count(0)
                item.click()

            def clear_search():
                page.locator('#q').fill('')
                page.locator('#q').press('Enter')
                expect(page.locator('#side-search-state')).to_be_hidden()

            def reveal_thinking():
                process = page.locator('#msgs > .turn-process').filter(has_text='1 段思考')
                expect(process).to_have_count(1)
                if not process.locator('.turn-process-body').is_visible():
                    process.locator('.fold-toggle').click()
                expect(process.locator('.msg[data-role=thinking]')).to_contain_text(THINKING)

            try:
                with private_environment(root):
                    with server() as (base, opener):
                        allowed_base[0] = base
                        page.goto(base, wait_until='domcontentloaded')
                        uid, item = row(SEEDED)
                        decoy_uid, _ = row(DECOY)
                        page.locator('#view button[data-v="date"]').click()
                        expect(item).to_contain_text(TITLE)
                        expect(item.locator('.cwd')).to_have_attribute('title', str(work))
                        expect(item.locator('.cwd')).to_contain_text('中文 工作目录')
                        expect(item.locator('.source-icon[data-source="agy"]')).to_have_count(1)
                        uid = open_row(SEEDED, ANSWER)
                        expect(page.locator('#detail .dmeta')).to_contain_text(str(work))
                        user = page.locator('#msgs .msg[data-role=user]')
                        expect(user).to_contain_text(USER)
                        expect(user).not_to_contain_text('ADDITIONAL_METADATA')
                        expect(user).not_to_contain_text('2026-10-04T00:00:00Z')
                        assert not user.inner_text().startswith('<USER_REQUEST>\n')
                        image = user.locator('img')
                        expect(image).to_have_count(1)
                        image.scroll_into_view_if_needed()
                        page.wait_for_function('() => [...document.querySelectorAll("#msgs .msg[data-role=user] img")].some(img => img.complete && img.naturalWidth === 1)')
                        reveal_thinking()
                        payload = messages(opener, base, uid)
                        assert [m['text'] for m in payload['messages'] if m['role'] == 'user'] == [USER]
                        search('agyzebracorn', uid, decoy_uid)
                        expect(page.locator('#msgs')).to_contain_text(ANSWER)
                        clear_search()
                        search('agyquokkaridge', uid, decoy_uid)
                        reveal_thinking()
                        clear_search()
                        assert_native(native, expected_native, 'list/open/body+thinking search')
                        progress('Chinese title, file URI cwd, Agy source, user envelope, body and thinking search/render')

                        open_row(DECOY, '另一个回答')
                        if not page.locator('#a-turns').is_visible():
                            page.locator('#a-more').click()
                        if page.locator('#a-turns').get_attribute('aria-pressed') != 'true':
                            page.locator('#a-turns').click()
                        page.get_by_role('button', name='展开工具调用组', exact=True).click()
                        expect(page.locator('#msgs')).to_contain_text('fixture_read')
                        expect(page.locator('#msgs')).to_contain_text('fixture.txt')
                        expect(page.locator('#msgs')).to_contain_text('完整工具结果 agyrawresult')
                        expect(page.locator('#msgs')).to_contain_text('Error: model output must contain')
                        detail = messages(opener, base, decoy_uid)
                        assert any(m.get('error') and m['text'].startswith('Error:') for m in detail['messages'])
                        user = page.locator('#msgs .msg[data-role=user]')
                        expect(user).to_contain_text(TAG_USER)
                        expect(user).not_to_contain_text('USER_SETTINGS_CHANGE')
                        expect(user).not_to_contain_text(SETTINGS)
                        system = page.locator('#msgs .msg[data-role=system]')
                        expect(system).to_have_count(6)
                        expect(system.filter(has_text=SYSTEM_NOTICE)).to_be_visible()
                        expect(system.filter(has_text='agytasknotice')).to_be_visible()
                        expect(system.filter(has_text=SYSTEM_NOTICE)).not_to_contain_text('<SYSTEM_MESSAGE>')
                        expect(system.filter(has_text=SYSTEM_NOTICE)).not_to_contain_text('[Message] timestamp=')
                        expect(system.filter(has_text=SYSTEM_NOTICE).locator('.native-message-state')).to_have_text('系统消息')
                        expect(system.filter(has_text='agytasknotice').locator('.native-message-state')).to_have_attribute(
                            'title', re.compile('来源：fixture/task-46.*', re.S))
                        expect(system.filter(has_text=SETTINGS).locator('.native-message-state')).to_have_text('设置变更')
                        expect(system.filter(has_text=UNKNOWN_TAG).locator('.native-message-state')).to_have_text('附加信息 · EXTRA_NATIVE_TAG')
                        expect(system.filter(has_text='agysystembroken')).to_contain_text('<SYSTEM_MESSAGE>')
                        assert page.locator('#msgs .tool-msg .msg[data-role=system]').count() == 0
                        system_messages = [m for m in detail['messages'] if m['role'] == 'system']
                        assert len(system_messages) == 6 and all(m['counted'] is False for m in system_messages), system_messages
                        notice = next(m for m in system_messages if m['text'] == SYSTEM_NOTICE)
                        assert notice['native_type'] == 'SYSTEM_MESSAGE' and notice['system_sender'] == 'system', notice
                        assert notice['system_priority'] == 'MESSAGE_PRIORITY_LOW', notice
                        user_message = next(m for m in detail['messages'] if m['role'] == 'user')
                        assert user_message['native_metadata'][0]['tag'] == 'ADDITIONAL_METADATA', user_message
                        assert user_message['text'] == TAG_USER and user_message['native_type'] == 'USER_INPUT', user_message
                        assert all(m.get('native_type') for m in detail['messages']), detail['messages']
                        assert_native(native, expected_native, 'native type/tag classification')
                        progress('system/task notifications, settings changes, metadata and unknown tags are classified; literal/code tags and malformed envelopes stay intact')
                        open_row(SEEDED, ANSWER)
                        progress('documented raw tool JSON, unknown-step content and native ERROR_MESSAGE remain visible')

                        # Idle and an explicit no-op DB commit must not republish identical bytes.
                        wait_for(lambda: directory.joinpath('summary.json').is_file(), 'seed mirror')
                        stamps = mirror_stamps(directory)
                        execute(db, 'UPDATE conversation_summaries SET title=title WHERE conversation_id=?', (SEEDED,))
                        expected_native = native_snapshot(native)
                        page.wait_for_timeout(2300)  # Cross two real 1-second mirror polls.
                        assert mirror_stamps(directory) == stamps, 'unchanged content rewrote mirror'
                        assert_native(native, expected_native, 'idle/no-op commit')
                        progress('unchanged database content preserves summary/messages mtime')

                        # BUG-20261005-082202-dc28f8: a catalog entry can predate
                        # any readable transcript. Do not claim cached history exists.
                        index_uid, index_item = row(INDEX_ONLY)
                        index_item.click()
                        warning = page.locator('.native-history-warning')
                        expect(warning).to_contain_text('仅找到 Agy 的会话索引')
                        expect(warning).to_contain_text('没有已缓存的历史')
                        expect(warning).to_contain_text('请在 Agy 中确认该会话是否仍可打开')
                        expect(warning).not_to_contain_text('保留上次读取的历史')
                        expect(page.locator('#msgs .msg')).to_have_count(0)
                        index_detail = messages(opener, base, index_uid)
                        assert index_detail['messages'] == []
                        assert index_detail['meta']['migration_warnings'] == [warning.inner_text()]
                        assert_native(native, expected_native, 'index-only missing transcript')
                        write_transcript(native, INDEX_ONLY, [record(0, 'USER_INPUT', 'AGY FIRST READ')])
                        expected_native = native_snapshot(native)
                        expect(page.locator('#msgs')).to_contain_text('AGY FIRST READ')
                        expect(warning).to_have_count(0)
                        assert_native(native, expected_native, 'first readable transcript')
                        progress('index-only session explains absent body/cache; first native transcript appears live')

                        # A missing transcript is unavailable, not an empty conversation.
                        open_row(SEEDED, ANSWER)
                        native_bytes = path.read_bytes()
                        saved_mirror = directory.joinpath('messages.jsonl').read_bytes()
                        path.unlink()
                        expected_native = native_snapshot(native)
                        wait_for(lambda: json.loads(directory.joinpath('summary.json').read_text())
                                 .get('transcript_missing') is True, 'missing transcript warning')
                        expect(page.locator('.native-history-warning')).to_contain_text('保留上次读取的历史')
                        expect(page.locator('.native-history-warning')).not_to_contain_text('没有已缓存的历史')
                        cached_detail = messages(opener, base, uid)
                        assert cached_detail['meta']['migration_warnings'] == [
                            page.locator('.native-history-warning').inner_text()]
                        assert directory.joinpath('messages.jsonl').read_bytes() == saved_mirror
                        assert_native(native, expected_native, 'missing transcript read')
                        path.write_bytes(native_bytes)
                        expected_native = native_snapshot(native)
                        wait_for(lambda: json.loads(directory.joinpath('summary.json').read_text())
                                 .get('transcript_missing') is False, 'restored transcript')
                        expect(page.locator('.native-history-warning')).to_have_count(0)
                        assert_native(native, expected_native, 'restored transcript read')
                        progress('missing transcript retains cached history with notice; restoration clears notice')

                        # A complete JSON object without its LF remains an unfinished tail.
                        partial = encoded(record(2, 'USER_INPUT', 'AGY TORN RECORD'))
                        with path.open('ab') as stream:
                            stream.write(partial[:-1])
                        time.sleep(1.3)
                        expect(page.locator('#msgs')).not_to_contain_text('AGY TORN RECORD')
                        with path.open('ab') as stream:
                            stream.write(b'\n')
                            stream.write(encoded(record(3, 'PLANNER_RESPONSE', 'AGY COMPLETED TAIL')))
                        expect(page.locator('#msgs')).to_contain_text('AGY TORN RECORD')
                        expect(page.locator('#msgs')).to_contain_text('AGY COMPLETED TAIL')
                        write_transcript(native, SEEDED, seed_records)
                        expected_native = native_snapshot(native)
                        expect(page.locator('#msgs')).not_to_contain_text('AGY COMPLETED TAIL')
                        assert_native(native, expected_native, 'completed LF tail')
                        progress('unfinished JSONL tail stays hidden until LF; following records remain readable')

                        # Append with no catalog update; then rewrite an old same-size row.
                        before = messages(opener, base, uid)
                        with path.open('ab') as stream:
                            stream.write(encoded(record(2, 'USER_INPUT', '追加原生输入')))
                            stream.write(encoded(record(3, 'PLANNER_RESPONSE', '追加原生回答 agyappend')))
                        expected_native = native_snapshot(native)
                        expect(page.locator('#msgs')).to_contain_text('追加原生回答 agyappend')
                        appended = messages(opener, base, uid, **checkpoint(before))
                        assert not appended['reset'], appended
                        assert '追加原生回答 agyappend' in [m['text'] for m in appended['messages']]
                        before = messages(opener, base, uid)
                        previous_size = path.stat().st_size
                        path.write_bytes(path.read_bytes().replace(b'REWRITE_A', b'REWRITE_B'))
                        assert path.stat().st_size == previous_size
                        expected_native = native_snapshot(native)
                        expect(page.locator('#msgs')).to_contain_text('REWRITE_B')
                        expect(page.locator('#msgs')).not_to_contain_text('REWRITE_A')
                        assert messages(opener, base, uid, **checkpoint(before))['reset']
                        before = messages(opener, base, uid)
                        write_transcript(native, SEEDED, seed_records)
                        expected_native = native_snapshot(native)
                        expect(page.locator('#msgs')).to_contain_text(ANSWER)
                        expect(page.locator('#msgs')).not_to_contain_text('追加原生回答')
                        assert messages(opener, base, uid, **checkpoint(before))['reset']
                        assert_native(native, expected_native, 'append/rewrite/rewind')
                        progress('live native append, same-size old-row rewrite and truncation reset')

                        # Real history-gap clicks; HTTP checks only supplement visible rows.
                        pages_uid = open_row(PAGES, 'AGY PAGE 0479')
                        opening = messages(opener, base, pages_uid, window=1)
                        assert opening['partial'] and opening['partial']['omitted'] > 0
                        saved = checkpoint(opening)
                        cursor = opening['partial']['cursor']
                        reconstructed = list(opening['messages'][:opening['partial']['head']])
                        while cursor:
                            chunk = get_json(opener, base, '/api/messages/' + pages_uid + '/page?' + urlencode({'cursor': cursor}))
                            assert chunk['messages']
                            assert not {'version', 'anchor', 'reset'} & chunk.keys()
                            reconstructed.extend(chunk['messages'])
                            cursor = chunk['page']['next']
                        reconstructed.extend(opening['messages'][opening['partial']['head']:])
                        assert [m['text'] for m in reconstructed] == [f'AGY PAGE {index:04d}' for index in range(480)]
                        expect(page.locator('.history-gap-load')).to_be_visible()
                        with page.expect_response(lambda response: urlsplit(response.url).path.endswith('/page')) as loaded:
                            page.locator('.history-gap-load').click()
                        assert loaded.value.status == 200, loaded.value.text()
                        expect(page.locator('.history-gap')).to_have_count(0)
                        for marker in ('AGY PAGE 0000', 'AGY PAGE 0100', 'AGY PAGE 0479'):
                            expect(page.locator('#msgs')).to_contain_text(marker)
                        assert re.findall(r'AGY PAGE \d{4}', page.locator('#msgs').inner_text()) == [
                            f'AGY PAGE {index:04d}' for index in range(480)], 'visible history duplicated/reordered rows'
                        unchanged = messages(opener, base, pages_uid, **saved)
                        assert not unchanged['reset'] and unchanged['end'] == opening['end']
                        assert unchanged['anchor'] == opening['anchor']
                        page.reload(wait_until='domcontentloaded')
                        open_row(PAGES, 'AGY PAGE 0479')
                        expect(page.locator('.history-gap-load')).to_be_visible()
                        page.locator('.history-gap-load').click()
                        expect(page.locator('.history-gap')).to_have_count(0)
                        expect(page.locator('#msgs')).to_contain_text('AGY PAGE 0100')
                        with page_path.open('ab') as stream:
                            stream.write(encoded(record(480, 'USER_INPUT', '分页后的新输入')))
                        expected_native = native_snapshot(native)
                        expect(page.locator('#msgs')).to_contain_text('分页后的新输入')
                        expect(page.locator('#msgs')).to_contain_text('AGY PAGE 0100')
                        assert_native(native, expected_native, 'pagination/reload/live append')
                        progress('history-gap clicks, full page order, stable live checkpoint, refresh and append')

                        # Replace the actual catalog inode at the configured path.
                        replacement = root / 'replacement.db'
                        seed_catalog(replacement, work, title='替换数据库后的 Agy 标题')
                        os.replace(replacement, db)
                        expected_native = native_snapshot(native)
                        page.wait_for_function('title => S.sessions.some(r => r.source === "agy" && r.title === title)',
                            arg='替换数据库后的 Agy 标题')
                        open_row(SEEDED, ANSWER)
                        expect(page.locator('#side .item').filter(has_text='替换数据库后的 Agy 标题')).to_be_visible()
                        assert_native(native, expected_native, 'DB replacement')
                        stamps = mirror_stamps(directory)
                        progress('atomic native catalog replacement discovered on page')

                    # Leave only a mirror bearing our marker for restart cleanup.
                    orphan = mirror / 'cli' / ORPHAN
                    orphan.mkdir()
                    orphan.joinpath('summary.json').write_text(json.dumps({'format': 'sessiondock-agy-mirror'}))
                    orphan.joinpath('messages.jsonl').write_text('')
                    with server() as (base, opener):
                        allowed_base[0] = base
                        page.goto(base, wait_until='domcontentloaded')
                        restarted_uid = open_row(SEEDED, ANSWER)
                        assert restarted_uid == uid, 'service restart changed native identity'
                        wait_for(lambda: not orphan.exists(), 'owned stale mirror cleanup after restart')
                        assert mirror_stamps(directory) == stamps, 'restart rewrote identical mirror'
                        assert_native(native, expected_native, 'restart')

                        # Refuse unsupported native operations before touching either provider.
                        if not page.locator('#a-session-action').is_visible():
                            page.locator('#a-more').click()
                        page.locator('#a-session-action').click()
                        popup = page.locator('dialog.app-popup[open]')
                        expect(popup).to_contain_text('/resume')
                        popup.locator('[data-popup-action="ok"]').click()
                        refused = page.request.delete(base + '/api/session/' + uid + '?force=1')
                        assert refused.status == 409 and refused.json()['code'] == 'agy_delete_unsupported', refused.text()

                        def refuse_transfer():
                            if not page.locator('#a-clone-group').is_visible():
                                page.locator('#a-more').click()
                            page.locator('#a-clone-group').click()
                            dialog = page.locator('#clone-group-dialog')
                            expect(dialog.locator('.transfer-error')).to_contain_text('agy')
                            expect(dialog.locator('.clone-confirm')).to_be_disabled()
                            dialog.locator('input[name="transfer-mode"][value="move"]').check()
                            expect(dialog.locator('.clone-confirm')).to_be_disabled()
                            dialog.locator('.clone-cancel').click()

                        refuse_transfer()
                        parent_uid = corpus.uid(mixed_sid)
                        attached = page.request.post(base + '/api/session/nest', data={'uid': uid, 'parent_uid': parent_uid})
                        assert attached.status == 200, attached.text()
                        page.reload(wait_until='domcontentloaded')
                        page.locator(f'#side .item[data-uid="{parent_uid}"]').click()
                        expect(page.locator('#msgs')).to_contain_text('混合组父会话')
                        refuse_transfer()
                        moved = page.request.post(base + '/api/session/clone/plan', data={
                            'uid': parent_uid, 'mode': 'move', 'new_ids': False})
                        assert moved.status == 409 and 'agy' in moved.text(), moved.text()
                        detached = page.request.post(base + '/api/session/nest', data={'uid': uid, 'parent_uid': None})
                        assert detached.status == 200, detached.text()
                        assert mixed_path.read_bytes() == mixed_before
                        assert_native(native, expected_native, 'unsupported delete/clone/move')
                        progress('native delete guidance and whole mixed-provider clone/move refusal preserve all sources')

                        # Native deletion is performed by the fixture, never UI DELETE.
                        # Keep the transcript to prove the catalog is the authority.
                        clear_search()
                        open_row(DECOY, '另一个回答')
                        execute(db, 'DELETE FROM conversation_summaries WHERE conversation_id=?', (SEEDED,))
                        expected_native = native_snapshot(native)
                        page.wait_for_function('sid => !S.sessions.some(r => r.source === "agy" && r.sid === sid)',
                            arg=SEEDED)
                        expect(page.locator(f'#side .item[data-uid="{uid}"]')).to_have_count(0)
                        wait_for(lambda: not directory.exists(), 'deleted catalog row mirror cleanup')
                        assert path.exists(), 'ordinary read deleted native transcript'
                        assert_native(native, expected_native, 'catalog row deletion')
                        assert not errors, errors
                        progress('restart mtime/identity, stale mirror cleanup and native catalog deletion')
            finally:
                context.close()
                browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    run(args.binary)
    progress('Agy native read-only history browser complete; no native writes and no CLI launches')


if __name__ == '__main__':
    main()
