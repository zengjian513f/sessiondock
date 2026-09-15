#!/usr/bin/env python3
"""Operator-run poll-path benchmark against real CLI histories (read-only).

One sessiondock binary on an isolated loopback port: the given native read
roots (and optional Codex name index) are read only; state, host and
lifecycle directories are scratch; `--hosts` free-shell ptyhost instances are
created through `/api/term/create` so `/api/live` and `/api/term/list` have
managed hosts to observe. Reports medians of 5 (urllib + `perf_counter`) for
`/api/sessions`, `/api/sessions?sig=`, `/api/live`, `/api/term/list`, eight
concurrent lists, the `force=1` paths, the browser cadence (a call every
3.1 s / 8.5 s, i.e. after the source TTLs) and VmRSS. Run it once per binary
(`--label before` / `--label after`) to compare. Requires
--i-understand-this-reads-real-histories. Never point it at a deployment's
directories.
"""
# run_validation: skip
import argparse, json, os, socket, statistics, subprocess, sys, tempfile, threading, time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, Request, build_opener

REPO = Path(__file__).resolve().parents[1]
SOURCES = ("claude", "codex", "grok")
SHELL = ('stty -echo 2>/dev/null; printf "RS_SHELL_READY\\n"; '
         'while IFS= read -r c; do case "$c" in quit) exit 0 ;; *) printf "RS_UNKNOWN\\n" ;; esac; done')
op = build_opener(ProxyHandler({}))


def die(msg, code=1):
    print(msg, file=sys.stderr, flush=True)
    raise SystemExit(code)


def get(base, path, timeout=60):
    t = time.perf_counter()
    try:
        with op.open(base + path, timeout=timeout) as r:
            body = r.read(); code = r.status
    except HTTPError as e:
        body, code = e.read(), e.code
    return time.perf_counter() - t, code, body


def post(base, path, payload):
    req = Request(base + path, data=json.dumps(payload).encode(), method="POST",
                  headers={"Content-Type": "application/json"})
    try:
        with op.open(req, timeout=60) as r:
            return r.status, json.loads(r.read() or b"{}")
    except HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def vmrss_mb(pid):
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith("VmRSS:"):
            return int(line.split()[1]) // 1024
    return -1


def timed(base, path, n=5, gap=0.0):
    xs, codes, size = [], set(), 0
    for _ in range(n):
        if gap:
            time.sleep(gap)
        dt, code, body = get(base, path)
        xs.append(dt); codes.add(code); size = len(body)
    return statistics.median(xs) * 1000, min(xs) * 1000, sorted(codes), size


def burst(base, path, k=8):
    res = [None] * k

    def one(i):
        res[i] = get(base, path)
    t = time.perf_counter()
    threads = [threading.Thread(target=one, args=(i,)) for i in range(k)]
    [x.start() for x in threads]; [x.join() for x in threads]
    return time.perf_counter() - t, sorted(str(r[1]) for r in res)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__)
    for src in SOURCES:
        ap.add_argument(f"--{src}-root", type=Path)
    ap.add_argument("--codex-index", type=Path, help="Codex session_index.jsonl (read-only)")
    ap.add_argument("--registry", type=Path,
                    help="a debug-runs.json whose read-only copy hides registered runs like the deployment")
    ap.add_argument("--binary", type=Path, default=REPO / "target/release/sessiondock")
    ap.add_argument("--ptyhost", type=Path, default=REPO / "target/release/ptyhost")
    ap.add_argument("--hosts", type=int, default=26)
    ap.add_argument("--label", default="")
    ap.add_argument("--json", type=Path, dest="json_out")
    ap.add_argument("--i-understand-this-reads-real-histories", action="store_true", dest="consent")
    a = ap.parse_args(argv)
    roots = {s: getattr(a, s + "_root") for s in SOURCES if getattr(a, s + "_root")}
    if not roots:
        die("need at least one of --claude-root / --codex-root / --grok-root")
    if not a.consent:
        die("pass --i-understand-this-reads-real-histories", 2)
    if a.hosts < 0:
        die("--hosts must be >= 0")
    binary = a.binary.resolve(strict=True)
    ptyhost = a.ptyhost.resolve(strict=True)
    resolved = {s: p.resolve(strict=True) for s, p in roots.items()}
    out = {"label": a.label, "binary": str(binary), "hosts": a.hosts}
    with tempfile.TemporaryDirectory(prefix="sessiondock-bench-polls-") as tmp:
        root = Path(tmp)
        for name in ("host", "work", "ledger", "state"):
            (root / name).mkdir(mode=0o700)
        if a.registry:
            target = root / "state" / "debug-runs.json"
            target.write_bytes(a.registry.read_bytes()); target.chmod(0o600)
        cfg = root / "launcher.json"
        cfg.touch(mode=0o600)
        cfg.write_text(json.dumps({
            "schema": 2, "host_binary": str(ptyhost), "host_dir": str(root / "host"),
            "adapters": [{"id": "synthetic-shell-v1", "source": "codex",
                          "executable": "/bin/sh", "args": ["-c", SHELL],
                          "env": {"PATH": "/usr/bin:/bin", "TERM": "xterm-256color"}}],
            "profiles": []}))
        init = subprocess.run([str(binary), "--initialize-lifecycle", str(root / "ledger")],
                              cwd=REPO, env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        if init.returncode:
            die("initialize-lifecycle: " + (init.stderr.decode() or init.stdout.decode()))
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]
        env = {k: v for k, v in os.environ.items() if not k.startswith("SESSIONDOCK_")}
        env.update({"SESSIONDOCK_BIND": f"127.0.0.1:{port}", "SESSIONDOCK_WEB_DIR": str(REPO / "legacy-web"),
                    "SESSIONDOCK_PROC_SCAN": "1",
                    "SESSIONDOCK_STATE_DIR": str(root / "state"), "SESSIONDOCK_PTYHOST_DIR": str(root / "host"),
                    "SESSIONDOCK_LIFECYCLE_DIR": str(root / "ledger"), "SESSIONDOCK_LAUNCHER_CONFIG": str(cfg)})
        if a.codex_index:
            env["SESSIONDOCK_CODEX_INDEX"] = str(a.codex_index.resolve(strict=True))
        for src, path in resolved.items():
            env["SESSIONDOCK_" + src.upper() + "_ROOT"] = str(path)
        base = f"http://127.0.0.1:{port}"
        created = []
        with open(root / "server.log", "wb") as log:
            proc = subprocess.Popen([str(binary)], cwd=REPO, env=env, stdout=log, stderr=log)
            try:
                for _ in range(200):
                    if proc.poll() is not None:
                        die(f"server exited early: {(root / 'server.log').read_bytes()[-2000:]!r}")
                    try:
                        with op.open(base + "/api/health", timeout=1) as r:
                            r.read(); break
                    except (OSError, URLError):
                        time.sleep(0.05)
                else:
                    die("health check timed out")
                out["rss_start_mb"] = vmrss_mb(proc.pid)
                dt, code, body = get(base, "/api/sessions?force=1")
                rows = json.loads(body)["sessions"]
                out["rows"] = len(rows); out["list_bytes"] = len(body); out["cold_list_ms"] = dt * 1000
                print(f"[{a.label}] rows={len(rows)} list={len(body)/1024:.0f}KB cold={dt*1000:.1f}ms "
                      f"rss={out['rss_start_mb']}MB", flush=True)
                for i in range(a.hosts):
                    code, rec = post(base, "/api/term/create", {"source": "codex", "cwd": str(root / "work"),
                                                                 "request_id": f"bench-host-{i:03d}"})
                    if code != 200 or not rec.get("record_id"):
                        die(f"term/create {code} {rec}")
                    created.append(rec)
                deadline = time.time() + 60
                while time.time() < deadline:
                    _, _, body = get(base, "/api/term/list?force=1")
                    if sum(1 for p in json.loads(body).get("pending", []) if p.get("running")) >= a.hosts:
                        break
                    time.sleep(0.2)
                _, _, body = get(base, "/api/term/list?force=1")
                tl = json.loads(body)
                out["pending_running"] = sum(1 for p in tl.get("pending", []) if p.get("running"))
                out["hosts_seen"] = len(tl.get("hosts", []))
                print(f"[{a.label}] hosts running={out['pending_running']} hosts={out['hosts_seen']}", flush=True)
                time.sleep(3.5)
                out["rss_after_hosts_mb"] = vmrss_mb(proc.pid)
                for path in ("/api/sessions", "/api/live", "/api/term/list"):
                    for _ in range(3):
                        get(base, path)
                _, _, body = get(base, "/api/sessions")
                sig = json.loads(body)["sig"]
                results = {
                    "sessions_full_hot": timed(base, "/api/sessions"),
                    "sessions_sig_hot": timed(base, f"/api/sessions?sig={sig}"),
                    "live_hot": timed(base, "/api/live"),
                    "term_list_hot": timed(base, "/api/term/list"),
                }
                walls, codes = [], []
                for _ in range(5):
                    w, codes = burst(base, "/api/sessions")
                    walls.append(w)
                results["sessions_8x_wall"] = (statistics.median(walls) * 1000, min(walls) * 1000, codes, 0)
                results["sessions_force"] = timed(base, "/api/sessions?force=1")
                results["live_force"] = timed(base, "/api/live?force=1")
                results["term_list_force"] = timed(base, "/api/term/list?force=1")
                results["live_cadence_3s"] = timed(base, "/api/live", gap=3.1)
                results["term_list_cadence_3s"] = timed(base, "/api/term/list", gap=3.1)
                results["sessions_sig_cadence_8s"] = timed(base, f"/api/sessions?sig={sig}", n=3, gap=8.5)
                out["results"] = {k: {"median_ms": v[0], "min_ms": v[1], "codes": v[2], "bytes": v[3]}
                                  for k, v in results.items()}
                out["rss_end_mb"] = vmrss_mb(proc.pid)
                for k, v in results.items():
                    print(f"[{a.label}] {k:<26} median {v[0]:8.2f} ms  min {v[1]:8.2f} ms  {v[2]} {v[3]/1024:.0f}KB",
                          flush=True)
                print(f"[{a.label}] rss start={out['rss_start_mb']}MB after-hosts={out['rss_after_hosts_mb']}MB "
                      f"end={out['rss_end_mb']}MB", flush=True)
            finally:
                for rec in created:
                    post(base, "/api/term/kill", {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})
                proc.terminate()
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proc.kill(); proc.wait(timeout=5)
        # The kill above stops every instance this run created; any survivor
        # would still hold this scratch host dir on its command line.
        subprocess.run(["pkill", "-f", str(root / "host")], check=False)
    if a.json_out:
        a.json_out.write_text(json.dumps(out, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
