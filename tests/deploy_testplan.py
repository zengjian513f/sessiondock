#!/usr/bin/env python3
"""Offline pins for the deploy test gate (deploy/testplan.py and deploy.py --test).

Representative changed paths map to suite-name patterns against a stubbed
`run_validation.py --list`; the alias table is checked against the real `--list` so it
cannot rot silently; the base commit resolves from an explicit ref, the OLDEST
`etc/deployed-commit` marker, then origin/main / HEAD~1, on this checkout's own history.
`build --test none` skips, `build --test affected` prints the plan and hands `--only`,
`--binary`, `--log-dir` and `--json` to a stubbed runner, records `test_*` in
artifacts.json and exits 1 with the failing suite names and log paths; an unmatched path
selects the full sweep; `push --dry-run` echoes the recorded mode and `deploy --dry-run`
takes the base from the fixture target's marker. Nothing is executed: the streaming
runner is replaced, targets are ssh=null with a temporary prefix, no cargo.
"""
from __future__ import annotations

import contextlib
import io
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "deploy"))
import deploy  # noqa: E402
import testplan as tp  # noqa: E402

STAGE_ROOT = ROOT / "target" / "deploy"
STUB = """cargo_test rust 1500s cargo test
cargo_fmt rust 120s cargo fmt
cargo_clippy rust 900s cargo clippy
cargo_check_windows rust 900s cargo check
cargo_build rust 900s cargo build
node_contracts node 120s node --test
audit_browser python 900s python3 tests/audit_browser.py
brand_names_check python 900s python3 tests/brand_names_check.py
bug_report_http_suite python 900s python3 tests/bug_report_http_suite.py
claude_prompt_suite python 900s python3 tests/claude_prompt_suite.py
deploy_native_handlers python 900s python3 tests/deploy_native_handlers.py
deploy_testplan python 900s python3 tests/deploy_testplan.py
files_browser python 900s python3 tests/files_browser.py
files_read_suite python 900s python3 tests/files_read_suite.py
history_browser python 900s python3 tests/history_browser.py
host_identity python 900s python3 tests/host_identity.py
hub_browser python 900s python3 tests/hub_browser.py
hub_http_suite python 900s python3 tests/hub_http_suite.py
legacy_browser python 900s python3 tests/legacy_browser.py
lifecycle_browser python 900s python3 tests/lifecycle_browser.py
lifecycle_browser_native_binding python 900s python3 tests/lifecycle_browser.py --native-binding
lifecycle_http_suite python 900s python3 tests/lifecycle_http_suite.py
media_browser python 900s python3 tests/media_browser.py
media_get_suite python 900s python3 tests/media_get_suite.py
native_spans python 900s python3 tests/native_spans.py --browser
outbox_suite python 900s python3 tests/outbox_suite.py
search_browser python 900s python3 tests/search_browser.py
search_suite python 900s python3 tests/search_suite.py
send_browser python 900s python3 tests/send_browser.py
send_http_suite python 900s python3 tests/send_http_suite.py
sessions_list_suite python 900s python3 tests/sessions_list_suite.py
terminal_browser python 900s python3 tests/terminal_browser.py
term_send_http_suite python 900s python3 tests/term_send_http_suite.py
trash_http_suite python 900s python3 tests/trash_http_suite.py
36 suites
"""
NAMES = [ln.split()[0] for ln in STUB.splitlines() if not ln.endswith(" suites")]
CARGO = {n for n in NAMES if n.startswith("cargo_")}
BROWSERS = {n for n in NAMES if "_browser" in n}


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, timeout=30,
                          check=True).stdout.strip()


def suites_of(path: str) -> tuple[set[str], set[str], bool]:
    plan = tp.plan_for("affected", [path], NAMES)
    return set(plan["suites"]), set(plan["scripts"]), plan["full"]


def opt(argv: list[str], name: str) -> str | None:
    """Value after `--<name>` in argv (spelled indirectly: run_validation appends `--binary`
    to any suite whose source quotes that flag, and this suite takes no such argument)."""
    flag = "--" + name
    return argv[argv.index(flag) + 1] if flag in argv else None


class MappingTest(unittest.TestCase):
    def test_ptyhost_crates(self) -> None:
        for path in ("crates/ptyhost/src/main.rs", "crates/ptyhost-client/src/lib.rs"):
            s, sc, full = suites_of(path)
            self.assertFalse(full, path)
            self.assertTrue(CARGO <= s, (path, s))
            self.assertTrue({"terminal_browser", "term_send_http_suite", "lifecycle_browser", "lifecycle_http_suite",
                             "host_identity", "native_spans"} <= s, (path, s))
            self.assertNotIn("search_suite", s)
            self.assertNotIn("legacy_browser", s)

    def test_sessiondock_modules(self) -> None:
        s, _, full = suites_of("crates/sessiondock/src/search/mod.rs")
        self.assertFalse(full)
        self.assertEqual(s - CARGO, {"search_browser", "search_suite"})
        s, _, _ = suites_of("crates/sessiondock/src/media.rs")
        self.assertTrue({"media_browser", "media_get_suite", "native_spans"} <= s and CARGO <= s, s)
        s, _, _ = suites_of("crates/sessiondock/src/lifecycle/service.rs")
        self.assertTrue({"lifecycle_browser", "lifecycle_http_suite", "send_browser", "send_http_suite", "outbox_suite"} <= s, s)
        s, _, _ = suites_of("crates/sessiondock/src/delivery/store.rs")
        self.assertTrue({"send_browser", "send_http_suite", "outbox_suite"} <= s and "search_suite" not in s, s)
        s, _, _ = suites_of("crates/sessiondock/src/hub/mod.rs")
        self.assertEqual(s - CARGO, {"hub_browser", "hub_http_suite"})
        s, _, _ = suites_of("crates/sessiondock/src/files/read.rs")
        self.assertEqual(s - CARGO, {"files_browser", "files_read_suite"})
        s, _, _ = suites_of("crates/sessiondock/src/sessions/history.rs")
        self.assertTrue({"history_browser", "sessions_list_suite", "native_spans"} <= s, s)
        s, _, _ = suites_of("crates/sessiondock/src/trash/plan.rs")
        self.assertEqual(s - CARGO, {"trash_http_suite"})
        s, _, _ = suites_of("crates/sessiondock/src/bridge/claude.rs")
        self.assertEqual(s - CARGO, {"claude_prompt_suite"})
        for broad in ("crates/sessiondock/src/api/read.rs", "crates/sessiondock/src/main.rs", "crates/sessiondock/src/lib.rs",
                      "crates/sessiondock/src/config.rs", "crates/sessiondock/src/security.rs",
                      "crates/sessiondock/src/observe.rs", "crates/sessiondock/src/newmodule/x.rs"):
            self.assertTrue(suites_of(broad)[2], f"{broad} must select the full sweep")
        s, _, full = suites_of("crates/sessiondock/tests/hub_http.rs")
        self.assertFalse(full)
        self.assertEqual(s, CARGO)
        self.assertTrue(suites_of("crates/sessiondock/tests/fixtures/claude/x.jsonl")[2])

    def test_frontend_deploy_docs_tests(self) -> None:
        s, sc, full = suites_of("legacy-web/app.js")
        self.assertFalse(full)
        self.assertEqual(s, BROWSERS | {"node_contracts", "brand_names_check"})
        self.assertTrue(s.isdisjoint(CARGO))
        s, sc, full = suites_of("deploy/deploy.py")
        self.assertEqual((s, sc, full), ({"deploy_native_handlers", "deploy_testplan"}, {"tests/deploy_dry_run.py"}, False))
        for doc in ("docs/deployment.md", "README.md", "crates/ptyhost-client/README.md"):
            s, sc, full = suites_of(doc)
            self.assertEqual((s, sc, full), (set(), {"tests/check_docs_links.py", "tests/check_agents_md.py"}, False), doc)
        self.assertEqual(suites_of("tests/search_suite.py")[0], {"search_suite"})
        self.assertEqual(suites_of("tests/legacy_contract.mjs")[0], {"node_contracts"})
        self.assertEqual(suites_of("tests/lifecycle_browser.py")[0], {"lifecycle_browser", "lifecycle_browser_native_binding"})
        self.assertEqual(suites_of("tests/hub_fake_node.py")[0], {"hub_browser", "hub_http_suite"})
        self.assertEqual(suites_of("tests/check_docs_links.py")[1], {"tests/check_docs_links.py"})
        self.assertTrue(suites_of("tests/fake_claude_cli.py")[2], "a shared helper without a suite prefix is broad")
        self.assertTrue(suites_of("tests/fixtures/x.json")[2])

    def test_broad_ignored_and_union(self) -> None:
        for path in ("Cargo.toml", "Cargo.lock", ".github/workflows/ci.yml", "weird/thing.txt", "web/src/main.ts"):
            self.assertTrue(suites_of(path)[2], path)
        for path in (".gitignore", ".gitattributes"):
            self.assertEqual(suites_of(path), (set(), set(), False), path)
        plan = tp.plan_for("affected", ["crates/sessiondock/src/search/mod.rs", "docs/x.md", "tests/legacy_contract.mjs"], NAMES)
        self.assertFalse(plan["full"])
        self.assertEqual(set(plan["suites"]), CARGO | {"search_browser", "search_suite", "node_contracts"})
        self.assertEqual(plan["scripts"], ["tests/check_agents_md.py", "tests/check_docs_links.py"])
        self.assertEqual(plan["rules"]["docs/x.md"], "docs/")
        plan = tp.plan_for("affected", ["docs/x.md", "Cargo.lock"], NAMES)
        self.assertEqual((plan["full"], plan["suites"], plan["broad_by"]), (True, [], ["Cargo.lock"]))
        self.assertEqual(plan["scripts"], ["tests/check_agents_md.py", "tests/check_docs_links.py"],
                         "scripts outside the sweep still run next to a full sweep")
        self.assertTrue(tp.plan_for("full", [], NAMES)["full"])
        self.assertEqual(tp.plan_for("none", ["Cargo.lock"], NAMES)["full"], False)

    def test_alias_table_against_real_list(self) -> None:
        """Every alias row still selects at least one Python suite of the real runner."""
        names = tp.list_suites("target/release/sessiondock")
        self.assertGreater(len(names), 50)
        for module in tp.MODULE_SUITES:
            s, _, full = (lambda p: (set(p["suites"]), p["scripts"], p["full"]))(
                tp.plan_for("affected", [f"crates/sessiondock/src/{module}/x.rs"], names))
            self.assertFalse(full, module)
            self.assertTrue(s - {n for n in names if n.startswith("cargo_")}, f"{module}: no Python suite matched")
        s = set(tp.plan_for("affected", ["crates/ptyhost/src/main.rs"], names)["suites"])
        self.assertTrue({"terminal_browser", "lifecycle_browser", "host_identity"} <= s, s)
        s = set(tp.plan_for("affected", ["legacy-web/index.html"], names)["suites"])
        self.assertTrue({"node_contracts", "legacy_browser", "brand_names_check"} <= s and len(s) > 20, s)
        self.assertEqual(set(tp.plan_for("affected", ["deploy/sdtargets/linux.py"], names)["suites"]),
                         {"deploy_native_handlers", "deploy_testplan"})


class BaseCommitTest(unittest.TestCase):
    def test_explicit_and_markers(self) -> None:
        h1, h2 = git("rev-parse", "HEAD~1"), git("rev-parse", "HEAD~2")
        self.assertEqual(tp.resolve_base("HEAD~1", {})[0], h1)
        with self.assertRaises(ValueError):
            tp.resolve_base("no-such-ref-xyz", {})
        sha, how = tp.resolve_base(None, {"a": f"{h1} {h1[:7]} 2026-09-15T00:00:00Z dirty=0",
                                          "b": f"{h2} {h2[:7]} 2026-09-14T00:00:00Z dirty=0",
                                          "c": None, "d": "not a sha at all"})
        self.assertEqual(sha, h2, "the OLDEST marker (largest diff) wins")
        # `HEAD~2` follows first parents; the count is `git rev-list`'s, which also
        # counts commits merged in between, so derive it instead of hard-coding 2.
        behind = git("rev-list", "--count", f"{h2}..HEAD")
        self.assertIn(f"oldest etc/deployed-commit marker (b, {behind} commits behind HEAD)", how)
        sha, how = tp.resolve_base(None, {"only": f"{h1[:10]} short"})
        self.assertEqual(sha, h1, "abbreviated markers resolve")
        sha, how = tp.resolve_base(None, {"x": None})
        expect = tp.commit_of("origin/main") or h1
        self.assertEqual(sha, expect)
        self.assertIn("origin/main" if tp.commit_of("origin/main") else "HEAD~1", how)

    def test_changed_files(self) -> None:
        h1 = git("rev-parse", "HEAD~1")
        want = sorted(set(git("diff", "--name-only", f"{h1}..HEAD").splitlines()))
        self.assertEqual(tp.changed_files(h1, False), want)
        self.assertTrue(set(want) <= set(tp.changed_files(h1, True)))


class FakeRun:
    """Replaces testplan.stream: records argv, prints progress, writes tests.json, returns rc."""

    def __init__(self, rc: int = 0, failing: tuple[str, ...] = ()):
        self.rc, self.failing, self.calls = rc, failing, []

    def __call__(self, argv, log, deadline, out=print):
        self.calls.append(list(argv))
        out("[  1/1] START stub")
        log.write("stub\n")
        if str(tp.RUNNER) in argv:
            j = Path(opt(argv, "json"))
            names = (opt(argv, "only") or "full").split(",")
            log_dir = Path(opt(argv, "log-dir"))
            rows = [{"name": n, "status": "FAIL" if n in self.failing else "PASS", "log": str(log_dir / f"{n}.log")}
                    for n in names]
            j.parent.mkdir(parents=True, exist_ok=True)
            j.write_text(json.dumps({"results": rows}), encoding="utf-8")
            return 1 if any(r["status"] == "FAIL" for r in rows) else self.rc
        return self.rc


class GateCliTest(unittest.TestCase):
    """deploy.py in-process with the runner, the diff and `--list` stubbed."""

    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="sessiondock-deploy-testplan-"))
        self.prefix = self.root / "prefix"
        for sub in ("bin", "web", "etc", "host"):
            (self.prefix / sub).mkdir(parents=True)
        (self.prefix / "bin" / "sessiondock").write_bytes(b"fake\n")
        (self.prefix / "web" / "index.html").write_text("<html></html>\n", encoding="utf-8")
        h2 = git("rev-parse", "HEAD~2")
        (self.prefix / "etc" / "deployed-commit").write_text(f"{h2} {h2[:7]} 2026-09-14T00:00:00Z dirty=0\n", encoding="utf-8")
        self.targets = self.root / "targets.json"
        self.targets.write_text(json.dumps({"build": {}, "targets": [
            {"name": "local", "kind": "linux-node", "ssh": None, "prefix": str(self.prefix),
             "health_url": "http://127.0.0.1:1/api/meta",
             "service": {"unit": "sessiondock-nonexistent-test.service", "scope": "user"}, "binaries": ["sessiondock"]}]}))
        self.saved = (tp.stream, tp.changed_files, tp.list_suites)
        self.fake = FakeRun()
        tp.stream = self.fake
        tp.changed_files = lambda base, dirty: ["legacy-web/app.js", "docs/deployment.md"]
        tp.list_suites = lambda binary: NAMES
        # This fixture builds HEAD with a stubbed diff/runner. Real workspace
        # edits must not turn its synthetic clean build into a refusal.
        clean_build = patch.object(deploy, "dirty_files", return_value=[])
        clean_build.start()
        self.addCleanup(clean_build.stop)
        self.stages: list[Path] = []

    def tearDown(self) -> None:
        tp.stream, tp.changed_files, tp.list_suites = self.saved
        shutil.rmtree(self.root, ignore_errors=True)
        for stage in self.stages:
            if stage.resolve().is_relative_to(STAGE_ROOT.resolve()):
                shutil.rmtree(stage, ignore_errors=True)

    def cli(self, *argv: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        before = set(STAGE_ROOT.iterdir()) if STAGE_ROOT.is_dir() else set()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                rc = deploy.main([*argv, "--targets-file", str(self.targets)])
            except SystemExit as e:
                rc = int(e.code or 0)
        for line in out.getvalue().splitlines():   # only stages this call created are ours to delete
            if line.startswith("stage /") and Path(line.split(" ", 1)[1].strip()) not in before:
                self.stages.append(Path(line.split(" ", 1)[1].strip()))
        return rc, out.getvalue(), err.getvalue()

    def artifacts(self) -> dict:
        return json.loads((self.stages[-1] / "artifacts.json").read_text(encoding="utf-8"))

    def test_none_skips(self) -> None:
        rc, out, _ = self.cli("build", "--web-only", "--test", "none")
        self.assertEqual(rc, 0, out)
        self.assertIn("tests skipped by --test none", out)
        self.assertEqual(self.fake.calls, [])
        art = self.artifacts()
        self.assertEqual((art["test_mode"], art["test_base"], art["test_suites"], art["test_result"]),
                         ("none", None, [], "skipped"))
        rc, out, _ = self.cli("build", "--web-only")
        self.assertEqual(self.artifacts()["test_mode"], "none", "build defaults to none")
        rc, out, _ = self.cli("push", "--dry-run", "--targets", "local", "--web-only", "--stage", str(self.stages[-1]))
        self.assertEqual(rc, 0, out)
        self.assertIn("stage tests: mode=none result=skipped base=- suites=0", out)
        self.assertEqual(self.fake.calls, [], "push never runs tests")

    def test_affected_plan_runner_and_record(self) -> None:
        rc, out, err = self.cli("build", "--web-only", "--test", "affected", "--test-base", "HEAD~1")
        self.assertEqual(rc, 0, out + err)
        base = git("rev-parse", "HEAD~1")
        self.assertIn(f"test gate: mode=affected  base {base[:12]} (--test-base HEAD~1)", out)
        self.assertIn("changed files (2):", out)
        self.assertIn("  legacy-web/app.js  [legacy-web/]", out)
        self.assertIn("  docs/deployment.md  [docs/]", out)
        expected = sorted(BROWSERS | {"node_contracts", "brand_names_check"})
        self.assertIn(f"selected suites ({len(expected)}): {', '.join(expected)}", out)
        self.assertIn("extra scripts: tests/check_agents_md.py, tests/check_docs_links.py", out)
        self.assertIn("[  1/1] START stub", out, "runner output is streamed to the console")
        self.assertIn("tests: passed (mode affected", out)
        stage = self.stages[-1]
        self.assertEqual(len(self.fake.calls), 3)
        runner = self.fake.calls[0]
        self.assertEqual(runner[:2], [sys.executable, str(tp.RUNNER)])
        self.assertEqual(opt(runner, "only"), ",".join(expected))
        self.assertEqual(opt(runner, "binary"), "target/release/sessiondock", "web-only stage has no binary")
        self.assertEqual(opt(runner, "log-dir"), str(stage / "logs" / "validation"))
        self.assertEqual(opt(runner, "json"), str(stage / "logs" / "tests.json"))
        self.assertNotIn("--" + "include-real", runner)
        self.assertEqual(self.fake.calls[1:], [[sys.executable, "tests/check_agents_md.py"], [sys.executable, "tests/check_docs_links.py"]])
        self.assertTrue((stage / "logs" / "tests.log").is_file())
        art = self.artifacts()
        self.assertEqual((art["test_mode"], art["test_base"], art["test_result"], art["test_full"]), ("affected", base, "passed", False))
        self.assertEqual(art["test_suites"], expected + ["tests/check_agents_md.py", "tests/check_docs_links.py"])
        rc, out, _ = self.cli("push", "--dry-run", "--targets", "local", "--web-only", "--stage", str(stage))
        self.assertIn(f"stage tests: mode=affected result=passed base={base[:12]} suites={len(expected) + 2}", out)

    def test_failure_exits_before_push(self) -> None:
        self.fake.failing = ("legacy_browser", "brand_names_check")
        rc, out, err = self.cli("deploy", "--dry-run", "--targets", "local", "--web-only", "--test-base", "HEAD~1")
        self.assertEqual(rc, 1)
        self.assertIn("tests: failed (mode affected", out)
        self.assertIn("failing suites (nothing was uploaded):", err)
        stage = self.stages[-1]
        for name in self.fake.failing:
            self.assertIn(f"  {name}  {stage / 'logs' / 'validation' / (name + '.log')}", err)
        self.assertNotIn("DRY RUN: push", out, "push must not start after a failed gate")
        self.assertEqual(self.artifacts()["test_result"], "failed")
        self.assertFalse(list(stage.glob("report-*.json")), "no push report was written")

    def test_unknown_path_is_full(self) -> None:
        tp.changed_files = lambda base, dirty: ["weird/thing.txt", "docs/x.md"]
        rc, out, _ = self.cli("build", "--web-only", "--test", "affected", "--test-base", "HEAD~1")
        self.assertEqual(rc, 0, out)
        self.assertIn("selected: full sweep (broad rule hit by: weird/thing.txt)", out)
        self.assertIsNone(opt(self.fake.calls[0], "only"))
        self.assertEqual([c[1] for c in self.fake.calls], [str(tp.RUNNER), "tests/check_agents_md.py", "tests/check_docs_links.py"],
                         "a full sweep runs the runner without --only, then the docs scripts the .md change asked for")
        self.assertEqual((self.artifacts()["test_full"], self.artifacts()["test_suites"]),
                         (True, ["tests/check_agents_md.py", "tests/check_docs_links.py"]))
        rc, out, _ = self.cli("build", "--web-only", "--test", "full")
        self.assertIn("test gate: mode=full", out)
        self.assertIsNone(opt(self.fake.calls[-1], "only"))

    def test_deploy_default_affected_uses_marker(self) -> None:
        rc, out, err = self.cli("deploy", "--dry-run", "--targets", "local", "--web-only")
        self.assertEqual(rc, 0, out + err)
        h2 = git("rev-parse", "HEAD~2")
        behind = git("rev-list", "--count", f"{h2}..HEAD")
        self.assertIn(f"base {h2[:12]} (oldest etc/deployed-commit marker (local, {behind} commits behind HEAD))", out)
        self.assertIn("DRY RUN: push", out)
        self.assertIn("stage tests: mode=affected result=passed", out)
        self.assertEqual(self.artifacts()["test_base"], h2)


if __name__ == "__main__":
    unittest.main()
