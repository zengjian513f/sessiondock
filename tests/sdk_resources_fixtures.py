"""Private native protocol peers and read-only inspection of their SDK hosts."""
# run_validation: skip
from contextlib import contextmanager
import json
import os
from pathlib import Path
import socket
from types import SimpleNamespace

from agy_sdk_fixtures import AgyFixture, SID as AGY_SID
from claude_sdk_fixtures import ClaudeFixture
from codex_sdk_browser import server
from grok_sdk_fixtures import GrokFixture
from history_fixtures import Corpus, isolated_server
from send_browser import initialize


def peer(source, root, binary):
    if source == 'claude':
        fixture = ClaudeFixture(root)
    elif source == 'grok':
        fixture = GrokFixture(root)
    else:
        fixture = AgyFixture(root, binary.resolve().with_name('ptyhost'))
        fixture.sid = AGY_SID
    for name in ('claude', 'codex', 'grok', 'state'):
        (root / name).mkdir(exist_ok=True)
    fixture.proc = root / 'proc'
    (fixture.proc / 'sys/kernel/random').mkdir(parents=True)
    (fixture.proc / 'sys/kernel/random/boot_id').write_text('private-sdk-resource-boot')
    (root / 'node-id').write_text('a' * 32)
    if source != 'claude':
        initialize('--initialize-lifecycle', root / 'ledger', binary.resolve())
    return fixture


@contextmanager
def serve(source, fixture, binary, with_agent):
    root = fixture.root
    extra = {'SESSIONDOCK_NODE_ID_FILE':str(root / 'node-id'),
             'SESSIONDOCK_NATIVE_SDKS':'auto',
             'SESSIONDOCK_RESOURCE_AGENT_MODE':'auto' if with_agent else 'off',
             'SESSIONDOCK_RESOURCE_AGENT_SOCKET':str(root / 'collector.sock')}
    if source == 'claude':
        with fixture.server(binary, extra, preserve_sdkhosts=True) as base:
            yield base
    elif source == 'grok':
        with server(SimpleNamespace(root=root), binary, 'off', {
            **fixture.env, **extra, 'SESSIONDOCK_LIFECYCLE_DIR':str(root / 'ledger'),
            'SESSIONDOCK_PTYHOST_DIR':str(root / 'unused-host'),
            'SESSIONDOCK_STATE_DIR':str(root / 'state')}) as base:
            yield base
    else:
        with isolated_server(Corpus(root), binary, host_dir=root / 'host',
            lifecycle_dir=root / 'ledger', state_dir=root / 'state',
            launcher_config=fixture.launcher, extra_env={**fixture.extra_env, **extra}) as (base, _):
            yield base


def active_hosts(root):
    result = []
    for path in (root / 'state/sdkhost').glob('*/control.sock'):
        try:
            with socket.socket(socket.AF_UNIX) as client:
                client.settimeout(2)
                directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
                try:
                    client.connect(f'/proc/self/fd/{directory}/control.sock')
                finally:
                    os.close(directory)
                client.sendall(b'"Status"\n')
                value = json.loads(client.makefile('rb').readline())['value']
                if value['exit'] is None:
                    result.append(value['pid'])
        except (OSError, ValueError):
            continue
    return result
