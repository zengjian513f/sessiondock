#!/usr/bin/env python3
"""Chromium: list summary projection agrees with ordinary synthetic records.

Open actual list rows and search results, including large unused nested fields,
strict discarded-field validation, duplicate keys, rewrites and partial lines.
No native homes, real CLIs, internal JS entry points or timing claims.
"""

import argparse
import json
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_fixtures import (BINARY, Corpus, batch35_agent_meta, batch35_meta,
                            claude_row, codex_message, codex_row, encoded,
                            isolated_server)


TITLE = 'Projection title Alpha'
ANSWER = 'Projection answer needle'
MODEL = 'projection-model'
UNRELATED = 'UNRELATED_FIELD_ONLY'


def unused_tree():
    # Tiny scalars/containers as well as strings: avoids testing only a giant
    # text field. This is well inside the existing summary tail window.
    return [{'text': UNRELATED + ' λ 🐈', 'items': [None, True, -0.0, 1e30],
             'object': {'nested': [index, 'escaped\\quote"']}}
            for index in range(1200)]


def padded(row, source, tree):
    row = json.loads(json.dumps(row))
    if source == 'claude' and 'message' in row:
        row['unused_root'] = {'text': UNRELATED}
        row['message']['usage'] = tree
    else:
        row['unused_root'] = tree
    if source == 'codex':
        row['payload']['unused_usage'] = tree
    return row


def invalid_rows():
    # Every malformed row crosses the small-record decoder threshold. A valid
    # discarded prefix cannot make its suffix syntactically/UTF-8 valid.
    prefix = b'{"type":"custom-title","customTitle":"INVALID TITLE","unused":"' + b'x' * 66000
    return [prefix + suffix for suffix in (
        b'\\uD800"}\n',                 # lone surrogate
        b'\xff"}\n',                   # invalid UTF-8
        b'","number":1e999}\n',         # unrepresentable number
        b'","array":[true,]}\n',        # trailing comma in skipped array
        b'","object":{"key":01}}\n',   # invalid skipped number
    )]


def deep_record(source):
    # The recursive serde fast path cannot decide this record. Both the
    # summary scanner and unchanged history scanner must accept the shape.
    return (b'{"type":' + json.dumps('file-history-snapshot' if source == 'claude'
                                      else 'event_msg').encode()
            + b',"payload":{"type":"token_count"},"unused":{"string":"'
            + ('λ🐈' * 11000).encode() + b'","deep":'
            + b'[' * 260 + b'{"duplicate":0,"duplicate":-0.0}' + b']' * 260 + b'}}\n')


def reply(source, sid, uid='a1', parent='a0'):
    if source == 'claude':
        row = claude_row(sid, 'assistant', uid, parent, ANSWER)
        row['message']['model'] = MODEL
        return row
    return codex_message('assistant', ANSWER)


def fixture(corpus):
    tree = unused_tree()
    for source in ('claude', 'codex', 'grok'):
        (corpus.root / source).mkdir(parents=True)
    for source in ('claude', 'codex'):
        for variant in ('plain', 'padded'):
            sid = source + '-' + variant
            if source == 'claude':
                rows = [claude_row(sid, 'user', 'u0', None, TITLE)]
            else:
                # If the projection trusts a syntactically invalid discarded
                # field, this head record incorrectly becomes the first title.
                invalid_prompt = (b'{"type":"response_item","payload":{"type":"message","role":"user",'
                                  b'"content":[{"type":"input_text","text":"INVALID TITLE"}]},"unused":"'
                                  + b'x' * 66000 + b'\\uD800"}\n')
                rows = [batch35_meta(sid), invalid_prompt, codex_message('user', TITLE),
                        codex_row('event_msg', {'type': 'task_started'})]
            answer = reply(source, sid, 'a0', 'u0')
            summary_answer = padded(answer, source, tree) if variant == 'padded' else answer
            if variant == 'padded':
                assert 64 * 1024 < len(encoded(summary_answer)) < 512 * 1024
            unexpected = {'type': 'file-history-snapshot' if source == 'claude' else 'event_msg',
                          'message': [{'unexpected': 'complete array fallback'}],
                          'payload': [{'unexpected': 'complete array fallback'}],
                          'unused': 'x' * 66000}
            rows += [summary_answer,
                     unexpected, deep_record(source), reply(source, sid)]
            if source == 'claude':
                # Duplicate retained keys, including escaped key spelling and
                # the whole message object: last value wins without merging.
                rows.append(b'{"type":"assistant","message":{"model":"discarded"},'
                            b'"message":{"model":"projection-model","content":"Projection answer needle",'
                            b'"stop_reason":"end_turn"},"uuid":"a2","parentUuid":"a1",'
                            b'"sessionId":' + json.dumps(sid).encode() + b',"unused":"' + b'x' * 66000 + b'"}\n')
                rows.append(b'{"type":"custom-title","customTitle":"WRONG TITLE",'
                            b'"custom\\u0054itle":"Projection title Alpha","unused":"' + b'x' * 66000 + b'"}\n')
            else:
                model = codex_row('turn_context', {'model': MODEL})
                rows += [padded(model, source, tree) if variant == 'padded' else model,
                         codex_row('event_msg', {'type': 'task_complete', 'error': {}})]
            corpus.put(sid, source, rows, [])
            if source == 'claude':
                # At the first list read, the large nested message and deep
                # record are complete in the tail, not clipped out of it.
                assert corpus.paths[sid].stat().st_size < 512 * 1024
            # Match mtime-derived fields of the two variants at initial read.
            os.utime(corpus.paths[sid], ns=(1800000000000000000, 1800000000000000000))

    # Ownership must survive projected session_meta payloads and Claude ids.
    owner = 'codex-padded'
    agent_meta = batch35_agent_meta('projection-agent', owner)
    # Complete metadata must fit the existing 96 KiB head window; keeping
    # it larger than 64 KiB still exercises the projected metadata fields.
    agent_meta['unused_root'] = {'text': 'x' * 70000}
    corpus.put('projection-agent', 'codex', [
        agent_meta,
        codex_message('user', 'Projected child prompt'),
        codex_message('assistant', 'Projected child answer'),
        codex_row('event_msg', {'type': 'task_complete'})], [], parent=owner)
    owner_path = corpus.paths['claude-padded']
    child = owner_path.with_suffix('') / 'subagents' / 'agent-worker.jsonl'
    child.parent.mkdir(parents=True)
    child.write_bytes(encoded(claude_row('claude-padded', 'user', 'cu', None,
                                        'Claude child prompt', isSidechain=True))
                      + encoded(padded(claude_row('claude-padded', 'assistant', 'ca', 'cu',
                                                 'Claude child answer', isSidechain=True), 'claude', tree)))
    child.with_suffix('.meta.json').write_text(json.dumps({'description': 'Projected Claude worker',
                                                          'agentType': 'reviewer'}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-summary-projection-') as temporary:
        corpus = Corpus(Path(temporary))
        fixture(corpus)
        # Use the shared read-only fixture; optional managed state is disabled.
        with isolated_server(corpus, args.binary) as (base, _), sync_playwright() as pw:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**launch)
            try:
                context = browser.new_context(viewport={'width': 1900, 'height': 1000},
                                              service_workers='block')
                context.route('**/*', lambda route: route.continue_()
                              if route.request.url.startswith(base + '/') else route.abort())
                page = context.new_page()
                errors = []
                page.on('pageerror', lambda error: errors.append(str(error)))

                def rows():
                    response = context.request.get(base + '/api/sessions?force=1')
                    assert response.ok, response.status
                    return {row['uid']: row for row in response.json()['sessions']}

                def refresh():
                    result = rows()
                    page.goto(base, wait_until='networkidle')
                    return result

                def item(sid):
                    return page.locator(f'#side .item[data-uid="{corpus.uid(sid)}"]:not(.agent)')

                def open_main(sid, title=TITLE, answer=ANSWER):
                    expect(item(sid).locator('.t')).to_have_text(title)
                    item(sid).click()
                    expect(page.locator('#detail > .dhead')).to_contain_text(title)
                    expect(page.locator('#detail > .dhead')).to_contain_text(MODEL)
                    expect(page.locator('#msgs')).to_contain_text(answer)
                    expect(page.locator('#migration-read-error')).to_have_count(0)

                listed = refresh()
                for source in ('claude', 'codex'):
                    plain, padded_sid = source + '-plain', source + '-padded'
                    left, right = listed[corpus.uid(plain)], listed[corpus.uid(padded_sid)]
                    for key in ('title', 'cwd', 'created', 'model', 'branch', 'supported', 'turn'):
                        assert left.get(key) == right.get(key), (source, key, left, right)
                    assert left['supported'] and left['title'] == TITLE and left['model'] == MODEL
                    for sid in (plain, padded_sid):
                        open_main(sid)
                        meta = context.request.get(base + '/api/messages/' + corpus.uid(sid)).json()['meta']
                        invalid_notes = [note for note in meta.get('migration_warnings', [])
                                         if '跳过无效的JSONL 记录' in note]
                        assert invalid_notes == (['跳过无效的JSONL 记录 ×1']
                                                 if source == 'codex' else []), meta
                    print(f'PASS {source}: ordinary/large summaries, strict bad-line and duplicate-key semantics, click history', flush=True)

                # Claude titles are last-value wins: accepting even the last
                # malformed row would visibly replace the valid custom title.
                # Refresh separately so the initial list also sees the large
                # nested message/payload records inside its bounded windows.
                for source in ('claude', 'codex'):
                    for variant in ('plain', 'padded'):
                        sid = source + '-' + variant
                        with corpus.paths[sid].open('ab') as stream:
                            stream.write(b''.join(invalid_rows()))
                listed = refresh()
                for source in ('claude', 'codex'):
                    for variant in ('plain', 'padded'):
                        sid = source + '-' + variant
                        assert listed[corpus.uid(sid)]['title'] == TITLE
                        assert listed[corpus.uid(sid)]['supported']
                        open_main(sid)
                        meta = context.request.get(base + '/api/messages/' + corpus.uid(sid)).json()['meta']
                        count = 6 if source == 'codex' else 5
                        assert f'跳过无效的JSONL 记录 ×{count}' in meta.get('migration_warnings', []), meta

                for source, agent, answer in (
                    ('claude', 'worker', 'Claude child answer'),
                    ('codex', 'projection-agent', 'Projected child answer'),
                ):
                    sid = source + '-padded'
                    parent = listed[corpus.uid(sid)]
                    assert len(parent['agent_items']) == 1, parent
                    assert parent['agent_items'][0]['id'] == agent, parent
                    open_main(sid)
                    page.locator('#a-view-switch').click()
                    page.locator(f'#session-view-menu button[data-agent="{agent}"]').click()
                    expect(page.locator('#msgs')).to_contain_text(answer)

                def search(query):
                    old = page.locator('#stat').get_attribute('data-seq') or ''
                    page.locator('#q').fill(query)
                    page.locator('#q').press('Enter')
                    page.wait_for_function("old => String(document.querySelector('#stat').dataset.seq || '') !== old", arg=old)
                    expect(page.locator('#search-progress')).not_to_be_visible()

                search(ANSWER)
                expect(page.locator('#side .item[data-uid]:not(.agent)')).to_have_count(4)
                for source in ('claude', 'codex'):
                    for variant in ('plain', 'padded'):
                        sid = source + '-' + variant
                        expect(item(sid).locator('.snip')).to_contain_text(ANSWER)
                        open_main(sid)
                search(UNRELATED)
                expect(page.locator('#side .item[data-uid]:not(.agent)')).to_have_count(0)
                page.locator('#side-search-exit').click()

                # Equal-size replacement invalidates summaries; opening and
                # searching afterward must agree with the changed list title.
                rewritten = TITLE.replace('Alpha', 'Omega')
                for sid in ('claude-plain', 'claude-padded', 'codex-plain', 'codex-padded'):
                    path = corpus.paths[sid]
                    old = path.read_bytes()
                    new = old.replace(TITLE.encode(), rewritten.encode())
                    assert len(new) == len(old) and new != old
                    path.write_bytes(new)
                listed = refresh()
                for sid in ('claude-plain', 'claude-padded', 'codex-plain', 'codex-padded'):
                    assert listed[corpus.uid(sid)]['title'] == rewritten
                    open_main(sid, rewritten)
                search(ANSWER)
                expect(page.locator('#side .item[data-uid]:not(.agent)')).to_have_count(4)
                open_main('claude-padded', rewritten)
                page.locator('#side-search-exit').click()

                # A projected rename and an unreadable content shape affect
                # neither row nor view before LF commits the entire record.
                sid = 'claude-padded'
                path = corpus.paths[sid]
                rename = encoded({'type': 'custom-title', 'customTitle': 'Committed projected title',
                                  'unused': unused_tree()})
                with path.open('ab') as stream:
                    stream.write(rename[:-1])
                listed = refresh()
                assert listed[corpus.uid(sid)]['title'] == rewritten
                open_main(sid, rewritten)
                with path.open('ab') as stream:
                    stream.write(b'\n')
                listed = refresh()
                assert listed[corpus.uid(sid)]['title'] == 'Committed projected title'
                open_main(sid, 'Committed projected title')
                broken = encoded({'type': 'user', 'sessionId': sid, 'message': {'content': 42},
                                  'unused': unused_tree()})
                with path.open('ab') as stream:
                    stream.write(broken[:-1])
                listed = refresh()
                assert listed[corpus.uid(sid)]['supported']
                open_main(sid, 'Committed projected title')
                with path.open('ab') as stream:
                    stream.write(b'\n')
                listed = refresh()
                assert not listed[corpus.uid(sid)]['supported'], listed[corpus.uid(sid)]
                item(sid).click()
                # A fresh page has no previous snapshot to keep. The existing
                # visible read error is rendered directly in the message pane.
                expect(page.locator('#detail')).to_contain_text('读取失败: Claude content 不是字符串、对象或数组')
                assert not errors, errors
                print('PASS projected agent ownership, actual search/click, same-size rewrite and partial-line commit', flush=True)
            finally:
                browser.close()


if __name__ == '__main__':
    main()
