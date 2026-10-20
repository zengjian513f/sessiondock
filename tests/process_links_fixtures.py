"""Private Hub transport observations shared by SSH browser regressions."""
# run_validation: skip
from contextlib import contextmanager
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
from types import SimpleNamespace
import json
import socketserver
from threading import Lock


@contextmanager
def counted_node(node):
    """Private forwarding fixture: observe Hub polls and fail one publication."""
    counts = SimpleNamespace(gets=0, posts=0, delivered=0, fail_posts=0, fail_gets=0,
                             reports=[], legacy=False, corrupt_base=False)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def forward(self):
            body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
            tracked = self.path.split('?')[0] == '/api/process-links'
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
                path = self.path.split('?')[0] if tracked and counts.legacy else self.path
                upstream.request(self.command, path, body=body,
                    headers={key: value for key, value in self.headers.items()
                        if key.lower() not in ('host', 'connection')})
                response = upstream.getresponse()
                streaming = 'text/event-stream' in response.getheader('Content-Type', '')
                payload = None if streaming else response.read()
                observation = None
                if tracked and self.command == 'GET' and response.status == 200:
                    value = json.loads(payload)
                    sync = response.getheader('x-sessiondock-connections') == '1'
                    if sync and counts.corrupt_base and value['base']:
                        value['base'] = '0' * 32
                        payload = json.dumps(value, separators=(',', ':')).encode()
                        counts.corrupt_base = False
                    report = value['report'] if sync else value
                    observation = dict(bytes=len(payload), sync=sync,
                        revision=value.get('revision'), base=value.get('base'),
                        connections=len(report.get('connections', [])), path=self.path,
                        sampled_at=report['sampled_at'], boot=report['boot_id'])
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
                        counts.reports.append(observation)
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


@contextmanager
def history_collector(root, report):
    """Mutable synthetic evidence; all production sockets/histories stay isolated."""
    lock = Lock()
    state = SimpleNamespace(fail=False)

    def replace(value):
        with lock:
            state.report = value
            state.full = json.dumps(dict(ok=True, result=value), separators=(',', ':')).encode() + b'\n'
            state.projected = json.dumps(dict(ok=True, result={k: v for k, v in value.items()
                if k not in ('connections', 'outgoing', 'incoming')}), separators=(',', ':')).encode() + b'\n'
    state.replace = replace
    replace(report)

    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            try:
                request = json.loads(self.rfile.readline())
                with lock:
                    if state.fail:
                        payload = b'{"ok":false,"error":"synthetic outage"}\n'
                    elif request['op'] in ('report', 'catalog', 'publish'):
                        payload = state.projected if request.get('bindings_only') else state.full
                    else:
                        payload = json.dumps(dict(ok=True, result=dict(version=1,
                            node_id=state.report['node_id'], boot_id=state.report['boot_id'],
                            availability='unavailable', samples=[]))).encode() + b'\n'
                self.wfile.write(payload)
            except (BrokenPipeError, ConnectionResetError):
                pass

    class Server(socketserver.ThreadingUnixStreamServer):
        daemon_threads = True
    with Server(str(root / 'collector.sock'), Handler) as server:
        worker = Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            yield state
        finally:
            server.shutdown()
            worker.join(timeout=5)
