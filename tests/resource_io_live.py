#!/usr/bin/env python3
# run_validation: skip
"""Opt-in root-only, bounded native I/O smoke test. Uses no production agent.

Copy this script and the resource-agent binary to a private local directory
before sudo on root-squashed shared filesystems. --shared-dir is optional and
must be writable by --uid. All helper links are killed/reaped in finally.
"""
import argparse
import ctypes
import json
import os
import select
import shutil
import socket
import subprocess
import tempfile
import time


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--agent', required=True)
    ap.add_argument('--uid', type=int, default=65534)
    ap.add_argument('--shared-dir')
    args = ap.parse_args()
    if os.geteuid() != 0:
        ap.error('run with root privileges; this is an explicit opt-in test')
    tempfile.gettempdir()  # avoid tempfile's four-byte directory probe in child
    lib = ctypes.CDLL(None, use_errno=True)
    lib.fstatfs.argtypes = [ctypes.c_int, ctypes.c_void_p]

    def links():
        if not shutil.which('bpftool'):
            return set()
        return {row['id'] for row in json.loads(subprocess.check_output(['bpftool', '-j', 'link', 'show']))}

    before = links()
    start = time.monotonic()
    helper = subprocess.Popen([os.path.abspath(args.agent), '--io-probe-helper', str(args.uid)],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    owned = set()
    child = None
    try:
        def message():
            assert select.select([helper.stdout], [], [], 10)[0], 'helper output timeout'
            line = helper.stdout.readline()
            assert line, f'helper exited: {helper.stderr.read()[:2000]}'
            return json.loads(line)

        assert message()['event']['event'] == 'ready'
        ready_ms = (time.monotonic() - start) * 1000
        owned = links() - before
        expected = {'local_read': 4915200, 'local_write': 2277376, 'tcp_send': 8192, 'tcp_receive': 8192}
        identity_read, identity_write = os.pipe()
        child = os.fork()
        if child == 0:
            os.close(identity_read)
            os.setgid(args.uid)
            os.setuid(args.uid)
            stat = open('/proc/self/stat').read().rsplit(')', 1)[1].split()
            os.write(identity_write, json.dumps({'pid': os.getpid(), 'start': int(stat[19])}).encode())
            os.close(identity_write)
            with tempfile.TemporaryFile() as f:
                f.write(b'x' * 1048576)
                f.flush()
                for _ in range(1000):
                    os.pread(f.fileno(), 4096, 0)
                for _ in range(100):
                    os.pwrite(f.fileno(), b'x' * 4096, 0)
                for _ in range(100):
                    os.preadv(f.fileno(), [bytearray(4096), bytearray(4096)], 0)
                    os.pwritev(f.fileno(), [b'x' * 4096, b'x' * 4096], 0)
            if args.shared_dir:
                with tempfile.TemporaryFile(dir=args.shared_dir) as f:
                    f.write(b'x' * 4096)
                    f.flush()
                    for _ in range(100):
                        os.pread(f.fileno(), 4096, 0)
                        os.pwrite(f.fileno(), b'x' * 4096, 0)
                    filesystem = ctypes.create_string_buffer(256)
                    assert lib.fstatfs(f.fileno(), filesystem) == 0
                    magic = ctypes.cast(filesystem, ctypes.POINTER(ctypes.c_long))[0]
                    assert magic != 0  # Do not contaminate file counters by logging from the child.
            listener = socket.socket()
            listener.bind(('127.0.0.1', 0))
            listener.listen()
            sender = socket.socket()
            sender.connect(listener.getsockname())
            receiver, _ = listener.accept()
            sender.sendall(b'x' * 8192)
            assert len(receiver.recv(4096, socket.MSG_PEEK)) == 4096
            got = 0
            while got < 8192:
                got += len(receiver.recv(8192 - got))
            os._exit(0)
        os.close(identity_write)
        identity = json.loads(os.read(identity_read, 256))
        os.close(identity_read)
        _, status = os.waitpid(child, 0)
        assert os.waitstatus_to_exitcode(status) == 0
        child = None
        totals = {}
        operations = {}
        for _ in range(3):
            row = message()
            assert row['lost'] == 0, row
            for sample in row['event'].get('samples', []):
                if sample['process']['pid'] == identity['pid']:
                    assert sample['process'] == identity, (sample, identity)
                    operations[sample['kind']] = operations.get(sample['kind'], 0) + sample['operations']
                    totals[sample['kind']] = totals.get(sample['kind'], 0) + sample['bytes']
            if totals.get('tcp_receive') == 8192:
                break
        if args.shared_dir:
            if 'nfs_read' in totals:
                expected.update(nfs_read=409600, nfs_write=413696)
            else:
                expected['local_read'] += 409600
                expected['local_write'] += 413696
        assert totals == expected, (totals, expected)
        expected_operations = {'local_read':1100, 'local_write':201}
        if args.shared_dir:
            if 'nfs_read' in totals:
                expected_operations.update(nfs_read=100, nfs_write=101)
            else:
                expected_operations['local_read'] += 100
                expected_operations['local_write'] += 101
        assert {k:v for k,v in operations.items() if not k.startswith('tcp_')} == expected_operations, operations
        print(json.dumps({'passed': True, 'ready_ms': round(ready_ms, 2), 'links': len(owned),
                          'process': identity, 'bytes': totals, 'operations': operations}))
    finally:
        if child:
            os.kill(child, 9)
            os.waitpid(child, 0)
        helper.kill()
        helper.wait(timeout=3)
        diagnostics = helper.stderr.read()
        assert not diagnostics, diagnostics[:2000]
        assert not (links() & owned), 'helper links survived exit'


if __name__ == '__main__':
    main()
