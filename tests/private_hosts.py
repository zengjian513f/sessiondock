"""Own test hosts for the whole fixture, including across server restarts."""
from contextlib import contextmanager
import json
from pathlib import Path
import signal
import socket
import time


def running_identity(pid):
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        return fields[19] if fields[0] != "Z" else None
    except FileNotFoundError:
        return None


def cleanup_hosts(root):
    """Stop only launch-guarded hosts in this fixture before deleting its files."""
    host = root / "host"
    processes = {}
    errors = []
    for path in host.glob("*.json"):
        record = json.loads(path.read_text())
        meta = record["meta"]
        if Path("/proc/self/stat").exists():
            for pid in (record.get("host_pid"), record.get("pid")):
                if pid and (identity := running_identity(pid)) is not None:
                    processes[pid] = identity
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.settimeout(2)
                stream.connect(str(host / (record["name"] + ".sock")))
                stream.sendall(json.dumps({"op": "launch_guard_v1",
                    "expected_source": meta["source"],
                    "expected_launch_id": meta["launch_id"],
                    "expected_instance_id": meta["instance_id"],
                    "request": {"op": "kill", "force": True}}).encode() + b"\n")
        except FileNotFoundError:
            pass  # A host that already exited removes its socket.
        except OSError as error:
            errors.append(f"{record['name']}: {error}")
    deadline = time.monotonic() + 6

    def remaining():
        return [pid for pid, identity in processes.items() if running_identity(pid) == identity]

    while (list(host.glob("*.sock")) or remaining()) and time.monotonic() < deadline:
        time.sleep(.05)
    assert not list(host.glob("*.sock")), ("private test host sockets survived cleanup", errors)
    assert not remaining(), f"private test host/CLI processes survived cleanup: {remaining()}"


@contextmanager
def private_hosts(root):
    """Place inside TemporaryDirectory and outside browsers and isolated servers.

    A runner's graceful timeout must take the same cleanup path as a failed
    assertion. SIGKILL cannot run teardown; callers should allow a TERM grace.
    """
    previous = signal.getsignal(signal.SIGTERM)

    def terminate(signum, frame):
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGTERM, terminate)
    try:
        yield
    finally:
        try:
            cleanup_hosts(Path(root))
        finally:
            signal.signal(signal.SIGTERM, previous)
