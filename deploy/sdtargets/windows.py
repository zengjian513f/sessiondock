r"""Windows node handler: native `cargo build` on the node, relaunch in desktop session 1.

Automates docs/deploy-windows.md sections 2-4 plus the operator recipe around the node's
`restart-session1.cmd`. Everything that runs on the node is a cmd.exe string: `CmdShell`
passes it to ssh verbatim (no POSIX wrapping) and decodes code-page output leniently.
Multi-step work is uploaded as ASCII `.cmd` files rendered from `deploy/windows/*.cmd`
(inside one `a & echo %ERRORLEVEL%` line cmd.exe expands ERRORLEVEL at parse time, so
only a script yields real exit codes); uploads use scp because rsync is unavailable.

`stage()` uploads a zip of the commit, extracts it into `extra["source_dir"]` (keeping
`target\`), builds with the toolchain's REAL `cargo.exe` (`extra["toolchain_bin"]`; the
rustup shims are reparse points an elevated SSH cannot run), stages `bin\<name>.new.exe`
and `web.staging\`. `swap()` runs ONE script mirroring `restart-session1.cmd` with the
swap inserted after the stop phase (the running exe is locked; ptyhost.exe hosts are never
touched), so the restart happens there and `restart()` is a no-op afterwards. `verify()`
polls `/api/meta`, compares certutil hashes, checks the ptyhost.exe pid set survived and
that the service landed in session 1. Optional `service` keys `python` / `shortcut`
default to the `set PY=` / `set LNK=` lines of `service["restart_cmd"]` read at probe time.

As of 2026-09-15 this path was written from the recipe and unit-tested offline only; the
Windows node was unreachable, so it has not run on a real machine yet.
"""
from __future__ import annotations

import hashlib
import json
import re
import subprocess
import tarfile
import tempfile
import time
import zipfile
from datetime import datetime, timezone
from pathlib import Path

from .base import (SSH_BASE_OPTS, ProbeResult, Shell, ShellError, TargetHandler,
                   VerifyResult, register)

TEMPLATES = Path(__file__).resolve().parent.parent / "windows"
BUILD_TIMEOUT = 960.0
SWAP_TIMEOUT = 240.0
WEB_STAGING = "web.staging"


def _scp_path(win: str) -> str:
    return win.replace("\\", "/")


def _backup_stamp(name: str) -> str:
    parts = name.rsplit("-", 2)
    return parts[-2] + parts[-1] if len(parts) == 3 else name


def render(template: str, subs: dict[str, str]) -> str:
    """Fill `@KEY@` placeholders; the result must stay ASCII and gets CRLF line ends."""
    text = (TEMPLATES / template).read_text(encoding="ascii")
    for key, value in subs.items():
        text = text.replace(f"@{key}@", value)
    if re.search(r"@[A-Z_]+@", text):
        raise ValueError(f"{template}: unfilled placeholder")
    text.encode("ascii")
    return "\r\n".join(text.splitlines()) + "\r\n"


class CmdShell(Shell):
    """Shell for a Windows OpenSSH target: cmd.exe strings, scp uploads, curl.exe health."""

    def run(self, cmd: str, timeout: float = 60, check: bool = False,
            posix: bool | None = None) -> tuple[int, str]:
        """`posix` is accepted for signature parity and ignored: cmd.exe strings go raw."""
        argv = self.ssh_argv() + [cmd]
        self.log(f"> {cmd if len(cmd) < 300 else cmd[:300] + ' ...'}")
        try:
            p = subprocess.run(argv, capture_output=True, text=True, errors="replace", timeout=timeout)
        except subprocess.TimeoutExpired as e:
            out = e.stdout if isinstance(e.stdout, str) else ""
            raise ShellError(cmd, 124, f"timeout after {timeout}s\n{out}")
        out = (p.stdout + p.stderr).replace("\r\n", "\n")
        if check and p.returncode != 0:
            raise ShellError(cmd, p.returncode, out)
        return p.returncode, out

    def scp_upload(self, local: Path, remote: str, *, timeout: float = 300) -> None:
        argv = ["scp", "-q", *SSH_BASE_OPTS, *self.t.ssh_opts]
        if self.t.ssh_port:
            argv += ["-P", str(self.t.ssh_port)]
        argv += [str(local), f"{self.t.ssh}:{_scp_path(remote)}"]
        self.log("$ " + " ".join(argv))
        p = subprocess.run(argv, capture_output=True, text=True, errors="replace", timeout=timeout)
        if p.returncode != 0:
            raise ShellError(" ".join(argv), p.returncode, p.stdout + p.stderr)

    def http_json(self, url: str, timeout: float = 10) -> dict | None:
        rc, out = self.run(f'curl.exe -sS --max-time {int(timeout)} "{url}"', timeout=timeout + 5)
        if rc != 0:
            return None
        try:
            return json.loads(out)
        except ValueError:
            return None


@register
class WindowsNode(TargetHandler):
    kind = "windows-node"

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.sh = CmdShell(self.t, self.log)
        self.tmp = tempfile.TemporaryDirectory(prefix="sd-deploy-win-")
        self.expected_sha: dict[str, str] = {}
        self.staged: list[str] = []
        self.web_digest_before: str | None = None
        self.web_changed = False
        self.restarted = False
        self.service_pids_before: list[int] = []
        self.vars: dict[str, str] = {k: str(v) for k, v in
                                     (("PY", self.t.service.get("python")),
                                      ("LNK", self.t.service.get("shortcut"))) if v}

    # -- config -----------------------------------------------------------------
    @property
    def prefix(self) -> str:
        return self.t.prefix.replace("/", "\\").rstrip("\\")

    @property
    def source_dir(self) -> str:
        d = str(self.t.extra["source_dir"]).replace("/", "\\").rstrip("\\")
        if len(d) < 4 or d[1] != ":" or d.lower() == self.prefix.lower() \
                or self.prefix.lower().startswith(d.lower() + "\\") \
                or d.lower().startswith(self.prefix.lower() + "\\"):
            raise ValueError(f"refusing source_dir {d!r} (absolute path outside the prefix)")
        return d

    @property
    def deploy_dir(self) -> str:
        return self.source_dir + "\\.deploy"

    @property
    def toolchain_bin(self) -> str:
        return str(self.t.extra["toolchain_bin"]).rstrip("\\")

    @property
    def restart_cmd(self) -> str:
        return str(self.t.service["restart_cmd"])

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
        return names

    # -- remote helpers -----------------------------------------------------------
    def _tasklist(self, image: str) -> list[int]:
        _, out = self.sh.run(f'tasklist /FI "IMAGENAME eq {image}" /FO CSV /NH', timeout=30)
        pids = []
        for line in out.splitlines():
            cells = [c.strip('"') for c in line.strip().split('","')]
            if len(cells) >= 2 and cells[0].lower() == image.lower() and cells[1].isdigit():
                pids.append(int(cells[1]))
        return sorted(pids)

    def _sha(self, path: str) -> str | None:
        rc, out = self.sh.run(f'certutil -hashfile "{path}" SHA256', timeout=60)
        if rc != 0:
            return None
        for line in out.splitlines():
            h = line.replace(" ", "").strip().lower()
            if len(h) == 64 and all(c in "0123456789abcdef" for c in h):
                return h
        return None

    def _web_digest(self, d: str) -> str | None:
        ps = (f"$r='{d}'; if (Test-Path -LiteralPath $r) {{ Get-ChildItem -LiteralPath $r -Recurse -File | "
              "ForEach-Object { $_.FullName.Substring($r.Length).TrimStart('\\').Replace('\\','/') + ' ' + "
              "(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash } }")
        rc, out = self.sh.run(f'powershell -NoProfile -Command "{ps}"', timeout=120)
        lines = sorted(l.strip() for l in out.splitlines() if l.strip())
        if rc != 0 or not lines:
            return None
        return hashlib.sha256("\n".join(lines).encode()).hexdigest()[:16]

    def _host_records(self) -> int:
        _, out = self.sh.run(f'dir /b "{self.prefix}\\host\\*.json" 2>nul | find /c /v ""', timeout=30)
        tok = out.split()
        return int(tok[-1]) if tok and tok[-1].isdigit() else 0

    def _load_vars(self) -> None:
        """Read PY/LNK from the node's restart-session1.cmd unless configured."""
        if "PY" in self.vars and "LNK" in self.vars:
            return
        rc, out = self.sh.run(f'findstr /b /c:"set PY=" /c:"set LNK=" "{self.restart_cmd}"', timeout=30)
        for line in out.splitlines():
            k, _, v = line.strip()[4:].partition("=")
            if k in ("PY", "LNK") and v and k not in self.vars:
                self.vars[k] = v.replace("%SD%", self.prefix)
        if "PY" not in self.vars:
            self.log(f"warning: no `set PY=` in {self.restart_cmd} (rc={rc}); swap() will refuse")
        self.vars.setdefault("LNK", f"{self.prefix}\\SessionDock.lnk")

    def _upload_script(self, name: str, text: str) -> str:
        local = Path(self.tmp.name) / name
        local.write_bytes(text.encode("ascii"))
        remote = f"{self.deploy_dir}\\{name}"
        self.sh.run(f'if not exist "{self.deploy_dir}" mkdir "{self.deploy_dir}"', timeout=30, check=True)
        self.sh.scp_upload(local, remote)
        return remote

    def _run_script(self, remote: str, marker: str, timeout: float) -> str:
        rc, out = self.sh.run(f'"{remote}"', timeout=timeout)
        if rc != 0 or marker not in out:
            raise ShellError(remote, rc, out)
        return out

    def _source_zip(self) -> Path:
        """`artifacts.source_zip`, or the tar converted locally (prefix directory stripped)."""
        if self.a.source_zip:
            with zipfile.ZipFile(self.a.source_zip) as zf:
                if "Cargo.toml" not in zf.namelist():
                    raise ValueError(f"{self.a.source_zip}: Cargo.toml must be at the zip root")
            return Path(self.a.source_zip)
        if not self.a.source_archive:
            raise RuntimeError("windows-node needs artifacts.source_zip or source_archive")
        out = Path(self.tmp.name) / f"source-{self.a.short}.zip"
        with tarfile.open(self.a.source_archive) as tf, zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zf:
            members = [m for m in tf.getmembers() if m.name != "pax_global_header"]
            names = {m.name for m in members}
            strip = 0 if "Cargo.toml" in names else 1
            for m in members:
                if not m.isfile() or (strip and "/" not in m.name):
                    continue
                rel = m.name.split("/", 1)[1] if strip else m.name
                fh = tf.extractfile(m)
                if fh is not None:
                    zf.writestr(rel, fh.read())
            if strip and "Cargo.toml" not in zf.namelist():
                raise ValueError(f"{self.a.source_archive}: no Cargo.toml at the root")
        return out

    def _web_zip(self) -> Path:
        out, root = Path(self.tmp.name) / "web.zip", Path(self.a.web_dir)
        with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zf:
            for f in sorted(p for p in root.rglob("*") if p.is_file()):
                zf.write(f, f.relative_to(root).as_posix())
        return out

    # -- steps ----------------------------------------------------------------------
    def probe(self) -> ProbeResult:
        try:
            rc, out = self.sh.run("ver", timeout=20)
        except ShellError as e:
            return ProbeResult(reachable=False, detail=str(e).splitlines()[0])
        if rc != 0 or "Windows" not in out:
            return ProbeResult(reachable=False, detail=f"not a Windows host: rc={rc} {out.strip()[:120]}")
        self.service_pids_before = self._tasklist("sessiondock.exe")
        meta = self.sh.http_json(self.t.health_url)
        res = ProbeResult(reachable=True, active=bool(self.service_pids_before),
                          build=meta.get("build") if meta else None,
                          detail=f"sessiondock.exe pids={self.service_pids_before}")
        for name in self.bins:
            sha = self._sha(f"{self.prefix}\\bin\\{name}.exe")
            if sha:
                res.binary_sha[name] = sha
        res.ptyhost_pids = self._tasklist("ptyhost.exe")
        res.host_records = self._host_records()
        _, out = self.sh.run(f'type "{self.prefix}\\etc\\deployed-commit" 2>nul', timeout=20)
        res.deployed_commit = out.strip() or None
        self.web_digest_before = self._web_digest(f"{self.prefix}\\web")
        self._load_vars()
        self.before = res
        return res

    def _scripts(self) -> dict[str, str]:
        p, bins = self.prefix, " ".join(self.bins)
        term_list = self.t.health_url.rsplit("/api/", 1)[0] + "/api/term/list"
        out = {}
        if self.ship_bin:
            out["build.cmd"] = render("build.cmd", {
                "SD": p, "SRC": self.source_dir, "TC": self.toolchain_bin,
                "ZIP": f"{self.deploy_dir}\\source-{self.a.short}.zip", "BINS": bins,
                "PKGS": " ".join(f"-p {n}" for n in self.bins)})
        out["backup.cmd"] = render("backup.cmd", {"SD": p, "BACKUP": self.backup_dir or
                                                  f"{p}\\backup-deploy-{self.a.short}-<UTC stamp>", "BINS": bins})
        out["swap-restart.cmd"] = render("swap-restart.cmd", {
            "SD": p, "PY": self.vars.get("PY", "<set PY= from restart-session1.cmd>"),
            "LNK": self.vars.get("LNK", f"{p}\\SessionDock.lnk"), "BINS": bins, "TERM_LIST": term_list})
        return out

    def plan(self) -> list[str]:
        p, d = self.prefix, self.deploy_dir
        steps: list[str] = []
        if self.ship_bin:
            steps += [f"scp <source zip of {self.a.short}> -> {d}\\source-{self.a.short}.zip",
                      f"scp build.cmd -> {d}\\build.cmd ; run it (timeout {int(BUILD_TIMEOUT)}s): extract, "
                      f"real cargo.exe build --release --locked, MTIME check, copy -> bin\\<name>.new.exe, certutil"]
        if self.ship_web:
            steps.append(f"scp web.zip -> {d}\\web.zip ; Expand-Archive -> {p}\\{WEB_STAGING}\\ "
                         "(build changes iff content differs)")
        steps += [f"scp backup.cmd ; run: copy bin\\*.exe + robocopy /MIR web -> {p}\\backup-deploy-{self.a.short}-<stamp>\\",
                  f"scp swap-restart.cmd ; run (timeout {int(SWAP_TIMEOUT)}s): process_identity.py --include-supervisor "
                  "-> move /y .new.exe -> robocopy /MIR web.staging web -> schtasks /IT explorer.exe SessionDock.lnk",
                  "restart: no-op (done inside swap-restart.cmd)",
                  f"verify: {self.t.health_url} within {self.o.health_timeout:.0f}s, new sessiondock.exe pid in session 1, "
                  "certutil sha == staged, build changed iff web changed, ptyhost.exe pids kept, host records >= before",
                  f"marker: {p}\\etc\\deployed-commit = '{self.a.commit} {self.a.short} {self.a.built_at} dirty={int(self.a.dirty)}'"]
        for name, text in self._scripts().items():
            steps.append(f"--- {name} ---")
            steps += ["    " + line for line in text.splitlines()]
        return steps

    def stage(self) -> None:
        p = self.prefix
        scripts = self._scripts()
        if self.ship_bin:
            remote = self._upload_script("build.cmd", scripts["build.cmd"])   # also creates .deploy\
            self.sh.scp_upload(self._source_zip(), f"{self.deploy_dir}\\source-{self.a.short}.zip", timeout=600)
            out = self._run_script(remote, "BUILD_OK", BUILD_TIMEOUT)
            mt: dict[tuple[str, str], str] = {}
            for line in out.splitlines():
                tok = line.split()
                if len(tok) == 3 and tok[0] in ("MTIME_BEFORE", "MTIME_AFTER"):
                    mt[(tok[0], tok[1])] = tok[2]
                elif len(tok) >= 3 and tok[0] == "NEW_SHA":
                    self.expected_sha[tok[1]] = "".join(tok[2:]).lower()
            compiled = "Compiling" in out
            for name in self.bins:
                if name not in self.expected_sha:
                    raise RuntimeError(f"build.cmd printed no NEW_SHA for {name}")
                fresh = mt.get(("MTIME_AFTER", name)) != mt.get(("MTIME_BEFORE", name))
                if compiled and not fresh:
                    raise RuntimeError(f"cargo compiled but target\\release\\{name}.exe kept its mtime")
                self.staged.append(name)
                self.log(f"staged {name}.new.exe sha256={self.expected_sha[name]} fresh={fresh}")
        if self.ship_web:
            web_zip = self._web_zip()
            self.sh.run(f'if not exist "{self.deploy_dir}" mkdir "{self.deploy_dir}"', timeout=30, check=True)
            self.sh.scp_upload(web_zip, f"{self.deploy_dir}\\web.zip")
            staging = f"{p}\\{WEB_STAGING}"
            self.sh.run(f'rd /s /q "{staging}" 2>nul & powershell -NoProfile -Command "Expand-Archive -LiteralPath '
                        f"'{self.deploy_dir}\\web.zip' -DestinationPath '{staging}' -Force\" & "
                        f'if exist "{staging}\\index.html" (echo WEB_STAGED) else (exit /b 5)', timeout=120, check=True)
            after = self._web_digest(staging)
            self.web_changed = after != self.web_digest_before
            self.log(f"web digest {self.web_digest_before} -> {after} changed={self.web_changed}")

    def backup(self) -> str:
        stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
        self.backup_dir = f"{self.prefix}\\backup-deploy-{self.a.short}-{stamp}"
        remote = self._upload_script("backup.cmd", self._scripts()["backup.cmd"])
        self._run_script(remote, "BACKUP_OK", 300)
        return self.backup_dir

    def swap(self) -> None:
        if "PY" not in self.vars:
            raise RuntimeError("service.python unknown: configure it or keep `set PY=` in restart-session1.cmd")
        remote = self._upload_script("swap-restart.cmd", self._scripts()["swap-restart.cmd"])
        out = self._run_script(remote, "===END===", SWAP_TIMEOUT)
        self.restarted = True
        for name in self.staged:
            if f"SWAPPED {name}" not in out:
                raise ShellError(remote, 0, f"{name}.new.exe was not swapped in\n{out}")

    def restart(self) -> None:
        if self.restarted:
            self.log("restart: already done by swap-restart.cmd")
            return
        self.sh.run(f'"{self.restart_cmd}"', timeout=SWAP_TIMEOUT, check=True)
        self.restarted = True

    def verify(self) -> VerifyResult:
        t0 = time.monotonic()
        before = self.before or ProbeResult(reachable=True)
        deadline = t0 + self.o.health_timeout
        meta, pids = None, []
        while True:
            pids = self._tasklist("sessiondock.exe")
            meta = self.sh.http_json(self.t.health_url) if pids else None
            if meta and (not self.restarted or pids != self.service_pids_before):
                break
            if time.monotonic() >= deadline:
                break
            time.sleep(1.0)
        res = VerifyResult(ok=True, build=meta.get("build") if meta else None, active=bool(pids))
        problems: list[str] = []
        if not pids:
            problems.append("no sessiondock.exe process")
        if meta is None:
            problems.append(f"{self.t.health_url} did not answer within {self.o.health_timeout:.0f}s")
        if self.restarted and pids and pids == self.service_pids_before:
            problems.append(f"sessiondock.exe pids {pids} unchanged: no restart happened")
        _, out = self.sh.run('powershell -NoProfile -Command "Get-CimInstance Win32_Process | Where-Object '
                             "{ $_.Name -eq 'sessiondock.exe' } | ForEach-Object { 'SESSION ' + $_.ProcessId + ' ' + "
                             '$_.SessionId }"', timeout=60)
        for line in out.splitlines():
            tok = line.split()
            if len(tok) == 3 and tok[0] == "SESSION" and tok[2] != "1":
                problems.append(f"sessiondock.exe pid {tok[1]} runs in session {tok[2]}, not 1")
        expected = dict(before.binary_sha)
        expected.update(self.expected_sha)
        for name in self.bins:
            sha = self._sha(f"{self.prefix}\\bin\\{name}.exe")
            if sha:
                res.binary_sha[name] = sha
            if name in expected and sha != expected[name]:
                problems.append(f"{name}.exe: on-disk sha {sha} != expected {expected[name]}")
        if meta is not None and before.build is not None:
            if self.web_changed and res.build == before.build:
                problems.append(f"web changed but build still {res.build}")
            if not self.web_changed and res.build != before.build and not self.staged:
                problems.append(f"web unchanged but build {before.build} -> {res.build}")
        res.ptyhost_pids = self._tasklist("ptyhost.exe")
        lost = sorted(set(before.ptyhost_pids) - set(res.ptyhost_pids))
        if lost:
            problems.append(f"ptyhost.exe pids lost: {lost}")
        res.host_records = self._host_records()
        if before.host_records is not None and res.host_records < before.host_records:
            problems.append(f"host records {before.host_records} -> {res.host_records}")
        res.ok = not problems
        res.detail = "; ".join(problems) if problems else (
            f"pids {self.service_pids_before}->{pids}, ptyhost {before.ptyhost_pids}->{res.ptyhost_pids}, "
            f"build {before.build}->{res.build}")
        res.elapsed = time.monotonic() - t0
        return res

    def rollback(self, backup_dir: str) -> None:
        p = self.prefix
        self.expected_sha, self.staged, self.restarted = {}, [], False
        for name in self.bins:
            rc, _ = self.sh.run(f'if exist "{backup_dir}\\bin\\{name}.exe" (copy /y "{backup_dir}\\bin\\{name}.exe" '
                                f'"{p}\\bin\\{name}.new.exe" >nul & echo RESTORED) else (echo ABSENT)', timeout=60)
            sha = self._sha(f"{p}\\bin\\{name}.new.exe")
            if sha:
                self.expected_sha[name] = sha
                self.staged.append(name)
        rc, out = self.sh.run(f'rd /s /q "{p}\\{WEB_STAGING}" 2>nul & robocopy "{backup_dir}\\web" "{p}\\{WEB_STAGING}" '
                              "/MIR /NFL /NDL /NJH /NJS /NP >nul", timeout=120)
        if rc >= 8:
            raise ShellError("robocopy backup web -> web.staging", rc, out)
        self.web_changed = self._web_digest(f"{p}\\{WEB_STAGING}") != self.web_digest_before
        self.swap()

    def write_marker(self) -> None:
        line = f"{self.a.commit} {self.a.short} {self.a.built_at} dirty={int(self.a.dirty)}"
        self.sh.run(f"powershell -NoProfile -Command \"Set-Content -LiteralPath '{self.prefix}\\etc\\deployed-commit' "
                    f"-Value '{line}' -Encoding ascii\"", timeout=30, check=True)

    def prune_backups(self, keep: int) -> list[str]:
        _, out = self.sh.run(f'dir /b /ad "{self.prefix}\\backup-deploy-*" 2>nul', timeout=30)
        names = sorted((n.strip() for n in out.splitlines() if n.strip().startswith("backup-deploy-")),
                       key=_backup_stamp)
        doomed = names[:-keep] if keep > 0 else names
        for n in doomed:
            self.sh.run(f'rd /s /q "{self.prefix}\\{n}"', timeout=120, check=True)
        return [f"{self.prefix}\\{n}" for n in doomed]
