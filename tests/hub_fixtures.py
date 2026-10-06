"""Shared fixture helpers extracted from the former `hub_http_suite.py` (removed 2026-10-06
with the non-browser suites); imported by browser suites."""
# run_validation: skip
from __future__ import annotations
import http.client
import json
import os
import socket
import subprocess
import sys
import time
from pathlib import Path
from frontend_paths import frontend_dir


REPO = Path(__file__).resolve().parents[1]


def fail(name, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {name}: {why}; {text[:400]}")


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def scoped(nid, uid):
    source, tail = uid.split(":", 1)
    return f"{source}:{nid}~{tail}"


class FakeNode:
    def __init__(self, nid, name):
        self.process = subprocess.Popen(
            [sys.executable, str(REPO / "tests/hub_fake_node.py"), "--id", nid, "--name", name],
            stdout=subprocess.PIPE, text=True)
        info = json.loads(self.process.stdout.readline())
        self.port, self.token, self.nid, self.name = info["port"], info["token"], nid, name

    def control(self, path, body=None):
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        conn.request("POST" if body is not None else "GET", path, json.dumps(body) if body is not None else None)
        response = conn.getresponse()
        data = json.loads(response.read())
        conn.close()
        return data

    def set(self, **values):
        assert self.control("/__control", {"set": values})["ok"]

    def pop(self, *keys):
        assert self.control("/__control", {"pop": list(keys)})["ok"]

    def state(self):
        return self.control("/__state")

    def stop(self):
        self.process.kill()
        self.process.wait()


class Hub:
    def __init__(self, binary, root, nodes):
        self.binary, self.root = binary, root
        self.port = free_port()
        self.env = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
        audit = root / "audit"
        audit.mkdir(mode=0o700)
        self.env.update({
            "SESSIONDOCK_HUB_BIND": f"127.0.0.1:{self.port}",
            "SESSIONDOCK_HUB_NODES": str(root / "hub-nodes.json"),
            "SESSIONDOCK_HUB_CACHE_DIR": str(root / "hub-cache"),
            "SESSIONDOCK_HUB_NETWORKS": "127.0.0.0/8",
            "SESSIONDOCK_WEB_DIR": str(frontend_dir()),
            "SESSIONDOCK_AUDIT_DIR": str(audit),
        })
        for node in nodes:
            token = root / f"{node.name}.token"
            token.write_text(node.token + "\n")
            token.chmod(0o600)
            self.command("register", "--name", node.name, "--url", f"http://127.0.0.1:{node.port}",
                         "--token-file", str(token))
        self.process = None

    def command(self, *args, env=None, check=True):
        result = subprocess.run([str(self.binary), *args], env=env or self.env, capture_output=True, text=True,
                                timeout=30)
        if check and result.returncode != 0:
            fail("hub " + " ".join(args), f"exit {result.returncode}", result.stderr)
        return result

    def start(self):
        self.process = subprocess.Popen([str(self.binary)], env=self.env, cwd=REPO,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        for _ in range(200):
            if self.process.poll() is not None:
                fail("hub start", self.process.stderr.read())
            try:
                status, _, _ = self.request("GET", "/api/meta")
                if status == 200:
                    return
            except OSError:
                pass
            time.sleep(0.05)
        fail("hub start", "no /api/meta within 10 s")

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()

    def request(self, method, path, body=None, headers=None, raw=None):
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=30)
        payload = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
        base = {"Content-Type": "application/json"} if body is not None else {}
        conn.request(method, path, payload, {**base, **(headers or {})})
        response = conn.getresponse()
        data = response.read()
        result = (response.status, response.headers, data)   # HTTPMessage: case-insensitive keys
        conn.close()
        return result

    def json(self, method, path, body=None, headers=None):
        status, response_headers, data = self.request(method, path, body, headers)
        try:
            return status, json.loads(data) if data else None
        except ValueError:
            return status, {"raw": data.decode("utf-8", "replace")}

    def audit_events(self):
        events = []
        for path in sorted((self.root / "audit").glob("hub-*.jsonl")):
            events.extend(json.loads(line) for line in path.read_text().splitlines() if line)
        return events
