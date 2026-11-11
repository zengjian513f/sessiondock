"""Private native-platform SDK worker, controlled through JSON lines on stdin.

No browser dependency on the target: a Chromium suite may connect through an
SSH loopback forward. Native wire peers come from the existing SDK fixtures.
"""
# run_validation: skip
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from urllib.request import ProxyHandler, build_opener


def peer(source, root, binary):
    if source == 'agy':
        from agy_sdk_fixtures import AgyFixture, SID
        value = AgyFixture(root, binary)
        value.sid = SID
        config = json.loads(value.launcher.read_text())
        config['profiles'][0].update(executable=sys.executable, args=[str(value.executable)])
        value.launcher.write_text(json.dumps(config))
        return value, value.extra_env
    if source == 'grok':
        from grok_sdk_fixtures import GrokFixture
        value = GrokFixture(root)
        return value, value.env
    if source == 'kiro':
        from kiro_sdk_fixtures import KiroFixture
        value = KiroFixture(root, binary)
        return value, value.env
    if source == 'claude':
        from claude_sdk_fixtures import ClaudeFixture
        value = ClaudeFixture(root)
    else:
        # These peers only need one synthetic workspace; do not pretend their
        # Unix-only fixture filename algorithm describes a native Windows CLI.
        import codebuddy_sdk_fixtures as buddy
        buddy.project_key = lambda _: 'platform-fixture-workspace'
        if source == 'droid':
            import droid_sdk_fixtures as droid
            droid.project_key = buddy.project_key
            value = droid.DroidFixture(root)
        else:
            value = buddy.CodebuddyFixture(root)
        wrapper = root / 'fake-peer.py'
        wrapper.write_text('import sys\nsys.path.insert(0,' + repr(str(Path(__file__).parent)) + ')\n'
            'import codebuddy_sdk_fixtures as buddy\nbuddy.project_key=lambda _: "platform-fixture-workspace"\n'
            + ('import droid_sdk_fixtures as peer\npeer.project_key=buddy.project_key\n' if source == 'droid' else 'peer=buddy\n')
            + 'peer.fake_cli()\n')
        config = json.loads(value.launcher.read_text())
        config['profiles'][0]['args'] = [str(wrapper), '--fake-cli']
        value.launcher.write_text(json.dumps(config))
    return value, {'SESSIONDOCK_' + source.upper() + '_ROOT': str(value.projects)}


class Worker:
    def __init__(self, args):
        self.args = args
        self.root = Path(tempfile.mkdtemp(prefix='sdk-platform-' + args.source + '-')).resolve()
        self.child = None
        self.log = (self.root / 'service.log').open('w+b')
        self.peer, extra = peer(args.source, self.root, args.binary)
        self.state = self.root / 'durable'
        self.state.mkdir(mode=0o700)
        self.ledger = self.root / 'ledger-platform'
        self.ledger.mkdir(mode=0o700)
        self.home = self.root / 'service-home'
        self.home.mkdir()
        self.env = {key: value for key, value in os.environ.items()
                    if key.upper() in ('PATH', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'PATHEXT', 'LANG', 'TMP', 'TEMP', 'TMPDIR')}
        self.env.update(HOME=str(self.home), USERPROFILE=str(self.home), PYTHONUTF8='1',
            SESSIONDOCK_BIND=f'127.0.0.1:{args.port}', SESSIONDOCK_WEB_DIR=str(args.web),
            SESSIONDOCK_NATIVE_SDKS='auto', SESSIONDOCK_CODEX_APP_SERVER='off',
            SESSIONDOCK_STATE_DIR=str(self.state), SESSIONDOCK_LIFECYCLE_DIR=str(self.ledger),
            SESSIONDOCK_LAUNCHER_CONFIG=str(self.peer.launcher),
            SESSIONDOCK_PROC_ROOT=str(self.root / 'proc'))
        for source in ('claude', 'codex', 'grok', 'kiro', 'codebuddy', 'droid', 'opencode'):
            empty = self.root / ('empty-' + source)
            empty.mkdir()
            self.env['SESSIONDOCK_' + source.upper() + '_ROOT'] = str(empty)
        self.env['SESSIONDOCK_OPENCODE_DB'] = str(self.root / 'empty-opencode/empty.db')
        self.env.update(extra)
        config = json.loads(self.peer.launcher.read_text())
        config['host_binary'] = str(args.binary)
        self.env['SESSIONDOCK_PTYHOST_DIR'] = config['host_dir']
        for profile in config['profiles']:
            env = profile.setdefault('env', {})
            env.update(PYTHONUTF8='1', USERPROFILE=env.get('HOME', str(self.home)))
            env['PATH'] = self.env.get('PATH', '')
        self.peer.launcher.write_text(json.dumps(config))
        init = subprocess.run([str(args.binary), '--initialize-lifecycle', str(self.ledger)],
            env=self.env, capture_output=True, text=True, timeout=15)
        assert init.returncode == 0, init.stderr
        self.opener = build_opener(ProxyHandler({}))
        self.base = f'http://127.0.0.1:{args.port}'

    def start(self):
        self.child = subprocess.Popen([str(self.args.binary)], env=self.env,
            cwd=self.root, stdout=self.log, stderr=self.log)
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            assert self.child.poll() is None, self.diagnostic()
            try:
                with self.opener.open(self.base + '/api/health', timeout=.5) as reply:
                    if reply.status == 200:
                        return {'root': str(self.root), 'sid': self.peer.sid, 'base': self.base}
            except OSError:
                time.sleep(.05)
        raise AssertionError(self.diagnostic())

    def diagnostic(self):
        self.log.flush()
        text = (self.root / 'service.log').read_text(errors='replace')[-12000:]
        for path in self.state.glob('sdkhost/*/stderr'):
            text += '\n' + path.parent.name + ':\n' + path.read_text(errors='replace')[-4000:]
        return text

    def stop_service(self):
        if self.child and self.child.poll() is None:
            self.child.terminate()
            try: self.child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.child.kill(); self.child.wait(timeout=5)

    def evidence(self):
        hosts = [dict(key=path.parent.name, **json.loads(path.read_text()))
                 for path in self.state.glob('sdkhost/*/status.json')]
        if self.args.source == 'agy': events = self.peer.events()
        elif self.args.source in ('grok', 'kiro'): events = self.peer.received()
        else: events = self.peer.frames()
        from storage_tiering_fixtures import read_inputs
        return {'hosts': hosts, 'events': events, 'inputs': read_inputs(self.state / 'native-sdk-inputs.json')}

    def cleanup(self):
        self.stop_service()
        if self.args.driver:
            for path in self.state.glob('sdkhost/*/status.json'):
                status = json.loads(path.read_text())
                if status['exit'] is not None and status.get('stdout_eof') and status.get('stderr_eof'):
                    continue
                request = {'root': str(self.state / 'sdkhost'), 'key': path.parent.name, 'action': 'stop'}
                result = subprocess.run([str(self.args.driver)], input=json.dumps(request),
                    capture_output=True, text=True, timeout=20)
                if result.returncode or 'error' in json.loads(result.stdout):
                    raise AssertionError(('private host cleanup', result.stderr, result.stdout))
        self.log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', required=True, choices=('agy', 'claude', 'codebuddy', 'droid', 'grok', 'kiro'))
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--web', type=Path, required=True)
    parser.add_argument('--driver', type=Path, required=True)
    parser.add_argument('--port', type=int, default=0)
    args = parser.parse_args()
    if not args.port:
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0)); args.port = reservation.getsockname()[1]
    args.binary = args.binary.resolve(strict=True)
    worker = Worker(args)
    try:
        print(json.dumps(worker.start()), flush=True)
        for line in sys.stdin:
            action = json.loads(line)['action']
            try:
                if action == 'restart':
                    worker.stop_service(); value = worker.start()
                elif action == 'evidence': value = worker.evidence()
                elif action == 'diagnostic': value = worker.diagnostic()
                elif action == 'close': break
                else: raise ValueError(action)
                print(json.dumps({'ok': True, 'value': value}), flush=True)
            except Exception as error:
                print(json.dumps({'ok': False, 'error': repr(error), 'log': worker.diagnostic()}), flush=True)
    finally:
        worker.cleanup()


if __name__ == '__main__': main()
