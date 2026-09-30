#!/usr/bin/env python3
"""Shared SSH attribution: isolated node pair/Hub, process consumers and Chromium."""
import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import shutil
import tempfile
import time
from types import SimpleNamespace

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
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='process-links-') as temporary, sync_playwright() as pw:
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
            corpora.append(corpus)
            procs.append(proc)
            nodes.append(SimpleNamespace(name=name, nid=name * 32, port=free_port(), token=TOKEN))
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
                        local.append(stack.enter_context(isolated_server(corpus, args.binary,
                            state_dir=corpus.root / 'state', extra_env=environment)))
                    if not restarted:
                        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, nodes)
                        hub.start()
                        stack.callback(hub.stop)
                    base, opener = local[1]
                    report = wait_for(lambda: next((b for b in get_json(opener, base, '/api/process-links')['bindings']
                        if b['process']['pid'] == 300 and b['session']['node_id'] == nodes[0].nid), None))
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
                    assert child['spawned_by'] == expected
                    assert 'spawned_by' not in next(r for r in rows if r['sid'] == 'older')
                    context = browser.new_context(service_workers='block')
                    stack.callback(context.close)
                    if not restarted:
                        page = context.new_page()
                        page.goto(f'http://127.0.0.1:{hub.port}', wait_until='networkidle')
                        uid = scoped(nodes[1].nid, corpora[1].uid('child'))
                        parent = scoped(nodes[0].nid, corpora[0].uid('parent'))
                        page.wait_for_function('([uid, parent]) => nestParentOf(S.sessions.find(s => s.uid === uid), new Map(S.sessions.map(s => [spawnKey(s.node_id,s.source,s.sid),s])))?.uid === parent', arg=[uid, parent])
                        page.locator(f'[data-uid="{parent}"]').first.click()
                        page.locator(f'[data-uid="{uid}"]').first.click()
                        assert page.evaluate('S.sel') == uid
                        response = context.request.post(f'http://127.0.0.1:{hub.port}' + '/api/process-links?node=' + nodes[1].nid,
                            data={'boot_id':'forged','links':[]})
                        assert response.status == 403
                        response = context.request.post(base + '/api/process-links', data={'boot_id':'forged','links':[]})
                        assert response.status == 403
                        # Persisted per-process identity survives no live SSH evidence.
                        shutil.rmtree(procs[0] / '200')
                        (procs[1] / '100/environ').write_bytes(b'')
                        print('PASS automatic remote CLI nesting, Python attribution, chronology and browser gates', flush=True)
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
