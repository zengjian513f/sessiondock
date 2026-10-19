"""Private, synthetic Claude stream-json peer for browser integration tests.

Run through a launcher profile: python3 claude_sdk_fixtures.py --fake-cli.
The peer speaks the official SDK's NDJSON control protocol, never imports an
SDK, contacts a provider or reads the user's CLI home. The console helper uses
a private PTY with a fixed synthetic shell loop.
"""
# run_validation: skip
from __future__ import annotations

from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from urllib.request import ProxyHandler, build_opener


class MonotonicClock:
    """Test-process clock shim; production retains its fixed ten-minute idle timer."""
    def __init__(self, root):
        root = Path(root)
        self.offset = root / 'monotonic-offset'
        self.offset.write_text('0')
        source = root / 'monotonic-clock.c'
        self.library = root / 'monotonic-clock.so'
        source.write_text(r'''
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>
int clock_gettime(clockid_t clock, struct timespec *stamp) {
    static int (*native_clock)(clockid_t, struct timespec *) = NULL;
    if (!native_clock) native_clock = dlsym(RTLD_NEXT, "clock_gettime");
    int result = native_clock(clock, stamp);
    if (!result && clock == CLOCK_MONOTONIC) {
        const char *path = getenv("SESSIONDOCK_TEST_CLOCK_OFFSET");
        if (path) {
            int fd = open(path, O_RDONLY);
            if (fd >= 0) {
                char value[32] = {0};
                if (read(fd, value, sizeof(value) - 1) > 0) stamp->tv_sec += strtoll(value, NULL, 10);
                close(fd);
            }
        }
    }
    return result;
}
''')
        subprocess.run(['cc', '-shared', '-fPIC', '-O2', str(source), '-ldl', '-o', str(self.library)], check=True, timeout=15)
        self.seconds = 0

    @property
    def env(self):
        return {'LD_PRELOAD': str(self.library), 'SESSIONDOCK_TEST_CLOCK_OFFSET': str(self.offset)}

    def advance(self, seconds):
        self.seconds += seconds
        # Atomic replacement prevents clock reads from seeing an empty file.
        pending = self.offset.with_suffix('.next')
        pending.write_text(str(self.seconds))
        pending.replace(self.offset)


def project_key(cwd):
    """Native 2.1.302 JS UTF-16 naming, including non-BMP and long cwd."""
    units = [int.from_bytes(part, "little") for part in
             [str(cwd).encode("utf-16-le")[i:i + 2] for i in range(0, len(str(cwd).encode("utf-16-le")), 2)]]
    name = "".join(chr(unit) if unit < 128 and chr(unit).isalnum() else "-" for unit in units)
    if len(units) <= 200:
        return name
    number = 0
    for unit in units:
        number = (31 * number + unit) & 0xffffffff
    if number >= 0x80000000:
        number -= 0x100000000
    number = abs(number)
    suffix = ""
    while number:
        suffix = "0123456789abcdefghijklmnopqrstuvwxyz"[number % 36] + suffix
        number //= 36
    return name[:200] + "-" + (suffix or "0")


def native_uid(path):
    return "claude:" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


class ClaudeFixture:
    def __init__(self, root):
        self.root = Path(root)
        self.lifecycle_initialized = False
        self.home = self.root / "cli-home"
        self.projects = self.home / ".claude/projects"
        self.cwd = self.root / "workspace"
        self.received = self.root / "claude-native-received.jsonl"
        self.sid = "6179c0af-3c16-487d-8333-7c4ce66cf0e6"
        for directory in (self.projects, self.cwd, self.root / "host", self.root / "lifecycle", self.root / "state", self.root / "proc"):
            directory.mkdir(parents=True, exist_ok=True)
        (self.root / "proc/stat").write_text("btime 1700000000\n")
        self.path = self.projects / project_key(self.cwd) / (self.sid + ".jsonl")
        self.path.parent.mkdir()
        self.path.write_text(json.dumps({"type": "user", "uuid": "seed-u", "parentUuid": None,
            "sessionId": self.sid, "cwd": str(self.cwd), "isSidechain": False,
            "timestamp": "2026-10-18T10:00:00Z", "message": {"role": "user", "content": "earlier synthetic Claude prompt"}}) + "\n" +
            json.dumps({"type": "assistant", "uuid": "seed-a", "parentUuid": "seed-u", "sessionId": self.sid,
                "cwd": str(self.cwd), "isSidechain": False, "timestamp": "2026-10-18T10:00:01Z",
                "message": {"id": "seed-message", "role": "assistant", "model": "fake-claude", "content": [{"type": "text", "text": "earlier synthetic Claude answer"}]}}) + "\n")
        self.uid = native_uid(self.path)
        self.launcher = self.root / "launcher.json"
        self.launcher.write_text(json.dumps({"host_binary": "/bin/true", "host_dir": str(self.root / "host"),
            "profiles": [{"id": "claude-fixture", "source": "claude", "executable": sys.executable,
                "args": [str(Path(__file__).resolve()), "--fake-cli"], "env": {"HOME": str(self.home),
                    "CLAUDE_CONFIG_DIR": str(self.home / ".claude"), "SESSIONDOCK_FAKE_CLAUDE_LOG": str(self.received)},
                "env_remove": ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "LD_PRELOAD", "SESSIONDOCK_TEST_CLOCK_OFFSET"]}]}))

    def frames(self, kind=None):
        rows = [json.loads(line) for line in self.received.read_text().splitlines()] if self.received.exists() else []
        return [row for row in rows if kind is None or row.get("type") == kind]

    @contextmanager
    def server(self, binary, extra_env=None):
        from frontend_paths import frontend_dir
        from send_browser import initialize
        if not self.lifecycle_initialized:
            initialize("--initialize-lifecycle", self.root / "lifecycle", Path(binary).resolve(strict=True))
            self.lifecycle_initialized = True
        from history_fixtures import REPO
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        env = {key: value for key, value in os.environ.items() if not key.startswith("SESSIONDOCK_")}
        env.update(SESSIONDOCK_BIND=f"127.0.0.1:{port}", SESSIONDOCK_WEB_DIR=str(frontend_dir()),
            SESSIONDOCK_CLAUDE_ROOT=str(self.projects), SESSIONDOCK_LAUNCHER_CONFIG=str(self.launcher),
            SESSIONDOCK_PROC_ROOT=str(self.root / "proc"), SESSIONDOCK_PTYHOST_DIR=str(self.root / "host"),
            SESSIONDOCK_LIFECYCLE_DIR=str(self.root / "lifecycle"), SESSIONDOCK_STATE_DIR=str(self.root / "state"),
            SESSIONDOCK_CODEX_APP_SERVER="off")
        env.update(extra_env or {})
        base = f"http://127.0.0.1:{port}"
        opener = build_opener(ProxyHandler({}))
        with tempfile.TemporaryFile() as log:
            child = subprocess.Popen([str(Path(binary).resolve(strict=True))], cwd=REPO, env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    assert child.poll() is None, "isolated SessionDock exited before health"
                    try:
                        with opener.open(base + "/api/health", timeout=.5) as response:
                            if response.status == 200:
                                break
                    except OSError:
                        time.sleep(.05)
                else:
                    raise AssertionError("isolated SessionDock failed to become healthy")
                yield base
            except BaseException:
                log.seek(0)
                print(log.read().decode("utf-8", "replace"), file=sys.stderr)
                raise
            finally:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)


@contextmanager
def managed_console(fixture):
    """A real private PTY with synthetic Claude identity and fixed shell input."""
    import shutil
    from history_fixtures import REPO
    from private_hosts import private_hosts
    from spawned_by_fixtures import START, proc_pid
    from terminal_browser import SHELL_SCRIPT, stop
    metadata = {'source': 'claude', 'sid': fixture.sid, 'uid': fixture.uid,
                'instance_id': 'synthetic-' + uuid.uuid4().hex}
    environment = {key: value for key, value in os.environ.items()
                   if key in {'PATH', 'LANG', 'LC_ALL', 'LC_CTYPE'}}
    environment['TERM'] = 'xterm-256color'
    with private_hosts(fixture.root):
        process = subprocess.Popen([str(REPO / 'target/debug/ptyhost'), '--dir', str(fixture.root / 'host'),
            'run', '--name', 'claude-view-console', '--cwd', str(fixture.cwd), '--meta', json.dumps(metadata),
            '--', '/bin/sh', '-c', SHELL_SCRIPT], env=environment,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        record_path = fixture.root / 'host/claude-view-console.json'
        try:
            deadline = time.monotonic() + 5
            while not record_path.exists():
                assert process.poll() is None, 'private console exited during startup'
                assert time.monotonic() < deadline, 'private console startup timeout'
                time.sleep(.03)
            record = json.loads(record_path.read_text())
            pid = record['pid']
            START[pid] = 10000
            proc_pid(fixture.root / 'proc', pid, 'claude', ['claude', '--resume', fixture.sid], 1,
                     cwd=str(fixture.cwd), fds={3: fixture.path})
            yield
        finally:
            stop(process)
            if 'pid' in locals():
                shutil.rmtree(fixture.root / 'proc' / str(pid))


def fake_cli():
    args = sys.argv[2:]
    if "--version" in args:
        print("2.1.302 (synthetic Claude Code)")
        return
    if "update" in args:
        raise SystemExit("synthetic peer never updates real software")
    assert "--print" in args and "--input-format" in args and "--output-format" in args, args
    assert "--permission-prompt-tool=stdio" in args, args
    value = lambda flag: next((arg.split("=", 1)[1] for arg in args if arg.startswith(flag + "=")), None)
    resume = value("--resume")
    fork = "--fork-session" in args
    sid = value("--session-id") if fork else Path(resume).stem if resume else value("--session-id")
    assert sid, "explicit native session identity required"
    cwd = str(Path.cwd())
    home = Path(os.environ["CLAUDE_CONFIG_DIR"])
    path = Path(resume) if resume and not fork else home / "projects" / project_key(cwd) / (sid + ".jsonl")
    assert path.is_relative_to(home / "projects"), path
    assert not any(os.environ.get(key) for key in ("CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CODEX_THREAD_ID", "CLAUDE_CODE_SESSION_ID"))
    log = Path(os.environ["SESSIONDOCK_FAKE_CLAUDE_LOG"])
    mutex = threading.Lock()
    output_lock = threading.Lock()
    state = {"model": value("--model") or "fake-claude", "effort": value("--effort") or "low",
        "permission_mode": "default", "fast_mode": False, "completed": 0, "context": 1200, "usage_available": True}
    pending = {}
    inputs = queue.Queue()
    interrupted = threading.Event()
    previous = json.loads(path.read_text().splitlines()[-1])["uuid"] if path.is_file() else None

    def materialize_fork():
        nonlocal previous
        if not fork or path.exists(): return
        boundary = value('--resume-session-at')
        assert boundary, 'fork snapshot must freeze a native chain UUID'
        rows = [json.loads(line) for line in Path(resume).read_text().splitlines()]
        index = next(i for i, row in enumerate(rows) if row.get('uuid') == boundary)
        rows = rows[:index + 1]
        path.parent.mkdir(parents=True, exist_ok=True)
        for row in rows: row['sessionId'] = sid
        path.write_text(''.join(json.dumps(row) + '\n' for row in rows))
        previous = boundary

    def send(frame):
        with output_lock:
            print(json.dumps({**frame, "session_id": sid}), flush=True)

    def record(kind, identifier, message):
        nonlocal previous
        materialize_fork()
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a") as stream:
            stream.write(json.dumps({"type": kind, "uuid": identifier, "parentUuid": previous,
                "sessionId": sid, "cwd": cwd, "timestamp": "2026-10-18T11:00:00Z", "isSidechain": False,
                "message": message}) + "\n")
        previous = identifier

    def assistant(content, message_id):
        frame_id = str(uuid.uuid4())
        message = {"role": "assistant", "id": message_id, "model": state["model"], "content": content,
            "usage": {"input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 30, "cache_creation_input_tokens": 10}}
        record("assistant", frame_id, message)
        send({"type": "assistant", "uuid": frame_id, "message": message})

    def stream_text(content, message_id):
        send({"type": "stream_event", "event": {"type": "message_start", "message": {"id": message_id, "model": state["model"]}}})
        send({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}})
        for part in (content[:8], content[8:]):
            send({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": part}}})
        send({"type": "stream_event", "event": {"type": "content_block_stop", "index": 0}})
        assistant([{"type": "text", "text": content}], message_id)

    def request_control(request):
        key = str(uuid.uuid4())
        event = threading.Event()
        with mutex:
            pending[key] = [event, None]
        send({"type": "control_request", "request_id": key, "request": request})
        while not event.wait(.05):
            if interrupted.is_set():
                send({"type": "control_cancel_request", "request_id": key})
                with mutex:
                    pending.pop(key, None)
                return None
        with mutex:
            return pending.pop(key)[1]

    def model_usage():
        count = state["completed"]
        return {state["model"]: {"inputTokens": count * 100, "outputTokens": count * 20,
            "thinkingTokens": count * 5, "cacheReadInputTokens": count * 30,
            "cacheCreationInputTokens": count * 10, "costUSD": count * .01,
            "contextWindow": 200000, "maxOutputTokens": 32000, "webSearchRequests": 0}}

    def turns():
        while True:
            frame = inputs.get()
            interrupted.clear()
            content = frame["message"]["content"]
            if isinstance(content, list):
                assert any(block.get("type") == "image" and block["source"]["type"] == "base64" for block in content), content
                content = " ".join(block.get("text", "") for block in content if block.get("type") == "text")
            if content in ('foreign-identity-user', 'foreign-identity-approval'):
                foreign = {"type": "user", "uuid": "foreign-user", "message": {"role": "user", "content": "FOREIGN_NATIVE_CONTENT"}} if content.endswith('user') else {
                    "type": "control_request", "request_id": "foreign-approval", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "title": "FOREIGN_NATIVE_APPROVAL", "input": {"command": "must-not-execute"}}}
                with output_lock:
                    print(json.dumps({**foreign, "session_id": str(uuid.uuid4())}), flush=True)
                time.sleep(5)
                continue
            message_id = "msg-" + str(uuid.uuid4())
            record("user", frame["uuid"], frame["message"])
            send(frame)
            send({"type": "system", "subtype": "init", "cwd": cwd, "model": state["model"],
                "permissionMode": state["permission_mode"], "fast_mode_state": "on" if state["fast_mode"] else "off"})
            send({"type": "system", "subtype": "session_state_changed", "state": "running", "sdk_host_only": True})
            if content == "crash":
                print("synthetic stderr token=should-not-leak", file=sys.stderr, flush=True)
                os._exit(7)
            if content == "error":
                send({"type": "result", "subtype": "error_during_execution", "is_error": True,
                    "uuid": str(uuid.uuid4()), "errors": ["synthetic native execution failure"],
                    "modelUsage": {}, "total_cost_usd": 0})
                send({"type": "system", "subtype": "session_state_changed", "state": "idle", "sdk_host_only": True})
                continue
            if content == "/compact":
                send({"type": "system", "subtype": "status", "status": "compacting", "uuid": str(uuid.uuid4())})
                time.sleep(.3)
                state["context"] = 200
                send({"type": "system", "subtype": "compact_boundary", "uuid": str(uuid.uuid4()),
                    "compact_metadata": {"trigger": "manual", "pre_tokens": 1200}})
                send({"type": "system", "subtype": "status", "status": None, "compact_result": "success", "uuid": str(uuid.uuid4())})
            if content == "retry":
                send({"type": "system", "subtype": "api_retry", "attempt": 1, "max_retries": 3,
                    "retry_delay_ms": 2000, "error_status": 429, "error": "rate_limit", "uuid": str(uuid.uuid4())})
                interrupted.wait(2)
            if content == "rate-limit":
                send({"type": "rate_limit_event", "uuid": str(uuid.uuid4()), "rate_limit_info": {
                    "status": "allowed_warning", "rateLimitType": "five_hour", "utilization": .8, "resetsAt": 1900000000}})
            if content == "usage-unavailable": state["usage_available"] = False
            if content == "usage-available": state["usage_available"] = True
            if content == 'background':
                send({'type':'system','subtype':'background_tasks_changed','tasks':[{'task_id':'synthetic-background','task_type':'local_bash','description':'Synthetic native background job'}]})
            if content == 'finish-background':
                send({'type':'system','subtype':'background_tasks_changed','tasks':[]})
            stream_text("streaming Claude progress", message_id)
            if content == "hold":
                interrupted.wait(30)
            elif content in ("approval", "question"):
                call_id = "tool-" + str(uuid.uuid4())
                tool = "Bash" if content == "approval" else "AskUserQuestion"
                tool_input = {"command": "printf synthetic"} if content == "approval" else {"questions": [{"question": "Choose a synthetic answer?", "header": "Choice", "multiSelect": False,
                    "options": [{"label": "Alpha", "description": "synthetic Alpha"}, {"label": "Beta", "description": "synthetic Beta"}]}]}
                assistant([{"type": "tool_use", "id": call_id, "name": tool, "input": tool_input}], "msg-tool-" + call_id)
                response = request_control({"subtype": "can_use_tool", "tool_name": tool, "input": tool_input,
                    "tool_use_id": call_id, "decision_reason": "synthetic browser permission"})
                outcome = ((response or {}).get("response") or {}).get("response", {})
                output = "synthetic tool allowed" if outcome.get("behavior") == "allow" else "synthetic tool denied"
                if tool == "AskUserQuestion" and outcome.get("behavior") == "allow":
                    output = "synthetic answer " + json.dumps(outcome["updatedInput"]["answers"])
                result_message = {"role": "user", "content": [{"type": "tool_result", "tool_use_id": call_id,
                    "is_error": outcome.get("behavior") != "allow", "content": output}]}
                result_id = str(uuid.uuid4())
                record("user", result_id, result_message)
                send({"type": "user", "uuid": result_id, "message": result_message})
            elif content in ("elicitation", "elicitation-url"):
                request = {"subtype": "elicitation", "mcp_server_name": "Synthetic MCP", "message": "Synthetic MCP input",
                    "title": "Synthetic MCP form", "mode": "url" if content.endswith("url") else "form"}
                if request["mode"] == "url": request.update(url="https://example.invalid/authorize", elicitation_id="synthetic-url")
                else: request.update(requested_schema={"type": "object", "properties": {
                    "choice": {"type": "string", "title": "Synthetic choice", "enum": ["Alpha", "Beta"]},
                    "enabled": {"type": "boolean", "title": "Synthetic enabled", "default": True},
                    "count": {"type": "integer", "title": "Synthetic count", "minimum": 1}}, "required": ["choice", "count"]},
                    property_order=["choice", "enabled", "count"])
                response = request_control(request)
                stream_text("MCP response " + json.dumps(((response or {}).get("response") or {}).get("response", {})), "mcp-response-" + message_id)
            elif content == "dialog":
                assert "refusal_fallback_prompt" in state.get("dialog_kinds", []), "host did not declare a dialog renderer"
                response = request_control({"subtype": "request_user_dialog", "dialog_kind": "refusal_fallback_prompt",
                    "payload": {"originalModel": state["model"], "fallbackModel": "synthetic-fallback", "guidanceText": "Synthetic fallback choice"}})
                stream_text("Dialog response " + json.dumps(((response or {}).get("response") or {}).get("response", {})), "dialog-response-" + message_id)
            stream_text("Claude interrupted" if interrupted.is_set() else "Claude synthetic final " + content, "msg-final-" + message_id)
            state["completed"] += 1
            send({"type": "result", "subtype": "success", "is_error": False, "uuid": str(uuid.uuid4()), "result": "final",
                "usage": {"input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 30, "cache_creation_input_tokens": 10},
                "modelUsage": model_usage(), "total_cost_usd": state["completed"] * .01})
            send({"type": "system", "subtype": "session_state_changed", "state": "idle", "sdk_host_only": True})

    threading.Thread(target=turns, daemon=True).start()
    with log.open("a") as stream:
        stream.write(json.dumps({"type": "spawn", "pid": os.getpid(), "args": args, "cwd": cwd, "sid": sid}) + "\n")
    for line in sys.stdin:
        frame = json.loads(line)
        with log.open("a") as stream:
            stream.write(json.dumps(frame) + "\n")
        if frame["type"] == "user":
            inputs.put(frame)
        elif frame["type"] == "control_response":
            key = frame["response"]["request_id"]
            with mutex:
                if key in pending:
                    pending[key][1] = frame
                    pending[key][0].set()
        elif frame["type"] == "control_request":
            request = frame["request"]
            subtype = request["subtype"]
            response = {}
            if subtype == "initialize":
                if os.environ.get('SESSIONDOCK_FAKE_STARTUP_HOOKS'):
                    temporary_sid = str(uuid.uuid4())
                    for kind in ('hook_started', 'hook_response'):
                        with output_lock:
                            print(json.dumps({"type": "system", "subtype": kind, "hook_event": "SessionStart",
                                "session_id": temporary_sid, "stdout": "startup-hook-must-not-project",
                                "stderr": "startup-hook-must-not-project"}), flush=True)
                if os.environ.get('SESSIONDOCK_FAKE_STARTUP_FAILURE'):
                    print('claude: command not found; token=synthetic-startup-secret', file=sys.stderr, flush=True)
                    os._exit(127)
                fail_once = os.environ.get('SESSIONDOCK_FAKE_FORK_FAIL_ONCE')
                if fork and fail_once and Path(fail_once).is_file():
                    Path(fail_once).unlink()
                    os._exit(7)
                state["dialog_kinds"] = request.get("supportedDialogKinds", [])
                response = {"models": [{"value": "fake-claude", "displayName": "Synthetic Claude", "supportsEffort": True, "supportedEffortLevels": ["low", "high"]}],
                    "commands": [{"name": "compact", "description": "Compact synthetic context"}], "pid": os.getpid(), "fast_mode_state": "off"}
            elif subtype == "set_model":
                state["model"] = request["model"]
            elif subtype == "set_permission_mode":
                state["permission_mode"] = request["mode"]
            elif subtype == "apply_flag_settings":
                if "effortLevel" in request["settings"]: state["effort"] = request["settings"]["effortLevel"]
                if "fastMode" in request["settings"]: state["fast_mode"] = request["settings"]["fastMode"]
            elif subtype in ("get_context_usage", "get_usage"):
                if not state["usage_available"]:
                    send({"type": "control_response", "response": {"subtype": "error", "request_id": frame["request_id"], "error": "synthetic experimental usage unavailable"}})
                    continue
                if subtype == "get_context_usage":
                    assert request["detail"] == "summary", request
                    response = {"totalTokens": state["context"], "maxTokens": 200000, "rawMaxTokens": 250000,
                        "percentage": state["context"] / 2000, "categories": [{"name": "Messages", "tokens": state["context"], "kind": "used"}]}
                else:
                    assert request["skip_behaviors"] is True, request
                    response = {"session": {"total_cost_usd": state["completed"] * .01, "model_usage": model_usage()},
                        "subscription_type": "pro", "rate_limits_available": True,
                        "rate_limits": {"five_hour": {"utilization": 80, "resets_at": "2030-03-17T17:46:40Z"}}, "behaviors": None}
            elif subtype == "interrupt":
                interrupted.set()
            else:
                send({"type": "control_response", "response": {"subtype": "error", "request_id": frame["request_id"], "error": "unsupported synthetic control"}})
                continue
            send({"type": "control_response", "response": {"subtype": "success", "request_id": frame["request_id"], "response": response}})


if __name__ == "__main__":
    if "--fake-cli" not in sys.argv:
        raise SystemExit("Use as an imported browser fixture or with --fake-cli")
    fake_cli()
