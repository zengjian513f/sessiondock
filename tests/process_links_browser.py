#!/usr/bin/env python3
"""Shared SSH attribution: isolated node pair/Hub, process consumers and Chromium."""
import argparse
from contextlib import ExitStack, contextmanager
import json
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
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


@contextmanager
def counted_node(node):
    """Private forwarding fixture: observe Hub polls and fail one publication."""
    counts = SimpleNamespace(gets=0, posts=0, delivered=0, fail_posts=0, fail_gets=0)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def forward(self):
            body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
            tracked = self.path == '/api/process-links'
            if tracked and self.command == 'GET' and counts.fail_gets:
                counts.fail_gets -= 1
                self.send_response(503)
                self.send_header('Content-Length', '0')
                self.end_headers()
                return
            if tracked and self.command == 'POST':
                counts.posts += 1
                if counts.fail_posts:
                    counts.fail_posts -= 1
                    self.send_response(503)
                    self.send_header('Content-Length', '0')
                    self.end_headers()
                    return
            upstream = HTTPConnection('127.0.0.1', node.port, timeout=5)
            try:
                upstream.request(self.command, self.path, body=body,
                    headers={key: value for key, value in self.headers.items()
                        if key.lower() not in ('host', 'connection')})
                response = upstream.getresponse()
                streaming = 'text/event-stream' in response.getheader('Content-Type', '')
                payload = None if streaming else response.read()
                self.send_response(response.status)
                for key, value in response.getheaders():
                    if key.lower() not in ('connection', 'transfer-encoding', 'content-length'):
                        self.send_header(key, value)
                if payload is not None:
                    self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                if streaming:
                    while chunk := response.read1(65536):
                        self.wfile.write(chunk)
                        self.wfile.flush()
                else:
                    self.wfile.write(payload)
                if tracked and response.status == 200:
                    if self.command == 'GET':
                        counts.gets += 1
                    else:
                        counts.delivered += 1
            except OSError:
                # Browser streams may close when the selected session changes.
                self.close_connection = True
            finally:
                upstream.close()

        do_GET = forward
        do_POST = forward

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    worker = Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        yield SimpleNamespace(**{**vars(node), 'port': server.server_port}), counts
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=5)


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
            born = {'older': '09:00', 'parent': '10:00', 'stale': '10:30', 'child': '11:00'}
            for sid, hour in born.items():
                corpus.put(sid, 'codex', [
                    codex_row('session_meta', {'id': sid, 'cwd': '/synthetic/ssh',
                        'timestamp': '2026-09-11T' + hour + ':00Z'}),
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
                # A local `codex exec` and its helper inherit the launcher's
                # thread ID; the nearer child CLI still owns the helper.
                proc_pid(proc, 800, 'codex', ['codex', 'exec'], 100,
                         env=[('CODEX_THREAD_ID', 'parent')], fds={3: str(corpus.paths['child'])})
                proc_pid(proc, 601, 'codex-code-mode-host', ['codex-code-mode-host'], 800,
                         env=[('CODEX_THREAD_ID', 'parent')])
            else:
                proc_pid(proc, 100, 'sh', ['sh'], 1, env=[('SSH_CONNECTION', '10.0.0.1 50000 10.0.0.2 50022')])
                stat = proc / '100/stat'
                stat.write_text(stat.read_text().replace('10000', '21000'))
                proc_pid(proc, 200, 'codex', ['codex'], 100, fds={3: str(corpus.paths['child'])})
                proc_pid(proc, 300, 'python', ['python', 'train.py'], 100)
                proc_pid(proc, 400, 'codex', ['codex', 'resume'], 100, fds={3: str(corpus.paths['older'])})
                proc_pid(proc, 600, 'python', ['python', 'ambiguous.py'], 1, env=[('SSH_CONNECTION', '10.0.0.1 50001 10.0.0.2 50022')])
                # Created after the initiator, but its CLI started much later:
                # an SSH resume of an existing session, not a launch that created it.
                proc_pid(proc, 701, 'codex', ['codex', 'resume'], 100, fds={3: str(corpus.paths['stale'])})
                stat = proc / '701/stat'
                stat.write_text(stat.read_text().replace(' 70100', ' 9000000000'))
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
                        forwarded = [stack.enter_context(counted_node(node)) for node in nodes]
                        hub_nodes = [node for node, _ in forwarded]
                        counts = forwarded[1][1]
                        with isolated_server(ghost_corpus, args.binary, extra_env=ghost_env):
                            hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [*hub_nodes, ghost])
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
                    stale = next(b for b in bindings if b['process']['pid'] == 701)
                    assert stale['session']['sid'] == 'stale' and stale['initiator']['sid'] == 'parent', stale
                    get_json(opener, base, '/api/process-links')
                    rows = get_json(opener, base, '/api/sessions?force=1')['sessions']
                    assert 'nest_parent' not in next(r for r in rows if r['sid'] == 'stale'), 'SSH resume must not nest'
                    local_base, local_opener = local[0]
                    helper = wait_for(lambda: next((b for b in get_json(local_opener, local_base, '/api/process-links')['bindings']
                        if b['process']['pid'] == 601), None))
                    assert helper['session']['sid'] == 'child', helper
                    print('PASS inherited launcher identity yields to nearer child CLI; SSH resume not nested', flush=True)
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
                            assert remote['metrics']['memory_pss_bytes']['value'] == 1701 * 1024, inclusive
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
                        # Once destination reports acknowledge the successful publication,
                        # unchanged Hub polls must not cause another POST (including empty ones).
                        start_gets = counts.gets
                        wait_for(lambda: counts.gets >= start_gets + 3)
                        stable_posts = [counter.posts for _, counter in forwarded]
                        start_gets = counts.gets
                        wait_for(lambda: counts.gets >= start_gets + 3)
                        assert [counter.posts for _, counter in forwarded] == stable_posts
                        # A failed read must discard the successful-send cache, then
                        # republish on recovery even though process identities are unchanged.
                        delivered = counts.delivered
                        counts.fail_gets = 1
                        wait_for(lambda: counts.delivered > delivered)
                        delivered = counts.delivered
                        # A new descendant may inherit its parent's binding immediately;
                        # it still needs its own durable link, and a failed POST must retry.
                        counts.fail_posts = 1
                        proc_pid(procs[1], 700, 'python', ['python', 'late.py'], 100)
                        wait_for(lambda: counts.delivered > delivered and counts.fail_posts == 0)
                        wait_for(lambda: any(b['process']['pid'] == 700 for b in
                            get_json(opener, base, '/api/process-links')['bindings']))
                        page.locator(f'[data-uid="{uid}"]').first.click()
                        page.evaluate('pollSessions()')
                        page.wait_for_function('([uid,node]) => S.sessions.find(s => s.uid === uid)?.nest_parent?.node_id === node',
                            arg=[uid, nodes[0].nid])
                        print('PASS stable polls skip publish, read recovery republishes and new descendant retries failed publication', flush=True)
                        # Persisted per-process identity survives no live SSH evidence.
                        late_stat = procs[1] / '700/stat'
                        late_stat.write_text(late_stat.read_text().replace('S 100 ', 'S 1 '))
                        shutil.rmtree(procs[0] / '200')
                        (procs[1] / '100/environ').write_bytes(b'')
                        print('PASS remote CLI attribution and automatic nesting, chronology and browser gates', flush=True)
                    else:
                        assert any(b['process']['pid'] == 700 and b['session']['sid'] == 'parent'
                            for b in bindings), 'late descendant must retain its own persisted link'
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
