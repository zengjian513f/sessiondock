"""Select native units in Chromium, move across private hosts, retain nesting."""
# run_validation: skip
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace

from playwright.sync_api import expect, sync_playwright
from history_fixtures import BINARY, Corpus, codex_row, codex_message, isolated_server
from hub_fixtures import Hub, free_port, scoped
from node_auth_fixtures import node_env, TOKEN
from opencode_sdk_fixtures import FakeOpenCode, SID
from session_bundle_browser import Peer
from session_transfer_browser import ident
from transfer_ui import select_target


def corpus_at(root, count=5):
    corpus = Corpus(root)
    for name in ('claude', 'codex', 'grok', 'workspace', 'state', 'proc', 'ids', 'trash'):
        (root/name).mkdir(parents=True, exist_ok=True)
    for number in range(1, count + 1):
        meta = {'id': ident(number), 'cwd': str(root/'workspace'), 'timestamp': '2026-11-01T00:00:00Z'}
        if number == 4:
            meta['parent_thread_id'] = ident(2)
            meta['source'] = {'subagent': {'thread_spawn': {'parent_thread_id': ident(2)}}}
        if number == 5:
            meta['forked_from_id'] = ident(1)
        corpus.put(str(number), 'codex', [codex_row('session_meta', meta),
            codex_message('user', f'研究会话 {number}：保留父子归属与跨机器历史'),
            codex_message('assistant', f'会话 {number} 的独立历史')], [])
        path = corpus.paths[str(number)]
        native_path = root/'codex/sessions'/f'rollout-2026-11-01T00-00-00-{ident(number)}.jsonl'
        native_path.parent.mkdir(exist_ok=True)
        path.rename(native_path)
        corpus.paths[str(number)] = native_path
    return corpus


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument('--peer', default='lyra', help='Private SSH peer with the shared checkout')
    parser.add_argument('--screenshots', type=Path, default=Path('target/selected-move-review'))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='sessiondock-selected-move-') as temporary, sync_playwright() as pw, ExitStack() as stack:
        root = Path(temporary)
        source = corpus_at(root/'source')
        third = corpus_at(root/'third', 1)
        nodes = [SimpleNamespace(name=name, nid=letter*32, port=free_port(), token=TOKEN)
                 for name, letter in (('源机器', 'a'), ('目标机器', 'b'), ('第三节点', 'c'))]
        for corpus, node in ((source, nodes[0]), (third, nodes[2])):
            (corpus.root/'ids/node-id').write_text(node.nid+'\n')
        metadata = source.root/'state/session-metadata.json'
        metadata.write_text(json.dumps({'schema_version': 1, 'revision': 1, 'sessions': {
            source.uid('1'): {'fork_parent_visible': True},
            source.uid('2'): {'nest_parent': {'source': 'codex', 'sid': ident(1)}, 'nest_initialized': True},
            source.uid('3'): {'nest_parent': {'source': 'codex', 'sid': ident(2)}, 'nest_initialized': True}}}))
        (third.root/'state/session-metadata.json').write_text(json.dumps({'schema_version': 1, 'revision': 1,
            'sessions': {third.uid('1'): {'nest_parent': {'source': 'codex', 'sid': ident(1), 'node_id': nodes[0].nid},
                                        'nest_initialized': True}}}))
        remote = Peer(args.peer, root, source,
                      {kind: str(source.root/kind) for kind in ('claude', 'codex', 'grok')}, nodes[1], args.binary)
        stack.callback(remote.close)
        remote.call('start')
        fixture_root = source.root/'opencode-fixture'
        fixture_root.mkdir()
        fake = stack.enter_context(FakeOpenCode(fixture_root))
        env = node_env(source.root, nodes[0].port, '127.0.0.0/8')
        env.update(SESSIONDOCK_PROC_ROOT=str(source.root/'proc'), SESSIONDOCK_OPENCODE_DB=str(fake.db),
                   SESSIONDOCK_OPENCODE_ROOT=str(source.root/'opencode-mirror'))
        base, _ = stack.enter_context(isolated_server(source, args.binary, state_dir=source.root/'state',
            trash_dir=source.root/'trash', extra_env=env))
        stack.enter_context(isolated_server(third, args.binary, state_dir=third.root/'state',
            trash_dir=third.root/'trash', extra_env=node_env(third.root, nodes[2].port, '127.0.0.0/8')))
        hubroot = root/'hub'
        hubroot.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, nodes)
        hub.start()
        stack.callback(hub.stop)
        browser = pw.chromium.launch(headless=True)
        stack.callback(browser.close)
        context = browser.new_context(viewport={'width': 1280, 'height': 900}, service_workers='block')
        stack.callback(context.close)
        page = context.new_page()
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.set_default_timeout(20000)
        page.goto(f'http://127.0.0.1:{hub.port}', wait_until='networkidle')
        catalog = page.request.get(base+'/api/sessions?force=1').json()['sessions']
        unsupported = next(row['uid'] for row in catalog if row['source'] == 'opencode')
        response = page.request.post(base+'/api/session/nest', data={'uid': unsupported, 'parent_uid': source.uid('1')})
        assert response.ok, response.text()
        page.reload(wait_until='networkidle')
        selected = scoped(nodes[0].nid, source.uid('1'))

        def choose(uid):
            if page.locator('#detail .mobile-back').is_visible():
                page.locator('#detail .mobile-back').click()
            row = page.locator(f'#side .item[data-uid="{uid}"]')
            if not row.is_visible() and page.locator('#nest-toggle').get_attribute('aria-pressed') == 'true':
                page.locator('#nest-toggle').click()
            row.click(button='right')
            page.locator('#item-menu [data-act="clone"]').click()
            dialog = page.locator('#clone-group-dialog')
            expect(dialog.locator('.transfer-tree input[data-uid]').first).to_be_attached()
            return dialog

        def checkbox(dialog, uid):
            return dialog.locator(f'.transfer-tree input[data-uid="{uid}"]')

        # Inspect the full tree in both responsive layouts, with default selections,
        # disabled unsupported/other-node members and linked native dependencies.
        for width, theme in ((1280, 'light'), (390, 'dark')):
            page.set_viewport_size({'width': width, 'height': 900})
            page.emulate_media(color_scheme=theme)
            dialog = choose(selected)
            for number in ('1', '2', '3', '4', '5'):
                expect(checkbox(dialog, scoped(nodes[0].nid, source.uid(number)))).to_be_checked()
            expect(checkbox(dialog, scoped(nodes[0].nid, unsupported))).to_be_disabled()
            expect(checkbox(dialog, scoped(nodes[2].nid, third.uid('1')))).to_be_disabled()
            checkbox(dialog, scoped(nodes[0].nid, source.uid('2'))).uncheck()
            expect(checkbox(dialog, scoped(nodes[0].nid, source.uid('4')))).not_to_be_checked()
            expect(checkbox(dialog, scoped(nodes[0].nid, source.uid('3')))).to_be_checked()
            expect(dialog.locator('#transfer-select-all')).to_have_js_property('indeterminate', True)
            expect(dialog.locator('[name="transfer-mode"], #transfer-new-ids')).to_have_count(0)
            page.screenshot(path=str(args.screenshots/f'tree-{width}-{theme}.png'), full_page=True)
            dialog.locator('.transfer-table-scroll').hover()
            page.mouse.wheel(0, 600)
            page.wait_for_function("document.querySelector('#clone-group-dialog .transfer-table-scroll').scrollTop > 0")
            page.screenshot(path=str(args.screenshots/f'tree-bottom-{width}-{theme}.png'), full_page=True)
            dialog.locator('.clone-cancel').click()
        print('PASS tree defaults, unsupported/remote gray, native dependency linking, responsive layout', flush=True)

        page.set_viewport_size({'width': 1280, 'height': 900})
        page.emulate_media(color_scheme='light')

        def move(anchor, owner, selected_uid, destination, *, stale=False, lose_reply=False):
            dialog = choose(anchor)
            dialog.locator('#transfer-select-all').uncheck()
            expect(dialog.locator('.clone-confirm')).to_be_disabled()
            checkbox(dialog, selected_uid).check()
            if stale:
                held = []
                def delay_plan(route):
                    response = route.fetch()
                    held.append((route, response))
                page.route('**/api/session/clone/plan', delay_plan, times=1)
                select_target(dialog, destination.nid)
                for _ in range(200):
                    if held: break
                    page.wait_for_timeout(25)
                assert held, 'plan response was not captured'
                select_target(dialog, nodes[2].nid)
                with page.expect_response(lambda response: response.url.endswith('/api/session/clone/plan')
                        and response.request.post_data_json['target_node'] == nodes[2].nid):
                    held[0][0].fulfill(response=held[0][1])
                expect(dialog.locator('.clone-confirm')).to_be_enabled()
                print('PASS late preview cannot replace a newer destination; current target is replanned', flush=True)
            select_target(dialog, destination.nid)
            try:
                expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=30000)
            except Exception:
                print('PLAN FAILURE', dialog.inner_text(), errors, flush=True)
                page.screenshot(path=str(args.screenshots/'plan-failure.png'), full_page=True)
                raise
            if stale:
                with source.paths['1'].open('ab') as stream:
                    stream.write((json.dumps(codex_message('assistant', '预览之后追加的原生历史'))+'\n').encode())
                with page.expect_response('**/api/session/transfer/clone', timeout=60000) as refused:
                    dialog.locator('.clone-confirm').click()
                assert refused.value.status == 409, refused.value.text()
                assert source.paths['1'].exists()
                saved = json.loads(metadata.read_text())['sessions']
                assert 'node_id' not in saved[source.uid('2')]['nest_parent']
                expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=30000)
                expect(dialog.locator('.clone-confirm')).to_have_text('迁移所选会话',timeout=30000)
                print('PASS stale selected plan aborts before source cleanup and replans without changing nesting', flush=True)
            if lose_reply:
                results = []
                def lost(route):
                    response = route.fetch(timeout=60000)
                    assert response.ok, response.text()
                    results.append(response.json())
                    route.abort('failed')
                page.route('**/api/session/transfer/clone', lost, times=1)
                dialog.locator('.clone-confirm').click()
                expect(dialog).not_to_be_visible(timeout=60000)
                assert len(results) == 1
                result = results[0]
                print('PASS lost completion reply resolves through the same durable operation without another move', flush=True)
            else:
                with page.expect_response(lambda response: response.url.endswith('/api/session/transfer/clone'), timeout=60000) as completed:
                    dialog.locator('.clone-confirm').click()
                response = completed.value
                assert response.ok, response.text()
                result = response.json()
            assert result['mode'] == 'move' and result['new_ids'] is False, result
            pending = page.evaluate("""async () => {
                const response = await fetch('/api/session/transfers');
                return {status: response.status, body: await response.json()};
            }""")
            assert pending['status'] == 200, pending
            assert isinstance(pending['body']['operations'], list), pending
            expect(dialog).not_to_be_visible(timeout=30000)
            return result

        result = move(scoped(nodes[0].nid, unsupported), nodes[0], selected, nodes[1], stale=True)
        assert set(result['link_map'].values()) == {'codex:'+ident(1)}
        assert not source.paths['1'].exists() and source.paths['2'].exists() and source.paths['5'].exists()
        saved = json.loads(metadata.read_text())['sessions']
        assert saved[source.uid('2')]['nest_parent']['node_id'] == nodes[1].nid
        assert saved[unsupported]['nest_parent']['node_id'] == nodes[1].nid
        assert json.loads(remote.read(source.paths['1']).splitlines()[0])['payload']['id'] == ident(1)
        print('PASS selected parent moves with fixed native ID; excluded native/unsupported children remain with remote parent', flush=True)

        def verify_third():
            response = page.request.get(f'http://127.0.0.1:{hub.port}/api/sessions?force=1')
            rows = response.json()['sessions']
            row = next(row for row in rows if row['uid'] == scoped(nodes[2].nid, third.uid('1')))
            assert row['nest_parent']['node_id'] == nodes[1].nid, row
            detail = page.request.get(f'http://127.0.0.1:{hub.port}/api/messages/'+row['uid']).json()
            assert detail['meta']['nest_parent']['node_id'] == nodes[1].nid, detail
        verify_third()
        hub.stop(); hub.start()
        remote.call('stop'); remote.call('start')
        page.goto(f'http://127.0.0.1:{hub.port}', wait_until='networkidle')
        verify_third()
        child = scoped(nodes[0].nid, source.uid('2'))
        result = move(child, nodes[0], child, nodes[1], lose_reply=True)
        assert set(result['link_map'].values()) == {'codex:'+ident(2), 'codex:'+ident(4)}
        assert source.paths['3'].exists() and not source.paths['2'].exists() and not source.paths['4'].exists()
        saved = json.loads(metadata.read_text())['sessions']
        assert saved[source.uid('3')]['nest_parent']['node_id'] == nodes[1].nid
        incoming = json.loads(remote.read(root/'destination/state/session-metadata.json'))['sessions']
        assert 'node_id' not in incoming[source.uid('2')]['nest_parent']
        print('PASS child/native dependency unit moves independently, reconnects to target parent; grandchild remains cross-node', flush=True)
        fork = scoped(nodes[0].nid, source.uid('5'))
        result = move(fork, nodes[0], fork, nodes[1])
        assert set(result['link_map'].values()) == {'codex:'+ident(5)} and not source.paths['5'].exists()
        print('PASS a self-contained native fork moves alone without dragging its parent or siblings', flush=True)
        # Native IDs stay stable when the parent returns, while its children stay
        # on the target; local and third-node references follow the same lineage.
        result = move(scoped(nodes[1].nid, source.uid('1')), nodes[1], scoped(nodes[1].nid, source.uid('1')), nodes[0])
        assert source.paths['1'].exists() and set(result['link_map'].values()) == {'codex:'+ident(1)}
        page.reload(wait_until='networkidle')
        rows = page.request.get(f'http://127.0.0.1:{hub.port}/api/sessions?force=1').json()['sessions']
        external = next(row for row in rows if row['uid'] == scoped(nodes[2].nid, third.uid('1')))
        assert external['nest_parent']['node_id'] == nodes[0].nid, external
        assert 'node_id' not in json.loads(metadata.read_text())['sessions'][unsupported]['nest_parent']
        print('PASS return move preserves cross-node tree and local parent restoration; Hub/node restart retains ownership', flush=True)
        for options in ({'mode': 'clone'}, {'mode': 'move', 'new_ids': True}):
            rejected = page.request.post(base+'/api/session/clone/plan', data={
                'uid': source.uid('1'), 'target_node': nodes[1].nid, **options})
            assert rejected.status == 409 and rejected.json()['code'] == 'move_format', rejected.text()
        assert not errors, errors
    print('PASS selected cross-host migration with fixed identities and durable cross-node nesting')


if __name__ == '__main__':
    main()
