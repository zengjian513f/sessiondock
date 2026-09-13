#!/usr/bin/env python3
"""WP-C: synthesized Codex /rename command event on isolated HTTP."""
from __future__ import annotations

import argparse
import json
import tempfile
from pathlib import Path
from urllib.parse import urlencode

from history_parity import (
    BINARY, Corpus, codex_message, codex_row, cursor_query, encoded, get_json, isolated_server)


RENAMED_AT = "2026-09-11T10:00:30Z"
TITLE = "WillowRename"
NAME = "WillowRename"


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def stamp(row, ts):
    row["timestamp"] = ts
    return row


def turns(sid, extra=()):
    meta = stamp(codex_row("session_meta", {
        "id": sid, "session_id": sid, "cwd": "/synthetic/rename",
        "timestamp": "2026-09-11T10:00:00Z"}), "2026-09-11T10:00:00Z")
    first_u = stamp(codex_message("user", f"{sid} first question", 1), "2026-09-11T10:00:00Z")
    first_a = stamp(codex_message("assistant", f"{sid} first answer", 2), "2026-09-11T10:00:05Z")
    second = stamp(codex_message("user", f"{sid} second question", 3), "2026-09-11T10:01:00Z")
    return [meta, first_u, first_a, *extra, second]


def build(root: Path):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir(parents=True)
    corpus.put("R", "codex", turns("R"), ["R first question", "R first answer", "R second question"])
    corpus.put("S", "codex", turns("S"), ["S first question", "S first answer", "S second question"])
    index = root / "session_index.jsonl"
    index.write_bytes(encoded({"id": "R", "thread_name": NAME, "updated_at": RENAMED_AT}))
    return corpus, index


def search_results(opener, base, q):
    route = "/api/search?" + urlencode({"q": q, "limit": "60"})
    with opener.open(base + route, timeout=10) as resp:
        raw = resp.read(2 * 1024 * 1024)
    try:
        payload = json.loads(raw) if raw else {}
        if isinstance(payload, dict):
            return payload.get("results") or []
    except json.JSONDecodeError:
        payload = None
    rows = []
    for line in raw.splitlines():
        if not line.strip():
            continue
        packet = json.loads(line)
        if isinstance(packet, dict) and packet.get("results") is not None:
            rows = packet["results"]
        elif isinstance(packet, dict) and isinstance(packet.get("data"), dict):
            rows = packet["data"].get("results") or rows
    return rows or []


def commands(messages):
    return [m for m in messages if m.get("role") == "command"]


def check_r(msgs, area="R-command"):
    cmds = commands(msgs["messages"])
    if len(cmds) != 1:
        fail(area, f"want 1 command got {len(cmds)}")
    cmd = cmds[0]
    want_text = f"/rename {NAME}"
    if cmd.get("text") != want_text or cmd.get("inferred") is not True or cmd.get("counted") is not False:
        fail(area, f"fields {cmd}")
    if not str(cmd.get("event_id") or "").startswith("rename:"):
        fail(area, f"event_id {cmd.get('event_id')}")
    at = cmd.get("ts") or ""
    last_le = -1
    first_gt = None
    for i, m in enumerate(msgs["messages"]):
        if m is cmd:
            continue
        ts = m.get("ts") or ""
        if ts <= at:
            last_le = i
        elif first_gt is None:
            first_gt = i
    idx = msgs["messages"].index(cmd)
    if idx != last_le + 1:
        fail(area, f"index {idx} want {last_le + 1} (after last ts <= renamed_at)")
    if first_gt is not None and idx >= first_gt:
        fail(area, f"command not before first later event (idx {idx} later {first_gt})")
    if msgs.get("message_total") != 3:
        fail(area, f"message_total {msgs.get('message_total')} want 3")
    passed(area)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    n = 0
    with tempfile.TemporaryDirectory(prefix="codex-rename-parity-") as tmp:
        root = Path(tmp)
        corpus, index = build(root)
        with isolated_server(corpus, args.binary,
                             extra_env={"SESSIONDOCK_CODEX_INDEX": str(index)}) as (base, opener):
            uid_r, uid_s = corpus.uid("R"), corpus.uid("S")
            r = get_json(opener, base, "/api/messages/" + uid_r)
            check_r(r)
            n += 1
            s = get_json(opener, base, "/api/messages/" + uid_s)
            if commands(s["messages"]):
                fail("S-none", f"got {commands(s['messages'])}")
            passed("S-none")
            n += 1
            q = cursor_query(r, append=1)
            again = get_json(opener, base, "/api/messages/" + uid_r + "?" + q)
            if commands(again.get("messages") or []):
                fail("append", "rename repeated on append")
            passed("append")
            n += 1
            hits = search_results(opener, base, TITLE)
            for row in hits:
                if row.get("uid") == uid_r or row.get("sid") == "R":
                    snippet = str(row.get("snippet") or "")
                    if "/rename" in snippet or TITLE in snippet and "command" in str(row).lower():
                        fail("search", f"inferred command matched {row}")
            if any(TITLE in str(h.get("snippet") or "") and h.get("sid") == "R" for h in hits):
                fail("search", "title word hit via inferred command snippet")
            passed("search")
            n += 1
            listed = get_json(opener, base, "/api/sessions")
            row = next((x for x in listed["sessions"] if x.get("sid") == "R"), None)
            if not row:
                fail("list", "R missing")
            if TITLE not in str(row.get("title") or "") and row.get("renamed_to") not in (NAME, TITLE):
                fail("list", f"title/renamed_to {row.get('title')} {row.get('renamed_to')}")
            if not row.get("renamed_at"):
                fail("list", "renamed_at empty")
            passed("list")
            n += 1
    print(f"codex_rename_suite: {n} checks passed")


if __name__ == "__main__":
    main()
