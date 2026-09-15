"""OS-managed deployment mutex shared by a repository and all its worktrees."""
from __future__ import annotations

import errno
import json
import os
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path


def repository_lock(root: Path) -> Path:
    common = subprocess.run(["git", "rev-parse", "--git-common-dir"], cwd=root,
                            capture_output=True, text=True, check=True, timeout=30).stdout.strip()
    return (root / common).resolve() / "sessiondock-deploy.lock"


class DeploymentLock:
    """Never unlink the lock file: crash recovery is the kernel's responsibility."""

    def __init__(self, path: Path, command: str, timeout: float | None = None):
        self.path, self.command, self.timeout = path, command, timeout
        self.fd: int | None = None

    def _try_lock(self) -> bool:
        try:
            if os.name == "nt":
                import msvcrt
                os.lseek(self.fd, 0, os.SEEK_SET)
                msvcrt.locking(self.fd, msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        except OSError as exc:
            if exc.errno not in (errno.EACCES, errno.EAGAIN, errno.EDEADLK):
                raise
            return False

    def owner(self) -> str:
        try:
            # Byte zero is reserved for the Windows mutex; metadata is diagnostic.
            with self.path.open("rb") as stream:
                stream.seek(1)
                doc = json.load(stream)
            return f"PID {doc['pid']} ({doc['command']}, since {doc['started_at']})"
        except (OSError, ValueError, KeyError):
            return "owner metadata unavailable"

    def __enter__(self):
        self.fd = os.open(self.path, os.O_RDWR | os.O_CREAT, 0o600)
        started, next_notice = time.monotonic(), 0.0
        try:
            if os.fstat(self.fd).st_size == 0:
                os.write(self.fd, b" ")
            while not self._try_lock():
                now = time.monotonic()
                if now >= next_notice:
                    print(f"deployment lock: waiting for {self.owner()}", flush=True)
                    next_notice = now + 30
                if self.timeout is not None and now - started >= self.timeout:
                    raise TimeoutError(f"deployment lock timeout: {self.owner()}")
                time.sleep(min(0.2, max(0, self.timeout - (now - started)))
                           if self.timeout is not None else 0.2)
            owner = {"pid": os.getpid(), "command": self.command,
                     "started_at": datetime.now(timezone.utc).isoformat()}
            os.lseek(self.fd, 1, os.SEEK_SET)
            os.write(self.fd, json.dumps(owner).encode())
            os.ftruncate(self.fd, os.lseek(self.fd, 0, os.SEEK_CUR))
            print(f"deployment lock: acquired by PID {os.getpid()} ({self.command})", flush=True)
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *_):
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None
