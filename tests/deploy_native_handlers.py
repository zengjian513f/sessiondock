#!/usr/bin/env python3
"""Offline pins for the macOS and Windows deploy handlers (deploy/sdtargets/{macos,windows}.py).

A fake Shell records every command instead of executing it and answers from a small
state machine (launchd pid, tasklist, shasum/certutil, web digests), so the exact
command sequence of stage/backup/swap/restart/verify/rollback is asserted for both
kinds and the verify invariants are shown to fail when ptyhost pids vanish, the
on-disk hash differs, the build hash does not move with web, or the Windows service
lands outside desktop session 1. The per-platform test step (DeployOptions.test_mode:
Rust tests on the node between extraction and build) is pinned for both kinds: absent
with `none`, present with `affected`/`full`, and a failing run stops stage() before
anything is staged. No network, no subprocess, < 5 s.

    python3 tests/deploy_native_handlers.py
"""
from __future__ import annotations

import io
import json
import re
import sys
import tarfile
import tempfile
import time
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "deploy"))

from sdtargets import base  # noqa: E402
from sdtargets.base import Artifacts, DeployOptions, ShellError, Target, handler_for  # noqa: E402
from sdtargets import macos, windows  # noqa: E402

SHA_OLD = "a" * 64
SHA_NEW = "b" * 64
FAILURES: list[str] = []


def check(cond: bool, msg: str) -> None:
    if not cond:
        FAILURES.append(msg)
        print(f"FAIL: {msg}")


def make_artifacts(tmp: Path, *, prefixed: bool = False, web_only: bool = False) -> Artifacts:
    web = tmp / "web"
    web.mkdir(exist_ok=True)
    (web / "index.html").write_text("<html>x</html>")
    (web / "app.js").write_text("1")
    tar_path = tmp / "source.tar"
    top = "sessiondock-1234567/" if prefixed else ""
    with tarfile.open(tar_path, "w") as tf:
        for name, data in ((f"{top}Cargo.toml", b"[workspace]\n"),
                           (f"{top}crates/sessiondock/src/main.rs", b"fn main() {}\n")):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tf.addfile(info, io.BytesIO(data))
    return Artifacts(commit="1234567890abcdef1234567890abcdef12345678", short="1234567", dirty=False,
                     built_at="2026-09-15T04:00:00Z", web_dir=web, binaries={}, sha256={},
                     source_archive=tar_path, web_only=web_only)


# --------------------------------------------------------------------------- macOS
MAC_TARGET = Target(name="macos-node", kind="macos-node", prefix="/Users/example/sessiondock",
                    ssh="user@macos-node", ssh_port=22,
                    service={"launchd_label": "cn.example.sessiondock", "gui_uid": 501},
                    build_on_target=True,
                    extra={"source_dir": "/Users/example/sessiondock-src", "cargo": "/Users/example/.cargo/bin/cargo"})


class MacFake(base.Shell):
    """Answers like vela would; mutations move hashes/digests around in `self.state`."""

    def __init__(self, target, *, web_differs: bool = True, tests_pass: bool = True):
        super().__init__(target)
        self.calls: list[str] = []
        self.tests_pass = tests_pass
        p = target.prefix
        self.state = {"pid": 100, "ptyhost": [51212], "hosts": 2, "digest": {f"{p}/web": "d-old"},
                      "files": {f"{p}/bin/sessiondock": SHA_OLD}, "marker": None,
                      "backups": ["backup-deploy-f9c51d2-20260914-195842", "backup-deploy-a3c322c-20260915-002433",
                                  "backup-deploy-3669092-20260914-204511"]}
        self.web_differs = web_differs
        self.state["build"] = "build-d-old"

    def run(self, cmd, timeout=60, check=False):
        self.calls.append(cmd)
        s, files = self.state, self.state["files"]
        rc, out = 0, ""
        m_cp = re.match(r"cp -f (\S+) (\S+)$", cmd)
        m_mv = re.match(r"mv -f (\S+) (\S+)$", cmd)
        m_cpmv = re.match(r"cp -f (\S+) (\S+) && mv -f (\S+) (\S+)$", cmd)
        if cmd == "uname -s":
            out = "Darwin\n"
        elif cmd.startswith("launchctl print"):
            out = f"\tstate = running\n\tpid = {s['pid']}\n"
        elif cmd.startswith("curl"):
            out = json.dumps({"build": s["build"]})
        elif cmd.startswith("shasum -a 256 "):
            path = cmd.split()[3]
            out = f"{files[path]}  {path}\n" if path in files else ""
        elif cmd.startswith("pgrep -x ptyhost"):
            out = "".join(f"{p}\n" for p in s["ptyhost"])
        elif "grep -c '\\.json$'" in cmd:
            out = f"{s['hosts']}\n"
        elif cmd.startswith("cat ") and "deployed-commit" in cmd:
            out = s["marker"] or ""
        elif "xargs shasum -a 256" in cmd:
            d = cmd.split()[1]
            out = s["digest"].get(d, "") + "\n" if d in s["digest"] else ""
            rc = 0 if d in s["digest"] else 1
        elif "cargo test" in cmd:
            rc = 0 if self.tests_pass else 101
            out = "test result: ok. 12 passed\n" if self.tests_pass else "test result: FAILED. 1 failed\n"
        elif "cargo build" in cmd:
            out = "   Compiling sessiondock v0.1.0\n    Finished `release` profile [optimized] target(s) in 40.0s\n"
            files["/Users/example/sessiondock-src/target/release/sessiondock"] = SHA_NEW
        elif m_cpmv:
            files[m_cpmv.group(4)] = files[m_cpmv.group(1)]
        elif m_cp:
            files[m_cp.group(2)] = files[m_cp.group(1)]
        elif m_mv:
            files[m_mv.group(2)] = files.pop(m_mv.group(1))
        elif cmd.startswith("rsync -a --delete "):
            src, dst = cmd.split()[3].rstrip("/"), cmd.split()[4].rstrip("/")
            s["digest"][dst] = s["digest"][src]
        elif cmd.startswith("launchctl kickstart -k"):
            s["pid"] += 1
            s["build"] = "build-" + s["digest"][f"{self.t.prefix}/web"]
        elif cmd.startswith("test -f "):
            rc = 0 if cmd.split()[2] in files else 1
        elif cmd.startswith("test -d ") and "rsync" in cmd:
            src, dst = cmd.split()[-2].rstrip("/"), cmd.split()[-1].rstrip("/")
            s["digest"][dst] = s["digest"][src]
        elif cmd.startswith("printf"):
            s["marker"] = cmd.split("'")[3]
        elif "-name 'backup-deploy-*'" in cmd:
            out = "".join(f"{n}\n" for n in s["backups"])
        elif cmd.startswith("rm -rf ") and "backup-deploy-" in cmd:
            s["backups"].remove(cmd.split("/")[-1])
        elif cmd.startswith("mkdir -p ") and "backup-deploy-" in cmd:
            bk = cmd.split()[2].removesuffix("/bin")
            files[f"{bk}/bin/sessiondock"] = files[f"{self.t.prefix}/bin/sessiondock"]
            s["digest"][f"{bk}/web"] = s["digest"][f"{self.t.prefix}/web"]
        if check and rc != 0:
            raise ShellError(cmd, rc, out)
        return rc, out

    def upload(self, local, remote, *, delete=False, timeout=300):
        self.calls.append(f"UPLOAD {Path(local).name}{'/' if str(local).endswith('/') else ''} -> {remote}"
                          f"{' --delete' if delete else ''}")
        if remote.endswith("web.staging/"):
            self.state["digest"][remote.rstrip("/")] = "d-new" if self.web_differs else "d-old"


def mac_handler(tmp, *, web_differs=True, opts=None, prefixed=False, web_only=False, tests_pass=True):
    h = macos.MacOSNode(MAC_TARGET, make_artifacts(tmp, prefixed=prefixed, web_only=web_only),
                        opts or DeployOptions(health_timeout=0), lambda s: None)
    h.sh = MacFake(MAC_TARGET, web_differs=web_differs, tests_pass=tests_pass)
    return h


def test_macos_test_step(tmp: Path) -> None:
    """test_mode != none: workspace tests run on the node after tar -x and before cargo build."""
    src, p = "/Users/example/sessiondock-src", MAC_TARGET.prefix
    test_cmd = (f"cd {src} && mkdir -p /private/tmp/sdtest && TMPDIR=/private/tmp/sdtest /Users/example/.cargo/bin/cargo "
                "test --workspace --locked >.deploy-test.log 2>&1; rc=$?; tail -n 40 .deploy-test.log; "
                "[ $rc -eq 0 ] && rm -f .deploy-test.log; exit $rc")
    h = mac_handler(tmp, opts=DeployOptions(health_timeout=0, test_mode="affected", log_dir=tmp / "logs"))
    h.probe()
    plan = h.plan()
    at = [i for i, s in enumerate(plan) if s.startswith("test (affected): ") and "cargo test --workspace --locked" in s]
    check(len(at) == 1, f"mac plan lacks the test step: {plan}")
    check(at and at[0] < next(i for i, s in enumerate(plan) if "cargo build" in s), "mac plan: test before build")
    h.sh.calls.clear()
    h.stage()
    check(h.sh.calls[3] == test_cmd, f"mac test command {h.sh.calls[3]!r}")
    check("tar -xf" in h.sh.calls[2] and "cargo build" in h.sh.calls[4], f"mac test sits between tar -x and build: {h.sh.calls}")
    check(h.expected_sha == {"sessiondock": SHA_NEW}, "mac stage still stages after passing tests")
    # mode none: the default full-cycle pin already proves no `cargo test` call; `full` behaves like affected
    h2 = mac_handler(tmp, opts=DeployOptions(health_timeout=0, test_mode="full"))
    h2.probe(); h2.sh.calls.clear(); h2.stage()
    check(sum("cargo test" in c for c in h2.sh.calls) == 1, "mac full mode runs the tests once")
    # a failing run stops stage() before anything is built or staged
    h3 = mac_handler(tmp, opts=DeployOptions(health_timeout=0, test_mode="affected", log_dir=tmp / "logs"), tests_pass=False)
    h3.probe(); h3.sh.calls.clear()
    try:
        h3.stage()
        check(False, "failing node tests must stop stage()")
    except RuntimeError as e:
        check("cargo test failed on the node (rc=101)" in str(e) and f"{src}/.deploy-test.log" in str(e)
              and str(tmp / "logs" / "macos-node.log") in str(e), f"mac test failure message {e}")
    check(not any("cargo build" in c or ".new" in c for c in h3.sh.calls) and h3.staged == [] and h3.expected_sha == {},
          f"nothing built or staged after failing tests: {h3.sh.calls}")
    # web-only never tests (there is nothing to build)
    h4 = mac_handler(tmp, opts=DeployOptions(web_only=True, health_timeout=0, test_mode="full"))
    h4.probe(); h4.sh.calls.clear(); h4.stage()
    check(not any("cargo" in c for c in h4.sh.calls), "web-only stage runs no tests")


def test_macos_full_cycle(tmp: Path) -> None:
    h = mac_handler(tmp)
    sh = h.sh
    p, src = MAC_TARGET.prefix, "/Users/example/sessiondock-src"
    before = h.probe()
    check(before.reachable and before.active and before.build == "build-d-old", f"mac probe {before}")
    check(before.binary_sha == {"sessiondock": SHA_OLD} and before.ptyhost_pids == [51212]
          and before.host_records == 2, f"mac probe fields {before}")
    check(sh.calls == [
        "uname -s",
        "launchctl print gui/501/cn.example.sessiondock 2>/dev/null | grep -E '^[[:space:]]*(state|pid) =' || true",
        "curl -sS --max-time 10 http://127.0.0.1:8741/api/meta",
        f"shasum -a 256 {p}/bin/sessiondock 2>/dev/null || true",
        "pgrep -x ptyhost || true",
        f"ls {p}/host 2>/dev/null | grep -c '\\.json$' || true",
        f"cat {p}/etc/deployed-commit 2>/dev/null || true",
        f"cd {p}/web 2>/dev/null && find . -type f | LC_ALL=C sort | xargs shasum -a 256 | shasum -a 256 | cut -c1-16",
    ], f"mac probe sequence {sh.calls}")
    plan = h.plan()
    check(any("cargo build --release --locked -p sessiondock" in s for s in plan)
          and any("launchctl kickstart -k gui/501/cn.example.sessiondock" in s for s in plan), f"mac plan {plan}")
    check(not any("-p ptyhost" in s for s in plan), "mac plan builds ptyhost without --with-ptyhost")

    sh.calls.clear()
    h.stage()
    check(sh.calls == [
        f"mkdir -p {src}/.deploy",
        f"UPLOAD source.tar -> {src}/.deploy/source.tar",
        f"cd {src} && find . -mindepth 1 -maxdepth 1 ! -name target ! -name .deploy -exec rm -rf {{}} + && "
        "tar -xf .deploy/source.tar --strip-components=0 && rm -rf .deploy && test -f Cargo.toml",
        f"cd {src} && /Users/example/.cargo/bin/cargo build --release --locked -p sessiondock >.deploy-build.log 2>&1; "
        "rc=$?; tail -n 25 .deploy-build.log; rm -f .deploy-build.log; exit $rc",
        f"cp -f {src}/target/release/sessiondock {p}/bin/sessiondock.new",
        f"shasum -a 256 {p}/bin/sessiondock.new 2>/dev/null || true",
        f"mkdir -p {p}/web.staging",
        f"UPLOAD web/ -> {p}/web.staging/ --delete",
        f"cd {p}/web.staging 2>/dev/null && find . -type f | LC_ALL=C sort | xargs shasum -a 256 | shasum -a 256 | cut -c1-16",
    ], f"mac stage sequence {sh.calls}")
    check(h.expected_sha == {"sessiondock": SHA_NEW} and h.web_changed, "mac stage recorded new sha / web changed")

    sh.calls.clear()
    bk = h.backup()
    check(re.fullmatch(rf"{re.escape(p)}/backup-deploy-1234567-\d{{8}}-\d{{6}}", bk) is not None, f"backup name {bk}")
    check(sh.calls == [f"mkdir -p {bk}/bin && cd {p}/bin && find . -maxdepth 1 -type f ! -name '*.new' -exec cp -p {{}} {bk}/bin/ \\; "
                       f"&& cp -Rp {p}/web {bk}/web"], f"mac backup {sh.calls}")

    sh.calls.clear()
    h.swap()
    h.restart()
    check(sh.calls == [
        f"mv -f {p}/bin/sessiondock.new {p}/bin/sessiondock",
        f"rsync -a --delete {p}/web.staging/ {p}/web/ && rm -rf {p}/web.staging",
        "launchctl kickstart -k gui/501/cn.example.sessiondock",
    ], f"mac swap/restart {sh.calls}")

    sh.calls.clear()
    v = h.verify()
    check(v.ok, f"mac verify should pass: {v.detail}")
    check(v.build == "build-d-new" and v.binary_sha == {"sessiondock": SHA_NEW} and v.ptyhost_pids == [51212],
          f"mac verify fields {v}")
    check(sh.calls == [
        "launchctl print gui/501/cn.example.sessiondock 2>/dev/null | grep -E '^[[:space:]]*(state|pid) =' || true",
        "curl -sS --max-time 10 http://127.0.0.1:8741/api/meta",
        f"shasum -a 256 {p}/bin/sessiondock 2>/dev/null || true",
        "pgrep -x ptyhost || true",
        f"ls {p}/host 2>/dev/null | grep -c '\\.json$' || true",
    ], f"mac verify sequence {sh.calls}")

    sh.calls.clear()
    h.write_marker()
    check(sh.calls == [f"printf '%s\\n' '1234567890abcdef1234567890abcdef12345678 1234567 2026-09-15T04:00:00Z dirty=0' "
                       f"> {p}/etc/deployed-commit"], f"mac marker {sh.calls}")
    check(sh.state["marker"] == "1234567890abcdef1234567890abcdef12345678 1234567 2026-09-15T04:00:00Z dirty=0", "marker text")

    # rollback restores the backup and verify passes against the pre-deploy state again
    sh.calls.clear()
    h.rollback(bk)
    check(sh.calls == [
        f"test -f {bk}/bin/sessiondock",
        f"cp -f {bk}/bin/sessiondock {p}/bin/sessiondock.new && mv -f {p}/bin/sessiondock.new {p}/bin/sessiondock",
        f"shasum -a 256 {p}/bin/sessiondock 2>/dev/null || true",
        f"test -d {bk}/web && rsync -a --delete {bk}/web/ {p}/web/",
        f"rm -rf {p}/web.staging",
        f"cd {p}/web 2>/dev/null && find . -type f | LC_ALL=C sort | xargs shasum -a 256 | shasum -a 256 | cut -c1-16",
        "launchctl kickstart -k gui/501/cn.example.sessiondock",
    ], f"mac rollback sequence {sh.calls}")
    v2 = h.verify()
    check(v2.ok and v2.binary_sha == {"sessiondock": SHA_OLD} and v2.build == "build-d-old", f"mac verify after rollback {v2}")

    # prune keeps the newest by UTC stamp, not by name
    sh.calls.clear()
    removed = h.prune_backups(2)
    check(removed == [f"{p}/backup-deploy-f9c51d2-20260914-195842"], f"mac prune {removed}")
    check(sh.calls[1:] == [f"rm -rf {p}/backup-deploy-f9c51d2-20260914-195842"], f"mac prune commands {sh.calls}")


def test_macos_web_only_and_prefix(tmp: Path) -> None:
    h = mac_handler(tmp, opts=DeployOptions(web_only=True, health_timeout=0))
    h.probe()
    h.sh.calls.clear()
    h.stage()
    check(not any("cargo" in c or "source.tar" in c for c in h.sh.calls), f"web-only must not build: {h.sh.calls}")
    check(h.sh.calls[0] == f"mkdir -p {MAC_TARGET.prefix}/web.staging", "web-only stages web")
    h.backup(); h.swap(); h.restart()
    v = h.verify()
    check(v.ok and v.binary_sha == {"sessiondock": SHA_OLD}, f"web-only verify {v}")
    # a tar wrapped in one prefix directory is stripped
    (tmp / "pre").mkdir(exist_ok=True)
    h2 = mac_handler(tmp / "pre", prefixed=True)
    h2.probe(); h2.sh.calls.clear(); h2.stage()
    check(any("--strip-components=1" in c for c in h2.sh.calls), "prefixed tar gets --strip-components=1")
    # ptyhost only with the option
    h3 = macos.MacOSNode(MAC_TARGET, make_artifacts(tmp), DeployOptions(with_ptyhost=True, health_timeout=0), lambda s: None)
    check(h3.bins == ["sessiondock", "ptyhost"], "with_ptyhost adds ptyhost")
    check(any("-p sessiondock -p ptyhost" in s for s in h3.plan()), "plan builds ptyhost with --with-ptyhost")


def test_macos_verify_catches(tmp: Path) -> None:
    def deployed(**kw):
        h = mac_handler(tmp, **kw)
        h.probe(); h.stage(); h.backup(); h.swap(); h.restart()
        return h
    h = deployed()
    h.sh.state["ptyhost"] = []
    v = h.verify()
    check(not v.ok and "ptyhost pids lost: [51212]" in v.detail, f"lost ptyhost undetected: {v.detail}")
    h = deployed()
    h.sh.state["files"][f"{MAC_TARGET.prefix}/bin/sessiondock"] = "c" * 64
    v = h.verify()
    check(not v.ok and "on-disk sha" in v.detail, f"sha mismatch undetected: {v.detail}")
    h = deployed()
    h.sh.state["build"] = "build-d-old"
    v = h.verify()
    check(not v.ok and "web changed but build still" in v.detail, f"stale build undetected: {v.detail}")
    h = deployed(web_differs=False)
    v = h.verify()
    check(v.ok and v.build == "build-d-old", f"identical web must keep the build hash: {v.detail}")
    h = deployed()
    h.sh.state["pid"] = 100
    v = h.verify()
    check(not v.ok and "unchanged: no restart" in v.detail, f"missing restart undetected: {v.detail}")
    h = deployed()
    h.sh.state["hosts"] = 1
    v = h.verify()
    check(not v.ok and "host records 2 -> 1" in v.detail, f"host record loss undetected: {v.detail}")
    bad = Target(**{**MAC_TARGET.__dict__, "extra": {"source_dir": "/Users/example/sessiondock/src"}})
    try:
        macos.MacOSNode(bad, make_artifacts(tmp), DeployOptions(), lambda s: None).plan()
        check(False, "source_dir inside the prefix must be refused")
    except ValueError:
        pass


# --------------------------------------------------------------------------- Windows
WIN_TARGET = Target(name="windows-node", kind="windows-node", prefix="C:\\Example\\sessiondock",
                    ssh="user@windows-node", ssh_port=22,
                    service={"restart_cmd": "C:\\Example\\sessiondock\\restart-session1.cmd"},
                    build_on_target=True,
                    extra={"source_dir": "C:\\Example\\sessiondock-src",
                           "toolchain_bin": "C:\\Example\\.rustup\\toolchains\\stable-x86_64-pc-windows-msvc\\bin"})
WPY = "C:\\Example\\anaconda3\\python.exe"


class WinFake(windows.CmdShell):
    def __init__(self, target, *, web_differs=True, session="1", tests_pass=True):
        super().__init__(target)
        self.calls: list[str] = []
        self.tests_pass = tests_pass
        self.rendered_test = None     # set from the uploaded build.cmd (`set "TEST=..."`)
        p = target.prefix
        self.state = {"svc": [100], "ptyhost": [7064, 11268], "hosts": 3, "build": "build-d-old", "session": session,
                      "digest": {f"{p}\\web": "d-old"}, "files": {f"{p}\\bin\\sessiondock.exe": SHA_OLD},
                      "backups": ["backup-deploy-f9c51d2-20260914-195842", "backup-deploy-a3c322c-20260915-002433"]}
        self.web_differs = web_differs

    def run(self, cmd, timeout=60, check=False):
        self.calls.append(cmd)
        s, files, p = self.state, self.state["files"], self.t.prefix
        rc, out = 0, ""
        if cmd == "ver":
            out = "\nMicrosoft Windows [Version 10.0.19045.5011]\n"
        elif cmd.startswith('tasklist /FI "IMAGENAME eq '):
            image = cmd.split("eq ")[1].split('"')[0]
            pids = s["svc"] if image == "sessiondock.exe" else s["ptyhost"]
            out = "".join(f'"{image}","{pid}","Console","1","12,345 K"\n' for pid in pids) or \
                "INFO: No tasks are running which match the specified criteria.\n"
        elif cmd.startswith("curl.exe"):
            out = json.dumps({"build": s["build"]})
        elif cmd.startswith("certutil -hashfile "):
            path = cmd.split('"')[1]
            rc = 0 if path in files else 1
            out = f"SHA256 hash of {path}:\n{files[path]}\nCertUtil: -hashfile command completed successfully.\n" if path in files else ""
        elif "Get-ChildItem -LiteralPath" in cmd:
            d = cmd.split("$r='")[1].split("'")[0]
            out = f"index.html {s['digest'][d]}\n" if d in s["digest"] else ""
        elif "\\host\\*.json" in cmd:
            out = f"{s['hosts']}\n"
        elif cmd.startswith("type "):
            out = ""
        elif cmd.startswith("findstr /b"):
            out = f"set PY={WPY}\nset LNK=%SD%\\SessionDock.lnk\n"
        elif cmd.endswith("build.cmd\""):
            if self.rendered_test == "1" and not self.tests_pass:
                rc, out = 18, "===TEST===\ntest result: FAILED. 1 failed\nTEST_FAILED rc=101 log=C:\\Example\\sessiondock-src\\.deploy-test.log\n"
            else:
                files[f"{p}\\bin\\sessiondock.new.exe"] = SHA_NEW
                test = "===TEST===\ntest result: ok. 12 passed\nTEST_OK\n" if self.rendered_test == "1" else "===TEST===\nTEST_SKIPPED\n"
                out = (test + "===BUILD===\nMTIME_BEFORE sessiondock 638000000000000000\n   Compiling sessiondock v0.1.0\n"
                       "    Finished `release` profile [optimized] target(s) in 60.0s\nMTIME_AFTER sessiondock 638000000600000000\n"
                       f"===STAGE===\nNEW_SHA sessiondock {SHA_NEW.upper()}\nBUILD_OK\n")
        elif cmd.endswith("backup.cmd\""):
            out = "BACKUP_OK\n"
        elif cmd.endswith("swap-restart.cmd\""):
            swapped = f"{p}\\bin\\sessiondock.new.exe" in files
            if swapped:
                files[f"{p}\\bin\\sessiondock.exe"] = files.pop(f"{p}\\bin\\sessiondock.new.exe")
            if f"{p}\\web.staging" in s["digest"]:
                s["digest"][f"{p}\\web"] = s["digest"].pop(f"{p}\\web.staging")
            s["svc"] = [s["svc"][0] + 100]
            s["build"] = "build-" + s["digest"][f"{p}\\web"]
            out = ("===STOP_OLD===\nVerified target PIDs: 100\n===SWAP_BINARY===\n" + ("SWAPPED sessiondock\n" if swapped else "")
                   + "===SWAP_WEB===\nWEB_MIRRORED\n===LAUNCH_IN_SESSION1===\n===VERIFY===\n"
                   f"sessiondock.exe pid={s['svc'][0]} session={s['session']} parent=pythonw.exe\nterm/list=200\n===END===\n")
        elif "Expand-Archive" in cmd and "web.staging" in cmd:
            s["digest"][f"{p}\\web.staging"] = "d-new" if self.web_differs else "d-old"
            out = "WEB_STAGED\n"
        elif "Where-Object { $_.Name -eq 'sessiondock.exe' }" in cmd:
            out = "".join(f"SESSION {pid} {s['session']}\n" for pid in s["svc"])
        elif cmd.startswith("if exist ") and "copy /y" in cmd:
            files[f"{p}\\bin\\sessiondock.new.exe"] = SHA_OLD
            out = "RESTORED\n"
        elif cmd.startswith("rd /s /q ") and "robocopy" in cmd:
            s["digest"][f"{p}\\web.staging"] = "d-old"
            rc = 1
        elif cmd.startswith("dir /b /ad"):
            out = "".join(f"{n}\n" for n in s["backups"])
        elif cmd.startswith("rd /s /q ") and "backup-deploy-" in cmd:
            s["backups"].remove(cmd.split("\\")[-1].rstrip('"'))
        if check and rc != 0:
            raise ShellError(cmd, rc, out)
        return rc, out

    def scp_upload(self, local, remote, *, timeout=300):
        self.calls.append(f"SCP {Path(local).name} -> {remote}")
        if Path(local).name == "build.cmd":
            m = re.search(r'set "TEST=(\d)"', Path(local).read_text(encoding="ascii"))
            self.rendered_test = m.group(1) if m else None


def win_handler(tmp, *, web_differs=True, session="1", opts=None, tests_pass=True):
    h = windows.WindowsNode(WIN_TARGET, make_artifacts(tmp), opts or DeployOptions(health_timeout=0), lambda s: None)
    h.sh = WinFake(WIN_TARGET, web_differs=web_differs, session=session, tests_pass=tests_pass)
    return h


def test_windows_test_step(tmp: Path) -> None:
    """test_mode != none renders TEST=1 into build.cmd; TEST_FAILED / missing TEST_OK stop stage()."""
    h = win_handler(tmp, opts=DeployOptions(health_timeout=0, test_mode="full"))
    h.probe()
    plan = "\n".join(h.plan())
    check("cargo.exe test -p sessiondock --locked (full; TEST_FAILED = FAILED before staging)" in plan
          and f"timeout {int(windows.BUILD_TIMEOUT + windows.TEST_TIMEOUT)}s" in plan, f"win plan lacks the test step: {plan[:600]}")
    h.sh.calls.clear()
    h.stage()
    check(h.sh.rendered_test == "1" and h.expected_sha == {"sessiondock": SHA_NEW}, "win stage with tests on")
    text = (Path(h.tmp.name) / "build.cmd").read_bytes().decode("ascii")
    order = ["===EXTRACT===", "===TOOLCHAIN===", "===TEST===", 'if not "%TEST%"=="1" ( echo TEST_SKIPPED & goto build )',
             '"%TC%\\cargo.exe" test -p sessiondock --locked > "%SRC%\\.deploy-test.log" 2>&1',
             "TEST_FAILED rc=%RC% log=%SRC%\\.deploy-test.log & exit /b 18", "echo TEST_OK", ":build", "===BUILD===",
             '"%TC%\\cargo.exe" build --release --locked -p sessiondock', "===STAGE==="]
    positions = [text.find(n) for n in order]
    check(all(x >= 0 for x in positions) and positions == sorted(positions), f"build.cmd test phase order {positions}")
    check('set "RUSTC=%TC%\\rustc.exe"' in text and text.find('set "RUSTC=') < text.find("===TEST==="), "RUSTC set before the tests")
    # mode none renders TEST=0 and the default full-cycle pin stays byte-identical in its command sequence
    h0 = win_handler(tmp)
    h0.probe(); h0.sh.calls.clear(); h0.stage()
    check(h0.sh.rendered_test == "0" and "tests skipped (mode none)" in "\n".join(h0.plan()), "win mode none skips tests")
    # TEST_FAILED (exit 18) stops before anything is staged
    hf = win_handler(tmp, opts=DeployOptions(health_timeout=0, test_mode="affected"), tests_pass=False)
    hf.probe(); hf.sh.calls.clear()
    try:
        hf.stage()
        check(False, "TEST_FAILED must stop stage()")
    except ShellError as e:
        check("TEST_FAILED rc=101" in str(e) and ".deploy-test.log" in str(e), f"win test failure surfaces the log: {e}")
    check(hf.staged == [] and hf.expected_sha == {} and not any("web.zip" in c for c in hf.sh.calls),
          f"nothing staged after TEST_FAILED: {hf.sh.calls}")
    # a script that exits 0 without TEST_OK while the phase is on is rejected too
    hm = win_handler(tmp, opts=DeployOptions(health_timeout=0, test_mode="affected"))
    hm.probe()
    orig = hm.sh.run

    def no_marker(cmd, timeout=60, check=False):
        rc, out = orig(cmd, timeout, check)
        return rc, out.replace("TEST_OK\n", "")
    hm.sh.run = no_marker
    try:
        hm.stage()
        check(False, "missing TEST_OK must be rejected")
    except RuntimeError as e:
        check("no TEST_OK" in str(e), f"missing TEST_OK message {e}")


def test_windows_full_cycle(tmp: Path) -> None:
    h = win_handler(tmp)
    sh = h.sh
    p, dep = WIN_TARGET.prefix, "C:\\Example\\sessiondock-src\\.deploy"
    before = h.probe()
    check(before.reachable and before.active and before.build == "build-d-old" and before.ptyhost_pids == [7064, 11268]
          and before.binary_sha == {"sessiondock": SHA_OLD} and before.host_records == 3, f"win probe {before}")
    check(h.vars == {"PY": WPY, "LNK": f"{p}\\SessionDock.lnk"}, f"PY/LNK read from restart-session1.cmd: {h.vars}")
    check(sh.calls == [
        "ver",
        'tasklist /FI "IMAGENAME eq sessiondock.exe" /FO CSV /NH',
        'curl.exe -sS --max-time 10 "http://127.0.0.1:8741/api/meta"',
        f'certutil -hashfile "{p}\\bin\\sessiondock.exe" SHA256',
        'tasklist /FI "IMAGENAME eq ptyhost.exe" /FO CSV /NH',
        f'dir /b "{p}\\host\\*.json" 2>nul | find /c /v ""',
        f'type "{p}\\etc\\deployed-commit" 2>nul',
        f"powershell -NoProfile -Command \"$r='{p}\\web'; if (Test-Path -LiteralPath $r) {{ Get-ChildItem -LiteralPath $r "
        "-Recurse -File | ForEach-Object { $_.FullName.Substring($r.Length).TrimStart('\\').Replace('\\','/') + ' ' + "
        "(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash } }\"",
        f'findstr /b /c:"set PY=" /c:"set LNK=" "{p}\\restart-session1.cmd"',
    ], f"win probe sequence {sh.calls}")

    plan = "\n".join(h.plan())
    for needle in ("real cargo.exe build --release --locked", "process_identity.py --include-supervisor",
                   "--- swap-restart.cmd ---", 'schtasks /create /tn sd-restart-s1 /tr "explorer.exe %LNK%" /sc once /st 23:59 /it /f',
                   f'set "LNK={p}\\SessionDock.lnk"', "restart: no-op"):
        check(needle in plan, f"win plan lacks {needle!r}")

    sh.calls.clear()
    h.stage()
    check(sh.calls == [
        f'if not exist "{dep}" mkdir "{dep}"',
        f"SCP build.cmd -> {dep}\\build.cmd",
        f"SCP source-1234567.zip -> {dep}\\source-1234567.zip",
        f'"{dep}\\build.cmd"',
        f'if not exist "{dep}" mkdir "{dep}"',
        f"SCP web.zip -> {dep}\\web.zip",
        f'rd /s /q "{p}\\web.staging" 2>nul & powershell -NoProfile -Command "Expand-Archive -LiteralPath '
        f"'{dep}\\web.zip' -DestinationPath '{p}\\web.staging' -Force\" & "
        f'if exist "{p}\\web.staging\\index.html" (echo WEB_STAGED) else (exit /b 5)',
        f"powershell -NoProfile -Command \"$r='{p}\\web.staging'; if (Test-Path -LiteralPath $r) {{ Get-ChildItem -LiteralPath $r "
        "-Recurse -File | ForEach-Object { $_.FullName.Substring($r.Length).TrimStart('\\').Replace('\\','/') + ' ' + "
        "(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash } }\"",
    ], f"win stage sequence {sh.calls}")
    check(h.expected_sha == {"sessiondock": SHA_NEW} and h.staged == ["sessiondock"] and h.web_changed, "win stage state")
    with zipfile.ZipFile(Path(h.tmp.name) / "source-1234567.zip") as zf:
        check(sorted(zf.namelist()) == ["Cargo.toml", "crates/sessiondock/src/main.rs"], f"source zip {zf.namelist()}")
    with zipfile.ZipFile(Path(h.tmp.name) / "web.zip") as zf:
        check(sorted(zf.namelist()) == ["app.js", "index.html"], f"web zip {zf.namelist()}")

    sh.calls.clear()
    bk = h.backup()
    check(re.fullmatch(rf"{re.escape(p)}\\backup-deploy-1234567-\d{{8}}-\d{{6}}", bk) is not None, f"win backup name {bk}")
    check(sh.calls == [f'if not exist "{dep}" mkdir "{dep}"', f"SCP backup.cmd -> {dep}\\backup.cmd", f'"{dep}\\backup.cmd"'],
          f"win backup sequence {sh.calls}")
    rendered = (Path(h.tmp.name) / "backup.cmd").read_bytes().decode("ascii")
    check(f'set "BK={bk}"' in rendered and 'robocopy "%SD%\\web" "%BK%\\web" /MIR' in rendered, "backup.cmd content")

    sh.calls.clear()
    h.swap()
    h.restart()
    check(sh.calls == [f'if not exist "{dep}" mkdir "{dep}"', f"SCP swap-restart.cmd -> {dep}\\swap-restart.cmd",
                       f'"{dep}\\swap-restart.cmd"'], f"win swap/restart sequence {sh.calls}")
    script = (Path(h.tmp.name) / "swap-restart.cmd").read_bytes()
    check(all(b < 128 for b in script) and b"\r\n" in script and b"\n" not in script.replace(b"\r\n", b""),
          "swap-restart.cmd must be ASCII with CRLF")
    text = script.decode("ascii")
    order = ["===STOP_OLD===", f'"%PY%" "%SD%\\process_identity.py" --include-supervisor', "===SWAP_BINARY===",
             "call :swap", "===SWAP_WEB===", 'robocopy "%SD%\\web.staging" "%SD%\\web" /MIR', 'del /q "%SD%\\STOP"',
             "===LAUNCH_IN_SESSION1===", 'schtasks /create /tn sd-restart-s1 /tr "explorer.exe %LNK%" /sc once /st 23:59 /it /f',
             "schtasks /run /tn sd-restart-s1", "===VERIFY===", "/api/term/list", "===END===",
             'move /y "%SD%\\bin\\%1.new.exe" "%SD%\\bin\\%1.exe"']
    positions = [text.find(n) for n in order]
    check(all(x >= 0 for x in positions) and positions == sorted(positions), f"swap-restart.cmd phase order {positions}")
    check(f'set "PY={WPY}"' in text and f'set "LNK={p}\\SessionDock.lnk"' in text and f'set "SD={p}"' in text, "swap-restart vars")
    check("taskkill" not in text and "start_sessiondock.py start" not in text, "swap-restart must not kill or start via SSH")
    check("for %N in (sessiondock)" in text.replace("%%", "%"), "swap-restart swaps the configured binaries")

    sh.calls.clear()
    v = h.verify()
    check(v.ok, f"win verify should pass: {v.detail}")
    check(v.build == "build-d-new" and v.binary_sha == {"sessiondock": SHA_NEW} and v.ptyhost_pids == [7064, 11268], f"win verify {v}")
    check(sh.calls == [
        'tasklist /FI "IMAGENAME eq sessiondock.exe" /FO CSV /NH',
        'curl.exe -sS --max-time 10 "http://127.0.0.1:8741/api/meta"',
        "powershell -NoProfile -Command \"Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'sessiondock.exe' } | "
        "ForEach-Object { 'SESSION ' + $_.ProcessId + ' ' + $_.SessionId }\"",
        f'certutil -hashfile "{p}\\bin\\sessiondock.exe" SHA256',
        'tasklist /FI "IMAGENAME eq ptyhost.exe" /FO CSV /NH',
        f'dir /b "{p}\\host\\*.json" 2>nul | find /c /v ""',
    ], f"win verify sequence {sh.calls}")

    sh.calls.clear()
    h.write_marker()
    check(sh.calls == [f"powershell -NoProfile -Command \"Set-Content -LiteralPath '{p}\\etc\\deployed-commit' -Value "
                       "'1234567890abcdef1234567890abcdef12345678 1234567 2026-09-15T04:00:00Z dirty=0' -Encoding ascii\""],
          f"win marker {sh.calls}")

    sh.calls.clear()
    h.rollback(bk)
    check(sh.calls == [
        f'if exist "{bk}\\bin\\sessiondock.exe" (copy /y "{bk}\\bin\\sessiondock.exe" "{p}\\bin\\sessiondock.new.exe" >nul & '
        "echo RESTORED) else (echo ABSENT)",
        f'certutil -hashfile "{p}\\bin\\sessiondock.new.exe" SHA256',
        f'rd /s /q "{p}\\web.staging" 2>nul & robocopy "{bk}\\web" "{p}\\web.staging" /MIR /NFL /NDL /NJH /NJS /NP >nul',
        f"powershell -NoProfile -Command \"$r='{p}\\web.staging'; if (Test-Path -LiteralPath $r) {{ Get-ChildItem -LiteralPath $r "
        "-Recurse -File | ForEach-Object { $_.FullName.Substring($r.Length).TrimStart('\\').Replace('\\','/') + ' ' + "
        "(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash } }\"",
        f'if not exist "{dep}" mkdir "{dep}"', f"SCP swap-restart.cmd -> {dep}\\swap-restart.cmd", f'"{dep}\\swap-restart.cmd"',
    ], f"win rollback sequence {sh.calls}")
    v2 = h.verify()
    check(v2.ok and v2.binary_sha == {"sessiondock": SHA_OLD} and v2.build == "build-d-old", f"win verify after rollback {v2}")

    sh.calls.clear()
    removed = h.prune_backups(1)
    check(removed == [f"{p}\\backup-deploy-f9c51d2-20260914-195842"]
          and sh.calls == [f'dir /b /ad "{p}\\backup-deploy-*" 2>nul', f'rd /s /q "{p}\\backup-deploy-f9c51d2-20260914-195842"'],
          f"win prune {removed} {sh.calls}")


def test_windows_verify_catches(tmp: Path) -> None:
    def deployed(**kw):
        h = win_handler(tmp, **kw)
        h.probe(); h.stage(); h.backup(); h.swap(); h.restart()
        return h
    h = deployed(session="0")
    v = h.verify()
    check(not v.ok and "runs in session 0, not 1" in v.detail, f"session 0 undetected: {v.detail}")
    h = deployed()
    h.sh.state["ptyhost"] = [7064]
    v = h.verify()
    check(not v.ok and "ptyhost.exe pids lost: [11268]" in v.detail, f"lost ptyhost.exe undetected: {v.detail}")
    h = deployed()
    h.sh.state["files"][f"{WIN_TARGET.prefix}\\bin\\sessiondock.exe"] = "c" * 64
    v = h.verify()
    check(not v.ok and "on-disk sha" in v.detail, f"win sha mismatch undetected: {v.detail}")
    h = deployed(web_differs=False)
    v = h.verify()
    check(v.ok and v.build == "build-d-old", f"identical web must keep the build hash: {v.detail}")
    h = deployed()
    h.sh.state["build"] = "build-d-old"
    v = h.verify()
    check(not v.ok and "web changed but build still" in v.detail, f"win stale build undetected: {v.detail}")
    # web-only: no zip, no build.cmd
    h = win_handler(tmp, opts=DeployOptions(web_only=True, health_timeout=0))
    h.probe(); h.sh.calls.clear(); h.stage()
    check(not any("build.cmd" in c or "source-" in c for c in h.sh.calls), f"win web-only must not build: {h.sh.calls}")
    # unknown PY refuses to swap instead of guessing
    h = win_handler(tmp)
    h.probe(); h.vars.pop("PY")
    try:
        h.swap()
        check(False, "swap without PY must refuse")
    except RuntimeError as e:
        check("service.python" in str(e), f"swap refusal message {e}")
    # build.cmd that compiled but left the exe untouched is rejected
    h = win_handler(tmp)
    h.probe()
    orig = h.sh.run

    def stale(cmd, timeout=60, check=False):
        rc, out = orig(cmd, timeout, check)
        return rc, out.replace("MTIME_AFTER sessiondock 638000000600000000", "MTIME_AFTER sessiondock 638000000000000000")
    h.sh.run = stale
    try:
        h.stage()
        check(False, "stale exe after a compile must be rejected")
    except RuntimeError as e:
        check("kept its mtime" in str(e), f"stale message {e}")


def test_windows_source_dir_guard() -> None:
    bad = Target(**{**WIN_TARGET.__dict__, "extra": {**WIN_TARGET.extra, "source_dir": "C:\\Example\\sessiondock\\src"}})
    try:
        windows.WindowsNode(bad, make_artifacts(Path(tempfile.mkdtemp())), DeployOptions(), lambda s: None).plan()
        check(False, "windows source_dir inside the prefix must be refused")
    except ValueError:
        pass


def test_registry_and_templates() -> None:
    check(handler_for("macos-node") is macos.MacOSNode and handler_for("windows-node") is windows.WindowsNode,
          "handler_for resolves both native kinds")
    for name in ("build.cmd", "backup.cmd", "swap-restart.cmd"):
        raw = (ROOT / "deploy" / "windows" / name).read_bytes()
        check(all(b < 128 for b in raw), f"deploy/windows/{name} must be ASCII")
    check(macos.tar_strip_components is not None, "macos exports tar_strip_components")
    try:
        windows.render("build.cmd", {"SD": "x"})
        check(False, "render must reject unfilled placeholders")
    except ValueError:
        pass
    build = windows.render("build.cmd", {"SD": "C:\\p", "SRC": "C:\\s", "TC": "C:\\tc", "ZIP": "C:\\s\\.deploy\\z.zip",
                                         "BINS": "sessiondock", "PKGS": "-p sessiondock", "TEST": "0"})
    for needle in ('set "RUSTC=%TC%\\rustc.exe"', 'set "RUSTDOC=%TC%\\rustdoc.exe"', '"%TC%\\cargo.exe" build --release --locked -p sessiondock',
                   'if /i not "%%~nxD"=="target"', "Expand-Archive -LiteralPath", "MTIME_", "certutil -hashfile", "BUILD_OK",
                   # the mtime probe carries single quotes, so the for /f command must be backquoted
                   'for /f "usebackq delims=" %%T in (`powershell'):
        check(needle in build, f"build.cmd lacks {needle!r}")


def main() -> int:
    t0 = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="sd-native-handlers-") as d:
        tmp = Path(d)
        for fn in (test_registry_and_templates, test_windows_source_dir_guard, lambda: test_macos_full_cycle(tmp), lambda: test_macos_web_only_and_prefix(tmp),
                   lambda: test_macos_verify_catches(tmp), lambda: test_macos_test_step(tmp), lambda: test_windows_full_cycle(tmp),
                   lambda: test_windows_verify_catches(tmp), lambda: test_windows_test_step(tmp)):
            fn()
    print(f"deploy_native_handlers: {'FAILED ' + str(len(FAILURES)) if FAILURES else 'ok'} in {time.monotonic() - t0:.2f}s")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
