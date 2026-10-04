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
from urllib.parse import parse_qs, unquote, urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import BINARY as DEBUG_BINARY, REPO, build_corpus, get_json, isolated_server
from frontend_paths import FrontendHTML, entry_asset, frontend_dir, local_asset

BINARY = next(p for p in (REPO / "target/release/sessiondock", DEBUG_BINARY) if p.is_file())
BODY_CAP = 2 * 1024 * 1024
PLACEHOLDER = re.compile(r"__SESSIONDOCK_[A-Z0-9_]+__")
TYPES = {".js": "javascript", ".css": "text/css", ".webmanifest": "manifest",
         ".woff2": "woff2", ".svg": "svg", ".png": "image/png"}


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
    web = frontend_dir()
    entry = entry_asset(web)
    entry_path = "/" + entry.relative_to(web).as_posix()
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

    version = meta_content(page, "sessiondock-build")
    document = FrontendHTML(page)
    resource_paths = {}
    for src in document.resources:
        url = urlsplit(src)
        if url.scheme or url.netloc:
            continue
        asset = local_asset(web, src)
        path = "/" + asset.relative_to(web).as_posix()
        resource_paths[path] = src
        versions = parse_qs(url.query).get("v", [])
        if (asset.suffix in (".js", ".css", ".woff2", ".ttf") and versions != [version]) or (versions and versions != [version]):
            fail("asset version", f"{src} must carry HTML build v={version}", body)
    if not version or entry_path not in resource_paths or not document.styles:
        fail("asset version", "missing build, served entry or stylesheet", body)
    passed("asset version")

    js_body = b""
    cache_paths = [path for path in resource_paths if Path(path).suffix in (".js", ".css")]
    for path in cache_paths:
        status, hdrs, raw = fetch(opener, base, path)
        if status != 200:
            fail(path, f"HTTP {status}", raw)
        if path == entry_path:
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

    for path in dict.fromkeys([*cache_paths, "/vendor/xterm.js", "/fonts/UbuntuSansMono.woff2"]):
        status, hdrs, raw = fetch(opener, base, f"{path}?v={version}")
        cache = (hdrs.get("cache-control") or "").lower()
        if status != 200 or "immutable" not in cache or "max-age=" not in cache:
            fail(path, f"versioned HTTP {status} Cache-Control={cache!r} (want immutable)", raw)
    status, hdrs, raw = fetch(opener, base, f"{entry_path}?v=stale")
    if status != 200 or "immutable" in (hdrs.get("cache-control") or "").lower():
        fail(f"{entry_path}?v=stale", f"HTTP {status} Cache-Control={hdrs.get('cache-control')!r} must revalidate", raw)
    status, _, raw = fetch(opener, base, "/typography.css")
    css = raw.decode("utf-8", "replace")
    if status != 200 or PLACEHOLDER.search(css) or f"?v={version}" not in css:
        fail("/typography.css", f"HTTP {status}; font URLs must carry ?v={version}", raw)
    font_urls = re.findall(r"url\(\s*['\"]?([^'\"\s)]+)", css)
    for src in font_urls:
        url = urlsplit(src)
        if url.scheme or url.netloc:
            continue
        local_asset(web, src)
        if parse_qs(url.query).get("v") != [version]:
            fail("typography fonts", f"unversioned font URL: {src}", raw)
        status, hdrs, font = fetch(opener, base, "/" + url.path.lstrip("/") + "?" + url.query)
        if status != 200 or "immutable" not in (hdrs.get("cache-control") or "").lower():
            fail("typography fonts", f"{src}: HTTP {status}, expected immutable font", font)
    passed("immutable versioned assets")

    status, _, raw = fetch(opener, base, "/__no_such_asset__.js")
    if status != 404:
        fail("unknown", f"HTTP {status} (want 404)", raw)
    status, _, raw = fetch(opener, base, "/fonts/")
    listing = raw.decode("utf-8", "replace").lower()
    if status != 404 or any(token in listing for token in ("index of", "parent directory", "cascadiamono")):
        fail("listing", f"HTTP {status} looks like a directory listing", raw)
    passed("unknown 404")

    nested_path = "/web/dist-migration" + entry_path
    status, _, raw = fetch(opener, base, nested_path)
    if status != 404:
        fail("entry path", f"{nested_path}: HTTP {status} (want 404)", raw)
    passed("entry path")

    for path in ("/files.html", "/file.html"):
        status, _, raw = fetch(opener, base, path)
        if status != 200 or PLACEHOLDER.search(raw.decode("utf-8", "replace")):
            fail(path, f"HTTP {status}", raw)
        entry_asset(web, path.lstrip("/"))
        for src in FrontendHTML(raw.decode("utf-8", "replace")).resources:
            url = urlsplit(src)
            if url.scheme or url.netloc:
                continue
            asset = local_asset(web, src, path.lstrip("/"))
            asset_path = "/" + asset.relative_to(web).as_posix()
            resource_paths[asset_path] = src
            versions = parse_qs(url.query).get("v", [])
            if asset.suffix in (".js", ".css") and versions != [version]:
                fail(path, f"unversioned page resource: {src}", raw)
            status_asset, _, raw_asset = fetch(opener, base, asset_path + ("?" + url.query if url.query else ""))
            if status_asset != 200:
                fail(path, f"{src}: HTTP {status_asset}", raw_asset)
    passed("files.html/file.html")

    resource_paths.update({"/manifest.webmanifest": "", "/fonts/CascadiaMono.woff2": "", "/icons/icon.svg": ""})
    status, _, raw = fetch(opener, base, "/manifest.webmanifest")
    if status != 200:
        fail("manifest", f"HTTP {status}", raw)
    for icon in json.loads(raw).get("icons", []):
        asset = local_asset(web, icon["src"])
        resource_paths["/" + asset.relative_to(web).as_posix()] = icon["src"]
    for path in resource_paths:
        needle = TYPES.get(Path(unquote(path)).suffix)
        if needle is None:
            continue
        status, hdrs, raw = fetch(opener, base, path)
        ctype = (hdrs.get("content-type") or "").lower()
        if status != 200 or needle not in ctype:
            fail(path, f"HTTP {status} Content-Type={ctype!r}", raw)
    passed("content types")

    disk = hashlib.sha256(entry.read_bytes()).hexdigest()
    served = hashlib.sha256(js_body).hexdigest()
    if served != disk:
        fail("entry sha256", f"{entry_path}: served={served} disk={disk}", js_body[:80])
    passed("entry sha256")

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
