#!/usr/bin/env python3
"""Map every Python node-service config knob to its SESSIONDOCK_* equivalent."""
# run_validation: skip
import argparse, json, re, sys
from pathlib import Path
import env_reference

ROOT = Path(__file__).resolve().parents[1]
PY = ROOT.parent / "sessiondock"
MODS = ("server.py", "term.py", "term_host.py", "files.py", "media.py",
        "audit.py", "trash.py", "session_meta.py")
HEAD = ("Python knob", "Source", "Purpose (from help/comment)", "Rust variable", "Status")
NONE = "no equivalent (Rust deliberately none)"
LOOP = "partial (combined SocketAddr; loopback only)"
PTY = "partial (explicit dir; ptyhost only, no tmux/auto/boolean)"
HUB = "no equivalent (Rust has no hub federation)"
OSN = "no equivalent (OS env; Rust deliberately none)"
MAP = {
    "--host": ("SESSIONDOCK_BIND", LOOP), "--port": ("SESSIONDOCK_BIND", LOOP),
    "PORT": ("SESSIONDOCK_BIND", LOOP),
    "--allow": ("—", "no equivalent (Rust deliberately loopback-only)"),
    "ALLOW": ("—", "no equivalent (Rust deliberately loopback-only)"),
    "--terminal": ("SESSIONDOCK_PTYHOST_DIR", PTY),
    "--terminal-backend": ("SESSIONDOCK_PTYHOST_DIR", PTY),
    "SESSIONDOCK_TERM_BACKEND": ("SESSIONDOCK_PTYHOST_DIR", PTY),
    "--node-token-file": ("SESSIONDOCK_NODE_TOKEN_FILE", "node listener credential (batch 38 H1; with NODE_BIND/NODE_ID_FILE/NODE_PEERS)"),
    "--node-id-file": ("SESSIONDOCK_NODE_ID_FILE", "node identity file, minted O_EXCL 0600 on first start (batch 38 H1)"),
    "SESSIONDOCK_HOST_BIN": ("—", "no equivalent (Rust does not discover a host binary)"),
    "SESSIONDOCK_HOST_ENV_WRAPPER": ("—", "no equivalent (login-shell wrapper not reproduced)"),
    "SESSIONDOCK_HOST_SCOPE": ("—", "no equivalent (Rust does not spawn systemd scopes)"),
    "PATH": ("—", OSN), "PATHEXT": ("—", OSN),
    "XDG_RUNTIME_DIR": ("—", OSN), "DBUS_SESSION_BUS_ADDRESS": ("—", OSN),
}
FALL = {
    "PORT": "run.sh --port", "ALLOW": "run.sh --allow",
    "--host": "bind host", "--port": "bind port",
    "SESSIONDOCK_HOST_ENV_WRAPPER": "optional login-shell wrapper around ptyhost",
    "SESSIONDOCK_HOST_SCOPE": "set 0 to disable systemd-run scope",
    "XDG_RUNTIME_DIR": "user runtime dir for systemd-run probe",
    "DBUS_SESSION_BUS_ADDRESS": "session bus for systemd-run probe",
    "PATH": "executable search path", "PATHEXT": "Windows executable suffixes",
}
ENV_RE = re.compile(
    r"(?:os\.getenv|os\.environ\.get|os\.environ\[)\s*\(?\s*['\"]([A-Z][A-Z0-9_]*)['\"]")
SH_RE = re.compile(r"\$\{([A-Z][A-Z0-9_]+)(?::-[^}]*)?\}")
HELP_RE = re.compile(r"help\s*=\s*((?:['\"](?:\\.|[^'\"\\])*['\"]\s*)+)")
STR_RE = re.compile(r"['\"]((?:\\.|[^'\"\\])*)['\"]")
FLAG_RE = re.compile(r"['\"](--[A-Za-z0-9-]+)['\"]")

def clip(text, n=96):
    text = " ".join((text or "").split())
    return text if len(text) <= n else text[: n - 1].rstrip() + "…"

def close(text, i):
    depth, n = 0, len(text)
    while i < n:
        c = text[i]
        if c in "\"'":
            q = c; i += 1
            while i < n and text[i] != q:
                i += 2 if text[i] == "\\" else 1
            i += 1; continue
        if c == "#" and depth:
            nl = text.find("\n", i); i = n if nl < 0 else nl + 1; continue
        depth += (c == "(") - (c == ")")
        if c == ")" and depth == 0:
            return i
        i += 1
    return -1

def read(path):
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return ""

def named_comment(text, name):
    for match in re.finditer(r"^[ \t]*#[^\n]*", text, re.M):
        if name in match.group(0):
            return clip(match.group(0).lstrip(" \t#"))
    for match in re.finditer(r'"""(.*?)"""', text, re.S):
        if name not in match.group(1):
            continue
        for sent in re.split(r"(?<=[。.\n])", match.group(1)):
            if name in sent:
                return clip(sent)
    return FALL.get(name, "")

def around(text, pos, name):
    chunk = text[max(0, pos - 1200):pos]
    for match in reversed(list(re.finditer(r'"""(.*?)"""', chunk, re.S))):
        body = " ".join(match.group(1).split())
        if name in body:
            return clip(body)
    for line in reversed(chunk.splitlines()):
        s = line.strip()
        if s.startswith("#") and name in s:
            return clip(s.lstrip("# "))
        if s.startswith(("def ", "class ")):
            break
    return named_comment(text, name)

def scan(text, source, shell=False):
    rows = []
    if not shell:
        i = 0
        while True:
            j = text.find("add_argument(", i)
            if j < 0:
                break
            k = close(text, j + 12)
            if k < 0:
                break
            blob, i = text[j:k + 1], k + 1
            flags = FLAG_RE.findall(blob)
            if not flags:
                continue
            hm = HELP_RE.search(blob)
            purpose = "".join(STR_RE.findall(hm.group(1))) if hm else around(text, j, flags[0])
            rows.append((flags[0], source, clip(purpose) or FALL.get(flags[0], "CLI flag")))
    seen = {item[0] for item in rows}
    for match in (SH_RE if shell else ENV_RE).finditer(text):
        name = match.group(1)
        if name in seen:
            continue
        seen.add(name)
        rows.append((name, source, around(text, match.start(), name) or "environment variable"))
    return rows

def python_knobs():
    rows = scan(read(PY / "run.sh"), "run.sh", True)
    for name in MODS:
        rows.extend(scan(read(PY / "sessiondock" / name), name))
    return rows

def collect_rows():
    used, out = set(), []
    for knob, source, purpose in python_knobs():
        rust, status = MAP.get(knob, ("—", NONE))
        if rust.startswith("SESSIONDOCK_"):
            used.add(rust)
        out.append(dict(python=knob, source=source, purpose=purpose, rust=rust, status=status))
    try:
        rust_rows = env_reference.collect()
    except (OSError, ValueError):
        rust_rows = []
    for item in rust_rows:
        if item["var"] in used:
            continue
        purpose = clip(f"{item.get('field') or ''}: {item.get('validation') or ''}".strip(": "))
        out.append(dict(python="—", source="config.rs", rust=item["var"],
                        status="Rust-only", purpose=purpose or "Rust-only"))
    return out

def render(items):
    lines = ["| " + " | ".join(HEAD) + " |", "| " + " | ".join("---" for _ in HEAD) + " |"]
    keys = ("python", "source", "purpose", "rust", "status")
    for item in items:
        lines.append("| " + " | ".join(item[k].replace("|", "\\|") for k in keys) + " |")
    return "\n".join(lines) + "\n"

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="print {rows:[...]} as JSON")
    args = parser.parse_args(argv)
    items = collect_rows()
    try:
        if args.json:
            json.dump({"rows": items}, sys.stdout, ensure_ascii=False, indent=2)
            sys.stdout.write("\n")
        else:
            sys.stdout.write(render(items))
    except BrokenPipeError:
        pass
    return 0

if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BrokenPipeError:
        raise SystemExit(0)
