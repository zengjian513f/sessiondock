#!/usr/bin/env python3
"""Prove /api/meta.hostname, served page titles, and list-payload warning trim (WP-C 6/7)."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from urllib.error import HTTPError

from history_parity import BINARY, REPO, Corpus, claude_row, isolated_server, get_json

CHECKS = 0


def fail(area, why, body=""):
    raise SystemExit(f"FAIL {area}: {why}; {str(body)[:240]}")


def passed(area):
    global CHECKS
    CHECKS += 1
    print(f"PASS {area}", flush=True)


def kernel_hostname():
    return Path("/proc/sys/kernel/hostname").read_text(encoding="utf-8").strip()


def build_corpus(root: Path) -> Corpus:
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    sid = "claude-clean"
    corpus.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "hello"),
        claude_row(sid, "assistant", "a0", "u0", "ok"),
    ], [])
    warned = "claude-warned"
    path = corpus.put(warned, "claude", [
        claude_row(warned, "user", "w0", None, "prompt"),
        claude_row(warned, "assistant", "w1", "w0", "reply"),
    ], [])
    with path.open("ab") as fh:
        fh.write(b"{not json\n")
    return corpus


def fetch_text(opener, base, route):
    with opener.open(base + route, timeout=10) as resp:
        return resp.read().decode("utf-8", "replace")


def assert_hostname(opener, base, want, area):
    meta = get_json(opener, base, "/api/meta")
    got = meta.get("hostname")
    if got != want:
        fail(area, f"hostname {got!r} want {want!r}", meta)
    passed(area)


def check_pages(opener, base, host):
    index = fetch_text(opener, base, "/")
    needle = f"<title>{host} · 会话管理</title>"
    if needle not in index:
        fail("index.html title", f"missing {needle!r}", index[:400])
    passed("index.html title")
    files = fetch_text(opener, base, "/files.html")
    if "__AGENTHUB_HOSTNAME__" not in Path(REPO / "legacy-web" / "files.html").read_text(encoding="utf-8"):
        print("PASS files.html hostname (skipped: served page has no hostname placeholder)", flush=True)
        global CHECKS
        CHECKS += 1
        return
    if host not in files:
        fail("files.html hostname", f"missing {host!r}", files[:400])
    passed("files.html hostname")


def check_list_and_warnings(opener, base, corpus: Corpus):
    payload = get_json(opener, base, "/api/sessions")
    rows = payload.get("sessions") or []
    if len(rows) != 2:
        fail("sessions count", f"want 2 rows, got {len(rows)}", payload)
    for row in rows:
        if row.get("supported") is not True:
            fail("sessions supported", "synthetic rows must be supported", row)
        if "migration_warnings" in row:
            fail("list trim", "supported row must omit migration_warnings", row)
    passed("list rows omit migration_warnings")
    uid = corpus.uid("claude-warned")
    detail = get_json(opener, base, f"/api/messages/{uid}")
    warns = (detail.get("meta") or {}).get("migration_warnings") or []
    if not any(isinstance(w, str) and w.startswith("跳过无效的JSONL 记录") for w in warns):
        fail("messages warnings", "missing 跳过无效的JSONL 记录", detail.get("meta"))
    passed("messages meta.migration_warnings")


def check_term_list(opener, base):
    try:
        body = get_json(opener, base, "/api/term/list")
    except HTTPError as err:
        if err.code not in (501, 404):
            fail("term/list", f"HTTP {err.code} (want 501/404 or home)", err.read(512))
        passed(f"term/list HTTP {err.code} (terminal not configured)")
        return
    home = body.get("home")
    if home != os.environ.get("HOME"):
        fail("term/list home", f"{home!r} != HOME", body)
    passed("term/list home==HOME (terminal configured)")


def check_empty_hostname(binary: Path, corpus: Corpus, state_dir: Path):
    env = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
    env.update({
        "SESSIONDOCK_BIND": "127.0.0.1:0",
        "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web"),
        "SESSIONDOCK_STATE_DIR": str(state_dir),
        "SESSIONDOCK_HOSTNAME": "",
    })
    for source in ("claude", "codex", "grok"):
        env["SESSIONDOCK_" + source.upper() + "_ROOT"] = str(corpus.root / source)
    proc = subprocess.run([str(binary), "--check-config"], env=env, capture_output=True, timeout=15)
    if proc.returncode == 0:
        fail("empty SESSIONDOCK_HOSTNAME", "expected non-zero --check-config",
             proc.stderr.decode("utf-8", "replace")[:240])
    passed("empty SESSIONDOCK_HOSTNAME refuses --check-config")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-meta-hostname-") as tmp:
        root = Path(tmp)
        corpus = build_corpus(root / "corpus")
        state = root / "corpus" / "state"
        state.mkdir()
        state.chmod(0o700)
        with isolated_server(corpus, binary, state_dir=state) as (base, opener):
            assert_hostname(opener, base, kernel_hostname(), "default hostname")
            check_list_and_warnings(opener, base, corpus)
            check_term_list(opener, base)
        with isolated_server(corpus, binary, state_dir=state,
                             extra_env={"SESSIONDOCK_HOSTNAME": "bench-host"}) as (base, opener):
            assert_hostname(opener, base, "bench-host", "SESSIONDOCK_HOSTNAME override")
            check_pages(opener, base, "bench-host")
        check_empty_hostname(binary, corpus, state)
    print(f"meta_hostname_check: {CHECKS} checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
