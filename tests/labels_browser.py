#!/usr/bin/env python3
"""Node-owned labels/groups through Chromium, real nodes and Hub union/downsync."""
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_parity import BINARY, Corpus, codex_row, isolated_server
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-labels-browser-') as temporary, sync_playwright() as pw:
        root = Path(temporary)
        corpora, nodes, states = [], [], []
        for name in ('a', 'b'):
            corpus = Corpus(root / name)
            for sid in ('same', 'other'):
                corpus.put(sid, 'codex', [codex_row('session_meta', {'id': sid, 'cwd': '/synthetic', 'timestamp': '2026-09-30T00:00:00Z'}),
                    codex_row('response_item', {'type': 'message', 'role': 'user', 'content': f'{name} {sid}'})], [])
            state = corpus.root / 'state'
            state.mkdir()
            (state / 'session-metadata.json').write_text(json.dumps({'schema_version': 1, 'revision': 1,
                'sessions': {corpus.uid('same'): {'starred': True, 'starred_at': 1}}}))
            ids = corpus.root / 'ids'; ids.mkdir()
            (ids / 'node-id').write_text(name * 32 + '\n')
            corpora.append(corpus); states.append(state)
            nodes.append(SimpleNamespace(name=name, nid=name * 32, port=free_port(), token=TOKEN))
        native = {p: p.read_bytes() for c in corpora for p in c.paths.values()}
        launch = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**launch)
        contexts, stacks = [], [ExitStack(), ExitStack()]
        hub = None
        errors = []
        def start_node(i):
            return stacks[i].enter_context(isolated_server(corpora[i], args.binary, state_dir=states[i],
                extra_env=node_env(corpora[i].root, nodes[i].port, '127.0.0.0/8')))[0]
        def page_at(base, mobile=False):
            context = browser.new_context(service_workers='block', viewport={'width': 390 if mobile else 1280, 'height': 900}, has_touch=mobile)
            contexts.append(context)
            context.route('**/*', lambda route: route.continue_() if route.request.url.startswith(base + '/') else route.abort())
            page = context.new_page(); page.on('pageerror', lambda error: errors.append(str(error)))
            page.goto(base, wait_until='networkidle')
            page.wait_for_function('SessionDockLabels.available && S.sessions.length > 0')
            return page
        def edit(page, uid, hold=False):
            item = page.locator(f'#side .item[data-uid="{uid}"]')
            if hold:
                item.dispatch_event('pointerdown', {'pointerType': 'touch', 'pointerId': 1, 'button': 0, 'clientX': 30, 'clientY': 300})
                expect(page.locator('#item-menu')).to_be_visible()
                item.dispatch_event('pointerup', {'pointerType': 'touch', 'pointerId': 1})
                item.dispatch_event('click')
            else:
                item.click(button='right')
            page.locator('#item-menu [data-act="labels"]').click()
            expect(page.locator('#session-label-save')).to_be_enabled()
        def create(page, kind, name):
            page.locator(f'#session-{kind}-new').fill(name)
            page.locator(f'#session-{kind}-create').click()
            expect(page.locator('#session-label-save')).to_be_enabled()
        def save(page):
            page.locator('#session-label-save').click()
            expect(page.locator('#session-label-dialog')).to_be_hidden(timeout=20000)
        def stored(i):
            return json.loads((states[i] / 'session-metadata.json').read_text())
        try:
            bases = [start_node(i) for i in range(2)]
            local = [page_at(base) for base in bases]
            uid_a, uid_b = corpora[0].uid('same'), corpora[1].uid('same')
            edit(local[0], uid_a)
            create(local[0], 'label', '重要')
            create(local[0], 'group', '待办')
            save(local[0])
            expect(local[0].locator(f'#side .item[data-uid="{uid_a}"] .session-row-labels')).to_contain_text('重要')
            local[0].locator('#view [data-v="group"]').click()
            expect(local[0].locator('#side .gname').filter(has_text='待办')).to_be_visible()
            local[0].locator('#session-group-filter').select_option('待办')
            expect(local[0].locator('#side .item')).to_have_count(1)
            local[0].locator('#session-group-filter').select_option('')
            edit(local[1], uid_b)
            create(local[1], 'label', '稍后'); save(local[1])
            assert stored(0)['sessions'][uid_a]['starred'] is True
            assert '重要' not in stored(1)['label_catalog']['labels']
            hubroot = root / 'hub'; hubroot.mkdir()
            hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, nodes)
            hub.start()
            page = page_at(f'http://127.0.0.1:{hub.port}')
            expect(page.locator('#session-label-chips button[data-label="重要"]')).to_be_visible()
            expect(page.locator('#session-label-chips button[data-label="稍后"]')).to_be_visible()
            for i in range(2):
                assert set(stored(i)['label_catalog']['labels']) == {'重要', '稍后'}
                assert stored(i)['label_catalog']['groups'] == ['待办']
            a, b = scoped(nodes[0].nid, uid_a), scoped(nodes[1].nid, uid_b)
            # Right-click isolates one tag. Normal clicks combine tags (OR).
            page.locator('#session-label-chips button[data-label="重要"]').click(button='right')
            expect(page.locator('#side .item')).to_have_count(1)
            expect(page.locator(f'#side .item[data-uid="{a}"]')).to_be_visible()
            page.locator('#session-label-chips button[data-label="稍后"]').click()
            expect(page.locator('#side .item')).to_have_count(2)
            later = page.locator('#session-label-chips button[data-label="稍后"]')
            later.dispatch_event('pointerdown', {'pointerType': 'touch', 'pointerId': 2, 'button': 0, 'clientX': 30, 'clientY': 200})
            page.wait_for_timeout(550)
            later = page.locator('#session-label-chips button[data-label="稍后"]')
            later.dispatch_event('pointerup', {'pointerType': 'touch', 'pointerId': 2})
            later.click()
            expect(page.locator('#side .item')).to_have_count(1)
            expect(page.locator(f'#side .item[data-uid="{b}"]')).to_be_visible()
            page.locator('#session-label-chips button[data-all]').click()
            # Batch adds a tag while retaining different original tags/groups.
            page.locator(f'#side .item[data-uid="{a}"]').click(button='right')
            page.locator('#item-menu [data-act="pick"]').click()
            page.locator(f'#side .item[data-uid="{b}"]').click()
            page.locator('#side-pick-labels').click()
            expect(page.locator('#session-label-save')).to_be_enabled()
            assert page.locator('#session-label-options input[data-label="重要"]').evaluate('(e) => e.indeterminate')
            create(page, 'label', '搁置'); save(page)
            assert set(stored(0)['sessions'][uid_a]['labels']) == {'重要', '搁置'}
            assert set(stored(1)['sessions'][uid_b]['labels']) == {'稍后', '搁置'}
            assert stored(0)['sessions'][uid_a]['group'] == '待办'
            assert 'group' not in stored(1)['sessions'][uid_b]
            page.locator('#side-pick-cancel').click()
            # Mobile session hold opens the same editor; remove a label/group.
            mobile = page_at(bases[0], mobile=True)
            edit(mobile, uid_a, hold=True)
            mobile.locator('#session-label-options input[data-label="重要"]').uncheck()
            mobile.locator('#session-label-group').select_option('')
            save(mobile)
            assert stored(0)['sessions'][uid_a]['labels'] == ['搁置']
            assert 'group' not in stored(0)['sessions'][uid_a]
            # Offline node does not block creation; the catalog lives on A.
            for context in contexts: context.close()
            contexts.clear(); stacks[1].close()
            page = page_at(f'http://127.0.0.1:{hub.port}')
            edit(page, a); create(page, 'label', '离线补同步')
            expect(page.locator('#session-label-note')).to_contain_text('恢复连接')
            save(page)
            assert '离线补同步' not in stored(1)['label_catalog']['labels']
            for context in contexts: context.close()
            contexts.clear(); hub.stop(); hub.start()
            # Rejoin without a Hub page: background union sync repairs B.
            bases[1] = start_node(1)
            rejoined = page_at(bases[1])
            expect(rejoined.locator('#session-label-chips button[data-label="离线补同步"]')).to_be_visible(timeout=25000)
            for context in contexts: context.close()
            contexts.clear(); hub.stop()
            for stack in stacks: stack.close()
            # Both nodes retain catalogs/assignments after restart with no Hub.
            bases = [start_node(i) for i in range(2)]
            for i, base in enumerate(bases):
                solo = page_at(base)
                expect(solo.locator('#session-label-chips button[data-label="离线补同步"]')).to_be_visible()
                assert stored(i)['sessions'][corpora[i].uid('same')]['starred'] is True
            assert all(p.read_bytes() == raw for p, raw in native.items())
            assert not errors, errors
            print('PASS labels browser: standalone creation, groups, badges, exclusive right-click/hold filtering, cross-node batch, tri-state preservation, mobile edit, offline/rejoin union, Hub/node restarts, native files unchanged')
        finally:
            for context in contexts: context.close()
            if hub: hub.stop()
            for stack in stacks: stack.close()
            browser.close()


if __name__ == '__main__':
    main()
