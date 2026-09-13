#!/usr/bin/env python3
"""Search-text cache contract (WP-B) over the search_suite fixtures; no Chromium.

Explicit directory and cache entries, cold/hot result identity,
append/rewrite invalidation, cached unsupported sessions, restart reuse, LRU
eviction, ordinary path aliases and permissions, warm-up without a search,
concurrent searches and a list during a search, NDJSON order, memory-only
mode with a pure state dir, `--check-config`.
"""
import argparse, http.client, json, os, stat, subprocess, sys, tempfile, threading, time
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode

sys.path.insert(0, str(Path(__file__).resolve().parent))
from history_parity import BINARY as DEBUG_BINARY, REPO, encoded, isolated_server  # noqa: E402
from search_suite import build, call, expect_ok, fail, passed  # noqa: E402
from history_parity import claude_row  # noqa: E402

BINARY = (p if (p := REPO / "target/release" / DEBUG_BINARY.name).is_file() else DEBUG_BINARY)


def entries(cache_dir):
    return sorted(p for p in cache_dir.iterdir() if p.is_file() and not p.name.startswith("."))


def header(path):
    with path.open("rb") as fh:
        return json.loads(fh.readline())


def uids(payload):
    return [row["uid"] for row in payload.get("results") or []]


def check_config(env_extra, corpus_root):
    environment = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    environment.update({"SESSIONDOCK_BIND": "127.0.0.1:0", "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web")})
    for source in ("claude", "codex", "grok"):
        environment["SESSIONDOCK_" + source.upper() + "_ROOT"] = str(corpus_root / source)
    environment.update(env_extra)
    done = subprocess.run([str(BINARY), "--check-config"], env=environment, capture_output=True, text=True, timeout=30)
    return done.returncode, done.stdout + done.stderr


def ndjson_packets(base, q, **flags):
    path = "/api/search?" + urlencode({"q": q, "progress": "1", **flags})
    host, port = base.replace("http://", "").split(":")
    conn = http.client.HTTPConnection(host, int(port), timeout=30)
    try:
        conn.request("GET", path)
        resp = conn.getresponse()
        body = resp.read(4 * 1024 * 1024)
        return resp.status, [json.loads(line) for line in body.splitlines() if line.strip()]
    finally:
        conn.close()


def run_persistent(tmp, data):
    # The state directory stays the metadata store's own: the cache gets its
    # own explicit 0700 directory beside it.
    state = data.root / "state"
    state.mkdir(mode=0o700)
    cache_dir = data.root / "search-text"
    cache_dir.mkdir(mode=0o700)
    with isolated_server(data, BINARY, state_dir=state, extra_env={"SESSIONDOCK_SEARCH_WARMUP": "0",
                                                                    "SESSIONDOCK_SEARCH_CACHE_DIR": str(cache_dir)}) as (base, opener):
        if entries(cache_dir):
            fail("cache dir", "entries before any search")
        route, cold, raw = expect_ok(opener, base, "zxneedle")
        files = entries(cache_dir)
        if len(files) != 12:
            fail(route, f"want 12 entries after the first search, got {[f.name for f in files]}")
        for path in files:
            if stat.S_IMODE(path.stat().st_mode) != 0o600:
                fail("entry mode", f"{path.name} {oct(path.stat().st_mode)}")
            head = header(path)
            if head.get("schema") != 1 or head.get("uid", "").replace(":", "-") != path.name or head.get("kind") not in ("text", "error"):
                fail("entry header", f"{path.name}: {head}")
        broken = [p for p in files if header(p)["kind"] == "error"]
        if len(broken) != 1 or header(broken[0])["status"] != 501:
            fail("error entry", f"want one cached 501 entry, got {[header(p) for p in broken]}")
        if not cold.get("partial") or len(cold.get("errors") or []) != 1:
            fail(route, f"unsupported session must stay an explicit error: {cold.get('errors')}", raw)
        passed("private cache directory, 0600 entries, one cached 501")

        before = {p.name: p.stat().st_mtime_ns for p in files}
        route, hot, raw = expect_ok(opener, base, "zxneedle")
        for key in ("results", "errors", "partial", "incomplete", "scanned", "total_pool", "truncated"):
            if cold.get(key) != hot.get(key):
                fail(route, f"cold/hot {key} differ: {cold.get(key)!r} vs {hot.get(key)!r}", raw)
        after = {p.name: p.stat().st_mtime_ns for p in entries(cache_dir)}
        if before != after:
            fail(route, "a hot search rewrote cache entries")
        for q, flags, token in (("zxcase", {"word": "1"}, "ZxCaSe"), ("zxca.e", {"regex": "1"}, "ZxCaSe"), ("猫", {}, "猫")):
            route, first, raw = expect_ok(opener, base, q, **flags)
            route, second, raw2 = expect_ok(opener, base, q, **flags)
            if first.get("results") != second.get("results"):
                fail(route, "chunked cached scan differs from the first scan", raw2)
            if not any(token in str(row.get("snippet")) for row in first["results"]):
                fail(route, f"missing {token!r}", raw)
        passed("hot search: identical results, entries untouched, word/regex/unicode over cached bodies")

        text_path = data.root / "claude" / "project-claude-text" / "claude-text.jsonl"
        if not text_path.is_file():
            text_path = next((data.root / "claude").rglob("claude-text.jsonl"))
        entry_before = {p.name: p.stat().st_mtime_ns for p in files}
        with text_path.open("ab") as fh:
            fh.write(encoded(claude_row("claude-text", "user", "u9", "a0", "zxappended fresh line")))
        route, payload, raw = expect_ok(opener, base, "zxappended")
        if len(uids(payload)) != 1:
            fail(route, "appended text not found after the file grew", raw)
        changed = [name for name, mtime in entry_before.items()
                   if (cache_dir / name).stat().st_mtime_ns != mtime]
        if len(changed) != 1:
            fail(route, f"exactly the appended session's entry must be rewritten, got {changed}")
        route, payload, raw = expect_ok(opener, base, "zxneedle")
        if len(uids(payload)) != 3:
            fail(route, "other sessions still match after one append", raw)
        passed("append invalidates one entry only")

        rows = [claude_row("claude-text", "user", "u0", None, "rewritten zxrewrite only")]
        text_path.write_bytes(b"".join(encoded(row) for row in rows))
        route, payload, raw = expect_ok(opener, base, "zxappended")
        if uids(payload):
            fail(route, "rewritten (shorter) file still served the old body", raw)
        route, payload, raw = expect_ok(opener, base, "zxrewrite")
        if len(uids(payload)) != 1:
            fail(route, "rewritten body not found", raw)
        passed("rewrite invalidates")

        # Concurrency: two searches plus a list at once — no reader_busy, no search_busy.
        results = {}

        def worker(name, route):
            try:
                with opener.open(base + route, timeout=30) as resp:
                    results[name] = resp.status
            except HTTPError as err:
                results[name] = err.code
            except Exception as err:  # noqa: BLE001
                results[name] = str(err)
        threads = [threading.Thread(target=worker, args=(f"search{i}", "/api/search?" + urlencode({"q": "zxneedle", "progress": "1"}))) for i in range(2)]
        threads.append(threading.Thread(target=worker, args=("list", "/api/sessions")))
        threads.append(threading.Thread(target=worker, args=("messages", "/api/messages/" + uids(payload)[0])))
        for th in threads:
            th.start()
        for th in threads:
            th.join()
        if any(code != 200 for code in results.values()):
            fail("concurrency", f"{results}")
        passed("two concurrent searches plus list/messages all 200")

        status, packets = ndjson_packets(base, "zx")
        done = [p["done"] for p in packets if p.get("type") == "progress"]
        if status != 200 or done != sorted(done) or done[0] != 0 or done[-1] != 12:
            fail("ndjson", f"progress {done}")
        matched = [p["results"][0]["uid"] for p in packets if p.get("type") == "matches"]
        final = [p for p in packets if p.get("type") == "result"][-1]["data"]
        with opener.open(base + "/api/sessions", timeout=10) as resp:
            pool_order = [row["uid"] for row in json.loads(resp.read())["sessions"]]
        order = {uid: index for index, uid in enumerate(pool_order)}
        if matched != sorted(matched, key=order.__getitem__):
            fail("ndjson", f"matches not in pool order: {matched}")
        if [row["uid"] for row in final["results"]] != sorted(matched, key=lambda uid: (order[uid])):
            fail("ndjson", "final result order differs from the pool order")
        passed("NDJSON progress and matches in pool order")
    # A restart with the same directories takes stock of the entries and
    # answers from them (the metadata store must still accept its own dir).
    with isolated_server(data, BINARY, state_dir=state, extra_env={"SESSIONDOCK_SEARCH_WARMUP": "0",
                                                                    "SESSIONDOCK_SEARCH_CACHE_DIR": str(cache_dir)}) as (base, opener):
        before = {p.name: p.stat().st_mtime_ns for p in entries(cache_dir)}
        route, payload, raw = expect_ok(opener, base, "zxrewrite")
        if len(uids(payload)) != 1:
            fail(route, "restart lost the cache", raw)
        if before != {p.name: p.stat().st_mtime_ns for p in entries(cache_dir)}:
            fail(route, "restart rewrote unchanged entries")
    passed("restart reuses the persisted entries; state dir untouched")
    return cache_dir


def run_cap(tmp, data):
    cache_dir = data.root / "search-text-cap"
    cache_dir.mkdir(mode=0o700)
    big_root = data.root / "claude" / "project-big"
    big_root.mkdir(parents=True, exist_ok=True)
    for index in range(4):
        sid = f"claude-big{index}"
        rows = [claude_row(sid, "user", "u0", None, f"zxbig{index} " + ("填充" * 200_000))]
        (big_root / f"{sid}.jsonl").write_bytes(b"".join(encoded(row) for row in rows))
    with isolated_server(data, BINARY, extra_env={"SESSIONDOCK_SEARCH_WARMUP": "0", "SESSIONDOCK_SEARCH_CACHE_DIR": str(cache_dir),
                                                  "SESSIONDOCK_SEARCH_CACHE_BYTES": str(2 * 1024 * 1024)}) as (base, opener):
        route, payload, raw = expect_ok(opener, base, "zxbig")
        if len(uids(payload)) != 4:
            fail(route, "all four large bodies must match regardless of the cap", raw)
        total = sum(p.stat().st_size for p in entries(cache_dir))
        if total > 2 * 1024 * 1024:
            fail("cap", f"cache holds {total} bytes over the 2 MiB cap")
        if len(entries(cache_dir)) >= 16:
            fail("cap", "nothing was evicted")
        route, again, raw = expect_ok(opener, base, "zxbig")
        if uids(again) != uids(payload):
            fail(route, "results changed after eviction", raw)
    passed("byte cap evicts least recently used entries, results unchanged")


def run_explicit_paths_and_values(tmp, data):
    explicit = tmp / "explicit-cache"
    explicit.mkdir(mode=0o700)
    with isolated_server(data, BINARY, extra_env={"SESSIONDOCK_SEARCH_CACHE_DIR": str(explicit),
                                                  "SESSIONDOCK_SEARCH_WARMUP": "0"}) as (base, opener):
        expect_ok(opener, base, "zxneedle")
        if len(entries(explicit)) != 12:
            fail("explicit dir", f"{len(entries(explicit))} entries")
    passed("explicit SESSIONDOCK_SEARCH_CACHE_DIR without a state dir")
    inside = data.root / "claude" / "cache-inside-root"
    inside.mkdir(mode=0o700)
    code, out = check_config({"SESSIONDOCK_SEARCH_CACHE_DIR": str(inside)}, data.root)
    if code != 0:
        fail("native-root cache", out)
    state = data.root / "state-overlap"
    state.mkdir(mode=0o700)
    child = state / "search-text"
    child.mkdir(mode=0o700)
    code, out = check_config({"SESSIONDOCK_SEARCH_CACHE_DIR": str(child), "SESSIONDOCK_STATE_DIR": str(state)}, data.root)
    if code != 0:
        fail("state child cache", out)
    wide = data.root / "wide-cache"
    wide.mkdir(mode=0o750)
    with isolated_server(data, BINARY, extra_env={"SESSIONDOCK_SEARCH_CACHE_DIR": str(wide),
                                                   "SESSIONDOCK_SEARCH_WARMUP": "0"}):
        pass
    code, out = check_config({"SESSIONDOCK_SEARCH_WORKERS": "0"}, data.root)
    if code != 0:
        fail("zero workers", out)
    code, out = check_config({"SESSIONDOCK_SEARCH_CACHE_BYTES": "12"}, data.root)
    if code != 0:
        fail("small cache", out)
    code, out = check_config({"SESSIONDOCK_SEARCH_CACHE_DIR": str(explicit), "SESSIONDOCK_SEARCH_WORKERS": "3",
                              "SESSIONDOCK_SEARCH_WARMUP": "7"}, data.root)
    if code != 0 or f"search_cache_dir={explicit}" not in out or "search_workers=3" not in out or "search_warmup=7" not in out:
        fail("check-config", out)
    passed("--check-config accepts cache aliases and values and echoes search settings")


def run_warmup(tmp, data):
    cache_dir = data.root / "search-text-warm"
    cache_dir.mkdir(mode=0o700)
    with isolated_server(data, BINARY, extra_env={"SESSIONDOCK_SEARCH_WARMUP": "1",
                                                  "SESSIONDOCK_SEARCH_CACHE_DIR": str(cache_dir)}) as (base, opener):
        deadline = time.time() + 15
        while time.time() < deadline and len(entries(cache_dir)) < 12:
            time.sleep(0.2)
        if len(entries(cache_dir)) < 12:
            fail("warm-up", f"{len(entries(cache_dir))} entries after 15 s without a search")
        with opener.open(base + "/api/sessions", timeout=10) as resp:
            if resp.status != 200:
                fail("warm-up", "list failed during warm-up")
        route, payload, raw = expect_ok(opener, base, "zxneedle")
        if len(uids(payload)) != 3:
            fail(route, "warm cache answers the first search", raw)
    passed("warm-up fills the cache before any search")


def run_memory_only(tmp, data):
    state = data.root / "state-memory"
    state.mkdir(mode=0o700)
    with isolated_server(data, BINARY, state_dir=state, extra_env={"SESSIONDOCK_SEARCH_WARMUP": "0"}) as (base, opener):
        route, first, raw = expect_ok(opener, base, "zxneedle")
        route, second, raw = expect_ok(opener, base, "zxneedle")
        if first.get("results") != second.get("results") or len(uids(first)) != 3:
            fail(route, "memory-only search differs between runs", raw)
        if list(tmp.rglob("search-text*")) or any(p.name != "session-metadata.json" and not p.name.startswith(".") for p in state.iterdir()):
            fail("memory-only", f"a state dir with only SESSIONDOCK_STATE_DIR must stay the metadata store's: {list(state.iterdir())}")
    passed("memory-only mode: a state dir alone persists nothing and stays pure")


def main():
    global BINARY
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    BINARY = args.binary
    with tempfile.TemporaryDirectory(prefix="sessiondock-search-cache-") as tmp:
        tmp = Path(tmp)
        corpus_root = tmp / "corpus"
        corpus_root.mkdir()
        data = build(corpus_root)
        run_memory_only(tmp, data)
        run_warmup(tmp, data)
        run_explicit_paths_and_values(tmp, data)
        # Mutates the fixtures (append, rewrite): keep it after the others.
        run_persistent(tmp, data)
        run_cap(tmp, data)


if __name__ == "__main__":
    main()
