"""Shared contract for deploy/deploy.py and the per-kind handlers.

Only the standard library. Nothing here knows a real address: targets come from
`deploy/targets.local.json` (gitignored); `deploy/targets.example.json` shows the shape.

Deployment order per target (deploy.py drives it, handlers implement the steps):

    probe -> stage -> backup -> swap -> restart -> verify -> (rollback on failure)

(native-build kinds inside stage(): extract source -> Rust tests unless
DeployOptions.test_mode == "none" -> cargo build -> copy as `.new`)

Invariants every handler must keep:
- Never delete or overwrite a running binary in place: upload as `<name>.new`, then
  rename over the old one (`mv -f` / `move /y`), because a running binary is "Text
  file busy" on Linux and locked on Windows.
- Never touch ptyhost session hosts: only the web service restarts. `probe()` records
  the ptyhost pids (or host records) and `verify()` proves the same set is still alive.
- Never touch private data or config under the prefix (etc/, state/, delivery/,
  lifecycle/, host/, audit/, trash/, search-cache/, hub/): only bin/ and web/ change.
- Backups live at `<prefix>/backup-deploy-<short commit>-<UTC stamp>/{bin,web}`.
- `verify()` must show the service active, `/api/meta` answering on the target's own
  loopback, the on-disk binary SHA-256 equal to the staged artifact (native builds:
  equal to the freshly built file), and the `build` field changed whenever web changed.
- A dry run performs `probe()` only and prints the plan; nothing is uploaded.
"""
from __future__ import annotations

import json
import shlex
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

SSH_BASE_OPTS = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=8",
                 "-o", "ServerAliveInterval=10", "-o", "ServerAliveCountMax=3"]


@dataclass
class Target:
    """One deployable machine. Fields mirror deploy/targets.example.json."""
    name: str                       # "lyra", "hub", ...
    kind: str                       # key into HANDLERS
    prefix: str                     # runtime directory holding bin/ web/ etc/
    ssh: str | None = None          # "user@host" or None = this machine
    ssh_port: int | None = None     # None = ssh config / default
    ssh_opts: list[str] = field(default_factory=list)
    health_url: str = "http://127.0.0.1:8741/api/meta"   # fetched ON the target
    service: dict = field(default_factory=dict)          # kind-specific
    binaries: list[str] = field(default_factory=lambda: ["sessiondock"])
    build_on_target: bool = False   # macOS/Windows build natively from a source archive
    extra: dict = field(default_factory=dict)
    enabled: bool = True


@dataclass
class Artifacts:
    """What `deploy.py build` produced on the build machine."""
    commit: str                     # full sha
    short: str                      # 7-12 chars, used in backup dir names
    dirty: bool                     # working tree differed from HEAD in crates/ or legacy-web/
    built_at: str                   # UTC ISO
    web_dir: Path                   # snapshot of legacy-web/ to rsync (trailing-slash semantics)
    binaries: dict[str, Path]       # name -> release file on the build machine (Linux glibc)
    sha256: dict[str, str]          # name -> hex digest of binaries[name]
    source_archive: Path | None     # `git archive HEAD` tar for build_on_target kinds
    web_only: bool = False
    source_zip: Path | None = None  # optional `git archive --format=zip` of the same commit
                                    # (windows-node; derived from source_archive when None)


@dataclass
class DeployOptions:
    dry_run: bool = False
    web_only: bool = False
    bin_only: bool = False
    with_ptyhost: bool = False
    keep_backups: int = 5
    health_timeout: float = 45.0
    log_dir: Path = Path("target/deploy")
    # Test mode the stage was built with (deploy.py --test): tests run once per PLATFORM.
    # Linux nodes and the hub receive the binary already tested on the build machine;
    # kinds that build natively (macos-node, windows-node) run the Rust tests inside
    # stage() after extracting the source and before building, unless "none".
    test_mode: str = "none"


@dataclass
class ProbeResult:
    reachable: bool
    detail: str = ""
    build: str | None = None        # /api/meta "build" before deploy
    active: bool | None = None      # service state before deploy
    binary_sha: dict[str, str] = field(default_factory=dict)
    ptyhost_pids: list[int] = field(default_factory=list)
    host_records: int | None = None
    deployed_commit: str | None = None   # <prefix>/etc/deployed-commit if present


@dataclass
class VerifyResult:
    ok: bool
    detail: str = ""
    build: str | None = None
    active: bool | None = None
    binary_sha: dict[str, str] = field(default_factory=dict)
    ptyhost_pids: list[int] = field(default_factory=list)
    host_records: int | None = None
    elapsed: float = 0.0


class ShellError(RuntimeError):
    def __init__(self, cmd: str, rc: int, out: str):
        super().__init__(f"rc={rc}: {cmd}\n{out[-2000:]}")
        self.cmd, self.rc, self.out = cmd, rc, out


class Shell:
    """Run commands on a target: locally when `target.ssh` is None, else over ssh.

    `run` returns (rc, combined output) and raises ShellError only with check=True.
    `upload` uses rsync over the same ssh settings (never pass `-e ssh -p` for an
    alias whose port lives in ~/.ssh/config: only when ssh_port is set explicitly).
    Every call has a timeout; nothing here is interactive.
    """

    def __init__(self, target: Target, log: Callable[[str], None] | None = None):
        self.t = target
        self.log = log or (lambda s: None)
        # Remote POSIX commands run as `sh -c <cmd>`: the login shell on some nodes is
        # zsh, whose `nomatch` aborts on an unmatched glob. Windows targets (cmd.exe
        # strings) get the raw command; a handler may also pass `posix=` per call.
        self.posix = not target.kind.startswith("windows")

    # -- ssh plumbing -----------------------------------------------------------
    def ssh_argv(self) -> list[str]:
        argv = ["ssh", *SSH_BASE_OPTS, *self.t.ssh_opts]
        if self.t.ssh_port:
            argv += ["-p", str(self.t.ssh_port)]
        return argv + [self.t.ssh]

    def rsync_rsh(self) -> str:
        parts = ["ssh", *SSH_BASE_OPTS, *self.t.ssh_opts]
        if self.t.ssh_port:
            parts += ["-p", str(self.t.ssh_port)]
        return " ".join(shlex.quote(p) for p in parts)

    # -- primitives ---------------------------------------------------------------
    def run(self, cmd: str, timeout: float = 60, check: bool = False,
            posix: bool | None = None) -> tuple[int, str]:
        """`cmd` is a POSIX shell string (Windows handlers build cmd.exe strings).

        `posix=None` follows `self.posix`; True wraps a remote command in `sh -c`
        so the target's login shell (bash, zsh, ...) never interprets it."""
        posix = self.posix if posix is None else posix
        if self.t.ssh is None:
            argv = ["bash", "-c", cmd]
        elif posix:
            argv = self.ssh_argv() + ["sh -c " + shlex.quote(cmd)]
        else:
            argv = self.ssh_argv() + [cmd]
        self.log(f"$ {cmd if len(cmd) < 300 else cmd[:300] + ' …'}")
        try:
            p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired as e:
            out = (e.stdout or "") + (e.stderr or "") if isinstance(e.stdout, str) else ""
            raise ShellError(cmd, 124, f"timeout after {timeout}s\n{out}")
        out = p.stdout + p.stderr
        if check and p.returncode != 0:
            raise ShellError(cmd, p.returncode, out)
        return p.returncode, out

    def upload(self, local: Path, remote: str, *, delete: bool = False,
               timeout: float = 300) -> None:
        """rsync `local` to `remote` (local path when target.ssh is None).
        Pass a trailing slash on `local` for directory-content semantics."""
        argv = ["rsync", "-a", "--no-owner", "--no-group"]
        if delete:
            argv.append("--delete")
        if self.t.ssh is not None:
            argv += ["-e", self.rsync_rsh()]
            dest = f"{self.t.ssh}:{remote}"
        else:
            dest = remote
        argv += [str(local), dest]
        self.log("$ " + " ".join(shlex.quote(a) for a in argv))
        p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
        if p.returncode != 0:
            raise ShellError(" ".join(argv), p.returncode, p.stdout + p.stderr)

    def http_json(self, url: str, timeout: float = 10) -> dict | None:
        """GET `url` from the target itself (loopback health) and parse JSON."""
        rc, out = self.run(f"curl -sS --max-time {int(timeout)} {shlex.quote(url)}",
                           timeout=timeout + 5)
        if rc != 0:
            return None
        try:
            return json.loads(out)
        except ValueError:
            return None

    def wait_http_json(self, url: str, deadline: float) -> dict | None:
        end = time.monotonic() + deadline
        while True:
            doc = self.http_json(url)
            if doc is not None:
                return doc
            if time.monotonic() >= end:
                return None
            time.sleep(1.0)


class TargetHandler:
    """Abstract per-kind deployer. Subclasses set `kind` and implement the steps.

    `self.sh` is the Shell for the target; `self.log` writes one line to the
    per-target log. Every step must be safe to run once; deploy.py never retries a
    step, it rolls back instead.
    """
    kind = ""

    def __init__(self, target: Target, artifacts: Artifacts, opts: DeployOptions,
                 log: Callable[[str], None]):
        self.t, self.a, self.o, self.log = target, artifacts, opts, log
        self.sh = Shell(target, log)
        self.backup_dir: str | None = None
        self.before: ProbeResult | None = None

    # steps ---------------------------------------------------------------------
    def probe(self) -> ProbeResult:
        raise NotImplementedError

    def plan(self) -> list[str]:
        """Human-readable list of what stage/swap/restart would do (for --dry-run)."""
        raise NotImplementedError

    def stage(self) -> None:
        """Upload binaries as `.new` and web to a staging dir; build natively if needed."""
        raise NotImplementedError

    def backup(self) -> str:
        """Copy current bin/ and web/ into the backup dir; return its path."""
        raise NotImplementedError

    def swap(self) -> None:
        """Rename staged binaries into place and sync web/ (the only mutations)."""
        raise NotImplementedError

    def restart(self) -> None:
        raise NotImplementedError

    def verify(self) -> VerifyResult:
        raise NotImplementedError

    def rollback(self, backup_dir: str) -> None:
        """Restore bin/ and web/ from `backup_dir`, restart, and leave verify to the caller."""
        raise NotImplementedError

    def write_marker(self) -> None:
        """Record `<prefix>/etc/deployed-commit`: '<full sha> <short> <built_at> dirty=<0|1>'."""
        raise NotImplementedError

    def prune_backups(self, keep: int) -> list[str]:
        """Delete all but the newest `keep` `backup-deploy-*` dirs; return what was removed."""
        raise NotImplementedError

    def list_backups(self) -> list[str]:
        """Existing `backup-deploy-*` dirs under the prefix, newest first (absolute paths).
        `deploy.py rollback` uses the first one when no `--backup` is given; a kind that
        cannot list them returns [] and then requires `--backup`."""
        return []


HANDLERS: dict[str, type[TargetHandler]] = {}


def register(cls: type[TargetHandler]) -> type[TargetHandler]:
    HANDLERS[cls.kind] = cls
    return cls


def handler_for(kind: str) -> type[TargetHandler]:
    # Import lazily so a broken optional module (e.g. windows.py) never blocks Linux.
    if kind not in HANDLERS:
        import importlib
        mod = {"linux-node": "linux", "hub": "hub", "macos-node": "macos",
               "windows-node": "windows"}.get(kind)
        if mod is None:
            raise KeyError(f"unknown target kind {kind!r}")
        importlib.import_module(f"{__package__}.{mod}")
    return HANDLERS[kind]


def load_targets(path: Path) -> list[Target]:
    doc = json.loads(Path(path).read_text())
    out = []
    for raw in doc["targets"]:
        raw = dict(raw)
        out.append(Target(**raw))
    return out
