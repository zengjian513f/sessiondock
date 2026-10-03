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
        self.cpu = 2
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
                                        'gpu_devices': {'value':['GPU-fixture'],'status':'partial'},
                                        'memory_pss_bytes': {'value':32*1024**2,'status':'ok'},
                                        'proc_storage_read_bytes_per_second': {'value':1024,'status':'partial'},
                                        'proc_storage_write_bytes_per_second': {'value':2048,'status':'partial'},
                                        'disk_read_operations_per_second': {'value': 12.5 if self.enabled else None, 'status': 'partial' if self.enabled else 'unavailable'}}}
                                   for b in self.bindings]
                        samples += samples[:1]  # Repeated records must not inflate process/GPU counts.
                        result = {**report, 'availability': 'observed', 'method': 'fixture', 'samples': samples,
                                  'unavailable': [], 'metric_availability': {}, 'sessions': [], 'diagnostic': diagnostic,
                                  'session_measurements': [{'session': self.bindings[0]['session'], 'metrics': {
                                      'memory_bandwidth_bytes_per_second': {'value':32*1024**2,'status':'partial','sampled_at':time.time()}}}]}
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
        page.clock.install()
        page.goto(base, wait_until='networkidle')
        sidebar = page.locator(f'#side .item[data-uid="{uid}"]')
        assert page.evaluate("[...document.querySelector('.header-filters').children].map(e=>e.id).slice(1,5).join(',')") == 'sidebar-resource-control,nest,view,node-picker'
        toggle = page.get_by_role('button', name='列表资源', exact=True)
        assert toggle.get_attribute('aria-pressed') == 'false'
        assert sidebar.locator('.item-resources').count() == 0
        original_width = page.locator('#left').bounding_box()['width']
        original_meta = sidebar.locator('.m').inner_text()
        toggle.click()
        assert page.locator('#left').bounding_box()['width'] == original_width + 144
        wait_for(lambda: sidebar.locator('[data-resource="cpu_cores"] .item-resource-value').inner_text() == '4')
        assert sidebar.locator('.item-resources .ui-icon').count() == 6
        assert sidebar.locator('[data-resource="process_count"] .item-resource-value').inner_text() == '2'
        assert sidebar.locator('[data-resource="gpu_count"] .item-resource-value').inner_text() == '2'
        assert sidebar.locator('.item-resources').evaluate('''e => {
            const cells=[...e.children];
            const names=cells.map(c=>c.dataset.resource);
            return names.join(',')==='cpu_cores,process_count,memory_pss_bytes,gpu_count,proc_storage_read_bytes_per_second,proc_storage_write_bytes_per_second'
              && [0,2,4].every(i=>cells[i].getBoundingClientRect().top===cells[i+1].getBoundingClientRect().top)
              && cells.every(c=>{const gap=c.querySelector('.item-resource-value').getBoundingClientRect().left-c.querySelector('.ui-icon').getBoundingClientRect().right;return gap>=2 && gap<=4;});
        }''')
        assert sidebar.locator('[data-resource="memory_pss_bytes"] .item-resource-value').inner_text() == '64M'
        assert sidebar.locator('[data-resource="proc_storage_read_bytes_per_second"] .item-resource-value').inner_text() == '2K/s'
        assert sidebar.locator('[data-resource="proc_storage_write_bytes_per_second"] .item-resource-value').inner_text() == '4K/s'
        assert sidebar.locator('.item-resources [title], .item-resources[title]').count() == 0
        assert sidebar.locator('.body > :nth-child(2)').get_attribute('class') == 'm'
        assert sidebar.locator('.m').inner_text() == original_meta
        assert sidebar.evaluate("e => e.querySelector('.item-resources').getBoundingClientRect().left >= e.querySelector('.body').getBoundingClientRect().right")
        toggle.click()
        assert not sidebar.locator('.item-resources').is_visible()
        assert page.locator('#left').bounding_box()['width'] == original_width
        toggle.click()
        page.reload(wait_until='networkidle')
        assert toggle.get_attribute('aria-pressed') == 'true'
        assert page.locator('#left').bounding_box()['width'] == original_width + 144
        # Rows without a star keep the same resource-column boundary.
        star = sidebar.locator('.item-star')
        aligned_left = sidebar.locator('.item-resources').bounding_box()['x']
        sidebar.evaluate("e => { e._testStar=e.querySelector('.item-star'); e._testStar.remove(); }")
        assert abs(sidebar.locator('.item-resources').bounding_box()['x']-aligned_left)<1
        sidebar.evaluate("e => { e.append(e._testStar); delete e._testStar; }")
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
        assert page.locator('#left').bounding_box()['width']==original_width+144
        sidebar.click()
        saved = sidebar.element_handle()
        collectors[0].cpu = 3
        page.evaluate('SessionDockSidebarResources.refresh()')
        assert sidebar.locator('[data-resource="cpu_cores"] .item-resource-value').inner_text() == '5'
        assert sidebar.evaluate('(node, old) => node === old', saved)
        assert 'sel' in sidebar.get_attribute('class')
        page.route('**/api/resources/summary', lambda route: route.fulfill(status=503, body='unavailable'))
        page.evaluate('SessionDockSidebarResources.refresh()')
        assert sidebar.locator('[data-resource="cpu_cores"] .item-resource-value').inner_text() == '—'
        page.unroute('**/api/resources/summary')
        page.evaluate('SessionDockSidebarResources.refresh()')
        assert sidebar.locator('[data-resource="cpu_cores"] .item-resource-value').inner_text() == '5'
        page.set_viewport_size({'width':390,'height':844})
        if not sidebar.is_visible():
            page.locator('.mobile-back:visible').click()
        assert sidebar.is_visible()
        assert sidebar.evaluate('(e) => e.clientWidth > 0 && e.scrollWidth <= e.clientWidth')
        assert sidebar.locator('.item-resources').is_visible()
        page.screenshot(path='/tmp/sidebar-resources-mobile.png')
        page.set_viewport_size({'width':1280,'height':960})

        page.screenshot(path='/tmp/sidebar-resources-desktop.png')
        assert page.locator('#detail [data-session-resources]').count() == 0
        other = page.locator(f'#side .item[data-uid="{scoped(nodes[2].nid, corpora[2].uid('probe'))}"]')
        other.locator('.body').click()
        selected_before = page.evaluate('S.sel')
        sidebar.locator('.item-resources').click()
        assert page.evaluate('S.sel') == selected_before
        assert page.locator('.sr-subtitle').inner_text() == sidebar.evaluate('(e) => e._resourceSession.title')
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
        wait_for(lambda: len(collectors[0].calls) > sent_before)
        assert collectors[0].calls[-1]['lease_seconds'] <= 50
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
        page.get_by_role('button', name='关闭资源面板').click()
        wait_for(lambda: not collectors[0].enabled)
        counts = [len(c.calls) for c in collectors]
        assert context.request.post(local[0][0] + '/api/resources/probe', data={'enabled': True}).status == 403
        assert context.request.post(f'http://127.0.0.1:{nodes[0].port}/api/resources/probe', data={'enabled': True}).status == 403
        assert context.request.post(base + '/api/session/resources/probe', data={'uid': scoped(nodes[0].nid, 'codex:missing'), 'scope': 'inclusive', 'enabled': True}).status == 400
        assert context.request.post(local[0][0] + '/api/session/resources/probe', data={'uid': 'codex:missing', 'scope': 'direct', 'enabled': True}).status == 404
        assert counts == [len(c.calls) for c in collectors]
        assert not collectors[2].calls and not collectors[3].calls
        print('PASS sidebar resource badges, refresh, outage, mobile layout and real browser/Hub/node probe start-stop, diagnostic GET, partial error, related-only routing, offline exclusion, authentication and invalid-session gate', flush=True)


if __name__ == '__main__':
    main()
