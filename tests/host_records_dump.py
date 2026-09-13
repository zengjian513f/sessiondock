#!/usr/bin/env python3
"""List sanitized ptyhost records from an explicit host directory.

Reads only JSON record files for isolated test debugging. Never connects,
never writes, and never prints tokens, endpoint addresses, argv, environment,
or unknown field values. Refuse $HOME/.local/share and paths containing
sessiondock/host unless --i-know-this-is-a-development-dir is passed.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import json
import os
import stat
import sys
from pathlib import Path

MAX_BYTES = 4 * 1024 * 1024
MAX_ENTRIES = 10_000
SOURCES = {"claude", "codex", "grok"}
INVALID = set('/\\:<>"|?*')
RESERVED = {"CON", "PRN", "AUX", "NUL"}
HEADERS = (
    "name", "host_pid", "pid", "created", "size", "cwd",
    "source", "sid", "uid", "instance", "endpoint",
)


def die(message, code=2):
    print(message, file=sys.stderr)
    raise SystemExit(code)


def valid_name(name):
    if not name or len(name) > 128 or name.startswith(".") or name.endswith((".", " ")):
        return False
    if any(ord(c) < 32 or c == "\x7f" or c in INVALID for c in name):
        return False
    stem = name.split(".", 1)[0].upper()
    if stem in RESERVED:
        return False
    return not (len(stem) == 4 and stem[:3] in {"COM", "LPT"} and stem[3] in "123456789")


def blocked(path: Path) -> bool:
    share = os.path.abspath(os.path.join(os.path.expanduser("~"), ".local", "share"))
    prefix = share.rstrip(os.sep) + os.sep
    texts = [str(path), os.path.abspath(path)]
    try:
        texts.append(os.path.realpath(path))
    except OSError:
        pass
    for text in texts:
        if "sessiondock/host" in text.replace("\\", "/"):
            return True
        absn = os.path.abspath(text)
        if absn == share or absn.startswith(prefix):
            return True
    return False


def as_int(value):
    return value if isinstance(value, int) and not isinstance(value, bool) else 0


def mask_cwd(cwd):
    if not cwd or not isinstance(cwd, str):
        return ""
    names = [part for part in Path(cwd).parts if part not in ("/", "\\")]
    return "/".join(names[-2:])


def mask_id(value, keep=6):
    if not value:
        return ""
    if len(value) <= keep:
        return value[: max(1, len(value) // 2)] + "..."
    return value[:keep] + "..."


def reviewed(meta):
    if not isinstance(meta, dict):
        return "", "", "", ""
    source = meta.get("source") if isinstance(meta.get("source"), str) else ""
    sid = meta.get("sid") if isinstance(meta.get("sid"), str) else ""
    uid = meta.get("uid") if isinstance(meta.get("uid"), str) else ""
    inst = meta.get("instance_id") if isinstance(meta.get("instance_id"), str) else ""
    return source if source in SOURCES else "", mask_id(sid), mask_id(uid), inst[:8]


def load_record(directory: Path, path: Path):
    name = path.stem
    if path.suffix != ".json" or not valid_name(name):
        return None
    try:
        info = path.lstat()
        raw = path.read_bytes() if stat.S_ISREG(info.st_mode) and info.st_size <= MAX_BYTES else b""
        data = json.loads(raw) if raw and len(raw) <= MAX_BYTES else None
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(data, dict) or data.get("name") != name:
        return None
    source, sid, uid, inst = reviewed(data.get("meta"))
    sock = directory / f"{name}.sock"
    try:
        exists = stat.S_ISSOCK(sock.lstat().st_mode)
    except OSError:
        exists = False
    return {
        "name": name, "host_pid": as_int(data.get("host_pid")),
        "pid": as_int(data.get("pid")), "created": as_int(data.get("created")),
        "cols": as_int(data.get("cols")), "rows": as_int(data.get("rows")),
        "cwd": mask_cwd(data.get("cwd")), "source": source, "sid": sid,
        "uid": uid, "instance": inst, "endpoint": exists,
    }


def collect(directory: Path):
    try:
        entries = sorted(directory.iterdir(), key=lambda item: item.name)
    except OSError as err:
        die(f"cannot read directory: {err}")
    if len(entries) > MAX_ENTRIES:
        die("host directory exceeds entry limit")
    return [row for path in entries if (row := load_record(directory, path))]


def render(rows):
    lines = ["| " + " | ".join(HEADERS) + " |",
             "| " + " | ".join("---" for _ in HEADERS) + " |"]
    for row in rows:
        cells = [
            row["name"], str(row["host_pid"]), str(row["pid"]), str(row["created"]),
            f"{row['cols']}×{row['rows']}", row["cwd"], row["source"],
            row["sid"], row["uid"], row["instance"],
            "true" if row["endpoint"] else "false",
        ]
        lines.append("| " + " | ".join(cells) + " |")
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Dump sanitized ptyhost records from an explicit directory.")
    parser.add_argument("--dir", required=True, metavar="DIR",
                        help="explicit host directory (no default)")
    parser.add_argument("--json", action="store_true", help="print sanitized rows as JSON")
    parser.add_argument("--i-know-this-is-a-development-dir", action="store_true",
                        dest="dev_dir", help="allow otherwise refused development paths")
    args = parser.parse_args(argv)
    directory = Path(args.dir)
    if blocked(directory) and not args.dev_dir:
        die("refusing $HOME/.local/share or sessiondock/host; "
            "pass --i-know-this-is-a-development-dir")
    if not directory.is_dir():
        die(f"not a directory: {args.dir}")
    rows = collect(directory)
    print(json.dumps(rows, ensure_ascii=False, indent=2) if args.json else render(rows))


if __name__ == "__main__":
    main()
