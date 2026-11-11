"""Private Windows native fixture controlled by the Chromium real suite.

No production roots, account files, model calls, or native session deletion.
CLI paths are explicit arguments; the output never contains listener tokens.
"""
# run_validation: skip
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.request import ProxyHandler, build_opener

import psutil

SID = '01a11d3f-7510-7d20-8f67-546a6975ebef'
OTHER = '01a11d3f-7510-7d20-8f67-546a6975ebee'


def seed(home, work, sid, provider, label):
    folder = home / 'sessions/2026/11/11'; folder.mkdir(parents=True, exist_ok=True)
    rows = [('session_meta', {'id': sid, 'timestamp': '2026-11-11T00:00:00Z', 'cwd': str(work),
        'originator': 'codex_cli_rs', 'cli_version': '0.167.1', 'source': 'cli',
        'model_provider': provider, 'history_mode': 'paginated'}),
        ('turn_context', {'turn_id': 'synthetic-turn', 'model': 'gpt-6-luna', 'effort': 'low', 'cwd': str(work)}),
        ('response_item', {'type': 'message', 'role': 'user', 'content': [{'type': 'input_text', 'text': label}]}),
        ('response_item', {'type': 'message', 'role': 'assistant', 'content': [{'type': 'output_text', 'text': 'Windows synthetic preserved answer'}]})]
    (folder / f'rollout-2026-11-11T00-00-00-{sid}.jsonl').write_text(''.join(
        json.dumps({'timestamp': '2026-11-11T00:00:00Z', 'type': t, 'payload': p, 'ordinal': i})+'\n'
        for i, (t, p) in enumerate(rows)), encoding='utf-8')


def prepare(args):
    for key in ('binary', 'codex', 'ptyhost', 'web'):
        setattr(args, key, str(Path(getattr(args, key)).resolve(strict=True)))
    root = Path(tempfile.mkdtemp(prefix='sessiondock-codex-browser-')).resolve()
    for name in ('home', 'state', 'work', 'host', 'claude', 'grok'):
        (root / name).mkdir()
    home = root / 'home/.codex'; home.mkdir()
    (home / 'config.toml').write_text('''model="gpt-6-luna"
model_reasoning_effort="low"
[model_providers.openai_catalog_cache]
name="Synthetic offline catalog"
wire_api="responses"
base_url="http://127.0.0.1:9/v1"
''')
    seed(home, root / 'work', SID, 'openai', 'Windows primary provider fixture')
    seed(home, root / 'work', OTHER, 'openai_catalog_cache', 'Windows secondary provider fixture')
    launcher = root / 'launcher.json'
    launcher.write_text(json.dumps({'host_binary': args.ptyhost, 'host_dir': str(root / 'host'),
        'profiles': [{'id': 'codex-native', 'source': 'codex', 'executable': args.codex,
            'args': ['-m', 'gpt-6-luna', '-c', 'model_reasoning_effort="low"'],
            'env': {'CODEX_HOME': str(home), 'HOME': str(root / 'home'), 'USERPROFILE': str(root / 'home')}}]}))
    subprocess.run([args.binary, '--initialize-lifecycle', str(root / 'ledger')], check=True, capture_output=True)
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', args.port or 0)); port = reservation.getsockname()[1]
    data = {'root': str(root), 'port': port, 'binary': args.binary, 'env': {
        'SESSIONDOCK_BIND': f'127.0.0.1:{port}', 'SESSIONDOCK_WEB_DIR': args.web,
        'SESSIONDOCK_STATE_DIR': str(root / 'state'), 'SESSIONDOCK_CODEX_ROOT': str(home / 'sessions'),
        'SESSIONDOCK_CLAUDE_ROOT': str(root / 'claude'), 'SESSIONDOCK_GROK_ROOT': str(root / 'grok'),
        'SESSIONDOCK_PTYHOST_DIR': str(root / 'host'), 'SESSIONDOCK_LIFECYCLE_DIR': str(root / 'ledger'),
        'SESSIONDOCK_LAUNCHER_CONFIG': str(launcher), 'SESSIONDOCK_NATIVE_SDKS': 'off',
        'HOME': str(root / 'home'), 'USERPROFILE': str(root / 'home'), 'CODEX_HOME': str(home)}}
    (root / 'fixture.json').write_text(json.dumps(data))
    start(root, data)
    return {'root': str(root), 'port': port}


def start(root, data):
    env = {k: v for k, v in os.environ.items() if not k.startswith('SESSIONDOCK_')}
    env.update(data['env'])
    with (root / 'service.log').open('ab') as log:
        proc = subprocess.Popen([data['binary']], env=env, cwd=root, stdin=subprocess.DEVNULL,
            stdout=log, stderr=log, creationflags=0x00000008 | 0x00000200 | 0x01000000)
    (root / 'service.json').write_text(json.dumps({'pid': proc.pid, 'created': psutil.Process(proc.pid).create_time()}))
    for _ in range(200):
        assert proc.poll() is None, (root / 'service.log').read_text(errors='replace')
        try:
            with build_opener(ProxyHandler({})).open(f'http://127.0.0.1:{data["port"]}/api/health', timeout=1): return
        except OSError: time.sleep(.1)
    raise AssertionError('private Windows service did not start')


def stop_process(pid, created, descendants=False):
    try:
        process = psutil.Process(pid)
        assert abs(process.create_time() - created) < .01, 'fixture PID incarnation changed'
        children = process.children(recursive=True) if descendants else []
        process.terminate(); process.wait(10)
        for child in children:
            try: child.terminate()
            except psutil.NoSuchProcess: pass
        psutil.wait_procs(children, timeout=10)
    except psutil.NoSuchProcess: pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('mode', choices=('prepare', 'snapshot', 'restart', 'kill-listener', 'stop'))
    parser.add_argument('--root', type=Path)
    parser.add_argument('--port', type=int)
    for key in ('binary', 'codex', 'ptyhost', 'web'): parser.add_argument('--' + key)
    args = parser.parse_args()
    assert os.name == 'nt'
    if args.mode == 'prepare':
        print(json.dumps(prepare(args))); return
    root = args.root.resolve()
    assert root.name.startswith('sessiondock-codex-browser-') and (root / 'fixture.json').is_file()
    data = json.loads((root / 'fixture.json').read_text())
    records = [json.loads(p.read_text()) for p in (root / 'home/.codex/sd-providers').glob('*.listener.json')]
    if args.mode == 'kill-listener':
        record = next(r for r in records if r['provider'] == 'openai')
        stop_process(record['pid'], record['started'] / 10_000_000, descendants=True)
        print(json.dumps({'ok': True})); return
    if args.mode == 'snapshot':
        token_files = list((root / 'home/.codex/sd-providers').glob('*.listener.token'))
        private_acl = True
        script = "$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $a=Get-Acl -LiteralPath $env:SESSIONDOCK_TEST_ACL_PATH; $bad=@($a.Access | Where-Object { $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value -ne $sid }); if ($bad.Count -gt 0) { exit 1 }"
        for path in token_files:
            result = subprocess.run(['powershell', '-NoProfile', '-NonInteractive', '-Command', script],
                env={**os.environ, 'SESSIONDOCK_TEST_ACL_PATH': str(path)}, capture_output=True)
            private_acl = private_acl and result.returncode == 0
        terminals = []
        for record_path in (root / 'host').glob('*.json'):
            record = json.loads(record_path.read_text())
            try:
                process = psutil.Process(record['pid'])
                command = process.cmdline()
                if '--remote' in command:
                    terminals.append({'pid': process.pid, 'argv': command})
            except (psutil.Error, KeyError): pass
        print(json.dumps({'listeners': records, 'terminals': terminals, 'private_acl': private_acl})); return
    service = json.loads((root / 'service.json').read_text())
    stop_process(service['pid'], service['created'])
    if args.mode == 'restart': start(root, data)
    else:
        for record in records:
            if record['pid']: stop_process(record['pid'], record['started'] / 10_000_000, descendants=True)
        # PTY fixtures are also test-owned; never inspect production host records.
        for record_path in (root / 'host').glob('*.json'):
            record = json.loads(record_path.read_text())
            pid = record.get('host_pid')
            if pid:
                try:
                    process = psutil.Process(pid)
                    if str(root).lower() in process.cwd().lower():
                        stop_process(pid, process.create_time(), descendants=True)
                except psutil.NoSuchProcess: pass
    print(json.dumps({'ok': True}))


if __name__ == '__main__': main()
