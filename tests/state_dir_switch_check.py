#!/usr/bin/env python3
# run_validation: skip
"""Validate a new empty SESSIONDOCK_STATE_DIR before switching.

Read-only operator tool (not a suite; never starts the server). Batch-35
directory rules it asserts: the candidate exists as a real directory,
mode exactly 0700, owned by this uid, with no symlink in it or in any
ancestor; it is empty and does not contain Python `session-meta.json`;
it is not equal to, inside, or containing any other SESSIONDOCK_* path
from the env file (STATE_DIR, DELIVERY_DIR, LIFECYCLE_DIR, PTYHOST_DIR,
AUDIT_DIR, TRASH_DIR, WEB_DIR, *_ROOT, FILE_ROOTS split on ":",
LAUNCHER_CONFIG's parent) and is not under ~/.local/share/agenthub,
~/.claude, ~/.codex or ~/.grok. The old STATE_DIR is reported only
(exists / session-metadata.json / .metadata.lock).
"""
from __future__ import annotations

import argparse
import json
import os
import stat
import sys
from pathlib import Path

NAMED = (
    "SESSIONDOCK_STATE_DIR", "SESSIONDOCK_DELIVERY_DIR", "SESSIONDOCK_LIFECYCLE_DIR",
    "SESSIONDOCK_PTYHOST_DIR", "SESSIONDOCK_AUDIT_DIR", "SESSIONDOCK_TRASH_DIR",
    "SESSIONDOCK_WEB_DIR",
)
ROWS: list[dict] = []


def emit(area: str, ok: bool, why: str) -> bool:
    print(f"{'PASS' if ok else 'FAIL'} {area}" + ("" if ok and not why else f": {why}"), flush=True)
    ROWS.append({"id": area, "status": "PASS" if ok else "FAIL", "detail": why})
    return ok


def abs_norm(p: str | Path) -> Path:
    return Path(os.path.abspath(str(p)))


def related(a: Path, b: Path) -> str | None:
    a, b = abs_norm(a), abs_norm(b)
    if a == b:
        return "equal"
    try:
        a.relative_to(b)
        return "inside"
    except ValueError:
        pass
    try:
        b.relative_to(a)
        return "contains"
    except ValueError:
        return None


def blocked_homes() -> list[tuple[str, Path]]:
    home = Path.home()
    return [
        ("~/.local/share/agenthub", home / ".local/share/agenthub"),
        ("~/.claude", home / ".claude"),
        ("~/.codex", home / ".codex"),
        ("~/.grok", home / ".grok"),
    ]


def load_env(path: Path) -> tuple[str, dict[str, str]]:
    text = path.read_text(encoding="utf-8")
    out: dict[str, str] = {}
    for raw in text.splitlines():
        s = raw.strip()
        if s.startswith("export "):
            s = s[7:].strip()
        if not s or s.startswith("#") or "=" not in s:
            continue
        key, _, value = s.partition("=")
        key, value = key.strip(), value.strip().strip("\"'")
        if key:
            out[key] = value
    return text, out


def rewrite_env(text: str, new_dir: str) -> str:
    lines, seen = [], False
    for line in text.splitlines(keepends=True):
        nl = "\n" if line.endswith("\n") else ""
        body = line[:-1] if nl else line
        stripped = body.lstrip()
        indent = body[: len(body) - len(stripped)]
        core = stripped[7:].lstrip() if stripped.startswith("export ") else stripped
        export = "export " if stripped.startswith("export ") else ""
        if core.startswith("SESSIONDOCK_STATE_DIR="):
            lines.append(f"{indent}{export}SESSIONDOCK_STATE_DIR={new_dir}{nl}")
            seen = True
        else:
            lines.append(line)
    if not seen:
        blob = "".join(lines)
        if blob and not blob.endswith("\n"):
            lines.append("\n")
        lines.append(f"SESSIONDOCK_STATE_DIR={new_dir}\n")
    return "".join(lines)


def env_paths(env: dict[str, str]) -> list[tuple[str, str]]:
    items: list[tuple[str, str]] = []
    seen: set[tuple[str, str]] = set()

    def add(label: str, value: str) -> None:
        if not value:
            return
        key = (label, value)
        if key not in seen:
            seen.add(key)
            items.append((label, value))

    for name in NAMED:
        add(name, env.get(name, ""))
    for key, value in env.items():
        if key.startswith("SESSIONDOCK_") and key.endswith("_ROOT"):
            add(key, value)
    for index, part in enumerate((env.get("SESSIONDOCK_FILE_ROOTS") or "").split(":")):
        add(f"SESSIONDOCK_FILE_ROOTS[{index}]", part)
    launcher = env.get("SESSIONDOCK_LAUNCHER_CONFIG")
    if launcher:
        add("SESSIONDOCK_LAUNCHER_CONFIG.parent", str(Path(launcher).parent))
    return items


def under_blocked(path: Path) -> str | None:
    for label, home in blocked_homes():
        rel = related(path, home)
        if rel in ("equal", "inside"):
            return label
    return None


def check_new_dir(path: Path) -> str | None:
    try:
        info = os.lstat(path)
    except FileNotFoundError:
        return "does not exist"
    except OSError as exc:
        return str(exc)
    if stat.S_ISLNK(info.st_mode):
        return "is a symlink"
    if not stat.S_ISDIR(info.st_mode):
        return "not a directory"
    mode = stat.S_IMODE(info.st_mode)
    if mode != 0o700:
        return f"mode {oct(mode)} want 0o700"
    uid = os.getuid()
    if info.st_uid != uid:
        return f"owner uid {info.st_uid} want {uid}"
    current = abs_norm(path)
    while True:
        try:
            ancestor = os.lstat(current)
        except OSError as exc:
            return f"cannot lstat ancestor {current}: {exc}"
        if stat.S_ISLNK(ancestor.st_mode):
            return f"symlink ancestor {current}"
        parent = current.parent
        if parent == current:
            break
        current = parent
    try:
        names = os.listdir(path)
    except OSError as exc:
        return f"cannot list: {exc}"
    for name in names:
        try:
            child = os.lstat(path / name)
        except OSError as exc:
            return f"cannot lstat {name}: {exc}"
        if stat.S_ISLNK(child.st_mode):
            return f"contains symlink {name}"
    return None


def check_empty(path: Path) -> str | None:
    try:
        names = sorted(os.listdir(path))
    except OSError as exc:
        return str(exc)
    if "session-meta.json" in names:
        return "contains session-meta.json"
    if names:
        return "not empty: " + ", ".join(names[:16])
    return None


def check_disjoint(path: Path, env: dict[str, str]) -> str | None:
    blocked = under_blocked(path)
    if blocked:
        return f"under {blocked}"
    for label, value in env_paths(env):
        rel = related(path, Path(value))
        if rel:
            return f"{rel} {label}={value}"
    return None


def report_old(env: dict[str, str]) -> str:
    raw = env.get("SESSIONDOCK_STATE_DIR")
    if not raw:
        return "SESSIONDOCK_STATE_DIR unset"
    old = Path(raw)
    if under_blocked(old):
        return f"{raw} under a CLI/Python home; not inspected"
    try:
        info = os.lstat(old)
        exists = stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode)
    except FileNotFoundError:
        exists = False
    except OSError as exc:
        return f"{raw} lstat error: {exc}"
    meta = lock = False
    if exists:
        try:
            names = set(os.listdir(old))
        except OSError as exc:
            return f"{raw} exists=True listdir error: {exc}"
        meta, lock = "session-metadata.json" in names, ".metadata.lock" in names
    return f"exists={exists} session-metadata.json={meta} .metadata.lock={lock}"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env-file", required=True, help="KEY=VALUE env file (read-only)")
    parser.add_argument("--new-state-dir", required=True, help="candidate empty state directory")
    parser.add_argument("--json", help="write a JSON report to PATH")
    parser.add_argument("--print-env", action="store_true",
                        help="print env with SESSIONDOCK_STATE_DIR replaced (stdout only)")
    args = parser.parse_args(argv)
    env_path, new_dir = Path(args.env_file), Path(args.new_state_dir)
    try:
        text, env = load_env(env_path)
    except OSError as exc:
        emit("env_file", False, str(exc))
        return 1
    ok = True
    err = check_new_dir(new_dir)
    ok &= emit("new_dir", err is None, err or "exists, directory, mode 0700, owner uid, no symlink")
    err = check_empty(new_dir)
    ok &= emit("empty", err is None, err or "empty, no session-meta.json")
    err = check_disjoint(new_dir, env)
    ok &= emit("disjoint", err is None, err or "disjoint from env SESSIONDOCK_* paths and CLI homes")
    emit("old_state", True, report_old(env))
    if args.json:
        Path(args.json).write_text(json.dumps({
            "env_file": str(env_path), "new_state_dir": str(abs_norm(new_dir)),
            "checks": ROWS, "ok": ok,
        }, ensure_ascii=False) + "\n", encoding="utf-8")
    if args.print_env and ok:
        sys.stdout.write(rewrite_env(text, str(abs_norm(new_dir))))
        sys.stdout.flush()
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
