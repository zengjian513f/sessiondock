#!/usr/bin/env python3
# run_validation: skip
"""Private LinuxNodeHandler/HubHandler deployment and rollback rehearsal.

This is an operator-run integration drill, not a unit test. Supply two distinct
real binary directories (each containing sessiondock and sessiondock-hub):

    python3 tests/deploy_cutover_drill.py --old-bin-dir PATH --new-bin-dir PATH

--web-dir optionally copies an existing frontend build into the new generation;
otherwise both web generations are private fixtures. No build, fleet inventory,
SSH, production directory or systemd service is used. Only the exact handler
restart command is adapted to stop/start an owned real child. Shell.upload/run
and HTTP are real. Byte comparisons supplement the handler's existing staging
checksum contract; this script introduces no checksum scheme.

Exercises stage, backup, swap, restart, list_backups and rollback. Deliberately
does not call probe/verify (they inspect systemd and machine-wide ptyhost PIDs),
write_marker or prune. This does not validate systemd, proxy routing, Chromium,
Vue interactions, persistent ptyhost sessions, fleet transport or deploy.py's
automatic failure orchestration. Rollback is explicitly triggered after a
successful cutover, then repeated after a real HTTP outage.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import time
from urllib.error import URLError
from urllib.request import ProxyHandler, build_opener

REPO = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO / "deploy"))

from sdtargets.base import Artifacts, DeployOptions, Shell, Target  # noqa: E402
from sdtargets.hub import HubHandler  # noqa: E402
from sdtargets.linux import LinuxNodeHandler, q  # noqa: E402


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def snapshot(root):
    """Exact bytes and stable metadata, without digests or atime checks."""
    result = {}
    for path in sorted(root.rglob("*")):
        info = path.lstat()
        require(not path.is_symlink(), f"unexpected symlink: {path}")
        if path.is_file():
            result[path.relative_to(root).as_posix()] = (
                path.read_bytes(), stat.S_IMODE(info.st_mode), info.st_mtime_ns)
    return result


class PrivateProcess:
    def __init__(self, prefix, name, kind):
        self.prefix, self.name, self.kind = prefix, name, kind
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            self.port = sock.getsockname()[1]
        self.base = f"http://127.0.0.1:{self.port}"
        self.opener = build_opener(ProxyHandler({}))
        # Allowlist: do not inherit user CLI credentials, service settings,
        # proxy configuration or paths to real session roots.
        self.env = {k: v for k, v in os.environ.items()
                    if k in {"PATH", "LANG", "LC_ALL", "LC_CTYPE"}}
        home = prefix / "private-home"
        home.mkdir(mode=0o700)
        self.env.update({"HOME": str(home), "XDG_CONFIG_HOME": str(home / "config"),
                         "XDG_DATA_HOME": str(home / "data"),
                         "SESSIONDOCK_WEB_DIR": str(prefix / "web"),
                         "SESSIONDOCK_AUDIT_DIR": str(prefix / "audit"),
                         "SESSIONDOCK_ASYNC_WORKERS": "2", "SESSIONDOCK_READ_WORKERS": "2"})
        if kind == "hub":
            self.env.update({"SESSIONDOCK_HUB_BIND": f"127.0.0.1:{self.port}",
                             "SESSIONDOCK_HUB_NODES": str(prefix / "etc/hub-nodes.json"),
                             "SESSIONDOCK_HUB_CACHE_DIR": str(prefix / "hub/cache"),
                             "SESSIONDOCK_HUB_NETWORKS": "127.0.0.0/8"})
        else:
            self.env.update({"SESSIONDOCK_BIND": f"127.0.0.1:{self.port}",
                             "SESSIONDOCK_STATE_DIR": str(prefix / "state"),
                             "SESSIONDOCK_TRASH_DIR": str(prefix / "trash"),
                             "SESSIONDOCK_SEARCH_CACHE_DIR": str(prefix / "search-cache"),
                             "SESSIONDOCK_PROC_ROOT": str(prefix / "private-proc")})
            for source in ("claude", "codex", "grok"):
                directory = prefix / "roots" / source
                directory.mkdir(parents=True, mode=0o700)
                self.env[f"SESSIONDOCK_{source.upper()}_ROOT"] = str(directory)
        self.process = None
        self.restarts = 0
        self.log = (prefix / "process.log").open("w+b")

    def get(self, route):
        with self.opener.open(self.base + route, timeout=2) as response:
            require(response.status == 200, f"{route}: HTTP {response.status}")
            return response.read()

    def stop(self):
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)

    def restart(self):
        self.stop()
        self.process = subprocess.Popen([str(self.prefix / "bin" / self.name)],
                                        cwd=self.prefix, env=self.env,
                                        stdout=self.log, stderr=self.log)
        self.restarts += 1
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                self.log.flush()
                self.log.seek(0)
                raise AssertionError(f"{self.name} exited: " + self.log.read().decode(errors="replace"))
            try:
                json.loads(self.get("/api/meta"))
                return
            except (OSError, URLError):
                # Bounded readiness retries, never a service polling loop.
                time.sleep(0.05)
        raise AssertionError(f"{self.name}: no real /api/meta within 15 seconds")

    def close(self):
        self.stop()
        self.log.close()


class RestartOnlyShell(Shell):
    """Everything runs via the original Shell except this one exact command."""
    def __init__(self, target, service, restart_command, log):
        super().__init__(target, log)
        self.service, self.restart_command = service, restart_command

    def run(self, cmd, timeout=60, check=False, posix=None):
        if cmd == self.restart_command:
            self.log("$ [private child restart] " + self.service.name)
            self.service.restart()
            return 0, ""
        require("systemctl" not in cmd and "ssh" not in cmd,
                "drill must not access service manager or remote hosts")
        return super().run(cmd, timeout=timeout, check=check, posix=posix)


def web_versions(root, web_dir, old_web_dir=None):
    old, new = root / "old-web", root / "new-web"
    old.mkdir(mode=0o700)
    (old / "index.html").write_text(
        '<!doctype html><html><head><meta name="sessiondock-mode" '
        'content="__SESSIONDOCK_MODE__"></head><body>drill-old-entry</body></html>\n')
    if old_web_dir:
        shutil.copytree(old_web_dir, old, dirs_exist_ok=True)
        require((old / "index.html").is_file(), "--old-web-dir requires index.html")
        with (old / "index.html").open("a") as output:
            output.write("\n<!-- drill-old-entry -->\n")
    (old / "old-only.txt").write_text("removed on cutover\n")
    if web_dir:
        shutil.copytree(web_dir, new)
        require((new / "index.html").is_file(), "--web-dir requires index.html")
        with (new / "index.html").open("a") as output:
            output.write("\n<!-- drill-new-entry -->\n")
    else:
        shutil.copytree(old, new)
        (new / "old-only.txt").unlink()
        (new / "index.html").write_text(
            '<!doctype html><html><head><meta name="sessiondock-mode" '
            'content="__SESSIONDOCK_MODE__"></head><body>drill-new-entry'
            ' with changed web content</body></html>\n')
    (new / "new-only.txt").write_text("removed on rollback\n")
    # Valid releases can contain changed bytes with equal size and mtime.
    # rsync's default quick check must not silently skip this asset.
    for directory, value in ((old, b"old\n"), (new, b"new\n")):
        asset = directory / "drill-version.txt"
        asset.write_bytes(value)
        os.utime(asset, ns=(1_700_000_000_000_000_000,) * 2)
    return old, new


def drill(root, kind, old_bins, new_bins, old_web, new_web):
    name = "sessiondock-hub" if kind == "hub" else "sessiondock"
    prefix = root / kind
    prefix.mkdir(mode=0o700)
    for directory in ("bin", "etc", "state", "host", "lifecycle", "delivery",
                      "audit", "trash", "search-cache", "hub/cache", "private-proc"):
        (prefix / directory).mkdir(parents=True, mode=0o700)
    shutil.copy2(old_bins / name, prefix / "bin" / name)
    shutil.copytree(old_web, prefix / "web")
    for directory in ("etc", "state", "host", "lifecycle", "delivery", "audit",
                      "trash", "search-cache", "hub"):
        sentinel = prefix / directory / "drill-sentinel"
        sentinel.write_bytes(b"private sentinel\x00\r\n" + directory.encode())
        sentinel.chmod(0o600)
    (prefix / "etc/hub-nodes.json").write_text("[]\n")
    (prefix / "etc/service.env").write_text("DRILL_CONFIGURATION=preserve\n")
    (prefix / "etc/deployed-commit").write_text("drill-old-marker\n")
    preserved = {d: snapshot(prefix / d) for d in ("etc", "state", "host", "lifecycle",
                                                   "delivery", "audit", "trash", "search-cache", "hub")}
    old_bin = snapshot(prefix / "bin")
    old_disk_web, new_disk_web = snapshot(old_web), snapshot(new_web)
    service = PrivateProcess(prefix, name, kind)
    log = lambda text: print(f"[{kind}] {text}", flush=True)
    target = Target(name="private-drill", kind=kind, prefix=str(prefix), ssh=None,
                    binaries=[name], health_url=service.base + "/api/meta",
                    service={"unit": "private-drill.service"})
    # Fill the existing Artifacts contract using its existing shell checksum.
    rc, output = Shell(target).run(f"sha256sum {q(str(new_bins / name))} | cut -c1-64",
                                  timeout=10, check=True)
    artifacts = Artifacts(commit="d" * 40, short="ddddddd", dirty=False,
                          built_at="private-drill", web_dir=new_web,
                          binaries={name: new_bins / name}, sha256={name: output.strip()},
                          source_archive=None)
    handler = (HubHandler if kind == "hub" else LinuxNodeHandler)(
        target, artifacts, DeployOptions(), log)
    handler.sh = RestartOnlyShell(target, service,
                                 f"{handler.systemctl} restart {q(handler.unit)}", log)

    def unchanged():
        for directory, before in preserved.items():
            # The real hub may initialize its own cache files at startup.
            # Every pre-existing sentinel/config must retain bytes/mode/mtime.
            after = snapshot(prefix / directory)
            require(all(after.get(path) == value for path, value in before.items()),
                    f"{kind}: {directory} sentinel/config changed")

    def http_version(version):
        expected_bins = old_bins if version == "old" else new_bins
        running = Path(f"/proc/{service.process.pid}/exe")
        require(running.read_bytes() == (expected_bins / name).read_bytes(),
                f"{kind}: running process is not the {version} binary")
        meta = json.loads(service.get("/api/meta"))
        capabilities = meta.get("capabilities", meta)
        require(capabilities.get("hub") is (kind == "hub"), f"{kind}: wrong meta mode: {meta}")
        require(isinstance(meta.get("build"), str) and meta["build"], f"{kind}: meta lacks build")
        entry = service.get("/")
        require(f"drill-{version}-entry".encode() in entry, f"{kind}: wrong HTTP entry")
        require(service.get("/index.html") == entry, f"{kind}: index alias mismatch")
        require(service.get("/drill-version.txt") == f"{version}\n".encode(),
                f"{kind}: wrong HTTP equal-size/mtime asset")
        return meta, entry

    try:
        service.restart()
        old_meta, old_entry = http_version("old")
        old_pid = service.process.pid
        unchanged()
        handler.stage()
        require((prefix / "bin" / (name + ".new")).read_bytes() == (new_bins / name).read_bytes(),
                f"{kind}: staging binary differs")
        require(snapshot(prefix / "web.staging") == new_disk_web, f"{kind}: staging web differs")
        require((prefix / "bin" / name).read_bytes() == (old_bins / name).read_bytes(),
                f"{kind}: stage modified live binary")
        require(snapshot(prefix / "web") == old_disk_web, f"{kind}: stage modified live web")
        backup = Path(handler.backup())
        require(snapshot(backup / "bin") == old_bin, f"{kind}: backup bin differs")
        require(snapshot(backup / "web") == old_disk_web, f"{kind}: backup web differs")
        require(str(backup) in handler.list_backups(), f"{kind}: backup not discoverable")
        unchanged()
        handler.swap()
        require((prefix / "bin" / name).read_bytes() == (new_bins / name).read_bytes(),
                f"{kind}: swap did not install new binary")
        require(snapshot(prefix / "web") == new_disk_web, f"{kind}: swap web differs (including equal-size/mtime asset)")
        require(service.process.poll() is None and service.process.pid == old_pid,
                f"{kind}: swap interrupted running child")
        http_version("old")  # The service holds the old web snapshot until restart.
        handler.restart()
        require(service.process.pid != old_pid, f"{kind}: child did not restart")
        new_meta, _ = http_version("new")
        require(new_meta["build"] != old_meta["build"], f"{kind}: cutover build did not change")
        unchanged()
        require(not (prefix / "web.staging").exists(), f"{kind}: web staging remains")
        require(not (prefix / "bin" / (name + ".new")).exists(), f"{kind}: bin staging remains")
        log("PASS actual old -> new bin/web/HTTP; private data and configuration unchanged")

        def restored():
            require(snapshot(prefix / "bin") == old_bin, f"{kind}: rollback bin differs")
            require(snapshot(prefix / "web") == old_disk_web, f"{kind}: rollback web differs")
            meta, entry = http_version("old")
            require(meta["build"] == old_meta["build"] and entry == old_entry,
                    f"{kind}: rollback did not restore actual HTTP entry/meta")
            unchanged()
            require(snapshot(backup / "bin") == old_bin and snapshot(backup / "web") == old_disk_web,
                    f"{kind}: rollback modified backup")

        handler.rollback(str(backup))
        restored()
        log("PASS actual rollback new -> old bin/web/HTTP; backup and sentinels unchanged")
        # Reinstall, then create a real outage, rather than faking an HTTP failure.
        handler.stage()
        handler.swap()
        handler.restart()
        http_version("new")
        service.stop()
        try:
            service.get("/api/meta")
        except (OSError, URLError):
            pass
        else:
            raise AssertionError(f"{kind}: stopped child still serves HTTP")
        handler.rollback(str(backup))
        restored()
        require(service.restarts == 5, f"{kind}: unexpected restart count")
        log("PASS rollback from real HTTP outage; old entry/meta available again")
    finally:
        service.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--old-bin-dir", type=Path, required=True)
    parser.add_argument("--new-bin-dir", type=Path, default=REPO / "target/release")
    parser.add_argument("--web-dir", type=Path)
    parser.add_argument("--old-web-dir", type=Path, help="preserved previous release frontend")
    args = parser.parse_args()
    for tool in ("bash", "rsync", "sha256sum", "cut"):
        require(shutil.which(tool), f"required real command missing: {tool}")
    for name in ("sessiondock", "sessiondock-hub"):
        for directory in (args.old_bin_dir, args.new_bin_dir):
            require((directory / name).is_file() and os.access(directory / name, os.X_OK),
                    f"missing executable: {directory / name}")
        require((args.old_bin_dir / name).read_bytes() != (args.new_bin_dir / name).read_bytes(),
                f"old/new {name} must contain distinct binary bytes")
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="sessiondock-deploy-cutover-") as directory:
        root = Path(directory)
        root.chmod(0o700)
        print(f"Private drill root: {root}; all listeners are loopback", flush=True)
        old, new = web_versions(root, args.web_dir, args.old_web_dir)
        for kind in ("linux-node", "hub"):
            drill(root, kind, args.old_bin_dir.resolve(), args.new_bin_dir.resolve(), old, new)
    print(f"PASS deploy_cutover_drill: both handlers; cleaned children/directories ({time.monotonic() - started:.2f}s)",
          flush=True)


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"FAIL deploy_cutover_drill: {error}", file=sys.stderr, flush=True)
        sys.exit(1)
