"""Linux node handler (kind "linux-node"): systemd --user unit, build-machine glibc binaries.

Layout on the target: `<prefix>/{bin,web,etc,state,delivery,lifecycle,host,...}`.
Only `bin/`, `web/`, `etc/deployed-commit` and `backup-deploy-*` change. The unit
restarts with `KillMode=process`, so detached `ptyhost` session hosts survive; the
probe records their pids and `verify()` proves every one of them is still alive.

Every remote round trip is one ssh call running a small POSIX shell script; the
probe prints `key=value` lines that both `probe()` and `verify()` parse.
"""
from __future__ import annotations

import json
import re
import shlex
import time
from datetime import datetime, timezone

from .base import ProbeResult, ShellError, TargetHandler, VerifyResult, failed_probe, register

BACKUP_RE = re.compile(r"^backup-deploy-[0-9a-f]+-[0-9]{8}-[0-9]{6}$")
PTYHOST = "ptyhost"


def q(value: str) -> str:
    return shlex.quote(value)


@register
class LinuxNodeHandler(TargetHandler):
    kind = "linux-node"
    default_unit = "sessiondock.service"
    ships_ptyhost = True            # the hub subclass has no session hosts

    # -- target facts ---------------------------------------------------------------
    @property
    def prefix(self) -> str:
        return self.t.prefix.rstrip("/") or "/"

    @property
    def unit(self) -> str:
        return self.t.service.get("unit", self.default_unit)

    @property
    def systemctl(self) -> str:
        return "systemctl --user" if self.t.service.get("scope", "user") == "user" else "systemctl"

    def path(self, *parts: str) -> str:
        return "/".join((self.prefix, *parts))

    def ship_web(self) -> bool:
        return not self.o.bin_only

    def binaries(self) -> list[str]:
        """Binary names this run replaces (none with --web-only)."""
        if self.o.web_only:
            return []
        names = list(self.t.binaries)
        if self.o.with_ptyhost and self.ships_ptyhost and PTYHOST not in names:
            names.append(PTYHOST)
        if self.o.with_sdkhost and self.kind == "linux-node" and "sdkhost" not in names:
            names.append("sdkhost")
        return names

    def watched_binaries(self) -> list[str]:
        """Binary names whose release version the probe records (shipped or not)."""
        names = list(self.t.binaries)
        if self.ships_ptyhost and PTYHOST not in names:
            names.append(PTYHOST)
        if self.o.with_sdkhost and self.kind == "linux-node" and "sdkhost" not in names:
            names.append("sdkhost")
        return names

    # -- probe --------------------------------------------------------------------------
    def _probe_script(self) -> str:
        p = q(self.prefix)
        say = "printf '%s\\n'"      # dash's echo interprets backslashes; the meta JSON may hold some
        lines = [
            f"P={p}",
            f'{say} "hostname=$(hostname)"',
            'if [ -d "$P/bin" ] && [ -d "$P/web" ]; then echo layout=ok; else echo layout=missing; fi',
            f'{say} "active=$({self.systemctl} is-active {q(self.unit)} 2>/dev/null || true)"',
            f'{say} "meta=$(curl -sS --max-time 8 {q(self.t.health_url)} 2>/dev/null || true)"',
        ]
        for name in self.watched_binaries():
            lines.append(f'{say} "version:{name}=$(cat "$P/bin/{name}.version" 2>/dev/null)"')
        lines += [
            f'{say} "pids=$(pgrep -x ptyhost 2>/dev/null | tr "\\n" " " || true)"',
            f'if [ -d "$P/host" ]; then {say} "hosts=$(ls "$P/host" 2>/dev/null | wc -l)"; fi',
            f'{say} "marker=$(cat "$P/etc/deployed-commit" 2>/dev/null || true)"',
            "exit 0",
        ]
        return "\n".join(lines)

    def _parse_probe(self, out: str) -> dict[str, str]:
        facts: dict[str, str] = {}
        for line in out.splitlines():
            key, sep, value = line.partition("=")
            if sep and re.fullmatch(r"[a-z]+(:[A-Za-z0-9_.-]+)?", key):
                facts[key] = value.strip()
        return facts

    def _snapshot(self) -> tuple[ProbeResult, dict[str, str]]:
        """One round trip: service state, meta build, binary versions, ptyhost pids."""
        try:
            rc, out = self.sh.run(self._probe_script(), timeout=60)
        except ShellError as e:
            return failed_probe(self.t, e.rc, e.out or str(e)), {}
        if rc == 255 or "layout=" not in out:
            tail = out.strip().splitlines()[-1] if out.strip() else f"rc={rc}"
            return failed_probe(self.t, rc, f"probe failed: {tail}"), {}
        facts = self._parse_probe(out)
        expected = self.t.extra.get("expected_hostname")
        if expected and facts.get("hostname") != expected:
            return ProbeResult(False, f"target identity mismatch: expected {expected!r}, got {facts.get('hostname')!r}"), facts
        if facts.get("layout") != "ok":
            return ProbeResult(False, f"prefix layout missing under {self.prefix}"), facts
        meta = None
        if facts.get("meta"):
            try:
                meta = json.loads(facts["meta"])
            except ValueError:
                meta = None
        pids = [int(x) for x in facts.get("pids", "").split() if x.isdigit()]
        hosts = facts.get("hosts")
        result = ProbeResult(
            reachable=True,
            detail="",
            build=(meta or {}).get("build") if isinstance(meta, dict) else None,
            active=facts.get("active") == "active",
            binary_versions={k.split(":", 1)[1]: v for k, v in facts.items() if k.startswith("version:") and v},
            ptyhost_pids=pids,
            host_records=int(hosts) if hosts and hosts.isdigit() else None,
            deployed_commit=facts.get("marker") or None,
        )
        return result, facts

    def probe(self) -> ProbeResult:
        result, facts = self._snapshot()
        self.before = result
        if result.reachable:
            self.log(f"probe: hostname={facts.get('hostname')} active={result.active} build={result.build} "
                     f"versions={result.binary_versions} "
                     f"ptyhost={len(result.ptyhost_pids)} hosts={result.host_records} "
                     f"marker={result.deployed_commit!r}")
        else:
            self.log(f"probe: {result.detail}")
        return result

    # -- plan -----------------------------------------------------------------------------
    def restart_note(self, before: ProbeResult) -> str:
        return f"KillMode=process: {len(before.ptyhost_pids)} ptyhost hosts keep running"

    def plan(self) -> list[str]:
        before = self.before or ProbeResult(False)
        out = []
        if self.kind == "linux-node":
            out.append("check native SDK input ledger before staging and backup; defer while in-process SDK work exists (independent sdkhost work survives)")
        for name in self.binaries():
            want = self.a.version
            have = before.binary_versions.get(name, "")
            state = "unchanged" if have == want else f"{have or '-'} -> {want}"
            out.append(f"upload {name} -> {self.path('bin', name + '.new')}, then mv -f over bin/{name} ({state})")
        if not self.binaries():
            out.append("binaries: skipped (--web-only)")
        if self.ship_web():
            out.append(f"rsync web snapshot -> {self.path('web.staging')}/, then rsync -a --delete into web/ on the target")
        else:
            out.append("web: skipped (--bin-only)")
        out.append(f"backup bin/{{{','.join(self.backup_names())}}} and web/ -> {self.path('backup-deploy-' + self.a.short + '-<stamp>')}")
        out.append(f"{self.systemctl} restart {self.unit} ({self.restart_note(before)})")
        hosts = "no host records" if before.host_records is None else f"host records >= {before.host_records}"
        out.append(f"verify {self.t.health_url}: release {self.a.version}, "
                   f"{len(before.ptyhost_pids)} ptyhost pids alive, {hosts}")
        out.append(f"write {self.path('etc', 'deployed-commit')} = {self.a.commit[:12]}; prune backups keep {self.o.keep_backups}")
        return out

    # -- stage / backup / swap / restart ----------------------------------------------
    def check_native_work(self) -> None:
        """Do not knowingly restart a node that owns an unfinished SDK turn.

        This is a preflight observation, not an admission lock. Keep it before
        backup so a refusal cannot trigger the restart in automatic rollback.
        """
        if self.kind != "linux-node":
            return
        script = '''import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
if not path.exists():
    sys.exit(0)
rows = json.loads(path.read_text())
active = [row for queue in rows.values() for row in queue
          if row.get("state") in ("sending", "accepted", "recovering")
          and not (row.get("result") or {}).get("sdkhost")]
if active:
    print("Native SDK work is unfinished; deployment deferred before backup/swap/restart:")
    for row in active:
        target = row.get("target", {})
        print(target.get("source", ""), target.get("uid", ""),
              row.get("request_id", ""), row["state"])
    sys.exit(1)
'''
        self.sh.run(
            f"python3 -c {q(script)} {q(self.path('state', 'native-sdk-inputs.json'))}",
            timeout=30, check=True,
        )

    def stage(self) -> None:
        self.check_sdkhost_dependency(self.binaries())
        self.check_native_work()
        try:
            for name in self.binaries():
                local = self.a.binaries.get(name)
                if local is None:
                    raise RuntimeError(f"stage has no binary {name!r}; build it first")
                dest = self.path("bin", name + ".new")
                self.sh.upload(local, dest)
                self.sh.run(f"chmod 0755 {q(dest)}", timeout=60, check=True)
                self.sh.run(f"printf '%s\\n' {q(self.a.version)} > {q(dest + '.version')}", check=True)
                self.log(f"staged {dest} version={self.a.version}")
            if self.ship_web():
                self.sh.upload(self.a.web_dir.as_posix() + "/", self.path("web.staging") + "/", delete=True)
                self.log(f"staged web -> {self.path('web.staging')}/")
        except Exception:
            self._discard_staging()
            raise

    def _discard_staging(self) -> None:
        news = " ".join(q(self.path("bin", n)) for name in self.binaries() for n in (name + ".new", name + ".new.version"))
        cmd = f"rm -rf {q(self.path('web.staging'))}" + (f" && rm -f {news}" if news else "")
        self.sh.run(cmd, timeout=60)

    def backup_names(self) -> list[str]:
        names = list(self.t.binaries)
        if self.o.with_ptyhost and self.ships_ptyhost and PTYHOST not in names:
            names.append(PTYHOST)
        if self.o.with_sdkhost and self.kind == "linux-node" and "sdkhost" not in names:
            names.append("sdkhost")
        return names

    def backup(self) -> str:
        self.check_native_work()
        stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
        bdir = self.path(f"backup-deploy-{self.a.short}-{stamp}")
        copies = " && ".join(
            f'if [ -f "$P/bin/{n}" ]; then cp -p "$P/bin/{n}" "$B/bin/{n}"; fi' for name in self.backup_names() for n in (name, name + ".version"))
        self.sh.run(f'P={q(self.prefix)}; B={q(bdir)}; mkdir -p "$B/bin" "$B/web" && {copies} '
                    f'&& rsync -a --delete "$P/web/" "$B/web/"', timeout=300, check=True)
        self.backup_marker(bdir)
        self.backup_dir = bdir
        self.log(f"backup -> {bdir}")
        return bdir

    def swap(self) -> None:
        for name in self.binaries():
            new, cur = self.path("bin", name + ".new"), self.path("bin", name)
            self.sh.run(f"test -f {q(new)} && mv -f {q(new)} {q(cur)}", timeout=60, check=True)
            self.sh.run(f"mv -f {q(new + '.version')} {q(cur + '.version')}", check=True)
            self.log(f"swapped bin/{name}")
        if self.ship_web():
            staging = self.path("web.staging")
            # Equal size/mtime does not imply equal release contents.
            self.sh.run(f"test -f {q(staging + '/index.html')} && rsync -a --ignore-times --delete {q(staging + '/')} {q(self.path('web') + '/')} "
                        f"&& rm -rf {q(staging)}", timeout=300, check=True)
            self.log("swapped web/")

    def restart(self) -> None:
        self.sh.run(f"{self.systemctl} restart {q(self.unit)}", timeout=120, check=True)
        self.log(f"restarted {self.unit}")

    # -- verify ---------------------------------------------------------------------------
    def verify(self) -> VerifyResult:
        t0 = time.monotonic()
        before = self.before or ProbeResult(False)
        meta = self.sh.wait_http_json(self.t.health_url, self.o.health_timeout)
        after, facts = self._snapshot()
        problems: list[str] = []
        if meta is None:
            problems.append(f"{self.t.health_url} did not answer within {self.o.health_timeout:.0f}s")
        if not after.reachable:
            problems.append(after.detail)
        elif not after.active:
            problems.append(f"{self.unit} is not active ({facts.get('active') or '?'})")
        for name in self.binaries():
            have = after.binary_versions.get(name)
            if have != self.a.version:
                problems.append(f"bin/{name} version {have} != release {self.a.version}")
        if self.ship_web() and after.build != self.a.version:
            problems.append(f"served build {after.build} != release {self.a.version}")
        if after.build is None:
            problems.append("meta has no build")
        dead = sorted(set(before.ptyhost_pids) - set(after.ptyhost_pids))
        if dead:
            problems.append(f"ptyhost pids gone: {dead}")
        if before.host_records is not None and (after.host_records or 0) < before.host_records:
            problems.append(f"host records {after.host_records} < {before.host_records}")
        detail = "; ".join(problems) if problems else (
            f"build {before.build} -> {after.build}, "
            f"{len(after.ptyhost_pids)} ptyhost alive")
        self.log(f"verify: {'OK' if not problems else 'FAIL'}: {detail}")
        return VerifyResult(not problems, detail, after.build, after.active, after.binary_versions,
                            after.ptyhost_pids, after.host_records, time.monotonic() - t0)

    # -- rollback / marker / prune ---------------------------------------------------
    def rollback(self, backup_dir: str) -> None:
        if not BACKUP_RE.match(backup_dir.rsplit("/", 1)[-1]):
            raise RuntimeError(f"refusing to roll back from {backup_dir!r}: not a backup-deploy-* dir")
        self.sh.run(f"test -d {q(backup_dir + '/bin')}", timeout=30, check=True)
        script = "\n".join([
            f"set -e; P={q(self.prefix)}; B={q(backup_dir)}",
            'rm -rf "$P/web.staging"',
            'for f in "$B"/bin/*; do [ -f "$f" ] || continue; n=$(basename "$f");'
            ' case "$n" in *.version) ;; *) rm -f "$P/bin/$n.version" ;; esac;'
            ' cp -p "$f" "$P/bin/$n.new" && mv -f "$P/bin/$n.new" "$P/bin/$n"; echo "restored bin/$n"; done',
            'if [ -f "$B/web/index.html" ]; then rsync -a --ignore-times --delete "$B/web/" "$P/web/"; echo "restored web/"; fi',
        ])
        rc, out = self.sh.run(script, timeout=300, check=True)
        for line in out.strip().splitlines():
            self.log(f"rollback: {line}")
        self.restore_marker(backup_dir)
        self.restart()

    def list_backups(self) -> list[str]:
        """Newest first by directory mtime: hand-made backups carry local-time stamps,
        this tool stamps UTC, so the name alone does not order them."""
        rc, out = self.sh.run(f"ls -1td {q(self.prefix)}/backup-deploy-* 2>/dev/null || true", timeout=30)
        return [p.strip() for p in out.splitlines() if BACKUP_RE.match(p.strip().rsplit("/", 1)[-1])]

    def write_marker(self) -> None:
        text = f"{self.a.commit} {self.a.short} {self.a.built_at} dirty={int(self.a.dirty)}"
        self.sh.run(f"mkdir -p {q(self.path('etc'))} && printf '%s\\n' {q(text)} > {q(self.path('etc', 'deployed-commit'))}",
                    timeout=30, check=True)
        self.log(f"marker: {text}")

    def prune_backups(self, keep: int) -> list[str]:
        if keep <= 0:
            return []
        doomed = self.list_backups()[keep:]
        for path in doomed:
            self.sh.run(f"rm -rf {q(path)}", timeout=120, check=True)
            self.log(f"pruned {path}")
        return doomed
