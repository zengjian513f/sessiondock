#!/usr/bin/env python3
"""GET /api/session/input-history contract: shape, oldest-first order, duplicates,
hidden protocol/injection, Codex fork prefix, Claude ?agent=, uid 404, limit
budget, debug_run ignored. Synthetic loopback fixtures only.
"""
from __future__ import annotations
import argparse, hashlib, json, sys, tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode
sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, codex_message, codex_row,
    encoded, isolated_server)

BINARY = (p if (p := REPO / "target/release" / DEBUG_BINARY.name).is_file() else DEBUG_BINARY)
CAP, BUDGET, HIDE = 2 * 1024 * 1024, 100_000, "<INSTRUCTIONS>hidden-injection</INSTRUCTIONS>"
CLAUDE = ["line1\nline2", "猫 café 名录", "dup-text", "dup-text",
          "Claude Esc input", "Claude replacement", "! pwd", "claude-last"]


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)

def call(opener, base, **q):
    route = "/api/session/input-history" + ("?" + urlencode(q) if q else "")
    try:
        with opener.open(base + route, timeout=10) as resp:
            raw, code = resp.read(CAP + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    except Exception as err:
        fail(route, str(err))
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        payload = {}
    return route, code, payload, raw


def texts(opener, base, **q):
    route, code, payload, raw = call(opener, base, **q)
    hist = payload.get("history") if isinstance(payload, dict) else None
    if code != 200 or not isinstance(hist, list) or any(k not in payload for k in ("end", "version", "anchor")):
        fail(route, f"HTTP {code}", raw)
    if any(not isinstance(r, dict) or "text" not in r or "ts" not in r for r in hist):
        fail(route, "history items must be {text, ts}", raw)
    return route, raw, [r["text"] for r in hist]

def want(opener, base, code, area, **q):
    route, got, payload, raw = call(opener, base, **q)
    if got != code or not isinstance(payload, dict) or not payload.get("error"):
        fail(area, f"{route} HTTP {got} want {code}", raw)
    return payload

def build(root):
    data = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    sid = "claude-ih"
    u = lambda i, par, t, **k: claude_row(sid, "user", i, par, t, **k)
    a = lambda i, par: claude_row(sid, "assistant", i, par, "ok")
    rows, parent = [], None
    for i, text in enumerate(["line1\nline2", "猫 café 名录", "dup-text", "dup-text"]):
        rows += [u(f"u{i}", parent, text), a(f"a{i}", f"u{i}")]; parent = f"a{i}"
    rows += [u("esc", parent, "Claude Esc input"), u("rep", parent, "Claude replacement"),
             a("a4", "rep"), u("hid", "a4", HIDE), u("ws", "a4", "  \n"),
             u("sum", "a4", "INTERNAL COMPACT SUMMARY", isCompactSummary=True),
             u("cmd", "a4", "<bash-input>pwd</bash-input>"), a("a5", "cmd"),
             u("u4", "a5", "claude-last"), a("a6", "u4")]
    data.put(sid, "claude", rows, CLAUDE)
    agent = data.paths[sid].with_suffix("") / "subagents/agent-helper.jsonl"
    agent.parent.mkdir(parents=True)
    agent.write_bytes(b"".join(encoded(r) for r in [
        claude_row(sid, "user", "ag-u", None, "Claude agent-only", isSidechain=True, agentId="helper"),
        claude_row(sid, "assistant", "ag-a", "ag-u", "ok", isSidechain=True, agentId="helper")]))
    def meta(name, **extra):
        return codex_row("session_meta", {"id": name, "session_id": name,
            "cwd": "/synthetic/ih", "timestamp": "2026-09-11T09:00:00Z", **extra})
    prefix = [meta("codex-parent"), codex_message("user", "Codex inherited-prefix"),
              codex_message("assistant", "ok", 2)]
    data.put("codex-parent", "codex", prefix + [codex_message("user", "Codex discarded-tail", 3)], [])
    data.put("codex-fork", "codex", [meta("codex-fork", forked_from_id="codex-parent",
        history_mode="paginated", history_base={"thread_id": "codex-parent",
            "end_byte_offset": sum(len(encoded(r)) for r in prefix)}),
        codex_message("user", "Codex fork-input\n第二行"),
        *[x for n, t in ((2, "dup-text"), (4, "dup-text"), (6, HIDE))
          for x in (codex_message("assistant", "ok", n), codex_message("user", t, n + 1))],
        codex_row("response_item", {"type": "message", "role": "developer",
            "content": [{"type": "input_text", "text": HIDE}]}, 8)], [])
    gpath = root / "grok/project-ih/grok-ih"
    gpath.mkdir(parents=True)
    (gpath / "summary.json").write_text(json.dumps(
        {"info": {"id": "grok-ih", "cwd": "/synthetic/ih"}, "generated_title": "grok-ih"}))
    (gpath / "chat_history.jsonl").write_bytes(b"".join(encoded(r) for r in [
        {"type": "user", "content": "<user_query>Grok line1\nline2</user_query>", "prompt_index": 1},
        {"type": "assistant", "content": "ok"}, {"type": "user", "content": "Grok 猫", "prompt_index": 2},
        {"type": "assistant", "content": "ok"}] + [
        {"type": k, "content": t, **({"prompt_index": i} if k == "user" else {})}
        for i, t in ((3, "dup-text"), (4, "dup-text"), (5, HIDE)) for k in ("user", "assistant")
        if k == "user" or t != HIDE]))
    return data, data.uid("claude-ih"), data.uid("codex-parent"), data.uid("codex-fork"), \
        "grok:" + hashlib.sha1(str(gpath).encode()).hexdigest()[:16]

def run(opener, base, claude, parent, fork, grok):
    route, raw, ct = texts(opener, base, uid=claude)
    _, raw, gt = texts(opener, base, uid=grok)
    _, raw, pt = texts(opener, base, uid=parent)
    if ct != CLAUDE or gt != ["Grok line1\nline2", "Grok 猫", "dup-text", "dup-text"] \
            or pt != ["Codex inherited-prefix", "Codex discarded-tail"]:
        fail(route, f"order claude={ct!r} grok={gt!r} parent={pt!r}", raw)
    passed("shape and chronological oldest-first")
    if ct.count("dup-text") != 2 or gt.count("dup-text") != 2:
        fail(route, f"duplicates claude={ct} grok={gt}", raw)
    passed("duplicates kept")
    if "hidden-injection" in "\n".join(ct + gt + pt) or "INTERNAL COMPACT SUMMARY" in "\n".join(ct):
        fail(route, "protocol/injection leaked", raw)
    passed("hidden protocol/injection lines")
    _, raw, ft = texts(opener, base, uid=fork)
    if "Codex inherited-prefix" not in ft or "Codex fork-input\n第二行" not in ft \
            or "Codex discarded-tail" in ft or "hidden-injection" in "\n".join(ft) or ft.count("dup-text") != 2:
        fail(route, f"fork {ft!r}", raw)
    passed("fork inherited prefix")
    _, raw, ag = texts(opener, base, uid=claude, agent="helper")
    if "Claude agent-only" in ct or "line1\nline2" in ag or "claude-last" in ag:
        fail(route, f"agent leak parent={ct!r} agent={ag!r}", raw)
    want(opener, base, 404, "agent= scoping", uid=claude, agent="missing")
    passed("agent= scoping")
    want(opener, base, 404, "unknown uid", uid="claude:deadbeefdeadbeef")
    passed("unknown uid 404")
    want(opener, base, 404, "missing uid")
    passed("missing uid 404")
    _, raw, limited = texts(opener, base, uid=claude, limit="1")
    if limited != ct or len(ct) > BUDGET:
        fail(route, f"limit/budget {limited!r} n={len(ct)}", raw)
    passed("limit ignored; count ≤ 100000")
    # `debug_run` selects the list view only; the detail routes ignore it.
    _, raw, with_run = texts(opener, base, uid=claude, debug_run="1")
    if with_run != ct:
        fail("debug_run", f"history must ignore debug_run: {with_run!r}", raw)
    passed("debug_run ignored")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-input-history-") as tmp:
        data, claude, parent, fork, grok = build(Path(tmp))
        with isolated_server(data, args.binary) as (base, opener):
            run(opener, base, claude, parent, fork, grok)


if __name__ == "__main__":
    main()
