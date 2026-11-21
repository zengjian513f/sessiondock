"""Private stdio pressure and host loss fixtures for Chromium."""
# run_validation: skip
from contextlib import contextmanager
import json
import os
from pathlib import Path
import runpy
import shutil
import signal
import socket
import subprocess
import sys
import time
from types import SimpleNamespace

from history_fixtures import REPO
from frontend_paths import frontend_dir
from sdk_platform_fixtures import Worker
from sdkhost_fixtures import stop_private_sdkhosts


PAUSE = 'arm private stdin gate'
OFFLINE = 'hold'
BLOCKED_TEXT = 'private input ' * 8192
STDOUT_TEXT = 'Private output while stdin is blocked'
STDERR_TEXT = 'Private stderr while stdin is blocked'
OFFLINE_TEXT = 'Private output persisted before host loss'


def until(check, message, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(.03)
    raise AssertionError(message)


def alive(pid):
    try:
        return Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[0] != 'Z'
    except FileNotFoundError:
        return False


@contextmanager
def fixture(source, binary, gate):
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    worker = Worker(SimpleNamespace(source=source, binary=binary, web=frontend_dir(), driver=None, port=port))
    try:
        config = json.loads(worker.peer.launcher.read_text())
        profile = config['profiles'][0]
        profile['args'][:0] = [str(REPO / 'tests/sdk_hosted_recovery_fixtures.py')]
        for key, value in [('root', worker.root), ('source', source), ('sid', worker.peer.sid), ('gate', gate)]:
            profile['env'][f'SESSIONDOCK_FAKE_HOSTED_{key.upper()}'] = str(value)
        worker.peer.launcher.write_text(json.dumps(config))
        worker.start()
        yield worker
    finally:
        (worker.root / 'release-fixture').touch()
        (worker.root / 'release-descendant').touch()
        try:
            worker.cleanup()
            stop_private_sdkhosts(worker.root)
        finally:
            shutil.rmtree(worker.root)


def host_status(worker):
    paths = list((worker.state / 'sdkhost').glob(f'{worker.args.source}-*/status.json'))
    assert len(paths) == 1, paths
    return paths[0].parent, json.loads(paths[0].read_text())


def lose_private_host(host, root):
    pid = int((host / 'host.pid').read_text())
    argv = Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0')
    assert host.is_relative_to(root) and os.fsencode(host) in argv, argv
    os.kill(pid, signal.SIGKILL)
    until(lambda: not alive(pid), 'private sdkhost did not exit')


def checkpoint(worker):
    paths = [p for p in (worker.state / 'sdk-adapters' / worker.args.source).glob('*.json')
             if not p.name.endswith('-outbox.json')]
    assert len(paths) == 1, paths
    return json.loads(paths[0].read_text())


def blocked_receipt(host, source):
    prefix = 'request:' if source == 'droid' else 'user:'
    rows = [json.loads(line) for line in (host / 'receipts.jsonl').read_text().splitlines() if line]
    if source == 'droid':
        # Control calls share the request prefix. The pipe-blocked call is
        # the only one without a terminal receipt.
        request_ids = {row['id'] for row in rows if row['id'].startswith(prefix)}
        pending = request_ids - {row['id'] for row in rows if row['receipt']['state'] == 'written'}
        return next(iter(pending)) if len(pending) == 1 else None
    user = [row for row in rows if row['id'].startswith(prefix)]
    return user[-1]['id'] if user and user[-1]['receipt']['state'] == 'unknown' else None


def run_peer():
    root = Path(os.environ['SESSIONDOCK_FAKE_HOSTED_ROOT'])
    source = os.environ['SESSIONDOCK_FAKE_HOSTED_SOURCE']
    sid = os.environ['SESSIONDOCK_FAKE_HOSTED_SID']
    gate = os.environ['SESSIONDOCK_FAKE_HOSTED_GATE']
    if gate == PAUSE:
        # The private host uses a Linux pipe. Keep the payload small enough to
        # enter normally in Chromium while forcing a pending write.
        import fcntl
        fcntl.fcntl(sys.stdin.fileno(), fcntl.F_SETPIPE_SZ, 4096)
    target = sys.argv[1]
    sys.argv = sys.argv[1:]

    def output(text, key):
        message = {'role': 'assistant', 'content': [{'type': 'text', 'text': text}]}
        if source == 'droid':
            frame = {'jsonrpc': '2.0', 'factoryApiVersion': '1.0.0', 'type': 'notification',
                     'method': 'droid.session_notification', 'params': {
                         'sessionId': sid, 'notification': {'type': 'create_message',
                                                             'message': {'id': key, **message}}}}
        else:
            frame = {'type': 'assistant', 'session_id': sid, 'uuid': key, 'message': {'id': key, **message}}
        print(json.dumps(frame), flush=True)

    def hold_pipe():
        # A separate process owns both output descriptors after the CLI exits.
        # A bounded wait also releases it if the fixture is interrupted.
        return subprocess.Popen([sys.executable, '-c',
            "from pathlib import Path; import sys,time\n"
            "path=Path(sys.argv[1]); end=time.monotonic()+45\n"
            "while not path.is_file() and time.monotonic()<end: time.sleep(.03)",
            str(root / 'release-descendant')], stdin=subprocess.DEVNULL)

    class GatedInput:
        def __init__(self, pipe):
            self.pipe = pipe

        def __iter__(self):
            armed = False
            while line := self.pipe.readline(2048):
                if not line.endswith('\n'):
                    if armed and 'private input ' in line:
                        until(lambda: (root / 'request-output').exists(), 'output was not requested', timeout=60)
                        print(STDERR_TEXT, file=sys.stderr, flush=True)
                        output(STDOUT_TEXT, 'private-backpressure-output')
                        until(lambda: (root / 'release-fixture').exists(), 'input was not released', timeout=60)
                    line += self.pipe.readline()
                frame = json.loads(line)
                yield line
                if source == 'droid':
                    text = frame.get('params', {}).get('text')
                else:
                    text = frame.get('message', {}).get('content')
                if text != gate:
                    continue
                (root / 'gate-ready').touch()
                if gate == OFFLINE:
                    until(lambda: (root / 'exit-leader').exists(), 'test did not request the native exit')
                    output(OFFLINE_TEXT, 'private-offline-output')
                    child = hold_pipe()
                    (root / 'descendant').write_text(str(child.pid))
                    os._exit(0)
                armed = True

    sys.stdin = GatedInput(sys.stdin)
    runpy.run_path(target, run_name='__main__')


if __name__ == '__main__':
    run_peer()
