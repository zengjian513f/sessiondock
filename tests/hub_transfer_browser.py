#!/usr/bin/env python3
"""Hub unfinished-journal cache: Chromium execution, cancellation and recovery.

Uses temporary synthetic sessions and Linux inotify IN_OPEN to detect actual
ledger reads (independent of filesystem atime policy). No real CLI is launched.
"""
import argparse
from contextlib import ExitStack
import ctypes
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import tempfile
import threading
import time
from types import SimpleNamespace
import uuid

from playwright.sync_api import sync_playwright, expect
from history_parity import BINARY, Corpus, isolated_server
from hub_http_suite import Hub, free_port, scoped
from node_auth_suite import node_env, TOKEN
from session_files_browser import fixture, uid


class LedgerReads:
    def __init__(self, stack):
        self.libc = ctypes.CDLL(None, use_errno=True)
        self.fd = self.libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
        if self.fd < 0:
            raise OSError(ctypes.get_errno(), 'inotify_init1')
        stack.callback(os.close, self.fd)

    def watch(self, path):
        if self.libc.inotify_add_watch(self.fd, os.fsencode(path), 0x20) < 0:
            raise OSError(ctypes.get_errno(), 'inotify_add_watch')

    def take(self):
        events = bytearray()
        while True:
            try:
                events.extend(os.read(self.fd, 65536))
            except BlockingIOError:
                return events

    def assert_unopened(self):
        assert not self.take(), 'terminal journal was reopened'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='sessiondock-hub-transfer-') as tmp, \
            sync_playwright() as pw, ExitStack() as stack:
        root = Path(tmp)
        roots, claude, _, _ = fixture(root / 'source')
        source, destination = Corpus(root / 'source'), Corpus(root / 'destination')
        for corpus in (source, destination):
            for name in ('state', 'proc', 'ids', 'trash'):
                (corpus.root / name).mkdir(parents=True)
        roots['codex'] = str(source.root / 'codex')
        real_port = free_port()
        blocked, release = threading.Event(), threading.Event()
        mode = {'hold': False, 'offline': False}

        class Proxy(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                self.forward()

            def do_POST(self):
                self.forward()

            def forward(self):
                body = self.rfile.read(int(self.headers.get('Content-Length', '0')))
                if mode['offline'] and '/api/session/transfer/' in self.path:
                    raw = b'{"code":"move_node_unavailable","error":"fixture offline"}'
                    self.send_response(503)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(raw)))
                    self.end_headers()
                    self.wfile.write(raw)
                    return
                if mode['hold'] and self.path.endswith('/transfer/manifest'):
                    blocked.set()
                    release.wait(120)
                remote = http.client.HTTPConnection('127.0.0.1', real_port, timeout=35)
                try:
                    remote.request(self.command, self.path, body, headers={
                        k: v for k, v in self.headers.items() if k.lower() not in ('host', 'connection')})
                    response = remote.getresponse()
                    data = response.read()
                    self.send_response(response.status)
                    for key, value in response.getheaders():
                        if key.lower() not in ('transfer-encoding', 'content-length', 'connection'):
                            self.send_header(key, value)
                    self.send_header('Content-Length', str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError, http.client.RemoteDisconnected):
                    pass
                finally:
                    remote.close()

        proxy = ThreadingHTTPServer(('127.0.0.1', 0), Proxy)
        proxy.daemon_threads = True
        threading.Thread(target=proxy.serve_forever, daemon=True).start()
        stack.callback(proxy.server_close)
        stack.callback(proxy.shutdown)
        stack.callback(release.set)
        a = SimpleNamespace(name='source', nid='a' * 32, port=proxy.server_port, token=TOKEN)
        b = SimpleNamespace(name='target', nid='b' * 32, port=free_port(), token=TOKEN)
        for node, corpus, port in ((a, source, real_port), (b, destination, b.port)):
            (corpus.root / 'ids/node-id').write_text(node.nid + '\n')
            env = node_env(corpus.root, port, '127.0.0.0/8')
            env.update({f'SESSIONDOCK_{key.upper()}_ROOT': value for key, value in roots.items()})
            env['SESSIONDOCK_PROC_ROOT'] = str(corpus.root / 'proc')
            stack.enter_context(isolated_server(corpus, args.binary, state_dir=corpus.root / 'state',
                trash_dir=corpus.root / 'trash', extra_env=env))
        hubroot = root / 'hub'
        hubroot.mkdir()
        hub = Hub(args.binary.resolve().with_name('sessiondock-hub'), hubroot, [a, b])
        ledger = hubroot / 'transfers'
        ledger.mkdir()
        damaged = ledger / 'damaged.json'
        damaged.write_text('{incomplete')
        selected = scoped(a.nid, uid('claude', claude[2]))
        # Startup must scan these once, then never reread them for polling.
        terminal = []
        for phase in ('complete', 'aborted'):
            operation = str(uuid.uuid4())
            path = ledger / f'{operation}.json'
            path.write_text(json.dumps({'request': {'uid': selected, 'target_node': b.nid,
                'operation_id': operation}, 'phase': phase, 'result': None}))
            terminal.append(path)
        hub.start()
        stack.callback(hub.stop)
        reads = LedgerReads(stack)
        for path in terminal + [damaged]:
            reads.watch(path)
        launch = {'headless': True}
        if os.environ.get('PLAYWRIGHT_CHROMIUM_EXECUTABLE'):
            launch['executable_path'] = os.environ['PLAYWRIGHT_CHROMIUM_EXECUTABLE']
        browser = pw.chromium.launch(**launch)
        stack.callback(browser.close)
        context = browser.new_context(service_workers='block')
        stack.callback(context.close)
        base = f'http://127.0.0.1:{hub.port}'

        def pending():
            response = context.request.get(base + '/api/session/transfers')
            assert response.ok, response.text()
            return response.json()['operations']

        def begin():
            page = context.new_page()
            page.goto(base, wait_until='networkidle')
            page.locator(f'#side .item[data-uid="{selected}"]').click()
            page.locator('#a-clone-group').click()
            dialog = page.locator('#clone-group-dialog')
            expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=15000)
            dialog.locator('#transfer-target').select_option(b.nid)
            expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=15000)
            submitted = []
            page.on('request', lambda request: submitted.append(request.post_data_json)
                    if request.url.endswith('/api/session/transfer/clone') else None)
            return page, dialog, submitted

        assert pending() == []
        page, dialog, submitted = begin()
        with page.expect_response(lambda r: r.url.endswith('/api/session/transfer/clone'), timeout=30000) as done:
            dialog.locator('.clone-confirm').click()
        assert done.value.ok, done.value.text()
        operation = submitted[-1]['operation_id']
        path = ledger / f'{operation}.json'
        assert json.loads(path.read_text())['phase'] == 'complete'
        assert pending() == []
        # Execution options are durable. Legacy retries can omit them, while
        # a conflicting retry cannot turn a completed copy into a move.
        receipt = json.loads(path.read_text())
        old_request = {k:v for k,v in submitted[-1].items() if k not in ('mode','new_ids')}
        retried = context.request.post(base + '/api/session/transfer/clone', data=old_request)
        assert retried.ok and retried.json()['target_uid'] == done.value.json()['target_uid']
        conflict = context.request.post(base + '/api/session/transfer/clone', data={**submitted[-1], 'mode':'move'})
        assert conflict.status == 409
        assert json.loads(path.read_text()) == receipt, 'conflicting options altered the completed operation'
        reads.watch(path)
        page.close()

        for restart in (False, True):
            blocked.clear()
            release.clear()
            mode['hold'] = True
            page, dialog, submitted = begin()
            dialog.locator('.clone-confirm').click()
            assert blocked.wait(10), 'transfer did not reach preparation'
            operation = submitted[-1]['operation_id']
            expect(dialog.locator('.transfer-progress')).to_have_attribute('data-phase', 'preparing', timeout=10000)
            bar=dialog.locator('[role=progressbar]')
            expect(bar).to_be_visible()
            initial_progress=float(bar.get_attribute('aria-valuenow'))
            assert 0<=initial_progress<100
            assert '预计总进度' in bar.get_attribute('aria-valuetext')
            page.set_viewport_size({'width':390,'height':844})
            page.emulate_media(reduced_motion='reduce')
            assert bar.locator('i').evaluate('(el)=>getComputedStyle(el).animationName')=='none'
            assert bar.bounding_box()['width']<=390,'progress bar overflows mobile dialog'
            assert any(j['request']['operation_id'] == operation for j in pending())
            # More than two recovery ticks while the browser renews its lease.
            page.wait_for_timeout(11000)
            assert any(j['request']['operation_id'] == operation and j['phase'] == 'preparing' for j in pending())
            reads.assert_unopened()
            assert float(bar.get_attribute('aria-valuenow'))==initial_progress,'waiting alone advanced overall progress'
            if restart:
                mode['offline'] = True
                hub.stop()
                page.close()
                hub.start()
                # Failed reconciliation remains visible and retryable.
                deadline = time.monotonic() + 15
                while True:
                    operations = pending()
                    if any(j['request']['operation_id'] == operation and j['error'] for j in operations):
                        break
                    assert time.monotonic() < deadline, operations
                    time.sleep(.1)
                mode['offline'] = False
                deadline = time.monotonic() + 20
                while pending():
                    assert time.monotonic() < deadline, 'recovery failed after source returned'
                    time.sleep(.1)
            else:
                with page.expect_response(lambda r: r.url.endswith('/api/session/transfer/cancel'), timeout=10000) as cancelled:
                    dialog.locator('.transfer-abort').click()
                assert cancelled.value.ok, cancelled.value.text()
                assert pending() == []
                page.close()
            mode['hold'] = False
            release.set()
            path = ledger / f'{operation}.json'
            assert json.loads(path.read_text())['phase'] == 'aborted'
            assert all(path.is_file() for path in claude.values())
            # Restart legitimately rereads old terminal journals once. Start new
            # watches after it, then verify both old and newly terminal ledgers.
            if restart:
                reads.take()  # Discard the one legitimate startup read.
                for path in ledger.glob('*.json'):
                    reads.watch(path)
            else:
                reads.watch(path)
        observer = context.new_page()
        observer.goto(base, wait_until='networkidle')
        observer.wait_for_timeout(11000)
        assert pending() == []
        reads.assert_unopened()
        print('PASS hub transfers: startup terminal reads only, new task membership, foreground lease, '
              'completion/cancellation removal, offline restart recovery, preserved source and ledgers')
        observer.close()
        for local, late_error in ((False, False), (True, True)):
            page, dialog, submitted = begin()
            if local:
                dialog.locator('#transfer-target').select_option(a.nid)
                expect(dialog.locator('.clone-confirm')).to_be_enabled(timeout=15000)
            else:
                page.set_viewport_size({'width':390,'height':844})
            # Execute against the real nodes, but strand its response in the
            # browser. Also stall one progress request until its signal expires.
            page.evaluate('''() => {
                const original = window.fetch;
                window.transferFault = {active:false, stalled:false, polls:0, completions:0};
                window.fetch = async (url, options) => {
                    const fault = window.transferFault;
                    if (/\\/api\\/session\\/(transfer\\/clone|clone)$/.test(String(url))) {
                        fault.active = true;
                        const response = await original(url, options);
                        fault.result = await response.clone().json();
                        await new Promise((resolve, reject) => {fault.release=resolve; fault.reject=reject;});
                        return response;
                    }
                    if (fault.active && /\\/api\\/session\\/(transfer|clone)\\/progress$/.test(String(url))) {
                        fault.polls++;
                        if (!fault.stalled) {
                            fault.stalled = true;
                            await new Promise((_, reject) => options?.signal?.addEventListener('abort',
                                () => reject(options.signal.reason), {once:true}));
                        }
                    }
                    return original(url, options);
                };
            }''')
            dialog.locator('.clone-confirm').click()
            page.wait_for_function('transferFault.result?.phase === "complete"',timeout=30000)
            result=page.evaluate('transferFault.result')
            expect(dialog).to_have_count(0,timeout=20000)
            page.wait_for_function('(uid)=>S.sel===uid',arg=result['target_uid'],timeout=15000)
            expect(page.locator('#msgs')).to_contain_text('Branch A final')
            assert page.evaluate('transferFault.polls')>=2,'stalled progress did not retry'
            # A late execution response must not reopen/navigate after the user
            # has already left the completed target session.
            page.evaluate('(uid)=>openSession(uid)',selected)
            page.wait_for_function('(uid)=>S.sel===uid',arg=selected)
            page.evaluate('(fail)=>fail ? transferFault.reject(new Error("late connection loss")) : transferFault.release()',late_error)
            page.wait_for_timeout(300)
            assert page.evaluate('S.sel')==selected
            expect(page.locator('#clone-group-dialog')).to_have_count(0)
            page.close()
            print(f'PASS {"local" if local else "cross-node mobile"} completion recovered from stalled execution/progress; late response ignored',flush=True)


if __name__ == '__main__':
    main()
