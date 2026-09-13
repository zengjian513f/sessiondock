#!/usr/bin/env python3
"""HTTP contract for debug-run registry filtering (docs/read-model.md)."""
from __future__ import annotations

import argparse, json, os, sys, tempfile, time
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote, urlencode

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import (  # noqa: E402
    BINARY as DEBUG_BINARY, Corpus, claude_row, codex_message, codex_row,
    isolated_server)

CHECKS = 0


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")


def passed(area):
    global CHECKS
    CHECKS += 1
    print(f"PASS {area}", flush=True)


def fetch(opener, base, route):
    try:
        with opener.open(base + route, timeout=15) as resp:
            return resp.status, resp.read(2 * 1024 * 1024 + 1)
    except HTTPError as err:
        return err.code, err.read(4096)


def sids_of(payload):
    return {row["sid"] for row in payload.get("sessions") or [] if isinstance(row, dict)}


def search_uids(raw):
    text = raw.decode("utf-8", "replace")
    try:
        doc = json.loads(text)
        rows = doc.get("results") or doc.get("sessions") or []
        if isinstance(rows, list):
            return {row.get("uid") for row in rows if isinstance(row, dict)}
    except json.JSONDecodeError:
        pass
    found = set()
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(row, dict) and row.get("uid"):
            found.add(row["uid"])
        for item in row.get("results") or [] if isinstance(row, dict) else []:
            if isinstance(item, dict) and item.get("uid"):
                found.add(item["uid"])
    return found


def write_registry(path, runs):
    path.write_text(json.dumps({"version": 1, "runs": runs}, ensure_ascii=False), encoding="utf-8")
    os.chmod(path, 0o600)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEBUG_BINARY)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    unique = "zxneedleAonly"
    with tempfile.TemporaryDirectory(prefix="sessiondock-debug-runs-") as tmp:
        root = Path(tmp)
        for name in ("claude", "codex", "grok"):
            (root / name).mkdir()
        corpus = Corpus(root)
        cwd_a = "/tmp/monkey-run-1/claude/session-01"
        cwd_c = "/tmp/monkey-run-1/codex/session-01"
        corpus.put("claude-A", "claude", [
            claude_row("claude-A", "user", "ua", None, unique, cwd=cwd_a,
                       timestamp="2026-09-11T12:00:00Z"),
            claude_row("claude-A", "assistant", "aa", "ua", "ok", cwd=cwd_a,
                       timestamp="2026-09-11T12:00:01Z")], [])
        corpus.put("claude-B", "claude", [
            claude_row("claude-B", "user", "ub", None, "ordinary B", cwd="/synthetic/plain",
                       timestamp="2026-09-11T12:01:00Z")], [])
        corpus.put("codex-C", "codex", [
            codex_row("session_meta", {"id": "codex-C", "session_id": "codex-C",
                                       "cwd": cwd_c, "timestamp": "2026-09-11T10:00:00Z"}),
            codex_message("user", "codex C body", 1)], [])
        corpus.put("codex-D", "codex", [
            codex_row("session_meta", {"id": "codex-D", "session_id": "codex-D",
                                       "cwd": "/synthetic/codex-plain",
                                       "timestamp": "2026-09-11T10:01:00Z"}),
            codex_message("user", "codex D body", 1)], [])
        uid_a, uid_d = corpus.uid("claude-A"), corpus.uid("codex-D")
        state = root / "state"
        state.mkdir(mode=0o700)
        registry = state / "debug-runs.json"
        runs = {
            "monkey-1": {"root": "/tmp/monkey-run-1", "created": 1787660000.0,
                         "sessions": [{"source": "claude", "cwd": cwd_a,
                                       "sid": "claude-A", "uid": uid_a}]},
            "monkey-2": {"root": "/tmp/monkey-run-2", "created": 1787660001.0,
                         "sessions": [{"source": "codex", "sid": "codex-D"}]}}
        write_registry(registry, runs)
        with isolated_server(corpus, binary, state_dir=state) as (base, opener):
            code, raw = fetch(opener, base, "/api/sessions")
            if code != 200:
                fail("default", f"HTTP {code}", raw)
            default = json.loads(raw)
            if sids_of(default) != {"claude-B"} or not isinstance(default.get("sig"), str) or not default["sig"]:
                fail("default", "want exactly {claude-B} and sig", raw)
            passed("default list hides registered runs")
            sig0 = default["sig"]
            code, raw = fetch(opener, base, "/api/sessions?debug_run=monkey-1")
            view1 = json.loads(raw) if code == 200 else {}
            if code != 200 or sids_of(view1) != {"claude-A", "codex-C"}:
                fail("monkey-1", f"HTTP {code} sids={sids_of(view1)}", raw)
            if view1.get("sig") == sig0:
                fail("monkey-1", "sig must differ from default", raw)
            passed("debug_run=monkey-1 is {A,C} with new sig")
            code, raw = fetch(opener, base, "/api/sessions?debug_run=monkey-2")
            view2 = json.loads(raw) if code == 200 else {}
            if code != 200 or sids_of(view2) != {"codex-D"}:
                fail("monkey-2", f"HTTP {code} sids={sids_of(view2)}", raw)
            passed("debug_run=monkey-2 is {D}")
            code, raw = fetch(opener, base, "/api/sessions?debug_run=no-such-run")
            missing = json.loads(raw) if code == 200 else {}
            if code != 200 or sids_of(missing):
                fail("unknown", f"HTTP {code} sids={sids_of(missing)}", raw)
            passed("unknown debug_run is empty 200")
            for label, value in (("long", "x" * 65), ("dotdot", "../x")):
                q = urlencode({"debug_run": value})
                code, raw = fetch(opener, base, "/api/sessions?" + q)
                if code not in (200, 400):
                    fail(label, f"HTTP {code}", raw)
                if code == 200 and sids_of(json.loads(raw) if raw else {}):
                    fail(label, "200 must be empty", raw)
                print(f"NOTE invalid {label!r} -> HTTP {code}", flush=True)
                passed(f"invalid id {label} is 200-empty or 400")
            code, raw = fetch(opener, base, "/api/messages/" + quote(uid_a, safe=":"))
            if code != 200:
                fail("detail", f"HTTP {code}", raw)
            passed("messages of A ignore the registry")
            q = urlencode({"q": unique, "limit": "60"})
            code, raw = fetch(opener, base, "/api/search?" + q)
            if code != 200:
                fail("search-default", f"HTTP {code}", raw)
            if uid_a in search_uids(raw):
                fail("search-default", "A must be absent from default search", raw)
            passed("default search omits A")
            code, raw = fetch(opener, base, "/api/search?" + q + "&debug_run=monkey-1")
            if code != 200:
                fail("search-run", f"HTTP {code}", raw)
            if uid_a not in search_uids(raw):
                fail("search-run", "A must appear under monkey-1", raw)
            passed("search with debug_run=monkey-1 includes A")
            time.sleep(0.05)
            write_registry(registry, {"monkey-2": runs["monkey-2"]})
            deadline = time.monotonic() + 5
            seen = None
            while time.monotonic() < deadline:
                code, raw = fetch(opener, base, "/api/sessions")
                if code == 200:
                    seen = sids_of(json.loads(raw))
                    if seen == {"claude-A", "claude-B", "codex-C"}:
                        break
                time.sleep(0.05)
            else:
                fail("reload", f"still {seen} after 5s (want A,B,C)")
            passed("registry reloads by stat without restart")
    print(f"debug_runs_suite: {CHECKS} checks passed")


if __name__ == "__main__":
    main()
