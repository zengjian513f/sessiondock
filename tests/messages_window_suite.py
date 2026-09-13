#!/usr/bin/env python3
"""HTTP contract of GET /api/messages/{uid} beyond history_parity.

Window/partial budgets, status filtering, agent views, query validation,
append-reset, and version/cursor types. Synthetic fixtures, loopback only.
"""
from __future__ import annotations

import argparse, json, sys, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote, urlencode

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, codex_message, codex_row,
    encoded, isolated_server)

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
CAP = 8 * 1024 * 1024


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def fetch(opener, base, route, want=200):
    try:
        with opener.open(base + route, timeout=30) as resp:
            raw, code = resp.read(CAP + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    if len(raw) > CAP:
        fail(route, "oversized response", raw[:240])
    if code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)
    return payload, raw


def route(uid, **query):
    path = "/api/messages/" + quote(uid, safe=":")
    return path + ("?" + urlencode(query) if query else "")


def sidecar(corpus, owner, agent, text):
    path = corpus.paths[owner].with_suffix("") / "subagents" / f"agent-{agent}.jsonl"
    path.parent.mkdir(parents=True)
    path.write_bytes(b"".join(encoded(row) for row in [
        claude_row(owner, "user", agent + "-u", None, text, isSidechain=True, agentId=agent),
        claude_row(owner, "assistant", agent + "-a", agent + "-u", text + " a",
                   isSidechain=True, agentId=agent)]))
    path.with_suffix(".meta.json").write_text(
        json.dumps({"description": agent, "agentType": "reviewer"}))
    return path


def build(root):
    corpus = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    sid = "codex-window"
    rows = [codex_row("session_meta", {
        "id": sid, "session_id": sid, "cwd": "/synthetic/window",
        "timestamp": "2026-09-11T09:00:00Z"})]
    for i in range(1500):
        if i == 42:
            rows.append(codex_row("compacted", {}, i + 1))
        else:
            rows.append(codex_message(
                "user" if i % 2 == 0 else "assistant", f"w{i:04d}", i + 1))
    corpus.put(sid, "codex", rows, [])
    corpus.put("codex-status", "codex", [
        codex_row("session_meta", {"id": "codex-status", "session_id": "codex-status",
                                  "cwd": "/synthetic/window"}),
        codex_row("event_msg", {"type": "task_started"}),
        codex_message("user", "status prompt", 1),
        codex_row("event_msg", {"type": "task_complete"}),
    ], [])
    for sid, text in (("claude-main", "main q"), ("claude-other", "other q")):
        corpus.put(sid, "claude", [
            claude_row(sid, "user", "u0", None, text),
            claude_row(sid, "assistant", "a0", "u0", text + " a"),
        ], [])
    sidecar(corpus, "claude-main", "claude-agent", "agent q")
    sidecar(corpus, "claude-other", "other-agent", "foreign q")
    return corpus


def checkpoint(area, payload, raw, size):
    ver = payload.get("version")
    if not isinstance(ver, dict):
        fail(area, "version not object", raw)
    if ver.get("exists") is not True or not isinstance(ver.get("mtime"), int):
        fail(area, "version.exists bool / mtime int", raw)
    if not isinstance(ver.get("size"), int) or ver["size"] != size:
        fail(area, f"version.size {ver.get('size')} want file length {size}", raw)
    head = ver.get("head")
    if not isinstance(head, str) or not head.startswith("rs-m2-1:"):
        fail(area, "version.head schema", raw)
    if payload.get("end") != size:
        fail(area, f"end {payload.get('end')} != committed file length {size}", raw)
    if not isinstance(payload.get("reset"), bool) or not isinstance(payload.get("start"), int):
        fail(area, "reset/start types", raw)
    if not isinstance(payload.get("anchor"), str) or not payload["anchor"]:
        fail(area, "anchor type", raw)
    cur = (payload.get("meta") or {}).get("cursor")
    if not isinstance(cur, dict):
        fail(area, "meta.cursor missing", raw)
    if cur.get("end") != payload["end"] or cur.get("head") != head or cur.get("anchor") != payload["anchor"]:
        fail(area, "meta.cursor must mirror end/head/anchor", raw)


def run(opener, base, corpus):
    uid = corpus.uid("codex-window")
    size = corpus.paths["codex-window"].stat().st_size
    win, wraw = fetch(opener, base, route(uid, window="1"))
    partial = win.get("partial")
    if not isinstance(partial, dict):
        fail("window", "partial object required", wraw)
    for key in ("head", "tail", "omitted", "cursor"):
        if key not in partial:
            fail("window", f"partial missing {key}", wraw)
    head, tail, omitted = partial["head"], partial["tail"], partial["omitted"]
    if not isinstance(head, int) or not isinstance(tail, int) or head > 100 or tail > 500:
        fail("window", f"head {head} tail {tail} (want head<=100 tail<=500)", wraw)
    if not isinstance(omitted, int) or omitted <= 0:
        fail("window", f"omitted {omitted}", wraw)
    if not isinstance(partial.get("cursor"), str) or not partial["cursor"]:
        fail("window", "partial.cursor", wraw)
    msgs = win.get("messages") or []
    if len(msgs) != head + tail:
        fail("window", f"len(messages) {len(msgs)} != head+tail {head + tail}", wraw)
    checkpoint("window", win, wraw, size)
    full, fraw = fetch(opener, base, route(uid))
    if full.get("partial") is not None:
        fail("full", "non-windowed partial must be null", fraw)
    shown = full.get("messages") or []
    counted = sum(1 for row in shown if row.get("counted") is not False)
    if win.get("message_total") != counted or full.get("message_total") != counted:
        fail("full", f"message_total window={win.get('message_total')} full={full.get('message_total')} counted={counted}", fraw)
    if counted >= len(shown):
        fail("full", "compact counted:false should drop message_total below len(messages)", fraw)
    if len(shown) != 1500 or len(shown) > 100000:
        fail("full", f"expected 1500 display events under 100000 budget, got {len(shown)}", fraw)
    checkpoint("full", full, fraw, size)
    passed("window=1 partial{head,tail,omitted,cursor} counted message_total")
    passed("non-windowed full history under 100000-event budget")

    body, raw = fetch(opener, base, route(corpus.uid("codex-status")))
    if any(row.get("role") == "status" for row in body.get("messages") or []):
        fail("status", "status rows must not appear in messages", raw)
    act = body.get("activity")
    if not isinstance(act, dict) or act.get("role") != "status" or act.get("state") != "idle":
        fail("status", "activity should be last status (task_complete → idle)", raw)
    if body.get("activity_changed") is not True:
        fail("status", "activity_changed true on reset", raw)
    passed("status filtered; activity/activity_changed")

    main = corpus.uid("claude-main")
    agent, araw = fetch(opener, base, route(main, agent="claude-agent"))
    meta = agent.get("meta") or {}
    if meta.get("uid") != main or meta.get("agent_id") != "claude-agent" or meta.get("sid") != "claude-agent":
        fail("agent", f"view meta uid/agent_id/sid {meta}", araw)
    if meta.get("agent_type") != "reviewer":
        fail("agent", f"agent_type {meta.get('agent_type')}", araw)
    texts = [row.get("text") for row in agent.get("messages") or []]
    if "agent q" not in texts:
        fail("agent", f"sidecar texts {texts}", araw)
    asize = (corpus.paths["claude-main"].with_suffix("") / "subagents" /
             "agent-claude-agent.jsonl").stat().st_size
    if agent.get("end") != asize:
        fail("agent", f"end {agent.get('end')} != sidecar length {asize}", araw)
    unknown, uraw = fetch(opener, base, route(main, agent="no-such-agent"), want=404)
    if "子代理" not in str(unknown.get("error", "")):
        fail("agent", "unknown agent 404", uraw)
    foreign, frraw = fetch(opener, base, route(main, agent="other-agent"), want=404)
    if "子代理" not in str(foreign.get("error", "")):
        fail("agent", "foreign-session agent 404 (差异表 不属于该主会话)", frraw)
    passed("agent view meta; unknown 404; other-session agent 404")

    missing, mraw = fetch(opener, base, "/api/messages/claude:missing", want=404)
    if not missing.get("error") or missing.get("code") != "session_error":
        fail("uid", "invalid uid is 404 session_error (messages path is not 400)", mraw)
    passed("invalid uid 404")

    fetch(opener, base, route(main, agent="x" * 257), want=404)
    for key, n in (("head", 257), ("anchor", 513)):
        fetch(opener, base, route(main, **{key: "x" * n}))
    passed("message selectors have no additional length rejection")

    stale, sraw = fetch(opener, base, route(main, append="1", start=0, head="stale", anchor="stale"))
    if stale.get("reset") is not True or stale.get("messages") != []:
        fail("append", "stale append=1 must reset:true with empty messages", sraw)
    if stale.get("start") != stale.get("end"):
        fail("append", f"start {stale.get('start')} != end {stale.get('end')}", sraw)
    checkpoint("append", stale, sraw, corpus.paths["claude-main"].stat().st_size)
    passed("append=1 stale cursor reset empty start==end")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-messages-window-") as tmp:
        corpus = build(Path(tmp))
        with isolated_server(corpus, args.binary) as (ctx_base, opener):
            run(opener, ctx_base, corpus)


if __name__ == "__main__":
    main()
