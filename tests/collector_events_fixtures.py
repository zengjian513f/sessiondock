"""Controlled trace stream for private real-collector browser regressions."""
# run_validation: skip
import json
import os
import socket
import sys
import time


def report(root):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5)
        client.connect(str(root / 'agent.sock'))
        client.sendall(b'{"op":"report"}\n')
        return json.loads(client.makefile('rb').readline())['result']


def wait_report(root, predicate):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            value = report(root)
            if predicate(value):
                return value
        except (OSError, ValueError):
            pass
        time.sleep(.1)
    raise AssertionError('collector event observation timed out')


def tracer(root):
    events = root / 'trace-events'
    events.write_text('')
    script = root / 'trace-reader'
    script.write_text(f'''#!{sys.executable}
import time
print('READY', flush=True)
with open({str(events)!r}) as stream:
    while True:
        line = stream.readline()
        if line:
            print(line, end='', flush=True)
        else:
            time.sleep(.02)
''')
    script.chmod(0o700)
    return {**os.environ, 'RESOURCE_AGENT_BPFTRACE': str(script)}


def emit(root, text):
    with (root / 'trace-events').open('a') as stream:
        stream.write(text + '\n')


def opened(pid, start, at, port):
    # The real event parser converts monotonic nanoseconds to wall time and ticks.
    epoch = time.time() - time.clock_gettime(time.CLOCK_BOOTTIME)
    ticks = os.sysconf('SC_CLK_TCK')
    return (f'C {pid} {start * 1_000_000_000 // ticks} {int((at - epoch) * 1e9)} '
            f'1 10.0.0.1 {port} 10.0.0.2 50022')


def recover_after_loss(root, boot, stale_port=50001):
    wait_report(root, lambda r: r.get('collector', {}).get('ssh_connections'))
    emit(root, opened(201, 200, boot + 1, stale_port))
    wait_report(root, lambda r: any(c['process']['pid'] == 201
                                  for c in r.get('connections', [])))
    emit(root, 'lost 1 events')
    gap = wait_report(root, lambda r: r.get('collector', {}).get('lost_events') == 1)
    assert not gap['collector']['ssh_connections'], gap
    assert next(c for c in gap['connections'] if c['process']['pid'] == 201)['shared']
    # A new SSH connection after the gap must survive subsequent sample passes.
    emit(root, opened(200, 200, boot + 2, 50000))
    recovered = wait_report(root, lambda r: r['sampled_at'] > gap['sampled_at'] + 3
                            and any(c['process']['pid'] == 200 for c in r.get('connections', [])))
    assert recovered['collector']['lost_events'] == 1, recovered
    assert recovered['collector']['ssh_connections'], recovered
    assert next(c for c in recovered['connections'] if c['process']['pid'] == 201)['shared']
    assert not next(c for c in recovered['connections'] if c['process']['pid'] == 200)['shared']
