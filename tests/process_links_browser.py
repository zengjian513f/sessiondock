#!/usr/bin/env python3
"""Shared SSH attribution: isolated node pair/Hub, process consumers and Chromium."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import shutil
import subprocess
import os
import tempfile
import time
from types import SimpleNamespace
from urllib.parse import urlencode

from playwright.sync_api import sync_playwright
from history_parity import BINARY, Corpus, codex_row, isolated_server, get_json
from hub_http_suite import Hub, scoped
from node_auth_suite import node_env, TOKEN, free_port
from spawned_by_suite import proc_pid


def wait_for(function, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = function()
        if value:
            return value
        time.sleep(.15)
    raise AssertionError('SSH attribution timed out')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    parser.add_argument("--with-agent", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='process-links-') as temporary, sync_playwright() as pw, ExitStack() as agents:
        root = Path(temporary)
        corpora, nodes, procs = [], [], []
        for index, name in enumerate(('a', 'b')):
            corpus = Corpus(root / name)
            for sid in ('parent', 'child', 'older'):
                corpus.put(sid, 'codex', [
                    codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/ssh',
                        'timestamp': '2026-09-11T' + ('09' if sid == 'older' else '10' if sid == 'parent' else '11') + ':00:00Z'}),
                    codex_row('response_item', {'type': 'message', 'role': 'user', 'content': name + ' ' + sid})], [])
            (corpus.root / 'ids').mkdir()
            (corpus.root / 'ids/node-id').write_text(name * 32)
            (corpus.root / 'state').mkdir(mode=0o700)
            proc = corpus.root / 'proc'
            (proc / 'sys/kernel/random').mkdir(parents=True)
            (proc / 'sys/kernel/random/boot_id').write_text('test-boot-' + name)
            (proc / 'net').mkdir()
            (proc / 'stat').write_text('btime 1700000000\n')
            if index == 0:
                proc_pid(proc, 100, 'codex', ['codex'], 1, fds={3: str(corpus.paths['parent'])})
                proc_pid(proc, 200, 'ssh', ['ssh', 'remote', 'python'], 100, fds={4: 'socket:[900]'})
                proc_pid(proc, 500, 'ssh', ['ssh', '-M', 'remote'], 100, fds={4: 'socket:[901]'})
                (proc / 'net/tcp').write_text('header\n 0: 0100000A:C350 0200000A:C366 01 0 0 0 0 0 900\n 1: 0100000A:C351 0200000A:C366 01 0 0 0 0 0 901\n')
            else:
                proc_pid(proc, 100, 'sh', ['sh'], 1, env=[('SSH_CONNECTION', '10.0.0.1 50000 10.0.0.2 50022')])
                stat = proc / '100/stat'
                stat.write_text(stat.read_text().replace('10000', '21000'))
                proc_pid(proc, 200, 'codex', ['codex'], 100, fds={3: str(corpus.paths['child'])})
                proc_pid(proc, 300, 'python', ['python', 'train.py'], 100)
                proc_pid(proc, 400, 'codex', ['codex', 'resume'], 100, fds={3: str(corpus.paths['older'])})
                proc_pid(proc, 600, 'python', ['python', 'ambiguous.py'], 1, env=[('SSH_CONNECTION', '10.0.0.1 50001 10.0.0.2 50022')])
            for entry in proc.iterdir():
                if entry.name.isdigit():
                    stat = entry / 'stat'
                    stat.write_text(stat.read_text().rstrip() + ' 0 0\n')
                    (entry / 'smaps_rollup').write_text(f'Pss: {entry.name} kB\n')
            corpora.append(corpus)
            procs.append(proc)
            nodes.append(SimpleNamespace(name=name, nid=name * 32, port=free_port(), token=TOKEN))
        if args.with_agent:
            for corpus, proc in zip(corpora, procs):
                agent = subprocess.Popen([str(args.binary.resolve().with_name('resource-agent')),
                    '--uid', str(os.getuid()), '--node-id-file', str(corpus.root / 'ids/node-id'),
                    '--socket', str(corpus.root / 'agent.sock'), '--state', str(corpus.root / 'agent-state.json'),
                    '--proc-root', str(proc), '--events', 'off'])
                agents.callback(lambda p=agent: (p.terminate(), p.wait(timeout=10)))
                wait_for(lambda: (corpus.root / 'agent.sock').exists())
        hubroot = root / 'hub'
        hubroot.mkdir()
        hub = None
        browser = pw.chromium.launch(headless=True)
        try:
            for restarted in (False, True):
                with ExitStack() as stack:
                    local = []
                    for corpus, node, proc in zip(corpora, nodes, procs):
                        environment = node_env(corpus.root, node.port, '127.0.0.0/8')
                        environment['SESSIONDOCK_PROC_ROOT'] = str(proc)
                        if args.with_agent:
                            environment['SESSIONDOCK_RESOURCE_AGENT_SOCKET'] = str(corpus.root / 'agent.sock')
                        local.append(stack.enter_context(isolated_server(corpus, args.binary,
                            state_dir=corpus.root / 'state', extra_env=environment)))
                    if not restarted:
                        ghost = SimpleNamespace(name="unrelated-offline", nid="c" * 32, port=free_port(), token=TOKEN)
                        ghost_corpus = Corpus(root / 'unrelated-offline')
                        (ghost_corpus.root / 'ids').mkdir(parents=True)
                        (ghost_corpus.root / 'ids/node-id').write_text(ghost.nid)
                        ghost_env = node_env(ghost_corpus.root, ghost.port, '127.0.0.0/8')
                        ghost_env['SESSIONDOCK_PROC_ROOT'] = str(procs[0])
                        # Register a real private node, then take only that fixture offline.
                        with isolated_server(ghost_corpus, args.binary, extra_env=ghost_env):
                            hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [*nodes, ghost])
                        hub.start()
                        stack.callback(hub.stop)
                    base, opener = local[1]
                    report = wait_for(lambda: next((b for b in get_json(opener, base, '/api/process-links')['bindings']
                        if b['process']['pid'] == 300 and b['session']['node_id'] == nodes[0].nid), None))
                    if args.with_agent:
                        assert get_json(opener, base, '/api/process-links')['collector']['service'] == 'resource-agent'
                        assert get_json(opener, base, '/api/resources')['availability'] == 'observed'
                    assert report['session']['sid'] == 'parent'
                    assert report['launch_chain'][0]['process']['node_id'] == nodes[0].nid
                    assert report['launch_chain'][0]['process']['process']['pid'] == 200
                    bindings = get_json(opener, base, '/api/process-links')['bindings']
                    assert not any(b['process']['pid'] == 600 for b in bindings), 'SSH master must remain unassigned'
                    resumed = next(b for b in bindings if b['process']['pid'] == 400)
                    assert resumed['session']['sid'] == 'older'
                    assert resumed['session']['node_id'] == nodes[1].nid
                    assert resumed['initiator']['sid'] == 'parent'
                    assert resumed['initiator']['node_id'] == nodes[0].nid
                    rows = get_json(opener, base, '/api/sessions?force=1')['sessions']
                    child = next(r for r in rows if r['sid'] == 'child')
                    expected = {'source': 'codex', 'sid': 'parent', 'node_id': nodes[0].nid}
                    assert 'spawned_by' not in child and child.get('nest_parent') == expected, child
                    assert 'nest_parent' not in next(r for r in rows if r['sid'] == 'older')
                    context = browser.new_context(service_workers='block')
                    stack.callback(context.close)
                    if not restarted:
                        page = context.new_page()
                        page.goto(f'http://127.0.0.1:{hub.port}', wait_until='networkidle')
                        uid = scoped(nodes[1].nid, corpora[1].uid('child'))
                        parent = scoped(nodes[0].nid, corpora[0].uid('parent'))
                        page.wait_for_function('uid => S.sessions.some(s => s.uid === uid)', arg=uid)
                        assert page.evaluate('([uid,parent]) => nestParentOf(S.sessions.find(s => s.uid === uid), new Map(S.sessions.map(s => [spawnKey(s.node_id,s.source,s.sid),s])))?.uid === parent', [uid, parent])
                        page.locator(f'[data-uid="{parent}"]').first.click()
                        page.locator(f'[data-uid="{uid}"]').first.click()
                        assert page.evaluate('S.sel') == uid
                        if not page.evaluate('S.nest'):
                            page.locator('#nest-toggle').click()
                        child_item = page.locator(f'#side .item[data-uid="{uid}"]')
                        child_item.click(button='right')
                        page.locator('#item-menu [data-act="detach"]').click()
                        get_json(opener, base, '/api/process-links')
                        page.evaluate('pollSessions()')
                        page.wait_for_function('uid => !S.sessions.find(s => s.uid === uid)?.nest_parent', arg=uid)
                        assert 'nest_parent' not in next(r for r in get_json(opener, base, '/api/sessions?force=1')['sessions'] if r['sid'] == 'child')
                        child_item.click(button='right')
                        page.locator('#item-menu [data-act="attach"]').click()
                        page.locator(f'#side .item[data-uid="{parent}"]').click()
                        page.wait_for_function('([uid,node]) => S.sessions.find(s => s.uid === uid)?.nest_parent?.node_id === node', arg=[uid,nodes[0].nid])
                        response = context.request.post(f'http://127.0.0.1:{hub.port}' + '/api/process-links?node=' + nodes[1].nid,
                            data={'boot_id':'forged','links':[]})
                        assert response.status == 403
                        response = context.request.post(base + '/api/process-links', data={'boot_id':'forged','links':[]})
                        assert response.status == 403
                        if args.with_agent:
                            resource_url = '/api/session/resources?' + urlencode({'uid': parent, 'scope': 'direct'})
                            hubbase = f'http://127.0.0.1:{hub.port}'
                            direct = wait_for(lambda: (lambda data: data if any(
                                row['node_id'] == nodes[1].nid and
                                row['metrics'].get('memory_pss_bytes', {}).get('value') == 400 * 1024
                                for row in data['nodes']) else None)(get_json(opener, hubbase, resource_url)))
                            inclusive = get_json(opener, hubbase, resource_url.replace('scope=direct', 'scope=inclusive'))
                            remote = next(row for row in inclusive['nodes'] if row['node_id'] == nodes[1].nid)
                            assert remote['metrics']['memory_pss_bytes']['value'] == 1000 * 1024, inclusive
                            assert all(row['status'] == 'ok' for row in direct['nodes']), direct
                            assert {row['node_id'] for row in direct['nodes']} == {node.nid for node in nodes}
                            assert direct['partial'] is False, 'unrelated offline node must not taint totals'
                            assert remote['metrics']['network_send_bytes_per_second']['value'] is None
                            assert context.request.get(hubbase + resource_url.replace('scope=direct', 'scope=invalid')).status == 400
                            local_resources = get_json(opener, base, '/api/session/resources?' + urlencode({'uid': corpora[1].uid('child')}))
                            assert len(local_resources['nodes']) == 1
                            assert local_resources['nodes'][0]['metrics']['memory_pss_bytes']['value'] == 200 * 1024
                            page.locator(f'[data-uid="{parent}"]').first.click()
                            if page.get_by_role('button', name='列表资源', exact=True).get_attribute('aria-pressed') != 'true':
                                page.get_by_role('button', name='列表资源', exact=True).click()
                            page.locator('#side .item.sel .item-resources').click()
                            page.locator('.session-resources .sr-node').first.wait_for()
                            assert page.locator('.session-resources .sr-node').count() == 2
                            assert page.locator('.session-resources [data-scope]').count() == 2
                            page.get_by_role('button', name='关闭资源面板').click()
                            page.locator(f'[data-uid="{uid}"]').first.click()
                            if page.get_by_role('button', name='列表资源', exact=True).get_attribute('aria-pressed') != 'true':
                                page.get_by_role('button', name='列表资源', exact=True).click()
                            page.locator('#side .item.sel .item-resources').click()
                            page.wait_for_function("document.querySelectorAll('.session-resources .sr-node').length === 1")
                            assert page.locator('.session-resources .sr-node h3').inner_text() == 'b'
                            page.get_by_role('button', name='关闭资源面板').click()
                            print('PASS unrelated online/offline machines hidden, SSH target retained, totals filtered', flush=True)
                            print('PASS real session resources API, opaque UID resolution, remote direct/inclusive PSS, browser resource panel', flush=True)
                        # Persisted per-process identity survives no live SSH evidence.
                        shutil.rmtree(procs[0] / '200')
                        (procs[1] / '100/environ').write_bytes(b'')
                        print('PASS remote CLI attribution and automatic nesting, chronology and browser gates', flush=True)
                    else:
                        # Reusing PID 300 must not inherit its predecessor's saved link.
                        stat = procs[1] / '300/stat'
                        stat.write_text(stat.read_text().replace('30000', '30001').replace('S 100 ', 'S 1 '))
                        wait_for(lambda: not any(b['process']['pid'] == 300 for b in get_json(opener, base, '/api/process-links')['bindings']))
                        print('PASS restart without SSH evidence and PID reuse rejection', flush=True)
        finally:
            browser.close()
            if hub:
                hub.stop()


if __name__ == '__main__':
    main()
