"""Own test hosts for the whole fixture, including across server restarts."""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import sys
import tempfile
import threading
import time


def running_identity(pid):
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        return fields[19] if fields[0] != "Z" else None
    except (FileNotFoundError, ProcessLookupError):
        return None


def launching_hosts(host):
    """Processes started with exactly `--dir <host>` (Linux only).

    A launch the stopped server left in flight may not have published its
    record yet. These are only waited for; they are stopped through the
    record and socket they publish in this directory, never by PID.
    """
    wanted = os.path.realpath(host)
    found = {}
    for entry in Path("/proc").glob("[0-9]*"):
        try:
            argv = (entry / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        for flag, value in zip(argv, argv[1:]):
            if flag == b"--dir" and os.path.realpath(os.fsdecode(value)) == wanted:
                if (identity := running_identity(entry.name)) is not None:
                    found[int(entry.name)] = identity
                break
    return found


def stop_record(host, path, processes, stopped, errors):
    try:
        record = json.loads(path.read_text())
    except FileNotFoundError:
        return  # The host exited and removed its record.
    except (OSError, ValueError) as error:
        errors[path.name] = f"unreadable record: {error}"
        return
    if Path("/proc/self/stat").exists():
        for pid in (record.get("host_pid"), record.get("pid")):
            if pid and (identity := running_identity(pid)) is not None:
                processes[pid] = identity
    key = (path.name, record.get("host_pid"))
    if key in stopped:
        return
    meta = record.get("meta") or {}
    kill = {"op": "kill", "force": True}
    if all(meta.get(field) for field in ("source", "launch_id", "instance_id")):
        kill = {"op": "launch_guard_v1", "expected_source": meta["source"],
                "expected_launch_id": meta["launch_id"],
                "expected_instance_id": meta["instance_id"], "request": kill}
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
            stream.settimeout(2)
            stream.connect(str(host / (record["name"] + ".sock")))
            stream.sendall(json.dumps(kill).encode() + b"\n")
        stopped.add(key)
        errors.pop(path.name, None)
    except (FileNotFoundError, ConnectionRefusedError):
        # A host that exited removes its socket; one killed by its own test
        # owner can leave a socket file nobody listens on.
        stopped.add(key)
    except (OSError, KeyError) as error:
        errors[path.name] = str(error)


def listening(path):
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
            stream.settimeout(1)
            stream.connect(str(path))
        return True
    except (FileNotFoundError, ConnectionRefusedError):
        return False
    except OSError:
        return True  # Anything else is not proof the host is gone.


def live_sockets(directories):
    return [path for host in directories for path in host.glob("*.sock") if listening(path)]


def cleanup_hosts(root, hosts=("host",)):
    """Stop every host in this fixture's host directories before deleting them.

    Each record is stopped through its own socket (launch-guarded when the
    record carries a guard identity). Records are rescanned until the deadline
    so hosts still starting when the server stopped are reaped as soon as they
    publish. Every record is attempted even if another one fails.
    """
    directories = [Path(root) / name for name in hosts]
    processes, stopped, errors = {}, set(), {}
    deadline = time.monotonic() + 10

    def remaining():
        return [pid for pid, identity in processes.items() if running_identity(pid) == identity]

    def launching():
        if not Path("/proc/self/stat").exists():
            return {}
        found = {}
        for host in directories:
            found.update(launching_hosts(host))
        return found

    while True:
        for host in directories:
            for path in host.glob("*.json"):
                stop_record(host, path, processes, stopped, errors)
        sockets = live_sockets(directories)
        starting = launching()
        processes.update(starting)
        if not (sockets or remaining() or starting) or time.monotonic() >= deadline:
            break
        time.sleep(.05)
    sockets = live_sockets(directories)
    assert not sockets, ("private test host sockets survived cleanup", sockets, errors)
    assert not remaining(), ("private test host/CLI processes survived cleanup", remaining(), errors)


@contextmanager
def private_hosts(root, hosts=("host",)):
    """Place inside TemporaryDirectory and outside browsers and isolated servers.

    `hosts` names every host directory under `root` that a server or test may
    launch into. A runner's graceful timeout (SIGTERM), a closed terminal
    (SIGHUP) and Ctrl-C (SIGINT, unless the process started with it ignored)
    stop the hosts inside the signal handler first, then unwind as SystemExit
    or KeyboardInterrupt. A signal that lands inside a Playwright wait can
    leave `browser.close()` blocked forever during unwinding; the hosts are
    already gone by then, and a watchdog removes the private temporary
    fixture directory and ends the process if the unwinding has not reached
    this context within 15 seconds. SIGKILL cannot run
    teardown; callers should allow a TERM grace.
    """
    handled = [signal.SIGTERM, signal.SIGHUP]
    if signal.getsignal(signal.SIGINT) is signal.default_int_handler:
        handled.append(signal.SIGINT)
    previous = {signum: signal.getsignal(signum) for signum in handled}
    watchdogs = []

    def abandon(code):
        # Unwinding is stuck after the hosts were stopped. TemporaryDirectory
        # cannot run either, so remove the fixture directory when it is one.
        directory = Path(root).resolve()
        if directory.parent == Path(tempfile.gettempdir()).resolve() and directory.name.startswith("sessiondock-"):
            shutil.rmtree(directory, ignore_errors=True)
        os._exit(code)

    def terminate(signum, frame):
        code = 128 + signum
        watchdog = threading.Timer(15, abandon, (code,))
        watchdog.daemon = True
        watchdog.start()
        watchdogs.append(watchdog)
        try:
            cleanup_hosts(Path(root), hosts)
        except BaseException as error:  # Still unwind; the finally retries.
            print("private host cleanup on signal failed:", error, file=sys.stderr, flush=True)
        if signum == signal.SIGINT:
            raise KeyboardInterrupt
        raise SystemExit(code)

    for signum in handled:
        signal.signal(signum, terminate)
    try:
        yield
    finally:
        try:
            cleanup_hosts(Path(root), hosts)
        finally:
            for watchdog in watchdogs:
                watchdog.cancel()
            for signum, handler in previous.items():
                signal.signal(signum, handler)
