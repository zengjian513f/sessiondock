"""Explicit and signal-safe teardown for private test SDK hosts."""
# run_validation: skip
from __future__ import annotations

import atexit
from collections import defaultdict
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

_LOCK = threading.RLock()
_REGISTERED_ROOTS: set[Path] = set()
_LIVE_SERVERS: list[tuple[Path, object]] = []
_WATCHDOGS: list[threading.Timer] = []
_HOOKS_INSTALLED = False
_IN_SIGNAL = False
_PREVIOUS_SIGNALS: dict[int, object] = {}


def _proc_info(pid: int, uid: int):
    try:
        if os.stat(f'/proc/{pid}').st_uid != uid:
            return None
        raw = (Path('/proc') / str(pid) / 'stat').read_text()
    except OSError:
        return None
    end = raw.rfind(')')
    comm = raw[raw.find('(') + 1:end]
    fields = raw[end + 2:].split()
    return {
        'pid': pid,
        'comm': comm,
        'state': fields[0],
        'ppid': int(fields[1]),
        'sid': int(fields[3]),
        'starttime': fields[19],
    }


def _scoped_sdkhosts_linux(scope: Path) -> dict[int, tuple[str, Path]]:
    if not Path('/proc/self/stat').exists():
        return {}
    uid = os.getuid()
    resolved_scope = scope.resolve()
    found: dict[int, tuple[str, Path]] = {}
    for entry in Path('/proc').glob('[0-9]*'):
        pid = int(entry.name)
        info = _proc_info(pid, uid)
        if not info or info['state'] == 'Z' or info['comm'] != 'sdkhost':
            continue
        try:
            argv = (entry / 'cmdline').read_bytes().split(b'\0')
        except OSError:
            continue
        for flag, value in zip(argv, argv[1:]):
            if flag == b'--dir' and value:
                host_path = Path(os.fsdecode(value)).resolve()
                if host_path == resolved_scope or host_path.is_relative_to(resolved_scope):
                    found[pid] = (info['starttime'], host_path)
                break
    return found


def _kill_tree_linux(root_pid: int, expected_starttime: str) -> None:
    uid = os.getuid()
    procs: dict[int, dict] = {}
    for entry in Path('/proc').glob('[0-9]*'):
        info = _proc_info(int(entry.name), uid)
        if info:
            procs[info['pid']] = info
    root_info = procs.get(root_pid)
    if not root_info or root_info['starttime'] != expected_starttime:
        return
    children = defaultdict(list)
    by_sid = defaultdict(list)
    for pid, info in procs.items():
        children[info['ppid']].append(pid)
        by_sid[info['sid']].append(pid)
    seen: set[int] = set()
    todo = list(children[root_pid])
    while todo:
        cur = todo.pop()
        if cur in seen:
            continue
        seen.add(cur)
        todo.extend(children[cur])
        if procs[cur]['sid'] == cur:
            for member in by_sid[cur]:
                if member not in seen and member != root_pid:
                    todo.append(member)
    for target in [*seen, root_pid]:
        expected = procs[target]['starttime']
        current = _proc_info(target, uid)
        if current and current['starttime'] == expected and current['state'] != 'Z':
            try:
                os.kill(target, signal.SIGKILL)
            except ProcessLookupError:
                pass


def _candidate_sdkhost_dirs(root: Path, host_dir: Path | None) -> list[Path]:
    if host_dir is not None:
        resolved = Path(host_dir).resolve()
        assert resolved.is_relative_to(root)
        return [resolved] if resolved.is_dir() else []
    if not root.is_dir():
        return []
    found: list[Path] = []
    seen: set[Path] = set()
    candidates = [root] if root.name == 'sdkhost' else []
    default = root / 'state/sdkhost'
    if default.is_dir():
        candidates.append(default)
    try:
        candidates.extend(path for path in root.rglob('sdkhost') if path.is_dir())
    except OSError:
        pass
    for candidate in candidates:
        resolved = candidate.resolve()
        if resolved not in seen and (resolved == root or resolved.is_relative_to(root)):
            seen.add(resolved)
            found.append(resolved)
    return found


def stop_private_sdkhosts(root, *, host_dir=None, best_effort=False):
    root = Path(root).resolve()
    assert not str(root).startswith('/srv/sessiondock'), root
    scope = Path(host_dir).resolve() if host_dir is not None else root
    if host_dir is not None:
        assert scope.is_relative_to(root)
    failures = []

    def alive(pid):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        try:
            return (Path('/proc') / str(pid) / 'stat').read_text().rsplit(')', 1)[1].split()[0] != 'Z'
        except (FileNotFoundError, ProcessLookupError):
            return False

    def wait_exited(pid):
        deadline = time.monotonic() + 5
        while alive(pid) and time.monotonic() < deadline:
            time.sleep(.02)
        if alive(pid):
            raise AssertionError(('private SDK fixture process survived teardown', pid))

    def rpc(path, request):
        assert path.resolve().is_relative_to(root)
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(3)
            if Path('/proc/self/fd').is_dir():
                directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
                try:
                    client.connect(f'/proc/self/fd/{directory}/control.sock')
                finally:
                    os.close(directory)
            else:
                client.connect(str(path))
            client.sendall(json.dumps(request).encode() + b'\n')
            with client.makefile('rb') as response:
                value = json.loads(response.readline())
        if isinstance(request, dict) and 'Signal' in request and value.get('error') == 'child already exited':
            status = json.loads((path.parent / 'status.json').read_text())
            assert status['exit'] is not None, status
            return None
        assert not value.get('error'), value
        return value['value']

    # Allow in-flight sdkhost launches inside scope a brief moment to bind control.sock.
    wait_bind_deadline = time.monotonic() + 1.0
    while time.monotonic() < wait_bind_deadline:
        starting = _scoped_sdkhosts_linux(scope)
        if not starting or all((hdir / 'control.sock').exists() for _, hdir in starting.values()):
            break
        time.sleep(.02)

    sockets = []
    seen_sockets = set()
    for directory in _candidate_sdkhost_dirs(root, host_dir):
        for path in directory.glob('*/control.sock'):
            resolved = path.resolve()
            if resolved not in seen_sockets:
                seen_sockets.add(resolved)
                sockets.append(path)
    for _, hdir in _scoped_sdkhosts_linux(scope).values():
        sock = hdir / 'control.sock'
        if sock.exists() and sock.resolve() not in seen_sockets:
            seen_sockets.add(sock.resolve())
            sockets.append(sock)

    for path in sockets:
        try:
            host_pid = int((path.parent / 'host.pid').read_text())
            try:
                status = rpc(path, 'Status')
            except (ConnectionResetError, json.JSONDecodeError):
                # An explicit UI stop may finish between connect and the ACK.
                # Only accept the durable exit record, then verify both PIDs.
                status = json.loads((path.parent / 'status.json').read_text())
                assert status['exit'] is not None, status
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

    for pid, (starttime, _) in _scoped_sdkhosts_linux(scope).items():
        _kill_tree_linux(pid, starttime)
        try:
            wait_exited(pid)
        except Exception as error:
            failures.append(str(error))

    if not best_effort:
        assert not failures, failures


def _terminate_servers(scope: Path | None = None) -> None:
    with _LOCK:
        entries = list(_LIVE_SERVERS)
        _LIVE_SERVERS[:] = [(r, p) for r, p in entries if p.poll() is None]
    for root, proc in entries:
        if proc.poll() is not None:
            continue
        if scope is not None and not (root == scope or root.is_relative_to(scope) or scope.is_relative_to(root)):
            continue
        try:
            proc.terminate()
            proc.wait(timeout=3)
        except Exception:
            try:
                proc.kill()
                proc.wait(timeout=2)
            except Exception:
                pass


def cleanup_registered_sdkhosts(*, scope: Path | None = None, best_effort: bool = True) -> None:
    resolved_scope = scope.resolve() if scope is not None else None
    _terminate_servers(resolved_scope)
    with _LOCK:
        if resolved_scope is None:
            targets = sorted(_REGISTERED_ROOTS)
        else:
            targets = sorted(
                r for r in _REGISTERED_ROOTS
                if r == resolved_scope or r.is_relative_to(resolved_scope)
            )
            if not targets and any(resolved_scope.is_relative_to(r) for r in _REGISTERED_ROOTS):
                targets = [resolved_scope]
    first_error = None
    for target in targets:
        try:
            stop_private_sdkhosts(target, best_effort=best_effort)
        except BaseException as error:
            if first_error is None:
                first_error = error
        finally:
            if resolved_scope is None or target == resolved_scope or target.is_relative_to(resolved_scope):
                with _LOCK:
                    _REGISTERED_ROOTS.discard(target)
    with _LOCK:
        if not _REGISTERED_ROOTS:
            for timer in _WATCHDOGS:
                timer.cancel()
            _WATCHDOGS.clear()
            global _IN_SIGNAL
            _IN_SIGNAL = False
    if first_error is not None and not best_effort:
        raise first_error


def _abandon(code: int) -> None:
    temp_root = Path(tempfile.gettempdir()).resolve()
    with _LOCK:
        roots = sorted(_REGISTERED_ROOTS)
    if not roots:
        return
    for root in roots:
        current = root
        while current.parent != current and current.parent != temp_root:
            current = current.parent
        if current.parent == temp_root and current.name.startswith(('sessiondock-', 'sd-', 'tmp')):
            shutil.rmtree(current, ignore_errors=True)
    os._exit(code)


def _on_signal(signum, frame):
    global _IN_SIGNAL
    code = 128 + signum
    if not _IN_SIGNAL:
        _IN_SIGNAL = True
        with _LOCK:
            has_work = bool(_REGISTERED_ROOTS or _LIVE_SERVERS)
        if has_work:
            watchdog = threading.Timer(15, _abandon, (code,))
            watchdog.daemon = True
            watchdog.start()
            with _LOCK:
                _WATCHDOGS.append(watchdog)
        try:
            cleanup_registered_sdkhosts(best_effort=True)
        except BaseException as error:
            print('private sdkhost cleanup on signal failed:', error, file=sys.stderr, flush=True)
    prev = _PREVIOUS_SIGNALS.get(signum)
    if callable(prev) and prev is not signal.default_int_handler and prev is not _on_signal:
        prev(signum, frame)
    if signum == signal.SIGINT:
        raise KeyboardInterrupt
    raise SystemExit(code)


def ensure_signal_handlers() -> None:
    global _HOOKS_INSTALLED
    with _LOCK:
        if _HOOKS_INSTALLED:
            return
        _HOOKS_INSTALLED = True
        orig_cleanup = tempfile.TemporaryDirectory.cleanup

        def _guarded_cleanup(self):
            try:
                td = Path(self.name).resolve()
            except Exception:
                td = None
            if td is not None and not str(td).startswith('/srv/sessiondock'):
                cleanup_registered_sdkhosts(scope=td, best_effort=(sys.exc_info()[0] is not None))
            return orig_cleanup(self)

        tempfile.TemporaryDirectory.cleanup = _guarded_cleanup
        atexit.register(lambda: cleanup_registered_sdkhosts(best_effort=True))
        if threading.current_thread() is threading.main_thread():
            signals = [signal.SIGTERM, signal.SIGHUP]
            if signal.getsignal(signal.SIGINT) not in (signal.SIG_IGN, signal.SIG_DFL):
                signals.append(signal.SIGINT)
            for sig in signals:
                _PREVIOUS_SIGNALS[sig] = signal.getsignal(sig)
                signal.signal(sig, _on_signal)


def register_private_sdkhosts(root, server_process=None) -> Path:
    resolved = Path(root).resolve()
    assert not str(resolved).startswith('/srv/sessiondock'), resolved
    ensure_signal_handlers()
    with _LOCK:
        _REGISTERED_ROOTS.add(resolved)
        if server_process is not None:
            _LIVE_SERVERS[:] = [(r, p) for r, p in _LIVE_SERVERS if p.poll() is None]
            _LIVE_SERVERS.append((resolved, server_process))
    return resolved


ensure_signal_handlers()
