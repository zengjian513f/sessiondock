#!/usr/bin/env python3
"""Raw-socket checks of the local-only security middleware.

Starts one isolated loopback server against a synthetic corpus. Never talks
to a non-loopback address or production directories.
"""
from __future__ import annotations

import argparse
import http.client
import json
import os
import socket
import sys
import tempfile
from pathlib import Path
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import BINARY as DEBUG_BINARY, Corpus, REPO, isolated_server

RELEASE = REPO / "target/release" / (
    "sessiondock.exe" if os.name == "nt" else "sessiondock"
)
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
URI_MAX, BODY_MAX = 64 * 1024, 4 * 1024 * 1024
LEAK = ("[workspace]", "SessionDock workspace")


def fail(name, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {name}: {why}; {text[:240]}")


def passed(name):
    print(f"PASS {name}", flush=True)


def loopback(base):
    parsed = urlsplit(base)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not parsed.port:
        fail("base", f"non-loopback {base}")
    return parsed.hostname, parsed.port


def code_of(raw):
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        payload = {}
    return payload.get("code") if isinstance(payload, dict) else None


def body_of(buf):
    idx = buf.find(b"\r\n\r\n")
    return buf[idx + 4:] if idx >= 0 else buf


def call(host, port, method, path, headers=None, body=None, timeout=8):
    conn = http.client.HTTPConnection(host, port, timeout=timeout)
    try:
        conn.request(method, path, body=body, headers=headers or {})
        resp = conn.getresponse()
        return resp.status, resp.read(65536)
    except (TimeoutError, OSError, http.client.HTTPException) as err:
        fail(path, str(err))
    finally:
        conn.close()


def assert_err(host, port, name, method, path, status, code, headers=None, body=None):
    got, raw = call(host, port, method, path, headers, body)
    if got != status or code_of(raw) != code:
        fail(name, f"HTTP {got} code={code_of(raw)!r} (want {status} {code})", raw)
    return raw


def raw_exchange(host, port, blob, timeout=5):
    if host != "127.0.0.1":
        fail("raw", f"non-loopback {host}")
    sock = socket.create_connection((host, port), timeout=timeout)
    try:
        try:
            sock.sendall(blob)
        except OSError:
            pass
        try:
            sock.shutdown(socket.SHUT_WR)
        except OSError:
            pass
        buf = b""
        while True:
            try:
                part = sock.recv(8192)
            except TimeoutError:
                return None, buf
            if not part:
                break
            buf += part
            if len(buf) > 65536:
                break
        status = 0
        if buf.startswith(b"HTTP/1."):
            try:
                status = int(buf.split()[1])
            except (IndexError, ValueError):
                status = 0
        return status, buf
    except OSError as err:
        return -1, str(err).encode()
    finally:
        sock.close()


def run(host, port):
    assert_err(host, port, "host rejection", "GET", "/api/health", 403, "local_only",
               {"Host": "example.com"})
    passed("host rejection")
    star = {"Content-Type": "application/json"}
    assert_err(host, port, "origin rejected", "POST", "/api/session/star", 403, "cross_origin",
               {**star, "Origin": "https://evil.example"}, b"{}")
    passed("origin rejected")
    got, raw = call(host, port, "POST", "/api/session/star",
                    {**star, "Origin": f"http://127.0.0.1:{port}"}, b"{}")
    if got == 403:
        fail("origin accepted", "matching Origin rejected", raw)
    passed("origin accepted")
    for header in ("x-agenthub-protocol", "x-agenthub-node-token"):
        assert_err(host, port, "hub headers", "GET", "/api/health", 403, "hub_unsupported",
                   {header: "1"})
    # Batch 38 H1: hub traffic has its own listener; on loopback a well-formed
    # credential pair is still refused before any handler, on every route.
    pair = {"x-agenthub-protocol": "1", "x-agenthub-node-token": "a" * 48}
    for path in ("/api/meta", "/api/sessions", "/api/nodes", "/"):
        got, raw = call(host, port, "GET", path, pair)
        if got != 403 or code_of(raw) != "hub_unsupported":
            fail("hub headers", f"{path} HTTP {got} code={code_of(raw)!r} (want 403 hub_unsupported)", raw)
    meta = json.loads(call(host, port, "GET", "/api/meta")[1])
    if meta.get("protocol") != 0 or meta.get("node_id") is not None:
        fail("hub headers", "without a node identity /api/meta must stay protocol 0 / node_id null", meta)
    passed("hub headers")
    got, raw = call(host, port, "GET", "/api/health?" + "a" * URI_MAX)
    if got != 414:
        fail("uri too long", f"HTTP {got} code={code_of(raw)!r} (want Python request-line status 414)", raw)
    passed("uri too long")
    blob = (
        f"POST /api/session/star HTTP/1.1\r\nHost: {host}:{port}\r\n"
        f"Content-Type: application/json\r\nContent-Length: {BODY_MAX + 1}\r\n"
        "Connection: close\r\n\r\n"
    ).encode() + b'{"x":"' + b"y" * 64 + b'"}'
    status, buf = raw_exchange(host, port, blob)
    if status != 413 or code_of(body_of(buf)) != "body_too_large":
        fail("body too large", f"HTTP {status} code={code_of(body_of(buf))!r}", buf)
    passed("body too large")
    # Python's terminal handlers do not impose the metadata route's body cap.
    payload = json.dumps({"uid": "claude:missing", "future": "x" * (BODY_MAX + 1)}).encode()
    status, raw = call(host, port, "POST", "/api/term/takeover", star, payload)
    if status != 501:
        fail("terminal body policy", f"HTTP {status} (want disabled terminal 501)", raw)
    passed("terminal body policy")
    # `debug_run` is the list-view selector (Python `filter_rows`), not a
    # policy gate: an id no registry knows is an empty view, HTTP 200.
    status, raw = call(host, port, "GET", "/api/sessions?debug_run=abc")
    try:
        view = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        view = {}
    if status != 200 or not isinstance(view, dict) or view.get("sessions") != []:
        fail("debug_run", f"HTTP {status} body={raw[:200]!r} (want 200 with sessions [])", raw)
    passed("debug_run")
    chunked = (
        f"POST /api/health HTTP/1.1\r\nHost: {host}:{port}\r\n"
        "Transfer-Encoding: chunked\r\nContent-Type: application/json\r\n"
        "Connection: close\r\n\r\nfffffff\r\n{"
    ).encode()
    status, buf = raw_exchange(host, port, chunked)
    if status is None:
        fail("chunked", "no 4xx or close within 5s", buf)
    if status not in (-1, 0) and not (400 <= status < 500):
        fail("chunked", f"HTTP {status} (want 4xx or close)", buf)
    passed("chunked refused")
    got, raw = call(host, port, "POST", "/api/health", star, b"{}")
    if got not in (405, 404):
        fail("method", f"POST /api/health HTTP {got} (want 405 or 404)", raw)
    passed("method not allowed")
    for path in ("/../Cargo.toml", "/legacy-web/../AGENTS.md"):
        got, raw = call(host, port, "GET", path)
        text = raw.decode("utf-8", "replace")
        if got != 404:
            fail("traversal", f"{path} HTTP {got}", raw)
        if any(marker in text for marker in LEAK):
            fail("traversal", f"{path} leaked file contents", raw)
    passed("path traversal")


def main():
    parser = argparse.ArgumentParser(description="Local-only security middleware suite")
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-security-") as tmp:
        root = Path(tmp)
        corpus = Corpus(root)
        for name in ("claude", "codex", "grok"):
            (root / name).mkdir()
        with isolated_server(corpus, args.binary) as (base, _opener):
            host, port = loopback(base)
            run(host, port)
        # Behind an authenticating reverse proxy that forwards `Host $http_host`,
        # SESSIONDOCK_PUBLIC_HOSTS names the exact authorities the gate accepts;
        # anything else is still local_only and Origin is still keyed on Host.
        with isolated_server(corpus, args.binary,
                             extra_env={"SESSIONDOCK_PUBLIC_HOSTS": "example.com, lan.test:8443"}) as (base, _opener):
            host, port = loopback(base)
            for public in ("example.com", "lan.test:8443", "EXAMPLE.COM"):
                got, raw = call(host, port, "GET", "/api/health", {"Host": public})
                if got != 200:
                    fail("public host", f"Host {public}: HTTP {got}", raw)
            assert_err(host, port, "public host: other rejected", "GET", "/api/health", 403, "local_only",
                       {"Host": "other.example"})
            assert_err(host, port, "public host: wrong port rejected", "GET", "/api/health", 403, "local_only",
                       {"Host": "lan.test:9443"})
            got, raw = call(host, port, "POST", "/api/session/star",
                            {"Content-Type": "application/json", "Host": "example.com",
                             "Origin": "https://example.com"}, b"{}")
            if got == 403:
                fail("public host origin", "same-origin POST through the public host was refused", raw)
            assert_err(host, port, "public host: foreign origin", "POST", "/api/session/star", 403, "cross_origin",
                       {"Content-Type": "application/json", "Host": "example.com",
                        "Origin": "https://other.example"}, b"{}")
            passed("public hosts (SESSIONDOCK_PUBLIC_HOSTS)")


if __name__ == "__main__":
    main()
