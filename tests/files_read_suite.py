#!/usr/bin/env python3
"""HTTP contract of the read-only file service (resolve-files, file, files).

Synthetic temp fixtures and an isolated loopback server only. Stdlib plus
tests/history_parity.py. Asserts live Rust handlers, not guessed status codes.
"""
from __future__ import annotations

import argparse, http.client, json, os, sys, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode, urlsplit
from urllib.request import Request

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, isolated_server)

BINARY = (p if (p := REPO / "target/release" / DEBUG_BINARY.name).is_file() else DEBUG_BINARY)
RAW, BLOB, RESOLVE = 32 * 1024 * 1024, 3 * 1024 * 1024, 4 * 1024 * 1024
LIST_KEYS = ("path", "root", "parent", "entries", "total", "offset", "writable", "errors", "incomplete")


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def fetch(opener, base, method, path, headers=None, body=None):
    req = Request(base + path, data=body, method=method)
    for key, value in (headers or {}).items():
        req.add_header(key, value)
    try:
        with opener.open(req, timeout=20) as resp:
            return resp.status, {k.lower(): v for k, v in resp.headers.items()}, resp.read(2 * 1024 * 1024)
    except HTTPError as err:
        return err.code, {k.lower(): v for k, v in err.headers.items()}, err.read(8192)
    except Exception as err:
        fail(path, str(err))


def want(opener, base, area, status, method, path, headers=None, body=None, code=None):
    got, hdrs, raw = fetch(opener, base, method, path, headers, body)
    payload = None
    if raw[:1] in (b"{", b"["):
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            payload = None
    if got != status:
        fail(area, f"HTTP {got} want {status}", raw)
    if code and not (isinstance(payload, dict) and payload.get("code") == code):
        fail(area, f"code={None if not isinstance(payload, dict) else payload.get('code')!r} want {code}", raw)
    return hdrs, raw, payload


def route(uid, ref, directory=False, **extra):
    return f"/api/session/{'files' if directory else 'file'}?" + urlencode({"uid": uid, "ref": ref, **extra})


def leak(raw, *needles):
    blob = raw if isinstance(raw, (bytes, bytearray)) else str(raw).encode()
    for needle in needles:
        if needle and needle.encode() in blob:
            return needle
    return None


def listing_ok(data, files, outside, area, raw):
    if not isinstance(data, dict) or any(k not in data for k in LIST_KEYS):
        fail(area, "listing missing documented keys", raw)
    if data.get("writable") is not False or data.get("root") != str(Path(files).anchor):
        fail(area, f"writable/root {data.get('writable')!r} {data.get('root')!r}", raw)
    root, forbidden = str(files), str(outside)
    for row in data.get("entries") or []:
        path = row.get("path") if isinstance(row, dict) else None
        if not isinstance(path, str) or not (path == root or path.startswith(root + "/")):
            fail(area, f"entry path outside file root {path!r}", raw)
        if path == forbidden or path.startswith(forbidden + "/"):
            fail(area, f"listed outside root {path!r}", raw)
    if leak(raw, forbidden):
        fail(area, "listing leaked outside path", raw)


def run(opener, base, uid, files, outside, notes, blob, pdf, nested, huge, escape, inside, secret):
    if "127.0.0.1" not in base:
        fail("base", f"non-loopback {base}")
    unmentioned, text = files / "unmentioned.txt", notes.read_bytes()
    _, raw, data = want(opener, base, "resolve", 200, "POST", "/api/session/resolve-files",
                        {"Content-Type": "application/json"},
                        json.dumps({"uid": uid, "refs": [str(notes), str(unmentioned)]}).encode())
    # resolve/list JSON include authorized wire paths (docs/files.md `root`/`path`);
    # they must never publish the outside sibling or its secret.
    if leak(raw, str(outside), str(secret)):
        fail("resolve", f"leaked path {leak(raw, str(outside), str(secret))}", raw)
    errs = [row for row in (data or {}).get("errors") or [] if row.get("ref") == str(unmentioned)]
    if not errs or errs[0].get("status") != 404 or errs[0].get("code") != "file_not_referenced":
        fail("resolve", "unmentioned ref must be 404 file_not_referenced", raw)
    _, denied, _ = want(opener, base, "unmentioned GET", 404, "GET",
                        route(uid, str(unmentioned)), code="file_not_referenced")
    _, nav, listing = want(opener, base, "nav outside", 200, "GET",
                     route(uid, str(files), True, path=str(outside)))
    if listing.get("path") != str(outside):
        fail("navigation", "grant did not permit outside-directory navigation", nav)
    if leak(denied, str(files), str(outside)):
        fail("unauthorized", "error body leaked an absolute path", denied)
    passed("unmentioned direct ref stays 404; directory grant permits filesystem navigation")

    hdrs, raw, _ = want(opener, base, "text", 200, "GET", route(uid, str(notes)))
    ctype, length = hdrs.get("content-type", ""), hdrs.get("content-length")
    if not ctype.startswith("text/plain") or length != str(len(text)) or raw != text:
        fail("text", f"type={ctype!r} length={length!r}", raw)
    if str(files).encode() in raw:
        fail("text", "file body leaked absolute file root", raw)
    passed("authorized text GET 200 content-type Content-Length")

    size = blob.stat().st_size
    hdrs, raw, _ = want(opener, base, "range 206", 206, "GET",
                        route(uid, str(blob), download="1"), {"Range": "bytes=0-99"})
    if hdrs.get("content-range") != f"bytes 0-99/{size}" or hdrs.get("content-length") != "100" or len(raw) != 100:
        fail("range 206", f"range={hdrs.get('content-range')!r} len={hdrs.get('content-length')!r}", raw)
    hdrs, _, _ = want(opener, base, "range 416", 416, "GET",
                      route(uid, str(blob), download="1"), {"Range": "bytes=999999999-"})
    if hdrs.get("content-range") != f"bytes */{size}":
        fail("range 416", f"Content-Range {hdrs.get('content-range')!r}", raw)
    # Multipart is 416: response.rs rejects commas in the range body (docs agree).
    want(opener, base, "multi-range", 416, "GET",
         route(uid, str(blob), download="1"), {"Range": "bytes=0-1,3-4"})
    passed("Range bytes=0-99 → 206; bytes=999999999- → 416; multi-range → 416")

    hdrs, raw, _ = want(opener, base, "pdf", 200, "GET", route(uid, str(pdf)))
    if hdrs.get("content-type") != "application/pdf":
        fail("pdf", f"content-type {hdrs.get('content-type')!r}", raw)
    if hdrs.get("content-security-policy") != "frame-ancestors 'self'":
        fail("pdf", f"CSP {hdrs.get('content-security-policy')!r}", raw)
    if hdrs.get("x-content-type-options") != "nosniff":
        fail("pdf", f"nosniff {hdrs.get('x-content-type-options')!r}", raw)
    passed("PDF CSP frame-ancestors 'self' and nosniff")

    for path, label in ((escape, "symlink outside"), (inside, "symlink inside")):
        want(opener, base, label, 200, "GET", route(uid, str(path)))
    passed("symlink outside and inside both resolve and read normally")

    _, raw, data = want(opener, base, "list root", 200, "GET", route(uid, str(files), True))
    listing_ok(data, files, outside, "list root", raw)
    if data.get("parent") != str(files.parent):
        fail("list root", "parent navigation must continue above configured roots", raw)
    names = {row.get("name"): row for row in data["entries"]}
    for name in ("escape.link", "inside.link"):
        row = names.get(name) or {}
        if row.get("kind") != "file" or row.get("symlink") is not True:
            fail("list root", f"{name} must be a navigable file symlink", raw)
    _, raw, nested_list = want(opener, base, "list nested", 200, "GET", route(uid, str(nested), True))
    listing_ok(nested_list, files, outside, "list nested", raw)
    if nested_list.get("parent") != str(files):
        fail("list nested", f"parent {nested_list.get('parent')!r}", raw)
    if not any(row.get("name") == "child.txt" for row in nested_list["entries"]):
        fail("list nested", "missing child.txt", raw)
    passed("directory listing shape; parents continue to OS root; symlinks navigable")

    want(opener, base, "raw 32 MiB", 413, "GET", route(uid, str(huge)), code="file_raw_budget")
    passed("size over 32 MiB raw limit → 413 file_raw_budget")

    endpoint = urlsplit(base)
    conn = http.client.HTTPConnection(endpoint.hostname, endpoint.port, timeout=10)
    try:
        conn.putrequest("POST", "/api/session/resolve-files")
        conn.putheader("Content-Type", "application/json")
        conn.putheader("Content-Length", str(RESOLVE + 1))
        conn.endheaders()
        response = conn.getresponse()
        assert response.status == 413, response.status
        response.read()
    finally:
        conn.close()
    passed("resolve-files body over 4 MiB → 413")

    want(opener, base, "unknown uid", 404, "GET", route("codex:missing", str(notes)), code="session_error")
    passed("unknown uid → 404 session_error")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-files-read-") as tmp:
        root = Path(tmp)
        corpus = Corpus(root)
        for name in ("claude", "codex", "grok"):
            (root / name).mkdir()
        files, outside = root / "files", root / "outside"
        files.mkdir()
        outside.mkdir()
        notes = files / "notes.md"
        notes.write_text("scoped file content\n")
        blob, pdf, nested = files / "blob.bin", files / "doc.pdf", files / "nested"
        blob.write_bytes(bytes(BLOB))
        pdf.write_bytes(b"%PDF-1.4\nsynthetic")
        nested.mkdir()
        (nested / "child.txt").write_text("nested directory content\n")
        (files / "unmentioned.txt").write_text("not a session reference")
        (files / "mentioned.txt").write_text("authorized by the selected view")
        huge = files / "huge.bin"
        huge.write_bytes(b"")
        os.truncate(huge, RAW + 1)
        secret = outside / "secret.txt"
        secret.write_text("outside-secret-bytes")
        escape, inside = files / "escape.link", files / "inside.link"
        os.symlink(secret, escape)
        os.symlink("notes.md", inside)
        sid = "claude-files-read"
        refs = [files, notes, blob, pdf, nested, files / "mentioned.txt", huge, escape, inside]
        row = claude_row(sid, "user", "u0", None, "Open " + " ".join(f"`{path}`" for path in refs), cwd=str(files))
        corpus.put(sid, "claude", [row], [])
        with isolated_server(corpus, args.binary, file_roots=(files,)) as (base, opener):
            run(opener, base, corpus.uid(sid), files, outside, notes, blob, pdf, nested, huge, escape, inside, secret)


if __name__ == "__main__":
    main()
