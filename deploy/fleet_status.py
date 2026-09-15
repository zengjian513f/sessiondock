"""Read-only fleet status table for SessionDock deployments.

Usage:
    python3 deploy/fleet_status.py [--targets-file PATH] [--targets a,b]
        [--json] [--dry-run] [--timeout SECONDS] [--strict]

Loads JSON (default deploy/targets.local.json; schema:
deploy/targets.example.json). Skips enabled=false. Probes in parallel
(max 8) via ssh, or bash -c when ssh is null; -p only if ssh_port is
set. --dry-run prints the exact command per target and exits 0.
--json emits per-target dicts. --strict exits 1 if any target is
unreachable. Bad arguments or an unreadable file exit 2.
"""
from __future__ import annotations

import argparse
import json
import shlex
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

DASH = "-"
FIELDS = ("target", "kind", "reach", "active", "build", "hostname",
          "sha", "web", "ptyhost", "hosts", "backups", "commit", "age")


@dataclass
class Target:
    """One machine from the targets JSON. Mirrors deploy/targets.example.json."""
    name: str
    kind: str
    prefix: str
    ssh: str | None = None
    ssh_port: int | None = None
    ssh_opts: list[str] = field(default_factory=list)
    health_url: str = "http://127.0.0.1:8741/api/meta"
    service: dict = field(default_factory=dict)
    binaries: list[str] = field(default_factory=lambda: ["sessiondock"])
    enabled: bool = True
    extra: dict = field(default_factory=dict)


def load_targets(path: Path) -> list[Target]:
    doc = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(doc, dict) or not isinstance(doc.get("targets"), list):
        raise ValueError("missing 'targets' list")
    out: list[Target] = []
    for raw in doc["targets"]:
        if not isinstance(raw, dict):
            raise ValueError("target is not an object")
        name, kind, prefix = raw.get("name"), raw.get("kind"), raw.get("prefix")
        if not name or not kind or prefix is None:
            raise ValueError("target requires name, kind, prefix")
        port = raw.get("ssh_port")
        out.append(Target(
            name=str(name), kind=str(kind), prefix=str(prefix),
            ssh=str(raw["ssh"]) if raw.get("ssh") else None,
            ssh_port=int(port) if port is not None else None,
            ssh_opts=list(raw.get("ssh_opts") or []),
            health_url=str(raw.get("health_url") or "http://127.0.0.1:8741/api/meta"),
            service=dict(raw.get("service") or {}),
            binaries=list(raw.get("binaries") or ["sessiondock"]),
            enabled=bool(raw.get("enabled", True)), extra=dict(raw.get("extra") or {}),
        ))
    return out


def posix_inner(t: Target, hasher: str) -> str:
    q, pfx = shlex.quote, t.prefix
    parts = ["set +e", f"printf '%s\\n' \"$(curl -sS --max-time 6 {q(t.health_url)})\""]
    if t.kind == "macos-node":
        spec = q(f"gui/{t.service.get('gui_uid', '')}/{t.service.get('launchd_label', '')}")
        parts.append("printf 'active=%s\\n' \"$(launchctl print "
                     f"{spec} 2>/dev/null | grep -m1 'state =' | sed 's/.*= //')\"")
    else:
        unit = q(str(t.service.get("unit", "sessiondock.service")))
        parts.append(f"printf 'active=%s\\n' \"$(systemctl --user is-active {unit})\"")
    for name in t.binaries:
        parts.append(f"printf 'sha_{name}=%s\\n' "
                     f"\"$({hasher} {q(f'{pfx}/bin/{name}')} 2>/dev/null | cut -c1-12)\"")
    parts += [
        f"printf 'web=%s\\n' \"$({hasher} {q(pfx + '/web/app.js')} 2>/dev/null | cut -c1-12)\"",
        f"printf 'commit=%s\\n' \"$(cat {q(pfx + '/etc/deployed-commit')} 2>/dev/null)\"",
        "printf 'ptyhost=%s\\n' \"$(pgrep -xc ptyhost)\"",
        f"printf 'hosts=%s\\n' \"$(ls {q(pfx + '/host')} 2>/dev/null | wc -l)\"",
        f"printf 'backups=%s\\n' \"$(ls -1d {q(pfx)}/backup-* 2>/dev/null | wc -l)\"",
    ]
    return "; ".join(parts)


def inner_command(t: Target) -> str:
    if t.kind in ("linux-node", "hub"):
        return posix_inner(t, "sha256sum")
    if t.kind == "macos-node":
        return posix_inner(t, "shasum -a 256")
    if t.kind != "windows-node":
        raise ValueError(f"unknown target kind {t.kind!r}")
    pfx = t.prefix.rstrip("\\/")
    return " & ".join([
        f"curl.exe -sS --max-time 6 {t.health_url}",
        f"certutil -hashfile {pfx}\\bin\\sessiondock.exe SHA256",
        f"type {pfx}\\etc\\deployed-commit",
        'tasklist /FI "IMAGENAME eq ptyhost.exe" /NH',
        'tasklist /FI "IMAGENAME eq sessiondock.exe" /NH',
    ])


def argv_for(t: Target, inner: str) -> list[str]:
    if t.ssh is None:
        return ["bash", "-c", inner]
    argv = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8"]
    if t.ssh_port is not None:
        argv += ["-p", str(t.ssh_port)]
    return argv + list(t.ssh_opts) + [t.ssh, inner]


def _nz(v: object | None) -> str:
    s = "" if v is None else str(v).strip()
    return s if s else DASH

def short_commit(marker: str) -> tuple[str, str]:
    toks = marker.split()
    if not toks or not toks[0]:
        return DASH, DASH
    if len(toks) < 3:
        return toks[0][:7], DASH
    token = toks[2][:-1] + "+00:00" if toks[2].endswith("Z") else toks[2]
    try:
        dt = datetime.fromisoformat(token)
    except ValueError:
        return toks[0][:7], DASH
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    secs = max(0, int((datetime.now(timezone.utc) - dt.astimezone(timezone.utc)).total_seconds()))
    return toks[0][:7], (f"{secs // 3600}h" if secs < 86400 else f"{secs // 86400}d")


def extract_health(text: str) -> dict:
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("{") and s.endswith("}"):
            try:
                doc = json.loads(s)
            except ValueError:
                continue
            if isinstance(doc, dict):
                return doc
    return {}

def parse_kv(text: str) -> dict[str, str]:
    out: dict[str, str] = {}
    for line in text.splitlines():
        if "=" in line and not line.lstrip().startswith("{"):
            k, v = line.split("=", 1)
            if k.strip():
                out[k.strip()] = v.strip()
    return out

def blank_row(t: Target, reach: str, *, raw: str = "", error: str | None = None) -> dict:
    row = {k: DASH for k in FIELDS}
    row.update(target=t.name, kind=t.kind, reach=reach, raw=(raw or "")[:2000], error=error)
    return row

def status_row(t: Target, **kw: object) -> dict:
    err = kw.get("error")
    row = blank_row(t, str(kw.get("reach") or "ok"), raw=str(kw.get("raw") or ""),
                    error=None if err is None else str(err))
    row.update({k: kw[k] for k in FIELDS
                if k in kw and kw[k] is not None and k not in {"target", "kind"}})
    return row


def parse_output(t: Target, out: str) -> dict:
    health = extract_health(out)
    build, hostname = _nz(health.get("build")), _nz(health.get("hostname"))
    if t.kind == "windows-node":
        sha, commit, pty, running = DASH, "", 0, False
        for line in out.splitlines():
            s, low = line.strip(), line.strip().lower()
            compact = "".join(c for c in s if c in "0123456789abcdefABCDEF")
            if len(compact) == 64 and compact == s.replace(" ", "").replace("\t", ""):
                sha = compact[:12]
            elif low.startswith("ptyhost.exe"):
                pty += 1
            elif low.startswith("sessiondock.exe"):
                running = True
            elif s and not low.startswith(("sha256", "certutil")):
                tok = s.split()
                if tok and len(tok[0]) >= 7 and all(c in "0123456789abcdefABCDEF" for c in tok[0][:7]):
                    commit = s
        cshort, age = short_commit(commit)
        return status_row(t, active="running" if running else "stopped", build=build,
                          hostname=hostname, sha=sha, ptyhost=str(pty),
                          commit=cshort, age=age, raw=out)
    kv = parse_kv(out)
    health = extract_health(kv.get("health", "")) or health
    sha_key = f"sha_{t.binaries[0]}" if t.binaries else "sha"
    cshort, age = short_commit(kv.get("commit", ""))
    return status_row(
        t, active=_nz(kv.get("active")), build=_nz(health.get("build")),
        hostname=_nz(health.get("hostname")), sha=_nz(kv.get(sha_key)),
        web=_nz(kv.get("web")), ptyhost=_nz(kv.get("ptyhost")),
        hosts=_nz(kv.get("hosts")), backups=_nz(kv.get("backups")),
        commit=cshort, age=age, raw=out,
    )


def probe(t: Target, timeout: float) -> dict:
    try:
        argv = argv_for(t, inner_command(t))
    except ValueError as e:
        return blank_row(t, "error", error=str(e))
    try:
        p = subprocess.run(argv, capture_output=True, text=True,
                           timeout=timeout, errors="replace")
    except subprocess.TimeoutExpired as e:
        raw = e.stdout if isinstance(e.stdout, str) else (
            e.stdout.decode("utf-8", "replace") if e.stdout else "")
        return blank_row(t, "timeout", raw=raw, error=f"timeout after {timeout}s")
    except OSError as e:
        return blank_row(t, "error", error=str(e))
    raw, err = p.stdout or "", (p.stderr or "").strip() or None
    if p.returncode != 0 and not raw.strip():
        return blank_row(t, "error", raw=raw, error=err or f"exit {p.returncode}")
    row = parse_output(t, raw)
    if p.returncode != 0 and row.get("error") is None:
        row["error"] = err
    return row


def print_table(rows: list[dict]) -> None:
    grid = [[str(row[c]) for c in FIELDS] for row in rows]
    widths = [max([len(FIELDS[i]), *(len(v[i]) for v in grid)]) for i in range(len(FIELDS))]
    for vals in (list(FIELDS), *grid):
        print("  ".join(vals[i].ljust(widths[i]) for i in range(len(FIELDS))).rstrip())


def select_targets(all_targets: list[Target], subset: str) -> list[Target]:
    enabled = [t for t in all_targets if t.enabled]
    if not subset.strip():
        return enabled
    want = [n.strip() for n in subset.split(",") if n.strip()]
    if not want:
        raise ValueError("empty --targets")
    missing = [n for n in want if n not in {t.name for t in all_targets}]
    if missing:
        raise ValueError("unknown target(s): " + ", ".join(missing))
    return [t for t in enabled if t.name in set(want)]


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description="Read-only SessionDock fleet status.")
    p.add_argument("--targets-file", default="deploy/targets.local.json")
    p.add_argument("--targets", default="", help="comma-separated subset by name")
    p.add_argument("--json", action="store_true")
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--timeout", type=float, default=20)
    p.add_argument("--strict", action="store_true")
    args = p.parse_args(argv)
    if args.timeout <= 0:
        print("timeout must be positive", file=sys.stderr)
        return 2
    try:
        targets = select_targets(load_targets(Path(args.targets_file)), args.targets)
    except (OSError, ValueError, json.JSONDecodeError, KeyError, TypeError) as e:
        print(f"targets file: {e}", file=sys.stderr)
        return 2
    if args.dry_run:
        items = []
        for t in targets:
            try:
                cmd = shlex.join(argv_for(t, inner_command(t)))
            except ValueError as e:
                cmd = f"# {e}"
            items.append({"target": t.name, "kind": t.kind, "command": cmd})
        if args.json:
            print(json.dumps(items, indent=2, ensure_ascii=False))
        else:
            for it in items:
                print(it["command"])
        return 0
    rows: list[dict] = []
    if targets:
        with ThreadPoolExecutor(max_workers=min(8, len(targets))) as pool:
            futs = {pool.submit(probe, t, args.timeout): t.name for t in targets}
            by_name = {futs[f]: f.result() for f in as_completed(futs)}
        rows = [by_name[t.name] for t in targets]
    if args.json:
        print(json.dumps(rows, indent=2, ensure_ascii=False))
    else:
        print_table(rows)
    return 1 if args.strict and any(r["reach"] in {"timeout", "error"} for r in rows) else 0


if __name__ == "__main__":
    sys.exit(main())
