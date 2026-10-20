"""Explicit teardown for SDK hosts inside a synthetic fixture's private root."""
# run_validation: skip
from __future__ import annotations

import json
import os
from pathlib import Path
import signal
import socket
import time


def stop_private_sdkhosts(root):
    root = Path(root).resolve()
    failures = []

    def alive(pid):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        try:
            return (Path('/proc') / str(pid) / 'stat').read_text().rsplit(')', 1)[1].split()[0] != 'Z'
        except FileNotFoundError:
            return False

    def wait_exited(pid):
        deadline = time.monotonic() + 5
        while alive(pid) and time.monotonic() < deadline:
            time.sleep(.02)
        assert not alive(pid), ('private SDK fixture process survived teardown', pid)

    def rpc(path, request):
        assert path.resolve().is_relative_to(root)
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(3)
            directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                client.connect(f'/proc/self/fd/{directory}/control.sock')
            finally:
                os.close(directory)
            client.sendall(json.dumps(request).encode() + b'\n')
            with client.makefile('rb') as response:
                value = json.loads(response.readline())
        assert not value.get('error'), value
        return value['value']

    for path in (root / 'state/sdkhost').glob('*/control.sock'):
        try:
            host_pid = int((path.parent / 'host.pid').read_text())
            status = rpc(path, 'Status')
            if status['exit'] is None or not (status.get('stdout_eof', False) and status.get('stderr_eof', False)):
                try:
                    rpc(path, {'Signal': {'signal': signal.SIGKILL}})
                except (json.JSONDecodeError, ConnectionResetError):
                    pass  # The private host can exit before returning the signal ACK.
            wait_exited(status['pid'])
            wait_exited(host_pid)
        except (FileNotFoundError, ConnectionRefusedError):
            continue  # Explicit fixture stop already released this host.
        except Exception as error:
            failures.append(str(error))
    assert not failures, failures
