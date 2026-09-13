#!/usr/bin/env python3
"""Contract coverage for static serving and HTML template injection.

Isolated loopback server, synthetic corpus. urllib only; no Chromium.
"""
from __future__ import annotations

import argparse
import hashlib
import html
import json
import re
import sys
import tempfile
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import BINARY as DEBUG_BINARY, REPO, build_corpus, get_json, isolated_server

BINARY = next(p for p in (REPO / "target/release/sessiondock", DEBUG_BINARY) if p.is_file())
BODY_CAP = 2 * 1024 * 1024
PLACEHOLDER = re.compile(r"__SESSIONDOCK_[A-Z0-9_]+__")
TYPES = (("/app.js", "javascript"), ("/style.css", "text/css"),
         ("/manifest.webmanifest", "manifest"), ("/fonts/CascadiaMono.woff2", "woff2"),
         ("/icons/icon.svg", "svg"))


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def fetch(opener, base, path, headers=None):
    request = Request(base + path, method="GET")
    for key, value in (headers or {}).items():
        request.add_header(key, value)
    try:
        with opener.open(request, timeout=10) as resp:
            return resp.status, {k.lower(): v for k, v in resp.headers.items()}, resp.read(BODY_CAP + 1)
    except HTTPError as err:
        return err.code, {k.lower(): v for k, v in err.headers.items()}, err.read(4096)
    except (URLError, TimeoutError, OSError) as err:
        fail(path, str(err))


def meta_content(page, name):
    match = re.search(rf'<meta\s+name="{re.escape(name)}"\s+content="([^"]*)"', page, re.I)
    if not match:
        fail("html", f"missing meta[name={name!r}]", page.encode())
    return html.unescape(match.group(1))


def run(base, opener):
    status, headers, body = fetch(opener, base, "/")
    if status != 200:
        fail("/", f"HTTP {status}", body)
    page = body.decode("utf-8", "replace")
    leftover = PLACEHOLDER.findall(page)
    mode = meta_content(page, "sessiondock-mode")
    if leftover or mode != "local":
        fail("html", f"unreplaced={leftover} mode={mode!r}", body)
    passed("html placeholders/mode")

    raw_caps = meta_content(page, "sessiondock-capabilities")
    try:
        caps = json.loads(raw_caps)
    except json.JSONDecodeError as err:
        fail("capabilities", str(err), raw_caps.encode())
    if not isinstance(caps, dict) or caps.get("backend") != "rust":
        fail("capabilities", f"backend={caps!r}", body)
    passed("capabilities meta")

    cache = (headers.get("cache-control") or "").lower()
    if "no-store" not in cache and "no-cache" not in cache and "max-age=0" not in cache:
        fail("html cache", f"Cache-Control={cache!r} allows caching", body)
    passed("html Cache-Control")

    derived = re.search(r"""(?:src|href)=["']([^"']+\?v=([^"'&]+))["']""", page)
    if not derived:
        fail("asset version", "no versioned asset URL in HTML", body)
    version = derived.group(2)
    if meta_content(page, "sessiondock-build") != version:
        fail("asset version", f"build meta != URL v={version!r}", body)
    mismatched = [item for item in re.findall(r"[?&]v=([^\"'&]+)", page) if item != version]
    if mismatched:
        fail("asset version", f"URL versions {mismatched} != {version!r}", body)
    passed("asset version")

    js_body = b""
    for path in ("/app.js", "/style.css"):
        status, hdrs, raw = fetch(opener, base, path)
        if status != 200:
            fail(path, f"HTTP {status}", raw)
        if path == "/app.js":
            js_body = raw
        etag, last_mod = hdrs.get("etag"), hdrs.get("last-modified")
        if not etag and not last_mod:
            fail(path, "missing ETag and Last-Modified", raw)
        cond = {"If-None-Match": etag} if etag else {"If-Modified-Since": last_mod}
        status304, _, raw304 = fetch(opener, base, path, cond)
        if status304 != 304:
            fail(path, f"conditional HTTP {status304} (want 304)", raw304)
        if etag and etag.strip('"') != version:
            fail(path, f"ETag {etag!r} != HTML version {version!r}", raw)
    passed("asset revalidation")

    status, _, raw = fetch(opener, base, "/__no_such_asset__.js")
    if status != 404:
        fail("unknown", f"HTTP {status} (want 404)", raw)
    status, _, raw = fetch(opener, base, "/fonts/")
    listing = raw.decode("utf-8", "replace").lower()
    if status != 404 or any(token in listing for token in ("index of", "parent directory", "cascadiamono")):
        fail("listing", f"HTTP {status} looks like a directory listing", raw)
    passed("unknown 404")

    nested = fetch(opener, base, "/legacy-web/app.js")
    alt = fetch(opener, base, "/app.js")
    if alt[0] == 200:
        js_body = alt[2]
        if nested[0] != 404:
            fail("/legacy-web/app.js", f"HTTP {nested[0]} (want 404)", nested[2])
    elif nested[0] == 200:
        js_body = nested[2]
        if alt[0] != 404:
            fail("/app.js", f"HTTP {alt[0]} (want 404)", alt[2])
    else:
        fail("app.js path", f"/app.js HTTP {alt[0]} /legacy-web/app.js HTTP {nested[0]}", nested[2])
    passed("app.js path")

    for path in ("/files.html", "/file.html"):
        status, _, raw = fetch(opener, base, path)
        if status != 200 or PLACEHOLDER.search(raw.decode("utf-8", "replace")):
            fail(path, f"HTTP {status}", raw)
    passed("files.html/file.html")

    for path, needle in TYPES:
        status, hdrs, raw = fetch(opener, base, path)
        ctype = (hdrs.get("content-type") or "").lower()
        if status != 200 or needle not in ctype:
            fail(path, f"HTTP {status} Content-Type={ctype!r}", raw)
    passed("content types")

    disk = hashlib.sha256((REPO / "legacy-web" / "app.js").read_bytes()).hexdigest()
    served = hashlib.sha256(js_body).hexdigest()
    if served != disk:
        fail("app.js sha256", f"served={served} disk={disk}", js_body[:80])
    passed("app.js sha256")

    meta = get_json(opener, base, "/api/meta")
    host = meta.get("hostname")
    if not host or (str(host) not in page and html.escape(str(host)) not in page):
        fail("hostname", f"/api/meta hostname={host!r} missing from HTML", body)
    passed("hostname")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-static-assets-") as tmp:
        corpus = build_corpus(Path(tmp))
        with isolated_server(corpus, args.binary) as (base, opener):
            try:
                run(base, opener)
            except AssertionError as err:
                raise SystemExit(f"FAIL {err}") from None


if __name__ == "__main__":
    main()
