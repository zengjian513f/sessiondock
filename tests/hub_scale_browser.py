#!/usr/bin/env python3
"""Large cross-node child counts and usable lists through a real Hub/Chromium.

Three private nodes contain synthetic native histories. Same-SID parents on
different nodes and providers must stay separate; children on two nodes add to
one parent. Reload timings and Hub RSS/HWM are observations, not machine gates.
"""

import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import statistics
import tempfile
import time
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from frontend_framework_browser import launch_chromium
from history_fixtures import BINARY, Corpus, codex_message, codex_row
from history_fixtures import isolated_server
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import TOKEN, node_env


def put(corpus, source, sid, title):
    if source == 'codex':
        rows = [codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/hub-scale'}),
                codex_message('user', title)]
    else:
        rows = [{'type': 'user', 'uuid': sid + '-message', 'parentUuid': None,
                 'sessionId': sid, 'cwd': '/synthetic/hub-scale',
                 'timestamp': '2026-09-11T10:00:00Z',
                 'message': {'role': 'user', 'content': title}}]
    corpus.put(sid, source, rows, [])
    return corpus.uid(sid)


def memory(pid):
    status = Path(f'/proc/{pid}/status')
    if not status.exists():
        return {}
    fields = dict(line.split(':', 1) for line in status.read_text().splitlines())
    return {key: round(int(fields[field].split()[0]) / 1024, 1)
            for key, field in [('rss_mib', 'VmRSS'), ('hwm_mib', 'VmHWM')]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--parents', type=int, default=800)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-hub-scale-') as temporary, \
            sync_playwright() as pw, ExitStack() as stack:
        root = Path(temporary)
        corpora = [Corpus(root / name) for name in ('a', 'b', 'c')]
        nodes = [SimpleNamespace(name=name, nid=name * 32, port=free_port(), token=TOKEN)
                 for name in ('a', 'b', 'c')]
        metadata = [{}, {}, {}]
        expected = {}
        target = child = None
        for index in range(args.parents):
            sid = f'scale-{index:04d}'
            for node_index, source, count in [(0, 'codex', 2), (0, 'claude', 0), (1, 'codex', 1)]:
                uid = put(corpora[node_index], source, sid, f'{nodes[node_index].name} {source} {sid}')
                uid = scoped(nodes[node_index].nid, uid)
                expected[uid] = count
                if index == 0 and node_index == 0 and source == 'codex':
                    target = uid
            for node_index, parent_index in [(1, 0), (2, 0), (2, 1)]:
                title = f'child {nodes[node_index].name} to {nodes[parent_index].name} {sid}'
                uid = put(corpora[node_index], 'codex', f'{sid}-child-{parent_index}', title)
                metadata[node_index][uid] = {
                    'nest_initialized': True,
                    'nest_parent': {'source': 'codex', 'sid': sid, 'node_id': nodes[parent_index].nid}}
                if index == 0 and node_index == 2 and parent_index == 0:
                    child = (scoped(nodes[node_index].nid, uid), title)
        for corpus, node, rows in zip(corpora, nodes, metadata):
            (corpus.root / 'state').mkdir()
            (corpus.root / 'ids').mkdir()
            (corpus.root / 'ids/node-id').write_text(node.nid + '\n')
            (corpus.root / 'state/session-metadata.json').write_text(json.dumps({
                'schema_version': 1, 'revision': 1, 'sessions': rows}))
            stack.enter_context(isolated_server(corpus, args.binary, state_dir=corpus.root / 'state',
                extra_env=node_env(corpus.root, node.port, '127.0.0.0/8')))
        hubroot = root / 'hub'
        hubroot.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, nodes)
        hub.env['TOKIO_WORKER_THREADS'] = '2'
        hub.start()
        stack.callback(hub.stop)
        browser = launch_chromium(pw)
        stack.callback(browser.close)
        context = browser.new_context(service_workers='block', viewport={'width': 1280, 'height': 900})
        stack.callback(context.close)
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        timings = []
        for sample in range(5):
            start = time.perf_counter()
            if sample:
                page.reload()
            else:
                page.goto(f'http://127.0.0.1:{hub.port}')
                page.wait_for_function('S.sessions.length > 0')
                page.locator('#nest-hidden').click()
            try:
                page.wait_for_function('count => S.sessions.length === count && S.sessions.every(row => Number.isInteger(row.child_count))', arg=len(expected), timeout=30000)
            except Exception:
                print('list diagnostics', page.evaluate('({mode:S.childMode, count:S.sessions.length, rows:S.sessions.slice(0,12).map(s => ({uid:s.uid,sid:s.sid,parent:s.nest_parent,count:s.child_count}))})'), errors, flush=True)
                raise
            expect(page.locator('#side .item').first).to_be_visible()
            timings.append(round((time.perf_counter() - start) * 1000, 1))
            actual = page.evaluate('Object.fromEntries(S.sessions.map(row => [row.uid, row.child_count]))')
            assert actual == expected, ('cross-node/provider counts differ',
                [(uid, count, actual.get(uid)) for uid, count in expected.items()
                 if actual.get(uid) != count][:12])
            print(f'PASS hub scale sample={sample + 1} parents={len(expected)} list_ms={timings[-1]}', flush=True)
        # A real filter and disclosure must load exactly the selected remote
        # branch. Same-SID parents elsewhere must not gain any of its children.
        page.locator('#q').fill('a codex scale-0000')
        parent = page.locator(f'#side .item[data-uid="{target}"]')
        expect(parent).to_be_visible()
        parent.locator('.nest-caret').click()
        page.wait_for_function('count => S.sessions.length === count', arg=len(expected) + 2)
        page.locator('#q').fill(child[1])
        page.locator(f'#side .item[data-uid="{child[0]}"]').click()
        expect(page.locator('#msgs')).to_contain_text(child[1])
        assert not errors, errors
        print('METRICS ' + json.dumps({'suite': 'hub_scale_browser', 'parents': len(expected),
              'samples': len(timings), 'list_ms': timings, 'list_p50_ms': statistics.median(timings),
              'list_p95_ms': max(timings), **memory(hub.process.pid)}), flush=True)
        print('PASS hub scale: exact summed counts, node/source isolation, reload, filter, expand and open', flush=True)


if __name__ == '__main__':
    main()
