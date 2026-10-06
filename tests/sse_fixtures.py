"""Shared fixture helpers extracted from the former `sse_suite.py` (removed 2026-10-06
with the non-browser suites); imported by browser suites."""
# run_validation: skip
from __future__ import annotations
import http.client
import json
import time
from history_fixtures import cursor_query


# KeepAlive::new().interval(Duration::from_secs(15)).text("ping") in read.rs
KEEPALIVE, POLL, CAP = 15, 4, 2 * 1024 * 1024


def fail(why, body=""):
    if isinstance(body, (bytes, bytearray)):
        text = body.decode("utf-8", "replace")
    elif isinstance(body, dict):
        text = json.dumps(body, ensure_ascii=False, default=str)
    else:
        text = str(body)
    raise SystemExit(f"FAIL {why}; {text[:240]}")


def sse_next(conn, resp, deadline, comments=False):
    name, data, notes, size = "message", [], [], 0
    while time.monotonic() < deadline:
        if conn.sock is not None:
            conn.sock.settimeout(max(deadline - time.monotonic(), 0.05))
        try:
            raw = resp.readline(65536)
        except (TimeoutError, OSError, http.client.HTTPException):
            return None
        if not raw:
            return {"closed": True, "event": "closed"}
        size += len(raw)
        if size > CAP:
            fail("SSE event exceeds 2 MiB", raw)
        line = raw.decode("utf-8", "replace").rstrip("\r\n")
        if line == "":
            if data:
                try:
                    return {"event": name, "data": json.loads("\n".join(data))}
                except json.JSONDecodeError:
                    fail("SSE data is not JSON", "\n".join(data))
            if notes and comments:
                return {"event": "keepalive", "comment": " | ".join(notes)}
            name, data, notes, size = "message", [], [], 0
            continue
        if line.startswith(":"):
            notes.append(line[1:].lstrip() or ""); continue
        key, _, value = line.partition(":")
        value = value[1:] if value.startswith(" ") else value
        if key == "event" and value: name = value
        elif key == "data": data.append(value)
    return None


def open_watch(host, port, uid, snap, **extra):
    path = "/api/watch?" + cursor_query(snap, uid=uid, **extra)
    conn = http.client.HTTPConnection(host, port, timeout=8)
    try:
        conn.request("GET", path, headers={"Accept": "text/event-stream"})
        resp = conn.getresponse()
        if resp.status != 200: fail(f"GET {path} HTTP {resp.status}", resp.read(1024))
        return conn, resp
    except Exception:
        conn.close(); raise
