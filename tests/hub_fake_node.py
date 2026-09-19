#!/usr/bin/env python3
# run_validation: skip
"""A fake SessionDock node for hub tests: stdlib HTTP server, one synthetic session.

A NodeHandler without any
package dependency. Never starts a CLI or reads real sessions.
Answers `/api/meta`, `/api/sessions?sig=`, `/api/live`, `/api/term/list`,
`/api/search` (JSON or NDJSON with `progress=1`), `/api/messages/<uid>`,
`/api/watch` SSE, `/api/media/*`, `/api/trash`, the POST routes the hub
proxies, and a WebSocket echo at `/api/term/attach`.

Usage: hub_fake_node.py --id <32 hex> --name NodeA [--port 0] [--token T]
Prints one JSON line `{"port", "id", "name", "token"}` once listening.

Test control (never token-checked, unaffected by `offline`):
  POST /__control  {"set": {...}} merges into the state, {"pop": [keys]} removes
  GET  /__state    the state (`gets`, `writes`, `uploads`, `frames`, flags)
State keys mirror the fixture: offline, deleted, slow (seconds added to every
/api answer), pad (filler bytes in each session row), token (what the node
expects), term_delay, search_* (steps/delay/stop/prepare_delay/matches/
incomplete/error/json/pool/scanned/truncated), backend, pause_stream, stopped.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import struct
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlparse

PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j0i8AAAAASUVORK5CYII=")
BUILD = "fake-node-build"
WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
BACKENDS = (("ptyhost", "默认宿主"), ("tmux", "tmux"))


def backends(current):
    return [{"name": n, "label": label, "available": True, "current": n == current,
             "unavailable_reason": ""} for n, label in BACKENDS]


class NodeHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def log_message(self, *_args):
        pass

    @property
    def state(self):
        return self.server.state

    # ---- helpers ----------------------------------------------------------

    def _json(self, data, status=200):
        raw = json.dumps(data, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _send(self, status, raw, ctype):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _control(self, u):
        if u.path == "/__state":
            return self._json(self.state)
        if u.path == "/__control":
            raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            body = json.loads(raw or b"{}")
            for key, value in (body.get("set") or {}).items():
                self.state[key] = value
            for key in body.get("pop") or []:
                self.state.pop(key, None)
            return self._json({"ok": True})
        return self._json({"error": "unknown control"}, 404)

    def _gate(self):
        """Fixture rule: a protocol header needs the right token; offline = 503."""
        s = self.state
        if self.headers.get("X-SessionDock-Protocol") and self.headers.get("X-SessionDock-Node-Token") != s["token"]:
            self._json({"error": "forbidden"}, 403)
            return False
        return True

    def messages(self, q):
        start = int(q.get("start", ["0"])[0])
        s = self.state
        return {"meta": s["row"], "version": {"head": "fixed", "size": len(s["messages"]), "mtime": 1},
                "anchor": "anchor", "reset": start == 0, "start": start, "end": len(s["messages"]),
                "messages": s["messages"][start:], "message_total": len(s["messages"]),
                "partial": None, "activity": None, "activity_changed": False}

    # ---- GET ----------------------------------------------------------------

    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        if u.path.startswith("/__"):
            return self._control(u)
        if not self._gate():
            return
        s = self.state
        if u.path.startswith("/api/"):
            if s.get("offline"):
                return self._json({"error": "offline"}, 503)
            if s.get("slow"):
                time.sleep(float(s["slow"]))
        s["gets"].append((u.path, q))
        if u.path == "/api/meta":
            return self._json({"mode": "local", "protocol": 1, "node_id": s["id"],
                               "build": BUILD, "hostname": s["name"]})
        if u.path == "/api/nodes":
            return self._json({"mode": "local", "nodes": []})
        if u.path == "/api/sessions":
            rows = [s["row"]] if not s.get("deleted") else []
            sig = "fixture-" + hashlib.sha256(json.dumps(rows, sort_keys=True).encode()).hexdigest()[:12]
            if q.get("sig", [""])[0] == sig and q.get("force", ["0"])[0] != "1":
                return self._json({"unchanged": True, "sig": sig})
            if s.get("pad"):
                # `pad` bytes of filler in the row: the hub's JSON cap test.
                rows = [{**row, "pad": "x" * int(s["pad"])} for row in rows]
            return self._json({"sessions": rows, "sig": sig, "built_at": 0})
        if u.path == "/api/live":
            return self._json({"uids": [], "tmux_uids": [], "started_at": {}})
        if u.path == "/api/search":
            return self.search(q)
        if u.path == "/api/term/list":
            time.sleep(s.get("term_delay", 0))
            current = s.get("backend", "tmux")
            return self._json({"enabled": s.get("term_enabled", True),
                               "unavailable_reason": s.get("term_reason", ""),
                               "sources": s.get("term_sources", {"claude": True, "codex": True}),
                               "home": "/home/" + s["name"], "sessions": s.get("term_sessions", []),
                               "pending": s["pending"], "backend": current,
                               "backends": backends(current)})
        if u.path == "/api/term/complete-dir":
            return self._json({"directories": ["/home/" + s["name"] + "/work/"]})
        if u.path == "/api/term/new-status":
            return self._json({"waiting": True, "running": True})
        if u.path == "/api/session/outbox":
            return self._json({"outbox": [], "outbox_version": {"epoch": "same-epoch", "revision": 0}})
        if u.path == "/api/session/input-history":
            return self._json({"history": []})
        if u.path.startswith("/api/messages/"):
            if unquote(u.path.rsplit("/", 1)[1]) != s["row"]["uid"]:
                return self._json({"error": "wrong UID"}, 404)
            return self._json(self.messages(q))
        if u.path == "/api/watch":
            return self.watch(q)
        if u.path.startswith("/api/media/"):
            return self._send(200, PNG, "image/png")
        # Staged conversation attachment bytes an editor preview reads back.
        if u.path == "/api/session/conversation/attachment":
            return self._send(200, PNG, "image/png")
        if u.path == "/api/term/attach":
            return self.attach()
        if u.path == "/api/trash":
            return self._json({"items": [{"id": "claude/same-trash", "uid": s["row"]["uid"], "source": "claude",
                                          "title": s["name"] + " deleted", "cwd": "/same/project",
                                          "origin": "/same/file", "deleted_at": "2026-09-01T00:00:00Z",
                                          "size": 10, "restorable": True}], "size": 10})
        if u.path.startswith("/api/"):
            return self._json({"error": "not found"}, 404)
        return self._json({"error": "no static assets"}, 404)

    def search(self, q):
        s = self.state
        data = {"results": [{**s["row"], "hits": 1, "snippet": s["name"] + " needle"}],
                "total_pool": s.get("search_pool", 1), "truncated": s.get("search_truncated", False)}
        if "search_scanned" in s:
            data["scanned"] = s["search_scanned"]
        if q.get("progress") != ["1"] or s.get("search_json"):
            return self._json(data)
        self.send_response(200)
        self.send_header("Content-Type", "application/x-ndjson")
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True

        def emit(event):
            self.wfile.write(json.dumps(event).encode() + b"\n")
            self.wfile.flush()

        try:
            time.sleep(s.get("search_prepare_delay", 0))
            if s.get("search_matches"):
                emit({"type": "matches", "results": data["results"]})
            steps = s.get("search_steps", 1)
            for i in range(s.get("search_stop", steps)):
                emit({"type": "progress", "done": i, "total": steps})
                time.sleep(s.get("search_delay", 0))
            if s.get("search_incomplete"):
                return
            if s.get("search_error"):
                emit({"type": "error", "error": "private upstream details"})
            else:
                emit({"type": "result", "data": data})
        except (BrokenPipeError, ConnectionResetError):
            pass

    def watch(self, q):
        s = self.state
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True
        try:
            last = -1
            while not s.get("stopped"):
                if s.get("pause_stream"):
                    time.sleep(.1)
                    continue
                if last != len(s["messages"]):
                    last = len(s["messages"])
                    data = self.messages(q)
                    self.wfile.write(b"data: " + json.dumps(data).encode() + b"\n\n")
                    q["start"] = [str(last)]
                else:
                    self.wfile.write(b": heartbeat\n\n")
                self.wfile.flush()
                time.sleep(.1)
        except (BrokenPipeError, ConnectionResetError):
            pass

    # ---- WebSocket echo -----------------------------------------------------

    def attach(self):
        key = self.headers.get("Sec-WebSocket-Key")
        if self.headers.get("Upgrade", "").lower() != "websocket" or not key:
            return self._json({"error": "websocket upgrade required"}, 400)
        accept = base64.b64encode(hashlib.sha1((key + WS_GUID).encode()).digest()).decode()
        self.send_response(101)
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.send_header("Sec-WebSocket-Accept", accept)
        self.end_headers()
        self.wfile.flush()
        self.close_connection = True
        s = self.state
        sock = self.connection
        if s.get("attach_error"):
            reason = ("attach failed: " + s["attach_error"]).encode()
            ws_send(sock, struct.pack("!H", 1011) + reason, 0x8)
            return
        ws_send(sock, s["name"].encode(), 0x2)
        try:
            while True:
                op, payload = ws_recv(self.rfile)
                if op is None or op == 0x8:
                    break
                s["frames"].append(payload.decode("utf-8", "replace"))
                ws_send(sock, payload, op)
        except (OSError, ConnectionError, ValueError):
            pass

    # ---- POST -----------------------------------------------------------------

    def do_POST(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        if u.path.startswith("/__"):
            return self._control(u)
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if not self._gate():
            return
        s = self.state
        if s.get("offline") and u.path.startswith("/api/"):
            return self._json({"error": "offline"}, 503)
        if u.path == "/api/session/attachment":
            s["uploads"].append((q, len(raw)))
            return self._json({"ok": True, "name": q["name"][0], "size": len(raw), "attachment_id": "1",
                               "media": {"src": "/api/media/dddddddddddddddddddddddddddddddd"}})
        body = json.loads(raw or b"{}")
        s["writes"].append((u.path, body))
        if u.path == "/api/term/create":
            info = {"name": "same-terminal", "source": body["source"], "sid": "new-sid",
                    "cwd": body["cwd"], "token": "fixture-lease", "started": time.time()}
            s["pending"].append(info)
            return self._json(info)
        if u.path == "/api/term/backend":
            wanted = str(body.get("backend") or "")
            if wanted not in ("tmux", "ptyhost"):
                return self._json({"error": f"未知终端后端: {wanted}"}, 400)
            s["backend"] = wanted
            return self._json({"ok": True, "backend": wanted, "backends": backends(wanted)})
        if u.path == "/api/term/claim":
            return self._json({"ok": True, "token": "fixture-lease"})
        if u.path == "/api/sessions/delete":
            s["deleted"] = True
            return self._json({"ok": True, "deleted": [{"uid": uid} for uid in body["uids"]], "errors": []})
        if u.path == "/api/sessions/fork-visibility":
            return self._json({"ok": True, "updated": [
                {"uid": uid, "fork_parent_visible": body["visible"]} for uid in body["uids"]], "errors": []})
        if u.path == "/api/session/star":
            s["row"]["starred"] = body["starred"]
            return self._json({"uid": body["uid"], "starred": body["starred"]})
        return self._json({"ok": True, "uid": body.get("uid", ""), "path": "/same/file", "removed": 1, "freed": 10})


def ws_send(sock, payload, op):
    head = bytes([0x80 | op])
    n = len(payload)
    if n < 126:
        head += bytes([n])
    elif n < 65536:
        head += bytes([126]) + struct.pack("!H", n)
    else:
        head += bytes([127]) + struct.pack("!Q", n)
    sock.sendall(head + payload)


def ws_recv(rfile):
    head = rfile.read(2)
    if len(head) < 2:
        return None, b""
    op = head[0] & 0x0F
    masked = head[1] & 0x80
    n = head[1] & 0x7F
    if n == 126:
        n = struct.unpack("!H", rfile.read(2))[0]
    elif n == 127:
        n = struct.unpack("!Q", rfile.read(8))[0]
    mask = rfile.read(4) if masked else b""
    payload = rfile.read(n)
    if masked:
        payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
    return op, payload


def start_node(nid, name, port=0, token=None, bind="127.0.0.1"):
    srv = ThreadingHTTPServer((bind, port), NodeHandler)
    srv.daemon_threads = True
    row = {"uid": "claude:same-file-hash", "sid": "same-native-id", "source": "claude",
           "title": name + " session", "cwd": "/same/project", "size": 20,
           "created": "2026-09-01T00:00:00Z", "updated": "2026-09-07T00:00:00Z", "agents": 0}
    srv.state = {"id": nid, "name": name, "token": token or name * 32, "row": row,
                 "messages": [{"role": "user", "text": name + " needle", "ts": row["created"]},
                              {"role": "assistant", "text": "reply " + name, "ts": row["updated"],
                               "media": [{"src": "/api/media/dddddddddddddddddddddddddddddddd",
                                          "mime": "image/png"}]}],
                 "pending": [], "writes": [], "gets": [], "uploads": [], "frames": []}
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--id", required=True, help="32 hex node id")
    ap.add_argument("--name", required=True)
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--token", default=None, help="default: name repeated 32 times")
    ap.add_argument("--bind", default="127.0.0.1")
    args = ap.parse_args()
    srv = start_node(args.id, args.name, args.port, args.token, args.bind)
    print(json.dumps({"port": srv.server_port, "id": args.id, "name": args.name,
                      "token": srv.state["token"]}), flush=True)
    try:
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        pass
    finally:
        srv.state["stopped"] = True
        srv.shutdown()
        srv.server_close()


if __name__ == "__main__":
    sys.exit(main())
