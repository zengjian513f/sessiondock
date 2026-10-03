#!/usr/bin/env python3
"""Cross-node manual nesting through Chromium, a real hub and two real isolated nodes."""
from browser_runtime import js
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import sync_playwright
from history_parity import BINARY, Corpus, codex_row, isolated_server
from hub_http_suite import Hub, free_port, scoped
from nest_tree_browser import open_item_menu
from node_auth_suite import node_env, TOKEN


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-hub-nest-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        corpora, nodes = [], []
        for name in ('a', 'b'):
            corpus = Corpus(root / name)
            for sid in ('same', 'child'):
                corpus.put(sid, 'codex', [
                    codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/nest', 'timestamp': '2026-09-11T10:00:00Z'}),
                    codex_row('response_item', {'type': 'message', 'role': 'user', 'content': f'{name} {sid}'})], [])
            (corpus.root / 'state').mkdir()
            if name == 'b':
                (corpus.root / 'state/session-metadata.json').write_text(json.dumps({
                    'schema_version': 1, 'revision': 1, 'sessions': {
                        corpus.uid('child'): {'spawned_by': {'source': 'codex', 'sid': 'same'}}}}))
            corpora.append(corpus)
            nodes.append(SimpleNamespace(name=name, nid=name * 32, port=free_port(), token=TOKEN))
            ids = corpus.root / 'ids'
            ids.mkdir()
            (ids / 'node-id').write_text(name * 32 + '\n')
        hub = None
        launch = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**launch)
        try:
            parent = scoped(nodes[0].nid, corpora[0].uid('same'))
            child = scoped(nodes[1].nid, corpora[1].uid('child'))
            twin = scoped(nodes[1].nid, corpora[1].uid('same'))
            expected = {'source': 'codex', 'sid': 'same', 'node_id': nodes[0].nid}
            for restart in (False, True):
                with ExitStack() as stack:
                    locals_ = []
                    for corpus, node in zip(corpora, nodes):
                        locals_.append(stack.enter_context(isolated_server(corpus, args.binary, state_dir=corpus.root / 'state',
                            extra_env=node_env(corpus.root, node.port, '127.0.0.0/8'))))
                    if hub is None:
                        hubroot = root / 'hub'
                        hubroot.mkdir()
                        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, nodes)
                    hub.start()
                    stack.callback(hub.stop)
                    context = browser.new_context(service_workers='block', viewport={'width': 1280, 'height': 900})
                    stack.callback(context.close)
                    page = context.new_page()
                    page.goto(f'http://127.0.0.1:{hub.port}', wait_until='networkidle')
                    page.wait_for_function(js('S.sessions.length === 4', 'runtime.core.state.catalog.sessions.length === 4'))
                    if not page.evaluate(js('S.nest', 'runtime.core.state.sidebar.nest')):
                        page.locator('#nest-toggle').click()
                    def depth(uid, value):
                        page.wait_for_function('([uid, depth]) => document.querySelector(`#side .item[data-uid="${uid}"]`)?.dataset.depth === String(depth)', arg=[uid, value])
                    def action(uid, act):
                        open_item_menu(page, uid)
                        page.locator(f'#item-menu [data-act="{act}"]').click()
                    if not restart:
                        action(child, 'attach')
                        page.locator(f'#side .item[data-uid="{parent}"]').click()
                    depth(child, 1)
                    depth(twin, 0)
                    page.wait_for_function(js('([uid, node]) => S.sessions.find(s => s.uid === uid).nest_parent?.node_id === node', '([uid, node]) => runtime.core.state.catalog.sessions.find(s => s.uid === uid).nest_parent?.node_id === node'), arg=[child, nodes[0].nid])
                    assert page.evaluate(js('uid => S.sessions.find(s => s.uid === uid).nest_parent', 'uid => runtime.core.state.catalog.sessions.find(s => s.uid === uid).nest_parent'), child) == expected
                    saved = json.loads((corpora[1].root / 'state/session-metadata.json').read_text())
                    assert saved['sessions'][corpora[1].uid('child')]['nest_parent'] == expected
                    status, _, raw = hub.request('POST', '/api/session/nest', {'uid': parent, 'parent_uid': child})
                    assert status == 400 and '子会话' in json.loads(raw)['error'], (status, raw)
                    # Unauthenticated local callers cannot forge a resolved remote parent.
                    response = context.request.post(locals_[1][0] + '/api/session/nest', data={'uid': corpora[1].uid('child'), 'remote_parent': expected})
                    assert response.status == 403, response.text()
                    if restart:
                        action(child, 'detach')
                        depth(child, 0)
                        action(child, 'attach')
                        page.locator(f'#side .item[data-uid="{parent}"]').click()
                        page.wait_for_function(js('uid => !!S.sessions.find(s => s.uid === uid).nest_parent', 'uid => !!runtime.core.state.catalog.sessions.find(s => s.uid === uid).nest_parent'), arg=child)
                        depth(child, 1)
                        action(child, 'attach')
                        page.locator(f'#side .item[data-uid="{twin}"]').click()
                        depth(child, 1)
                        page.wait_for_function(js('uid => S.sessions.find(s => s.uid === uid).nest_parent?.sid === "same" && !S.sessions.find(s => s.uid === uid).nest_parent?.node_id', 'uid => runtime.core.state.catalog.sessions.find(s => s.uid === uid).nest_parent?.sid === "same" && !runtime.core.state.catalog.sessions.find(s => s.uid === uid).nest_parent?.node_id'), arg=child)
                        assert 'node_id' not in page.evaluate(js('uid => S.sessions.find(s => s.uid === uid).nest_parent', 'uid => runtime.core.state.catalog.sessions.find(s => s.uid === uid).nest_parent'), child)
                    print('PASS cross-node nest: ' + ('node/hub restart, detach, restore, local reattach' if restart else 'click attach, same-SID isolation, cycle, trusted descriptor'), flush=True)
        finally:
            browser.close()
            if hub:
                hub.stop()


if __name__ == '__main__':
    main()
