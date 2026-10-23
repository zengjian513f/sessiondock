"""Private real collector with boot-long evidence and retained process ancestry."""
# run_validation: skip
from contextlib import contextmanager
import json
import os
import socket
import subprocess
import tempfile
import time
from pathlib import Path


@contextmanager
def real_collector(root, binary, node, count):
    session = dict(node_id=node, source='codex', sid='small-0', title='Synthetic owner ' * 12)
    process = lambda pid: dict(pid=pid, start=pid * 100)
    record = dict(process=process(200),
        connection=dict(client_ip='10.0.0.1', client_port=50000, server_ip='10.0.0.2', server_port=50022),
        ssh=True, shared=False, session=session,
        launch_chain=[dict(process=dict(node_id=node, boot_id='private-boot', process=process(100)), session=session)])
    saved = dict(node_id=node, boot_id='private-boot', catalog=dict(node_id=node,
        boot_id='private-boot', owners=[], sessions=[]), links=[], bindings=[], connections=dict(
            records=[dict(record, opened_at=i+1., closed_at=i+2.) for i in range(count)],
            parents=[[process(pid), process(pid+1)] for pid in range(200, 350)], origins=[], shared=[]))
    path = root / 'agent-state.json'
    path.write_text(json.dumps(saved, separators=(',', ':')))
    size = path.stat().st_size
    del saved
    with tempfile.TemporaryFile() as log:
        child = subprocess.Popen([str(binary.resolve()), '--uid', str(os.getuid()),
            '--node-id-file', str(root / 'ids/node-id'), '--socket', str(root / 'collector.sock'),
            '--state', str(path), '--proc-root', str(root / 'proc'), '--events', 'off'],
            stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 60
            while True:
                assert child.poll() is None, 'private collector exited'
                assert time.monotonic() < deadline, 'private collector startup timed out'
                try:
                    with socket.socket(socket.AF_UNIX) as client:
                        client.settimeout(1)
                        client.connect(str(root / 'collector.sock'))
                        client.sendall(b'{"op":"health"}\n')
                        if json.loads(client.makefile('rb').readline()).get('ok'):
                            break
                except (OSError, ValueError):
                    pass
                time.sleep(.1)
            yield (size, None), []
            assert child.poll() is None, 'collector exited while serving retained history'
            deadline = time.monotonic() + 30
            while True:
                persisted = json.loads(path.read_text())
                # A completed refresh writes current native owners alongside
                # every old connection, rather than truncating boot evidence.
                if persisted['catalog']['owners']:
                    assert len(persisted['connections']['records']) == count
                    break
                assert time.monotonic() < deadline, 'collector did not persist refreshed attribution'
                time.sleep(.1)
            peak = next(line for line in Path(f'/proc/{child.pid}/status').read_text().splitlines()
                        if line.startswith('VmHWM:'))
            print(f'Real collector retained {count} records through streaming save; {peak}', flush=True)
        finally:
            child.terminate()
            try:
                child.wait(timeout=30)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
                raise
