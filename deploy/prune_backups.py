"""List and prune old SessionDock deployment backups on linux-node, hub, and macos-node.

Families under <prefix> are pruned independently, keeping the newest N stamped
entries of each family. A: directories backup-* directly under prefix
(backup-deploy-*, backup-web-*, backup-<label>-* are one family). B: directories
web.rollback-* directly under prefix. C: files bin/sessiondock.before-* and
bin/sessiondock.bak-*. Age is the last YYYYMMDD-HHMMSS, YYYYMMDD-HHMM, or
YYYYMMDD token in the name; no token means kept (no stamp). Symlinks are never
followed or deleted. Without --yes this is a dry run. Targets JSON is parsed
here (not via deploy/sdtargets). windows-node is skipped (windows) and never
touched. Listing is one portable POSIX sh loop (macOS find has no -printf).
Deletion is one command of exact, Python-validated paths.
"""
from __future__ import annotations

import argparse
import json
import re
import shlex
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

HANDLED = {"linux-node", "hub", "macos-node"}
STAMP_RE = re.compile(r"\d{8}-\d{6}|\d{8}-\d{4}|\d{8}")
GLOB_CHARS = frozenset("*?[]")


@dataclass
class Target:
    """One machine from the targets JSON (fields as in deploy/targets.example.json)."""
    name: str
    kind: str
    prefix: str
    ssh: str | None = None
    ssh_port: int | None = None
    ssh_opts: list[str] = field(default_factory=list)
    service: dict = field(default_factory=dict)
    enabled: bool = True


@dataclass
class Entry:
    typ: str  # L, D, F
    path: str
    family: str
    rel: str
    stamp: str | None
    action: str = ""


def load_targets(path: Path) -> list[Target]:
    doc = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(doc, list):
        raw = doc
    elif isinstance(doc, dict) and isinstance(doc.get("targets"), list):
        raw = doc["targets"]
    elif isinstance(doc, dict) and "name" in doc:
        raw = [doc]
    else:
        raise SystemExit(f"cannot parse targets in {path}")
    out: list[Target] = []
    for item in raw:
        opts = item.get("ssh_opts") or []
        if isinstance(opts, str):
            opts = shlex.split(opts)
        port = item.get("ssh_port")
        prefix = str(item.get("prefix") or "")
        if prefix != "/":
            prefix = prefix.rstrip("/")
        out.append(Target(
            name=item["name"], kind=item["kind"], prefix=prefix,
            ssh=item.get("ssh"),
            ssh_port=int(port) if port is not None else None,
            ssh_opts=list(opts), service=dict(item.get("service") or {}),
            enabled=bool(item.get("enabled", True)),
        ))
    return out


def stamp_key(stamp: str) -> tuple[int, int]:
    if "-" in stamp:
        day, tod = stamp.split("-", 1)
        return int(day), int(tod.ljust(6, "0"))
    return int(stamp), 0


def classify(prefix: str, path: str) -> str | None:
    if not path.startswith(prefix + "/"):
        return None
    rel = path[len(prefix) + 1:]
    if "/" not in rel:
        if rel.startswith("backup-"):
            return "A"
        if rel.startswith("web.rollback-"):
            return "B"
        return None
    if rel.count("/") == 1 and (
        rel.startswith("bin/sessiondock.before-")
        or rel.startswith("bin/sessiondock.bak-")
    ):
        return "C"
    return None


def listing_cmd(prefix: str) -> str:
    q = shlex.quote(prefix)
    return (
        f"for p in {q}/backup-* {q}/web.rollback-* "
        f"{q}/bin/sessiondock.before-* {q}/bin/sessiondock.bak-*; "
        "do [ -e \"$p\" ] || [ -L \"$p\" ] || continue; "
        "if [ -L \"$p\" ]; then t=L; elif [ -d \"$p\" ]; then t=D; else t=F; fi; "
        "printf '%s\\t%s\\n' \"$t\" \"$p\"; done"
    )


def run_on_target(t: Target, cmd: str, timeout: float) -> tuple[int, str]:
    if t.ssh is None:
        argv = ["bash", "-c", cmd]
    else:
        argv = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8"]
        if t.ssh_port is not None:
            argv += ["-p", str(t.ssh_port)]
        # The login shell may be zsh (nomatch aborts on unmatched globs): force sh.
        argv += list(t.ssh_opts) + [t.ssh, "sh -c " + shlex.quote(cmd)]
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired as e:
        extra = (e.stdout or "") + (e.stderr or "")
        return 124, f"timeout after {timeout}s\n{extra}"
    return p.returncode, (p.stdout or "") + (p.stderr or "")


def parse_listing(prefix: str, text: str) -> list[Entry]:
    entries: list[Entry] = []
    for line in text.splitlines():
        line = line.strip("\r")
        if not line.strip():
            continue
        typ, sep, path = line.partition("\t")
        if not sep or typ not in {"L", "D", "F"}:
            continue
        path = path.strip()
        family = classify(prefix, path)
        if family is None:
            continue
        rel = path[len(prefix) + 1:]
        found = STAMP_RE.findall(rel)
        entries.append(Entry(typ, path, family, rel, found[-1] if found else None))
    return entries


def decide(entries: list[Entry], keep: int) -> list[Entry]:
    ordered: list[Entry] = []
    for family in ("A", "B", "C"):
        stamped, nostamp, links = [], [], []
        for e in entries:
            if e.family != family:
                continue
            if e.typ == "L":
                e.action = "kept (symlink)"
                links.append(e)
            elif e.stamp is None:
                e.action = "kept (no stamp)"
                nostamp.append(e)
            else:
                stamped.append(e)
        stamped.sort(key=lambda e: stamp_key(e.stamp or ""), reverse=True)
        for i, e in enumerate(stamped):
            e.action = "keep" if i < keep else "DELETE"
        ordered.extend(stamped)
        ordered.extend(nostamp)
        ordered.extend(links)
    return ordered


def validate_delete(prefix: str, e: Entry) -> str | None:
    if not e.path.startswith(prefix + "/"):
        return "path is outside prefix"
    if ".." in e.path.split("/"):
        return "path contains .. segment"
    if any(ch.isspace() for ch in e.path):
        return "path contains whitespace"
    if any(ch in GLOB_CHARS for ch in e.path):
        return "path contains glob characters"
    if classify(prefix, e.path) != e.family:
        return "path does not match a backup family"
    if e.family in {"A", "B"} and e.typ != "D":
        return "family A/B delete requires a directory"
    if e.family == "C" and e.typ != "F":
        return "family C delete requires a file"
    return None


def delete_cmd(entries: list[Entry]) -> str:
    parts = []
    for e in entries:
        flag = "-rf" if e.typ == "D" else "-f"
        parts.append(f"rm {flag} -- {shlex.quote(e.path)}")
    return " && ".join(parts)


def process_target(t: Target, keep: int, yes: bool, timeout: float) -> bool:
    if t.kind == "windows-node":
        print(f"== {t.name} ({t.kind}) prefix={t.prefix} skipped (windows)")
        return True
    if t.kind not in HANDLED:
        print(f"== {t.name} ({t.kind}) prefix={t.prefix} skipped ({t.kind})")
        return True
    mode = "DELETING" if yes else "dry-run"
    print(f"== {t.name} ({t.kind}) prefix={t.prefix} {mode}")
    if not t.prefix or t.prefix == "/" or "sessiondock" not in t.prefix:
        print("error: refusing prefix (empty, /, or missing sessiondock)")
        return False
    rc, out = run_on_target(t, listing_cmd(t.prefix), timeout)
    if rc != 0:
        print(f"unreachable: rc={rc}")
        if out.strip():
            print(out.rstrip())
        return False
    decided = decide(parse_listing(t.prefix, out), keep)
    n_keep = n_del = n_ns = n_sy = 0
    to_delete: list[Entry] = []
    for e in decided:
        print(f"{e.action:<16} {e.family} {e.rel}")
        if e.action == "keep":
            n_keep += 1
        elif e.action == "DELETE":
            n_del += 1
            to_delete.append(e)
        elif e.action == "kept (no stamp)":
            n_ns += 1
        else:
            n_sy += 1
    print(
        f"summary keep={n_keep} DELETE={n_del} "
        f"kept (no stamp)={n_ns} kept (symlink)={n_sy}"
    )
    if not yes or not to_delete:
        return True
    for e in to_delete:
        err = validate_delete(t.prefix, e)
        if err:
            print(f"error: refusing to delete {e.path}: {err}")
            return False
    drc, dout = run_on_target(t, delete_cmd(to_delete), timeout)
    print(f"delete rc={drc}")
    if drc != 0 and dout.strip():
        print(dout.rstrip())
    return drc == 0


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(description="List and prune SessionDock deployment backups")
    p.add_argument("--targets-file", default="deploy/targets.local.json")
    p.add_argument("--targets", default="", help="comma-separated names (default: all)")
    p.add_argument("--keep", type=int, default=3, metavar="N")
    p.add_argument("--yes", action="store_true", help="actually delete (otherwise dry run)")
    p.add_argument("--timeout", type=float, default=60, metavar="SECONDS")
    args = p.parse_args(argv)
    if args.keep < 0:
        p.error("--keep must be >= 0")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    path = Path(args.targets_file)
    if not path.is_file():
        print(f"error: targets file not found: {path}", file=sys.stderr)
        return 1
    targets = load_targets(path)
    if args.targets.strip():
        wanted = [n.strip() for n in args.targets.split(",") if n.strip()]
        missing = [n for n in wanted if n not in {t.name for t in targets}]
        if missing:
            print("error: unknown target(s): " + ", ".join(missing), file=sys.stderr)
            return 1
        allow = set(wanted)
        targets = [t for t in targets if t.name in allow]
    ok = True
    for t in targets:
        if not process_target(t, args.keep, args.yes, args.timeout):
            ok = False
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
