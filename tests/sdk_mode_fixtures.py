"""Private provider bridges for SDK/CLI mode selection browser coverage."""
# run_validation: skip
from contextlib import contextmanager
import json
from types import SimpleNamespace
from unittest.mock import patch

from agy_sdk_fixtures import AgyFixture
from claude_sdk_fixtures import ClaudeFixture
from codex_sdk_browser import build, server
from codex_sdk_status_browser import StatusDaemon
from grok_sdk_fixtures import GrokFixture
from history_fixtures import REPO, Corpus, isolated_server
from node_auth_fixtures import node_env
from hub_fixtures import free_port
from opencode_sdk_fixtures import FakeOpenCode
from send_browser import initialize


@contextmanager
def provider(source, root, binary):
    extra = node_env(root, free_port(), '127.0.0.0/8')
    if source == 'claude':
        peer = ClaudeFixture(root)
        with peer.server(binary, extra) as base:
            yield base, peer, extra
    elif source == 'codex':
        corpus = build(root)
        with StatusDaemon(root/'app.sock', corpus.paths['sdk'], '01a1sdk0-0000-7000-8000-000000000001') as peer:
            with server(corpus, binary, root/'app.sock', extra) as base:
                yield base, peer, extra
    elif source == 'grok':
        for name in ('claude', 'codex', 'grok', 'state'):
            (root/name).mkdir()
        peer = GrokFixture(root)
        initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
        with server(SimpleNamespace(root=root), binary, 'off', {
            **peer.env, **extra, 'SESSIONDOCK_NATIVE_SDKS': 'auto',
            'SESSIONDOCK_LIFECYCLE_DIR': str(root/'ledger'),
            'SESSIONDOCK_PTYHOST_DIR': str(root/'unused-host'),
            'SESSIONDOCK_STATE_DIR': str(root/'state'),
        }) as base:
            yield base, peer, extra
    elif source == 'agy':
        peer = AgyFixture(root, REPO/'target/debug/ptyhost')
        with patch.dict('os.environ', peer.env):
            initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
            with isolated_server(Corpus(root), binary, host_dir=root/'host', lifecycle_dir=root/'ledger',
                    state_dir=root/'state', launcher_config=peer.launcher,
                    extra_env={**peer.extra_env, **extra}) as (base, _):
                yield base, peer, extra
    else:
        for name in ('claude', 'codex', 'grok', 'host', 'state', 'proc'):
            (root/name).mkdir()
        (root/'proc/stat').write_text('btime 1700000000\n')
        with FakeOpenCode(root) as peer:
            launcher = root/'launcher.json'
            launcher.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO/'target/debug/ptyhost'),
                'host_dir': str(root/'host'), 'profiles': [peer.profile()]}))
            initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
            with isolated_server(Corpus(root), binary, host_dir=root/'host', state_dir=root/'state',
                    lifecycle_dir=root/'ledger', launcher_config=launcher, extra_env={**extra,
                        'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_OPENCODE_DB': str(peer.db),
                        'SESSIONDOCK_OPENCODE_ROOT': str(root/'mirror'),
                        'SESSIONDOCK_PROC_ROOT': str(root/'proc')}) as (base, _):
                yield base, peer, extra
