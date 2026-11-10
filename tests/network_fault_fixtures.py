"""Loopback faults that return headers then stall the real response body."""
import threading
import asyncio
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit


class PausedTransport:
    """Forward real HTTP/SSE bytes; pause downstream delivery or cut TCP sockets.

    The gate is outside Chromium, so a stalled transport never blocks its JS
    thread. Only this fixture's loopback connections are affected.
    """
    def __init__(self, upstream):
        target = urlsplit(upstream)
        assert target.hostname == '127.0.0.1'
        self.target = (target.hostname, target.port)
        self.loop = asyncio.new_event_loop()
        self.ready = threading.Event()
        self.paused = False
        self.held_bytes = 0
        self.writers = set()
        self.thread = threading.Thread(target=self._run, daemon=True)

    async def _client(self, reader, writer):
        upstream = None
        try:
            incoming, upstream = await asyncio.open_connection(*self.target)
            self.writers.update((writer, upstream))

            async def copy(source, destination, downstream=False):
                while chunk := await source.read(65536):
                    if downstream and self.paused:
                        self.held_bytes += len(chunk)
                        try:
                            while self.paused:
                                await asyncio.sleep(.01)
                        finally:
                            self.held_bytes -= len(chunk)
                    destination.write(chunk)
                    await destination.drain()

            tasks = [asyncio.create_task(copy(reader, upstream)),
                     asyncio.create_task(copy(incoming, writer, True))]
            try:
                await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            finally:
                for task in tasks: task.cancel()
                await asyncio.gather(*tasks, return_exceptions=True)
        except (ConnectionError, OSError):
            pass
        finally:
            for stream in (writer, upstream):
                if stream:
                    self.writers.discard(stream)
                    stream.close()

    def _run(self):
        asyncio.set_event_loop(self.loop)
        self.server = self.loop.run_until_complete(asyncio.start_server(self._client, '127.0.0.1', 0))
        self.url = f'http://127.0.0.1:{self.server.sockets[0].getsockname()[1]}'
        self.ready.set()
        self.loop.run_forever()
        pending = asyncio.all_tasks(self.loop)
        for task in pending: task.cancel()
        self.loop.run_until_complete(asyncio.gather(*pending, return_exceptions=True))
        self.loop.close()

    def __enter__(self):
        self.thread.start()
        assert self.ready.wait(5), 'private transport failed to start'
        return self

    def cut(self):
        async def close():
            for writer in tuple(self.writers): writer.transport.abort()
        asyncio.run_coroutine_threadsafe(close(), self.loop).result(5)

    def __exit__(self, *_):
        self.paused = False
        self.cut()
        self.loop.call_soon_threadsafe(self.server.close)
        self.loop.call_soon_threadsafe(self.loop.stop)
        self.thread.join(5)


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
