#!/usr/bin/env python3
"""Real Hub/node HTTP and Chromium diagnostic controls, private Unix collectors."""

import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
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
from history_fixtures import BINARY, Corpus, codex_row, isolated_server
from hub_fixtures import Hub, scoped
from node_auth_fixtures import TOKEN, free_port
from process_links_browser import wait_for
from process_links_fixtures import counted_node


class Collector:
    """Only the local collector protocol is fake; HTTP and attribution stay real."""
    def __init__(self, path, node_id, bindings):
        self.path, self.node_id, self.bindings = path, node_id, bindings
        self.calls, self.fail, self.enabled = [], False, False
        self.cpu = 2
        self.gpu_devices = ['GPU-fixture']
        self.extra_samples = []
        self.background_samples = []
        self.sample_age = 0
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
                                        'cpu_cores': {'value':self.cpu,'status':'ok'},
                                        'gpu_devices': {'value':self.gpu_devices,'status':'partial'},
                                        'gpu_memory_bytes': {'value':32*1024**2 if self.gpu_devices else 0,'status':'partial'},
                                        'memory_pss_bytes': {'value':32*1024**2,'status':'ok'},
                                        'proc_storage_read_bytes_per_second': {'value':1024,'status':'partial'},
                                        'proc_storage_write_bytes_per_second': {'value':2048,'status':'partial'},
                                        'disk_read_operations_per_second': {'value': 12.5 if self.enabled else None, 'status': 'partial' if self.enabled else 'unavailable'}}}
                                   for b in self.bindings]
                        samples += samples[:1]  # Repeated records must not inflate process/GPU counts.
                        samples += self.extra_samples + self.background_samples
                        result = {**report, 'availability': 'observed', 'method': 'fixture', 'samples': samples,
                                  'unavailable': [], 'metric_availability': {}, 'sessions': [], 'diagnostic': diagnostic,
                                  'session_measurements': [{'session': self.bindings[0]['session'], 'metrics': {
                                      'memory_bandwidth_bytes_per_second': {'value':32*1024**2,'status':'partial','sampled_at':time.time()}}}]}
                        result['sampled_at'] -= self.sample_age
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
            collector.background_samples = [
                {'process': {'pid':10000+i,'start':20000+i}, 'cpu_seconds':10, 'rss_bytes':1048576,
                 'threads':1, 'read_bytes':1024, 'write_bytes':2048,
                 'metrics': {key: {'value':1,'status':'ok','reason':'synthetic process sample'}
                             for key in ('cpu_cores','memory_pss_bytes','proc_storage_read_bytes_per_second',
                                         'proc_storage_write_bytes_per_second','network_receive_bytes_per_second')}}
                for i in range(2200)]
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
        forwarded = [stack.enter_context(counted_node(node)) for node in nodes[:3]]
        traffic = [counts for _, counts in forwarded]
        with isolated_server(corpora[3], args.binary, extra_env=environments[3]):
            hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot,
                      [node for node, _ in forwarded] + nodes[3:])
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
        page.clock.install()
        page.goto(base, wait_until='networkidle')
        sidebar = page.locator(f'#side .item[data-uid="{uid}"]')
        assert page.locator('#sidebar-resources-toggle,.item-resources').count() == 0
        original_width = page.locator('#left').bounding_box()['width']
        page.evaluate("localStorage.setItem('sessiondock.sidebarResources', 'true')")
        page.reload(wait_until='networkidle')
        assert page.locator('#left').bounding_box()['width'] == original_width
        assert not page.evaluate("document.body.classList.contains('sidebar-resources')")
        before = len([r for c in traffic for r in c.resources])
        assert context.request.get(base + '/api/resources/summary').status != 200
        assert len([r for c in traffic for r in c.resources]) == before
        for url, _ in local:
            assert context.request.get(url + '/api/resources/summary').status != 200
            assert context.request.get(url + '/api/resources/compact').status == 405
        # A detached SSH workload can outlive all launch evidence. The GPU
        # sampler still sees it, but it must neither be assigned by guesswork
        # nor disappear as zero when Hub hides its unverified execution node.
        for collector in collectors[:3]:
            collector.gpu_devices = []
        unassigned = {'process': {'pid':100, 'start':1001}, 'cpu_seconds':0,
                      'rss_bytes':0, 'threads':1, 'read_bytes':None, 'write_bytes':None,
                      'metrics': {'gpu_devices': {'value':['GPU-detached-1','GPU-detached-2'],'status':'partial'},
                                  'gpu_memory_bytes': {'value':64*1024**2,'status':'partial'}}}
        collectors[2].extra_samples = [unassigned, unassigned]
        # Same PID, different incarnation: the existing binding is not evidence.
        metric = context.request.get(resource_url).json()['totals']['metrics']['gpu_count']
        assert metric['value'] is None and metric['attribution_incomplete'] is True, metric
        view = context.request.get(resource_url).json()
        assert {n['node_id'] for n in view['nodes']} == {nodes[0].nid,nodes[1].nid}, view
        assert view['totals']['metrics']['gpu_count']['value'] is None, view
        assert view['totals']['metrics']['gpu_memory_bytes']['value'] is None, view
        sidebar.locator('.body').click()
        page.locator('#a-resources').click()
        gpu = page.locator('.sr-totals .sr-metric').nth(1)
        wait_for(lambda: gpu.locator('dd').inner_text() == '—')
        gpu.hover()
        assert '未归属' in gpu.get_attribute('title')
        for scope in ('direct', 'inclusive'):
            page.locator(f'.session-resources [data-scope="{scope}"]').click()
            wait_for(lambda: gpu.locator('dd').inner_text() == '—')
        page.locator('#a-chat').click()
        # Keep a confirmed positive subtotal, with explicit incomplete coverage.
        collectors[0].gpu_devices = ['GPU-confirmed']
        view = context.request.get(resource_url).json()
        metric = view['totals']['metrics']['gpu_count']
        assert metric['value'] == 1 and metric['status'] == 'partial' and metric['attribution_incomplete'], metric
        # Once full process identity is verified, count remote devices once.
        collectors[2].bindings.append({'process': unassigned['process'], 'session':owner,
                                      'first_observed_at':time.time(), 'launch_chain':[]})
        metric = context.request.post(local[2][0] + '/api/resources/compact',
            data={'session':owner,'inclusive':True}).json()['metrics']['gpu_count']
        assert metric['value'] == 2 and not metric.get('attribution_incomplete'), metric
        collectors[2].bindings.pop()
        collectors[2].extra_samples = []
        collectors[0].gpu_devices = []
        for collector in collectors[:3]:
            collector.gpu_devices = ['GPU-fixture']
        print('PASS unassigned remote GPU coverage, PID incarnation, known subtotal, verified recovery and true zero', flush=True)
        # Pointer moves change only the guide; releasing commits one new width.
        handle = page.locator('#drag').bounding_box()
        start_width = page.locator('#left').bounding_box()['width']
        x, y = handle['x']+handle['width']/2, handle['y']+80
        page.mouse.move(x,y)
        page.mouse.down()
        page.mouse.move(x+100,y,steps=20)
        assert page.locator('#left').bounding_box()['width']==start_width
        assert page.locator('#drag').bounding_box()['x']>handle['x']+90
        page.mouse.up()
        assert page.locator('#left').bounding_box()['width']>=start_width+99
        page.locator('#drag').dblclick()
        assert page.locator('#left').bounding_box()['width']==original_width
        collectors[0].cpu = 3
        page.set_viewport_size({'width':390,'height':844})
        if not sidebar.is_visible():
            page.locator('.mobile-back:visible').click()
        assert sidebar.is_visible()
        assert sidebar.evaluate('(e) => e.clientWidth > 0 && e.scrollWidth <= e.clientWidth')
        for theme in ('light', 'dark'):
            page.evaluate('(theme) => applyTheme(theme)', theme)
            for size, viewport in (('mobile', {'width':390,'height':844}),
                                   ('desktop', {'width':1280,'height':960})):
                page.set_viewport_size(viewport)
                page.screenshot(path=f'/tmp/sidebar-resources-{theme}-{size}.png')
        page.set_viewport_size({'width':1280,'height':960})
        other = page.locator(f'#side .item[data-uid="{scoped(nodes[2].nid, corpora[2].uid("probe"))}"]')
        other.locator('.body').click()
        selected_before = page.evaluate('S.sel')
        sidebar.locator('.body').click()
        assert page.evaluate('S.sel') == uid
        assert not page.locator('dialog[open]').count()
        assert not any(c.enabled for c in collectors)
        page.locator('#a-resources').click()
        assert page.locator('.sr-subtitle').inner_text() == page.evaluate('uid => S.sessions.find(session => session.uid === uid).title', uid)
        assert '64' in page.locator('.sr-totals .sr-metric').filter(has=page.locator('dt', has_text='内存带宽')).inner_text()
        page.locator('.sr-probe[data-state="active"]').wait_for()
        page.locator('.sr-metric').filter(has=page.locator('dt', has_text='本地读次数')).filter(has_text='12.5').first.wait_for()
        assert collectors[0].calls[-1]['enabled'] and collectors[1].calls[-1]['enabled']
        assert collectors[0].calls[-1]['lease_id'] == collectors[1].calls[-1]['lease_id']
        assert 1 <= collectors[0].calls[-1]['lease_seconds'] <= 60
        sent_before = len(collectors[0].calls)
        page.clock.fast_forward(20000)
        page.mouse.move(605,300)
        page.clock.fast_forward(11000)
        wait_for(lambda: len(collectors[0].calls) > sent_before
                 and collectors[0].calls[-1]['lease_seconds'] <= 50)
        page.clock.fast_forward(48000)
        assert collectors[0].enabled
        page.clock.fast_forward(2000)
        page.wait_for_function("document.querySelector('.sr-probe').textContent === '未探测'")
        wait_for(lambda: not collectors[0].enabled and not collectors[1].enabled)
        page.wait_for_function("Array.from(document.querySelectorAll('.sr-metric')).filter(e => e.querySelector('dt').textContent === '本地读次数').every(e => e.querySelector('dd').textContent === '—')")
        assert '64' in page.locator('.sr-totals .sr-metric').filter(has=page.locator('dt', has_text='内存带宽')).inner_text()
        # Focus/visibility alone must not count as activity, matching page sleep.
        page.evaluate("dispatchEvent(new Event('focus'));document.dispatchEvent(new Event('visibilitychange'))")
        page.clock.fast_forward(2000)
        assert not collectors[0].enabled and not collectors[1].enabled
        collectors[1].fail = True
        page.mouse.move(600,300)
        page.clock.fast_forward(1100)
        page.get_by_text('b：机器探测请求失败，请检查采集服务', exact=True).wait_for()
        wait_for(lambda: collectors[0].enabled)
        assert not collectors[1].enabled
        page.locator('#a-chat').click()
        wait_for(lambda: not collectors[0].enabled)
        # The header resource view uses the same lease lifecycle without a modal.
        collectors[1].fail = False
        sidebar.locator('.body').click()
        page.locator('#a-resources').click()
        page.locator('.sr-probe[data-state="active"]').wait_for()
        wait_for(lambda: collectors[0].enabled and collectors[1].enabled)
        assert page.locator('dialog:modal').count() == 0
        page.locator('#a-chat').click()
        wait_for(lambda: not collectors[0].enabled and not collectors[1].enabled)
        counts = [len(c.calls) for c in collectors]
        assert context.request.post(local[0][0] + '/api/resources/probe', data={'enabled': True}).status == 403
        assert context.request.post(f'http://127.0.0.1:{nodes[0].port}/api/resources/probe', data={'enabled': True}).status == 403
        assert context.request.post(base + '/api/session/resources/probe', data={'uid': scoped(nodes[0].nid, 'codex:missing'), 'scope': 'inclusive', 'enabled': True}).status == 400
        assert context.request.post(local[0][0] + '/api/session/resources/probe', data={'uid': 'codex:missing', 'scope': 'direct', 'enabled': True}).status == 404
        assert counts == [len(c.calls) for c in collectors]
        assert not collectors[2].calls and not collectors[3].calls
        # The actual browser's sidebar, scopes, refresh and diagnostic lifecycle
        # above must transfer summaries, never the multi-MB process samples.
        records = [r for c in traffic for r in c.resources]
        assert records and all(r[1] == '/api/resources/compact' for r in records), records[:20]
        for method in ('POST',):
            sizes = [r[3] for r in records if r[0] == method and r[2] == 200]
            assert sizes and max(sizes) < 16000, (method, sizes)
            print(f'PASS compact {method} resource responses: {len(sizes)} requests, max {max(sizes)} bytes', flush=True)
        raw_size = len(context.request.get(local[0][0] + '/api/resources').body())
        assert raw_size > 1000000, raw_size
        print(f'PASS full resource fixture {raw_size} bytes stays on node', flush=True)
        # Lease tests advanced browser time; align it with fresh server samples
        # before checking resource-column freshness again.
        page.clock.set_system_time(datetime.now(timezone.utc))
        # A temporarily failing new endpoint must not fall back to a full fetch.
        traffic[0].fail_resources = True
        assert context.request.get(resource_url).json()['partial']
        assert not any(r[1] == '/api/resources' for c in traffic for r in c.resources)
        traffic[0].fail_resources = False
        # Stale contributors remain missing and keep the partial indication.
        collectors[0].sample_age = 30
        view = context.request.get(resource_url).json()
        assert next(r for r in view['nodes'] if r['node_id'] == nodes[0].nid)['status'] == 'stale'
        assert view['partial'] is True
        collectors[0].sample_age = 0
        # A rolling upgrade can mix legacy and compact nodes. Browser values and
        # native-identity totals remain equivalent for both direct/inclusive.
        def stable(value):
            if isinstance(value, dict):
                return {k:stable(v) for k,v in value.items() if k != 'sampled_at'}
            if isinstance(value, list):
                return [stable(v) for v in value]
            return value
        expected_views = {scope: stable(context.request.get(resource_url.replace('inclusive', scope)).json())
                          for scope in ('direct','inclusive')}
        traffic[0].legacy_resources = True
        for scope in ('direct','inclusive'):
            assert stable(context.request.get(resource_url.replace('inclusive', scope)).json()) == expected_views[scope]
        assert any(r[1] == '/api/resources' and r[3] > 1000000 for r in traffic[0].resources)
        print('PASS compact/legacy fleet equivalence, stale and transient failure without full-data fallback', flush=True)
        traffic[0].legacy_resources = False
        collectors[0].bindings.append({
            'process': {'pid':101,'start':1001},
            'session': {'node_id':nodes[0].nid,'source':'claude','sid':'child'},
            'initiator':owner, 'first_observed_at':time.time(), 'launch_chain':[]})
        page.locator('#a-resources').click()
        for scope, cpu, count in [('direct',5,2), ('inclusive',8,3)]:
            page.locator(f'.session-resources [data-scope="{scope}"]').click()
            view = context.request.get(resource_url.replace('inclusive', scope)).json()
            assert view['totals']['metrics']['cpu_cores']['value'] == cpu, view
            assert view['totals']['metrics']['process_count']['value'] == count, view
            assert view['totals']['metrics']['gpu_count']['value'] == 2, view
            wait_for(lambda: page.locator('.sr-totals .sr-metric').filter(
                has=page.locator('dt', has_text='CPU')).locator('dd').inner_text().startswith(str(cpu)))
        for width, theme in [(1280,'light'),(390,'dark')]:
            page.set_viewport_size({'width':width,'height':960})
            page.evaluate('(theme) => applyTheme(theme)', theme)
            if not page.locator('#a-resources').is_visible():
                sidebar.locator('.body').click()
            page.locator('#a-resources').click()
            page.locator('.sr-totals').wait_for()
            page.screenshot(path=f'/tmp/resource-compact-hub-{width}-{theme}.png')
        page.set_viewport_size({'width':1280,'height':960})
        page.locator('#a-chat').click()
        direct = context.new_page()
        direct.set_viewport_size({'width':1280,'height':960})
        direct.goto(local[0][0], wait_until='networkidle')
        direct.locator('#side .item').first.click()
        direct.locator('#a-resources').click()
        for scope, cpu in [('direct',3), ('inclusive',6)]:
            direct.locator(f'.session-resources [data-scope="{scope}"]').click()
            wait_for(lambda: direct.locator('.sr-totals .sr-metric').filter(
                has=direct.locator('dt', has_text='CPU')).locator('dd').inner_text().startswith(str(cpu)))
        for width, theme in [(1280,'light'),(390,'dark')]:
            direct.set_viewport_size({'width':width,'height':960})
            direct.evaluate('(theme) => applyTheme(theme)', theme)
            if not direct.locator('#a-resources').is_visible():
                direct.locator('#side .item').first.locator('.body').click()
            direct.locator('#a-resources').click()
            direct.locator('.sr-totals').wait_for()
            direct.screenshot(path=f'/tmp/resource-compact-node-{width}-{theme}.png')
        direct.set_viewport_size({'width':1280,'height':960})
        direct.locator('#a-chat').click()
        print('PASS direct node and Hub browser scopes, child attribution and same-device deduplication', flush=True)
        print('PASS retired sidebar endpoints, normal layout and real browser/Hub/node probe start-stop, diagnostic GET, partial error, related-only routing, offline exclusion, authentication and invalid-session gate', flush=True)


if __name__ == '__main__':
    main()
