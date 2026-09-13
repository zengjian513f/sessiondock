#!/usr/bin/env python3
"""Read-only GET /api/session/outbox contract on an isolated loopback server.

Disabled 501, empty initialized ledger, Python-compatible empty snapshots for
unknown UIDs, ignored extra query fields, and a fresh epoch after restart.
Synthetic temp fixtures only; no production directories.
"""
from __future__ import annotations

import argparse, json, os, socket, subprocess, sys, tempfile
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, isolated_server)

BINARY = (p if (p := REPO / "target/release" / DEBUG_BINARY.name).is_file() else DEBUG_BINARY)
SECRETS = ("argv", "token", "host_instance", "node-token")


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def fetch(opener, base, route, want=200):
    try:
        with opener.open(base + route, timeout=10) as resp:
            raw, code = resp.read(65536), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    except Exception as err:
        fail(route, str(err))
    if code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)
    return payload, raw


def leaks(area, raw, *needles):
    text = raw.decode("utf-8", "replace") if isinstance(raw, (bytes, bytearray)) else str(raw)
    lowered = text.lower()
    for needle in (*needles, *SECRETS):
        if needle and needle.lower() in lowered:
            fail(area, f"response leaked {needle!r}", raw)


@contextmanager
def serve(corpus, binary, delivery=None):
    orig, extra = subprocess.Popen, (
        {} if delivery is None else {"SESSIONDOCK_DELIVERY_DIR": str(delivery)})

    def popen(*args, **kwargs):
        if extra:
            env = dict(kwargs.get("env") or {})
            env.update(extra)
            kwargs["env"] = env
        return orig(*args, **kwargs)

    subprocess.Popen = popen
    try:
        with isolated_server(corpus, binary) as pair:
            yield pair
    finally:
        subprocess.Popen = orig


def initialize(binary, directory):
    directory.mkdir()
    directory.chmod(0o700)
    with socket.socket() as occupied:
        occupied.bind(("127.0.0.1", 0))
        env = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
        env["SESSIONDOCK_BIND"] = "127.0.0.1:%d" % occupied.getsockname()[1]
        out = subprocess.run(
            [str(binary), "--initialize-delivery", str(directory)],
            cwd=REPO, env=env, capture_output=True, timeout=20)
    if out.returncode:
        fail("initialize", out.stderr.decode("utf-8", "replace") or out.stdout.decode("utf-8", "replace"))


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    sid = "claude-outbox"
    corpus.put(sid, "claude", [claude_row(sid, "user", "u0", None, "synthetic outbox parent")], [])
    return corpus


def empty_ok(area, payload, raw):
    if not isinstance(payload.get("outbox"), list) or payload["outbox"]:
        fail(area, "expected empty outbox list", raw)
    ver = payload.get("outbox_version")
    if not isinstance(ver, dict) or not isinstance(ver.get("epoch"), str) or not ver["epoch"]:
        fail(area, "outbox_version.epoch must be a nonempty str", raw)
    if type(ver.get("revision")) is not int or ver["revision"] < 0:
        fail(area, "outbox_version.revision must be a non-negative int", raw)
    return ver


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-outbox-") as tmp:
        root = Path(tmp)
        corpus = build(root / "native")
        uid = corpus.uid("claude-outbox")
        route = "/api/session/outbox?uid=" + quote(uid, safe=":")
        needles = (str(corpus.root), str(corpus.paths["claude-outbox"]))
        with serve(corpus, args.binary) as (base, opener):
            payload, raw = fetch(opener, base, route, 501)
            if payload.get("code") != "delivery_disabled" or "outbox" in payload:
                fail("disabled", "want 501 delivery_disabled without outbox", raw)
            leaks("disabled", raw, *needles)
        passed("disabled 501 delivery_disabled")
        delivery = root / "delivery"
        initialize(args.binary, delivery)
        needles += (str(delivery),)
        with serve(corpus, args.binary, delivery) as (base, opener):
            payload, raw = fetch(opener, base, route)
            ver = empty_ok("empty", payload, raw)
            leaks("empty", raw, *needles)
            passed("empty outbox epoch/revision")
            payload, raw = fetch(opener, base, "/api/session/outbox?uid=claude:unknown", 200)
            if empty_ok("unknown", payload, raw) != {"epoch": "none", "revision": 0}:
                fail("unknown", "want Python's empty unsupported-source snapshot", raw)
            leaks("unknown", raw, *needles)
            passed("unknown uid empty snapshot")
            payload, raw = fetch(opener, base, "/api/session/outbox", 200)
            if empty_ok("missing", payload, raw) != {"epoch": "none", "revision": 0}:
                fail("missing", "want Python's empty unsupported-source snapshot", raw)
            leaks("missing", raw, *needles)
            passed("missing uid empty snapshot")
            # `debug_run` selects the list view only; outbox ignores it.
            payload, raw = fetch(opener, base, route + "&debug_run=1", 200)
            if "outbox" not in payload:
                fail("debug_run", "outbox must ignore debug_run", raw)
            leaks("debug_run", raw, *needles)
            passed("debug_run ignored")
            epoch = ver["epoch"]
        with serve(corpus, args.binary, delivery) as (base, opener):
            payload, raw = fetch(opener, base, route)
            ver = empty_ok("restart", payload, raw)
            if ver["epoch"] == epoch:
                fail("restart", "epoch must change after reopen", raw)
            leaks("restart", raw, *needles)
        passed("restart new epoch")


if __name__ == "__main__":
    main()
