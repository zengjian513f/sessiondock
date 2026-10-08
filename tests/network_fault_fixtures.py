"""Loopback faults that return headers then stall the real response body."""
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class StalledResponses:
    def __init__(self):
        self.entries = []
        owner = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def log_message(self, *_):
                pass

            def do_OPTIONS(self):
                self.send_response(204)
                self.send_header('Access-Control-Allow-Origin', '*')
                self.send_header('Access-Control-Allow-Headers', '*')
                self.send_header('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
                self.send_header('Content-Length', '0')
                self.end_headers()

            def do_POST(self):
                self.rfile.read(int(self.headers.get('Content-Length', '0')))
                self.do_GET()

            def do_GET(self):
                entry = owner.entries[int(self.path[1:])]
                try:
                    if entry['mode'] == 'headers':
                        entry['release'].wait(60)
                    self.send_response(200)
                    self.send_header('Access-Control-Allow-Origin', '*')
                    if entry['mode'] == 'events':
                        self.send_header('Content-Type', 'text/event-stream')
                        self.send_header('Connection', 'close')
                    else:
                        self.send_header('Content-Type', 'application/json')
                        self.send_header('Content-Length', str(len(entry['body'])))
                    self.end_headers()
                    if entry['mode'] == 'events':
                        self.wfile.write(b'event: change\ndata: {"initial":true}\n\n')
                    else:
                        self.wfile.write(entry['body'][:1])
                    self.wfile.flush()
                    entry['headers'].set()
                    if entry['mode'] != 'headers':
                        entry['release'].wait(60)
                    if entry['mode'] != 'events':
                        self.wfile.write(entry['body'][1:])
                        self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    pass

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.release()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def release(self):
        for entry in self.entries:
            entry['release'].set()

    def redirect(self, route, mode, body=None):
        if body is None:
            body = b'' if mode == 'events' else route.fetch().body()
        entry = dict(body=body, mode=mode, release=threading.Event(), headers=threading.Event())
        self.entries.append(entry)
        route.continue_(url=f'http://127.0.0.1:{self.server.server_port}/{len(self.entries)-1}')
