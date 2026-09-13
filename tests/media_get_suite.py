#!/usr/bin/env python3
"""HTTP contract of GET /api/media/{token} and lazy descriptor registration.

Synthetic Codex fixtures and an isolated loopback Rust server only. Stdlib plus
tests/history_parity.py, tests/media_browser.py and tests/native_spans.py.
"""
from __future__ import annotations

import argparse, base64, hashlib, http.client, json, os, struct, sys, tempfile, zlib
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, codex_row, isolated_server  # noqa: E402
from media_browser import GREEN, JPEG, PNG, TOKEN, image  # noqa: E402
from native_spans import images  # noqa: E402

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
GIF = "R0lGODdhAwACAIEAAOYeCgAAAAAAAAAAACwAAAAAAwACAAAIBgABCBwYEAA7"  # media_lazy.rs
WEBP = "UklGRh4AAABXRUJQVlA4TBEAAAAvAkAAAAdQj5pXof+BiOh/AAA="  # media_formats.rs
# Mime::extension maps jpeg → jpg (api/media.rs Content-Disposition).
EMBEDDED = ((PNG, "image/png", "png"), (JPEG, "image/jpeg", "jpg"),
            (GREEN, "image/png", "png"), (GIF, "image/gif", "gif"),
            (WEBP, "image/webp", "webp"))
VIEW_LIMIT, SID, SIZE = 16, "codex-media-get", 128  # VIEW_LIMIT: sessions/mod.rs

def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def b64(data):
    return base64.b64decode(data, validate=True)


def png_fill(fill, size=SIZE):
    small, kind = b64(PNG), b"npAD"
    payload = bytes([fill]) * (size - len(small) - 12)
    chunk = struct.pack(">I", len(payload)) + kind + payload + struct.pack(
        ">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
    out = small[:-12] + chunk + small[-12:]
    return out if len(out) == size else fail("png_fill", f"{len(out)} != {size}")


def meta(sid, cwd="/synthetic/media"):
    return codex_row("session_meta", {"id": sid, "session_id": sid, "cwd": cwd})


def msg(content, ordinal=1):
    return codex_row("response_item", {"type": "message", "role": "user", "content": content}, ordinal)


def build(root):
    corpus, files = Corpus(root), root / "files"
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    files.mkdir()
    (files / "disk.png").write_bytes(png_fill(1))
    content = [image("codex", data, mime) for data, mime, _ in EMBEDDED]
    content.append({"type": "input_text", "text": "five embedded"})
    corpus.put(SID, "codex", [meta(SID, str(files)), msg(content),
        msg([image("codex", "AAAA"), {"type": "input_text", "text": "invalid remains"}], 2),
        msg([{"type": "input_image", "image_url": str(files / "disk.png")},
             {"type": "input_text", "text": "file backed"}], 3)], [])
    for i in range(VIEW_LIMIT):
        name = f"codex-fill-{i:02d}"
        corpus.put(name, "codex", [meta(name), msg([{"type": "input_text", "text": f"fill {i}"}])], [])
    return corpus, files


def raw(base, method, path, headers=None):
    parsed = urlsplit(base)
    if parsed.hostname != "127.0.0.1" or not parsed.port:
        fail("base", f"non-loopback {base}")
    conn = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=8)
    try:
        conn.request(method, path, headers=headers or {})
        resp = conn.getresponse()
        return resp.status, {k.lower(): v for k, v in resp.getheaders()}, resp.read(2 * 1024 * 1024)
    except (TimeoutError, OSError, http.client.HTTPException) as err:
        fail(path, str(err))
    finally:
        conn.close()


def history(opener, base, uid):
    try:
        with opener.open(base + "/api/messages/" + uid, timeout=15) as resp:
            rawb, code = resp.read(2 * 1024 * 1024 + 1), resp.status
    except HTTPError as err:
        fail("messages", f"HTTP {err.code}", err.read(4096))
    if code != 200:
        fail("messages", f"HTTP {code}", rawb)
    try:
        return json.loads(rawb), rawb
    except json.JSONDecodeError:
        fail("messages", "not JSON", rawb)


def media_get(opener, base, src, want=200):
    try:
        with opener.open(base + src, timeout=15) as resp:
            return resp.status, {k.lower(): v for k, v in resp.headers.items()}, resp.read(2 * 1024 * 1024 + 1)
    except HTTPError as err:
        body = err.read(4096)
        if err.code != want:
            fail(src, f"HTTP {err.code} (want {want})", body)
        return err.code, {k.lower(): v for k, v in err.headers.items()}, body


def token_of(item, area, rawb):
    src = item.get("src") or ""
    if not TOKEN.fullmatch(src) or item.get("lazy") is not True or not item.get("alt"):
        fail(area, f"descriptor {item}", rawb)
    if {"mime", "width", "height"} & set(item):
        fail(area, "lazy descriptor must not invent inspected fields", rawb)
    return src


def run(opener, base, corpus, files):
    uid, disk = corpus.uid(SID), files / "disk.png"
    data, rawb = history(opener, base, uid)
    items = images(data)
    dumped = json.dumps(items)
    for secret in (PNG, JPEG, GREEN, GIF, WEBP, "AAAA", "data:image", str(disk)):
        if secret and secret in dumped:
            fail("history-leak", f"descriptor JSON contains {secret[:40]!r}", dumped.encode())
    if len(items) != 7:
        fail("descriptors", f"want 7 media items, got {len(items)}", rawb)
    passed("descriptors omit base64 and absolute paths")
    code, _, body = media_get(opener, base, "/api/media/" + "ab" * 16, want=404)
    if code != 404:
        fail("404", f"random token HTTP {code}", body)
    passed("token format 32-hex; random token 404")
    for item, (data_b64, mime, ext) in zip(items[:5], EMBEDDED):
        src, expected = token_of(item, "embedded", rawb), b64(data_b64)
        code, headers, body = media_get(opener, base, src)
        if (code != 200 or headers.get("content-type") != mime
                or headers.get("content-length") != str(len(expected)) or body != expected
                or hashlib.sha256(body).digest() != hashlib.sha256(expected).digest()):
            fail(src, f"HTTP {code} type={headers.get('content-type')} len={len(body)}", body[:80])
        if headers.get("cache-control") != "private, no-store":
            fail(src, f"Cache-Control={headers.get('cache-control')!r}", body[:80])
        if headers.get("x-content-type-options") != "nosniff":
            fail(src, "missing nosniff", body[:80])
        if headers.get("content-disposition") != f'inline; filename="image.{ext}"':
            fail(src, f"disposition={headers.get('content-disposition')!r}", body[:80])
        code2, _, warm = media_get(opener, base, src)
        if code2 != 200 or warm != expected:
            fail(src, "warm GET bytes differ", warm[:80])
    passed("embedded GET type/length/bytes/sha256")
    passed("Cache-Control private, no-store")
    passed("warm GET identical bytes")
    bad = token_of(items[5], "invalid", rawb)
    code, _, body = media_get(opener, base, bad, want=422)
    err = json.loads(body) if body else {}
    if code != 422 or not err.get("error") or b"AAAA" in body:
        fail("invalid", f"HTTP {code}", body)
    if not any("invalid remains" in (row.get("text") or "") for row in data["messages"]):
        fail("invalid", "readable text dropped", rawb)
    passed("invalid base64 still registered; GET 422")
    file_src, expected = token_of(items[6], "file", rawb), png_fill(1)
    code, headers, body = media_get(opener, base, file_src)
    if code != 200 or body != expected or headers.get("content-type") != "image/png":
        fail(file_src, f"HTTP {code} type={headers.get('content-type')}", body[:80])
    passed("file-backed GET")
    status, headers, body = raw(base, "HEAD", file_src)
    if status != 200 or body or headers.get("cache-control") != "private, no-store":
        fail("HEAD", f"HTTP {status} len={len(body)} cache={headers.get('cache-control')}", body)
    if headers.get("content-type") != "image/png" or headers.get("content-length") != str(len(expected)):
        fail("HEAD", f"type={headers.get('content-type')} length={headers.get('content-length')}", body)
    status, _, ranged = raw(base, "GET", file_src, {"Range": "bytes=0-1"})  # handler ignores Range
    if status != 200 or ranged != expected:
        fail("Range", f"HTTP {status} len={len(ranged)} (code returns full body)", ranged[:80])
    passed("HEAD metadata empty body; Range ignored as 200 full")
    for i in range(VIEW_LIMIT):
        history(opener, base, corpus.uid(f"codex-fill-{i:02d}"))
    code, _, body = media_get(opener, base, items[0]["src"], want=200)
    if code not in (200, 404):
        fail("evict", f"HTTP {code} after view eviction", body)
    if code == 200 and body != b64(PNG):
        fail("evict", "embedded bytes changed after view eviction", body[:80])
    passed("token survives view eviction/reload or 404 as documented")
    st, replacement = disk.stat(), png_fill(2)
    if len(replacement) != st.st_size:
        fail("replace", f"replacement size {len(replacement)} != {st.st_size}")
    disk.write_bytes(replacement)
    os.utime(disk, ns=(st.st_atime_ns, st.st_mtime_ns))
    code, _, body = media_get(opener, base, file_src, want=409)
    err = json.loads(body) if body else {}
    if code != 409 or not err.get("error") or str(files).encode() in body:
        fail("replace", f"HTTP {code} (want 409)", body)
    latest, lraw = history(opener, base, uid)
    fresh = token_of(images(latest)[6], "fresh-file", lraw)
    if fresh == file_src:
        fail("replace", "fresh history reused stale file token", lraw)
    code, _, body = media_get(opener, base, fresh)
    if code != 200 or body != replacement:
        fail(fresh, f"HTTP {code} after reload", body[:80])
    passed("file replace same size/mtime → 409; fresh history new token")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-media-get-") as tmp:
        corpus, files = build(Path(tmp))
        with isolated_server(corpus, args.binary, file_roots=(files,)) as (base, opener):
            run(opener, base, corpus, files)


if __name__ == "__main__":
    main()
