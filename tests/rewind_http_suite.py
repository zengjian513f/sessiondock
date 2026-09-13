#!/usr/bin/env python3
"""HTTP-only contract of persisted Claude timeline pins (POST /api/session/rewind).

Codes/fields from api/metadata.rs, claude_rewind_target, rewind_http.rs HTTP
scenarios (not copied Rust). Display pin only; native_rewind is always false.
Synthetic temp fixtures, loopback only.
"""
from __future__ import annotations

import argparse, json, sys, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote, urlencode
from urllib.request import Request

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, codex_message, codex_row,
    cursor_query, encoded, isolated_server)

BINARY = (p if (p := REPO / "target/release" / DEBUG_BINARY.name).is_file() else DEBUG_BINARY)
SID, RW = "claude-pin", "/api/session/rewind"
PINNED = ["q1", "a1", "q2", "a2", "q3", "a3"]
FULL = PINNED + ["q4", "a4", "q5", "a5", "q6", "a6"]


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area): print(f"PASS {area}", flush=True)


def call(opener, base, path, body=None, data=None):
    blob = data if data is not None else (None if body is None else json.dumps(body).encode())
    req = Request(base + path, data=blob, method="GET" if blob is None else "POST")
    if blob is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with opener.open(req, timeout=15) as resp:
            raw, code = resp.read(2 * 1024 * 1024 + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(8192), err.code
    except Exception as err:
        fail(path, str(err))
    try:
        parsed = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        parsed = {}
    return code, parsed if isinstance(parsed, dict) else {}, raw


def want(opener, base, path, status, body=None, data=None, code=None):
    got, payload, raw = call(opener, base, path, body, data)
    if got != status or (code and payload.get("code") != code):
        fail(path, f"HTTP {got} code={payload.get('code')!r} (want {status} {code})", raw)
    return payload, raw


def texts(payload):
    return [m.get("text") for m in payload.get("messages") or [] if isinstance(m, dict)]


def mpath(uid, extra="", **q):
    path = "/api/messages/" + quote(uid, safe=":")
    return path + ("?" + (extra or urlencode(q)) if extra or q else "")


def listed(opener, base):
    payload, raw = want(opener, base, "/api/sessions?force=1", 200)
    rows = payload.get("sessions")
    if not isinstance(rows, list): fail("/api/sessions", "sessions not a list", raw)
    return {row.get("sid"): row for row in rows if isinstance(row, dict)}, raw


def pin(opener, base, uid, target, status=200, code=None):
    return want(opener, base, RW, status, {"uid": uid, "target": target}, code=code)


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    rows, parent = [], None
    for i in range(1, 7):
        u, a = f"u{i}", f"a{i}"
        rows += [claude_row(SID, "user", u, parent, f"q{i}"),
                 claude_row(SID, "assistant", a, u, f"a{i}")]
        parent = a
        if i == 3:
            rows += [claude_row(SID, "user", "old-u", "a3", "abandoned q"),
                     claude_row(SID, "assistant", "old-a", "old-u", "abandoned a")]
    corpus.put(SID, "claude", rows, [])
    pre = [codex_row("session_meta", {
        "id": "codex-parent", "session_id": "codex-parent", "cwd": "/synthetic/rewind"}),
           codex_message("user", "parent")]
    cut = sum(len(encoded(r)) for r in pre)
    corpus.put("codex-parent", "codex", pre, [])
    corpus.put("codex-fork", "codex", [
        codex_row("session_meta", {
            "id": "codex-fork", "session_id": "codex-fork", "cwd": "/synthetic/rewind",
            "forked_from_id": "codex-parent", "history_mode": "paginated",
            "history_base": {"thread_id": "codex-parent", "end_byte_offset": cut}}),
        codex_message("user", "fork")], [])
    return corpus


def append(path, *rows):
    path.write_bytes(path.read_bytes() + b"".join(encoded(r) for r in rows))


def prefs(rows, raw=b""):
    claude, parent = rows.get(SID) or {}, rows.get("codex-parent") or {}
    if claude.get("starred") is not True or parent.get("fork_parent_visible") is not True:
        fail("prefs", f"starred={claude.get('starred')} visible={parent.get('fork_parent_visible')}", raw)


def reason(opener, base, uid, want_reason, extra=""):
    body, raw = want(opener, base, mpath(uid, extra), 200)
    pin_meta = (body.get("meta") or {}).get("timeline_pin") or {}
    rows, lraw = listed(opener, base)
    row = (rows.get(SID) or {}).get("timeline_pin") or {}
    if pin_meta.get("retired") is not True or pin_meta.get("retired_reason") != want_reason \
            or row.get("retired_reason") != want_reason or row.get("native_rewind") is not False:
        fail(want_reason, f"meta={pin_meta} row={row}", raw or lraw)
    return body, raw


def enabled(opener, base, corpus):
    meta, raw = want(opener, base, "/api/meta", 200)
    if (meta.get("capabilities") or {}).get("timeline_pin") is not True: fail("meta", "timeline_pin should be true", raw)
    rows, raw = listed(opener, base)
    claude, parent = rows.get(SID), rows.get("codex-parent")
    if not claude or not parent: fail("list", "need claude-pin and codex-parent", raw)
    uid, puid, path = claude["uid"], parent["uid"], corpus.paths[SID]
    native = path.read_bytes()
    want(opener, base, "/api/session/star", 200, {"uid": uid, "starred": True})
    want(opener, base, "/api/sessions/fork-visibility", 200, {"uids": [puid], "visible": True})
    before, braw = want(opener, base, mpath(uid), 200)
    if texts(before) != FULL: fail("natural", f"texts={texts(before)}", braw)
    body, raw = pin(opener, base, uid, "u4")
    if body.get("ok") is not True or body.get("pinned") is not True \
            or body.get("native_rewind") is not False or body.get("tip") != "a3" \
            or body.get("target") != "u4" or body.get("stale_end") != len(native) \
            or (body.get("timeline_pin") or {}).get("retired") is not False:
        fail("pin", "200 shape/tip/native_rewind", raw)
    passed("pin u4 200 native_rewind:false tip=a3")
    win, wraw = want(opener, base, mpath(uid, window="1"), 200)
    rows, lraw = listed(opener, base)
    row = (rows.get(SID) or {}).get("timeline_pin") or {}
    if texts(win) != PINNED or "q4" in texts(win) or row.get("target") != "u4" or row.get("tip") != "a3":
        fail("window", f"texts={texts(win)} pin={row}", wraw or lraw)
    passed("window=1 ends before u4; sessions timeline_pin")
    reset, rraw = want(opener, base, mpath(uid, cursor_query(before)), 200)
    if reset.get("reset") is not True or texts(reset) != PINNED: fail("reset", f"reset={reset.get('reset')} texts={texts(reset)}", rraw)
    passed("pre-pin cursor reset:true")
    pin(opener, base, uid, "never-written", 404)
    pin(opener, base, uid, "old-u", 409)
    passed("unknown 404; abandoned branch 409")
    rows, raw = listed(opener, base)
    prefs(rows, raw)
    if path.read_bytes() != native: fail("native", "pin rewrote native file")
    passed("star/visibility untouched by pin")
    want(opener, base, RW, 200, data=json.dumps(
        {"uid": uid, "target": "u4", "padding": "x" * (5 * 1024 * 1024)}).encode())
    passed("large ignored rewind field accepted")
    return uid


def after_restart(opener, base, corpus, uid):
    path = corpus.paths[SID]
    win, wraw = want(opener, base, mpath(uid, window="1"), 200)
    rows, raw = listed(opener, base)
    if texts(win) != PINNED or (rows.get(SID) or {}).get("timeline_pin", {}).get("target") != "u4":
        fail("restart", f"texts={texts(win)} pin={(rows.get(SID) or {}).get('timeline_pin')}", wraw or raw)
    passed("pin survives restart")
    cleared, craw = pin(opener, base, uid, None)
    natural, nraw = want(opener, base, mpath(uid), 200)
    if cleared.get("pinned") is not False or cleared.get("native_rewind") is not False \
            or cleared.get("timeline_pin") is not None or texts(natural) != FULL:
        fail("unpin", f"cleared={cleared} texts={texts(natural)}", craw or nraw)
    passed("target:null unpins; full history")
    pin(opener, base, uid, "u4")
    view, _ = want(opener, base, mpath(uid), 200)
    append(path, claude_row(SID, "user", "u7", "a6", "q7"))
    advanced, araw = reason(opener, base, uid, "native_advanced", cursor_query(view))
    if advanced.get("reset") is not True or "q7" not in texts(advanced): fail("native_advanced", f"reset={advanced.get('reset')} texts={texts(advanced)}", araw)
    pin(opener, base, uid, "u4")
    view, _ = want(opener, base, mpath(uid), 200)
    append(path, claude_row(SID, "user", "cont", "a3", "continued"))
    cont, craw = reason(opener, base, uid, "native_continued", cursor_query(view))
    if texts(cont)[-1:] != ["continued"] or "q7" in texts(cont): fail("native_continued", f"texts={texts(cont)}", craw)
    passed("native_advanced then native_continued")
    rows, raw = listed(opener, base)
    prefs(rows, raw)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-rewind-http-") as tmp:
        corpus = build(Path(tmp))
        uid = corpus.uid(SID)
        with isolated_server(corpus, args.binary) as (base, opener):
            meta, raw = want(opener, base, "/api/meta", 200)
            if (meta.get("capabilities") or {}).get("timeline_pin") is not False: fail("meta", "timeline_pin should be false", raw)
            pin(opener, base, uid, "u4", 501, "metadata_disabled")
            passed("disabled 501 metadata_disabled timeline_pin=false")
        state = corpus.root / "state"
        state.mkdir(mode=0o700); state.chmod(0o700)
        with isolated_server(corpus, args.binary, state_dir=state) as (base, opener):
            uid = enabled(opener, base, corpus)
        with isolated_server(corpus, args.binary, state_dir=state) as (base, opener):
            after_restart(opener, base, corpus, uid)


if __name__ == "__main__":
    main()
