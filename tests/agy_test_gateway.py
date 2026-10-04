"""Loopback-only, credential-free OpenAI fixture for operator AGY browser runs.

It never emits tool calls. Main replies echo the exact last USER_REQUEST;
AGY's separate title-generation request receives a short title instead.
"""
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import re
import threading

MODELS = ('sessiondock-fake', 'sessiondock-second')
THINKING = 'SESSIONDOCK_AGY_SYNTHETIC_THINKING'


@contextmanager
def gateway(root):
    records, errors = [], []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'object': 'list', 'data': [
                {'id': model, 'object': 'model', 'owned_by': 'synthetic-local'}
                for model in MODELS]}).encode())

        def do_POST(self):
            self.connection.settimeout(10)
            request = json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0))))
            messages = request.get('messages', [])
            # The planner identity occurs in main requests; title requests have
            # their own system instruction and must not echo that instruction.
            main = any('<identity>' in str(message.get('content', '')) for message in messages)
            content = next((str(message.get('content', '')) for message in reversed(messages)
                            if message.get('role') == 'user'), '')
            # Resume may append a SYSTEM_MESSAGE under the user role. Find
            # the latest actual user request, not merely the last role=user.
            matches = []
            for candidate in reversed(messages):
                if candidate.get('role') != 'user':
                    continue
                matches = re.findall(r'<USER_REQUEST>\s*([\s\S]*?)\s*</USER_REQUEST>',
                                     str(candidate.get('content', '')))
                if matches:
                    break
            user = matches[-1] if matches else content
            text = 'echo: ' + user if main else 'Synthetic AGY title'
            row = {'path': self.path, 'kind': 'main' if main else 'title',
                   'request': request, 'echo': user, 'response': text}
            records.append(row)
            with (root / 'gateway-requests.jsonl').open('a') as log:
                log.write(json.dumps(row, ensure_ascii=False) + '\n')
            # AGY 1.2.16 selects the gateway's last catalog entry for its
            # independent title generator. The interactive planner must use
            # the explicit --model sessiondock-fake selection.
            expected_model = MODELS[0] if main else MODELS[1]
            if request.get('model') != expected_model:
                errors.append('unexpected request model: ' + repr(request.get('model')))
                self.send_error(400, 'unexpected synthetic model')
                return
            if any(key in request for key in ('effort', 'reasoning_effort')):
                errors.append('custom model received explicit effort')
            print(f'GATEWAY {row["kind"]} model={request["model"]} echo={user[-120:]!r}', flush=True)
            message = {'role': 'assistant', 'content': text}
            if main:
                message['reasoning_content'] = THINKING
            self.send_response(200)
            if request.get('stream'):
                self.send_header('Content-Type', 'text/event-stream')
                self.end_headers()
                for delta, finish in ((message, None), ({}, 'stop')):
                    chunk = {'id': 'sessiondock-synthetic', 'object': 'chat.completion.chunk',
                             'model': expected_model, 'choices': [
                                 {'index': 0, 'delta': delta, 'finish_reason': finish}]}
                    self.wfile.write(('data: ' + json.dumps(chunk) + '\n\n').encode())
                self.wfile.write(b'data: [DONE]\n\n')
            else:
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({'id': 'sessiondock-synthetic',
                    'object': 'chat.completion', 'model': expected_model, 'choices': [
                        {'index': 0, 'message': message, 'finish_reason': 'stop'}],
                    'usage': {'prompt_tokens': 10, 'completion_tokens': 5,
                              'total_tokens': 15}}).encode())

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f'http://127.0.0.1:{server.server_port}', records, errors
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
