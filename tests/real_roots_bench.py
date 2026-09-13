#!/usr/bin/env python3
"""Operator-run read-model benchmark against real CLI histories (read-only).

Isolated loopback server; explicit native read roots only; never writes under them.
Requires --i-understand-this-reads-real-histories.
"""
# run_validation: skip
import argparse, json, math, os, socket, subprocess, sys, tempfile, time
from collections import Counter
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode
from urllib.request import HTTPRedirectHandler, ProxyHandler, build_opener

REPO = Path(__file__).resolve().parents[1]
SOURCES = ("claude", "codex", "grok")
MIB, OPEN_CAP, CAP = 1024 * 1024, 100 * 1024 * 1024, 64 * 1024 * 1024
PLAN = ("Operator-run read-model benchmark (docs/read-model.md). Isolated loopback "
        "server with only given read roots; never a state/host/delivery dir. "
        "Pass --i-understand-this-reads-real-histories to run.")


class NoRedirects(HTTPRedirectHandler):
    def redirect_request(self, *a):
        raise RuntimeError("redirect refused")


def die(msg, code=1):
    print(msg, file=sys.stderr, flush=True); raise SystemExit(code)


def pct(samples, frac):
    s = sorted(samples)
    return s[max(0, math.ceil(len(s) * frac) - 1)]


def vmrss(pid):
    try:
        text = Path(f"/proc/{pid}/status").read_text()
    except OSError as err:
        die(f"/proc/{pid}/status {err}")
    for line in text.splitlines():
        if line.startswith("VmRSS:"):
            n, unit = line.split(":")[1].split()
            return int(n) * 1024 if unit == "kB" else die("VmRSS unit " + unit)
    die("VmRSS missing")

def walk_stamp(roots):
    out = set()
    for root in roots:
        for dp, _dns, fns in os.walk(root, followlinks=False):
            for name in fns:
                p = os.path.join(dp, name)
                try:
                    st = os.lstat(p); out.add((p, st.st_size, st.st_mtime_ns))
                except OSError:
                    out.add((p, None, None))
    return out

def pull(opener, url, timeout=60):
    t0 = time.perf_counter()
    try:
        with opener.open(url, timeout=timeout) as resp:
            raw, code = resp.read(CAP + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(min(65536, CAP + 1)), err.code
    dt = time.perf_counter() - t0
    if len(raw) > CAP:
        die("oversized " + url)
    try:
        body = json.loads(raw) if raw else {}
    except json.JSONDecodeError:
        body = {"raw": raw[:200].decode("utf-8", "replace")}
    return dt, code, body


@contextmanager
def server(binary, roots):
    binary = binary.resolve(strict=True)
    env = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env.update(SESSIONDOCK_BIND=f"127.0.0.1:{port}", SESSIONDOCK_WEB_DIR=str(REPO / "legacy-web"))
    shown = {}
    for src, path in roots.items():
        key = "SESSIONDOCK_" + src.upper() + "_ROOT"
        env[key] = shown[key] = str(path.resolve(strict=True))
    opener, base = build_opener(ProxyHandler({}), NoRedirects()), f"http://127.0.0.1:{port}"
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen([str(binary)], cwd=REPO, env=env, stdout=log, stderr=log)
        try:
            for _ in range(150):
                if proc.poll() is not None:
                    die(f"isolated Rust server exited early ({proc.returncode})")
                try:
                    with opener.open(base + "/api/health", timeout=1) as resp:
                        resp.read(256); break
                except (OSError, URLError):
                    time.sleep(0.05)
            else:
                die("isolated Rust health check timed out")
            yield base, opener, proc, shown
        finally:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill(); proc.wait(timeout=5)


def verdict(metric, value, target, ok):
    print(f"{metric} {value} {target} {'PASS' if ok else 'FAIL'}", flush=True)
    return ok


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__)
    for src in SOURCES:
        p.add_argument(f"--{src}-root", type=Path)
    p.add_argument("--binary", type=Path, default=REPO / "target/release/sessiondock")
    p.add_argument("--open", type=int, default=20)
    p.add_argument("--rounds", type=int, default=10)
    p.add_argument("--json", type=Path, dest="json_out")
    p.add_argument("--i-understand-this-reads-real-histories", action="store_true", dest="consent")
    args = p.parse_args(argv)
    roots = {s: getattr(args, s + "_root") for s in SOURCES if getattr(args, s + "_root")}
    if not roots: die("need at least one of --claude-root / --codex-root / --grok-root")
    if args.open < 0 or args.rounds < 1: die("--open must be >= 0 and --rounds >= 1")
    if not args.consent:
        print(PLAN, flush=True)
        print("\n".join(f"  SESSIONDOCK_{s.upper()}_ROOT={path}" for s, path in roots.items()), flush=True)
        raise SystemExit(2)
    bad = [str(path) for path in roots.values() if not path.is_dir()]
    if bad: die("not a directory: " + ", ".join(bad))
    binary = args.binary if args.binary.is_absolute() else Path.cwd() / args.binary
    resolved = {s: path.resolve(strict=True) for s, path in roots.items()}
    if args.json_out and any(args.json_out.resolve().is_relative_to(path) for path in resolved.values()):
        die("--json OUT must not be under a native root")
    before = walk_stamp(resolved.values())
    with server(binary, resolved) as (base, opener, proc, env):
        print("\n".join(f"ENV {k}={v}" for k, v in env.items()), flush=True)
        cold_s, code, listed = pull(opener, base + "/api/sessions?force=1")
        if code != 200 or not isinstance(listed.get("sessions"), list):
            die(f"/api/sessions HTTP {code}")
        rows = listed["sessions"]
        n_ok = sum(1 for r in rows if r.get("supported") is not False)
        # Only unsupported rows carry migration_warnings (their fatal reason).
        kinds = Counter(str(w) for r in rows for w in (r.get("migration_warnings") or []))
        top5, sig0 = kinds.most_common(5), listed.get("sig")
        print(f"cold {cold_s:.3f}s rows={len(rows)} supported={n_ok} unsupported={len(rows) - n_ok}", flush=True)
        print("unsupported reasons " + json.dumps(top5, ensure_ascii=False), flush=True)
        warm, sig_ok = [], True
        for _ in range(args.rounds):
            dt, code, body = pull(opener, base + "/api/sessions")
            if code != 200: die(f"warm /api/sessions HTTP {code}")
            warm.append(dt); sig_ok = sig_ok and body.get("sig") == sig0
        p50, p95 = pct(warm, 0.5), pct(warm, 0.95)
        print(f"warm p50={p50:.3f}s p95={p95:.3f}s sig={'stable' if sig_ok else 'DRIFT'}", flush=True)
        rss_list = vmrss(proc.pid)
        print(f"rss_list {rss_list / MIB:.1f}MB", flush=True)
        chosen = sorted(rows, key=lambda r: r.get("updated") or "", reverse=True)[:args.open]
        opens, newest = [], None
        for r in chosen:
            uid = r.get("uid") or ""
            dt, code, body = pull(opener, base + "/api/messages/" + quote(uid, safe=":"))
            opens.append({"uid": uid, "s": round(dt, 4), "status": code,
                          "message_total": body.get("message_total"), "size": r.get("size")})
            if newest is None: newest = (uid, body)
            print(f"open {uid} {dt:.3f}s status={code} total={body.get('message_total')}", flush=True)
        for o in sorted(opens, key=lambda x: x["s"], reverse=True)[:5]:
            print(f"slow {o['uid']} {o['s']:.3f}s size={o.get('size')}", flush=True)
        rss_open = vmrss(proc.pid)
        print(f"rss_open {rss_open / MIB:.1f}MB", flush=True)
        inc = None
        if newest:
            uid, body = newest
            ver = body.get("version") or {}
            q = urlencode({"start": body.get("end"), "head": ver.get("head"), "anchor": body.get("anchor")})
            dt, code, _ib = pull(opener, base + "/api/messages/" + quote(uid, safe=":") + "?" + q)
            inc = {"uid": uid, "s": round(dt, 4), "status": code}
            print(f"incremental {uid} {dt:.3f}s status={code}", flush=True)
        final_s, code, final = pull(opener, base + "/api/sessions?force=1")
        if code != 200: die(f"final /api/sessions HTTP {code}")
        print(f"final_cold {final_s:.3f}s sig={'same' if final.get('sig') == sig0 else 'changed'}", flush=True)
    unchanged = walk_stamp(resolved.values()) == before
    print("roots unchanged" if unchanged else "FAIL roots changed", flush=True)
    small = [o for o in opens if (o.get("size") or 0) <= OPEN_CAP]
    open_s = max((o["s"] for o in small), default=0.0)
    open_ok = all(o["status"] == 200 and o["s"] <= 1.0 for o in small)
    print("METRIC value target PASS/FAIL", flush=True)
    checks = [
        verdict("cold_list", f"{cold_s:.3f}s", "1s", cold_s <= 1),
        verdict("warm_p50", f"{p50:.3f}s", "0.1s", p50 <= 0.1),
        verdict("warm_p95", f"{p95:.3f}s", "0.1s", p95 <= 0.1),
        verdict("open", f"{open_s:.3f}s", "1s", open_ok),
        verdict("rss_list", f"{rss_list / MIB:.1f}MB", "300MB", rss_list <= 300 * MIB),
        verdict("rss_open", f"{rss_open / MIB:.1f}MB", "512MB", rss_open <= 512 * MIB),
    ]
    ok = all(checks) and unchanged and sig_ok
    if args.json_out:
        payload = {"env": {k: str(v) for k, v in resolved.items()}, "cold_s": round(cold_s, 4),
                   "rows": len(rows), "supported": n_ok, "unsupported": len(rows) - n_ok,
                   "warnings_top5": top5, "warm_p50_s": round(p50, 4), "warm_p95_s": round(p95, 4),
                   "sig_stable": sig_ok, "rss_list": rss_list, "opens": opens, "rss_open": rss_open,
                   "incremental": inc, "final_cold_s": round(final_s, 4),
                   "roots_unchanged": unchanged, "pass": ok}
        args.json_out.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")
    raise SystemExit(0 if ok else 1)


if __name__ == "__main__":
    main()
