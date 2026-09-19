#!/usr/bin/env python3
"""HTTP/WebSocket contract of GET /api/term/records and GET /api/term/records/attach.

Isolated free POSIX shell plus an explicit ptyhost directory. No Chromium.
"""
from __future__ import annotations
import argparse, base64, hashlib, json, os, re, socket, struct, tempfile, time, uuid
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import urlparse
from urllib.request import Request
from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, isolated_server
from host_identity import guarded, host, request

BINARY = DEBUG_BINARY
PTYHOST = REPO / "target/debug/ptyhost"
ID_RE = re.compile(r"^\d+-\d+-[A-Za-z0-9._-]+$")
WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
VIEWER_CURSOR = b"\x1b[?25h"


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def call(opener, base, method, route, body=None, want=200):
    req = Request(base + route, data=None if body is None else json.dumps(body).encode(), method=method)
    if body is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with opener.open(req, timeout=20) as resp:
            raw, code = resp.read(2 * 1024 * 1024 + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    except (URLError, TimeoutError, OSError) as err:
        fail(route, str(err))
    if code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    try:
        return (json.loads(raw) if raw else {}), raw
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)


class Ws:
    def __init__(self, sock, leftover=b""):
        self.sock = sock
        self.buf = bytearray(leftover)
        self.binary = bytearray()
        self.texts = []
        self.close_code = None

    def _need(self, n):
        while len(self.buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                return False
            self.buf.extend(chunk)
        return True

    def send(self, payload, op=1):
        if isinstance(payload, str):
            payload = payload.encode()
        mask, n = os.urandom(4), len(payload)
        head = bytes([0x80 | op])
        if n < 126:
            head += bytes([0x80 | n])
        elif n < 65536:
            head += bytes([0x80 | 126]) + struct.pack("!H", n)
        else:
            head += bytes([0x80 | 127]) + struct.pack("!Q", n)
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        self.sock.sendall(head + mask + masked)

    def send_close(self, code=1000):
        self.send(struct.pack("!H", code), op=8)

    def recv_raw(self):
        if not self._need(2):
            return None, b""
        b0, b1 = self.buf[0], self.buf[1]
        del self.buf[:2]
        op, masked, n = b0 & 0x0F, bool(b1 & 0x80), b1 & 0x7F
        if n == 126:
            if not self._need(2):
                return None, b""
            n = struct.unpack("!H", bytes(self.buf[:2]))[0]
            del self.buf[:2]
        elif n == 127:
            if not self._need(8):
                return None, b""
            n = struct.unpack("!Q", bytes(self.buf[:8]))[0]
            del self.buf[:8]
        if n > 8 * 1024 * 1024:
            fail("websocket", f"frame too large {n}")
        mask = b"\0\0\0\0"
        if masked:
            if not self._need(4):
                return None, b""
            mask = bytes(self.buf[:4])
            del self.buf[:4]
        if not self._need(n):
            return None, b""
        payload = bytes(self.buf[:n])
        del self.buf[:n]
        if masked:
            payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        return op, payload

    def recv_app(self):
        while True:
            op, payload = self.recv_raw()
            if op is None:
                return None, b""
            if op == 9:
                try:
                    self.send(payload, op=10)
                except OSError:
                    pass
                continue
            if op == 10:
                continue
            return op, payload

    def apply(self, op, payload):
        if op == 1:
            try:
                self.texts.append(json.loads(payload))
            except json.JSONDecodeError:
                fail("websocket", "text frame is not JSON", payload)
        elif op == 2:
            self.binary.extend(payload)
        elif op == 8:
            self.close_code = struct.unpack("!H", payload[:2])[0] if len(payload) >= 2 else 0

    def pump(self, timeout, until):
        if until(self):
            return True
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.sock.settimeout(max(0.05, min(0.25, deadline - time.monotonic())))
            try:
                op, payload = self.recv_app()
            except socket.timeout:
                continue
            except OSError as err:
                fail("websocket", str(err))
            if op is None:
                return until(self)
            self.apply(op, payload)
            if until(self):
                return True
        return until(self)


def ws_open(base, path, area="websocket"):
    parsed = urlparse(base)
    sock = socket.create_connection((parsed.hostname, parsed.port), timeout=10)
    sock.settimeout(10)
    key = base64.b64encode(os.urandom(16)).decode()
    req = (f"GET {path} HTTP/1.1\r\nHost: {parsed.hostname}:{parsed.port}\r\n"
           f"Upgrade: websocket\r\nConnection: Upgrade\r\n"
           f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
    try:
        sock.sendall(req.encode())
        buf = bytearray()
        while b"\r\n\r\n" not in buf:
            chunk = sock.recv(4096)
            if not chunk:
                fail(area, "upgrade closed", bytes(buf))
            buf.extend(chunk)
            if len(buf) > 65536:
                fail(area, "oversized upgrade", bytes(buf[:240]))
        head, rest = bytes(buf).split(b"\r\n\r\n", 1)
        status = head.split(b"\r\n", 1)[0]
        if b" 101 " not in status:
            fail(area, "no 101", head[:240] + rest[:240])
        headers = {}
        for line in head.split(b"\r\n")[1:]:
            if b":" in line:
                name, value = line.split(b":", 1)
                headers[name.strip().lower()] = value.strip()
        expect = base64.b64encode(hashlib.sha1((key + WS_GUID).encode()).digest())
        if headers.get(b"sec-websocket-accept") != expect:
            fail(area, "bad Sec-WebSocket-Accept", head)
        return Ws(sock, rest)
    except Exception:
        sock.close()
        raise


@contextmanager
def attach_ws(base, rec_id, area="websocket"):
    ws = ws_open(base, f"/api/term/records/attach?id={rec_id}", area)
    try:
        yield ws
    finally:
        try:
            if ws.close_code is None:
                ws.send_close(1000)
        except OSError:
            pass
        try:
            ws.sock.close()
        except OSError:
            pass


def next_text(ws, area):
    ws.sock.settimeout(10)
    try:
        op, payload = ws.recv_app()
    except (socket.timeout, OSError) as err:
        fail(area, str(err))
    if op != 1:
        fail(area, f"expected a text frame, got opcode {op}", payload)
    try:
        msg = json.loads(payload)
    except json.JSONDecodeError:
        fail(area, "text frame is not JSON", payload)
    ws.texts.append(msg)
    return msg


def first_text(ws, area):
    """The stream opens with `timeline` (bounds) and then the `record` frame."""
    timeline = next_text(ws, area)
    if timeline.get("t") != "timeline" or not isinstance(timeline.get("start_ms"), int) \
            or not isinstance(timeline.get("end_ms"), int) or timeline["end_ms"] < timeline["start_ms"]:
        fail(area, "timeline frame", json.dumps(timeline).encode())
    ws.timeline = timeline
    return next_text(ws, area)


def host_send(record, instance, inner, area):
    reply = request(record, guarded(instance, inner))
    if reply.get("ok") is False and inner.get("op") == "resize":
        reply = request(record, inner)
    if reply.get("ok") is False:
        fail(area, f"host {inner.get('op')} rejected", json.dumps(reply).encode())
    return reply


def check_disabled(opener, base):
    err, raw = call(opener, base, "GET", "/api/term/records", want=501)
    if err.get("code") != "terminal_disabled":
        fail("disabled", err.get("code"), raw)
    meta, raw = call(opener, base, "GET", "/api/meta")
    if (meta.get("capabilities") or {}).get("terminal_records") is not False:
        fail("disabled", "terminal_records", raw)
    passed("disabled")


def wait_listed(opener, base, process):
    deadline, last, raw = time.monotonic() + 5, {}, b""
    while time.monotonic() < deadline:
        last, raw = call(opener, base, "GET", "/api/term/records")
        recs = last.get("records") or []
        if recs:
            rec = recs[0]
            if (rec.get("name") != "synthetic-identity-host" or rec.get("live") is not True
                    or rec.get("cols") != 80 or rec.get("rows") != 24
                    or not isinstance(rec.get("segments"), int) or rec["segments"] < 1
                    or not ID_RE.match(str(rec.get("id") or ""))
                    or rec.get("host_pid") != process.pid):
                fail("list_live", rec, raw)
            if len(recs) != 1:
                fail("list_live", f"{len(recs)} records", raw)
            passed("list_live")
            return rec
        time.sleep(0.05)
    fail("list_live", "no records", raw)


def check_bad_id(opener, base):
    err, raw = call(opener, base, "GET", "/api/term/records/attach?id=../x", want=404)
    if err.get("code") != "record_not_found":
        fail("attach_bad_id", err.get("code"), raw)
    err, raw = call(opener, base, "GET", "/api/term/records/attach", want=400)
    if err.get("code") != "invalid_record":
        fail("attach_bad_id", err.get("code"), raw)
    err, raw = call(opener, base, "GET", "/api/term/records/attach?id=1-2-nope", want=404)
    if err.get("code") != "record_not_found":
        fail("attach_bad_id", err.get("code"), raw)
    passed("attach_bad_id")


def check_needs_upgrade(opener, base, rec_id):
    err, raw = call(opener, base, "GET", f"/api/term/records/attach?id={rec_id}", want=400)
    if err.get("code") != "websocket_required":
        fail("attach_needs_upgrade", err.get("code"), raw)
    passed("attach_needs_upgrade")


def run(opener, base, process, record, instance):
    rec = wait_listed(opener, base, process)
    rec_id = rec["id"]
    check_bad_id(opener, base)
    check_needs_upgrade(opener, base, rec_id)

    with attach_ws(base, rec_id, "replay_and_follow") as live:
        msg = first_text(live, "replay_and_follow")
        if (msg.get("t") != "record" or msg.get("cols") != 80 or msg.get("rows") != 24
                or msg.get("live") is not True):
            fail("replay_and_follow", "record frame", json.dumps(msg).encode())
        if not live.pump(5, lambda w: b"RS_SHELL_READY" in w.binary):
            fail("replay_and_follow", "RS_SHELL_READY missing", bytes(live.binary[-240:]))
        host_send(record, instance, {"op": "send", "text": "ping\r"}, "replay_and_follow")
        if not live.pump(5, lambda w: b"RS_PING_OK" in w.binary):
            fail("replay_and_follow", "RS_PING_OK missing", bytes(live.binary[-240:]))
        host_send(record, instance, {"op": "resize", "cols": 90, "rows": 30}, "replay_and_follow")
        if not live.pump(5, lambda w: any(t.get("t") == "resize" and t.get("cols") == 90
                                          and t.get("rows") == 30 for t in w.texts)):
            fail("replay_and_follow", "resize frame", json.dumps(live.texts).encode())

        with attach_ws(base, rec_id, "no_input_accepted") as idle:
            first_text(idle, "no_input_accepted")
            idle.send("x", op=1)
            idle.send(b"y", op=2)
            idle.pump(0.5, lambda w: w.close_code is not None)
            if idle.close_code is not None:
                fail("no_input_accepted", f"closed code={idle.close_code}")
            cap = request(record, {"op": "capture", "styled": False})
            if "RS_UNKNOWN_INPUT" in (cap.get("text") or ""):
                fail("no_input_accepted", "host saw websocket payload", json.dumps(cap).encode())
            passed("no_input_accepted")

        host_send(record, instance, {"op": "send", "text": "quit\r"}, "replay_and_follow")

        def ended(w):
            kinds = [t.get("t") for t in w.texts]
            if "exit" not in kinds:
                return False
            after = False
            for t in w.texts:
                if t.get("t") == "exit":
                    after = True
                elif after and t.get("t") == "end" and VIEWER_CURSOR in w.binary:
                    # The socket stays open after `end`: the timeline can still seek.
                    return True
            return False

        if not live.pump(10, ended):
            fail("replay_and_follow",
                 f"exit/end texts={[t.get('t') for t in live.texts]} close={live.close_code}",
                 bytes(live.binary[-240:]))
        if live.close_code is not None:
            fail("replay_and_follow", f"closed after end code={live.close_code}")
        exit_msg = next(t for t in live.texts if t.get("t") == "exit")
        code = (exit_msg.get("exit") or {}).get("code")
        if code != 0:
            fail("replay_and_follow", f"exit.code={code}", json.dumps(exit_msg).encode())
        passed("replay_and_follow")

    with attach_ws(base, rec_id, "replay_ended") as replay:
        msg = first_text(replay, "replay_ended")
        if msg.get("t") != "record" or msg.get("live") is not False:
            fail("replay_ended", "record frame", json.dumps(msg).encode())

        def finished(w):
            return (b"RS_SHELL_READY" in w.binary and b"RS_PING_OK" in w.binary
                    and any(t.get("t") == "exit" for t in w.texts)
                    and any(t.get("t") == "end" for t in w.texts))

        if not replay.pump(10, finished):
            fail("replay_ended",
                 f"texts={[t.get('t') for t in replay.texts]} close={replay.close_code}",
                 bytes(replay.binary[-240:]))
        exit_msg = next(t for t in replay.texts if t.get("t") == "exit")
        if (exit_msg.get("exit") or {}).get("code") != 0:
            fail("replay_ended", "exit.code", json.dumps(exit_msg).encode())
        if replay.close_code is not None:
            fail("replay_ended", f"closed after end code={replay.close_code}")
        clocks = [t for t in replay.texts if t.get("t") == "clock"]
        if not clocks or not isinstance(clocks[-1].get("unix_ms"), int):
            fail("replay_ended", "clock frames", json.dumps(replay.texts).encode())
        passed("replay_ended")
        check_timeline(replay, rec_id)
    return 8


def check_timeline(ws, rec_id):
    """seek / play / pause on an ended recording: each seek answers with a fresh
    `record` frame (full state at that moment) and a `clock`; play paces the rest
    and ends with `end`; a seek to the end shows the final screen."""
    area = "timeline"
    start = ws.timeline["start_ms"]
    end = max(t.get("end_ms", 0) for t in ws.texts if t.get("t") in ("timeline", "clock"))
    ping_at = None
    # Seek to the very start: nothing of the session's output is on screen yet.
    n_texts, n_bin = len(ws.texts), len(ws.binary)
    ws.send(json.dumps({"t": "seek", "unix_ms": start}), op=1)

    def sought(w):
        fresh = w.texts[n_texts:]
        return any(t.get("t") == "record" for t in fresh) and any(t.get("t") == "clock" for t in fresh)

    if not ws.pump(10, sought):
        fail(area, f"seek(start) texts={[t.get('t') for t in ws.texts[n_texts:]]}")
    rec = next(t for t in ws.texts[n_texts:] if t.get("t") == "record")
    clock = next(t for t in ws.texts[n_texts:] if t.get("t") == "clock")
    if rec.get("live") is not False or rec.get("unix_ms") != clock.get("unix_ms"):
        fail(area, "seek(start) record/clock", json.dumps([rec, clock]).encode())
    if clock["unix_ms"] > start + 1000:
        fail(area, f"seek(start) clock {clock['unix_ms']} far from start {start}")
    ws.pump(0.5, lambda w: False)
    if b"RS_PING_OK" in ws.binary[n_bin:]:
        fail(area, "seek(start) already shows later output", bytes(ws.binary[n_bin:][-240:]))
    # Play at 16x from there: the output arrives paced, then `end`.
    n_texts, n_bin = len(ws.texts), len(ws.binary)
    ws.send(json.dumps({"t": "play", "speed": 16}), op=1)
    if not ws.pump(20, lambda w: b"RS_PING_OK" in w.binary[n_bin:]
                   and any(t.get("t") == "end" for t in w.texts[n_texts:])):
        fail(area, f"play texts={[t.get('t') for t in ws.texts[n_texts:]]}", bytes(ws.binary[n_bin:][-240:]))
    if any(t.get("t") == "record" for t in ws.texts[n_texts:]):
        fail(area, "play must not re-send the opening state", json.dumps(ws.texts[n_texts:]).encode())
    if ws.close_code is not None:
        fail(area, f"closed after play code={ws.close_code}")
    # Seek to the end: the screen at that moment already contains the ping output.
    n_texts, n_bin = len(ws.texts), len(ws.binary)
    ws.send(json.dumps({"t": "seek", "unix_ms": end + 60_000}), op=1)
    if not ws.pump(10, lambda w: any(t.get("t") == "record" for t in w.texts[n_texts:])
                   and any(t.get("t") == "clock" for t in w.texts[n_texts:])
                   and b"RS_PING_OK" in w.binary[n_bin:]):
        fail(area, f"seek(end) texts={[t.get('t') for t in ws.texts[n_texts:]]}", bytes(ws.binary[n_bin:][-240:]))
    clock = next((t for t in ws.texts[n_texts:] if t.get("t") == "clock"), None)
    if not clock or clock["unix_ms"] > end:
        fail(area, f"seek(end) clock {clock} beyond end {end}")
    # Pause is accepted silently; the socket stays usable.
    ws.send(json.dumps({"t": "pause"}), op=1)
    ws.pump(0.3, lambda w: False)
    if ws.close_code is not None:
        fail(area, f"closed after pause code={ws.close_code}")
    passed(area)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    if os.name != "posix":
        print("SKIP term_records_http_suite: POSIX required", flush=True)
        return
    if not args.binary.is_file() or not PTYHOST.is_file():
        missing = args.binary if not args.binary.is_file() else PTYHOST
        raise SystemExit(f"missing {missing}; need target/debug/sessiondock and target/debug/ptyhost")
    with tempfile.TemporaryDirectory(prefix="sessiondock-term-records-http-") as tmp:
        root = Path(tmp)
        for name in ("host", "work", "claude", "codex", "grok"):
            (root / name).mkdir(mode=0o700)
            (root / name).chmod(0o700)
        corpus = Corpus(root)
        with isolated_server(corpus, args.binary) as (base, opener):
            check_disabled(opener, base)
        instance = "synthetic-" + uuid.uuid4().hex
        with host(root, instance) as (process, record):
            with isolated_server(corpus, args.binary, host_dir=root / "host") as (base, opener):
                n = run(opener, base, process, record, instance)
        print(f"PASS term_records_http_suite: {n} scenarios", flush=True)


if __name__ == "__main__":
    main()
