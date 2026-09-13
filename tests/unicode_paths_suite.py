#!/usr/bin/env python3
"""Unicode and awkward file/directory names across the read model.

Synthetic temp fixtures and an isolated loopback Rust server only. Titles/cwd
must round-trip; UIDs are sha1(path)[:16] and must survive a server restart.
"""
from __future__ import annotations
import argparse, hashlib, json, os, sys, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote, urlencode
from urllib.request import Request
sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, codex_message, codex_row,
    encoded, isolated_server)

BINARY = (p if (p := REPO / "target/release" / DEBUG_BINARY.name).is_file() else DEBUG_BINARY)
UNI, CWD = "中文 😀 's.", "/synthetic/中文 😀 's."
TITLE = {k: f"zxuni-{k} {UNI}" for k in ("claude", "codex", "grok")}
SIDS = ("claude-uni", "codex-uni", "grok-uni")


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")
def passed(area):
    print(f"PASS {area}", flush=True)
def uid_of(source, path):
    return source + ":" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


def fetch(opener, base, route, want=200, method="GET", body=None):
    req = Request(base + route, data=body, method=method)
    if body is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with opener.open(req, timeout=15) as resp:
            raw, code = resp.read(2 * 1024 * 1024 + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    if code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    try:
        return (json.loads(raw) if raw else {}), raw
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)


def build(root):
    corpus = Corpus(root)
    for name in ("claude", "codex", "grok"):
        (root / name).mkdir()
    files, udir, note = root / "files", root / "files" / UNI, root / "files" / UNI / f"note {UNI}.md"
    udir.mkdir(parents=True)
    note.write_text("unicode-note\n", encoding="utf-8")
    proj = root / "claude" / f"proj {UNI}"
    proj.mkdir()
    cpath = proj / "claude-uni.jsonl"
    cpath.write_bytes(b"".join(encoded(r) for r in [
        claude_row("claude-uni", "user", "u0", None, TITLE["claude"], cwd=CWD),
        claude_row("claude-uni", "assistant", "a0", "u0", "ok", cwd=CWD),
        claude_row("claude-uni", "user", "u1", "a0", f"Open `{files}` `{udir}` `{note}`", cwd=CWD),
        claude_row("claude-uni", "assistant", "a1", "u1", "ok", cwd=CWD)]))
    corpus.paths["claude-uni"] = cpath
    agent = cpath.with_suffix("") / "subagents" / "agent-helper.jsonl"
    agent.parent.mkdir(parents=True)
    agent.write_bytes(b"".join(encoded(r) for r in [
        claude_row("claude-uni", "user", "au", None, TITLE["claude"] + " agent", cwd=CWD,
                   isSidechain=True, agentId="helper"),
        claude_row("claude-uni", "assistant", "aa", "au", "ok", cwd=CWD,
                   isSidechain=True, agentId="helper")]))
    agent.with_suffix(".meta.json").write_text(json.dumps({"description": "sidecar", "agentType": "reviewer"}))
    linked = root / "linked-source.jsonl"
    linked.write_bytes(encoded(
        claude_row("link-sid", "user", "l0", None, "linked-body", cwd=CWD)))
    os.symlink(linked, proj / "link.jsonl")
    (proj / "nl\n.jsonl").write_bytes(encoded(
        claude_row("newline-sid", "user", "n0", None, "newline-body", cwd=CWD)))
    try:
        (proj / "nul\0.jsonl").write_bytes(b"{}\n")
    except (OSError, ValueError):
        pass
    cx = root / "codex" / f"dir {UNI}"
    cx.mkdir()
    corpus.paths["codex-uni"] = cx / "rollout-codex-uni.jsonl"
    corpus.paths["codex-uni"].write_bytes(b"".join(encoded(r) for r in [
        codex_row("session_meta", {"id": "codex-uni", "session_id": "codex-uni", "cwd": CWD,
                                  "timestamp": "2026-09-11T09:00:00Z"}),
        codex_message("user", TITLE["codex"]), codex_message("assistant", "ok", 2)]))
    gdir = root / "grok" / quote(CWD, safe="") / "grok-uni"
    gdir.mkdir(parents=True)
    (gdir / "summary.json").write_text(json.dumps({
        "info": {"id": "grok-uni", "cwd": CWD}, "generated_title": TITLE["grok"]}), encoding="utf-8")
    (gdir / "chat_history.jsonl").write_bytes(b"".join(encoded(r) for r in [
        {"type": "user", "content": TITLE["grok"], "prompt_index": 1}, {"type": "assistant", "content": "ok"}]))
    corpus.paths["grok-uni"] = gdir
    return corpus, files, udir, note


def listed(opener, base):
    data, raw = fetch(opener, base, "/api/sessions")
    rows = data.get("sessions") if isinstance(data.get("sessions"), list) else None
    if not rows or not isinstance(data.get("sig"), str):
        fail("list", "need sessions list and sig", raw)
    return data, raw, {row["sid"]: row for row in rows if isinstance(row, dict) and "sid" in row}


def run(opener, base, corpus, files, udir, note):
    if "127.0.0.1" not in base:
        fail("base", f"non-loopback {base}")
    data, raw, by = listed(opener, base)
    nl = next((r for r in by.values() if "\n" in (r.get("path") or "")), None)
    if nl is None or nl.get("sid") != "newline-sid":
        fail("reject", "newline-named jsonl must be listed (walk skips links, not control chars)", raw)
    linked = by.get("link-sid")
    if not linked or "link.jsonl" not in (linked.get("path") or "") or any("\0" in str(r) for r in by.values()):
        fail("paths", f"symlink missing or NUL path listed {sorted(by)}", raw)
    passed("symlink session and newline-named jsonl are listed; NUL cannot be created")
    expect = {s: uid_of(src, corpus.paths[s]) for s, src in zip(SIDS, ("claude", "codex", "grok"))}
    if set(by) != set(SIDS) | {"newline-sid", "link-sid"}:
        fail("list", f"listed {sorted(by)}", raw)
    for sid, source in zip(SIDS, ("claude", "codex", "grok")):
        row = by[sid]
        if row.get("uid") != expect[sid] or row.get("cwd") != CWD or row.get("title") != TITLE[source]:
            fail("roundtrip", f"{sid} {row.get('uid')} title={row.get('title')!r} cwd={row.get('cwd')!r}", raw)
    passed("list unicode title/cwd exact round-trip")
    items, cu = by["claude-uni"].get("agent_items") or [], by["claude-uni"]["uid"]
    if not any(isinstance(it, dict) and it.get("id") == "helper" for it in items):
        fail("agent", "claude unicode project missing helper sidecar", raw)
    body, braw = fetch(opener, base, "/api/messages/" + quote(cu, safe=":") + "?" + urlencode({"agent": "helper"}))
    texts = [m.get("text") for m in body.get("messages") or [] if isinstance(m, dict)]
    if TITLE["claude"] + " agent" not in texts:
        fail("agent", "agent view missing sidecar text", braw)
    passed("claude agent sidecar under unicode project dir")
    for sid, source in zip(SIDS, ("claude", "codex", "grok")):
        uid = by[sid]["uid"]
        msg, mraw = fetch(opener, base, "/api/messages/" + quote(uid, safe=":"))
        texts = [m.get("text") for m in msg.get("messages") or [] if isinstance(m, dict)]
        if TITLE[source] not in texts:
            fail("messages", f"{sid} missing {TITLE[source]!r}", mraw)
        found, sraw = fetch(opener, base, "/api/search?" + urlencode({"q": TITLE[source]}))
        if uid not in [h.get("uid") for h in found.get("results") or []]:
            fail("search", f"{sid} not in hits", sraw)
        hist, hraw = fetch(opener, base, "/api/session/input-history?" + urlencode({"uid": uid}))
        lines = [r.get("text") for r in hist.get("history") or [] if isinstance(r, dict)]
        if TITLE[source] not in lines or any(k not in hist for k in ("end", "version", "anchor")):
            fail("input-history", f"{sid} {lines!r}", hraw)
    passed("messages search input-history for each unicode session")
    listing, lraw = fetch(opener, base, "/api/session/files?" + urlencode({"uid": cu, "ref": str(files)}))
    names = [e.get("name") for e in listing.get("entries") or [] if isinstance(e, dict)]
    if UNI not in names or listing.get("writable") is not False or listing.get("root") != files.anchor:
        fail("files", f"root listing names={names!r} root={listing.get('root')!r}", lraw)
    nested, nraw = fetch(opener, base, "/api/session/files?" + urlencode({"uid": cu, "ref": str(udir)}))
    if note.name not in [e.get("name") for e in nested.get("entries") or [] if isinstance(e, dict)]:
        fail("files", f"unicode dir missing {note.name!r}", nraw)
    passed("session/files lists unicode directory names")
    resolved, rraw = fetch(opener, base, "/api/session/resolve-files", method="POST",
                           body=json.dumps({"uid": cu, "refs": [str(note), str(udir)]}).encode())
    got = resolved.get("resolved") or {}
    if got.get(str(note)) != str(note) or got.get(str(udir)) != str(udir):
        fail("resolve-files", f"unicode refs {got}", rraw)
    passed("resolve-files accepts unicode references")
    return expect


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-unicode-paths-") as tmp:
        corpus, files, udir, note = build(Path(tmp))
        with isolated_server(corpus, args.binary, file_roots=(files,)) as (base, opener):
            first = run(opener, base, corpus, files, udir, note)
        with isolated_server(corpus, args.binary, file_roots=(files,)) as (base, opener):
            _, raw, by = listed(opener, base)
            if {sid: by[sid]["uid"] for sid in SIDS if sid in by} != first:
                fail("uid-stable", f"first={first} restart={by}", raw)
        passed("uids stable across two server starts")


if __name__ == "__main__":
    main()
