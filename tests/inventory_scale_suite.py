#!/usr/bin/env python3
"""Session-list scale suite: many sessions and gigabytes of native data must list within the read-model targets. No Chromium.

Synthetic corpus via tests/fixture_gen.py plus last user/assistant pair padding
(two files ≥ 50 MiB, the remaining bytes spread over up to 128 files), isolated
loopback server, no browser. Full mode (1500 sessions / 1 GiB) pins the
docs/read-model.md targets: cold ≤ 2 s, warm p50 ≤ 100 ms, RSS ≤ 300 MB after
the list and ≤ 512 MB after opening 20 unpadded sessions, stable sig across
opens; the two giants open with window=1 in ≤ 2 s and their RSS is reported.
--quick (300 sessions / 64 MiB) checks mechanics only.
"""
from __future__ import annotations

import argparse, json, subprocess, sys, tempfile, time
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote
from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, encoded, isolated_server

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
MIB, CAP, GEN = 1024 * 1024, 16 * 1024 * 1024, REPO / "tests/fixture_gen.py"
SRCS = ("claude", "codex", "grok")


def rss(pid):
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith("VmRSS:"):
            return int(line.split()[1]) * 1024
    raise SystemExit("FAIL rss: VmRSS missing")


def child_pid():
    pids = []
    for task in Path("/proc/self/task").iterdir():
        try:
            pids.extend((task / "children").read_text().split())
        except OSError:
            pass
    for pid in pids:
        try:
            if b"sessiondock" in Path(f"/proc/{pid}/cmdline").read_bytes():
                return int(pid)
        except OSError:
            pass
    raise SystemExit("FAIL pid: sessiondock child not found")


def fetch(opener, url, timeout=60):
    t0 = time.perf_counter()
    try:
        with opener.open(url, timeout=timeout) as resp:
            raw, code = resp.read(CAP + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(8192), err.code
    dt = time.perf_counter() - t0
    try:
        data = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        data = {}
    return data, dt, code


def role_of(obj):
    if obj.get("type") in ("user", "assistant"):
        return obj["type"]
    return (obj.get("payload") or {}).get("role")


def tail_objs(path, window=4 * 1024 * 1024):
    # A padded pair is two ~33 KiB lines; a 64 KiB tail would hold at most one
    # complete record on the second pad pass, so read a wider tail.
    size = path.stat().st_size
    with path.open("rb") as fh:
        fh.seek(max(0, size - window))
        data = fh.read()
    lines = data.splitlines()
    if size > window and lines:
        lines = lines[1:]
    out = []
    for line in lines:
        try:
            out.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    return out


def inflate(obj):
    """Large text so byte targets stay under the 50k-row / 100k-LF scan budgets."""
    blob = "P" * 32768
    msg = obj.get("message")
    if isinstance(msg, dict) and isinstance(msg.get("content"), str):
        msg["content"] = blob
        return
    content = (obj.get("payload") or {}).get("content")
    if isinstance(content, list) and content and isinstance(content[0], dict):
        if isinstance(content[0].get("text"), str):
            content[0]["text"] = blob
            return
    if isinstance(obj.get("content"), str):
        obj["content"] = blob


def pad(path, want):
    if path.stat().st_size >= want:
        return
    objs = tail_objs(path)
    users = [o for o in objs if role_of(o) == "user"]
    assts = [o for o in objs if role_of(o) == "assistant"]
    if not users or not assts:
        raise SystemExit(f"FAIL pad: no user/assistant pair in {path}")
    user, asst = json.loads(json.dumps(users[-1])), json.loads(json.dumps(assts[-1]))
    inflate(user)
    inflate(asst)
    parent, n, size = asst.get("uuid") or "", 0, path.stat().st_size
    with path.open("ab") as fh:
        while size < want:
            n += 1
            if "uuid" in user:
                uid, aid = f"u{n:015d}", f"a{n:015d}"
                user["uuid"], user["parentUuid"] = uid, parent
                asst["uuid"], asst["parentUuid"] = aid, uid
                parent = aid
            if isinstance(user.get("ordinal"), int):
                user["ordinal"] += 2
                asst["ordinal"] += 2
            if isinstance(user.get("prompt_index"), int):
                user["prompt_index"] += 1
                asst["prompt_index"] += 1
            blob = encoded(user) + encoded(asst)
            fh.write(blob)
            size += len(blob)


def jsonls(root):
    return [p for p in root.rglob("*.jsonl") if "subagents" not in p.parts]


def nbytes(root):
    return sum(p.stat().st_size for p in root.rglob("*") if p.is_file())


def generate(root, n, target, quick):
    q, r = divmod(n, 3)
    for i, src in enumerate(SRCS):
        k = q + (i < r)
        (root / src).mkdir(parents=True, exist_ok=True)
        if k == 0:
            continue
        subprocess.run(
            [sys.executable, str(GEN), str(root), "--sessions", str(k), "--sources", src,
             "--records", "8", "--images", "0", "--forks", "0", "--agents", "0",
             "--plain", "--force"],
            check=True, cwd=str(REPO), stdout=subprocess.DEVNULL)
    files = jsonls(root)
    picks = []
    for src in SRCS:
        group = [p for p in files if src in p.parts]
        if group:
            picks.append(sorted(group)[-1])
    if len(picks) < 2:
        picks = files[:2]
    if not picks:
        raise SystemExit("FAIL generate: no session jsonl")
    # fixture_gen holds records in memory; pad last pairs to hit size (two ≥ 50 MiB when not --quick).
    big = [] if quick else picks[:2]
    for path in big:
        pad(path, 50 * MIB)
    cur = nbytes(root)
    # Spread the remaining bytes over many other files rather than a few
    # giants: the read-model RSS target is about the sessions a user opens
    # (the real roots' newest 20 total tens of MB), and resident memory grows
    # with opened bytes by design (view LRU + AST cache), so the padded files
    # are reported separately (`rss_open_big`) instead of dominating the 20.
    spread = sorted(p for p in files if p not in big)[:128] or picks
    if cur < target and spread:
        extra = (target - cur + len(spread) - 1) // len(spread)
        for path in spread:
            pad(path, path.stat().st_size + extra)
    return n, {str(p) for p in big} | {str(p) for p in spread}


def pick20(rows, exclude=frozenset()):
    groups = {}
    for row in rows:
        if row.get("path") in exclude:
            continue
        groups.setdefault(row.get("source"), []).append(row)
    for group in groups.values():
        group.sort(key=lambda r: r.get("updated") or "", reverse=True)
    out = []
    for i in range(20):
        for group in groups.values():
            if i < len(group):
                out.append(group[i])
            if len(out) >= 20:
                return out
    return out


def row(name, value, target, ok):
    return (name, value, target, ok)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    parser.add_argument("--sessions", type=int, default=1500)
    parser.add_argument("--target-bytes", type=int, default=1024 ** 3)
    parser.add_argument("--quick", action="store_true",
                        help="300 sessions / 64 MiB; mechanics only")
    args = parser.parse_args()
    n, target = ((300, 64 * MIB) if args.quick else (args.sessions, args.target_bytes))
    with tempfile.TemporaryDirectory(prefix="sessiondock-inventory-scale-") as tmp:
        root = Path(tmp)
        n, padded = generate(root, n, target, args.quick)
        corpus = Corpus(root)
        with isolated_server(corpus, args.binary) as (base, opener):
            pid = child_pid()
            cold, t_cold, c_cold = fetch(opener, base + "/api/sessions?force=1")
            sessions = cold.get("sessions") if c_cold == 200 and isinstance(cold.get("sessions"), list) else []
            n_rows = len(sessions)
            n_ok = sum(1 for s in sessions if s.get("supported") is True)
            warm_t, warm_sig = [], []
            for _ in range(5):
                body, dt, code = fetch(opener, base + "/api/sessions")
                warm_t.append(dt)
                if code == 200:
                    warm_sig.append(body.get("sig"))
            p50 = sorted(warm_t)[len(warm_t) // 2]
            sig_ok = bool(warm_sig) and len(set(warm_sig)) == 1 and None not in warm_sig
            if c_cold == 200 and cold.get("sig") not in (None, ""):
                sig_ok = sig_ok and all(s == cold["sig"] for s in warm_sig)
            rss_list = rss(pid)
            for item in pick20(sessions, padded):
                fetch(opener, base + "/api/messages/" + quote(item["uid"], safe=":"))
            rss_open = rss(pid)
            # The padded giants (two ≥ 50 MiB when not --quick) open too; their
            # resident cost is informational, bounded by the view/AST budgets.
            bigs = sorted((s for s in sessions if s.get("path") in padded), key=lambda s: s.get("size") or 0, reverse=True)[:2]
            big_open = [fetch(opener, base + "/api/messages/" + quote(item["uid"], safe=":") + "?window=1", timeout=300) for item in bigs]
            big_ok = all(code == 200 for _, _, code in big_open)
            big_s = max((dt for _, dt, _ in big_open), default=0.0)
            rss_big = rss(pid)
            after, t_after, c_after = fetch(opener, base + "/api/sessions?force=1")
            if c_after == 200 and after.get("sig") not in (None, "") and warm_sig:
                sig_ok = sig_ok and after["sig"] == warm_sig[0]
            mb_list, mb_open = rss_list / MIB, rss_open / MIB
            checks = [
                row("cold_s", f"{t_cold:.3f}", "2.0", t_cold <= 2.0 and c_cold == 200),
                row("warm_p50_s", f"{p50:.3f}", "0.1", p50 <= 0.1),
                row("rss_list_mb", f"{mb_list:.1f}", "300", rss_list <= 300 * MIB),
                row("rss_open20_mb", f"{mb_open:.1f}", "512", rss_open <= 512 * MIB),
                row("sig_stable", "true" if sig_ok else "false", "true", sig_ok),
                row("rows", str(n_rows), str(n), n_rows == n),
                row("supported", f"{n_ok}/{n_rows}", "all", n_rows == n and n_ok == n_rows),
                row("force_after_open_s", f"{t_after:.3f}", "2.0", t_after <= 2.0 and c_after == 200),
                row("open_big_s", f"{big_s:.3f}", "2.0", big_ok and big_s <= 2.0),
                row("rss_open_big_mb", f"{rss_big / MIB:.1f}", "info", True),
            ]
            print("METRIC value target PASS/FAIL")
            hard = {"rows", "supported"} if args.quick else {c[0] for c in checks if c[0] not in ("force_after_open_s", "rss_open_big_mb")}
            failed = []
            for name, value, tgt, ok in checks:
                mark = "PASS" if ok else "FAIL"
                print(f"{name} {value} {tgt} {mark}")
                if name in hard and not ok:
                    failed.append(name)
            if failed:
                raise SystemExit(1)
            print("PASS inventory_scale_suite", flush=True)


if __name__ == "__main__":
    main()
