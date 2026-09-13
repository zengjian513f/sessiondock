#!/usr/bin/env python3
"""Scan git-tracked or staged files for commit-hygiene violations.

Enforces AGENTS.md: never commit deployment addresses, personal absolute
paths, credentials, runtime data, build outputs, or local environment files.
Placeholders /home/example, /home/u/, /opt/example and /srv/example are allowed.
Loopback 127.0.0.0/8 is allowed; other IPv4 is not.

CLI: check_commit_hygiene.py [--staged] [--paths FILE…] [--allow FILE] [--json]
"""
# run_validation: skip
import argparse
import fnmatch
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MAX_BYTES = 2 * 1024 * 1024
SKIP_PARTS = frozenset({"vendor", "fonts", "images"})
SKIP_SUFFIX = frozenset({
    ".lock", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".bmp",
    ".tif", ".tiff", ".woff", ".woff2", ".ttf", ".otf", ".eot", ".svg",
})
ALLOWED_HOME = frozenset({"example", "u"})
ARTIFACT_PARTS = frozenset({"target", "node_modules", ".runtime"})
HOME_RE = re.compile(r"(?<![A-Za-z0-9])(/(?:home|Users)/([^/\s\"'`]+)(?:/[^\s\"'`]*)?)")
WIN_RE = re.compile(r"([A-Za-z]:\\+Users\\+[^\s\"'`]*)", re.I)
IP_RE = re.compile(
    r"\b((?:(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d))\b"
)
CRED_RE = re.compile(
    r"sk-[A-Za-z0-9]{16,}|AKIA[0-9A-Z]{16}|ghp_[A-Za-z0-9]{20,}|"
    r"xoxb-[A-Za-z0-9-]{8,}|-----BEGIN [A-Z ]*PRIVATE KEY-----"
)
TOKEN_RE = re.compile(r"token=[A-Za-z0-9]{24,}")


def relpath(path: Path) -> str:
    try:
        return path.resolve().relative_to(ROOT.resolve()).as_posix()
    except ValueError:
        return path.as_posix()


def git_names(staged: bool) -> list[str]:
    git = ["git", "-C", str(ROOT)]
    cmd = git + (["diff", "--cached", "--name-only", "-z"] if staged else ["ls-files", "-z"])
    return [item for item in subprocess.check_output(cmd, text=True).split("\0") if item]


def skip_path(rel: str) -> bool:
    path = Path(rel)
    return bool(set(path.parts) & SKIP_PARTS) or path.suffix.lower() in SKIP_SUFFIX


def is_env(rel: str) -> bool:
    name = Path(rel).name
    return name.endswith(".local") or name == ".env" or (
        name.startswith(".env.") and name != ".env.example"
    )


def load_allow(path: Path) -> list[str]:
    return [ln.strip() for ln in path.read_text(encoding="utf-8").splitlines()
            if ln.strip() and not ln.strip().startswith("#")]


def is_allowed(rel: str, kind: str, patterns: list[str]) -> bool:
    keys = (rel, f"{rel}:{kind}")
    for pat in patterns:
        for key in keys:
            if fnmatch.fnmatch(key, pat):
                return True
            try:
                if re.search(pat, key):
                    return True
            except re.error:
                pass
    return False


def redact(span: str) -> str:
    span = " ".join(span.split())
    if len(span) <= 8:
        return span[:2] + "…"
    return span[:4] + "…" + span[-3:]


def hit(rel: str, number: int, kind: str, span: str) -> dict:
    return {"file": rel, "line": number, "kind": kind, "excerpt": redact(span)}


def private_ip(ip: str) -> bool:
    a, b, *_ = (int(p) for p in ip.split("."))
    return a == 10 or (a == 192 and b == 168) or (a == 172 and 16 <= b <= 31)


def scan_line(rel: str, number: int, line: str) -> list[dict]:
    items = []
    for match in HOME_RE.finditer(line):
        if match.group(2) not in ALLOWED_HOME:
            items.append(hit(rel, number, "personal-path", match.group(1)))
    for match in WIN_RE.finditer(line):
        items.append(hit(rel, number, "personal-path", match.group(1)))
    for match in IP_RE.finditer(line):
        ip = match.group(1)
        if ip.startswith("127."):
            continue
        kind = "private-ip" if private_ip(ip) else "non-loopback-ip"
        items.append(hit(rel, number, kind, ip))
    for match in CRED_RE.finditer(line):
        items.append(hit(rel, number, "credential", match.group(0)))
    if "tests" not in Path(rel).parts:
        for match in TOKEN_RE.finditer(line):
            items.append(hit(rel, number, "credential", match.group(0)))
    return items


def read_text(path: Path) -> str | None:
    try:
        if path.stat().st_size > MAX_BYTES:
            return None
        data = path.read_bytes()
    except OSError:
        return None
    if b"\0" in data:
        return None
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return None


def collect(rel: str, path: Path, tracked: bool) -> list[dict]:
    items = []
    if is_env(rel):
        items.append(hit(rel, 0, "env-file", rel))
    if tracked and set(Path(rel).parts) & ARTIFACT_PARTS:
        items.append(hit(rel, 0, "runtime-artifact", rel))
    text = read_text(path)
    if text is None:
        return items
    for number, line in enumerate(text.splitlines(), 1):
        items.extend(scan_line(rel, number, line))
    return items


def selected(args) -> list[tuple[str, Path, bool]]:
    if args.paths:
        out = []
        for item in args.paths:
            path = Path(item)
            path = path if path.is_absolute() else Path.cwd() / path
            out.append((relpath(path), path, False))
        return out
    return [(rel, ROOT / rel, True) for rel in git_names(args.staged)]


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--paths", nargs="+", metavar="FILE", help="scan these files (may be untracked)")
    parser.add_argument("--staged", action="store_true", help="limit to the git index")
    parser.add_argument("--allow", metavar="FILE", help="allowlist: one glob or regex per line")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    default_allow = ROOT / "tests/hygiene_allow.txt"
    allow_path = Path(args.allow) if args.allow else (default_allow if default_allow.is_file() else None)
    allow = load_allow(allow_path) if allow_path else []
    findings, checked = [], 0
    for rel, path, tracked in selected(args):
        if skip_path(rel) or not path.is_file():
            continue
        checked += 1
        for item in collect(rel, path, tracked):
            if not is_allowed(item["file"], item["kind"], allow):
                findings.append(item)
    findings.sort(key=lambda item: (item["file"], item["line"], item["kind"]))
    if args.json:
        json.dump({"checked": checked, "findings": findings}, sys.stdout, ensure_ascii=False)
        sys.stdout.write("\n")
    else:
        for item in findings:
            print(f"{item['file']}:{item['line']}: {item['kind']}: {item['excerpt']}")
        print(f"{len(findings)} finding(s)")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
