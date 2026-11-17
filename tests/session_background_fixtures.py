"""Private node proxy for migration outage/restart browser scenarios."""
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import threading


class BlockedNode:
    def __init__(self, port):
        self.blocked = True
        self.block_cancel = False
        self.hits = 0
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_): pass
            def do_GET(self): self.forward()
            def do_POST(self): self.forward()
            def forward(self):
                body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
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
                    connection.request(self.command, self.path, body=body, headers={k: v for k, v in self.headers.items() if k.lower() not in ('host', 'connection')})
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
