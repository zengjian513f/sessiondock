#!/usr/bin/env python3
"""Chromium: latest native Claude/Codex models reach list and title without reopening."""

import argparse
import os
from pathlib import Path
import tempfile

from playwright.sync_api import expect, sync_playwright
from history_parity import (BINARY, Corpus, batch35_meta, claude_row, codex_message,
                            codex_row, encoded, isolated_server)


def model_row(source, sid, model, index=0, **extra):
    if source == 'codex':
        return codex_row('turn_context', {'model': model})
    row = claude_row(sid, 'assistant', f'a{index}', 'u0', f'Model reply {index}', **extra)
    row['message']['model'] = model
    return row


def padding(source, count):
    # Put the latest model outside both metadata windows; include a record
    # crossing reverse-read chunks, then many short complete records.
    if source == 'codex':
        row = codex_row('event_msg', {'type': 'token_count', 'padding': 'x' * count})
    else:
        row = {'type': 'file-history-snapshot', 'snapshot': {'padding': 'x' * count}}
    return encoded(row)


def wait_model(page, uid, model):
    page.wait_for_function('({uid, model}) => S.sessions.find(s => s.uid === uid)?.model === model',
                           arg={'uid': uid, 'model': model}, timeout=20000)
    expect(page.locator(f'#side .item[data-uid="{uid}"] .m')).to_contain_text(model)
    expect(page.locator('#detail > .dhead')).to_contain_text(model)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-session-model-') as temporary:
        corpus = Corpus(Path(temporary))
        for source in ('claude', 'codex', 'grok'):
            (corpus.root / source).mkdir(parents=True, exist_ok=True)
        for source in ('claude', 'codex'):
            sid = source + '-model'
            head = ([batch35_meta(sid), codex_message('user', 'Codex model title')]
                    if source == 'codex' else [claude_row(sid, 'user', 'u0', None, 'Claude model title')])
            corpus.put(sid, source, head + [model_row(source, sid, 'initial-model')], [])
            with corpus.paths[sid].open('ab') as stream:
                stream.write(padding(source, 110000))
                stream.write(encoded(model_row(source, sid, 'current-model', 1)))
                stream.write(padding(source, 600000))
                stream.write(padding(source, 1000) * 20)
        corpus.put('claude-empty', 'claude', [claude_row('claude-empty', 'user', 'u0', None, 'No model yet')], [])
        agent_dir = corpus.paths['claude-model'].with_suffix('') / 'subagents'
        agent_dir.mkdir(parents=True)
        agent_path = agent_dir / 'agent-worker.jsonl'
        agent_path.write_bytes(encoded(claude_row('claude-model', 'user', 'u0', None, 'Worker', isSidechain=True))
                               + encoded(model_row('claude', 'claude-model', 'worker-model', isSidechain=True)))
        agent_path.with_suffix('.meta.json').write_text('{"description":"Worker","agentType":"reviewer"}')
        with isolated_server(corpus, args.binary) as (base, _), sync_playwright() as pw:
            launch = {'headless': True}
            if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
                launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
            browser = pw.chromium.launch(**launch)
            context = browser.new_context(viewport={'width': 1900, 'height': 1000}, service_workers='block')
            page = context.new_page()
            errors = []
            page.on('pageerror', lambda error: errors.append(str(error)))
            page.goto(base, wait_until='networkidle')
            page.wait_for_function('S.sessions.length === 3 && T.listLoaded')
            for source in ('claude', 'codex'):
                sid, uid = source + '-model', corpus.uid(source + '-model')
                path = corpus.paths[sid]
                page.locator(f'#side .item[data-uid="{uid}"]').click()
                wait_model(page, uid, 'current-model')
                assert context.request.get(base + '/api/messages/' + uid).json()['meta']['model'] == 'current-model'
                if source == 'claude':
                    row = next(r for r in context.request.get(base + '/api/sessions').json()['sessions'] if r['uid'] == uid)
                    assert row['agent_items'][0]['model'] == 'worker-model', row
                    with path.open('ab') as stream:
                        stream.write(encoded(model_row(source, sid, '<synthetic>', 2)))
                        stream.write(encoded(model_row(source, sid, 'sidechain-model', 3, isSidechain=True)))
                    page.evaluate('loadSessions(true)')
                    wait_model(page, uid, 'current-model')
                # A list-only update must repaint the open title. Block detail
                # refreshes so HTTP/SSE message rendering cannot mask this regression.
                page.evaluate('closeWatch()')
                page.route('**/api/messages/**', lambda route: route.abort())
                page.route('**/api/watch?*', lambda route: route.abort())
                new = encoded(model_row(source, sid, 'switched-model', 4))
                with path.open('ab') as stream:
                    stream.write(new[:-1])
                page.evaluate('loadSessions(true)')
                wait_model(page, uid, 'current-model')
                with path.open('ab') as stream:
                    stream.write(b'\n')
                page.evaluate('loadSessions(true)')
                wait_model(page, uid, 'switched-model')
                # Unrelated complete records retain the scalar cached model.
                with path.open('ab') as stream:
                    stream.write(padding(source, 1000))
                page.evaluate('loadSessions(true)')
                wait_model(page, uid, 'switched-model')
                # Equal-size rewriting and inode replacement must invalidate it.
                path.write_bytes(path.read_bytes().replace(b'switched-model', b'rewriteX-model'))
                page.evaluate('loadSessions(true)')
                wait_model(page, uid, 'rewriteX-model')
                replacement = path.with_suffix('.replacement')
                replacement.write_bytes(path.read_bytes().replace(b'rewriteX-model', b'replaced-model'))
                replacement.replace(path)
                page.evaluate('loadSessions(true)')
                wait_model(page, uid, 'replaced-model')
                page.unroute('**/api/messages/**')
                page.unroute('**/api/watch?*')
                # Normal background observation must pick up a further switch,
                # without a forced list refresh or reopening the selected row.
                page.evaluate('uid => watchSession(uid)', uid)
                with path.open('ab') as stream:
                    stream.write(encoded(model_row(source, sid, 'observed-model', 5)))
                wait_model(page, uid, 'observed-model')
                print(f'PASS {source}: cold model beyond windows, list/title updates, partial lines, rewrites and observation', flush=True)
            empty_uid = corpus.uid('claude-empty')
            page.locator(f'#side .item[data-uid="{empty_uid}"]').click()
            expect(page.locator('#detail > .dhead')).to_contain_text('No model yet')
            assert page.evaluate('uid => S.sessions.find(s => s.uid === uid).model', empty_uid) is None
            assert not errors, errors
            browser.close()
    print('PASS latest native session models browser', flush=True)


if __name__ == '__main__':
    main()
