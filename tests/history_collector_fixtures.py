"""Private real collector with expired evidence, live ancestry and bounded recovery."""
# run_validation: skip
from contextlib import contextmanager
import json
import os
import socket
import sqlite3
import resource
from threading import Thread
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
    path = root / 'agent-state.json'
    database = path.with_suffix('.sqlite3')
    scope = json.dumps([node, 'private-boot'], separators=(',', ':'))
    old_scope = json.dumps([node, 'previous-boot'], separators=(',', ':'))
    metadata = dict(node_id=node, boot_id='private-boot', catalog=dict(node_id=node,
        boot_id='private-boot', owners=[], sessions=[]), links=[], bindings=[],
        last_seen=[])
    now = time.time()
    with sqlite3.connect(database) as db:
        db.executescript('CREATE TABLE snapshots(scope TEXT PRIMARY KEY,data TEXT NOT NULL);'
            'CREATE TABLE evidence(scope TEXT,section TEXT,key TEXT,data TEXT NOT NULL,PRIMARY KEY(scope,section,key)) WITHOUT ROWID;')
        db.execute('INSERT INTO snapshots VALUES(?,?)', (scope,json.dumps(metadata)))
        db.execute('INSERT INTO snapshots VALUES(?,?)', (old_scope,json.dumps(dict(metadata,boot_id='previous-boot'))))
        def rows():
            import struct
            for i in range(count+9):
                row = dict(record, opened_at=i+1., closed_at=i+2.)
                if i >= count:
                    row.update(opened_at=now-30+i-count,closed_at=now-20+i-count)
                if i == count+4:
                    row.update(process=process(100),opened_at=1.,closed_at=2.) # live incarnation survives
                if i == count+5:
                    row.update(process=process(999),opened_at=now-100000,closed_at=None) # missing close, persisted absence expires
                if i == count+6:
                    row.update(process=process(101),opened_at=1.,closed_at=None) # live SSH, socket demonstrably gone
                if i >= count+7:
                    row.update(process=process(102+i-count-7),opened_at=1.,closed_at=None) # unknown socket coverage retains evidence
                key = [row['process'], row['connection'],struct.unpack('Q',struct.pack('d',row['opened_at']))[0]]
                yield (scope,'records',json.dumps(key,separators=(',',':')),json.dumps(row,separators=(',',':')))
            for i in range(30):
                row = dict(record, opened_at=i+1., closed_at=None)
                yield (old_scope,'records',str(i),json.dumps(row))
            for pid in range(200,350):
                yield (scope,'parents',json.dumps(process(pid),separators=(',',':')),json.dumps([process(pid),process(pid+1)]))
        db.executemany('INSERT INTO evidence VALUES(?,?,?,?)',rows())
    size = database.stat().st_size
    with tempfile.TemporaryFile() as log:
        command = [str(binary.resolve()), '--uid', str(os.getuid()),
            '--node-id-file', str(root / 'ids/node-id'), '--socket', str(root / 'collector.sock'),
            '--state', str(path), '--proc-root', str(root / 'proc'), '--events', 'off']
        started = time.monotonic()
        child = subprocess.Popen(command,stdout=log,stderr=log)
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
            health_times = []
            def cleanup_reads():
                for _ in range(15):
                    t = time.monotonic()
                    with socket.socket(socket.AF_UNIX) as client:
                        client.settimeout(1);client.connect(str(root/'collector.sock'))
                        client.sendall(b'{"op":"health"}\n');assert json.loads(client.makefile('rb').readline())['ok']
                    health_times.append((time.monotonic()-t)*1000)
                    time.sleep(.1)
            reader = Thread(target=cleanup_reads)
            reader.start()
            def exhaust_request_threads():
                reader.join(timeout=5)
                assert len(health_times)==15 and max(health_times)<200,health_times
                soft, hard = resource.prlimit(child.pid,resource.RLIMIT_NPROC)
                try:
                    resource.prlimit(child.pid,resource.RLIMIT_NPROC,(0,hard))
                    with socket.socket(socket.AF_UNIX) as client:
                        client.settimeout(.5);client.connect(str(root/'collector.sock'))
                        client.sendall(b'{"op":"health"}\n')
                        assert json.loads(client.makefile('rb').readline())['ok']
                    assert child.poll() is None,'collector stopped with thread creation disabled'
                finally:
                    resource.prlimit(child.pid,resource.RLIMIT_NPROC,(soft,hard))
                print('Requests succeeded with thread creation disabled',flush=True)
            print(f'Loaded {count} historical records; first health {(time.monotonic()-started):.3f}s', flush=True)
            assert time.monotonic()-started < 20, 'recovery blocked startup'
            yield (size, None), [exhaust_request_threads]
            assert child.poll() is None, 'collector exited while serving retained history'
            deadline = time.monotonic() + 40
            reader.join(timeout=5)
            while True:
                with sqlite3.connect(database) as db:
                    remaining = db.execute("SELECT count(*) FROM evidence WHERE section='records'").fetchone()[0]
                    old = db.execute('SELECT count(*) FROM snapshots WHERE scope=?',(old_scope,)).fetchone()[0]
                if remaining == 8 and old == 0: break
                assert time.monotonic() < deadline, (remaining, old)
                t = time.monotonic()
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(1);client.connect(str(root/'collector.sock'))
                    client.sendall(b'{"op":"health"}\n');assert json.loads(client.makefile('rb').readline())['ok']
                health_times.append((time.monotonic()-t)*1000)
                time.sleep(.1)
            child.terminate();child.wait(timeout=30)
            child = subprocess.Popen(command,stdout=log,stderr=log)
            deadline = time.monotonic()+20
            while True:
                try:
                    with socket.socket(socket.AF_UNIX) as client:
                        client.settimeout(1);client.connect(str(root/'collector.sock'))
                        client.sendall(b'{"op":"report"}\n');rows=json.loads(client.makefile('rb').readline())['result']['connections']
                    assert len(rows)==8, len(rows)
                    break
                except (OSError,ValueError):
                    assert time.monotonic()<deadline
                    time.sleep(.1)
            peak = next(line for line in Path(f'/proc/{child.pid}/status').read_text().splitlines() if line.startswith('VmHWM:'))
            print(f'Processed {count} old rows durably; restart retained 8 including supersession evidence; cleanup health max {max(health_times):.2f} ms; {peak}',flush=True)
        finally:
            child.terminate()
            try:
                child.wait(timeout=30)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
                raise
