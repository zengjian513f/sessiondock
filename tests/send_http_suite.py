#!/usr/bin/env python3
"""HTTP-only contract of the reliable-send routes against the fake Claude and Codex CLIs (batches 31–32). No Chromium."""
import argparse, json, os, shutil, subprocess, tempfile, time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request
from history_parity import BINARY as DEBUG_BINARY, REPO, Corpus, claude_row, codex_message, codex_row, isolated_server

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PTYHOST = REPO / "target/debug/ptyhost"
PY = shutil.which("python3") or "/usr/bin/python3"
CLAUDE_SID = "c0a1b2c3-d4e5-4f67-8899-aabbccddeeff"
CODEX_SID, SWALLOW_SID = "5d1e2f3a-4b5c-4d6e-8f70-1a2b3c4d5e01", "5d1e2f3a-4b5c-4d6e-8f70-1a2b3c4d5e03"
SEND_ID, CODEX_ID, LOST_ID = "send-0001", "codex-s001", "codex-lost1"
ROUTES = ("/api/session/send", "/api/session/draft-status", "/api/session/outbox/retry", "/api/session/outbox/discard")


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:240]}")

def passed(area):
    print(f"PASS {area}", flush=True)

def call(opener, base, method, route, body=None, want=200):
    req = Request(base + route, data=None if body is None else json.dumps(body).encode(), method=method)
    if body is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with opener.open(req, timeout=20) as resp:
            raw, code = resp.read(2 * 1024 * 1024 + 1), resp.status
    except HTTPError as err:
        raw, code = err.read(4096), err.code
    except (URLError, TimeoutError, OSError) as err:
        fail(route, str(err))
    if code != want:
        fail(route, f"HTTP {code} (want {want})", raw)
    try:
        return (json.loads(raw) if raw else {}), raw
    except json.JSONDecodeError:
        fail(route, "response is not JSON", raw)

def until(opener, base, method, route, pred, area, timeout, body=None, delay=0.1):
    deadline, last, raw = time.monotonic() + timeout, {}, b""
    while time.monotonic() < deadline:
        last, raw = call(opener, base, method, route, body)
        if pred(last):
            return last
        time.sleep(delay)
    fail(area, last, raw)

def resume(opener, base, source, uid, cwd, request_id, adapter):
    rec, raw = call(opener, base, "POST", "/api/term/create",
                    {"source": source, "resume_uid": uid, "cwd": cwd, "request_id": request_id,
                     "adapter_id": adapter})
    if rec.get("running") is not True or not rec.get("name") or not rec.get("instance_id"):
        fail("term/create", rec, raw)
    until(opener, base, "GET", "/api/term/list",
          lambda d: any(r.get("uid") == uid and r.get("instance_id") == rec["instance_id"]
                        for r in d.get("sessions") or []), "term/list", 15, delay=0.05)
    until(opener, base, "POST", "/api/session/draft-status",
          lambda d: d.get("ok") is True and d.get("draft_state") == "empty",
          "draft-status", 15, {"uid": uid, "name": rec["name"]})
    return rec

def leftovers(root):
    needle, pids = str(root).encode(), []
    for pid in Path("/proc").iterdir():
        try:
            if pid.name.isdigit() and needle in (pid / "cmdline").read_bytes() + (pid / "environ").read_bytes():
                pids.append(pid.name)
        except OSError:
            pass
    return pids

def run(opener, base, claude_uid, claude_cwd, codex_uid, swallow_uid, codex_cwd):
    created = []
    meta, raw = call(opener, base, "GET", "/api/meta")
    build = meta.get("build")
    if (meta.get("capabilities") or {}).get("outbox") is not True or not build:
        fail("capability", "outbox/build", raw)
    passed("capability")

    err, raw = call(opener, base, "POST", "/api/session/send", {}, want=409)
    if err.get("code") != "stale_build":
        fail("bad bodies", "empty body", raw)
    call(opener, base, "POST", "/api/session/send", {"uid": claude_uid, "nope": 1, "_build": build}, want=400)
    stale = {"uid": claude_uid, "name": "x", "text": "hi", "request_id": "r1", "_build": "stale"}
    err, raw = call(opener, base, "POST", "/api/session/send", stale, want=409)
    if err.get("code") != "stale_build" or err.get("reload") is not True:
        fail("bad bodies", err.get("code"), raw)
    passed("bad bodies")

    err, raw = call(opener, base, "POST", "/api/session/send",  # docs: 409 terminal_unlinked
                    {**stale, "request_id": "send-none1", "_build": build}, want=409)
    if err.get("code") != "terminal_unlinked":
        fail("no instance", err.get("code"), raw)
    passed("no instance")

    rec = resume(opener, base, "claude", claude_uid, claude_cwd, "resume-claude", "claude-cli-v1")
    created.append(rec)
    body = {"uid": claude_uid, "name": rec["name"], "text": "hello-1", "media": [],
            "request_id": SEND_ID, "_build": build}
    reply, raw = call(opener, base, "POST", "/api/session/send", body)
    item = reply.get("item") or {}
    if (reply.get("ok") is not True or item.get("state") not in ("persisted", "ambiguous")
            or not isinstance(reply.get("outbox"), list)):
        fail("claude confirmed", reply, raw)
    until(opener, base, "GET", f"/api/session/outbox?uid={claude_uid}",
          lambda d: not any(r.get("id") == SEND_ID for r in d.get("outbox") or []), "outbox", 20, delay=0.15)
    msgs, raw = call(opener, base, "GET", f"/api/messages/{claude_uid}")
    if "hello-1" not in [m.get("text") for m in msgs.get("messages") or [] if m.get("role") == "user"]:
        fail("claude confirmed", "hello-1 missing", raw)
    passed("claude confirmed")

    replay, raw = call(opener, base, "POST", "/api/session/send", body)
    if (replay.get("item") or {}).get("state") != "confirmed" or "text" in (replay.get("item") or {}):
        fail("replay", replay, raw)
    conflict, raw = call(opener, base, "POST", "/api/session/send", {**body, "text": "other-text"}, want=400)
    if conflict.get("code") != "request_conflict":
        fail("replay", conflict.get("code"), raw)
    passed("replay")

    draft, raw = call(opener, base, "POST", "/api/session/draft-status",
                      {"uid": claude_uid, "name": rec["name"], "_build": build})
    if draft.get("ok") is not True or draft.get("draft_state") != "empty":
        fail("draft status", draft, raw)
    passed("draft status")

    media_value = [{"kind": "image", "token": "abc"}]
    media, raw = call(opener, base, "POST", "/api/session/send",
                      {**body, "request_id": "send-media1", "media": media_value})
    if media.get("ok") is not True or (media.get("item") or {}).get("media") != media_value:
        fail("media accepted", media, raw)
    until(opener, base, "GET", f"/api/session/outbox?uid={claude_uid}",
          lambda d: not any(r.get("id") == "send-media1" for r in d.get("outbox") or []),
          "media confirmation", 20, delay=0.15)
    passed("media accepted and confirmed")

    # docs: an attempted row still in the outbox is 409; a confirmed row has left it (404 like unknown);
    # retry shares send's stale-build gate.
    call(opener, base, "POST", "/api/session/outbox/retry", {"uid": claude_uid, "id": SEND_ID, "_build": build}, want=404)
    call(opener, base, "POST", "/api/session/outbox/retry", {"uid": claude_uid, "id": "send-none", "_build": build}, want=404)
    call(opener, base, "POST", "/api/session/outbox/retry", {"uid": claude_uid, "id": "send-none"}, want=409)
    call(opener, base, "POST", "/api/session/outbox/discard", {"uid": claude_uid, "id": "nope"}, want=404)
    call(opener, base, "POST", "/api/session/outbox/discard", {"uid": claude_uid, "id": SEND_ID}, want=404)
    passed("retry/discard")

    crec = resume(opener, base, "codex", codex_uid, codex_cwd, "resume-codex", "codex-cli-v1")
    created.append(crec)
    cbody = {"uid": codex_uid, "name": crec["name"], "text": "hello-codex-1", "media": [],
             "request_id": CODEX_ID, "_build": build}
    reply, raw = call(opener, base, "POST", "/api/session/send", cbody)
    if reply.get("ok") is not True:
        fail("codex confirmed", reply, raw)
    until(opener, base, "GET", f"/api/session/outbox?uid={codex_uid}",
          lambda d: not any(r.get("id") == CODEX_ID for r in d.get("outbox") or []), "outbox", 20, delay=0.15)
    msgs, raw = call(opener, base, "GET", f"/api/messages/{codex_uid}")
    if "hello-codex-1" not in [m.get("text") for m in msgs.get("messages") or [] if m.get("role") == "user"]:
        fail("codex confirmed", "hello-codex-1 missing", raw)
    replay, raw = call(opener, base, "POST", "/api/session/send", cbody)
    if (replay.get("item") or {}).get("state") != "confirmed":
        fail("codex confirmed", replay, raw)
    call(opener, base, "POST", "/api/session/outbox/retry",
         {"uid": codex_uid, "id": CODEX_ID, "_build": build}, want=404)
    discard, raw = call(opener, base, "POST", "/api/session/outbox/discard", {"uid": codex_uid, "id": "nope"})
    if discard.get("ok") is not True:
        fail("codex confirmed", discard, raw)
    passed("codex confirmed")

    srec = resume(opener, base, "codex", swallow_uid, codex_cwd, "resume-swallow", "codex-swallow-v1")
    created.append(srec)
    call(opener, base, "POST", "/api/session/send",
         {"uid": swallow_uid, "name": srec["name"], "text": "lost in the tui", "media": [],
          "request_id": LOST_ID, "_build": build})
    until(opener, base, "GET", f"/api/session/outbox?uid={swallow_uid}",
          lambda d: (lambda row: row.get("state") == "failed" and row.get("attempts") == 1 and row.get("error"))(
              next((r for r in d.get("outbox") or [] if r.get("id") == LOST_ID), {})),
          "swallowed input", 10, delay=0.15)
    call(opener, base, "POST", "/api/session/outbox/retry",
         {"uid": swallow_uid, "id": LOST_ID, "_build": build}, want=409)
    discard, raw = call(opener, base, "POST", "/api/session/outbox/discard", {"uid": swallow_uid, "id": LOST_ID})
    if discard.get("ok") is not True:
        fail("swallowed input", discard, raw)
    snap, raw = call(opener, base, "GET", f"/api/session/outbox?uid={swallow_uid}")
    if any(r.get("id") == LOST_ID for r in snap.get("outbox") or []):
        fail("swallowed input", "row still visible", raw)
    passed("swallowed input")
    for rec in created:
        try:
            call(opener, base, "POST", "/api/term/kill",
                 {"record_id": rec["record_id"], "instance_id": rec["instance_id"]})
        except SystemExit:
            pass
    return 10


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    if os.name != "posix" or not PTYHOST.is_file():
        why = "POSIX required" if os.name != "posix" else "target/debug/ptyhost is not built"
        print(f"SKIP send_http_suite: {why}", flush=True)
        return
    with tempfile.TemporaryDirectory(prefix="sessiondock-send-http-") as tmp:
        root = Path(tmp)
        for name in ("host", "work", "work/claude-area", "work/codex-area", "ledger", "delivery",
                     "bin", "home", "claude", "codex", "grok"):
            (root / name).mkdir(mode=0o700, parents=True, exist_ok=True); (root / name).chmod(0o700)
        claude_cwd, codex_cwd = str(root / "work/claude-area"), str(root / "work/codex-area")
        corpus = Corpus(root)
        corpus.put(CLAUDE_SID, "claude", [
            claude_row(CLAUDE_SID, "user", "u0", None, "seed claude", cwd=claude_cwd),
            claude_row(CLAUDE_SID, "assistant", "a0", "u0", "seed answer", cwd=claude_cwd)], [])
        for sid, prompt in ((CODEX_SID, "seed codex"), (SWALLOW_SID, "seed swallow")):
            corpus.put(sid, "codex", [codex_row("session_meta", {"id": sid, "cwd": codex_cwd}),
                                      codex_message("user", prompt)], [])
        claude_uid, codex_uid, swallow_uid = corpus.uid(CLAUDE_SID), corpus.uid(CODEX_SID), corpus.uid(SWALLOW_SID)
        for name, script in (("fake-claude", "fake_claude_cli.py"), ("fake-codex", "fake_codex_cli.py")):
            path = root / "bin" / name
            if name == "fake-codex":
                path.write_text(
                    f'#!/bin/sh\ncase " $* " in *" {SWALLOW_SID} "*) '
                    f'exec {PY} {REPO / "tests" / script} --swallow 1 "$@" ;; '
                    f'*) exec {PY} {REPO / "tests" / script} --reply "$@" ;; esac\n')
            else:
                path.write_text(f"#!/bin/sh\nexec {PY} {REPO / 'tests' / script} \"$@\"\n")
            path.chmod(0o700)
        shared = {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TERM": "xterm-256color", "LANG": "C.UTF-8"}
        env_c = {**shared, "SESSIONDOCK_TEST_CLAUDE_ROOT": str(root / "claude")}
        env_x = {**shared, "SESSIONDOCK_TEST_CODEX_ROOT": str(root / "codex")}
        def prof(pid, source, exe, args, rargs, env, cwd):
            return {"id": pid, "source": source, "executable": str(root / "bin" / exe), "args": args,
                    "new_args": ["--session-id", "{session_id}"] if source == "claude" else [],
                    "resume_args": rargs, "env": env}
        cfg = root / "launcher.json"
        cfg.touch(mode=0o600)
        cfg.write_text(json.dumps({
            "schema": 2, "host_binary": str(PTYHOST.resolve()), "host_dir": str(root / "host"),
            "adapters": [], "profiles": [
                prof("claude-cli-v1", "claude", "fake-claude", ["--settings", "/synthetic/bridge-settings.json"],
                     ["--resume", "{sid}"], env_c, claude_cwd),
                prof("codex-cli-v1", "codex", "fake-codex", [], ["resume", "{sid}"], env_x, codex_cwd)]}))
        cfg.chmod(0o600)
        for flag, directory in (("--initialize-lifecycle", root / "ledger"),
                                ("--initialize-delivery", root / "delivery")):
            init = subprocess.run([str(args.binary), flag, str(directory)], cwd=REPO,
                                  env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
            if init.returncode:
                fail(flag, init.stderr.decode() or init.stdout.decode())
        with isolated_server(corpus, args.binary, host_dir=root / "host") as (base, opener):
            meta, raw = call(opener, base, "GET", "/api/meta")
            if (meta.get("capabilities") or {}).get("outbox") is not False:
                fail("capability", "outbox without delivery", raw)
            for route in ROUTES:
                err, raw = call(opener, base, "POST", route, {"uid": "claude:x", "name": "n"}, want=501)
                if err.get("code") != "delivery_send_disabled":
                    fail("capability", f"{route} {err.get('code')}", raw)
        with isolated_server(corpus, args.binary, host_dir=root / "host", lifecycle_dir=root / "ledger",
                             launcher_config=cfg, delivery_dir=root / "delivery") as (base, opener):
            n = run(opener, base, claude_uid, claude_cwd, codex_uid, swallow_uid, codex_cwd)
        # The server's exit SIGHUPs the ptyhost children; under a loaded parallel
        # sweep they can take a beat to die, so poll rather than check once.
        deadline = time.monotonic() + 5.0
        alive = leftovers(root)
        while alive and time.monotonic() < deadline:
            time.sleep(0.1)
            alive = leftovers(root)
        if alive:
            fail("cleanup", f"fake CLI still alive pids={alive}")
        print(f"PASS send_http_suite: {n} scenarios", flush=True)


if __name__ == "__main__":
    main()
