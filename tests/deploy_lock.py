#!/usr/bin/env python3
"""Deployment mutex concurrency, crash recovery, worktrees and command coverage.

Only temporary repositories and fake command bodies; no fleet access or builds.
"""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "deploy"))
from deployment_lock import DeploymentLock, repository_lock

HOLDER = """
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from deployment_lock import DeploymentLock
with DeploymentLock(Path(sys.argv[2]), 'holder'):
    print('READY', flush=True)
    sys.stdin.readline()
"""
WAITER = """
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from deployment_lock import DeploymentLock
with DeploymentLock(Path(sys.argv[2]), 'waiter', 10):
    print('ENTERED ' + Path(sys.argv[3]).read_text(), flush=True)
"""


class LockTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="sessiondock-lock-test-")
        self.root = Path(self.tmp.name)
        self.path = self.root / "deploy.lock"
        self.children = []

    def tearDown(self):
        for proc in self.children:
            if proc.poll() is None:
                proc.kill()
            proc.communicate(timeout=10)
        self.tmp.cleanup()

    def spawn(self, code, *args):
        proc = subprocess.Popen([sys.executable, "-u", "-c", code, str(ROOT / "deploy"),
                                 str(self.path), *map(str, args)], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.children.append(proc)
        return proc

    def holder(self):
        proc = self.spawn(HOLDER)
        self.assertIn("acquired", proc.stdout.readline())
        self.assertEqual(proc.stdout.readline().strip(), "READY")
        return proc

    def test_waiter_reads_latest_state_after_release(self):
        holder = self.holder()
        state = self.root / "workspace"
        state.write_text("old")
        waiter = self.spawn(WAITER, state)
        notice = waiter.stdout.readline()
        self.assertIn(f"waiting for PID {holder.pid}", notice)
        self.assertIsNone(waiter.poll())
        state.write_text("latest")
        holder.communicate("release\n", timeout=10)
        output, error = waiter.communicate(timeout=10)
        self.assertEqual(waiter.returncode, 0, error)
        self.assertIn("ENTERED latest", output)

    def test_timeout_and_crash_release_preserve_inode(self):
        holder = self.holder()
        inode = self.path.stat().st_ino
        with self.assertRaises(TimeoutError):
            with DeploymentLock(self.path, "contender", 0):
                self.fail("concurrent entry")
        holder.kill()
        holder.communicate(timeout=10)
        with DeploymentLock(self.path, "recovery", 1):
            owner = json.loads(self.path.read_bytes()[1:])
            self.assertEqual(owner["command"], "recovery")
        self.assertEqual(self.path.stat().st_ino, inode)

    def test_worktrees_share_lock(self):
        def git(*args):
            return subprocess.run(["git", *args], cwd=self.root, capture_output=True,
                                  text=True, check=True, timeout=15).stdout.strip()
        git("init", "-q")
        git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
            "commit", "--allow-empty", "-qm", "Fixture")
        worktree = self.root / "linked"
        git("worktree", "add", "--detach", str(worktree))
        self.assertEqual(repository_lock(self.root), repository_lock(worktree))

    def test_all_cli_commands_lock_before_reading_targets(self):
        # If admitted these commands would read the nonexistent targets file.
        # Holding the mutex must instead make EVERY command time out first.
        import deploy
        import contextlib
        import io
        from unittest.mock import patch
        with DeploymentLock(self.path, "holder"):
            for command in ("build", "deploy", "push", "rollback"):
                with self.subTest(command=command), \
                        patch.object(deploy, "repository_lock", return_value=self.path):
                    argv = [command, "--lock-timeout", "0", "--targets-file", str(self.root / "missing")]
                    if command == "rollback":
                        argv += ["--targets", "fixture"]
                    error = io.StringIO()
                    with contextlib.redirect_stderr(error), self.assertRaises(SystemExit) as caught:
                        deploy.main(argv)
                    self.assertEqual(caught.exception.code, 2)
                    self.assertIn("deployment lock timeout", error.getvalue())
                    self.assertFalse((self.root / "target").exists())


if __name__ == "__main__":
    unittest.main()
