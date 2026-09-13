#!/usr/bin/env python3
"""Operator-run preflight for replacement-checklist.md §4 / runbook-dev.md cutover."""
# run_validation: skip
import argparse, errno, json, os, socket, subprocess, sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
PRIV = ("SESSIONDOCK_STATE_DIR", "SESSIONDOCK_DELIVERY_DIR", "SESSIONDOCK_LIFECYCLE_DIR",
        "SESSIONDOCK_AUDIT_DIR", "SESSIONDOCK_TRASH_DIR", "SESSIONDOCK_PTYHOST_DIR")
ROOTS = ("SESSIONDOCK_CLAUDE_ROOT", "SESSIONDOCK_CODEX_ROOT", "SESSIONDOCK_GROK_ROOT")
LISTS = ("SESSIONDOCK_FILE_ROOTS", "SESSIONDOCK_FILE_WRITE_ROOTS")
FILES = ("SESSIONDOCK_CODEX_INDEX", "SESSIONDOCK_LAUNCHER_CONFIG")
LOOP, ROWS = {"127.0.0.1", "localhost", "::1"}, []

def emit(st, cid, detail):
    print(f"{st} {cid} {detail}", flush=True)
    ROWS.append({"id": cid, "status": st, "detail": str(detail)})

def lex(p):
    p = Path(p)
    return p if p.is_absolute() else Path.cwd() / p

def under(a, b):
    try:
        lex(a).relative_to(lex(b)); return True
    except ValueError:
        return False

def blocked(p):
    h = Path.home()
    return any(under(p, b) for b in (
        h / ".local/share/sessiondock", h / ".claude", h / ".codex", h / ".grok",
        REPO.parent / "sessiondock"))

def path_items(env):
    for k, v in env.items():
        if k in ("SESSIONDOCK_BIND", "SESSIONDOCK_HOSTNAME") or k.endswith("_LIMITS"):
            continue
        yield k, v.split(os.pathsep) if k in LISTS else [v]

def load_file(path):
    out = {}
    for raw in Path(path).read_text(encoding="utf-8").splitlines():
        s = raw.strip()
        if s.startswith("export "):
            s = s[7:].strip()
        if not s or s.startswith("#") or "=" not in s:
            continue
        k, _, v = s.partition("=")
        k, v = k.strip(), v.strip()
        if k.startswith("SESSIONDOCK_"):
            out[k] = v
    return out

def chk_binary(path):
    b = Path(path)
    if not b.is_file() or not os.access(b, os.X_OK):
        emit("FAIL", "binary", f"{path} missing or not executable"); return False
    emit("PASS", "binary", f"{path} size={b.stat().st_size}"); return True

def chk_bind(env, allow):
    raw = env.get("SESSIONDOCK_BIND")
    if not raw:
        emit("WARN", "bind", "SESSIONDOCK_BIND unset; server default 127.0.0.1:8741")
        raw = "127.0.0.1:8741"
    try:
        t = raw.strip()
        if t.startswith("["):
            host, _, rest = t[1:].partition("]"); port = int(rest[1:])
        else:
            host, _, ps = t.rpartition(":"); port = int(ps)
        if not host:
            raise ValueError("invalid bind")
    except (TypeError, ValueError):
        emit("FAIL", "bind", f"invalid SESSIONDOCK_BIND {raw!r}"); return
    if host not in LOOP:
        emit("WARN" if allow else "FAIL", "bind", f"non-loopback bind {host}:{port}")
    hbind, fam = (("127.0.0.1", socket.AF_INET) if host == "localhost"
                  else (host, socket.AF_INET6 if ":" in host else socket.AF_INET))
    sock = socket.socket(fam, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 0)
    try:
        sock.bind((hbind, port, 0, 0) if fam == socket.AF_INET6 else (hbind, port))
        if host in LOOP:
            emit("PASS", "bind", f"{host}:{port} free")
    except OSError as exc:
        emit("FAIL", "bind", "port busy" if exc.errno == errno.EADDRINUSE else str(exc))
    finally:
        sock.close()

def chk_abs(env):
    fail = False
    for k, parts in path_items(env):
        for v in parts:
            if "$HOME" in v or "~" in v:
                emit("WARN", "paths_absolute", f"{k} contains unexpanded $HOME or ~")
            if not v or not Path(v).is_absolute() or ".." in Path(v).parts:
                emit("FAIL", "paths_absolute", f"{k} not absolute, has .., or empty: {v!r}")
                fail = True
    if not fail:
        emit("PASS", "paths_absolute", "all path values absolute without ..")

def chk_exist(env):
    fail = False
    for k, parts in path_items(env):
        want_file = k in FILES
        for v in parts:
            if not v or blocked(v):
                continue
            p = Path(v)
            if not p.exists():
                emit("FAIL", "paths_exist", f"{k} missing: {v}"); fail = True; continue
            if want_file and not p.is_file():
                emit("FAIL", "paths_exist", f"{k} not a regular file: {v}"); fail = True
            elif not want_file and not p.is_dir():
                emit("FAIL", "paths_exist", f"{k} not a directory: {v}"); fail = True
            r = p.resolve()
            if r != p:
                emit("WARN", "paths_exist", f"{k} symlink {p} -> {r}")
    if not fail:
        emit("PASS", "paths_exist", "configured paths exist with expected types")

def chk_priv(env):
    fail, priv = False, {k: env[k] for k in PRIV if k in env}
    for v in priv.values():
        if blocked(v) or not Path(v).is_dir():
            continue
        mode = Path(v).stat().st_mode
        if not (mode & 0o022 == 0 and mode & 0o700 == 0o700):
            emit("FAIL", "private_dirs", f"chmod 700 {v}"); fail = True
    others = [("SESSIONDOCK_WEB_DIR", env.get("SESSIONDOCK_WEB_DIR") or str(REPO / "legacy-web"))]
    others += [(k, env[k]) for k in ROOTS if k in env]
    for k in LISTS:
        others += [(k, p) for p in env.get(k, "").split(os.pathsep) if p]
    others += [(k, str(Path(env[k]).parent)) for k in FILES if env.get(k)]
    names = list(priv)
    for i, a in enumerate(names):
        for b in names[i + 1:]:
            if under(priv[a], priv[b]) or under(priv[b], priv[a]):
                emit("FAIL", "private_dirs", f"{a} overlaps {b}"); fail = True
    for a, av in priv.items():
        for b, bv in others:
            if bv and (under(av, bv) or under(bv, av)):
                emit("FAIL", "private_dirs", f"{a} overlaps {b}"); fail = True
    if "SESSIONDOCK_FILE_WRITE_ROOTS" in env:
        if "SESSIONDOCK_FILE_ROOTS" not in env:
            emit("FAIL", "private_dirs", "SESSIONDOCK_FILE_WRITE_ROOTS set without SESSIONDOCK_FILE_ROOTS")
            fail = True
        else:
            reads = [p for p in env["SESSIONDOCK_FILE_ROOTS"].split(os.pathsep) if p]
            for w in env["SESSIONDOCK_FILE_WRITE_ROOTS"].split(os.pathsep):
                if w and not any(under(w, r) for r in reads):
                    emit("FAIL", "private_dirs", f"{w} not inside SESSIONDOCK_FILE_ROOTS"); fail = True
    if not fail:
        emit("PASS", "private_dirs", "modes owner-rwx without group/world write; pairwise disjoint")

def chk_pydata(env):
    fail, h = False, Path.home()
    pydata, clis = h / ".local/share/sessiondock", (h / ".claude", h / ".codex", h / ".grok")
    for k in PRIV:
        if k not in env:
            continue
        v = env[k]
        if under(v, pydata):
            emit("FAIL", "python_data", f"{k} equals or lies inside {pydata}"); fail = True
        for c in clis:
            if under(v, c):
                emit("FAIL", "python_data", f"{k} lies inside {c}"); fail = True
    if not fail:
        emit("PASS", "python_data", "native roots may be real CLI homes; the server never writes there")

def chk_web(env):
    raw = env.get("SESSIONDOCK_WEB_DIR")
    if not raw or not Path(raw).is_absolute():
        emit("WARN", "web_dir", "set SESSIONDOCK_WEB_DIR to an absolute path")
        raw = str(REPO / (raw or "legacy-web"))
    if blocked(raw):
        emit("FAIL", "web_dir", f"{raw} is under a CLI/Python home"); return
    miss = [n for n in ("index.html", "app.js") if not (Path(raw) / n).is_file()]
    if miss:
        emit("FAIL", "web_dir", f"missing {', '.join(miss)} in {raw}")
    else:
        emit("PASS", "web_dir", f"{raw} has index.html and app.js")

def chk_state(env):
    raw = env.get("SESSIONDOCK_STATE_DIR")
    if not raw:
        emit("PASS", "state_docs", "STATE_DIR unset"); return
    if blocked(raw) or not Path(raw).is_dir():
        return
    names = sorted(p.name for p in Path(raw).iterdir())
    if "session-meta.json" in names:
        emit("FAIL", "state_docs", "Python metadata file in Rust state dir — run tests/meta_import.py into an empty directory instead")
    meta = Path(raw) / "session-metadata.json"
    if not meta.is_file():
        emit("PASS", "state_docs", "state_dir empty (fresh store)")
    else:
        mode, data = meta.stat().st_mode & 0o777, None
        try:
            data = json.loads(meta.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            emit("FAIL", "state_docs", f"session-metadata.json: {exc}")
        if data is not None:
            bad = (f"session-metadata.json mode {oct(mode)} want 0600" if mode != 0o600
                   else None if data.get("schema_version") == 1 else
                   f"schema_version {data.get('schema_version')!r} != 1")
            emit("FAIL" if bad else "PASS", "state_docs",
                 bad or "existing metadata document accepted")
    extra = [n for n in names if n not in ("session-metadata.json", "session-meta.json", ".metadata.lock")
             and not n.startswith(".metadata-tmp-")]
    if extra:
        emit("WARN", "state_docs", "unexpected entries: " + ", ".join(extra[:16]))

def chk_cfg(binary, env):
    merged = {"PATH": os.environ.get("PATH", ""), **env}
    try:
        proc = subprocess.run([str(binary), "--check-config"], env=merged,
                              capture_output=True, text=True, timeout=20)
    except (subprocess.TimeoutExpired, OSError) as exc:
        emit("FAIL", "check_config", "timeout 20s" if isinstance(exc, subprocess.TimeoutExpired) else str(exc))
        return
    out, err = (proc.stdout or "").strip(), (proc.stderr or "").strip()
    lines = out.splitlines()
    if proc.returncode != 0 or not lines or lines[0] != "config=ok":
        if "usage" in f"{out}\n{err}".lower() and "config=ok" not in out:
            emit("FAIL", "check_config", "stale binary: --check-config printed usage instead of config=ok")
            return
        emit("FAIL", "check_config", (err.splitlines() or lines or ["check-config failed"])[-1])
        return
    emit("PASS", "check_config", "config=ok")
    for line in lines[1:]:
        if "=" in line:
            print(f"    {line}", flush=True)

def main():
    p = argparse.ArgumentParser(description="Preflight SESSIONDOCK_* before starting sessiondock")
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--env-file"); g.add_argument("--from-env", action="store_true")
    p.add_argument("--binary", default="target/release/sessiondock")
    p.add_argument("--allow-non-loopback", action="store_true"); p.add_argument("--json")
    args = p.parse_args()
    env = ({k: v for k, v in os.environ.items() if k.startswith("SESSIONDOCK_")}
           if args.from_env else load_file(args.env_file))
    ok = chk_binary(args.binary)
    chk_bind(env, args.allow_non_loopback); chk_abs(env); chk_exist(env); chk_priv(env)
    chk_pydata(env); chk_web(env); chk_state(env)
    if ok:
        chk_cfg(args.binary, env)
    npass = sum(r["status"] == "PASS" for r in ROWS)
    nwarn = sum(r["status"] == "WARN" for r in ROWS)
    nfail = sum(r["status"] == "FAIL" for r in ROWS)
    print(f"SUMMARY pass={npass} warn={nwarn} fail={nfail}", flush=True)
    if args.json:
        Path(args.json).write_text(json.dumps({
            "checks": ROWS, "summary": {"pass": npass, "warn": nwarn, "fail": nfail},
        }, ensure_ascii=False) + "\n", encoding="utf-8")
    raise SystemExit(1 if nfail else 0)

if __name__ == "__main__":
    main()
