"""Private SDK and CLI fixtures for explicit session start/stop browser suites."""
# run_validation: skip
from contextlib import contextmanager
import json
from types import SimpleNamespace
from unittest.mock import patch

from agy_sdk_fixtures import AgyFixture
from claude_sdk_fixtures import ClaudeFixture
from codex_sdk_browser import build, server
from codex_sdk_fixtures import FakeAppServer
from codex_sdk_actions_browser import environment
from grok_sdk_fixtures import GrokFixture
from history_fixtures import REPO, Corpus, isolated_server
from node_auth_fixtures import node_env
from hub_fixtures import free_port
from opencode_sdk_fixtures import FakeOpenCode
from send_browser import initialize


@contextmanager
def provider(source, root, binary, *, cli=False, report=False):
    extra = node_env(root, free_port(), '127.0.0.0/8')
    if report:
        for name in ('reports', 'audit', 'report-work'):
            (root/name).mkdir()
        extra.update(SESSIONDOCK_BUG_REPORT_DIR=str(root/'reports'),
            SESSIONDOCK_BUG_REPORT_WORKER='true', SESSIONDOCK_BUG_REPORT_REPO=str(root/'report-work'),
            SESSIONDOCK_AUDIT_DIR=str(root/'audit'))
    if cli:
        (root/'menu-host').mkdir()
        from terminal_browser import SHELL_SCRIPT
        launcher = root/'menu-cli-launcher.json'
        launcher.write_text(json.dumps({'schema':2, 'host_binary':str(REPO/'target/debug/ptyhost'),
            'host_dir':str(root/'menu-host'), 'profiles':[{'id':'menu-'+source, 'source':source,
            'executable':'/bin/sh', 'args':['-c', SHELL_SCRIPT]}]}))
        initialize('--initialize-lifecycle', root/'menu-ledger', binary.resolve())
        extra.update(SESSIONDOCK_LAUNCHER_CONFIG=str(launcher), SESSIONDOCK_PTYHOST_DIR=str(root/'menu-host'),
            SESSIONDOCK_LIFECYCLE_DIR=str(root/'menu-ledger'), SESSIONDOCK_NATIVE_SDKS='off',
            SESSIONDOCK_CODEX_APP_SERVER='off')
    if source == 'claude':
        peer = ClaudeFixture(root)
        with peer.server(binary, extra) as base:
            yield base, peer, extra
    elif source == 'codex':
        corpus = build(root)
        if cli:
            work = root/'work'; work.mkdir()
            path = corpus.paths['sdk']
            path.write_text(path.read_text().replace('/synthetic/sdk', str(work)))
        with StartDaemon(root/'app.sock', corpus.paths['sdk'], '01a1sdk0-0000-7000-8000-000000000001') as peer:
            with server(corpus, binary, root/'app.sock', {**environment(root, binary), **extra}) as base:
                yield base, peer, extra
    elif source == 'grok':
        for name in ('claude', 'codex', 'grok', 'state'):
            (root/name).mkdir()
        peer = GrokFixture(root)
        initialize('--initialize-lifecycle', root/'ledger', binary.resolve())
        with server(SimpleNamespace(root=root), binary, 'off', {
            **peer.env, 'SESSIONDOCK_NATIVE_SDKS': 'auto',
            'SESSIONDOCK_LIFECYCLE_DIR': str(root/'ledger'),
            'SESSIONDOCK_PTYHOST_DIR': str(root/'unused-host'),
            'SESSIONDOCK_STATE_DIR': str(root/'state'), **extra,
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
                    lifecycle_dir=root/'ledger', launcher_config=launcher, extra_env={
                        'SESSIONDOCK_NATIVE_SDKS': 'auto', 'SESSIONDOCK_OPENCODE_DB': str(peer.db),
                        'SESSIONDOCK_OPENCODE_ROOT': str(root/'mirror'),
                        'SESSIONDOCK_PROC_ROOT': str(root/'proc'), **extra}) as (base, _):
                yield base, peer, extra


class StartDaemon(FakeAppServer):
    async def _handle(self, writer, message):
        if message.get('method') == 'thread/loaded/list':
            await self.send(writer, {'id': message['id'], 'result': {'data': []}})
            return
        await super()._handle(writer, message)

