#!/usr/bin/env python3
"""Graceful shutdown (M7): SIGTERM/SIGINT drain SSE, media, search, and audit."""
import argparse, http.client, json, os, signal, socket, subprocess, sys, tempfile, threading, time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import ProxyHandler, Request, build_opener

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, NoRedirects, codex_row, cursor_query  # noqa: E402
from media_browser import image  # noqa: E402
from native_spans import MIB, images, padded_png  # noqa: E402

NAME = "sessiondock.exe" if os.name == "nt" else "sessiondock"
BINARY = (p if (p := REPO / "target/release" / NAME).is_file() else DEBUG_BINARY)
SID, DRAIN, CAP = "codex-shutdown", 5.0, 4 * 1024 * 1024

def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")

def passed(area): print(f"PASS {area}", flush=True)

def batch(names):
    return json.dumps({"page_id": "page-shut", "uid": "codex:shut", "_build": "test", "events": [
        {"event": n, "ts": "2026-09-12T10:00:00.000Z", "severity": "info", "data": {"n": 1},
         "content": {"x": "never"}} for n in names]}).encode()

def start(corpus, binary, audit):
    env = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0)); port = reservation.getsockname()[1]
    env.update({"SESSIONDOCK_BIND": f"127.0.0.1:{port}", "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web"),
                "SESSIONDOCK_AUDIT_DIR": str(audit)})
    for src in ("claude", "codex", "grok"):
        env["SESSIONDOCK_" + src.upper() + "_ROOT"] = str(corpus.root / src)
    log = tempfile.TemporaryFile(mode="w+b")
    proc = subprocess.Popen([str(binary.resolve(strict=True))], cwd=REPO, env=env, stdout=log, stderr=log)
    opener, base = build_opener(ProxyHandler({}), NoRedirects()), f"http://127.0.0.1:{port}"
    for _ in range(150):
        if proc.poll() is not None: fail("boot", f"exited {proc.returncode}")
        try:
            opener.open(base + "/api/health", timeout=0.4).read(4096); break
        except (OSError, URLError): time.sleep(0.05)
    else: fail("boot", "health timed out")
    return proc, opener, base, port, log

def sse_frame(resp, deadline):
    name, data = "message", []
    while time.monotonic() < deadline:
        try: raw = resp.readline(65536)
        except (TimeoutError, OSError, http.client.HTTPException): return None
        if not raw: return {"event": "closed"}
        line = raw.decode("utf-8", "replace").rstrip("\r\n")
        if line.startswith(":"): continue
        if line == "":
            if data or name != "message": return {"event": name, "data": "\n".join(data)}
            continue
        if line.startswith("event:"): name = line.split(":", 1)[1].strip()
        elif line.startswith("data:"): data.append(line[5:].lstrip() if line.startswith("data: ") else line[5:])
    return None

def until_eof(resp, deadline):
    n, last = 0, b""
    while time.monotonic() < deadline:
        try: chunk = resp.read(65536)
        except (TimeoutError, OSError, http.client.HTTPException): return n, last, "timeout"
        if not chunk: return n, last, "eof"
        n += len(chunk); last = chunk[:240]
    return n, last, "timeout"

def http_get(host, port, path, accept):
    conn = http.client.HTTPConnection(host, port, timeout=8)
    conn.request("GET", path, headers={"Accept": accept}); return conn, conn.getresponse()

def finish_post(sock, body):
    sock.sendall(b"\r\n" + body); buf = b""
    while b"\r\n\r\n" not in buf and len(buf) < 8192:
        chunk = sock.recv(4096)
        if not chunk: break
        buf += chunk
    try: sock.close()
    except OSError: pass
    head, _, rest = buf.partition(b"\r\n\r\n")
    try: return int(head.split()[1]), rest
    except (IndexError, ValueError): return 0, buf

def jsonl(audit):
    names = []
    for path in sorted(audit.glob("browser-*.jsonl")):
        for line in path.read_bytes().splitlines():
            if line: names.append(json.loads(line).get("event"))
    return names

def stop(proc):
    if proc.poll() is not None: return
    proc.terminate()
    try: proc.wait(timeout=5)
    except subprocess.TimeoutExpired: proc.kill(); proc.wait(timeout=5)

def exercise(corpus, binary, audit, sig, extra_term=False):
    tag = "SIGTERM" if sig == signal.SIGTERM else "SIGINT"
    proc, opener, base, port, log = start(corpus, binary, audit)
    host, inflight, watches = "127.0.0.1", {}, []
    try:
        req = Request(base + "/api/audit/browser", data=batch(["pre.shutdown"]), method="POST")
        try:
            with opener.open(req, timeout=10) as resp: code, raw = resp.status, resp.read(4096)
        except HTTPError as err: code, raw = err.code, err.read(4096)
        if code != 202: fail(f"{tag} audit-pre", f"HTTP {code} want 202", raw)
        uid = corpus.uid(SID)
        with opener.open(base + f"/api/messages/{uid}?window=1", timeout=30) as resp:
            snap = json.loads(resp.read(CAP + 1))
        media = images(snap); src = (media[0].get("src") if media else "") or ""
        if not src.startswith("/api/media/"):
            fail(f"{tag} media", "missing descriptor src", json.dumps(snap).encode())
        for _ in range(3):
            conn, resp = http_get(host, port, "/api/watch?" + cursor_query(snap, uid=uid), "text/event-stream")
            if resp.status != 200: fail(f"{tag} SSE", f"HTTP {resp.status}", resp.read(1024))
            frame = sse_frame(resp, time.monotonic() + 4)
            if not frame or frame.get("event") == "closed":
                fail(f"{tag} SSE", "no initial snapshot", json.dumps(frame or {}).encode())
            watches.append((conn, resp))
        search = "/api/search?" + urlencode({"q": "shutdown-needle", "progress": "1"})
        def grab(name, path, accept):
            try:
                conn, resp = http_get(host, port, path, accept)
                if resp.status == 200: n, last, how = until_eof(resp, time.monotonic() + DRAIN)
                else: n, last, how = 0, resp.read(4096), "status"
                inflight[name] = (resp.status, how, n, last); conn.close()
            except Exception as err: inflight[name] = (0, "err", 0, str(err).encode())
        threads = [threading.Thread(target=grab, args=a, daemon=True)
                   for a in (("media", src, "*/*"), ("search", search, "application/x-ndjson"))]
        for thread in threads: thread.start()
        late = batch(["during.drain"])
        sock = socket.create_connection((host, port), timeout=5); sock.settimeout(5)
        sock.sendall(b"POST /api/audit/browser HTTP/1.1\r\nHost: 127.0.0.1\r\n"
                     b"Content-Type: application/json\r\nContent-Length: %d\r\n" % len(late))
        time.sleep(0.15)
        if proc.poll() is not None: fail(f"{tag} signal", f"child exited before signal ({proc.returncode})")
        os.kill(proc.pid, sig); deadline = time.monotonic() + DRAIN
        for conn, resp in watches:
            event = (sse_frame(resp, deadline) or {}).get("event")
            if event not in {"closed", "migration-error", "error", "shutdown"}:
                fail(f"{tag} SSE", f"hung or unexpected event={event!r}")
            conn.close()
        status, body = finish_post(sock, late)
        try: payload = json.loads(body)
        except json.JSONDecodeError: payload = {}
        if status != 503 or payload.get("code") != "shutdown":
            fail(f"{tag} audit-503", f"HTTP {status} body={payload!r}", body)
        for thread in threads: thread.join(timeout=max(0.05, deadline - time.monotonic()))
        for i, name in enumerate(("media", "search")):
            if name not in inflight or threads[i].is_alive(): fail(f"{tag} {name}", "in-flight response hung")
            st, how, _n, last = inflight[name]
            if not (how == "eof" or (st == 503 and b"shutdown" in last)):
                fail(f"{tag} {name}", f"status={st} how={how}", last)
            if name == "media" and st == 200 and how == "eof" and _n != 3 * MIB:
                fail(f"{tag} media", f"bytes={_n} want {3 * MIB}", last)
        if extra_term and proc.poll() is None: os.kill(proc.pid, signal.SIGTERM)
        try: rc = proc.wait(timeout=max(2.0 if extra_term else 0.05, deadline - time.monotonic()))
        except subprocess.TimeoutExpired: fail(f"{tag} exit", "process still running (zombie/hang)")
        if rc != 0: fail(f"{tag} exit", f"exit {rc} want 0")
        if "browser.pre.shutdown" not in jsonl(audit):
            fail(f"{tag} jsonl", f"missing pre-signal events: {jsonl(audit)}")
    finally:
        stop(proc); log.close()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-shutdown-") as tmp:
        root = Path(tmp); corpus = Corpus(root)
        for src in ("claude", "codex", "grok"): (root / src).mkdir()
        b64 = __import__("base64").b64encode(padded_png(3 * MIB)).decode()
        corpus.put(SID, "codex", [
            codex_row("session_meta", {"id": SID, "session_id": SID, "cwd": "/synthetic/shutdown"}),
            codex_row("response_item", {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "shutdown-needle"}, image("codex", b64)]}, 1)], [])
        term, sigint = root / "audit-term", root / "audit-int"
        term.mkdir(mode=0o700); sigint.mkdir(mode=0o700)
        exercise(corpus, args.binary, term, signal.SIGTERM, extra_term=True)
        passed("SIGTERM exit 0 within 5s; second SIGTERM leaves no zombie")
        passed("SSE clients got shutdown/error event or clean EOF")
        passed("in-flight 3 MiB media GET and /api/search?progress=1 ended")
        passed("POST /api/audit/browser during drain is 503 shutdown")
        passed("audit JSONL kept events accepted before SIGTERM")
        exercise(corpus, args.binary, sigint, signal.SIGINT)
        passed("SIGINT same drain (exit 0, SSE EOF, in-flight EOF, 503, JSONL)")

if __name__ == "__main__":
    main()
