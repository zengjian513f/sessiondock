#!/usr/bin/env python3
"""HTTP-only contract of POST /api/bug-report and the bug-report attachment upload against the fake Claude and Codex CLIs. No Chromium, no model binary."""
from __future__ import annotations
import argparse, json, os, shutil, socket, subprocess, tempfile, time
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, Request, build_opener
from history_parity import BINARY as DEBUG_BINARY, REPO

RELEASE = REPO / "target/release" / DEBUG_BINARY.name
BINARY = RELEASE if RELEASE.is_file() else DEBUG_BINARY
PTYHOST = REPO / "target/debug/ptyhost"
PY = shutil.which("python3") or "/usr/bin/python3"
FINAL = ("submitted", "submitted_unconfirmed", "failed")
BUNDLE_FILES = ("description.md", "browser-state.json", "events.jsonl", "environment.json",
                "worker-prompt.md", "manifest.json")


def fail(area, why, body=b""):
    text = body.decode("utf-8", "replace") if isinstance(body, (bytes, bytearray)) else str(body)
    raise SystemExit(f"FAIL {area}: {why}; {text[:400]}")


def passed(area):
    print(f"PASS {area}", flush=True)


def call(opener, base, method, route, body=None, want=200, raw=None, content_type="application/json"):
    data = raw if raw is not None else (None if body is None else json.dumps(body).encode())
    req = Request(base + route, data=data, method=method)
    if data is not None:
        req.add_header("Content-Type", content_type)
    try:
        with opener.open(req, timeout=20) as resp:
            payload, code = resp.read(2 * 1024 * 1024 + 1), resp.status
    except HTTPError as err:
        payload, code = err.read(4096), err.code
    except (URLError, TimeoutError, OSError) as err:
        fail(route, str(err))
    if want is not None and code != want:
        fail(route, f"HTTP {code} (want {want})", payload)
    try:
        return (json.loads(payload) if payload else {}), payload
    except json.JSONDecodeError:
        fail(route, "response is not JSON", payload)


@contextmanager
def server(binary, root, extra):
    """One isolated Rust server; unlike history_parity.isolated_server it takes
    the bug-report variables."""
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    env = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
    env.update(SESSIONDOCK_BIND=f"127.0.0.1:{port}", SESSIONDOCK_WEB_DIR=str(REPO / "legacy-web"),
               SESSIONDOCK_CLAUDE_ROOT=str(root / "claude"), SESSIONDOCK_CODEX_ROOT=str(root / "codex"),
               SESSIONDOCK_GROK_ROOT=str(root / "grok"), SESSIONDOCK_PTYHOST_DIR=str(root / "host"),
               SESSIONDOCK_STATE_DIR=str(root / "state"), SESSIONDOCK_LIFECYCLE_DIR=str(root / "ledger"), SESSIONDOCK_AUDIT_DIR=str(root / "audit"),
               SESSIONDOCK_FILE_ROOTS=str(root / "work"), SESSIONDOCK_FILE_WRITE_ROOTS=str(root / "work"))
    (root / "state").mkdir(mode=0o700,exist_ok=True)
    env.update(extra)
    opener = build_opener(ProxyHandler({}))
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryFile(mode="w+b") as log:
        process = subprocess.Popen([str(binary)], cwd=REPO, env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 10
            while True:
                if process.poll() is not None:
                    log.seek(0)
                    fail("server", f"exited {process.returncode}", log.read(600))
                try:
                    with opener.open(base + "/api/health", timeout=0.3):
                        break
                except (OSError, URLError):
                    pass
                if time.monotonic() > deadline:
                    fail("server", "health check timed out")
                time.sleep(0.05)
            yield base, opener
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)


def manifest(path):
    return json.loads((Path(path) / "manifest.json").read_text(encoding="utf-8"))


def wait_final(path, area, timeout=60):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        document = manifest(path)
        if document.get("status") in FINAL:
            return document
        time.sleep(0.25)
    fail(area, f"worker never finished: {manifest(path)}")


def leftovers(root):
    needle, pids = str(root).encode(), []
    for pid in Path("/proc").iterdir():
        try:
            if pid.name.isdigit() and needle in (pid / "cmdline").read_bytes() + (pid / "environ").read_bytes():
                pids.append(pid.name)
        except OSError:
            pass
    return pids


def kill(opener, base, worker):
    try:
        call(opener, base, "POST", "/api/term/kill",
             {"record_id": worker["record_id"], "instance_id": worker["instance_id"]}, want=None)
    except SystemExit:
        pass


def check_bundle(path, area, attachments=0):
    for name in BUNDLE_FILES:
        if not (Path(path) / name).is_file():
            fail(area, f"{name} missing from the bundle")
    if os.name == "posix" and (Path(path).stat().st_mode & 0o777) != 0o700:
        fail(area, "bundle directory is not 0700")
    rows = [json.loads(line) for line in (Path(path) / "events.jsonl").read_text(encoding="utf-8").splitlines()]
    if not any(row.get("event") == "bug_report.created" for row in rows):
        fail(area, "events.jsonl lacks bug_report.created", json.dumps(rows[:3]))
    if any("content" in row for row in rows):
        fail(area, "events.jsonl carries a content blob (Rust stores structured metadata only)")
    document = manifest(path)
    if document.get("schema") != 1 or document.get("event_count") != len(rows):
        fail(area, "manifest schema/event_count", json.dumps(document))
    if len(document.get("attachments") or []) != attachments:
        fail(area, "manifest attachments", json.dumps(document.get("attachments")))
    prompt = (Path(path) / "worker-prompt.md").read_text(encoding="utf-8")
    if (document["report_id"] not in prompt or "随后立即 push" not in prompt
            or "python3 deploy/deploy.py deploy --all" not in prompt
            or "无需再次确认" not in prompt or "不要 push" in prompt or "不要部署" in prompt):
        fail(area, "worker prompt is not the Rust-repository version")
    environment = json.loads((Path(path) / "environment.json").read_text(encoding="utf-8"))
    if "build" not in environment or environment.get("git_head", {}).get("argv", [None])[0] != "git":
        fail(area, "environment.json", json.dumps(environment)[:300])
    return document


def run(opener, base, root, repo):
    created = []
    meta, raw = call(opener, base, "GET", "/api/meta")
    caps = meta.get("capabilities") or {}
    if caps.get("bug_report") is not True or caps.get("terminal_create") is not True:
        fail("capability", "bug_report/terminal_create", raw)
    build = meta.get("build")
    passed("capability")

    err, raw = call(opener, base, "POST", "/api/bug-report", {"description": "x", "source": "bash"}, want=400)
    if "不支持的处理会话类型" not in err.get("error", ""):
        fail("validation", "unsupported source", raw)
    err, raw = call(opener, base, "POST", "/api/bug-report", {"description": "x", "source": "grok"}, want=503)
    if "grok" not in err.get("error", ""):
        fail("validation", "missing grok profile", raw)
    err, raw = call(opener, base, "POST", "/api/bug-report", {"description": "   ", "source": "claude"}, want=400)
    if err.get("error") != "请描述遇到的问题":
        fail("validation", "empty description", raw)
    err, raw = call(opener, base, "POST", "/api/bug-report",
                    {"description": "x" * 50001, "source": "claude"}, want=400)
    if "50000" not in err.get("error", ""):
        fail("validation", "long description", raw)
    err, raw = call(opener, base, "POST", "/api/bug-report",
                    {"description": "x", "source": "claude", "attachments": [{"path": "/etc/passwd"}]}, want=400)
    if "附件" not in err.get("error", ""):
        fail("validation", "attachment outside the repository", raw)
    err, raw = call(opener, base, "POST", "/api/bug-report",
                    {"description": "x", "source": "claude", "attachments": "nope"}, want=400)
    if "附件列表格式无效" not in err.get("error", ""):
        fail("validation", "attachment shape", raw)
    call(opener, base, "POST", "/api/bug-report", {"description": "x", "source": "claude", "cols": "wide"}, want=400)
    reports = root / "reports"
    if any(reports.iterdir()):
        fail("validation", "a rejected request left a bundle behind")
    passed("validation")

    upload, raw = call(opener, base, "POST", "/api/session/attachment?uid=bug-report&name=%E6%88%AA%E5%9B%BE.png",
                       raw=b"\x89PNG\x01\x02\x03\x04", content_type="image/png")
    if (upload.get("ok") is not True or upload.get("attachment_id") != "1" or upload.get("kind") != "image"
            or upload.get("relative_path") != "sessiondock_attachments/1/截图.png"):
        fail("attachment upload", upload, raw)
    shot = Path(upload["path"])
    if shot != repo / "sessiondock_attachments/1/截图.png" or shot.read_bytes() != b"\x89PNG\x01\x02\x03\x04":
        fail("attachment upload", f"path {shot}")
    again, raw = call(opener, base, "POST", "/api/session/attachment?uid=bug-report&name=%E6%88%AA%E5%9B%BE.png&id=1",
                      raw=b"\x89PNG\x01\x02\x03\x04", content_type="image/png")
    if again.get("reused") is not True:
        fail("attachment upload", "identical content not reused", raw)
    other, raw = call(opener, base, "POST", "/api/session/attachment?uid=bug-report&name=%E6%88%AA%E5%9B%BE.png&id=1",
                      raw=b"\x89PNG\x09", content_type="image/png")
    if other.get("name") != "截图__1.png":
        fail("attachment upload", "different content not numbered", raw)
    # Exercise a large upload through the complete HTTP path. Attachments
    # up to 512 MiB are permitted.
    large_bytes = b"attachment-body-parity\n" * (33 * 1024 * 1024 // 23 + 1)
    large, raw = call(opener, base, "POST", "/api/session/attachment?uid=bug-report&name=large.bin",
                      raw=large_bytes, content_type="application/octet-stream")
    if large.get("ok") is not True or Path(large["path"]).read_bytes() != large_bytes:
        fail("attachment upload", "large HTTP upload changed or truncated bytes", raw)
    del large_bytes
    call(opener, base, "POST", "/api/session/attachment?uid=bug-report&name=a.png&id=07",
         raw=b"x", content_type="image/png", want=400)
    call(opener, base, "POST", "/api/session/attachment?uid=bug-report&name=a.png",
         raw=b"", content_type="image/png", want=400)
    # The JSON completion route is still the file-write one for other uids.
    err, raw = call(opener, base, "POST", "/api/session/attachment",
                    {"uid": "claude:none", "ref": "x", "job": "j"}, want=None)
    if err.get("code") in ("bug_report_disabled", "bug_report_invalid"):
        fail("attachment upload", "JSON path routed to bug-report", raw)
    passed("attachment upload")

    call(opener, base, "POST", "/api/audit/browser",
         {"page_id": "page-1", "uid": "claude:none", "events": [{"event": "dom.snapshot", "ts": "t"}]}, want=202)
    reply, raw = call(opener, base, "POST", "/api/bug-report", {
        "description": "点了按钮没反应，见 [附件1]", "uid": "claude:none", "page_id": "page-1",
        "source": "claude", "terminal_name": "sessiondock-none", "snapshot": {"data": {"selected": "claude:none"}},
        "cols": 100, "rows": 30, "_build": build,
        "attachments": [{"path": str(shot), "number": 1, "name": "截图.png", "kind": "image",
                         "mime": "image/png", "size": 8, "attachment_id": "1"}]}, want=202)
    worker = reply.get("worker") or {}
    for key in ("name", "source", "sid", "cwd", "token", "title", "kind", "report_id"):
        if key not in worker:
            fail("202 shape", f"worker.{key} missing", raw)
    if (reply.get("ok") is not True or not str(reply.get("report_id", "")).startswith("BUG-")
            or worker["kind"] != "bug-report" or worker["source"] != "claude"
            or worker["title"] != f"处理 {reply['report_id']}" or worker["cwd"] != str(repo)
            or Path(reply["path"]) != reports / reply["report_id"]):
        fail("202 shape", reply, raw)
    created.append(worker)
    passed("202 shape")

    document = check_bundle(reply["path"], "bundle", attachments=1)
    if not (Path(reply["path"]) / "attachments" / "01-截图.png").is_file():
        fail("bundle", "attachment copy missing")
    if document.get("terminal_file") != "" or (Path(reply["path"]) / "terminal.txt").exists():
        fail("bundle", "an unknown terminal name must not produce terminal.txt")
    rows = [json.loads(line) for line in (Path(reply["path"]) / "events.jsonl").read_text().splitlines()]
    if not any(row.get("event") == "browser.dom.snapshot" and row.get("page_id") == "page-1" for row in rows):
        fail("bundle", "page window event missing", json.dumps(rows)[:300])
    passed("bundle")

    final = wait_final(reply["path"], "claude submitted")
    if final.get("status") != "submitted" or (final.get("injection") or {}).get("basis") != "SEND":
        fail("claude submitted", json.dumps(final, ensure_ascii=False))
    injection = final.get("injection") or {}
    assert injection.get("basis") == "SEND" and injection.get("submitted_at")
    assert "confirmed_from" not in final and "composer_cleared" not in final
    jsonl = root / "claude/project-history" / f"{worker['sid']}.jsonl"
    users = [json.loads(line) for line in jsonl.read_text(encoding="utf-8").splitlines()]
    users = [row for row in users if row.get("type") == "user"]
    prompt = (Path(reply["path"]) / "worker-prompt.md").read_text(encoding="utf-8")
    if len(users) != 1 or users[0]["message"]["content"].strip() != prompt.strip():
        fail("claude submitted", "native user record is not the prompt", json.dumps(users)[:300])
    listing, raw = call(opener, base, "GET", "/api/term/list")
    if not any(row.get("record_id") == worker["record_id"] for row in listing.get("pending") or []):
        fail("claude submitted", "worker not listed as pending", raw)
    passed("claude submitted")

    reply2, raw = call(opener, base, "POST", "/api/bug-report",
                       {"description": "codex worker", "page_id": "page-1"}, want=202)
    worker2 = reply2.get("worker") or {}
    if worker2.get("source") != "codex" or worker2.get("sid") is not None:
        fail("codex SEND without native history", "default source is codex with a pending identity", raw)
    created.append(worker2)
    check_bundle(reply2["path"], "codex SEND without native history")
    events2 = (Path(reply2["path"]) / "events.jsonl").read_text(encoding="utf-8")
    if reply["report_id"] not in events2:
        fail("codex SEND without native history", "the first report's events are not in the page window")
    final2 = wait_final(reply2["path"], "codex SEND without native history")
    # Successful SEND belongs to the CLI even without a native rollout.
    if final2.get("status") != "submitted" or (final2.get("injection") or {}).get("basis") != "SEND":
        fail("codex SEND", json.dumps(final2, ensure_ascii=False))
    if "confirmed_from" in final2:
        fail("codex SEND", "must not fabricate native confirmation")
    passed("codex SEND without native history")

    # Private attachments and diagnostics share stable report/session submission.
    owner='report:private-test'
    private,_=call(opener,base,'POST','/api/session/conversation/attachment?uid='+owner+'&id=private&name=private.bin',raw=b'private report bytes',content_type='application/octet-stream')
    assert private['upload_id']=='private' and 'path' not in private
    draft,_=call(opener,base,'POST','/api/session/conversation',{'uid':owner,'revision':0,'value':{
        'text':'private report','attachments':[{'id':'private','number':7,'file':{'name':'private.bin','size':20,'type':'application/octet-stream'},'uploaded':{**private,'uid':owner}}],'quotes':[]}})
    request={'draft_uid':owner,'draft_revision':draft['draft']['revision'],'request_id':'private-report-submit',
        'description':'private report','source':'claude','attachments':[{'upload_id':'private','number':7}]}
    before=len(list(reports.iterdir()))
    answer,_=call(opener,base,'POST','/api/bug-report',request,want=202)
    replay,_=call(opener,base,'POST','/api/bug-report',request,want=202)
    lookup,_=call(opener,base,'GET','/api/session/conversation?uid='+owner+'&report_request_id=private-report-submit')
    assert answer['report_id']==replay['report_id']==lookup['report_id']
    assert answer['worker']['record_id']==replay['worker']['record_id'] and len(list(reports.iterdir()))==before+1
    created.append(answer['worker'])
    final_private=wait_final(answer['path'],'private report SEND')
    assert final_private['status']=='submitted' and final_private['attachments'][0]['number']==7
    assert Path(final_private['attachments'][0]['path']).read_bytes()==b'private report bytes'
    retained,_=call(opener,base,'GET','/api/session/conversation?uid=tmux:'+answer['worker']['name'])
    assert not retained['draft']['value']['text']
    assert retained['draft']['value']['session']['kind']=='bug-report'
    old=draft['draft']['value']
    old.update(requestId='report-send:'+answer['report_id'],report_text='private report',report_prompt='diagnostics')
    replayed,_=call(opener,base,'POST','/api/session/conversation',{'uid':owner,'revision':retained['draft']['revision'],'value':old})
    assert replayed['draft']['revision']==retained['draft']['revision']
    assert not replayed['draft']['value']['text'] and not replayed['draft']['value']['attachments']
    assert not list((root/'state/conversations/conversation-uploads').glob('*'))
    passed('private report, stable response lookup, no duplicate bundle/launch, no sent-draft resurrection, retain session identity')

    # A blocked first task is the ordinary draft; restart updates every report reference.
    (root/'gate').write_text('Update available\n❯ 1. Update and exit\n  2. Skip\nPress Enter to continue')
    blocked_owner='report:startup-test'
    saved,_=call(opener,base,'POST','/api/session/conversation',{'uid':blocked_owner,'revision':0,'value':{'text':'preserve through update','attachments':[],'quotes':[]}})
    blocked,_=call(opener,base,'POST','/api/bug-report',{'description':'preserve through update','source':'claude','draft_uid':blocked_owner,'draft_revision':saved['draft']['revision'],'request_id':'startup-report','_build':build},want=202)
    created.append(blocked['worker'])
    failed=wait_final(blocked['path'],'update menu refusal')
    assert failed['status']=='failed' and not (root/'gate.trace').exists()
    blocked_uid='tmux:'+blocked['worker']['name']
    retained,_=call(opener,base,'GET','/api/session/conversation?uid='+blocked_uid)
    assert retained['draft']['value']['text']=='preserve through update'
    kill(opener,base,blocked['worker'])
    (root/'gate').unlink()
    restarted,_=call(opener,base,'POST','/api/session/conversation/restart',{'uid':blocked_uid,'request_id':'resume-report','_build':build})
    created.append(restarted)
    next_uid='tmux:'+restarted['name']
    retained,_=call(opener,base,'GET','/api/session/conversation?uid='+next_uid)
    value=retained['draft']['value']
    sent,_=call(opener,base,'POST','/api/session/conversation/send',{'uid':next_uid,'name':restarted['name'],'text':value['text'],'request_id':value['requestId'],'draft_revision':retained['draft']['revision'],'attachments':[],'quotes':[],'_build':build})
    assert sent['state']=='sent' and not sent['draft']['value']['text']
    resumed=wait_final(blocked['path'],'resumed first task')
    assert resumed['status']=='submitted' and resumed['error'] is None and resumed['worker']['record_id']==restarted['record_id']
    cached,_=call(opener,base,'GET','/api/session/conversation?uid='+blocked_owner+'&report_request_id=startup-report')
    assert cached['worker']['record_id']==restarted['record_id']
    users=[json.loads(line) for line in (root/'claude/project-history'/f"{restarted['sid']}.jsonl").read_text().splitlines()]
    users=[row for row in users if row.get('type')=='user']
    assert len(users)==1 and users[0]['message']['content'].strip()==(Path(blocked['path'])/'worker-prompt.md').read_text().strip()
    listing,_=call(opener,base,'GET','/api/term/list')
    assert any(row.get('record_id')==restarted['record_id'] and row.get('kind')=='bug-report' and row.get('worker_status')=='submitted' for row in listing.get('pending',[]))
    ledger=json.loads((root/'state/conversations/conversation-ledger.json').read_text())
    assert all(isinstance(row['payload'],str) and 'text' not in row['result'] for row in ledger['requests'].values())
    passed('update menu refusal, report draft restart, common first-task SEND and synchronized status')

    health, raw = call(opener, base, "GET", "/api/health")
    audit = health.get("audit") or {}
    if audit.get("written_events", 0) < 4:
        fail("audit trail", "server events not written", raw)
    segments = "".join(path.read_text(encoding="utf-8") for path in (root / "audit").glob("browser-*.jsonl"))
    for event in ("bug_report.created", "bug_report.worker_started", "bug_report.worker_submission"):
        if event not in segments:
            fail("audit trail", f"{event} missing from the audit log")
    passed("audit trail")
    for worker in created:
        kill(opener, base, worker)
    return 10


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    if os.name != "posix" or not PTYHOST.is_file():
        why = "POSIX required" if os.name != "posix" else "target/debug/ptyhost is not built"
        print(f"SKIP bug_report_http_suite: {why}", flush=True)
        return
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="sessiondock-bug-report-") as tmp:
        root = Path(tmp).resolve()
        for name in ("host", "ledger", "audit", "reports", "bin", "home", "claude", "codex", "grok"):
            (root / name).mkdir(mode=0o700)
        (root / "work").mkdir(mode=0o755)
        repo = root / "work/repo"
        repo.mkdir(mode=0o755)
        for name, script in (("fake-claude", "fake_conversation_cli.py"), ("fake-codex", "fake_codex_cli.py")):
            path = root / "bin" / name
            path.write_text(f"#!/bin/sh\nexec {PY} {REPO / 'tests' / script} \"$@\"\n")
            path.chmod(0o700)
        shared = {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TERM": "xterm-256color", "LANG": "C.UTF-8"}
        env_c = {**shared, "SESSIONDOCK_TEST_CLAUDE_ROOT": str(root / "claude"), "SESSIONDOCK_TEST_GATE": str(root / "gate"), "SESSIONDOCK_TEST_GATE_TRACE": str(root / "gate.trace")}
        env_x = {**shared, "SESSIONDOCK_TEST_CODEX_ROOT": str(root / "codex")}

        def prof(pid, source, exe, args, env):
            return {"id": pid, "source": source, "executable": str(root / "bin" / exe), "args": args,
                    "new_args": ["--session-id", "{session_id}"] if source == "claude" else [],
                    "resume_args": ["--resume", "{sid}"] if source == "claude" else ["resume", "{sid}"],
                    "env": env}

        def launcher(name, profiles):
            cfg = root / name
            cfg.touch(mode=0o600)
            cfg.write_text(json.dumps({
                "schema": 2, "host_binary": str(PTYHOST.resolve()), "host_dir": str(root / "host"),
                "adapters": [], "profiles": profiles}))
            cfg.chmod(0o600)
            return cfg
        # The worker runs the source's one configured CLI on its default
        # model, exactly what /api/term/create starts.
        good = launcher("launcher.json", [
            prof("claude-cli-v1", "claude", "fake-claude", ["--reply"], env_c),
            prof("codex-cli-v1", "codex", "fake-codex", [], env_x),
        ])
        init = subprocess.run([str(binary), "--initialize-lifecycle", str(root / "ledger")], cwd=REPO,
                              env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=15)
        if init.returncode:
            fail("--initialize-lifecycle", init.stderr.decode() or init.stdout.decode())
        bug = {"SESSIONDOCK_BUG_REPORT_DIR": str(root / "reports"), "SESSIONDOCK_BUG_REPORT_REPO": str(repo)}

        with server(binary, root, {"SESSIONDOCK_LAUNCHER_CONFIG": str(good)}) as (base, opener):
            meta, raw = call(opener, base, "GET", "/api/meta")
            if (meta.get("capabilities") or {}).get("bug_report") is not False:
                fail("unconfigured", "capability must be false without the bug-report variables", raw)
            err, raw = call(opener, base, "POST", "/api/bug-report", {"description": "x"}, want=501)
            if err.get("code") != "bug_report_disabled":
                fail("unconfigured", err.get("code"), raw)
            err, raw = call(opener, base, "POST", "/api/session/attachment?uid=bug-report&name=a.png",
                            raw=b"x", content_type="image/png", want=501)
            if err.get("code") != "bug_report_disabled":
                fail("unconfigured", err.get("code"), raw)
            call(opener, base, "GET", "/api/bug-report", want=405)
            passed("unconfigured")
        with server(binary, root, {"SESSIONDOCK_LAUNCHER_CONFIG": str(good), **bug}) as (base, opener):
            n = run(opener, base, root, repo)
        alive = leftovers(root)
        if alive:
            fail("cleanup", f"fake CLI still alive pids={alive}")
        print(f"PASS bug_report_http_suite: {n + 1} scenarios", flush=True)


if __name__ == "__main__":
    main()
