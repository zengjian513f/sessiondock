"""Private real collector with boot-long evidence and retained process ancestry."""
# run_validation: skip
from contextlib import contextmanager
import json
import os
import subprocess
import tempfile
import time


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
            while not (root / 'collector.sock').exists():
                assert child.poll() is None, 'private collector exited'
                assert time.monotonic() < deadline, 'private collector startup timed out'
                time.sleep(.1)
            yield (size, None), []
        finally:
            child.terminate()
            child.wait(timeout=30)
