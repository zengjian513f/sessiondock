#!/usr/bin/env python3
"""Explicit native Claude startup check for a transcript, with no model requests.

Copies only the supplied transcript into a temporary home. No credentials,
daily configuration, original transcript writes, or tool execution are used.
"""
# run_validation: skip
import argparse
import fcntl
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import tempfile
import termios
import threading
import time


def check_startup(history, executable, expect_failure=False):
    raw = history.read_bytes()
    sid = next(json.loads(line)['sessionId'] for line in raw.splitlines()
               if json.loads(line).get('sessionId'))
    requests = []

    class NoModel(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            requests.append(self.path)
            self.send_response(503)
            self.end_headers()

    with tempfile.TemporaryDirectory(prefix='sessiondock-claude-startup-') as tmp:
        root = Path(tmp)
        home, cwd = root/'home', root/'work'
        cwd.mkdir()
        project = home/'projects'/str(cwd).replace('/', '-')
        project.mkdir(parents=True)
        (project/(sid+'.jsonl')).write_bytes(raw)
        (home/'.claude.json').write_text(json.dumps({
            'hasCompletedOnboarding': True, 'theme': 'dark',
            'projects': {str(cwd): {'hasTrustDialogAccepted': True}},
        }))
        server = ThreadingHTTPServer(('127.0.0.1', 0), NoModel)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 150, 0, 0))
        env = {
            'PATH': '/usr/local/bin:/usr/bin:/bin', 'HOME': str(home),
            'CLAUDE_CONFIG_DIR': str(home), 'TERM': 'xterm-256color', 'LANG': 'C.UTF-8',
            'ANTHROPIC_API_KEY': 'test-only-not-a-credential',
            'ANTHROPIC_BASE_URL': f'http://127.0.0.1:{server.server_port}',
            'DISABLE_AUTOUPDATER': '1', 'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC': '1',
        }
        process = subprocess.Popen([
            str(executable), '--safe-mode', '--setting-sources', '', '--strict-mcp-config',
            '--tools', '', '--model', 'claude-haiku-4-5-20251001', '--effort', 'low',
            '--resume', sid,
        ], cwd=cwd, env=env, stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
        os.close(slave)
        data, accepted, ready = b'', False, None
        deadline = time.monotonic()+25
        try:
            while time.monotonic() < deadline:
                if select.select([master], [], [], .2)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    data += chunk
                    if b'\x1b[6n' in chunk:
                        os.write(master, b'\x1b[1;1R')
                    if not accepted and b'ANTHROPIC_API_KEY' in data:
                        # Select Yes only for the isolated, non-credential key.
                        os.write(master, b'\x1b[A\r')
                        accepted = True
                    text = re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b'', data)
                    # The terminal can paint the final letters with cursor
                    # updates; match the stable prefix of the input footer.
                    if b'?forshort' in re.sub(rb'\s+', b'', text) and ready is None:
                        ready = time.monotonic()
                if process.poll() is not None or (ready and time.monotonic()-ready > 1):
                    break
            if expect_failure:
                assert process.wait(timeout=3) != 0
                assert b'M.message?.content[0]' in data, 'expected native metadata crash not reproduced'
                print('PASS native Claude reproduces the pre-fix null-content startup crash', flush=True)
            else:
                assert ready and process.poll() is None, (
                    'Claude did not remain at the native input prompt', process.poll(), len(data))
                assert b'unrecoverable interface error' not in data
                print('PASS native Claude resumes into its input prompt without exiting', flush=True)
            assert not requests, 'startup unexpectedly attempted a model request'
            assert history.read_bytes() == raw, 'supplied transcript was modified'
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            os.close(master)
            server.shutdown()
            server.server_close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--history', type=Path, required=True)
    parser.add_argument('--claude', type=Path, default=Path.home()/'.local/bin/claude')
    parser.add_argument('--expect-failure', action='store_true')
    args = parser.parse_args()
    check_startup(args.history, args.claude, args.expect_failure)
