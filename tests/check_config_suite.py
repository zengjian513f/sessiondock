#!/usr/bin/env python3
"""Startup-validation matrix through `sessiondock --check-config` (no server start, no Chromium)."""
from __future__ import annotations

import argparse
import os
import socket
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
WEB = str(REPO / "legacy-web")


def default_binary():
    release = REPO / "target/release/sessiondock"
    debug = REPO / "target/debug/sessiondock"
    return str(release if release.is_file() else debug)


def mkdir(path, mode=None):
    os.mkdir(path) if mode is None else os.mkdir(path, mode)
    return path


def base(**extra):
    env = {"SESSIONDOCK_BIND": "127.0.0.1:0", "SESSIONDOCK_WEB_DIR": WEB}
    env.update(extra)
    return env


def make_web(tmp):
    web = mkdir(os.path.join(tmp, "web"))
    Path(web, "index.html").write_text("<!doctype html>\n", encoding="utf-8")
    return web


def invoke(binary, env):
    try:
        proc = subprocess.run(
            [binary, "--check-config"],
            env={"PATH": os.environ["PATH"], **env},
            capture_output=True, text=True, encoding="utf-8", errors="replace",
            timeout=15,
        )
    except subprocess.TimeoutExpired:
        return 124, "", "timeout 15s"
    return proc.returncode, proc.stdout, proc.stderr


def last_line(text):
    lines = [line for line in text.splitlines() if line.strip()]
    return lines[-1] if lines else ""


def fail(name, expected, code, stderr):
    print(f"FAIL {name}: expected {expected} got exit={code} stderr={last_line(stderr)}", flush=True)
    raise SystemExit(1)


def check(name, code, out, err, want, needle, extras):
    blob = out + err
    if "usage:" in blob.lower() and "config=ok" not in out:
        print("stale binary: --check-config printed usage instead of config=ok", flush=True)
        raise SystemExit(1)
    if want == 0:
        missing = [item for item in extras if item not in out]
        if code != 0 or missing:
            fail(name, f"exit=0 stdout has {extras}", code, err)
        return
    expected = f"exit={want}" + (f" stderr contains {needle!r}" if needle else "")
    if code != want or (needle and needle not in err):
        fail(name, expected, code, err)


def real(path):
    return os.path.realpath(path)


def builders():
    def minimal_ok(_tmp):
        # Unset optionals stay disabled; "only loopback binds are allowed".
        return base(), 0, None, ["config=ok", "state_dir=(unset)"]

    def roots_ok(tmp):
        # "must be a directory" — canonicalize existing *_ROOT dirs and echo them.
        claude, codex, grok = (mkdir(os.path.join(tmp, n)) for n in ("claude", "codex", "grok"))
        return (
            base(SESSIONDOCK_CLAUDE_ROOT=claude, SESSIONDOCK_CODEX_ROOT=codex,
                 SESSIONDOCK_GROK_ROOT=grok),
            0, None,
            [f"claude_root={real(claude)}", f"codex_root={real(codex)}", f"grok_root={real(grok)}"],
        )

    def non_loopback_bind(_tmp):
        # "only loopback binds are allowed"
        return base(SESSIONDOCK_BIND="0.0.0.0:8741"), 1, "loopback", []

    def invalid_bind(_tmp):
        # "invalid SESSIONDOCK_BIND"
        return base(SESSIONDOCK_BIND="not-an-address"), 1, "SESSIONDOCK_BIND", []

    def empty_root(_tmp):
        # "must not be empty"
        return base(SESSIONDOCK_CLAUDE_ROOT=""), 1, "must not be empty", []

    def root_is_file(tmp):
        # "must be a directory"
        path = os.path.join(tmp, "notdir")
        Path(path).write_text("x", encoding="utf-8")
        return base(SESSIONDOCK_CLAUDE_ROOT=path), 1, "must be a directory", []

    def state_overlaps_root(tmp):
        # "metadata requires an independent directory outside host and native inputs"
        same = mkdir(os.path.join(tmp, "same"))
        return base(SESSIONDOCK_CLAUDE_ROOT=same, SESSIONDOCK_STATE_DIR=same), 1, "independent", []

    def state_inside_web(tmp):
        # "frontend and private runtime/native directories must not overlap"
        web = make_web(tmp)
        state = mkdir(os.path.join(web, "state"), 0o700)
        return (
            {"SESSIONDOCK_BIND": "127.0.0.1:0", "SESSIONDOCK_WEB_DIR": web,
             "SESSIONDOCK_STATE_DIR": state},
            1, "overlap", [],
        )

    def delivery_missing_dir(tmp):
        # "SESSIONDOCK_DELIVERY_DIR requires an explicit existing absolute directory"
        return base(SESSIONDOCK_DELIVERY_DIR=os.path.join(tmp, "missing")), 1, "SESSIONDOCK_DELIVERY_DIR", []

    def delivery_relative(_tmp):
        # same rule: not absolute → "explicit existing absolute directory without relative jumps"
        return base(SESSIONDOCK_DELIVERY_DIR="relative/dir"), 1, None, []

    def file_write_without_read(tmp):
        # "SESSIONDOCK_FILE_WRITE_ROOTS requires SESSIONDOCK_FILE_ROOTS"
        return base(SESSIONDOCK_FILE_WRITE_ROOTS=mkdir(os.path.join(tmp, "files"))), 1, "SESSIONDOCK_FILE_ROOTS", []

    def file_write_outside_read(tmp):
        # "each file write root must equal or lie inside an explicit read file root"
        a, b = mkdir(os.path.join(tmp, "a")), mkdir(os.path.join(tmp, "b"))
        return base(SESSIONDOCK_FILE_ROOTS=a, SESSIONDOCK_FILE_WRITE_ROOTS=b), 1, None, []

    def file_roots_too_many(tmp):
        # "SESSIONDOCK_FILE_ROOTS needs 1..16 explicit directories"
        paths = [mkdir(os.path.join(tmp, f"fr{i}")) for i in range(17)]
        return base(SESSIONDOCK_FILE_ROOTS=os.pathsep.join(paths)), 1, "1..16", []

    def file_roots_empty(_tmp):
        # "SESSIONDOCK_FILE_ROOTS must not be empty"
        return base(SESSIONDOCK_FILE_ROOTS=""), 1, None, []

    def codex_index_without_root(tmp):
        # "SESSIONDOCK_CODEX_INDEX requires an explicit existing absolute file and a Codex sessions root"
        index = os.path.join(tmp, "index.json")
        Path(index).write_text("{}", encoding="utf-8")
        return base(SESSIONDOCK_CODEX_INDEX=index), 1, "SESSIONDOCK_CODEX_INDEX", []

    def private_dirs_nested(tmp):
        # "trash requires an independent directory outside ... audit ..."
        priv = mkdir(os.path.join(tmp, "priv"), 0o700)
        inner = mkdir(os.path.join(priv, "inner"), 0o700)
        return base(SESSIONDOCK_AUDIT_DIR=priv, SESSIONDOCK_TRASH_DIR=inner), 1, None, []

    def symlinked_private_dir(tmp):
        # root() canonicalize follows STATE_DIR; validate does not reject the alias (store checks later).
        target = mkdir(os.path.join(tmp, "real"), 0o700)
        link = os.path.join(tmp, "link")
        os.symlink(target, link)
        return base(SESSIONDOCK_STATE_DIR=link), 0, None, ["config=ok"]

    def all_private_ok(tmp):
        # delivery must exist (0700) but --check-config must not need an initialized ledger.
        web = make_web(tmp)
        roots = {n: mkdir(os.path.join(tmp, n)) for n in ("claude", "codex", "grok")}
        priv = {n: mkdir(os.path.join(tmp, n), 0o700)
                for n in ("state", "delivery", "audit", "trash", "ptyhost")}
        env = {
            "SESSIONDOCK_BIND": "127.0.0.1:0", "SESSIONDOCK_WEB_DIR": web,
            "SESSIONDOCK_CLAUDE_ROOT": roots["claude"], "SESSIONDOCK_CODEX_ROOT": roots["codex"],
            "SESSIONDOCK_GROK_ROOT": roots["grok"], "SESSIONDOCK_STATE_DIR": priv["state"],
            "SESSIONDOCK_DELIVERY_DIR": priv["delivery"], "SESSIONDOCK_AUDIT_DIR": priv["audit"],
            "SESSIONDOCK_TRASH_DIR": priv["trash"], "SESSIONDOCK_PTYHOST_DIR": priv["ptyhost"],
        }
        extras = [
            f"claude_root={real(roots['claude'])}", f"codex_root={real(roots['codex'])}",
            f"grok_root={real(roots['grok'])}", f"state_dir={real(priv['state'])}",
            f"delivery_dir={priv['delivery']}", f"audit_dir={priv['audit']}",
            f"trash_dir={priv['trash']}", f"ptyhost_dir={real(priv['ptyhost'])}",
        ]
        return env, 0, None, extras

    def proc_scan_default_off(_tmp):
        # Unset: the scan is off and the Grok active file unset.
        return base(), 0, None, ["proc_scan=0", "proc_root=/proc", "grok_active=(unset)"]

    def proc_scan_on_ok(_tmp):
        # "1" switches the scan on; the real table is the default root.
        return base(SESSIONDOCK_PROC_SCAN="1"), 0, None, ["proc_scan=1", "proc_root=/proc"]

    def proc_scan_invalid_value(_tmp):
        # "SESSIONDOCK_PROC_SCAN must be 1 or 0"
        return base(SESSIONDOCK_PROC_SCAN="yes"), 1, "SESSIONDOCK_PROC_SCAN", []

    def proc_root_without_scan(tmp):
        # "SESSIONDOCK_PROC_ROOT and SESSIONDOCK_GROK_ACTIVE require SESSIONDOCK_PROC_SCAN=1"
        return base(SESSIONDOCK_PROC_ROOT=mkdir(os.path.join(tmp, "proc"))), 1, "SESSIONDOCK_PROC_SCAN=1", []

    def proc_root_synthetic_ok(tmp):
        # A synthetic tree (tests) is echoed canonicalized.
        proc = mkdir(os.path.join(tmp, "proc"))
        return base(SESSIONDOCK_PROC_SCAN="1", SESSIONDOCK_PROC_ROOT=proc), 0, None, [f"proc_root={real(proc)}"]

    def proc_root_is_file(tmp):
        # root(): "must be a directory"
        path = os.path.join(tmp, "notproc")
        Path(path).write_text("x", encoding="utf-8")
        return base(SESSIONDOCK_PROC_SCAN="1", SESSIONDOCK_PROC_ROOT=path), 1, "must be a directory", []

    def grok_active_missing(tmp):
        # "SESSIONDOCK_GROK_ACTIVE requires an explicit existing absolute file"
        return (base(SESSIONDOCK_PROC_SCAN="1", SESSIONDOCK_GROK_ACTIVE=os.path.join(tmp, "missing.json")),
                1, "SESSIONDOCK_GROK_ACTIVE", [])

    def grok_active_inside_native_root(tmp):
        # "Grok active-sessions file must stay outside frontend, native, host, ..."
        grok = mkdir(os.path.join(tmp, "grok"))
        active = os.path.join(grok, "active_sessions.json")
        Path(active).write_text("[]", encoding="utf-8")
        return (base(SESSIONDOCK_PROC_SCAN="1", SESSIONDOCK_GROK_ROOT=grok, SESSIONDOCK_GROK_ACTIVE=active),
                1, "Grok active-sessions file", [])

    def grok_active_ok(tmp):
        # Next to the Grok root (like ~/.grok/active_sessions.json beside ~/.grok/sessions), not inside it.
        grok = mkdir(os.path.join(tmp, "sessions"))
        active = os.path.join(tmp, "active_sessions.json")
        Path(active).write_text("[]", encoding="utf-8")
        return (base(SESSIONDOCK_PROC_SCAN="1", SESSIONDOCK_GROK_ROOT=grok, SESSIONDOCK_GROK_ACTIVE=active),
                0, None, [f"grok_active={active}", "proc_scan=1"])

    # Node listener (batch 38 H1): four settings together or nothing.
    NODE_TOKEN = "check-t0ken.check-t0ken.check-t0ken.check-t0ken~"

    def node_files(tmp):
        token = os.path.join(tmp, "node-token")
        Path(token).touch(mode=0o600)
        os.chmod(token, 0o600)
        Path(token).write_text(NODE_TOKEN + "\n", encoding="utf-8")
        ids = mkdir(os.path.join(tmp, "ids"), 0o700)
        return token, os.path.join(ids, "node-id")

    def node_env(tmp, **override):
        token, node_id = node_files(tmp)
        env = base(SESSIONDOCK_NODE_BIND="127.0.0.1:0", SESSIONDOCK_NODE_TOKEN_FILE=token,
                   SESSIONDOCK_NODE_ID_FILE=node_id, SESSIONDOCK_NODE_PEERS="127.0.0.0/8,10.100.100.0/24")
        for key, value in override.items():
            if value is None:
                env.pop(key)
            else:
                env[key] = value
        return env

    def node_unset(_tmp):
        # No node settings: the four lines print "(unset)".
        return base(), 0, None, ["node_bind=(unset)", "node_token_file=(unset)", "node_id_file=(unset)",
                                 "node_peers=(unset)"]

    def node_all_four_ok(tmp):
        # All four together: echoed as configured; the id is NOT minted by --check-config.
        env = node_env(tmp)
        return env, 0, None, ["node_bind=127.0.0.1:0", f"node_token_file={env['SESSIONDOCK_NODE_TOKEN_FILE']}",
                              f"node_id_file={env['SESSIONDOCK_NODE_ID_FILE']}",
                              "node_peers=127.0.0.0/8,10.100.100.0/24"]

    def node_missing_bind(tmp):
        # "... must be set together (the node listener fails closed)"
        return node_env(tmp, SESSIONDOCK_NODE_BIND=None), 1, "set together", []

    def node_missing_token(tmp):
        return node_env(tmp, SESSIONDOCK_NODE_TOKEN_FILE=None), 1, "set together", []

    def node_missing_id(tmp):
        return node_env(tmp, SESSIONDOCK_NODE_ID_FILE=None), 1, "set together", []

    def node_missing_peers(tmp):
        return node_env(tmp, SESSIONDOCK_NODE_PEERS=None), 1, "set together", []

    def node_bind_only(tmp):
        # One of four is the same refusal.
        node_files(tmp)
        return base(SESSIONDOCK_NODE_BIND="127.0.0.1:0"), 1, "set together", []

    def node_bind_invalid(tmp):
        # "invalid SESSIONDOCK_NODE_BIND"
        return node_env(tmp, SESSIONDOCK_NODE_BIND="not-an-address"), 1, "SESSIONDOCK_NODE_BIND", []

    def node_bind_wildcard(tmp):
        # "SESSIONDOCK_NODE_BIND must name one interface address, not a wildcard"
        return node_env(tmp, SESSIONDOCK_NODE_BIND="0.0.0.0:8742"), 1, "wildcard", []

    def node_bind_equals_bind(tmp):
        # "SESSIONDOCK_NODE_BIND must differ from SESSIONDOCK_BIND"
        return node_env(tmp, SESSIONDOCK_BIND="127.0.0.1:8741", SESSIONDOCK_NODE_BIND="127.0.0.1:8741"), 1, "differ", []

    def node_peers_empty(tmp):
        # "SESSIONDOCK_NODE_PEERS must be a comma-separated CIDR list"
        return node_env(tmp, SESSIONDOCK_NODE_PEERS=" , "), 1, "SESSIONDOCK_NODE_PEERS", []

    def node_peers_host_bits(tmp):
        # strict CIDR: "has host bits set"
        return node_env(tmp, SESSIONDOCK_NODE_PEERS="10.100.100.7/24"), 1, "host bits", []

    def node_peers_not_a_network(tmp):
        return node_env(tmp, SESSIONDOCK_NODE_PEERS="wireguard"), 1, "SESSIONDOCK_NODE_PEERS", []

    def node_token_missing_file(tmp):
        # "SESSIONDOCK_NODE_TOKEN_FILE requires an explicit existing absolute file"
        return node_env(tmp, SESSIONDOCK_NODE_TOKEN_FILE=os.path.join(tmp, "absent")), 1, "SESSIONDOCK_NODE_TOKEN_FILE", []

    def node_token_too_short(tmp):
        # token grammar: "node token must contain 32–256 characters"
        env = node_env(tmp)
        Path(env["SESSIONDOCK_NODE_TOKEN_FILE"]).write_text("short\n", encoding="utf-8")
        return env, 1, "32", []

    def node_token_bad_char(tmp):
        env = node_env(tmp)
        Path(env["SESSIONDOCK_NODE_TOKEN_FILE"]).write_text("!" * 40 + "\n", encoding="utf-8")
        return env, 1, "32", []

    def node_token_world_readable(tmp):
        # "SESSIONDOCK_NODE_TOKEN_FILE must not be readable by group or others"
        env = node_env(tmp)
        os.chmod(env["SESSIONDOCK_NODE_TOKEN_FILE"], 0o644)
        return env, 1, "group or others", []

    def node_id_existing_ok(tmp):
        # An existing valid id file is only read.
        env = node_env(tmp)
        Path(env["SESSIONDOCK_NODE_ID_FILE"]).write_text("a" * 32 + "\n", encoding="utf-8")
        return env, 0, None, ["config=ok", f"node_id_file={env['SESSIONDOCK_NODE_ID_FILE']}"]

    def node_id_invalid_content(tmp):
        # "SESSIONDOCK_NODE_ID_FILE holds an invalid node identity"
        env = node_env(tmp)
        Path(env["SESSIONDOCK_NODE_ID_FILE"]).write_text("A" * 32 + "\n", encoding="utf-8")
        return env, 1, "invalid node identity", []

    def node_id_parent_missing(tmp):
        # "SESSIONDOCK_NODE_ID_FILE parent directory must exist"
        return node_env(tmp, SESSIONDOCK_NODE_ID_FILE=os.path.join(tmp, "nowhere", "node-id")), 1, "parent directory", []

    def node_id_is_directory(tmp):
        # "SESSIONDOCK_NODE_ID_FILE must be a regular file"
        return node_env(tmp, SESSIONDOCK_NODE_ID_FILE=mkdir(os.path.join(tmp, "iddir"))), 1, "regular file", []

    def node_id_same_as_token(tmp):
        # "... must be different files"
        env = node_env(tmp)
        env["SESSIONDOCK_NODE_ID_FILE"] = env["SESSIONDOCK_NODE_TOKEN_FILE"]
        return env, 1, "different files", []

    def node_id_inside_native_root(tmp):
        # "node token and id files must stay outside frontend, native, ..."
        claude = mkdir(os.path.join(tmp, "claude"))
        return node_env(tmp, SESSIONDOCK_CLAUDE_ROOT=claude,
                        SESSIONDOCK_NODE_ID_FILE=os.path.join(claude, "node-id")), 1, "outside", []

    def node_token_inside_state_dir(tmp):
        state = mkdir(os.path.join(tmp, "state"), 0o700)
        env = node_env(tmp, SESSIONDOCK_STATE_DIR=state)
        moved = os.path.join(state, "node-token")
        os.rename(env["SESSIONDOCK_NODE_TOKEN_FILE"], moved)
        env["SESSIONDOCK_NODE_TOKEN_FILE"] = moved
        return env, 1, "outside", []

    def bug_report_env(tmp, **override):
        # dir 0700 + repo inside a file write root (which lies inside a read root).
        work = mkdir(os.path.join(tmp, "work"), 0o755)
        repo = mkdir(os.path.join(work, "repo"), 0o755)
        reports = mkdir(os.path.join(tmp, "reports"), 0o700)
        env = base(SESSIONDOCK_FILE_ROOTS=work, SESSIONDOCK_FILE_WRITE_ROOTS=work,
                   SESSIONDOCK_BUG_REPORT_DIR=reports, SESSIONDOCK_BUG_REPORT_REPO=repo)
        for key, value in override.items():
            if value is None:
                env.pop(key, None)
            else:
                env[key] = value
        return env

    def bug_report_unset(_tmp):
        return base(), 0, None, ["bug_report_dir=(unset)", "bug_report_repo=(unset)"]

    def bug_report_ok(tmp):
        env = bug_report_env(tmp)
        return env, 0, None, ["config=ok", f"bug_report_dir={env['SESSIONDOCK_BUG_REPORT_DIR']}",
                              f"bug_report_repo={env['SESSIONDOCK_BUG_REPORT_REPO']}"]

    def bug_report_dir_only(tmp):
        # "... must be set together (bug reports fail closed)"
        return bug_report_env(tmp, SESSIONDOCK_BUG_REPORT_REPO=None), 1, "set together", []

    def bug_report_repo_only(tmp):
        return bug_report_env(tmp, SESSIONDOCK_BUG_REPORT_DIR=None), 1, "set together", []

    def bug_report_dir_open_mode(tmp):
        # "bug-report directory requires private owner-only permissions (0700)"
        env = bug_report_env(tmp)
        os.chmod(env["SESSIONDOCK_BUG_REPORT_DIR"], 0o755)
        return env, 1, "0700", []

    def bug_report_dir_missing(tmp):
        return bug_report_env(tmp, SESSIONDOCK_BUG_REPORT_DIR=os.path.join(tmp, "absent")), 1, "SESSIONDOCK_BUG_REPORT_DIR", []

    def bug_report_repo_outside_write_roots(tmp):
        # "... must equal or lie inside a file write root"
        outside = mkdir(os.path.join(tmp, "elsewhere"))
        return bug_report_env(tmp, SESSIONDOCK_BUG_REPORT_REPO=outside), 1, "write root", []

    def bug_report_repo_without_write_roots(tmp):
        env = bug_report_env(tmp, SESSIONDOCK_FILE_WRITE_ROOTS=None)
        return env, 1, "write root", []

    def bug_report_dir_inside_file_root(tmp):
        # The bundle directory must stay outside every file access path.
        env = bug_report_env(tmp)
        inside = mkdir(os.path.join(env["SESSIONDOCK_FILE_ROOTS"], "reports"), 0o700)
        env["SESSIONDOCK_BUG_REPORT_DIR"] = inside
        return env, 1, "outside", []

    def bug_report_dir_equals_audit(tmp):
        env = bug_report_env(tmp)
        env["SESSIONDOCK_AUDIT_DIR"] = env["SESSIONDOCK_BUG_REPORT_DIR"]
        return env, 1, "outside", []

    def public_hosts_ok(_tmp):
        # Trimmed, lower-cased, echoed in order; unset prints "(unset)".
        return (base(SESSIONDOCK_PUBLIC_HOSTS=" Example.com, lan.test:8443 "), 0, None,
                ["config=ok", "public_hosts=example.com,lan.test:8443"])

    def public_hosts_unset(_tmp):
        return base(), 0, None, ["public_hosts=(unset)"]

    def public_hosts_empty(_tmp):
        # "SESSIONDOCK_PUBLIC_HOSTS must be a comma-separated host[:port] list"
        return base(SESSIONDOCK_PUBLIC_HOSTS=" , "), 1, "SESSIONDOCK_PUBLIC_HOSTS", []

    def public_hosts_invalid(_tmp):
        # "SESSIONDOCK_PUBLIC_HOSTS: invalid authority"
        return base(SESSIONDOCK_PUBLIC_HOSTS="example.com,http://bad/path"), 1, "invalid authority", []

    def hostname_default(_tmp):
        # Batch 44 WP-C: the system host name (Python `socket.gethostname()`),
        # never the product name while the kernel reports one.
        system = ""
        try:
            system = Path("/proc/sys/kernel/hostname").read_text(encoding="utf-8").strip()
        except OSError:
            pass
        return base(), 0, None, [f"hostname={system or socket.gethostname() or 'SessionDock'}",
                                 "debug_runs=(unset)"]

    def hostname_override(tmp):
        state = mkdir(os.path.join(tmp, "state"), 0o700)
        return (base(SESSIONDOCK_HOSTNAME=" lab-node ", SESSIONDOCK_STATE_DIR=state), 0, None,
                ["hostname=lab-node", f"debug_runs={real(state)}/debug-runs.json"])

    def hostname_empty(_tmp):
        return base(SESSIONDOCK_HOSTNAME="   "), 1, "SESSIONDOCK_HOSTNAME", []

    return [
        ("minimal_ok", minimal_ok),
        ("hostname_default", hostname_default),
        ("hostname_override", hostname_override),
        ("hostname_empty", hostname_empty),
        ("public_hosts_ok", public_hosts_ok),
        ("public_hosts_unset", public_hosts_unset),
        ("public_hosts_empty", public_hosts_empty),
        ("public_hosts_invalid", public_hosts_invalid),
        ("bug_report_unset", bug_report_unset),
        ("bug_report_ok", bug_report_ok),
        ("bug_report_dir_only", bug_report_dir_only),
        ("bug_report_repo_only", bug_report_repo_only),
        ("bug_report_dir_open_mode", bug_report_dir_open_mode),
        ("bug_report_dir_missing", bug_report_dir_missing),
        ("bug_report_repo_outside_write_roots", bug_report_repo_outside_write_roots),
        ("bug_report_repo_without_write_roots", bug_report_repo_without_write_roots),
        ("bug_report_dir_inside_file_root", bug_report_dir_inside_file_root),
        ("bug_report_dir_equals_audit", bug_report_dir_equals_audit),
        ("node_unset", node_unset),
        ("node_all_four_ok", node_all_four_ok),
        ("node_missing_bind", node_missing_bind),
        ("node_missing_token", node_missing_token),
        ("node_missing_id", node_missing_id),
        ("node_missing_peers", node_missing_peers),
        ("node_bind_only", node_bind_only),
        ("node_bind_invalid", node_bind_invalid),
        ("node_bind_wildcard", node_bind_wildcard),
        ("node_bind_equals_bind", node_bind_equals_bind),
        ("node_peers_empty", node_peers_empty),
        ("node_peers_host_bits", node_peers_host_bits),
        ("node_peers_not_a_network", node_peers_not_a_network),
        ("node_token_missing_file", node_token_missing_file),
        ("node_token_too_short", node_token_too_short),
        ("node_token_bad_char", node_token_bad_char),
        ("node_token_world_readable", node_token_world_readable),
        ("node_id_existing_ok", node_id_existing_ok),
        ("node_id_invalid_content", node_id_invalid_content),
        ("node_id_parent_missing", node_id_parent_missing),
        ("node_id_is_directory", node_id_is_directory),
        ("node_id_same_as_token", node_id_same_as_token),
        ("node_id_inside_native_root", node_id_inside_native_root),
        ("node_token_inside_state_dir", node_token_inside_state_dir),
        ("proc_scan_default_off", proc_scan_default_off),
        ("proc_scan_on_ok", proc_scan_on_ok),
        ("proc_scan_invalid_value", proc_scan_invalid_value),
        ("proc_root_without_scan", proc_root_without_scan),
        ("proc_root_synthetic_ok", proc_root_synthetic_ok),
        ("proc_root_is_file", proc_root_is_file),
        ("grok_active_missing", grok_active_missing),
        ("grok_active_inside_native_root", grok_active_inside_native_root),
        ("grok_active_ok", grok_active_ok),
        ("roots_ok", roots_ok),
        ("non_loopback_bind", non_loopback_bind),
        ("invalid_bind", invalid_bind),
        ("empty_root", empty_root),
        ("root_is_file", root_is_file),
        ("state_overlaps_root", state_overlaps_root),
        ("state_inside_web", state_inside_web),
        ("delivery_missing_dir", delivery_missing_dir),
        ("delivery_relative", delivery_relative),
        ("file_write_without_read", file_write_without_read),
        ("file_write_outside_read", file_write_outside_read),
        ("file_roots_too_many", file_roots_too_many),
        ("file_roots_empty", file_roots_empty),
        ("codex_index_without_root", codex_index_without_root),
        ("private_dirs_nested", private_dirs_nested),
        ("symlinked_private_dir", symlinked_private_dir),
        ("all_private_ok", all_private_ok),
    ]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default=default_binary())
    args = parser.parse_args()
    binary = Path(args.binary)
    if not binary.is_file():
        raise SystemExit(f"missing binary: {binary}")
    binary = str(binary.resolve())
    n = 0
    for name, build in builders():
        with tempfile.TemporaryDirectory() as tmp:
            env, want, needle, extras = build(tmp)
            code, out, err = invoke(binary, env)
            check(name, code, out, err, want, needle, extras)
            minted = Path(env.get("SESSIONDOCK_NODE_ID_FILE", ""))
            if name == "node_all_four_ok" and minted.exists():
                fail(name, "--check-config must not mint the node id", code, "")
        print(f"PASS {name}", flush=True)
        n += 1
    print(f"PASS check_config_suite: {n} cases", flush=True)


if __name__ == "__main__":
    main()
