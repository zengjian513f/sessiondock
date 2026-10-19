"""Reuse private synthetic native peers for durable input queue browser tests."""
# run_validation: skip
from __future__ import annotations

from contextlib import contextmanager
import json
from pathlib import Path

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
        return [row for rows in json.loads(path.read_text()).values() for row in rows] if path.exists() else []

    def entry(self, request):
        return next(row for row in self.journal() if row['request_id'] == request)

    @property
    def blocker(self):
        return {'grok': 'steer-active', 'agy': 'hold:queue', 'claude': 'hold'}[self.source]
