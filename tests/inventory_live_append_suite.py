#!/usr/bin/env python3
"""Live-append suite: the session list and an open session keep working while native files are appended concurrently. No Chromium."""
from __future__ import annotations

import argparse
import json
import tempfile
import threading
import uuid
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode

from history_parity import Corpus, claude_row, codex_message, codex_row, encoded, isolated_server

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target/release/sessiondock"
CAP, SIDS = 2 * 1024 * 1024, ("claude-live", "codex-live")

def clip(body):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    return text.replace("\n", " ")[:240]


def call(opener, base, route, timeout=20):
    try:
        with opener.open(base + route, timeout=timeout) as resp:
            raw, code = resp.read(CAP + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(8192), err.code
    except (URLError, TimeoutError, OSError) as err:
        return 0, None, str(err)
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        payload = None
    return code, payload if isinstance(payload, dict) else None, clip(raw)


def msg_path(uid, query=None):
    path = "/api/messages/" + quote(uid, safe=":")
    return path if not query else path + "?" + urlencode(query)


def retryable(code, payload):
    return code in (409, 503) and isinstance(payload, dict) and bool(payload.get("code"))


def projecting(path, source):
    n = 0
    for line in path.read_bytes().splitlines():
        if not line.strip():
            continue
        rec = json.loads(line)
        if source == "claude" and rec.get("type") in ("user", "assistant"):
            n += 1
        elif source == "codex":
            payload = rec.get("payload") or {}
            if rec.get("type") == "response_item" and payload.get("type") == "message":
                n += 1
    return n


def cursor_of(payload):
    ver = (payload or {}).get("version") or {}
    if not isinstance((payload or {}).get("end"), int) or not ver.get("head") or not payload.get("anchor"):
        return None
    return {"start": payload["end"], "head": ver["head"], "anchor": payload["anchor"]}


def build(root):
    corpus = Corpus(root)
    for name in ("claude", "codex", "grok"):
        (root / name).mkdir()
    sid, parent, rows = "claude-live", None, []
    for i in range(20):
        uid = f"c{i:02d}"
        rows.append(claude_row(sid, "user" if i % 2 == 0 else "assistant", uid, parent, f"c{i}"))
        parent = uid
    corpus.put(sid, "claude", rows, [])
    sid = "codex-live"
    rows = [codex_row("session_meta", {
        "id": sid, "session_id": sid, "cwd": "/synthetic/live", "timestamp": "2026-09-11T09:00:00Z"})]
    for i in range(19):
        role = "user" if i % 2 == 0 else "assistant"
        row = codex_message(role, f"x{i}", i + 1)
        row["payload"]["turn_id"] = f"t{i}"
        rows.append(row)
    corpus.put(sid, "codex", rows, [])
    gdir = root / "grok/%2Fsynthetic%2Flive" / "grok-live"
    gdir.mkdir(parents=True)
    (gdir / "summary.json").write_text(json.dumps({
        "generated_title": "Grok live", "info": {"id": "grok-live", "cwd": "/synthetic/live"},
        "created_at": "2026-09-11T08:00:00Z", "updated_at": "2026-09-11T08:30:00Z"}), encoding="utf-8")
    (gdir / "chat_history.jsonl").write_bytes(encoded({
        "type": "user", "content": "grok hello", "prompt_index": 0, "timestamp": "2026-09-11T08:00:00Z"}))
    corpus.paths["grok-live"] = gdir
    return corpus, parent


class Writer(threading.Thread):
    def __init__(self, corpus, parent):
        super().__init__(daemon=True)
        self.corpus, self.parent = corpus, parent
        self.stop, self.appends, self.n = threading.Event(), 0, 0

    def run(self):
        claude, codex = self.corpus.paths["claude-live"], self.corpus.paths["codex-live"]
        while not self.stop.is_set():
            kind = "user" if self.n % 2 == 0 else "assistant"
            uid = uuid.uuid4().hex
            with claude.open("ab") as fh:
                fh.write(encoded(claude_row("claude-live", kind, uid, self.parent, f"live-{self.n}")))
                fh.flush()
            self.parent = uid
            row = codex_message(kind, f"live-{self.n}", 20 + self.n)
            row["payload"]["turn_id"] = f"live-{self.n}-{uid[:8]}"
            with codex.open("ab") as fh:
                fh.write(encoded(row))
                fh.flush()
            self.n += 1
            self.appends += 1
            self.stop.wait(0.02)


def run(opener, base, corpus, rounds, writer):
    failures, prev_u, prev_s, grew = [], {}, {}, False
    uid, prev_n, prev_cur = corpus.uid("claude-live"), 0, None
    for i in range(1, rounds + 1):
        code, payload, raw = call(opener, base, "/api/sessions?force=1")
        sizes = {}
        if code != 200 or not isinstance((payload or {}).get("sessions"), list):
            failures.append(f"list round={i} HTTP {code} {raw}")
        else:
            rows = {row.get("sid"): row for row in payload["sessions"] if isinstance(row, dict)}
            for sid in SIDS:
                row = rows.get(sid)
                if not row:
                    failures.append(f"list round={i} missing {sid}")
                    continue
                u, s = row.get("updated"), row.get("size")
                sizes[sid] = s
                if sid in prev_s:
                    if (isinstance(s, int) and isinstance(prev_s[sid], int) and s < prev_s[sid]) or (
                            isinstance(u, str) and isinstance(prev_u.get(sid), str) and u < prev_u[sid]):
                        failures.append(
                            f"list round={i} {sid} decreased size={prev_s[sid]}->{s} updated={prev_u[sid]}->{u}")
                    if (isinstance(s, int) and s > prev_s[sid]) or (isinstance(u, str) and u > prev_u[sid]):
                        grew = True
                prev_s[sid], prev_u[sid] = s, u
        mcode, mpay, mraw = call(opener, base, msg_path(uid))
        n = len(mpay.get("messages") or []) if mcode == 200 and mpay else None
        if mcode == 200 and mpay is not None:
            if n < prev_n:
                failures.append(f"messages round={i} count {n} < {prev_n}")
            prev_n = n
            if prev_cur:
                ic, ip, ir = call(opener, base, msg_path(uid, prev_cur))
                if ic == 200 and ip is not None:
                    msgs = ip.get("messages") if isinstance(ip.get("messages"), list) else None
                    if not (ip.get("reset") or msgs or ip.get("end") == ip.get("start")):
                        failures.append(f"incremental round={i} neither messages nor reset {ir}")
                elif ic == 500 or not retryable(ic, ip):
                    failures.append(f"incremental round={i} HTTP {ic} {ir}")
            prev_cur = cursor_of(mpay)
        elif mcode == 500 or not retryable(mcode, mpay):
            failures.append(f"messages round={i} HTTP {mcode} {mraw}")
        print(f"round {i}/{rounds} list={code} msgs={mcode}/{n} appends={writer.appends} "
              f"size={sizes.get('claude-live')}/{sizes.get('codex-live')}", flush=True)
        writer.stop.wait(0.05)
    if not grew:
        failures.append("updated/size of live rows never strictly increased")
    writer.stop.set()
    writer.join(timeout=2)
    code, payload, raw = call(opener, base, "/api/sessions?force=1")
    if code != 200 or not isinstance((payload or {}).get("sessions"), list):
        failures.append(f"final list HTTP {code} {raw}")
        rows = {}
    else:
        rows = {row.get("sid"): row for row in payload["sessions"] if isinstance(row, dict)}
    for sid, source in (("claude-live", "claude"), ("codex-live", "codex")):
        path = corpus.paths[sid]
        want, size = projecting(path, source), path.stat().st_size
        row = rows.get(sid)
        if row and row.get("size") != size:
            failures.append(f"final list {sid} size {row.get('size')} != file {size}")
        mcode, mpay, mraw = call(opener, base, msg_path(corpus.uid(sid)))
        got = len(mpay.get("messages") or []) if mcode == 200 and mpay else None
        if mcode != 200 or got != want:
            failures.append(f"final messages {sid} HTTP {mcode} count={got} want={want} {mraw}")
    return failures, writer.appends


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--rounds", type=int, default=30)
    args = parser.parse_args()
    if args.rounds < 1:
        raise SystemExit("--rounds must be >= 1")
    with tempfile.TemporaryDirectory(prefix="sessiondock-inventory-live-") as tmp:
        corpus, parent = build(Path(tmp))
        writer = Writer(corpus, parent)
        with isolated_server(corpus, args.binary) as (base, opener):
            writer.start()
            try:
                failures, appends = run(opener, base, corpus, args.rounds, writer)
            finally:
                writer.stop.set()
                writer.join(timeout=2)
    if failures:
        print("FAIL inventory_live_append_suite:", flush=True)
        for item in failures:
            print(item, flush=True)
        raise SystemExit(1)
    print(f"PASS inventory_live_append_suite: rounds={args.rounds} appends={appends} list_failures=0", flush=True)


if __name__ == "__main__":
    main()
