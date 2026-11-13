"""Restartable private node for SDK creation browser regressions."""
from contextlib import contextmanager
import json
import os
import subprocess
import sys
import tempfile
import time
from urllib.request import ProxyHandler, build_opener

from frontend_paths import frontend_dir
from history_fixtures import REPO, get_json
from codex_sdk_fixtures import FakeAppServer


class UnloadedCreationServer(FakeAppServer):
    """A native daemon that forgets unpersisted threads during a node outage."""

    def __init__(self, *args):
        super().__init__(*args)
        self.loaded = set()
        self.fail_unsubscribe = False

    async def _handle(self, writer, message):
        method, params = message.get('method'), message.get('params') or {}
        mid = message.get('id')
        if method == 'config/read':
            result = {'config': {'model_provider': 'openai'}}
        elif method == 'thread/loaded/list':
            result = {'data': sorted(self.loaded)}
        elif method == 'thread/unsubscribe':
            if self.fail_unsubscribe:
                await self.send(writer, {'id': mid, 'error': {
                    'code': -32000, 'message': 'synthetic unsubscribe failure'}})
                return
            result = {'status': 'unsubscribed' if params['threadId'] in self.loaded else 'notLoaded'}
            self.loaded.discard(params['threadId'])
        elif method in ('thread/read', 'thread/resume', 'turn/interrupt') and params.get('threadId') not in self.loaded:
            await self.send(writer, {'id': mid, 'error': {
                'code': -32602, 'message': 'no rollout found for unloaded thread'}})
            return
        elif method == 'thread/read':
            result = {'thread': {'id': params['threadId'], 'modelProvider': 'openai',
                                 'status': {'type': 'idle'}}}
        else:
            await super()._handle(writer, message)
            if method == 'thread/start':
                self.loaded.add(self.thread_id)
            return
        await self.send(writer, {'id': mid, 'result': result})


@contextmanager
def node(corpus, binary, socket, port, state, extra_env=None):
    environment = {key: value for key, value in os.environ.items() if not key.startswith('SESSIONDOCK_')}
    proc_root = corpus.root / 'sdk-proc'
    proc_root.mkdir(exist_ok=True)
    (proc_root / 'stat').write_text('btime 1700000000\n')
    host = corpus.root / 'host'
    ledger = corpus.root / 'ledger'
    host.mkdir(mode=0o700, exist_ok=True)
    if not ledger.exists():
        subprocess.run([str(binary.resolve()), '--initialize-lifecycle', str(ledger)],
                       check=True, capture_output=True, timeout=20)
    config = corpus.root / 'launcher.json'
    config.write_text(json.dumps({'schema': 2, 'host_binary': str(REPO / 'target/debug/ptyhost'),
        'host_dir': str(host), 'profiles': [{'id': 'codex-cli', 'source': 'codex',
        'executable': sys.executable, 'args': [str(REPO / 'tests/fake_model_cli.py')]}]}))
    config.chmod(0o600)
    environment.update(SESSIONDOCK_BIND=f'127.0.0.1:{port}', SESSIONDOCK_WEB_DIR=str(frontend_dir()),
                       SESSIONDOCK_CODEX_APP_SERVER=str(socket), SESSIONDOCK_STATE_DIR=str(state),
                       SESSIONDOCK_PROC_ROOT=str(proc_root), SESSIONDOCK_PTYHOST_DIR=str(host),
                       SESSIONDOCK_LIFECYCLE_DIR=str(ledger), SESSIONDOCK_LAUNCHER_CONFIG=str(config),
                       SESSIONDOCK_FILE_ROOTS=str(corpus.root), SESSIONDOCK_FILE_WRITE_ROOTS=str(corpus.root))
    for source in ('claude', 'codex', 'grok'):
        environment['SESSIONDOCK_' + source.upper() + '_ROOT'] = str(corpus.root / source)
    environment.update(extra_env or {})
    base = f'http://127.0.0.1:{port}'
    with tempfile.TemporaryFile(mode='w+b') as log:
        process = subprocess.Popen([str(binary.resolve(strict=True))], cwd=REPO, env=environment,
                                   stdout=log, stderr=log)
        try:
            opener = build_opener(ProxyHandler({}))
            deadline = time.monotonic() + 10
            while True:
                assert process.poll() is None, 'isolated node exited early'
                try:
                    get_json(opener, base, '/api/health')
                    break
                except OSError:
                    assert time.monotonic() < deadline, 'isolated node did not become ready'
                    time.sleep(0.05)
            yield base
        except BaseException:
            log.seek(0)
            print(log.read().decode('utf-8', 'replace'), flush=True)
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
