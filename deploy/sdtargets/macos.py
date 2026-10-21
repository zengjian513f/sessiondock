"""macOS node handler: native `cargo build` on the node, launchd LaunchAgent restart.

Automates docs/deploy-macos.md sections 2, 4 and 5. The node builds from the
`git archive` tar that `deploy.py build` produced (`artifacts.source_archive`),
inside `target.extra["source_dir"]` (kept between runs so `target/` stays warm),
first runs the workspace Rust compile checks there when the stage's test mode is not `none`
(`TMPDIR=/private/tmp/sdtest`, section 2; a failure stops the target before anything
is staged), then copies the fresh binaries next to the running ones as `<name>.new`, renames them
over in `swap()` and restarts only the web service with
`launchctl kickstart -k gui/<gui_uid>/<launchd_label>`; detached ptyhost hosts
are never touched and `verify()` proves every probe-time ptyhost pid survived.

Every remote command is glob-free (`find -name` instead of shell wildcards)
because the node's login shell is zsh, which aborts on an unmatched glob.
"""
from __future__ import annotations

import shlex
import tarfile
import time
from datetime import datetime, timezone
from pathlib import Path

from .base import ProbeResult, ShellError, TargetHandler, VerifyResult, failed_probe, register

BUILD_TIMEOUT = 900.0
TEST_TIMEOUT = 1800.0
TEST_TMPDIR = "/private/tmp/sdtest"     # docs/deploy-macos.md section 2: short, real TMPDIR
TEST_LOG = ".deploy-test.log"
WEB_STAGING = "web.staging"


def _q(s: str) -> str:
    return shlex.quote(s)


def _backup_stamp(name: str) -> str:
    """Sort key: the UTC stamp at the end of `backup-deploy-<short>-<stamp>`."""
    return name.rsplit("-", 2)[-2] + name.rsplit("-", 2)[-1] if name.count("-") >= 3 else name


def _pids(out: str) -> list[int]:
    return sorted(int(tok) for tok in out.split() if tok.isdigit())


def tar_strip_components(archive: Path) -> int:
    """0 when `Cargo.toml` is at the tar root, 1 when a single prefix directory wraps it."""
    with tarfile.open(archive) as tf:
        names = [m.name for m in tf.getmembers() if m.name != "pax_global_header"]
    if "Cargo.toml" in names:
        return 0
    tops = {n.split("/", 1)[0] for n in names}
    if len(tops) == 1 and f"{next(iter(tops))}/Cargo.toml" in names:
        return 1
    raise ValueError(f"{archive}: no Cargo.toml at the root or under one prefix directory")


@register
class MacOSNode(TargetHandler):
    kind = "macos-node"

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.expected_versions: dict[str, str] = {}   # release dates of staged binaries
        self.staged: list[str] = []              # binaries copied as `.new` this run
        self.restarted = False

    # -- config -----------------------------------------------------------------
    @property
    def prefix(self) -> str:
        return self.t.prefix.rstrip("/")

    @property
    def label(self) -> str:
        return self.t.service["launchd_label"]

    @property
    def domain(self) -> str:
        return f"gui/{int(self.t.service.get('gui_uid', 501))}/{self.label}"

    @property
    def source_dir(self) -> str:
        d = str(self.t.extra["source_dir"]).rstrip("/")
        if not d.startswith("/") or d.count("/") < 2 or d == self.prefix \
                or self.prefix.startswith(d + "/") or d.startswith(self.prefix + "/"):
            raise ValueError(f"refusing source_dir {d!r} (must be an absolute dir outside the prefix)")
        return d

    @property
    def cargo(self) -> str:
        return str(self.t.extra.get("cargo", "~/.cargo/bin/cargo"))

    @property
    def ship_bin(self) -> bool:
        return not (self.o.web_only or self.a.web_only)

    @property
    def ship_web(self) -> bool:
        return not self.o.bin_only

    @property
    def bins(self) -> list[str]:
        names = list(self.t.binaries)
        if self.o.with_ptyhost and "ptyhost" not in names:
            names.append("ptyhost")
        if self.o.with_sdkhost and "sdkhost" not in names:
            names.append("sdkhost")
        return names

    @property
    def run_tests(self) -> bool:
        return self.ship_bin and getattr(self.o, "test_mode", "none") != "none"

    def test_cmd(self) -> str:
        """Section 2 of docs/deploy-macos.md; the log stays on the node when tests fail."""
        packages = " ".join(f"-p {name}" for name in self.bins)
        return (f"cd {_q(self.source_dir)} && mkdir -p {TEST_TMPDIR} && TMPDIR={TEST_TMPDIR} {self.cargo} check "
                f"{packages} --all-targets --locked >{TEST_LOG} 2>&1; rc=$?; tail -n 40 {TEST_LOG}; "
                f"[ $rc -eq 0 ] && rm -f {TEST_LOG}; exit $rc")

    # -- remote helpers -----------------------------------------------------------
    def _version(self, path: str) -> str | None:
        rc, out = self.sh.run(f"cat {_q(path + '.version')} 2>/dev/null", timeout=20)
        return out.strip() or None if rc == 0 else None

    def _service(self) -> tuple[bool, int | None]:
        rc, out = self.sh.run(f"launchctl print {self.domain} 2>/dev/null | "
                              f"grep -E '^[[:space:]]*(state|pid) =' || true", timeout=20)
        state = pid = None
        for line in out.splitlines():
            k, _, v = line.strip().partition("=")
            if k.strip() == "state":
                state = v.strip()
            elif k.strip() == "pid" and v.strip().isdigit():
                pid = int(v.strip())
        return state == "running", pid

    def _ptyhost_pids(self) -> list[int]:
        _, out = self.sh.run("pgrep -x ptyhost || true", timeout=20)
        return _pids(out)

    def _host_records(self) -> int:
        _, out = self.sh.run(f"ls {_q(self.prefix + '/host')} 2>/dev/null | grep -c '\\.json$' || true",
                             timeout=20)
        return int(out.split()[0]) if out.split() and out.split()[0].isdigit() else 0

    # -- steps ----------------------------------------------------------------------
    def probe(self) -> ProbeResult:
        try:
            rc, out = self.sh.run("uname -s", timeout=20)
        except ShellError as e:
            return failed_probe(self.t, e.rc, e.out or str(e))
        if rc != 0:
            return failed_probe(self.t, rc, out.strip() or f"probe failed: rc={rc}")
        if rc != 0 or "Darwin" not in out:
            return ProbeResult(reachable=False, detail=f"not a macOS host: rc={rc} {out.strip()[:120]}")
        active, pid = self._service()
        meta = self.sh.http_json(self.t.health_url)
        res = ProbeResult(reachable=True, active=active, build=meta.get("build") if meta else None)
        res.detail = f"launchd pid={pid}"
        self.service_pid_before = pid
        for name in self.bins:
            version = self._version(f"{self.prefix}/bin/{name}")
            if version:
                res.binary_versions[name] = version
        res.ptyhost_pids = self._ptyhost_pids()
        res.host_records = self._host_records()
        _, out = self.sh.run(f"cat {_q(self.prefix + '/etc/deployed-commit')} 2>/dev/null || true", timeout=20)
        res.deployed_commit = out.strip() or None
        self.before = res
        return res

    def plan(self) -> list[str]:
        p = self.prefix
        steps: list[str] = []
        if self.ship_bin:
            src = self.source_dir
            pk = " ".join(f"-p {n}" for n in self.bins)
            steps.append(f"rsync {self.a.source_archive} -> {src}/.deploy/source.tar; wipe {src}/* except target/ ; tar -x")
            if self.run_tests:
                steps.append(f"test ({self.o.test_mode}): cd {src} && TMPDIR={TEST_TMPDIR} {self.cargo} check {pk} "
                             f"--all-targets --locked   (timeout {int(TEST_TIMEOUT)}s; failure = FAILED before anything is staged)")
            steps += [
                f"cd {src} && {self.cargo} build --release --locked {pk}   (timeout {int(BUILD_TIMEOUT)}s)",
            ] + [f"cp -f {src}/target/release/{n} {p}/bin/{n}.new and write release date marker"
                 for n in self.bins]
        if self.ship_web:
            steps.append(f"rsync -a --ignore-times --delete {self.a.web_dir}/ -> {p}/{WEB_STAGING}/ (release date version)")
        steps.append(f"backup: cp -Rp {p}/bin {p}/web -> {p}/backup-deploy-{self.a.short}-<UTC stamp>/")
        if self.ship_bin:
            steps += [f"swap: mv -f {p}/bin/{n}.new {p}/bin/{n}" for n in self.bins]
        if self.ship_web:
            steps.append(f"swap: rsync -a --ignore-times --delete {p}/{WEB_STAGING}/ {p}/web/ && rm -rf {p}/{WEB_STAGING}")
        steps += [
            f"restart: launchctl kickstart -k {self.domain}   (ptyhost hosts untouched)",
            f"verify: {self.t.health_url} within {self.o.health_timeout:.0f}s, state running, new launchd pid, "
            "release versions match staged date, ptyhost pids kept, host records >= before",
            f"marker: {p}/etc/deployed-commit = '{self.a.commit} {self.a.short} {self.a.built_at} dirty={int(self.a.dirty)}'",
        ]
        return steps

    def stage(self) -> None:
        p = self.prefix
        if self.ship_bin:
            self.check_sdkhost_dependency(self.bins)
            if not self.a.source_archive:
                raise RuntimeError("macos-node needs artifacts.source_archive (git archive tar)")
            src = self.source_dir
            strip = tar_strip_components(Path(self.a.source_archive))
            self.sh.run(f"mkdir -p {_q(src + '/.deploy')}", timeout=20, check=True)
            self.sh.upload(Path(self.a.source_archive), f"{src}/.deploy/source.tar")
            self.sh.run(f"cd {_q(src)} && find . -mindepth 1 -maxdepth 1 ! -name target ! -name .deploy "
                        f"-exec rm -rf {{}} + && tar -xf .deploy/source.tar --strip-components={strip} "
                        f"&& rm -rf .deploy && test -f Cargo.toml", timeout=120, check=True)
            if self.run_tests:
                t0 = time.monotonic()
                rc, out = self.sh.run(self.test_cmd(), timeout=TEST_TIMEOUT)
                if rc != 0:
                    raise RuntimeError(f"cargo check failed on the node (rc={rc}); log {src}/{TEST_LOG} on the node, "
                                       f"tail in {self.o.log_dir / (self.t.name + '.log')}:\n{out[-1500:]}")
                self.log(f"tests ({self.o.test_mode}) passed in {time.monotonic() - t0:.0f}s")
            pk = " ".join(f"-p {n}" for n in self.bins)
            t0 = time.monotonic()
            rc, out = self.sh.run(
                f"cd {_q(src)} && {self.cargo} build --release --locked {pk} >.deploy-build.log 2>&1; rc=$?; "
                f"tail -n 25 .deploy-build.log; rm -f .deploy-build.log; exit $rc", timeout=BUILD_TIMEOUT)
            if rc != 0 or "Finished" not in out:
                raise ShellError("cargo build", rc, out)
            self.log(f"build finished in {time.monotonic() - t0:.0f}s")
            for name in self.bins:
                built = f"{src}/target/release/{name}"
                self.sh.run(f"cp -f {_q(built)} {_q(f'{p}/bin/{name}.new')}", timeout=60, check=True)
                self.sh.run(f"printf '%s\\n' {_q(self.a.version)} > {_q(f'{p}/bin/{name}.new.version')}", check=True)
                self.expected_versions[name] = self.a.version
                self.staged.append(name)
                self.log(f"staged {name}.new version={self.a.version}")
        if self.ship_web:
            staging = f"{p}/{WEB_STAGING}"
            self.sh.run(f"mkdir -p {_q(staging)}", timeout=20, check=True)
            # a trailing slash selects directory-content semantics, so this must stay a str
            self.sh.upload(str(self.a.web_dir).rstrip("/") + "/", staging + "/", delete=True)

    def discard_staging(self) -> None:
        """Remove bin/<name>.new{,.version} and web.staging/; source_dir stays as the warm build tree."""
        p = self.prefix
        news = " ".join(_q(f"{p}/bin/{name}{suffix}") for name in self.bins for suffix in (".new", ".new.version"))
        self.sh.run(f"rm -rf {_q(p + '/' + WEB_STAGING)}" + (f" && rm -f {news}" if news else ""), timeout=60, check=True)
        self.staged, self.expected_versions = [], {}
        self.log("discarded staged bin/*.new and web.staging/")

    def backup(self) -> str:
        stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
        d = f"{self.prefix}/backup-deploy-{self.a.short}-{stamp}"
        # bin/ is copied file by file so the `.new` files staged a moment ago stay out of the backup
        self.sh.run(f"mkdir -p {_q(d + '/bin')} && cd {_q(self.prefix + '/bin')} && "
                    f"find . -maxdepth 1 -type f ! -name '*.new' ! -name '*.new.version' -exec cp -p {{}} {_q(d + '/bin/')} \\; && "
                    f"cp -Rp {_q(self.prefix + '/web')} {_q(d + '/web')}", timeout=120, check=True)
        self.backup_marker(d)
        self.backup_dir = d
        return d

    def swap(self) -> None:
        p = self.prefix
        for name in self.staged:
            self.sh.run(f"mv -f {_q(f'{p}/bin/{name}.new')} {_q(f'{p}/bin/{name}')}", timeout=20, check=True)
            self.sh.run(f"mv -f {_q(f'{p}/bin/{name}.new.version')} {_q(f'{p}/bin/{name}.version')}", check=True)
        if self.ship_web:
            self.sh.run(f"rsync -a --ignore-times --delete {_q(p + '/' + WEB_STAGING + '/')} {_q(p + '/web/')} && "
                        f"rm -rf {_q(p + '/' + WEB_STAGING)}", timeout=120, check=True)

    def restart(self) -> None:
        self.sh.run(f"launchctl kickstart -k {self.domain}", timeout=30, check=True)
        self.restarted = True

    def verify(self) -> VerifyResult:
        t0 = time.monotonic()
        before = self.before or ProbeResult(reachable=True)
        old_pid = getattr(self, "service_pid_before", None)
        deadline = t0 + self.o.health_timeout
        meta, active, pid = None, False, None
        while True:
            active, pid = self._service()
            meta = self.sh.http_json(self.t.health_url) if active else None
            if meta and active and (not self.restarted or pid != old_pid):
                break
            if time.monotonic() >= deadline:
                break
            time.sleep(1.0)
        res = VerifyResult(ok=True, build=meta.get("build") if meta else None, active=active)
        problems: list[str] = []
        if not active:
            problems.append("launchd state is not running")
        if meta is None:
            problems.append(f"{self.t.health_url} did not answer within {self.o.health_timeout:.0f}s")
        if self.restarted and old_pid is not None and pid == old_pid:
            problems.append(f"launchd pid {pid} unchanged: no restart happened")
        expected = dict(before.binary_versions)
        expected.update(self.expected_versions)
        for name in self.bins:
            version = self._version(f"{self.prefix}/bin/{name}")
            if version:
                res.binary_versions[name] = version
            if name in expected and version != expected[name]:
                problems.append(f"{name}: version {version} != expected {expected[name]}")
        if self.ship_web and res.build != self.a.version:
            problems.append(f"served build {res.build} != release {self.a.version}")
        res.ptyhost_pids = self._ptyhost_pids()
        lost = sorted(set(before.ptyhost_pids) - set(res.ptyhost_pids))
        if lost:
            problems.append(f"ptyhost pids lost: {lost}")
        res.host_records = self._host_records()
        if before.host_records is not None and res.host_records < before.host_records:
            problems.append(f"host records {before.host_records} -> {res.host_records}")
        res.ok = not problems
        res.detail = "; ".join(problems) if problems else (
            f"pid {old_pid}->{pid}, ptyhost {before.ptyhost_pids}->{res.ptyhost_pids}, "
            f"build {before.build}->{res.build}")
        res.elapsed = time.monotonic() - t0
        return res

    def rollback(self, backup_dir: str) -> None:
        p = self.prefix
        self.expected_versions, self.staged = {}, []
        for name in self.bins:
            src = f"{backup_dir}/bin/{name}"
            rc, _ = self.sh.run(f"test -f {_q(src)}", timeout=20)
            if rc != 0:
                continue
            self.sh.run(f"cp -f {_q(src)} {_q(f'{p}/bin/{name}.new')} && "
                        f"mv -f {_q(f'{p}/bin/{name}.new')} {_q(f'{p}/bin/{name}')}", timeout=60, check=True)
            self.sh.run(f"rm -f {_q(f'{p}/bin/{name}.version')}; "
                        f"if [ -f {_q(src + '.version')} ]; then cp -p {_q(src + '.version')} {_q(f'{p}/bin/{name}.version')}; fi", check=True)
        self.sh.run(f"test -d {_q(backup_dir + '/web')} && rsync -a --ignore-times --delete {_q(backup_dir + '/web/')} "
                    f"{_q(p + '/web/')}", timeout=120, check=True)
        self.sh.run(f"rm -rf {_q(p + '/' + WEB_STAGING)}", timeout=20)
        self.restore_marker(backup_dir)
        self.restart()

    def write_marker(self) -> None:
        line = f"{self.a.commit} {self.a.short} {self.a.built_at} dirty={int(self.a.dirty)}"
        self.sh.run(f"printf '%s\\n' {_q(line)} > {_q(self.prefix + '/etc/deployed-commit')}",
                    timeout=20, check=True)

    def prune_backups(self, keep: int) -> list[str]:
        _, out = self.sh.run(f"cd {_q(self.prefix)} && find . -mindepth 1 -maxdepth 1 -type d "
                             f"-name 'backup-deploy-*' | sed 's#^\\./##'", timeout=30)
        names = sorted((n.strip() for n in out.splitlines() if n.strip()), key=_backup_stamp)
        doomed = names[:-keep] if keep > 0 else names
        for n in doomed:
            self.sh.run(f"rm -rf {_q(self.prefix + '/' + n)}", timeout=120, check=True)
        return [f"{self.prefix}/{n}" for n in doomed]
