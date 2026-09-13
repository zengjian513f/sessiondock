#!/usr/bin/env python3
"""Raw-HTTP coverage of GET /api/watch SSE for Claude, Codex and Grok.

Synthetic fixtures, loopback only. Checks delta `start == previous end`,
unfinished JSONL lines, same-size rewrite / truncate / replace reset, delete,
forged anchor, and the documented 15s keepalive comment.
"""
from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import os
import select
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import quote, urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, Corpus, claude_row, codex_message, codex_row,
    cursor_query, encoded, isolated_server)

# KeepAlive::new().interval(Duration::from_secs(15)).text("ping") in read.rs
KEEPALIVE, POLL, CAP = 15, 4, 2 * 1024 * 1024
REPO = Path(__file__).resolve().parents[1]
NAME = "sessiondock.exe" if os.name == "nt" else "sessiondock"
RELEASE = REPO / "target/release" / NAME
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY

def fail(why, body=""):
    if isinstance(body, (bytes, bytearray)):
        text = body.decode("utf-8", "replace")
    elif isinstance(body, dict):
        text = json.dumps(body, ensure_ascii=False, default=str)
    else:
        text = str(body)
    raise SystemExit(f"FAIL {why}; {text[:240]}")

def passed(area): print(f"PASS {area}", flush=True)
def uid_of(source, path): return f"{source}:{hashlib.sha1(str(path).encode()).hexdigest()[:16]}"
def payload(frame): return frame.get("data") if frame and isinstance(frame.get("data"), dict) else None
def texts(body): return [row.get("text") for row in (body.get("messages") or []) if isinstance(row, dict)]
def is_err(frame): return bool(frame and frame.get("event") in {"migration-error", "closed"})
def is_reset(frame): return bool(payload(frame) and payload(frame).get("reset") is True)
def window(host, port, uid): return get(host, port, f"/api/messages/{quote(uid, safe=':')}?window=1")

def message(source, sid, index, text):
    role = "user" if index % 2 == 0 else "assistant"
    if source == "claude":
        return claude_row(sid, role, f"r{index}", None if index == 0 else f"r{index - 1}", text)
    if source == "codex":
        return codex_message(role, text, index + 1)
    return {"type": role, "prompt_index": index // 2 + 1, "content": [{"type": "text", "text": text}]}

def native_bytes(source, sid, texts_):
    rows = [message(source, sid, i, t) for i, t in enumerate(texts_)]
    if source == "codex":
        rows = [codex_row("session_meta", {"id": sid, "session_id": sid, "cwd": "/synthetic/sse"})] + rows
    return b"".join(encoded(r) for r in rows)

def write_native(path, source, sid, texts_):
    path.write_bytes(native_bytes(source, sid, texts_))
    if source == "grok":
        (path.parent / "summary.json").write_text(json.dumps({"info": {"id": sid, "cwd": "/synthetic/sse"}}))

def put(corpus, source, sid, texts_):
    if source != "grok":
        path = corpus.put(sid, source, [message(source, sid, 0, "x")], texts_)
        write_native(path, source, sid, texts_)
        return path
    folder = corpus.root / "grok/project-sse" / sid
    folder.mkdir(parents=True)
    corpus.paths[sid] = folder
    chat = folder / "chat_history.jsonl"
    write_native(chat, source, sid, texts_)
    return chat

def append(path, data):
    with path.open("ab") as fh:
        fh.write(data); fh.flush(); os.fsync(fh.fileno())

def get(host, port, path, timeout=8):
    conn = http.client.HTTPConnection(host, port, timeout=timeout)
    try:
        conn.request("GET", path, headers={"Accept": "application/json"})
        resp = conn.getresponse(); body = resp.read(CAP + 1)
        if resp.status != 200:
            fail(f"GET {path} HTTP {resp.status}", body)
        return json.loads(body)
    finally:
        conn.close()

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

def need(conn, resp, pred, why, timeout=POLL):
    deadline, last = time.monotonic() + timeout, None
    while time.monotonic() < deadline:
        frame = sse_next(conn, resp, deadline, comments=True)
        if not frame: break
        last = frame
        if pred(frame): return frame
    fail(f"{why}: timed out", last)

def exercise(host, port, source, spec):
    uid, path, sid = spec["uid"], spec["path"], spec["sid"]
    if "/.local/share/agenthub" in str(path.resolve()):
        fail("refusing production native path")
    conn, resp = open_watch(host, port, uid, window(host, port, uid))
    try:
        body = payload(need(conn, resp, lambda f: payload(f), f"{source} snapshot"))
        if body.get("reset") is True and texts(body): fail(f"{source} aligned cursor sent a reset window", body)
        end = body["end"]
        append(path, encoded(message(source, sid, 3, f"{source} APPEND")))
        d = payload(need(conn, resp, lambda f: payload(f) and texts(payload(f)), f"{source} append"))
        if d.get("reset") is not False or d.get("start") != end or texts(d) != [f"{source} APPEND"]:
            fail(f"{source} delta must have start==previous end and one message", d)
        passed(f"{source} append-delta"); end = d["end"]
        half = encoded(message(source, sid, 4, f"{source} HALF"))
        append(path, half[:-1])
        until = time.monotonic() + 2.0
        while time.monotonic() < until:
            ready, _, _ = select.select([conn.sock], [], [], max(0.0, until - time.monotonic()))
            if not ready: break
            frame = sse_next(conn, resp, time.monotonic() + 1)
            if is_err(frame) or (payload(frame) and (texts(payload(frame)) or payload(frame).get("end") != end)):
                fail(f"{source} unfinished JSONL line was consumed", frame)
        append(path, half[-1:])
        done = payload(need(conn, resp, lambda f: payload(f) and f"{source} HALF" in texts(payload(f)), f"{source} half"))
        if done.get("reset") is not False: fail(f"{source} completing the line reset the cursor", done)
        passed(f"{source} partial-line")
        prev, raw = path.stat(), path.read_bytes()
        changed = raw.replace(b"PREFIX OLD", b"PREFIX NEW", 1)
        if changed == raw or len(changed) != prev.st_size: fail(f"{source} same-size rewrite fixture missing PREFIX OLD")
        path.write_bytes(changed); os.utime(path, ns=(prev.st_atime_ns, prev.st_mtime_ns))
        rewritten = payload(need(conn, resp, is_reset, f"{source} rewrite"))
        if not any("PREFIX NEW" in (t or "") for t in texts(rewritten)): fail(f"{source} rewrite reset missing PREFIX NEW", rewritten)
        passed(f"{source} same-size-rewrite")
        fc, fr = open_watch(host, port, uid, window(host, port, uid), anchor="forged-anchor")
        try:
            need(fc, fr, is_reset, f"{source} forged-anchor")
        finally:
            fc.close()
        passed(f"{source} forged-anchor")
        write_native(path, source, sid, [f"{source} PAD " + "x" * 6000])
        need(conn, resp, lambda f: is_reset(f) or is_err(f), f"{source} truncate")
        passed(f"{source} truncate")
        write_native(path, source, sid + "-b", [f"{source} OTHER SID"])
        need(conn, resp, lambda f: is_reset(f) or is_err(f), f"{source} replace")
        passed(f"{source} replace")
        path.unlink()
        need(conn, resp, lambda f: is_err(f) or is_reset(f) or (
            payload(f) and payload(f).get("version", {}).get("exists") is False), f"{source} delete")
        passed(f"{source} delete")
    finally:
        conn.close()

def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    files = {}
    for source in ("claude", "codex", "grok"):
        sid = f"{source}-sse"
        path = put(corpus, source, sid, [f"{source} PAD " + "x" * 6000, f"{source} PREFIX OLD", f"{source} LAST"])
        files[source] = {"sid": sid, "path": path, "uid": uid_of(source, corpus.paths[sid])}
    put(corpus, "claude", "claude-idle", ["idle keepalive"])
    return corpus, files, {"uid": uid_of("claude", corpus.paths["claude-idle"])}

def run(host, port, files, idle):
    conn, resp = open_watch(host, port, idle["uid"], window(host, port, idle["uid"]))
    try:
        need(conn, resp, lambda f: payload(f), "idle snapshot")
        started = time.monotonic()
        for source in ("claude", "codex", "grok"):
            exercise(host, port, source, files[source])
        need(conn, resp, lambda f: f.get("event") == "keepalive" or "ping" in str(f.get("comment") or ""),
             "keepalive ping", timeout=max(0.5, started + 2 * KEEPALIVE - time.monotonic()))
        passed("heartbeat ping within 15s")
    finally:
        conn.close()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-sse-suite-") as tmp:
        corpus, files, idle = build(Path(tmp))
        with isolated_server(corpus, args.binary) as (base, _opener):
            parsed = urlsplit(base.rstrip("/"))
            if parsed.scheme != "http" or parsed.hostname != "127.0.0.1": fail("refusing non-loopback base")
            run(parsed.hostname, parsed.port or 80, files, idle)

if __name__ == "__main__":
    main()
