"""Reuse private synthetic native peers for durable input queue browser tests."""
# run_validation: skip
from __future__ import annotations

from contextlib import contextmanager
import json
import os
from pathlib import Path
from storage_tiering_fixtures import read_inputs, legacy_inputs
import signal
import socket
import time

from agy_sdk_fixtures import AgyFixture
from claude_sdk_fixtures import ClaudeFixture
from grok_sdk_fixtures import GrokFixture
from history_fixtures import Corpus, isolated_server
from send_browser import initialize


class NativeQueueFixture:
    def __init__(self, root, source, binary, host_binary):
        self.root, self.source, self.binary = Path(root), source, binary
        self.root.mkdir()
        for name in ('claude', 'codex', 'grok', 'host', 'ledger', 'state'):
            (self.root / name).mkdir(exist_ok=True)
        if source == 'agy':
            self.peer = AgyFixture(self.root, host_binary.resolve(strict=True))
            self.work = self.peer.work
            from agy_sdk_fixtures import SID
            self.sid = SID
            self.extra = dict(self.peer.extra_env)
        elif source == 'grok':
            self.peer = GrokFixture(self.root)
            self.work, self.sid = self.peer.work, self.peer.sid
            self.extra = dict(self.peer.env)
            self.other_sid = '019a0000-0000-7000-8000-000000000063'
            other = self.peer.path.with_name(self.other_sid)
            other.mkdir()
            self.peer.write_summary(other, self.other_sid, str(self.work), 'Other queue watch identity')
            (other / 'chat_history.jsonl').write_text(json.dumps({'type': 'assistant', 'content': 'other native history'}) + '\n')
        else:
            self.peer = ClaudeFixture(self.root)
            self.work, self.sid = self.peer.cwd, self.peer.sid
            self.extra = {'SESSIONDOCK_CLAUDE_ROOT': str(self.peer.projects),
                          'SESSIONDOCK_PROC_ROOT': str(self.root / 'proc')}
        self.host_dir = Path(json.loads(self.peer.launcher.read_text())['host_dir'])
        self.extra['SESSIONDOCK_NATIVE_SDKS'] = 'auto'
        initialize('--initialize-lifecycle', self.root / 'ledger', binary.resolve(strict=True))

    @contextmanager
    def server(self):
        with isolated_server(Corpus(self.root), self.binary, host_dir=self.host_dir,
                             state_dir=self.root / 'state', lifecycle_dir=self.root / 'ledger',
                             launcher_config=self.peer.launcher, extra_env=self.extra) as (base, _):
            yield base

    def inputs(self, prefix=None):
        if self.source == 'grok':
            values = [row['params']['prompt'][0]['text'] for row in self.peer.received('session/prompt')]
        elif self.source == 'agy':
            values = [row['text'] for row in self.peer.inputs()]
        else:
            values = []
            for row in self.peer.frames('user'):
                value = row['message']['content']
                if isinstance(value, list):
                    value = '\n'.join(block.get('text', '') for block in value if block.get('type') == 'text')
                values.append(value)
        return [value for value in values if prefix is None or value.startswith(prefix)]

    def journal(self):
        path = self.root / 'state/native-sdk-inputs.json'
        return [row for rows in read_inputs(path).values() for row in rows]

    def entry(self, request):
        return next(row for row in self.journal() if row['request_id'] == request)

    def native_pid(self):
        if self.source == 'agy':
            return self.peer.inputs()[-1]['pid']
        if self.source == 'claude':
            return self.peer.frames('spawn')[-1]['pid']
        statuses = []
        for path in self.host_sockets():
            try:
                statuses.append(self.host_rpc(path, 'Status'))
            except (FileNotFoundError, ConnectionRefusedError):
                continue
        live = [row['pid'] for row in statuses if row['exit'] is None]
        assert len(live) == 1, ('expected exactly one live Grok SDK child', statuses)
        return live[0]

    def initialization_count(self):
        if self.source == 'agy':
            return sum('--input-format' in row.get('argv',[]) for row in self.peer.events() if row['kind']=='launch')
        if self.source == 'claude':
            return sum(frame.get('request', {}).get('subtype') == 'initialize'
                       for frame in self.peer.frames('control_request'))
        if self.source == 'grok':
            # Model discovery uses its own short-lived ACP peer at node start.
            # Only the conversation runtime must retain its initialization.
            return sum(frame.get('params', {}).get('clientInfo', {}).get('name') == 'sessiondock'
                       for frame in self.peer.received('initialize'))
        raise AssertionError('Initialization evidence is not configured for ' + self.source)

    def host_sockets(self):
        return list((self.root / 'state/sdkhost').glob('*/control.sock'))

    def host_rpc(self, path, request):
        assert path.resolve().is_relative_to(self.root.resolve())
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(3)
            directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                client.connect(f'/proc/self/fd/{directory}/control.sock')
            finally:
                os.close(directory)
            client.sendall(json.dumps(request).encode() + b'\n')
            with client.makefile('rb') as response:
                value = json.loads(response.readline())
        assert not value.get('error'), value
        return value['value']

    def close(self):
        """Only final fixture teardown ends detached hosts; server() never does."""
        failures = []
        for path in self.host_sockets():
            try:
                host_pid = int((path.parent / 'host.pid').read_text())
                status = self.host_rpc(path, 'Status')
                if status['exit'] is None or not (status.get('stdout_eof', False) and status.get('stderr_eof', False)):
                    try:
                        self.host_rpc(path, {'Signal': {'signal': signal.SIGKILL}})
                    except (json.JSONDecodeError, ConnectionResetError):
                        # Child exit can terminate the host before its ACK.
                        pass
                self.wait_exited(status['pid'])
                self.wait_exited(host_pid)
            except (FileNotFoundError, ConnectionRefusedError):
                # Explicit stop may already have removed this private host.
                continue
            except Exception as error:
                failures.append(str(error))
        assert not failures, failures

    @staticmethod
    def alive(pid):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        stat = Path('/proc') / str(pid) / 'stat'
        return not stat.exists() or stat.read_text().rsplit(')', 1)[1].split()[0] != 'Z'

    def wait_exited(self, pid):
        deadline = time.monotonic() + 5
        while self.alive(pid) and time.monotonic() < deadline:
            time.sleep(.02)
        assert not self.alive(pid), ('explicit stop left native child alive', pid)

    def wait_native_text(self, text):
        assert self.source == 'agy'
        path = self.peer.native / 'brain' / self.sid / '.system_generated/logs/transcript_full.jsonl'
        deadline = time.monotonic() + 5
        while text not in path.read_text() and time.monotonic() < deadline:
            time.sleep(.02)
        assert text in path.read_text(), 'native peer did not finish while node was offline'

    @property
    def blocker(self):
        return {'grok': 'steer-active', 'agy': 'hold:queue', 'claude': 'hold'}[self.source]
