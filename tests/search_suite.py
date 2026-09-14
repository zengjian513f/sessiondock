#!/usr/bin/env python3
"""Search contract coverage over 12 synthetic sessions; no Chromium.

Loopback-only claude/codex/grok fixtures: text-role matching, case/word/regex,
lookbehind 400, linear catastrophic regex, 200-hit cap, NDJSON progress, early close.
"""
import argparse, http.client, json, sys, tempfile, time
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode, urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, codex_message, codex_row,
    encoded, isolated_server, unreadable_claude_row)

BINARY = (p if (p := REPO / "target/release" / DEBUG_BINARY.name).is_file() else DEBUG_BINARY)
CAP = 2 * 1024 * 1024


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def grok_put(root, sid, rows):
    path = root / "grok/project-search" / sid
    path.mkdir(parents=True)
    (path / "summary.json").write_text(json.dumps({"info": {"id": sid, "cwd": "/synthetic/search"}, "generated_title": sid}))
    (path / "chat_history.jsonl").write_bytes(b"".join(encoded(row) for row in rows))


def build(root):
    data = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    sid = "claude-text"
    data.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "zxneedle ZxCaSe zxcase ZXCASE zxcaseword 猫"),
        claude_row(sid, "assistant", "a0", "u0", "Needle Cat cat caterpillar cat")], [])
    sid = "claude-tool"
    data.put(sid, "claude", [
        claude_row(sid, "user", "u0", None, "claude tool prompt"),
        claude_row(sid, "assistant", "a0", "u0", [{"type": "tool_use", "id": "c0", "name": "Bash",
            "input": {"command": "echo zxtoolsecret"}}]),
        claude_row(sid, "user", "r0", "a0", [{"type": "tool_result", "tool_use_id": "c0",
            "content": "zxtoolsecret out"}]),
        claude_row(sid, "assistant", "a1", "r0", "tool finished")], [])
    sid, rows, parent = "claude-cap", [claude_row("claude-cap", "user", "u0", None, "cap start")], "u0"
    for i in range(300):
        uid = f"m{i}"
        rows.append(claude_row(sid, "assistant" if i % 2 else "user", uid, parent, f"zxcapmark {i}"))
        parent = uid
    data.put(sid, "claude", rows, [])
    data.put("claude-broken", "claude", [
        claude_row("claude-broken", "user", "u0", None, "Unsupported synthetic history"),
        # Scalar content is unreadable (unknown kinds, duplicate
        # session_meta and missing parents are skipped).
        unreadable_claude_row("claude-broken", "u1", "u0")], [])

    def cx(sid, extra):
        data.put(sid, "codex", [codex_row("session_meta", {"id": sid, "cwd": "/synthetic/search"}), *extra], [])
    cx("codex-text", [codex_message("user", "zxneedle from Codex", 1),
                      codex_message("assistant", "Needle 猫", 2)])
    cx("codex-tool", [codex_message("user", "codex tool prompt", 1),
                      codex_row("response_item", {"type": "function_call", "name": "Bash", "call_id": "c0",
                          "arguments": json.dumps({"command": "zxtoolsecret"})}, 2),
                      codex_row("response_item", {"type": "function_call_output", "call_id": "c0",
                          "output": "zxtoolsecret out"}, 3)])
    cx("codex-word", [codex_message("user", "word check", 1),
                      codex_message("assistant", "Cat cat caterpillar CAT", 2)])
    cx("codex-long", [codex_message("user", "long a", 1),
                      codex_message("assistant", "a" * 12000 + "x", 2)])
    grok_put(root, "grok-text", [{"type": "user", "content": "zxneedle 猫", "prompt_index": 1},
                                 {"type": "assistant", "content": "Grok Needle"}])
    grok_put(root, "grok-tool", [
        {"type": "user", "content": "grok tool prompt", "prompt_index": 1},
        {"type": "assistant", "content": "calling", "tool_calls": [
            {"id": "g1", "name": "Bash", "arguments": json.dumps({"command": "zxtoolsecret"})}]},
        {"type": "tool_result", "tool_call_id": "g1", "content": "zxtoolsecret out"}])
    grok_put(root, "grok-case", [{"type": "user", "content": "Case Cat", "prompt_index": 1},
                                 {"type": "assistant", "content": "cat CAT Caterpillar"}])
    grok_put(root, "grok-unicode", [{"type": "user", "content": "边界 猫", "prompt_index": 1},
                                    {"type": "assistant", "content": "猫猫 mixed"}])
    return data


def call(opener, base, q, timeout=10, **flags):
    route = "/api/search?" + urlencode({"q": q, **{k: str(v) for k, v in flags.items()}})
    try:
        with opener.open(base + route, timeout=timeout) as resp:
            raw, code = resp.read(CAP + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(8192), err.code
    except Exception as err:
        fail(route, str(err))
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        payload = {}
    return route, code, payload, raw


def expect_ok(opener, base, q, **flags):
    route, code, payload, raw = call(opener, base, q, **flags)
    if code != 200 or not isinstance(payload, dict):
        fail(route, f"HTTP {code}", raw)
    return route, payload, raw


def hit(payload, token, raw, route):
    rows = [row for row in payload.get("results") or [] if token in str(row.get("snippet") or "")]
    if not rows:
        fail(route, f"missing snippet {token!r}", raw)
    return rows[0]


def ndjson(base, q, timeout=10, abort_after=None, **flags):
    path = "/api/search?" + urlencode({"q": q, "progress": "1", **flags})
    parsed = urlsplit(base)
    conn = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=timeout)
    try:
        conn.request("GET", path)
        resp = conn.getresponse()
        body = resp.read(abort_after or CAP + 1)
        if abort_after is not None:
            return path, resp.status, [], resp.getheader("content-type") or ""
        if len(body) > CAP:
            fail(path, "oversized NDJSON")
        packets = [json.loads(line) for line in body.splitlines() if line.strip()]
        return path, resp.status, packets, resp.getheader("content-type") or ""
    except Exception as err:
        fail(path, str(err))
    finally:
        conn.close()


def want_hits(opener, base, q, token, n, **flags):
    route, payload, raw = expect_ok(opener, base, q, **flags)
    got = hit(payload, token, raw, route).get("hits")
    if got != n:
        fail(route, f"hits={got} want {n}", raw)


def run(base, opener):
    route = "/api/sessions?force=1"
    try:
        with opener.open(base + route, timeout=10) as resp:
            raw = resp.read(CAP + 1)
            n = len(json.loads(raw).get("sessions") or [])
    except Exception as err:
        fail(route, str(err))
    if n != 12:
        fail(route, f"want 12 sessions got {n}", raw)
    route, payload, raw = expect_ok(opener, base, "zxtoolsecret")
    if payload.get("results"):
        fail(route, "tool-argument word hit text search", raw)
    route, payload, raw = expect_ok(opener, base, "zxneedle")
    if len(payload.get("results") or []) < 3:
        fail(route, f"want ≥3 provider text hits got {len(payload.get('results') or [])}", raw)
    route, payload, raw = expect_ok(opener, base, "猫")
    if not payload.get("results"):
        fail(route, "unicode 猫 missed text roles", raw)
    passed("text roles ignore tool arguments")
    want_hits(opener, base, "zxcase", "ZxCaSe", 4)
    want_hits(opener, base, "zxcase", "zxcase", 2, case="1")
    passed("case-insensitive default vs case=1")
    want_hits(opener, base, "zxcase", "ZxCaSe", 3, word="1")
    want_hits(opener, base, "zxcase", "zxcase", 1, word="1", case="1")
    passed("word=1 boundary")
    route, payload, raw = expect_ok(opener, base, "zxca.e")
    if payload.get("results"):
        fail(route, "literal dot should not match", raw)
    want_hits(opener, base, "zxca.e", "ZxCaSe", 4, regex="1")
    passed("regex valid pattern")
    expect_ok(opener, base, "(?<=cat)", regex="1")
    expect_ok(opener, base, r"(cat)\1", regex="1")
    passed("lookbehind and backreferences")
    started = time.perf_counter()
    path, code, packets, _ = ndjson(base, "(a+)+b", timeout=5, regex="1")
    if code != 200 or time.perf_counter() - started > 5:
        fail(path, f"HTTP {code} elapsed={time.perf_counter() - started:.2f}s", json.dumps(packets[-1:] or {}).encode())
    passed("catastrophic regex within 5s")
    route, payload, raw = expect_ok(opener, base, "zxcapmark")
    row = hit(payload, "zxcapmark", raw, route)
    if row.get("hits") != 200 or row.get("hits_capped") is not True:
        fail(route, f"hits={row.get('hits')} hits_capped={row.get('hits_capped')} want 200/true", raw)
    flags = (payload.get("errors"), payload.get("partial"), payload.get("incomplete"))
    if ("errors" in payload and not flags[0]) or ("partial" in payload and flags[1] is not True) \
            or ("incomplete" in payload and flags[2] is not True):
        fail(route, f"errors/partial/incomplete={flags}", raw)
    passed("hit cap 200 with partial/incomplete")
    path, code, packets, ctype = ndjson(base, "zxneedle")
    kinds = [pkt.get("type") for pkt in packets]
    final = next((pkt for pkt in reversed(packets) if pkt.get("type") == "result"), None)
    if code != 200 or "ndjson" not in ctype or "progress" not in kinds or not final:
        fail(path, f"HTTP {code} ctype={ctype} kinds={kinds}", json.dumps(packets[:3]).encode())
    if not (final.get("data") or {}).get("results"):
        fail(path, "NDJSON result packet missing results", json.dumps(final).encode())
    passed("NDJSON progress=1")
    ndjson(base, "zxneedle", abort_after=64)
    expect_ok(opener, base, "zxneedle")
    passed("early NDJSON close then query")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-search-suite-") as tmp:
        with isolated_server(build(Path(tmp)), args.binary) as (base, opener):
            run(base, opener)


if __name__ == "__main__":
    main()
