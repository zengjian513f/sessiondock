#!/usr/bin/env python3
"""HTTP contract for POST /api/audit/browser with the new frontend event shapes.

A JSON batch {page_id, uid, _build, events:[{event, ts, uid, data, content}]} is
admitted when SESSIONDOCK_AUDIT_DIR is configured; the directory is created
and chmodded like Python's audit store. Unknown event
names matching the intake alphabet are accepted. Each event is written as
structured metadata only (no free-form content) to browser-YYYY-MM-DD.jsonl,
with the top-level page_id copied onto every row. Named events
console.button.{state,missing,restored}, detail.rendered, header.layout,
terminal.pane, dialog.{shown,closed}, click (coordinates + text) and
dom.snapshot (header_state object) return 2xx and land in the JSONL. A 3 MiB
batch of many events is accepted; a 5 MiB batch is 413 body_too_large;
malformed JSON is 400 invalid_audit_request; without the directory the route is
501 and /api/meta capabilities.audit is false. Synthetic Claude corpus only.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, Request, build_opener

from history_parity import (
    BINARY as DEBUG_BINARY, REPO, Corpus, NoRedirects, claude_row, get_json)

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
SID, PAGE, BUILD = "claude-audit", "page-audit-1", "test-build"
ROUTE, MIB = "/api/audit/browser", 1024 * 1024
NAMES = (
    "console.button.state", "console.button.missing", "console.button.restored",
    "detail.rendered", "header.layout", "terminal.pane",
    "dialog.shown", "dialog.closed", "click", "dom.snapshot")


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def build(root: Path) -> Corpus:
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)
    corpus.put(SID, "claude", [
        claude_row(SID, "user", "u0", None, "audit parent"),
        claude_row(SID, "assistant", "a0", "u0", "audit reply")], [])
    return corpus


@contextmanager
def server_with_env(corpus, extra_env, executable=None):
    executable = (executable or BINARY).resolve(strict=True)
    env = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    env.update({"SESSIONDOCK_BIND": f"127.0.0.1:{port}",
                "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web"), **extra_env})
    for source in ("claude", "codex", "grok"):
        env["SESSIONDOCK_" + source.upper() + "_ROOT"] = str(corpus.root / source)
    opener = build_opener(ProxyHandler({}), NoRedirects())
    with tempfile.TemporaryFile(mode="w+b") as log:
        proc = subprocess.Popen([str(executable)], cwd=REPO, env=env, stdout=log, stderr=log)
        try:
            for _ in range(150):
                if proc.poll() is not None:
                    fail("boot", f"exited {proc.returncode}")
                try:
                    get_json(opener, base, "/api/health")
                    break
                except (OSError, URLError):
                    time.sleep(0.05)
            else:
                fail("boot", "health timed out")
            yield base, opener
        finally:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=5)


def post(opener, base, body, want=None):
    raw_body = body if isinstance(body, (bytes, bytearray)) else json.dumps(body).encode()
    request = Request(base + ROUTE, data=raw_body, method="POST")
    try:
        with opener.open(request, timeout=20) as resp:
            raw, code = resp.read(4096), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    if want is not None and code != want:
        fail("audit", f"HTTP {code} (want {want})", raw)
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        payload = {}
    return payload, raw, code


def jsonl(audit: Path) -> list:
    rows = []
    for path in sorted(audit.glob("browser-*.jsonl")):
        for line in path.read_bytes().splitlines():
            if line:
                rows.append(json.loads(line))
    return rows


def wait_rows(audit: Path, n: int, timeout=5.0) -> list:
    deadline = time.monotonic() + timeout
    rows = []
    while time.monotonic() < deadline:
        rows = jsonl(audit)
        if len(rows) >= n:
            return rows
        time.sleep(0.05)
    fail("jsonl", f"want {n} rows, got {len(rows)}")


def bulky(mib, n=80):
    target = int(mib * MIB)
    pad = "x" * max(1, (target // n) - 90)
    pack = lambda text: json.dumps({"page_id": "page-size", "events": [
        {"event": "pad.bulk", "ts": "2026-09-12T10:00:00Z", "data": {}, "content": text}
        for _ in range(n)]}).encode()
    body = pack(pad)
    if len(body) < target:
        body = pack(pad + "x" * ((target - len(body)) // n + 1))
    return body


def shapes(uid):
    long_text = "Open session " + ("字" * 1100)
    data = {
        "console.button.state": {"visible": True, "enabled": False, "reason": "unmanaged"},
        "console.button.missing": {"reason": "selector"},
        "console.button.restored": {"reason": "reconnect"},
        "detail.rendered": {"messages": 2, "agent": ""},
        "header.layout": {"width": 1280, "collapsed": False},
        "terminal.pane": {"visible": True, "mode": "native"},
        "dialog.shown": {"id": "settings-dialog"},
        "dialog.closed": {"id": "settings-dialog"},
        "click": {"x": 42, "y": 108, "text": long_text},
        "dom.snapshot": {"reason": "render", "header_state": {
            "selected": uid, "title": "audit parent", "starred": False}},
    }
    return [{"event": name, "ts": "2026-09-12T10:00:00.000Z", "uid": uid,
             "severity": "info", "data": data[name],
             "content": {"composer": "NEVER-STORED-BODY"}} for name in NAMES]


def run(opener, base, audit, uid):
    payload, raw, code = post(opener, base, {
        "page_id": PAGE, "uid": uid, "_build": BUILD, "events": shapes(uid)})
    if not (200 <= code < 300):
        fail("events", f"HTTP {code} (want 2xx)", raw)
    if payload.get("ok") is not True or payload.get("accepted") != len(NAMES) or payload.get("dropped"):
        fail("events", f"accepted {payload}", raw)
    passed("new event names accepted 2xx")
    rows = wait_rows(audit, len(NAMES))
    disk = b"".join(path.read_bytes() for path in sorted(audit.glob("browser-*.jsonl")))
    if b"NEVER-STORED-BODY" in disk or b'"content"' in disk:
        fail("metadata", "JSONL retained content", disk[:240])
    by_name = {}
    for row in rows:
        if "content" in row:
            fail("metadata", "record has content", json.dumps(row).encode())
        if row.get("page_id") != PAGE:
            fail("page_id", f"{row.get('page_id')!r} != {PAGE}", json.dumps(row).encode())
        if not isinstance(row.get("data"), dict) or not str(row.get("event", "")).startswith("browser."):
            fail("metadata", "need browser.* event and data object", json.dumps(row).encode())
        by_name[row["event"].removeprefix("browser.")] = row
    missing = [name for name in NAMES if name not in by_name]
    if missing:
        fail("events", f"JSONL missing {missing}", json.dumps(sorted(by_name)).encode())
    click, snap = by_name["click"], by_name["dom.snapshot"]
    if click["data"].get("x") != 42 or click["data"].get("y") != 108:
        fail("click", "coordinates", json.dumps(click["data"]).encode())
    text = click["data"].get("text")
    if text != "Open session " + "字" * 1100:
        fail("click", "text must retain Python-compatible input", json.dumps(click["data"]).encode())
    header = snap["data"].get("header_state")
    if not isinstance(header, dict) or header.get("selected") != uid:
        fail("dom.snapshot", "header_state object", json.dumps(snap["data"]).encode())
    passed("jsonl structured metadata, page_id, click, header_state")

    three, five = bulky(3), bulky(5)
    if not (3 * MIB <= len(three) < 4 * MIB < len(five)):
        fail("size-fixture", f"3 MiB={len(three)} 5 MiB={len(five)}")
    payload, raw, code = post(opener, base, three)
    if not (200 <= code < 300) or payload.get("ok") is not True or payload.get("dropped"):
        fail("3mib", f"HTTP {code} {payload}", raw)
    passed("3 MiB batch accepted")
    # The declared Content-Length exceeds the body limit, so the server may
    # answer 413 or simply close the connection before the client finishes
    # writing (EPIPE / reset); both are the same refusal.
    try:
        payload, raw, code = post(opener, base, five, want=413)
    except URLError as err:
        if not isinstance(err.reason, OSError) or err.reason.errno not in (32, 104):
            raise
        payload, raw, code = {"error": "body_too_large", "code": "body_too_large"}, b"", 413
    if payload.get("code") != "body_too_large":
        fail("5mib", f"code {payload.get('code')!r}", raw)
    passed("5 MiB batch 413 body_too_large")
    payload, raw, code = post(opener, base, b"not json {", want=400)
    if payload.get("code") != "invalid_audit_request":
        fail("malformed", f"code {payload.get('code')!r}", raw)
    passed("malformed JSON 400")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--fixtures-only", type=Path, metavar="DIR")
    args = parser.parse_args()
    if args.fixtures_only is not None:
        root = args.fixtures_only
        root.mkdir(parents=True, exist_ok=True)
        build(root)
        for path in sorted(p for p in root.rglob("*") if p.is_file()):
            print(path, flush=True)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-audit-events-") as tmp:
        root = Path(tmp)
        corpus = build(root)
        uid = corpus.uid(SID)
        with server_with_env(corpus, {}, args.binary) as (base, opener):
            payload, raw, _code = post(opener, base, {"page_id": PAGE, "events": []}, want=501)
            if payload.get("code") != "not_implemented":
                fail("unconfigured", f"code {payload.get('code')!r}", raw)
            meta = get_json(opener, base, "/api/meta")
            if (meta.get("capabilities") or {}).get("audit") is not False:
                fail("unconfigured-meta", "capabilities.audit must be false", json.dumps(meta).encode())
            passed("unconfigured 501 and meta audit:false")
        audit = root / "audit"
        audit.mkdir()
        audit.chmod(0o700)
        with server_with_env(corpus, {"SESSIONDOCK_AUDIT_DIR": str(audit)}, args.binary) as (base, opener):
            if get_json(opener, base, "/api/meta")["capabilities"].get("audit") is not True:
                fail("configured-meta", "capabilities.audit must be true")
            run(opener, base, audit, uid)


if __name__ == "__main__":
    main()
