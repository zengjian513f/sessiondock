"""Private pushable SSE snapshots for SDK selection browser suites."""
# run_validation: skip
from contextlib import contextmanager
import copy
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import queue
import threading

@contextmanager
def snapshots():
    clients, lock = [], threading.Lock()
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Access-Control-Allow-Origin', '*')
            self.end_headers()
            messages = queue.Queue()
            with lock:
                clients.append(messages)
                messages.put(copy.deepcopy(self.server.packet))
            try:
                while True:
                    packet = messages.get()
                    if packet is None: break
                    self.wfile.write(('event: sdk\ndata: ' + json.dumps(packet) + '\n\n').encode())
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                pass
            finally:
                with lock: clients.remove(messages)
        def log_message(self, *args): pass
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    def publish():
        server.packet['seq'] += 1
        with lock:
            for messages in clients: messages.put(copy.deepcopy(server.packet))
    server.publish = publish
    server.url = f'http://127.0.0.1:{server.server_port}/events'
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try: yield server
    finally:
        with lock:
            for messages in clients: messages.put(None)
        server.shutdown(); server.server_close(); worker.join()
