#!/usr/bin/env python3
"""Offline dry-run regression test for deploy/deploy.py.

Pins the web-only build stage, SKIPPED / UNSUPPORTED / PLANNED outcomes,
and the JSON report without touching a real machine. Every fixture target
uses ssh=null and a temporary prefix under /tmp. PASS when the dry-run
plan, table rows and report match.
"""
# run_validation: skip
from __future__ import annotations

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEPLOY = ROOT / "deploy" / "deploy.py"
STAGE_ROOT = ROOT / "target" / "deploy"
CLI_TIMEOUT = 120
RESULTS = {"OK", "PLANNED", "SKIPPED", "UNSUPPORTED", "FAILED", "WARN",
           "ROLLED_BACK", "ROLLBACK_FAILED"}
BUILD = {
    "packages": ["sessiondock", "sessiondock-hub"],
    "ptyhost_package": "ptyhost",
    "cargo": "~/.cargo/bin/cargo",
}


def git_head() -> str:
    return subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True,
                          text=True, timeout=15, check=True).stdout.strip()


def tree_state(root: Path) -> dict[str, int]:
    """Relative paths -> size; directories map to -1 so new empty dirs show up."""
    state: dict[str, int] = {}
    if not root.is_dir():
        return state
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        rel_dir = os.path.relpath(dirpath, root)
        if rel_dir != ".":
            state[rel_dir.replace("\\", "/") + "/"] = -1
        for name in sorted(filenames):
            path = Path(dirpath) / name
            rel = os.path.relpath(path, root).replace("\\", "/")
            state[rel] = path.stat().st_size
    return state


def table_results(stdout: str) -> dict[str, str]:
    lines = stdout.splitlines()
    header = next((i for i, line in enumerate(lines)
                   if line.split()[:3] == ["target", "kind", "result"]), None)
    if header is None:
        return {}
    found: dict[str, str] = {}
    for line in lines[header + 1:]:
        parts = line.split()
        if len(parts) >= 3 and parts[2] in RESULTS:
            found[parts[0]] = parts[2]
    return found


def combined(proc: subprocess.CompletedProcess) -> str:
    return (proc.stdout or "") + (proc.stderr or "")


class DeployDryRunTest(unittest.TestCase):
    stage_dir: Path | None = None

    @classmethod
    def setUpClass(cls) -> None:
        # Nested deploy self-checks must never contend with the real fleet lock.
        cls.checkout = tempfile.TemporaryDirectory(prefix="sessiondock-deploy-checkout-")
        cls.source = Path(cls.checkout.name)
        shutil.copytree(ROOT / "deploy", cls.source / "deploy",
                        ignore=shutil.ignore_patterns("*.local.*", "__pycache__"))
        shutil.copytree(ROOT / "legacy-web", cls.source / "legacy-web",
                        ignore=shutil.ignore_patterns("node_modules"))
        for args in (["init", "-q"], ["add", "deploy", "legacy-web"],
                     ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                      "commit", "-qm", "Fixture"]):
            subprocess.run(["git", *args], cwd=cls.source, check=True, capture_output=True)

    def setUp(self) -> None:
        self.root = tempfile.mkdtemp(prefix="sessiondock-deploy-dry-")
        self.prefix = Path(self.root) / "prefix"
        for sub in ("bin", "web", "etc", "host"):
            (self.prefix / sub).mkdir(parents=True)
        (self.prefix / "bin" / "sessiondock").write_bytes(b"fake-sessiondock\n")
        (self.prefix / "web" / "index.html").write_text("<html></html>\n", encoding="utf-8")
        (self.prefix / "web" / "app.js").write_text("/* fixture */\n", encoding="utf-8")
        missing = Path(self.root) / "missing-prefix"
        local = {
            "name": "local", "kind": "linux-node", "ssh": None,
            "prefix": str(self.prefix),
            "health_url": "http://127.0.0.1:1/api/meta",
            "service": {"unit": "sessiondock-nonexistent-test.service", "scope": "user"},
            "binaries": ["sessiondock"],
        }
        off = dict(local, name="off", enabled=False)
        doc = {
            "build": BUILD,
            "targets": [
                local,
                {"name": "missing", "kind": "linux-node", "ssh": None,
                 "prefix": str(missing),
                 "health_url": "http://127.0.0.1:1/api/meta",
                 "service": {"unit": "sessiondock-nonexistent-test.service", "scope": "user"},
                 "binaries": ["sessiondock"]},
                {"name": "weird", "kind": "no-such-kind", "ssh": None,
                 "prefix": str(self.prefix),
                 "health_url": "http://127.0.0.1:1/api/meta",
                 "binaries": ["sessiondock"]},
                off,
            ],
        }
        self.targets = Path(self.root) / "targets.json"
        self.targets.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
        self.before = tree_state(self.prefix)

    def tearDown(self) -> None:
        shutil.rmtree(self.root, ignore_errors=True)

    @classmethod
    def tearDownClass(cls) -> None:
        cls._drop_stage()
        cls.checkout.cleanup()

    @classmethod
    def _drop_stage(cls) -> None:
        stage = cls.stage_dir
        cls.stage_dir = None
        if stage is None:
            return
        try:
            stage.resolve().relative_to((cls.source / "target" / "deploy").resolve())
        except ValueError:
            return
        shutil.rmtree(stage, ignore_errors=True)

    def cli(self, argv: list[str]) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, str(self.source / "deploy" / "deploy.py"), *argv],
            cwd=self.source, capture_output=True, text=True, timeout=CLI_TIMEOUT,
        )

    def remember_stage(self, stdout: str) -> Path | None:
        for line in stdout.splitlines():
            if line.startswith("stage "):
                path = Path(line.split(" ", 1)[1].strip())
                type(self).stage_dir = path
                return path
        return type(self).stage_dir

    def require_stage(self) -> Path:
        stage = type(self).stage_dir
        self.assertIsNotNone(stage, "build test must create a stage first")
        assert stage is not None
        self.assertTrue((stage / "artifacts.json").is_file(), f"missing artifacts.json in {stage}")
        return stage

    def latest_report(self, stage: Path) -> dict:
        reports = sorted(stage.glob("report-*.json"), key=lambda p: p.stat().st_mtime)
        self.assertTrue(reports, f"no report-*.json under {stage}")
        return json.loads(reports[-1].read_text(encoding="utf-8"))

    def assert_prefix_unchanged(self) -> None:
        self.assertEqual(tree_state(self.prefix), self.before)

    def test_a_build_web_only(self) -> None:
        proc = self.cli(["build", "--web-only", "--allow-dirty", "--web-from-head",
                         "--targets-file", str(self.targets)])
        stage = self.remember_stage(proc.stdout)
        self.assertEqual(proc.returncode, 0, combined(proc))
        self.assertIsNotNone(stage, proc.stdout)
        assert stage is not None
        art = json.loads((stage / "artifacts.json").read_text(encoding="utf-8"))
        self.assertTrue(art.get("web_only"))
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=self.source,
                              check=True, capture_output=True, text=True).stdout.strip()
        self.assertEqual(art.get("commit"), head)
        self.assertTrue((stage / "web" / "index.html").is_file())
        self.assertEqual(art.get("binaries"), {})
        bin_files = [p for p in (stage / "bin").rglob("*") if p.is_file()] if (stage / "bin").is_dir() else []
        self.assertEqual(bin_files, [])

    def test_b_push_all_dry_run(self) -> None:
        stage = self.require_stage()
        proc = self.cli(["push", "--dry-run", "--all", "--targets-file", str(self.targets),
                         "--stage", str(stage), "--web-only"])
        self.assertEqual(proc.returncode, 1, combined(proc))
        rows = table_results(proc.stdout)
        self.assertEqual(rows.get("local"), "PLANNED", proc.stdout)
        self.assertEqual(rows.get("missing"), "SKIPPED", proc.stdout)
        self.assertEqual(rows.get("weird"), "UNSUPPORTED", proc.stdout)
        self.assertEqual(rows.get("off"), "SKIPPED", proc.stdout)
        report = self.latest_report(stage)
        by_name = {row["target"]: row for row in report["rows"]}
        self.assertEqual(set(by_name), {"local", "missing", "weird", "off"})
        self.assertEqual(by_name["local"]["result"], "PLANNED")
        self.assertEqual(by_name["missing"]["result"], "SKIPPED")
        self.assertIn("prefix layout missing", by_name["missing"].get("detail") or "")
        self.assertEqual(by_name["weird"]["result"], "UNSUPPORTED")
        self.assertEqual(by_name["off"]["result"], "SKIPPED")
        plan = "\n".join(by_name["local"].get("plan") or [])
        self.assertIn("web.staging", plan)
        self.assertIn("systemctl --user restart sessiondock-nonexistent-test.service", plan)
        self.assertIn("deployed-commit", plan)
        self.assert_prefix_unchanged()

    def test_c_push_local_dry_run(self) -> None:
        stage = self.require_stage()
        proc = self.cli(["push", "--dry-run", "--targets", "local", "--targets-file",
                         str(self.targets), "--stage", str(stage), "--web-only"])
        self.assertEqual(proc.returncode, 0, combined(proc))
        rows = table_results(proc.stdout)
        self.assertEqual(rows, {"local": "PLANNED"}, proc.stdout)
        self.assert_prefix_unchanged()

    def test_c_target_hostname_mismatch_never_stages(self) -> None:
        import socket
        doc = json.loads(self.targets.read_text())
        doc['targets'][0]['extra'] = {'expected_hostname': socket.gethostname() + '-wrong'}
        self.targets.write_text(json.dumps(doc))
        proc = self.cli(['push', '--targets', 'local', '--targets-file', str(self.targets),
                         '--stage', str(self.require_stage()), '--web-only'])
        self.assertEqual(proc.returncode, 1, combined(proc))
        report = self.latest_report(self.require_stage())
        self.assertIn('target identity mismatch', report['rows'][0]['detail'])
        self.assert_prefix_unchanged()

    def test_d_unknown_target(self) -> None:
        proc = self.cli(["push", "--dry-run", "--targets", "nope", "--targets-file",
                         str(self.targets), "--web-only"])
        self.assertNotEqual(proc.returncode, 0, combined(proc))
        blob = combined(proc)
        self.assertIn("nope", blob)
        self.assertIn("unknown", blob.lower())

    def test_e_rollback_bad_backup(self) -> None:
        proc = self.cli(["rollback", "--targets", "local", "--targets-file", str(self.targets),
                         "--backup", "/tmp/not-a-backup-dir"])
        self.assertNotEqual(proc.returncode, 0, combined(proc))
        rows = table_results(proc.stdout)
        self.assertEqual(rows.get("local"), "FAILED", proc.stdout)
        self.assertIn("backup-deploy-", combined(proc))
        self.assert_prefix_unchanged()


class SourceArchiveTests(unittest.TestCase):
    def test_dirty_source_matches_tracked_files_and_preserves_index(self):
        spec = importlib.util.spec_from_file_location("sessiondock_deploy_snapshot", DEPLOY)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory(prefix="sessiondock-source-fixture-") as temporary:
            root = Path(temporary)
            def git(*args):
                return subprocess.run(["git", *args], cwd=root, check=True,
                                      capture_output=True, text=True).stdout.strip()
            git("init", "-q")
            git("config", "user.name", "Synthetic Test")
            git("config", "user.email", "test@example.invalid")
            (root / "source.rs").write_text("baseline")
            (root / "deleted.rs").write_text("deleted baseline")
            git("add", ".")
            git("commit", "-qm", "fixture")
            head = git("rev-parse", "HEAD")
            (root / "source.rs").write_text("staged version")
            git("add", "source.rs")
            (root / "new.rs").write_text("new staged source")
            git("add", "new.rs")
            staged_diff = git("diff", "--cached", "--numstat")
            index = (root / ".git/index").read_bytes()
            (root / "new.rs").write_text("new working source")
            (root / "source.rs").write_text("working version")
            (root / "deleted.rs").unlink()
            (root / "runtime.env").write_text("untracked fixture configuration")
            with patch.object(module, "ROOT", root):
                tree = module.archive_source(root / "dirty.tar", True)
                self.assertEqual(module.source_tree(True), tree)
                module.archive_source(root / "head.tar", False)
            with tarfile.open(root / "dirty.tar") as archive:
                self.assertEqual(archive.extractfile("source.rs").read(), b"working version")
                self.assertNotIn("deleted.rs", archive.getnames())
                self.assertNotIn("runtime.env", archive.getnames())
                self.assertEqual(archive.extractfile("new.rs").read(), b"new working source")
            with tarfile.open(root / "head.tar") as archive:
                self.assertEqual(archive.extractfile("source.rs").read(), b"baseline")
                self.assertIn("deleted.rs", archive.getnames())
            self.assertEqual((root / ".git/index").read_bytes(), index)
            self.assertEqual(git("rev-parse", "HEAD"), head)
            self.assertEqual(git("diff", "--cached", "--numstat"), staged_diff)


if __name__ == "__main__":
    unittest.main()
