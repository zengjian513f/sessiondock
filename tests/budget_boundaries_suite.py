#!/usr/bin/env python3
"""Exact native-input budget boundaries over HTTP.

Constants from the `budgets` block at the top of sessions/mod.rs, which mirrors
the table in docs/read-model.md (物理工作预算): 64 MiB per record, 4 GiB per
file, 2,000,000 LF checkpoints, 1,000,000 records per view. There is no session
count, total-byte or directory-entry cap any more, and the list never parses a
file: every over-budget input is listed (the head/tail summary cannot see the
budget) and fails only when that session is opened (413/501 for it alone).
"""
from __future__ import annotations
import argparse, json, os, sys, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, isolated_server  # noqa: E402

BINARY = (p if (p := REPO / "target/release" / DEBUG_BINARY.name).is_file() else DEBUG_BINARY)
MIB = 1024 * 1024
FILE_LIMIT = 4 * 1024 * MIB  # budgets::FILE_BYTES
SESSIONS, ENTRY_LIMIT = 1_000, 20_000  # no session cap; the old directory-entry bound is gone too
LINE_LIMIT, ROW_LIMIT, MAX_LF = 64 * MIB, 1_000_000, 2_000_000  # RECORD_BYTES / VIEW_RECORDS / FILE_CHECKPOINTS
CAP, TAIL = 4 * MIB, b'"}}\n'
BIG_CAP = LINE_LIMIT + 4 * MIB


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def hist(root, sid):
    path = root / "claude/project-history" / f"{sid}.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    return path


def progress(sid, n=1):
    return (b'{"type":"progress","uuid":"u","parentUuid":null,"sessionId":"'
            + sid.encode() + b'"}\n') * n


def user_line(sid, extra=0):
    head = (b'{"type":"user","uuid":"u0","parentUuid":null,"sessionId":"'
            + sid.encode() + b'","message":{"role":"user","content":"')
    body = head + b"x" * (LINE_LIMIT + extra - len(head) - len(TAIL)) + TAIL
    if len(body) != LINE_LIMIT + extra:
        fail("fixture", f"{sid} len {len(body)} != {LINE_LIMIT + extra}")
    json.loads(body)
    return body


def first_json(path, records=None):
    with path.open("rb") as fh:
        first, chunk = fh.readline(), b" "
        json.loads(first)
        n = first.count(b"\n")
        while records is not None and n < records and chunk:
            chunk = fh.read(1 << 20); n += chunk.count(b"\n")
    if records is not None and n < records:
        fail("jsonl", f"{path.name} LF {n} < {records}", first[:80])
    return first


def call(opener, base, route, timeout=30, cap=CAP):
    try:
        with opener.open(base + route, timeout=timeout) as resp:
            raw, code = resp.read(cap + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(8192), err.code
    except Exception as err:
        fail(route, str(err))
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        payload = {}
    return payload, raw, code


def listed(opener, base, timeout=300):
    payload, raw, code = call(opener, base, "/api/sessions?force=1", timeout)
    if code != 200 or not isinstance(payload.get("sessions"), list):
        fail("list", f"HTTP {code}", raw)
    return {row.get("sid"): row for row in payload["sessions"] if isinstance(row, dict)}, raw


def serve(root, binary):
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True, exist_ok=True)
    return isolated_server(Corpus(root), binary)




def detail(opener, base, uid, code, needle="", window=True, cap=CAP):
    query = "?window=1" if window else ""
    payload, raw, got = call(opener, base, "/api/messages/" + quote(uid, safe=":") + query, 300, cap)
    blob = str(payload.get("error", "")) + str(payload.get("code", ""))
    if got != code:
        fail("detail", f"{uid} HTTP {got} want {code}", raw)
    if code == 501 and payload.get("code") != "unsupported_history":
        fail("detail", f"code={payload.get('code')}", raw)
    if needle and needle not in blob and needle not in str((payload.get("meta") or {}).get("migration_warnings", "")):
        fail("detail", f"missing {needle!r}", raw)
    return payload


def row_ok(rows, sid, supported, needle, raw):
    row = rows.get(sid)
    if not isinstance(row, dict) or bool(row.get("supported")) is not supported:
        fail(sid, f"supported={None if not row else row.get('supported')} want {supported}", raw)
    # Batch 44 WP-C: only an unsupported row carries migration_warnings (its reason).
    if supported and "migration_warnings" in row:
        fail(sid, "supported row must not carry migration_warnings", raw)
    warn = " ".join(str(x) for x in (row.get("migration_warnings") or []))
    if needle and needle not in warn:
        fail(sid, f"warning {warn!r} missing {needle!r}", raw)
    return row


def flood(folder, n):
    folder.mkdir(parents=True, exist_ok=True)
    for i in range(n):
        (folder / f"{i:05d}").write_bytes(b"")


def build_ok(root):
    hist(root, "line-ok").write_bytes(user_line("line-ok"))
    hist(root, "line-over").write_bytes(user_line("line-over", 1))
    rec_ok, rec_over = hist(root, "rec-ok"), hist(root, "rec-over")
    rec_ok.write_bytes(progress("rec-ok", ROW_LIMIT)); first_json(rec_ok, ROW_LIMIT)
    rec_over.write_bytes(progress("rec-over", ROW_LIMIT + 1)); first_json(rec_over, ROW_LIMIT + 1)
    hist(root, "lf-ok").write_bytes(progress("lf-ok") + b"\n" * (MAX_LF - 1))
    for i in range(5):
        sid, path = f"fat-{i}", hist(root, f"fat-{i}")
        path.write_bytes(progress(sid) + b"x" * (16 * MIB - len(progress(sid))))
        first_json(path)
        if path.stat().st_size != 16 * MIB:
            fail("fat", f"{sid} size {path.stat().st_size}")
    have = {p.stem for p in (root / "claude/project-history").glob("*.jsonl")}
    for i in range(SESSIONS - len(have)):
        hist(root, f"s{i:04d}").write_bytes(progress(f"s{i:04d}"))
    flood(root / "claude/pad", ENTRY_LIMIT - 2 - SESSIONS)


def run_ok(opener, base):
    rows, raw = listed(opener, base)
    if len(rows) != SESSIONS:
        fail("sessions", f"listed {len(rows)} want {SESSIONS}", raw)
    passed(f"{SESSIONS} sessions listed (no session cap)")
    ok = row_ok(rows, "line-ok", True, "", raw)
    # A 64 MiB message exceeds the 8 MiB history-page budget (413 for the
    # windowed request, docs/history-pages.md); the plain view opens it.
    detail(opener, base, ok["uid"], 413, "预算")
    payload = detail(opener, base, ok["uid"], 200, window=False, cap=BIG_CAP)
    text = "".join(m.get("text") or "" for m in payload.get("messages") or [] if isinstance(m, dict))
    if len(text) < LINE_LIMIT // 2:
        fail("line-ok", f"projected {len(text)} bytes", (payload.get("messages") or [""])[:1])
    passed("line == 64 MiB accepted (giant ordinary text materialized)")
    # The list summarizes 96 KiB + 512 KiB of a file: a 64 MiB + 1 line is
    # invisible to it and the row stays listed; opening the session is the
    # 64 MiB hard budget (an oversized line is never just "skipped").
    over = row_ok(rows, "line-over", True, "", raw)
    detail(opener, base, over["uid"], 501, "64 MiB")
    passed("line +1 byte: listed, open 501")
    rec = row_ok(rows, "rec-ok", True, "", raw)
    detail(opener, base, rec["uid"], 200)
    passed(f"{ROW_LIMIT} records accepted")
    bad = row_ok(rows, "rec-over", True, "", raw)
    detail(opener, base, bad["uid"], 501, "1000000")
    passed(f"{ROW_LIMIT + 1} records: listed, open 501")
    lf = row_ok(rows, "lf-ok", True, "", raw)
    detail(opener, base, lf["uid"], 200)
    passed(f"{MAX_LF} LF checkpoints accepted")
    sizes = [rows[f"fat-{i}"]["size"] for i in range(5)]
    total = sum(int(r.get("size") or 0) for r in rows.values())
    if any(size != 16 * MIB for size in sizes) or total <= 64 * MIB:
        fail("total", f"sizes={sizes} total={total}", raw)
    detail(opener, base, rows["fat-0"]["uid"], 200)
    passed("5×16 MiB files listed and opened (no total-byte cap)")
    passed(f"{ENTRY_LIMIT} directory entries walked (no entry cap)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-budget-") as tmp:
        tmp = Path(tmp)
        ok = tmp / "ok"
        build_ok(ok)
        with serve(ok, args.binary) as (base, opener):
            run_ok(opener, base)
        eq, path = tmp / "file-eq", hist(tmp / "file-eq", "file-eq")
        path.write_bytes(progress("file-eq")); os.truncate(path, FILE_LIMIT); first_json(path)
        with serve(eq, args.binary) as (base, opener):
            rows, raw = listed(opener, base, timeout=600)
            if "file-eq" not in rows or rows["file-eq"].get("size") != FILE_LIMIT:
                fail("file-eq", f"want listed size {FILE_LIMIT}", raw)
        passed("file == 4 GiB FILE_BYTES listed")
        # Over-budget files are listed (the walk only stats them) and 413 only
        # when opened; a flooded directory is walked without any entry cap.
        cases = (
            ("file-over", "over", lambda r: (p := hist(r, "over"), p.write_bytes(b"\n"), os.truncate(p, FILE_LIMIT + 1)), "4 GiB"),
            ("lf-over", "lf-over", lambda r: hist(r, "lf-over").write_bytes(progress("lf-over") + b"\n" * MAX_LF), "读取或索引预算"),
        )
        for name, sid, builder, needle in cases:
            root = tmp / name
            builder(root)
            with serve(root, args.binary) as (base, opener):
                rows, raw = listed(opener, base, timeout=120)
                if sid not in rows:
                    fail(name, "over-budget file must still be listed", raw)
                detail(opener, base, rows[sid]["uid"], 413, needle)
            passed(f"{name}: listed, open 413 {needle!r}")
        root = tmp / "entry-over"
        flood(root / "claude/pad", ENTRY_LIMIT)
        hist(root, "beside-flood").write_bytes(progress("beside-flood"))
        with serve(root, args.binary) as (base, opener):
            rows, raw = listed(opener, base, timeout=120)
            if "beside-flood" not in rows:
                fail("entry-over", f"{ENTRY_LIMIT} extra directory entries must not hide the session", raw)
        passed(f"entry-over: {ENTRY_LIMIT} entries walked, no cap")


if __name__ == "__main__":
    main()
