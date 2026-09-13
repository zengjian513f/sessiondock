#!/usr/bin/env python3
"""Read-side media accepts nlink>1 files; write-side rename still refuses them.

Batch 44 WP-D (docs/files.md, docs/media.md): GET /api/media of a hard-linked PNG
is ordinary; POST /api/session/files/action rename is 403 file_hardlink_forbidden.
Synthetic temp fixtures and an isolated loopback Rust server only.
"""
from __future__ import annotations

import argparse, json, os, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request

from history_parity import BINARY, Corpus, claude_row, get_json, isolated_server
SID = "claude-hardlink"
# 1×1 RGBA PNG (IHDR 1×1, IDAT, IEND).
PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489"
    "0000000a49444154789c63000100000500010d0a2db40000000049454e44ae426082"
)


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def claude_file_image(path):
    return {"type": "image", "source": {"type": "file", "path": str(path)}}


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    files = root / "files"
    files.mkdir()
    primary, alias = files / "disk.png", files / "disk-alias.png"
    primary.write_bytes(PNG)
    os.link(primary, alias)
    if primary.stat().st_nlink != 2:
        fail("fixture", f"nlink={primary.stat().st_nlink} (want 2)")
    content = [claude_file_image(primary),
               {"type": "text", "text": f"`{files}/` `{primary}`"}]
    corpus.put(SID, "claude", [
        claude_row(SID, "user", "u", None, content, cwd=str(files)),
        claude_row(SID, "assistant", "a", "u", "ok"),
    ], [])
    state = root / "state"
    state.mkdir(mode=0o700)
    return corpus, files, primary, alias, state


def media_items(payload):
    return [item for row in payload.get("messages", []) for item in row.get("media", [])]


def post_action(opener, base, body):
    raw = json.dumps(body).encode()
    req = Request(base + "/api/session/files/action", data=raw, method="POST",
                  headers={"Content-Type": "application/json"})
    try:
        with opener.open(req, timeout=15) as resp:
            return resp.status, json.loads(resp.read(65536) or b"{}")
    except HTTPError as err:
        blob = err.read(4096)
        try:
            parsed = json.loads(blob) if blob else {}
        except json.JSONDecodeError:
            parsed = {}
        return err.code, parsed


def run(opener, base, corpus, files, primary):
    checks = 0
    uid = corpus.uid(SID)
    payload = get_json(opener, base, "/api/messages/" + uid)
    items = media_items(payload)
    if not items:
        fail("messages", "no media descriptors", json.dumps(payload).encode())
    item = items[0]
    if item.get("error"):
        fail("messages", f"descriptor error={item.get('error')!r}", json.dumps(item).encode())
    src = item.get("src") or item.get("url") or ""
    if not src or "error" in item:
        fail("messages", f"missing token/url {item}", json.dumps(item).encode())
    passed("messages: hardlink PNG descriptor has token, no error")
    checks += 1

    path = src if src.startswith("/api/") else "/api/media/" + src.lstrip("/")
    try:
        with opener.open(base + path, timeout=15) as resp:
            code, ctype, body = resp.status, resp.headers.get("Content-Type", ""), resp.read(65536)
    except HTTPError as err:
        fail("media", f"HTTP {err.code}", err.read(4096))
    if code != 200 or "image/png" not in ctype.lower() or body != PNG:
        fail("media", f"HTTP {code} type={ctype!r} len={len(body)}", body[:80])
    passed("media GET: 200 image/png identical bytes")
    checks += 1

    status, err = post_action(opener, base, {
        "uid": uid, "ref": str(files) + "/", "action": "rename",
        "paths": [str(primary)], "name": "renamed.png",
    })
    code = str(err.get("code") or "")
    if status != 403 or "hardlink" not in code.lower():
        fail("rename", f"HTTP {status} body={err}", json.dumps(err).encode())
    passed(f"rename hardlink 403 {code}")
    checks += 1

    if not primary.exists() or primary.stat().st_nlink != 2:
        fail("nlink", f"exists={primary.exists()} nlink={primary.stat().st_nlink if primary.exists() else 0}")
    if not (files / "disk-alias.png").exists():
        fail("nlink", "alias removed")
    passed("source still nlink==2")
    checks += 1
    return checks


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-media-hardlink-") as tmp:
        corpus, files, primary, _alias, state = build(Path(tmp))
        with isolated_server(
            corpus, args.binary, state_dir=state,
            file_roots=(files,), file_write_roots=(files,),
        ) as (base, opener):
            n = run(opener, base, corpus, files, primary)
    print(f"media_hardlink_suite: {n} checks passed")


if __name__ == "__main__":
    main()
