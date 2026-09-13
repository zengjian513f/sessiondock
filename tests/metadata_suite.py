#!/usr/bin/env python3
"""HTTP contract of persisted preferences (star, fork-visibility) vs /api/sessions,
plus the server-recorded `spawned_by` row key (batch 36): seeded on disk, carried
by the row, untouched by star/unstar, durable across restart.

Loopback synthetic fixtures, no Chromium. Codes from api/metadata.rs; disk keys
from Document (schema_version 1). Request types omit deny_unknown_fields.
"""
import argparse, http.client, json, sys, tempfile, threading, time
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import Request

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, REPO, build_corpus, claude_row, isolated_server)

BINARY = (p if (p := REPO / "target/release" / DEBUG_BINARY.name).is_file() else DEBUG_BINARY)
KEYS = {"schema_version", "revision", "sessions"}
ROW_KEYS = {"starred", "starred_at", "fork_parent_visible", "spawned_by"}
SPAWNED_BY = {"source": "codex", "sid": "parent-native-sid"}
FILE, STAR, VIS = "session-metadata.json", "/api/session/star", "/api/sessions/fork-visibility"

def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")

def passed(area):
    print(f"PASS {area}", flush=True)

def call(opener, base, path, body=None, data=None):
    payload = data if data is not None else (None if body is None else json.dumps(body).encode())
    req = Request(base + path, data=payload, method="GET" if payload is None else "POST")
    if payload is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with opener.open(req, timeout=10) as resp:
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

def listed(opener, base):
    payload, raw = want(opener, base, "/api/sessions?force=1", 200)
    rows = payload.get("sessions")
    if not isinstance(rows, list):
        fail("/api/sessions", "sessions not a list", raw)
    return {row.get("sid"): row for row in rows if isinstance(row, dict)}, raw

def disk(state):
    raw = (state / FILE).read_bytes()
    try:
        data = json.loads(raw)
    except Exception as err:
        fail("disk", str(err), raw)
    rows = data.get("sessions")
    if set(data) != KEYS or data.get("schema_version") != 1 or not isinstance(rows, dict):
        fail("disk", f"keys={sorted(data)} version={data.get('schema_version')}", raw)
    for uid, row in rows.items():
        extra = set(row) - ROW_KEYS if isinstance(row, dict) else {"<not-object>"}
        if extra:
            fail("disk", f"{uid} extra {sorted(extra)}", raw)
    return data

def conc(host, port, uids):
    bad = []
    def work(uid):
        blob, hdr = json.dumps({"uid": uid, "starred": True}).encode(), {"Content-Type": "application/json"}
        for _ in range(40):
            conn = http.client.HTTPConnection(host, port, timeout=8)
            try:
                conn.request("POST", STAR, blob, hdr)
                resp = conn.getresponse(); raw, st = resp.read(4096), resp.status
            except Exception as err:
                bad.append(f"{uid}:{err}"); return
            finally:
                conn.close()
            if st == 200: return
            if st != 503:  # reader workers=4; try_acquire_owned → reader_busy
                bad.append(f"{uid}:HTTP {st} {raw[:80]!r}"); return
            time.sleep(0.05)
        bad.append(f"{uid}:timeout")
    ts = [threading.Thread(target=work, args=(u,)) for u in uids]
    for t in ts: t.start()
    for t in ts: t.join(15)
    if bad or any(t.is_alive() for t in ts):
        fail("concurrent", "; ".join(bad) or "thread hung")

def run(opener, base, state, extra):
    rows, raw = listed(opener, base)
    branch, parent = rows.get("claude-branch"), rows.get("codex-parent")
    if not branch or not parent:
        fail("list", "need claude-branch and codex-parent", raw)
    uid, puid = branch["uid"], parent["uid"]
    if branch.get("spawned_by") != SPAWNED_BY or "spawned_by" in parent:
        fail("spawned_by", f"branch={branch.get('spawned_by')} parent={parent.get('spawned_by')}", raw)
    passed("seeded spawned_by {source, sid} decorates its row only")
    saved, sraw = want(opener, base, STAR, 200, {"uid": uid, "starred": True})
    if saved.get("ok") is not True or saved.get("starred") is not True or saved.get("uid") != uid \
            or not isinstance(saved.get("starred_at"), (int, float)):
        fail("star", "response shape", sraw)
    rows, raw = listed(opener, base)
    if rows["claude-branch"].get("starred") is not True or rows["claude-branch"].get("spawned_by") != SPAWNED_BY:
        fail("star", "list missing starred or spawned_by", raw)
    want(opener, base, STAR, 200, {"uid": uid, "starred": False})
    rows, raw = listed(opener, base)
    if rows["claude-branch"].get("starred") is True or rows["claude-branch"].get("spawned_by") != SPAWNED_BY:
        fail("unstar", "still starred or spawned_by lost", raw)
    passed("star list unstar (spawned_by untouched)")
    want(opener, base, STAR, 400, {"uid": "", "starred": True}, code="invalid_metadata_uid")
    want(opener, base, STAR, 404, {"uid": "codex:missing", "starred": True}, code="session_missing")
    want(opener, base, STAR, 404, {"uid": "x" * 257, "starred": True}, code="session_missing")
    passed("invalid uid 400/404")
    endpoint = urlsplit(base)
    conn = http.client.HTTPConnection(endpoint.hostname, endpoint.port, timeout=10)
    try:
        conn.putrequest("POST", STAR)
        conn.putheader("Content-Type", "application/json")
        conn.putheader("Content-Length", str(4 * 1024 * 1024 + 1))
        conn.endheaders()
        response = conn.getresponse()
        assert response.status == 413, response.status
        response.read()
    finally:
        conn.close()
    passed("Python metadata body over 4 MiB 413")
    extra_code, _, extra_raw = call(opener, base, STAR, {"uid": uid, "starred": True, "unknown_field": 1})
    if extra_code != 200:
        fail("unknown", f"HTTP {extra_code} (request types omit deny_unknown_fields)", extra_raw)
    want(opener, base, STAR, 200, {"uid": uid, "starred": False})
    passed("unknown fields accepted (no deny_unknown_fields)")
    rows, raw = listed(opener, base)
    if rows["codex-parent"].get("fork_parent") is not True or rows["codex-parent"].get("fork_parent_visible"):
        fail("visibility", "default parent hidden", raw)
    vis, vraw = want(opener, base, VIS, 200, {"uids": [puid], "visible": True})
    if vis.get("ok") is not True or vis.get("updated") != [{"uid": puid, "fork_parent_visible": True}]:
        fail("visibility", "set shape", vraw)
    rows, raw = listed(opener, base)
    if rows["codex-parent"].get("fork_parent_visible") is not True:
        fail("visibility", "list after set", raw)
    want(opener, base, VIS, 200, {"uids": [puid], "visible": False})
    rows, raw = listed(opener, base)
    if rows["codex-parent"].get("fork_parent_visible") is True:
        fail("visibility", "list after clear", raw)
    want(opener, base, VIS, 200, {"uids": [puid], "visible": True})
    passed("fork visibility set/clear")
    parsed = urlsplit(base)
    conc(parsed.hostname, parsed.port, [rows[sid]["uid"] for sid in extra])
    rows, raw = listed(opener, base)
    missing = [sid for sid in extra if rows.get(sid, {}).get("starred") is not True]
    if missing:
        fail("concurrent", f"lost update {missing}", raw)
    passed("concurrent 8 stars persist")
    data = disk(state)
    if data["sessions"].get(uid, {}).get("spawned_by") != SPAWNED_BY:
        fail("disk", f"spawned_by row {data['sessions'].get(uid)}")
    passed("on-disk schema_version=1 keys, spawned_by row kept")

def cap(opener, base, enabled):
    meta, raw = want(opener, base, "/api/meta", 200)
    if (meta.get("capabilities") or {}).get("metadata") is not enabled:
        fail("meta", f"metadata should be {enabled}", raw)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-metadata-suite-") as tmp:
        root, extra = Path(tmp), [f"conc-{i}" for i in range(8)]
        corpus = build_corpus(root)
        for sid in extra:
            corpus.put(sid, "claude", [claude_row(sid, "user", "u0", None, sid)], [])
        star, vis = {"uid": "x", "starred": True}, {"uids": ["x"], "visible": True}
        with isolated_server(corpus, args.binary) as (base, opener):
            cap(opener, base, False)
            want(opener, base, STAR, 501, star, code="metadata_disabled")
            want(opener, base, VIS, 501, vis, code="metadata_disabled")
            passed("disabled 501 metadata:false")
        state = root / "state"
        state.mkdir(mode=0o700); state.chmod(0o700)
        # The server records spawned_by itself (no route); seed it the way a
        # recorded document looks, like meta_import.py does.
        seeded = state / FILE
        seeded.write_text(json.dumps({"schema_version": 1, "revision": 1, "sessions": {
            corpus.uid("claude-branch"): {"spawned_by": SPAWNED_BY}}}) + "\n")
        seeded.chmod(0o600)
        with isolated_server(corpus, args.binary, state_dir=state) as (base, opener):
            cap(opener, base, True)
            run(opener, base, state, extra)
        with isolated_server(corpus, args.binary, state_dir=state) as (base, opener):
            rows, raw = listed(opener, base)
            lost = [sid for sid in extra if rows.get(sid, {}).get("starred") is not True]
            vis = rows.get("codex-parent", {}).get("fork_parent_visible")
            if lost or vis is not True or rows.get("claude-branch", {}).get("spawned_by") != SPAWNED_BY:
                fail("restart", f"lost={lost} visible={vis} spawned_by={rows.get('claude-branch', {}).get('spawned_by')}", raw)
            disk(state)
            passed("durable across restart (stars, visibility, spawned_by)")


if __name__ == "__main__":
    main()
