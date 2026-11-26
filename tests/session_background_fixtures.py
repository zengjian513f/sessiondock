"""Private node proxy for migration outage/restart browser scenarios."""
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import threading


def directory_check_env(root, binary):
    host = root / 'directory-check-host'
    ledger = root / 'directory-check-ledger'
    launcher = root / 'directory-check-launcher.json'
    host.mkdir(mode=0o700)
    launcher.touch(mode=0o600)
    launcher.write_text(json.dumps({
        'host_binary': str(Path(binary).resolve().with_name('ptyhost')),
        'host_dir': str(host),
        'adapters': [{'id': 'fixture-shell', 'source': 'codex',
                      'executable': '/bin/sh', 'args': ['-c', 'exit 0']}],
    }))
    subprocess.run([str(Path(binary).resolve()), '--initialize-lifecycle', str(ledger)],
                   env={'PATH': '/usr/bin:/bin'}, check=True, timeout=20, capture_output=True)
    return {'SESSIONDOCK_LIFECYCLE_DIR': str(ledger), 'SESSIONDOCK_LAUNCHER_CONFIG': str(launcher),
            'SESSIONDOCK_PTYHOST_DIR': str(host)}


class BlockedNode:
    def __init__(self, port):
        self.blocked = True
        self.block_cancel = False
        self.hits = 0
        self.freeze_requested = threading.Event()
        self.freeze_release = threading.Event()
        self.freeze_release.set()
        self.freeze_operation = None
        self.tree_requests = []
        self.legacy_tree = False
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_GET(self): self.forward()
            def do_POST(self): self.forward()
            def forward(self):
                body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
                if self.path.endswith('/transfer/tree'):
                    request = json.loads(body)
                    owner.tree_requests.append(request.copy())
                    if owner.legacy_tree and request.pop('related_uids', None) is not None:
                        body = json.dumps(request).encode()
                if self.path.endswith('/transfer/freeze') and not owner.freeze_release.is_set():
                    owner.freeze_operation = json.loads(body)['operation_id']
                    owner.freeze_requested.set()
                    assert owner.freeze_release.wait(45)
                if (owner.blocked and self.path.endswith(('/transfer/manifest', '/transfer/check'))
                        or owner.block_cancel and (self.path.endswith('/transfer/abort')
                            or self.path.endswith('/transfer/freeze') and json.loads(body).get('release'))):
                    owner.hits += 1
                    raw = json.dumps({'code': 'move_node_unavailable', 'error': 'fixture temporary node outage'}).encode()
                    self.send_response(503)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(raw)))
                    self.end_headers()
                    self.wfile.write(raw)
                    return
                connection = http.client.HTTPConnection('127.0.0.1', port, timeout=90)
                try:
                    connection.request(self.command, self.path, body=body, headers={k: v for k, v in self.headers.items() if k.lower() not in ('host', 'connection', 'content-length')})
                    response = connection.getresponse()
                    raw = response.read()
                    self.send_response(response.status)
                    for key, value in response.getheaders():
                        if key.lower() not in ('transfer-encoding', 'content-length', 'connection'):
                            self.send_header(key, value)
                    self.send_header('Content-Length', str(len(raw)))
                    self.end_headers()
                    self.wfile.write(raw)
                except (BrokenPipeError, ConnectionResetError):
                    pass
                finally:
                    connection.close()

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.port = self.server.server_port
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
