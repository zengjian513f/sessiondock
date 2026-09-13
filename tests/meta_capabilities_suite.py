#!/usr/bin/env python3
"""Check /api/meta capabilities for ordinary and explicitly enabled services.

Synthetic fixtures in a temporary directory; loopback listeners only.
Never scans CLI homes, production state, or the network.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from urllib.error import URLError
from urllib.request import ProxyHandler, build_opener

from history_parity import BINARY, REPO, Corpus, get_json, isolated_server

RELEASE = REPO / "target/release" / ("sessiondock.exe" if os.name == "nt" else "sessiondock")
DEFAULT = RELEASE if RELEASE.is_file() else BINARY
# Default capabilities() plus every lib.rs overlay when the matching service is off.
BASE = {
    "backend": "rust", "stage": "replacement", "read_only": False,
    "storage_namespace": "sessiondock.",
    "sessions": True, "watch": True, "search": True, "live": sys.platform.startswith("linux"),
    "terminal": False, "outbox": False, "audit": False, "files": True,
    "mutations": False, "hub": False,
    "media": True, "media_remote": True, "media_lazy": True, "history_pages": True,
    "media_continuation": True, "history_semantics": "limited_native",
    "terminal_transport": False, "terminal_create": False, "terminal_pending": False,
    "terminal_bind": False, "terminal_takeover": False, "terminal_complete_dir": False,
    "terminal_backend": False, "metadata": False, "files_jobs": False,
    "file_thumbnails": False, "outbox_read": False,
}
# Present in current lib.rs / state.rs; older binaries omit them.
OPTIONAL = {"timeline_pin": "metadata", "terminal_input": "terminal", "files_write": None, "trash": None,
            "session_stop": "terminal_create", "bug_report": None}


def fail(area, why, body=""):
    if not isinstance(body, str):
        body = json.dumps(body, ensure_ascii=False, default=str)
    raise SystemExit(f"FAIL {area}: {why}; {body[:400]}")


def mkdir(root, name, mode=0o700):
    path = root / name
    path.mkdir()
    path.chmod(mode)
    return path


def check(area, opener, base, *, protocol=0, node_id=None, **flags):
    try:
        body = get_json(opener, base, "/api/meta")
    except Exception as err:
        fail(area, str(err))
    caps = body.get("capabilities")
    if not isinstance(caps, dict):
        fail(area, "missing capabilities", body)
    # Batch 38 H1: identity only with the node listener configured; `hub` never.
    if body.get("mode") != "local" or body.get("protocol") != protocol or body.get("node_id") != node_id:
        fail(area, f"mode/protocol/node_id (want local/{protocol}/{node_id})", body)
    want = dict(BASE)
    want.update({key: value for key, value in flags.items() if key in want})
    for key, linked in OPTIONAL.items():
        if key not in caps:
            continue
        want[key] = flags.get(key, flags.get(linked, False) if linked else False)
    if caps != want:
        fail(area, "capabilities mismatch", caps)
    print(f"PASS {area}", flush=True)


@contextmanager
def running(area, corpus, binary, **kwargs):
    proc_root = corpus.root / "proc-empty"
    proc_root.mkdir(exist_ok=True)
    extra_env = dict(kwargs.pop("extra_env", {}) or {})
    extra_env.setdefault("SESSIONDOCK_PROC_ROOT", str(proc_root))
    try:
        with isolated_server(corpus, binary, extra_env=extra_env, **kwargs) as pair:
            yield pair
    except AssertionError as err:
        fail(area, str(err))


def environment(corpus, port, extra):
    proc_root = corpus.root / "proc-empty"
    proc_root.mkdir(exist_ok=True)
    env = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    env.update(SESSIONDOCK_BIND=f"127.0.0.1:{port}", SESSIONDOCK_WEB_DIR=str(REPO / "legacy-web"),
               SESSIONDOCK_PROC_ROOT=str(proc_root), **extra)
    for source in ("claude", "codex", "grok"):
        env["SESSIONDOCK_" + source.upper() + "_ROOT"] = str(corpus.root / source)
    return env


@contextmanager
def running_env(area, binary, corpus, extra):
    """Like `running` for settings isolated_server does not know (node identity)."""
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    env = environment(corpus, port, extra)
    opener = build_opener(ProxyHandler({}))
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryFile(mode="w+b") as log:
        process = subprocess.Popen(
            [str(binary.resolve(strict=True))], cwd=REPO, env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 5
            while True:
                if process.poll() is not None:
                    log.seek(0)
                    fail(area, f"exited {process.returncode}", log.read(400).decode("utf-8", "replace"))
                try:
                    with opener.open(base + "/api/health", timeout=0.2):
                        break
                except (OSError, URLError):
                    pass
                if time.monotonic() > deadline:
                    fail(area, "health check timed out")
                time.sleep(0.05)
            yield base, opener
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)


def refuse(area, binary, corpus, extra):
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    env = environment(corpus, port, extra)
    opener = build_opener(ProxyHandler({}))
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryFile(mode="w+b") as log:
        process = subprocess.Popen(
            [str(binary.resolve(strict=True))], cwd=REPO, env=env, stdout=log, stderr=log)
        deadline = time.monotonic() + 5
        health = None
        while time.monotonic() < deadline:
            try:
                with opener.open(base + "/api/health", timeout=0.2) as response:
                    health = response.read(400)
                    break
            except (OSError, URLError):
                pass
            if process.poll() is not None:
                break
            time.sleep(0.05)
        log.seek(0)
        excerpt = log.read(400).decode("utf-8", "replace")
        code = process.poll()
        if code is None:
            process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=2)
            fail(area, "still running after 5s", excerpt)
        if health is not None:
            fail(area, f"exited {code} but answered /api/health", health)
        if code == 0:
            fail(area, "exited 0, expected non-zero", excerpt)
        print(f"PASS {area}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=DEFAULT)
    args = parser.parse_args()
    binary = args.binary
    with tempfile.TemporaryDirectory(prefix="sessiondock-meta-caps-") as temporary:
        root = Path(temporary)
        corpus = Corpus(root)
        for source in ("claude", "codex", "grok"):
            (root / source).mkdir()
        with running("bare", corpus, binary) as (base, opener):
            check("bare", opener, base)
        with running("state-dir", corpus, binary, state_dir=mkdir(root, "state")) as (base, opener):
            check("state-dir", opener, base, metadata=True, timeline_pin=True)
        files = mkdir(root, "files", 0o755)
        with running("file-roots", corpus, binary, file_roots=(files,)) as (base, opener):
            check("file-roots", opener, base, files=True, files_jobs=False)
        with running("audit", corpus, binary, audit_dir=mkdir(root, "audit")) as (base, opener):
            check("audit", opener, base, audit=True)
        host = mkdir(root, "host")
        with running("host-dir", corpus, binary, host_dir=host) as (base, opener):
            check("host-dir", opener, base, terminal=True, terminal_transport=True,
                  terminal_backend=True, terminal_input=True, files_write={"actions":["upload", "cancel", "mkdir", "new-file", "rename", "move"], "conflicts":["error", "keep", "skip"], "delete":"trash", "chunk_bytes":8*1024*1024, "job_bytes":1024**4, "max_items":2000}, files_jobs=True)
        shared = mkdir(root, "shared")
        with running_env("overlap-compatible", binary, corpus, {
                "SESSIONDOCK_STATE_DIR": str(shared),
                "SESSIONDOCK_AUDIT_DIR": str(shared)}) as (base, opener):
            check("overlap-compatible", opener, base, metadata=True, timeline_pin=True, audit=True)
        with running_env("ordinary-directory-mode", binary, corpus, {
                "SESSIONDOCK_AUDIT_DIR": str(mkdir(root, "audit-open", 0o755))}) as (base, opener):
            check("ordinary-directory-mode", opener, base, audit=True)
        both = mkdir(root, "audit-files")
        with running_env("operator-file-root", binary, corpus, {
                "SESSIONDOCK_AUDIT_DIR": str(both), "SESSIONDOCK_FILE_ROOTS": str(both)}) as (base, opener):
            check("operator-file-root", opener, base, audit=True, files=True)
        # Node identity (batch 38 H1): protocol 1 and the minted id, `hub` still false.
        token = root / "node-token"
        token.touch(mode=0o600)
        token.chmod(0o600)
        token.write_text("meta-t0ken.meta-t0ken.meta-t0ken.meta-t0ken~\n", encoding="utf-8")
        ids = mkdir(root, "ids")
        node = {"SESSIONDOCK_NODE_BIND": "127.0.0.1:0", "SESSIONDOCK_NODE_TOKEN_FILE": str(token),
                "SESSIONDOCK_NODE_ID_FILE": str(ids / "node-id"), "SESSIONDOCK_NODE_PEERS": "127.0.0.0/8"}
        with running_env("node-identity", binary, corpus, node) as (base, opener):
            minted = (ids / "node-id").read_text(encoding="utf-8").strip()
            if len(minted) != 32:
                fail("node-identity", f"node id file holds {minted!r}")
            check("node-identity", opener, base, protocol=1, node_id=minted)
        partial = dict(node)
        del partial["SESSIONDOCK_NODE_PEERS"]
        refuse("misconfig-node-partial", binary, corpus, partial)


if __name__ == "__main__":
    main()
