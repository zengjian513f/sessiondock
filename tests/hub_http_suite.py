#!/usr/bin/env python3
"""HTTP contract of the `sessiondock-hub` binary (batch 40 H4) over the wire.

Two `tests/hub_fake_node.py` nodes are registered with the `register`
subcommand (a server-side operation, never an HTTP route); the hub serves
the legacy page in hub mode on a loopback port. Covered: `--check-config`
and fail-closed configuration, `/api/meta`, `/api/nodes`, the static page,
resolve uniqueness (400), the `_build` 409 gate, JSON rewrite, offline 503
shape and recovery, SSE `data:` rewrite, WebSocket bidirectional copy, chunked
attachment upload, display/order routes with their audit records, explicit
node pass-through, the NDJSON search stream with progress, bulk writes and
the `list`/`remove` subcommands. No Chromium, no session root.
"""
from __future__ import annotations

import argparse
import base64
import http.client
import json
import os
import socket
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
NIDS = {"a": "a" * 32, "b": "b" * 32, "c": "c" * 32}


def fail(name, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {name}: {why}; {text[:400]}")


def passed(name):
    print(f"PASS {name}", flush=True)


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
            "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web"),
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


def ws_client(port, path):
    sock = socket.create_connection(("127.0.0.1", port), timeout=10)
    key = base64.b64encode(os.urandom(16)).decode()
    sock.sendall((f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\n"
                  f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
    rfile = sock.makefile("rb")
    status = rfile.readline()
    while rfile.readline() not in (b"\r\n", b""):
        pass
    return sock, rfile, status


def ws_send(sock, payload, op=0x1):
    mask = os.urandom(4)
    head = bytes([0x80 | op])
    n = len(payload)
    if n < 126:
        head += bytes([0x80 | n])
    else:
        head += bytes([0x80 | 126]) + struct.pack("!H", n)
    sock.sendall(head + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(payload)))


def ws_recv(rfile):
    head = rfile.read(2)
    if len(head) < 2:
        return None, b""
    op, n = head[0] & 0x0F, head[1] & 0x7F
    if n == 126:
        n = struct.unpack("!H", rfile.read(2))[0]
    elif n == 127:
        n = struct.unpack("!Q", rfile.read(8))[0]
    return op, rfile.read(n)


def check_config(binary, env):
    result = subprocess.run([str(binary), "--check-config"], env=env, capture_output=True, text=True, timeout=30)
    if result.returncode != 0:
        fail("check-config", result.stderr)
    lines = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
    for key in ("hub_bind", "hub_nodes", "hub_cache_dir", "hub_networks", "web_dir", "audit_dir"):
        if key not in lines:
            fail("check-config", f"missing {key}", result.stdout)
    if lines["hostname"] != "SessionDock":
        fail("check-config", "hostname", result.stdout)
    for name, broken in [("missing nodes file", {"SESSIONDOCK_HUB_NODES": ""}),
                         ("public bind", {"SESSIONDOCK_HUB_BIND": "0.0.0.0:1"}),
                         ("bad networks", {"SESSIONDOCK_HUB_NETWORKS": "10.0.0.1/24"})]:
        result = subprocess.run([str(binary), "--check-config"], env={**env, **broken}, capture_output=True,
                                text=True, timeout=30)
        if result.returncode == 0:
            fail("check-config", f"{name} accepted", result.stdout)
    passed("check-config prints the effective hub settings and fails closed")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", default=str(REPO / "target/release/sessiondock"),
                        help="sessiondock binary; sessiondock-hub is taken from the same directory")
    parser.add_argument("--hub-binary", default=None)
    args = parser.parse_args()
    hub_binary = Path(args.hub_binary) if args.hub_binary else Path(args.binary).resolve().parent / "sessiondock-hub"
    if not hub_binary.is_file():
        fail("setup", f"{hub_binary} not built (cargo build -p sessiondock)")
    nodes = [FakeNode(NIDS["a"], "NodeA"), FakeNode(NIDS["b"], "NodeB")]
    a, b = nodes
    try:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            hub = Hub(hub_binary, root, nodes)
            registry = root / "hub-nodes.json"
            if registry.stat().st_mode & 0o077:
                fail("register", "registry file is not private")
            if a.token in registry.read_text() and "http://127.0.0.1" not in registry.read_text():
                fail("register", "registry shape")
            listing = hub.command("list").stdout
            if NIDS["a"] not in listing or "NodeB" not in listing or a.token in listing:
                fail("list", "subcommand output", listing)
            check_config(hub_binary, hub.env)
            hub.start()
            try:
                run_cases(hub, a, b)
            finally:
                hub.stop()
            hub.command("remove", NIDS["b"])
            listing = hub.command("list").stdout
            if NIDS["b"] in listing or NIDS["a"] not in listing:
                fail("remove", "node still listed", listing)
            if hub.command("remove", NIDS["c"], check=False).returncode == 0:
                fail("remove", "unknown node accepted")
            passed("register/list/remove subcommands keep the registry server-side and private")
    finally:
        for node in nodes:
            node.stop()
    print("PASS hub_http_suite")


def run_cases(hub, a, b):
    status, meta = hub.json("GET", "/api/meta")
    if status != 200 or meta["mode"] != "hub" or meta["protocol"] != 1 or meta["hostname"] != "SessionDock":
        fail("meta", "shape", json.dumps(meta))
    build = meta["build"]
    status, headers, page = hub.request("GET", "/")
    text = page.decode()
    for needle in ('<meta name="agenthub-mode" content="hub">', "SessionDock · 会话管理", "sessiondock.hub.",
                   "location.pathname"):
        if needle not in text:
            fail("page", f"missing {needle!r}")
    if "media_lazy" in text:
        fail("page", "media_lazy must stay undeclared on the hub page")
    status, _, redirect = hub.request("GET", "/files.html?open=1&uid=x")
    if status != 303:
        fail("page", "files.html?open=1 must redirect", redirect)
    status, nodes = hub.json("GET", "/api/nodes")
    if [n["id"] for n in nodes["nodes"]] != [NIDS["a"], NIDS["b"]] or any("url" in n or "token" in n for n in nodes["nodes"]):
        fail("nodes", "public rows", json.dumps(nodes))
    if not all(m["enabled"] for m in nodes["machines"]) or a.token in json.dumps(nodes):
        fail("nodes", "machines", json.dumps(nodes))
    passed("meta, page in hub mode with the namespace script, nodes without credentials")

    for headers, method, path in [({"Host": "example.lan"}, "GET", "/api/meta"),
                                  ({"X-AgentHub-Protocol": "1"}, "GET", "/api/meta"),
                                  ({"Origin": "https://other.invalid"}, "POST", "/api/term/attach")]:
        status, _ = hub.json(method, path, {} if method == "POST" else None, headers)
        if status != 403:
            fail("gate", f"{headers} → {status}")
    status, body = hub.json("POST", "/api/nodes", {"name": "X"})
    if status != 400 or body["error"] != "操作必须明确指定同一台机器":
        fail("gate", "POST /api/nodes", json.dumps(body))
    passed("gate: loopback Host, hub headers, foreign Origin, no registration route")

    ua = scoped(NIDS["a"], "claude:same-file-hash")
    status, body = hub.json("POST", "/api/session/star", {"uid": ua, "starred": True})
    if status != 200 or a.state()["writes"][-1][1]["uid"] != "claude:same-file-hash" or body["uid"] != ua:
        fail("resolve", "star", json.dumps(body))
    status, body = hub.json("POST", "/api/session/rewind", {"uid": ua, "name": f"{NIDS['b']}~same-terminal"})
    if status != 400 or "同一台机器" not in body["error"]:
        fail("resolve", "mixed", json.dumps(body))
    status, body = hub.json("POST", "/api/term/create", {"_node": NIDS["b"], "_build": "stale"})
    if status != 409 or body.get("reload") is not True or body.get("build") != build:
        fail("resolve", "_build 409", json.dumps(body))
    status, body = hub.json("POST", "/api/session/send", {
        "uid": ua, "name": f"{NIDS['a']}~same-terminal", "text": "keep exact text", "_build": build,
        "media": [{"src": f"/api/nodes/{NIDS['a']}/api/media/{'d' * 32}"}]})
    forwarded = a.state()["writes"][-1][1]
    if status != 200 or forwarded["name"] != "same-terminal" or forwarded["media"][0]["src"] != f"/api/media/{'d' * 32}":
        fail("resolve", "send", json.dumps(body))
    status, body = hub.json("GET", f"/api/messages/{ua}?start=0")
    if status != 200 or body["meta"]["uid"] != ua or body["meta"]["node_name"] != "NodeA":
        fail("proxy", "messages rewrite", json.dumps(body))
    if body["messages"][1]["media"][0]["src"] != f"/api/nodes/{NIDS['a']}/api/media/{'d' * 32}":
        fail("proxy", "media src", json.dumps(body))
    status, headers, png = hub.request("GET", f"/api/nodes/{NIDS['a']}/api/media/{'d' * 32}")
    if status != 200 or not png.startswith(b"\x89PNG") or headers.get("Content-Type") != "image/png":
        fail("proxy", "media passthrough", png)
    status, body = hub.json("POST", f"/api/nodes/{NIDS['b']}/api/term/backend", {"backend": "ptyhost"})
    _, listing = hub.json("GET", "/api/term/list")
    if status != 200 or listing["capabilities"][NIDS["b"]]["backend"] != "ptyhost" or listing["capabilities"][NIDS["a"]]["backend"] != "tmux":
        fail("proxy", "explicit backend", json.dumps(listing))
    passed("resolve: one machine, 400 mixed/missing, 409 stale build, JSON and media pass-through, explicit route")

    # SSE: the first data event is rewritten, the connection stays open.
    conn = http.client.HTTPConnection("127.0.0.1", hub.port, timeout=15)
    conn.request("GET", f"/api/watch?uid={ua}&start=0")
    response = conn.getresponse()
    if response.status != 200 or "text/event-stream" not in response.getheader("Content-Type", ""):
        fail("sse", f"{response.status} {response.getheader('Content-Type')}")
    line = b""
    while not line.startswith(b"data: "):
        line = response.readline()
        if not line:
            fail("sse", "stream ended before data")
    event = json.loads(line[6:])
    if event["meta"]["uid"] != ua or event["meta"]["node_id"] != NIDS["a"]:
        fail("sse", "data line not rewritten", line)
    conn.close()
    passed("SSE data lines carry the wire namespace")

    sock, rfile, status = ws_client(hub.port, f"/api/term/attach?name={NIDS['a']}~same-terminal")
    if b" 101 " not in status:
        fail("websocket", "no 101", status)
    op, greeting = ws_recv(rfile)
    if (op, greeting) != (0x2, b"NodeA"):
        fail("websocket", f"greeting {op} {greeting!r}")
    ws_send(sock, b"terminal-echo")
    if ws_recv(rfile)[1] != b"terminal-echo":
        fail("websocket", "text echo")
    ws_send(sock, bytes([0, 255, 17, 0]), 0x2)
    if ws_recv(rfile) != (0x2, bytes([0, 255, 17, 0])):
        fail("websocket", "binary echo")
    ws_send(sock, b"", 0x8)
    sock.close()
    for _ in range(50):
        if len(a.state()["frames"]) == 2:
            break
        time.sleep(0.1)
    else:
        fail("websocket", "node saw " + str(a.state()["frames"]))
    passed("WebSocket frames are copied raw in both directions after the 101")

    payload = bytes(range(256)) * 1200   # 300 KiB, more than one socket chunk
    status, headers, raw = hub.request("POST", f"/api/session/attachment?uid={ua}&name=same-name.png",
                                       headers={"Content-Type": "image/png"}, raw=payload)
    body = json.loads(raw)
    upload = a.state()["uploads"][-1]
    if status != 200 or upload[1] != len(payload) or body["name"] != "same-name.png" or NIDS["a"] not in body["media"]["src"]:
        fail("upload", "chunked forward", raw)
    status, _, raw = hub.request("POST", f"/api/session/attachment?uid=bug-report&node={NIDS['b']}&name=%E6%88%AA%E5%9B%BE.png",
                                 headers={"Content-Type": "image/png"}, raw=b"\x89PNG")
    if status != 200 or json.loads(raw)["name"] != "截图.png" or b.state()["uploads"][-1][1] != 4:
        fail("upload", "bug-report node query", raw)
    status, _, raw = hub.request("POST", "/api/session/attachment?uid=bug-report&name=x.png",
                                 headers={"Content-Type": "image/png"}, raw=b"x")
    if status != 400:
        fail("upload", "guessed a machine", raw)
    passed("attachment uploads stream to the named machine; no machine is guessed")

    # Offline: two failed aggregate fetches strike B out; explicit actions are 503 until it answers.
    b.set(offline=True)
    for _ in range(2):
        hub.json("GET", "/api/sessions?force=1")
    status, body = hub.json("POST", "/api/session/star", {"uid": scoped(NIDS["b"], "claude:same-file-hash"), "starred": True})
    if status != 503 or body.get("node_offline") is not True or body.get("node_id") != NIDS["b"] or "offline_since" not in body:
        fail("offline", "503 shape", json.dumps(body))
    _, sessions = hub.json("GET", "/api/sessions?force=1")
    if not sessions["partial"] or not any(r["node_id"] == NIDS["b"] and r.get("stale") for r in sessions["sessions"]):
        fail("offline", "stale cached rows", json.dumps(sessions)[:400])
    b.pop("offline")
    status, _ = hub.json("POST", "/api/session/star", {"uid": scoped(NIDS["b"], "claude:same-file-hash"), "starred": False})
    if status != 200:
        fail("offline", f"recheck did not recover ({status})")
    passed("offline machine: 503 {error,node_offline,node_id,offline_since}, stale rows, inline recheck")

    status, body = hub.json("POST", f"/api/nodes/{NIDS['a']}/display", {"name": "机房 A", "color": "teal"})
    if status != 200 or body["node"] != {"id": NIDS["a"], "name": "机房 A", "color": "teal", "enabled": True}:
        fail("display", "rename/recolour", json.dumps(body))
    for bad, hint in [({"name": ""}, "不能为空"), ({"color": "#ff0000"}, "机器颜色"), ({"name": "NodeB"}, "已有机器"),
                      ({"enabled": "yes"}, "enabled")]:
        status, body = hub.json("POST", f"/api/nodes/{NIDS['a']}/display", bad)
        if status != 400 or hint not in body["error"]:
            fail("display", f"{bad} → {status} {body}")
    if hub.json("POST", f"/api/nodes/{NIDS['c']}/display", {"name": "x"})[0] != 404:
        fail("display", "unknown machine")
    status, body = hub.json("POST", f"/api/nodes/{NIDS['b']}/display", {"enabled": False})
    _, nodes = hub.json("GET", "/api/nodes")
    if status != 200 or [n["id"] for n in nodes["nodes"]] != [NIDS["a"]] or nodes["machines"][1]["enabled"] is not False:
        fail("display", "untick", json.dumps(nodes))
    if hub.json("GET", f"/api/nodes/{NIDS['b']}/api/live")[0] != 404 or hub.json("GET", f"/api/sessions?nodes={NIDS['b']}")[0] != 400:
        fail("display", "an unticked machine must not exist to the hub")
    hub.json("POST", f"/api/nodes/{NIDS['b']}/display", {"enabled": True})
    status, body = hub.json("POST", "/api/nodes/order", {"ids": [NIDS["b"], NIDS["a"]]})
    _, nodes = hub.json("GET", "/api/nodes")
    if status != 200 or [n["id"] for n in body["machines"]] != [NIDS["b"], NIDS["a"]] or nodes["nodes"][0]["id"] != NIDS["b"]:
        fail("order", "reorder", json.dumps(body))
    for bad in ([NIDS["a"]], [NIDS["a"], NIDS["b"], NIDS["a"]], "ab", None):
        if hub.json("POST", "/api/nodes/order", {"ids": bad})[0] != 400:
            fail("order", f"{bad} accepted")
    for _ in range(50):
        events = hub.audit_events()
        if sum(e["event"] == "hub.node.order.changed" for e in events) == 1:
            break
        time.sleep(0.1)
    else:
        fail("audit", "order record", json.dumps(events))
    display = [e for e in events if e["event"] == "hub.node.display.changed"]
    if not display or display[0]["data"]["to"]["name"] != "机房 A" or display[0]["category"] != "terminal":
        fail("audit", "display record", json.dumps(events))
    passed("display/order routes: validation, untick semantics, order persistence, audit records")

    uids = [scoped(NIDS[c], "claude:same-file-hash") for c in "ab"]
    status, body = hub.json("POST", "/api/sessions/delete", {"uids": uids})
    if status != 200 or {r["uid"] for r in body["deleted"]} != set(uids) or body["errors"]:
        fail("bulk", "delete", json.dumps(body))
    a.set(deleted=False)
    b.set(deleted=False)
    _, trash = hub.json("GET", "/api/trash")
    item = next(row for row in trash["items"] if row["node_name"] == "NodeB")
    if hub.json("POST", "/api/trash/restore", {"id": item["id"]})[0] != 200 or b.state()["writes"][-1][1]["id"] != "claude/same-trash":
        fail("bulk", "restore routes by machine")
    status, body = hub.json("POST", "/api/audit/browser", {"page_id": "p1", "uid": uids[0],
                                                           "events": [{"event": "page.loaded", "uid": uids[0]}]})
    if status != 200 or not any(w[0] == "/api/audit/browser" for w in a.state()["writes"]):
        fail("audit", "browser audit forwarding")
    passed("bulk delete/restore and browser audit route by machine")

    # NDJSON search: progress before, during and after B's slow scan; matches; result.
    b.set(search_steps=3, search_delay=0.2)
    conn = http.client.HTTPConnection("127.0.0.1", hub.port, timeout=30)
    conn.request("GET", "/api/search?q=needle&progress=1")
    response = conn.getresponse()
    if response.status != 200 or "application/x-ndjson" not in response.getheader("Content-Type", ""):
        fail("search", f"{response.status} {response.getheader('Content-Type')}")
    lines = [json.loads(line) for line in iter(response.readline, b"") if line.strip()]
    conn.close()
    b.pop("search_steps", "search_delay")
    kinds = [line["type"] for line in lines]
    if kinds[0] != "progress" or kinds[-1] != "result" or "matches" not in kinds:
        fail("search", "line order", json.dumps(kinds))
    scanning = [line for line in lines if line["type"] == "progress" and any(n["state"] == "scanning" for n in line["nodes"])]
    if not scanning:
        fail("search", "no scanning progress", json.dumps(lines)[:600])
    result = lines[-1]["data"]
    if {r["node_name"] for r in result["results"]} != {"机房 A", "NodeB"} or not all("~" in r["uid"] for r in result["results"]):
        fail("search", "result rows", json.dumps(result)[:600])
    if hub.json("GET", "/api/search?q=needle")[1]["results"].__len__() != 2:
        fail("search", "plain JSON search")
    passed("NDJSON search streams progress, matches and the final result")


if __name__ == "__main__":
    main()
