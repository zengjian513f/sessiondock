#!/usr/bin/env python3
"""HTTP contract of the node listener (batch 38 H1): `SESSIONDOCK_NODE_BIND`
with token file, id file and peer networks; `X-AgentHub-Node-Token` +
`X-AgentHub-Protocol: 1` from an allowed peer, 403 otherwise; the loopback
listener keeps refusing hub headers; the node listener serves `/api` only.

Both listeners bind 127.0.0.1 (the only address a test can bind) with
`SESSIONDOCK_NODE_PEERS=127.0.0.0/8`; a second run with peers that exclude
loopback proves the source-IP gate. Synthetic corpus, isolated directories,
no Chromium, no real CLI home.
"""
from __future__ import annotations

import argparse
import http.client
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
RELEASE = REPO / "target/release/sessiondock"
DEBUG = REPO / "target/debug/sessiondock"
BINARY = RELEASE if RELEASE.is_file() else DEBUG
TOKEN = "suite-t0ken.suite-t0ken.suite-t0ken.suite-t0ken~"
GOOD = {"X-AgentHub-Protocol": "1", "X-AgentHub-Node-Token": TOKEN}
AUTH_403 = {"error": "node authentication required", "code": "node_auth_required"}
PEER_403 = {"error": "forbidden", "code": "node_peer_denied"}


def fail(name, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {name}: {why}; {text[:300]}")


def passed(name):
    print(f"PASS {name}", flush=True)


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def corpus(root: Path):
    for name in ("claude", "codex", "grok"):
        (root / name).mkdir()
    project = root / "claude/project-history"
    project.mkdir()
    rows = [
        {"type": "user", "uuid": "u1", "parentUuid": None, "sessionId": "node-auth-1",
         "cwd": "/example/project", "timestamp": "2026-01-01T00:00:00.000Z",
         "message": {"role": "user", "content": "节点鉴权套件"}},
        {"type": "assistant", "uuid": "a1", "parentUuid": "u1", "sessionId": "node-auth-1",
         "timestamp": "2026-01-01T00:00:01.000Z",
         "message": {"role": "assistant", "content": [{"type": "text", "text": "人工合成回答"}]}},
    ]
    (project / "node-auth-1.jsonl").write_text(
        "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows), encoding="utf-8")


def environment(root: Path, port: int, extra: dict[str, str]):
    env = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    env.update({
        "SESSIONDOCK_BIND": f"127.0.0.1:{port}", "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web"),
        "SESSIONDOCK_CLAUDE_ROOT": str(root / "claude"), "SESSIONDOCK_CODEX_ROOT": str(root / "codex"),
        "SESSIONDOCK_GROK_ROOT": str(root / "grok"),
    })
    env.update(extra)
    return env


def node_env(root: Path, node_port: int, peers: str):
    token = root / "node-token"
    if not token.exists():
        token.touch(mode=0o600)
        token.write_text(TOKEN + "\n", encoding="utf-8")
    return {
        "SESSIONDOCK_NODE_BIND": f"127.0.0.1:{node_port}",
        "SESSIONDOCK_NODE_TOKEN_FILE": str(token),
        "SESSIONDOCK_NODE_ID_FILE": str(root / "ids" / "node-id"),
        "SESSIONDOCK_NODE_PEERS": peers,
    }


class Server:
    """One isolated binary; the exact child is terminated on exit."""

    def __init__(self, binary: Path, root: Path, extra: dict[str, str]):
        self.port = free_port()
        self.env = environment(root, self.port, extra)
        self.log = tempfile.TemporaryFile(mode="w+b")
        self.process = subprocess.Popen([str(binary)], cwd=REPO, env=self.env,
                                        stdout=self.log, stderr=self.log)

    def wait_healthy(self, name):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                fail(name, f"server exited {self.process.returncode}", self.excerpt())
            try:
                status, _ = call(self.port, "GET", "/api/health")
                if status == 200:
                    return self
            except OSError:
                pass
            time.sleep(0.05)
        self.stop()
        fail(name, "health check timed out", self.excerpt())

    def wait_exit(self, name):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            code = self.process.poll()
            if code is not None:
                return code
            time.sleep(0.05)
        self.stop()
        fail(name, "expected a startup refusal but the server kept running", self.excerpt())

    def excerpt(self):
        self.log.seek(0)
        return self.log.read(600)

    def stop(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.log.close()


def call(port, method, path, headers=None, host=None, timeout=8):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    try:
        merged = {"Host": host or f"127.0.0.1:{port}"}
        merged.update(headers or {})
        conn.request(method, path, headers=merged)
        response = conn.getresponse()
        return response.status, response.read(4 * 1024 * 1024)
    finally:
        conn.close()


def parse(raw):
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return None


def expect(name, port, path, status, body=None, code=None, headers=None, host=None):
    got, raw = call(port, "GET", path, headers, host)
    payload = parse(raw)
    if got != status:
        fail(name, f"{path} HTTP {got} (want {status})", raw)
    if body is not None and payload != body:
        fail(name, f"{path} body {payload!r} (want {body!r})", raw)
    if code is not None and (not isinstance(payload, dict) or payload.get("code") != code):
        fail(name, f"{path} code {payload!r} (want {code})", raw)
    return payload


def run(binary: Path, root: Path):
    corpus(root)
    (root / "ids").mkdir()
    node_port = free_port()
    server = Server(binary, root, node_env(root, node_port, "127.0.0.0/8")).wait_healthy("start")
    try:
        loop = server.port
        id_file = root / "ids" / "node-id"
        node_id = id_file.read_text(encoding="utf-8").strip()
        if len(node_id) != 32 or any(ch not in "0123456789abcdef" for ch in node_id):
            fail("id-minted", f"node id file holds {node_id!r}")
        if id_file.stat().st_mode & 0o777 != 0o600:
            fail("id-minted", f"node id file mode {oct(id_file.stat().st_mode & 0o777)}")
        passed("id-minted")

        meta = expect("meta-loopback", loop, "/api/meta", 200)
        if meta.get("protocol") != 1 or meta.get("node_id") != node_id or meta.get("mode") != "local":
            fail("meta-loopback", "protocol/node_id/mode", meta)
        if meta.get("capabilities", {}).get("hub") is not False:
            fail("meta-loopback", "capabilities.hub must stay false", meta)
        passed("meta-loopback")

        expect("nodes-loopback", loop, "/api/nodes", 200,
               {"mode": "local", "nodes": [{"id": node_id, "name": meta["hostname"], "online": True}]})
        passed("nodes-loopback")

        for header in ("X-AgentHub-Protocol", "X-AgentHub-Node-Token"):
            expect("loopback-hub-headers", loop, "/api/sessions", 403, code="hub_unsupported",
                   headers={header: GOOD[header]})
        expect("loopback-hub-headers", loop, "/api/sessions", 403, code="hub_unsupported", headers=GOOD)
        passed("loopback-hub-headers")

        cases = {
            "node-no-headers": {},
            "node-protocol-only": {"X-AgentHub-Protocol": "1"},
            "node-token-only": {"X-AgentHub-Node-Token": TOKEN},
            "node-wrong-protocol": {"X-AgentHub-Protocol": "99", "X-AgentHub-Node-Token": TOKEN},
            "node-wrong-token": {"X-AgentHub-Protocol": "1", "X-AgentHub-Node-Token": TOKEN[:-1] + "-"},
            "node-short-token": {"X-AgentHub-Protocol": "1", "X-AgentHub-Node-Token": TOKEN[:-1]},
        }
        for name, headers in cases.items():
            expect(name, node_port, "/api/sessions", 403, AUTH_403, headers=headers)
            passed(name)

        local = expect("node-ok-sessions", loop, "/api/sessions", 200)
        through = expect("node-ok-sessions", node_port, "/api/sessions", 200, headers=GOOD)
        if through.get("sessions") != local.get("sessions") or through.get("sig") != local.get("sig"):
            fail("node-ok-sessions", "node listing differs from the loopback listing", through)
        if len(through["sessions"]) != 1:
            fail("node-ok-sessions", f"{len(through['sessions'])} rows (want 1)", through)
        passed("node-ok-sessions")

        uid = local["sessions"][0]["uid"]
        messages = expect("node-ok-messages", node_port, f"/api/messages/{uid}", 200, headers=GOOD)
        if not messages.get("messages"):
            fail("node-ok-messages", "no messages through the node listener", messages)
        passed("node-ok-messages")

        node_meta = expect("node-meta", node_port, "/api/meta", 200, headers=GOOD)
        if node_meta.get("protocol") != 1 or node_meta.get("node_id") != node_id:
            fail("node-meta", "protocol/node_id", node_meta)
        passed("node-meta")

        # No Host gate on the node listener: the hub addresses the interface IP.
        expect("node-any-host", node_port, "/api/meta", 200, headers=GOOD, host="10.100.100.2:8742")
        passed("node-any-host")

        for path in ("/", "/index.html", "/app.js"):
            expect("node-static-404", node_port, path, 404, code="not_found", headers=GOOD)
        expect("node-static-404", node_port, "/", 403, AUTH_403)
        passed("node-static-404")

        expect("node-cross-site", node_port, "/api/sessions", 403, code="cross_site",
               headers={**GOOD, "Sec-Fetch-Site": "cross-site"})
        passed("node-cross-site")

        view = expect("node-debug-run", node_port, "/api/sessions?debug_run=x", 200, headers=GOOD)
        if view.get("sessions") != []:
            fail("node-debug-run", f"unknown run must be an empty view: {view!r}", b"")
        passed("node-debug-run")
    finally:
        server.stop()

    # Peer gate: loopback is not inside the configured networks.
    other_port = free_port()
    server = Server(binary, root, node_env(root, other_port, "10.100.100.0/24")).wait_healthy("peers")
    try:
        expect("node-peer-denied", other_port, "/api/sessions", 403, PEER_403, headers=GOOD)
        expect("node-peer-denied", other_port, "/api/meta", 403, PEER_403, headers=GOOD)
        meta = expect("id-stable", server.port, "/api/meta", 200)
        if meta.get("node_id") != node_id:
            fail("id-stable", "restart changed the node id", meta)
        passed("node-peer-denied")
        passed("id-stable")
    finally:
        server.stop()

    # Fail closed: three of four settings never start a service.
    partial = node_env(root, free_port(), "127.0.0.0/8")
    del partial["SESSIONDOCK_NODE_PEERS"]
    server = Server(binary, root, partial)
    code = server.wait_exit("fail-closed")
    excerpt = server.excerpt()
    server.stop()
    if code == 0 or b"set together" not in excerpt:
        fail("fail-closed", f"exit {code}", excerpt)
    passed("fail-closed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-node-auth-") as temporary:
        run(binary, Path(temporary))
    print("PASS node_auth_suite", flush=True)


if __name__ == "__main__":
    main()
