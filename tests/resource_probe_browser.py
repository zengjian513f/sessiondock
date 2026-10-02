#!/usr/bin/env python3
"""Real Hub/node HTTP and Chromium diagnostic controls, private Unix collectors."""
import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import socket
import tempfile
import threading
import time
from types import SimpleNamespace
from urllib.parse import urlencode

from playwright.sync_api import sync_playwright
from history_parity import BINARY, Corpus, codex_row, isolated_server
from hub_http_suite import Hub, scoped
from node_auth_suite import TOKEN, free_port
from process_links_browser import wait_for


class Collector:
    """Only the local collector protocol is fake; HTTP and attribution stay real."""
    def __init__(self, path, node_id, bindings):
        self.path, self.node_id, self.bindings = path, node_id, bindings
        self.calls, self.fail, self.enabled = [], False, False
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.bind(str(path))
        self.socket.listen()
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self):
        while True:
            try:
                conn, _ = self.socket.accept()
            except OSError:
                return
            with conn:
                try:
                    request = json.loads(conn.makefile('rb').readline())
                    op = request['op']
                    diagnostic = {'state': 'active' if self.enabled else 'off', 'remaining_seconds': 60 if self.enabled else 0}
                    report = {'version': 1, 'node_id': self.node_id, 'boot_id': 'fixture-boot',
                              'supported': True, 'sampled_at': time.time(), 'bindings': self.bindings,
                              'outgoing': [], 'incoming': []}
                    if op == 'probe':
                        self.calls.append(request['data'])
                        if self.fail:
                            conn.sendall(json.dumps({'ok': False, 'error': 'fixture probe failure'}).encode() + b'\n')
                            continue
                        self.enabled = request['data']['enabled']
                        result = {'state': 'active' if self.enabled else 'off', 'remaining_seconds': 60 if self.enabled else 0}
                    elif op == 'resources':
                        samples = [{'process': b['process'], 'cpu_seconds': 0, 'rss_bytes': 0, 'threads': 1,
                                    'read_bytes': None, 'write_bytes': None, 'metrics': {
                                        'disk_read_operations_per_second': {'value': 12.5 if self.enabled else None, 'status': 'partial' if self.enabled else 'unavailable'}}}
                                   for b in self.bindings]
                        result = {**report, 'availability': 'observed', 'method': 'fixture', 'samples': samples,
                                  'unavailable': [], 'metric_availability': {}, 'sessions': [], 'diagnostic': diagnostic}
                    else:
                        result = report
                    conn.sendall(json.dumps({'ok': True, 'result': result}).encode() + b'\n')
                except (OSError, ValueError):
                    pass

    def close(self):
        self.socket.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='resource-probe-') as temporary, sync_playwright() as pw, ExitStack() as stack:
        root = Path(temporary)
        nodes, corpora, collectors, environments = [], [], [], []
        owner = {'node_id': 'a' * 32, 'source': 'codex', 'sid': 'probe'}
        for name in ('a', 'b', 'c', 'd'):
            corpus = Corpus(root / name)
            corpus.put('probe', 'codex', [codex_row('session_meta', {'id': 'probe', 'cwd': '/synthetic/probe', 'timestamp': '2026-09-11T10:00:00Z'}),
                       codex_row('response_item', {'type': 'message', 'role': 'user', 'content': name + ' probe'})], [])
            (corpus.root / 'ids').mkdir()
            (corpus.root / 'ids/node-id').write_text(name * 32)
            proc = corpus.root / 'proc'
            (proc / 'sys/kernel/random').mkdir(parents=True)
            (proc / 'sys/kernel/random/boot_id').write_text('fixture-boot')
            (proc / 'stat').write_text('btime 1700000000\n')
            (proc / 'net').mkdir()
            node = SimpleNamespace(name=name, nid=name * 32, port=free_port(), token=TOKEN)
            binding = {'process': {'pid': 100, 'start': 1000}, 'session': owner if name in ('a', 'b') else {'node_id': node.nid, 'source': 'codex', 'sid': 'probe'}, 'first_observed_at': time.time(), 'launch_chain': []}
            collector = Collector(corpus.root / 'collector.sock', node.nid, [binding])
            stack.callback(collector.close)
            # Avoid helper-generated token writes: all paths are private fixture state.
            token = corpus.root / 'token'
            token.write_text(TOKEN)
            token.chmod(0o600)
            environment = {'SESSIONDOCK_NODE_BIND': f'127.0.0.1:{node.port}', 'SESSIONDOCK_NODE_TOKEN_FILE': str(token),
                           'SESSIONDOCK_NODE_ID_FILE': str(corpus.root / 'ids/node-id'), 'SESSIONDOCK_NODE_PEERS': '127.0.0.0/8',
                           'SESSIONDOCK_PROC_ROOT': str(proc), 'SESSIONDOCK_RESOURCE_AGENT_SOCKET': str(collector.path)}
            nodes.append(node)
            corpora.append(corpus)
            collectors.append(collector)
            environments.append(environment)
        local = [stack.enter_context(isolated_server(corpus, args.binary, extra_env=env))
                 for corpus, env in zip(corpora[:3], environments[:3])]
        hubroot = root / 'hub'
        hubroot.mkdir()
        with isolated_server(corpora[3], args.binary, extra_env=environments[3]):
            hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, nodes)
        hub.start()
        stack.callback(hub.stop)
        launch = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**launch)
        stack.callback(browser.close)
        context = browser.new_context(service_workers='block')
        base = f'http://127.0.0.1:{hub.port}'
        uid = scoped(nodes[0].nid, corpora[0].uid('probe'))
        resource_url = base + '/api/session/resources?' + urlencode({'uid': uid, 'scope': 'inclusive'})
        view = wait_for(lambda: (lambda data: data if {n['node_id'] for n in data.get('nodes', [])} == {nodes[0].nid, nodes[1].nid} else None)(context.request.get(resource_url).json()))
        assert all(row['diagnostic']['state'] == 'off' for row in view['nodes'])
        page = context.new_page()
        page.goto(base, wait_until='networkidle')
        page.locator(f'[data-uid="{uid}"]').first.click()
        page.get_by_role('button', name='查看会话资源').click()
        page.get_by_role('button', name='探测 60 秒', exact=True).click()
        page.get_by_role('button', name='停止探测', exact=True).wait_for()
        page.locator('.sr-metric').filter(has=page.locator('dt', has_text='本地读次数')).filter(has_text='12.5').first.wait_for()
        assert '次/s' in page.locator('.sr-metric').filter(has=page.locator('dt', has_text='本地读次数')).last.inner_text()
        assert [c.calls for c in collectors] == [[{'enabled': True}], [{'enabled': True}], [], []]
        view = context.request.get(resource_url).json()
        assert all(row['diagnostic']['state'] == 'active' and row['diagnostic']['remaining_seconds'] == 60 for row in view['nodes'])
        page.get_by_role('button', name='停止探测', exact=True).click()
        page.get_by_role('button', name='探测 60 秒', exact=True).wait_for()
        page.wait_for_function("Array.from(document.querySelectorAll('.sr-metric')).filter(e => e.querySelector('dt').textContent === '本地读次数').every(e => e.querySelector('dd').textContent === '—')")
        assert all(c.calls[-1] == {'enabled': False} for c in collectors[:2])
        collectors[1].fail = True
        page.get_by_role('button', name='探测 60 秒', exact=True).click()
        page.get_by_text('b：机器探测请求失败，请检查采集服务', exact=True).wait_for()
        page.get_by_role('button', name='停止探测', exact=True).wait_for()
        assert collectors[0].enabled and not collectors[1].enabled
        counts = [len(c.calls) for c in collectors]
        assert context.request.post(local[0][0] + '/api/resources/probe', data={'enabled': True}).status == 403
        assert context.request.post(f'http://127.0.0.1:{nodes[0].port}/api/resources/probe', data={'enabled': True}).status == 403
        assert context.request.post(base + '/api/session/resources/probe', data={'uid': scoped(nodes[0].nid, 'codex:missing'), 'scope': 'inclusive', 'enabled': True}).status == 400
        assert context.request.post(local[0][0] + '/api/session/resources/probe', data={'uid': 'codex:missing', 'scope': 'direct', 'enabled': True}).status == 404
        assert counts == [len(c.calls) for c in collectors]
        assert not collectors[2].calls and not collectors[3].calls
        print('PASS real browser/Hub/node probe start-stop, diagnostic GET, partial error, related-only routing, offline exclusion, authentication and invalid-session gate', flush=True)


if __name__ == '__main__':
    main()
