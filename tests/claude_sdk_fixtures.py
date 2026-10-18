"""Private, synthetic Claude stream-json peer for browser integration tests.

Run through a launcher profile: python3 claude_sdk_fixtures.py --fake-cli.
It speaks the official SDK's NDJSON control protocol, never imports an SDK,
contacts a provider, starts a PTY, or reads the user's CLI home.
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
                "env_remove": ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]}]}))

    def frames(self, kind=None):
        rows = [json.loads(line) for line in self.received.read_text().splitlines()] if self.received.exists() else []
        return [row for row in rows if kind is None or row.get("type") == kind]

    @contextmanager
    def server(self, binary, extra_env=None):
        from frontend_paths import frontend_dir
        from send_browser import initialize
        initialize("--initialize-lifecycle", self.root / "lifecycle", Path(binary).resolve(strict=True))
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
    sid = Path(resume).stem if resume else value("--session-id")
    assert sid, "explicit native session identity required"
    cwd = str(Path.cwd())
    home = Path(os.environ["CLAUDE_CONFIG_DIR"])
    path = Path(resume) if resume else home / "projects" / project_key(cwd) / (sid + ".jsonl")
    assert path.is_relative_to(home / "projects"), path
    assert not any(os.environ.get(key) for key in ("CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CODEX_THREAD_ID", "CLAUDE_CODE_SESSION_ID"))
    log = Path(os.environ["SESSIONDOCK_FAKE_CLAUDE_LOG"])
    mutex = threading.Lock()
    output_lock = threading.Lock()
    state = {"model": value("--model") or "fake-claude", "effort": value("--effort") or "low"}
    pending = {}
    inputs = queue.Queue()
    interrupted = threading.Event()
    previous = json.loads(path.read_text().splitlines()[-1])["uuid"] if resume else None

    def send(frame):
        with output_lock:
            print(json.dumps({**frame, "session_id": sid}), flush=True)

    def record(kind, identifier, message):
        nonlocal previous
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a") as stream:
            stream.write(json.dumps({"type": kind, "uuid": identifier, "parentUuid": previous,
                "sessionId": sid, "cwd": cwd, "timestamp": "2026-10-18T11:00:00Z", "isSidechain": False,
                "message": message}) + "\n")
        previous = identifier

    def assistant(content, message_id):
        frame_id = str(uuid.uuid4())
        message = {"role": "assistant", "id": message_id, "model": state["model"], "content": content}
        record("assistant", frame_id, message)
        send({"type": "assistant", "uuid": frame_id, "message": message})

    def stream_text(content, message_id):
        send({"type": "stream_event", "event": {"type": "message_start", "message": {"id": message_id, "model": state["model"]}}})
        send({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}})
        for part in (content[:8], content[8:]):
            send({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": part}}})
        send({"type": "stream_event", "event": {"type": "content_block_stop", "index": 0}})
        assistant([{"type": "text", "text": content}], message_id)

    def request_permission(tool, tool_input, call_id):
        key = str(uuid.uuid4())
        event = threading.Event()
        with mutex:
            pending[key] = [event, None]
        send({"type": "control_request", "request_id": key, "request": {"subtype": "can_use_tool",
            "tool_name": tool, "input": tool_input, "tool_use_id": call_id, "decision_reason": "synthetic browser permission"}})
        while not event.wait(.05):
            if interrupted.is_set():
                send({"type": "control_cancel_request", "request_id": key})
                with mutex:
                    pending.pop(key, None)
                return None
        with mutex:
            return pending.pop(key)[1]

    def turns():
        while True:
            frame = inputs.get()
            interrupted.clear()
            content = frame["message"]["content"]
            message_id = "msg-" + str(uuid.uuid4())
            record("user", frame["uuid"], frame["message"])
            send(frame)
            send({"type": "system", "subtype": "init", "cwd": cwd, "model": state["model"]})
            send({"type": "system", "subtype": "session_state_changed", "state": "running", "sdk_host_only": True})
            if content == "crash":
                print("synthetic stderr token=should-not-leak", file=sys.stderr, flush=True)
                os._exit(7)
            if content == "error":
                send({"type": "result", "subtype": "error_during_execution", "is_error": True,
                    "uuid": str(uuid.uuid4()), "errors": ["synthetic native execution failure"]})
                send({"type": "system", "subtype": "session_state_changed", "state": "idle", "sdk_host_only": True})
                continue
            stream_text("streaming Claude progress", message_id)
            if content == "hold":
                interrupted.wait(30)
            elif content in ("approval", "question"):
                call_id = "tool-" + str(uuid.uuid4())
                tool = "Bash" if content == "approval" else "AskUserQuestion"
                tool_input = {"command": "printf synthetic"} if content == "approval" else {"questions": [{"question": "Choose a synthetic answer?", "header": "Choice", "multiSelect": False,
                    "options": [{"label": "Alpha", "description": "synthetic Alpha"}, {"label": "Beta", "description": "synthetic Beta"}]}]}
                assistant([{"type": "tool_use", "id": call_id, "name": tool, "input": tool_input}], "msg-tool-" + call_id)
                response = request_permission(tool, tool_input, call_id)
                outcome = ((response or {}).get("response") or {}).get("response", {})
                output = "synthetic tool allowed" if outcome.get("behavior") == "allow" else "synthetic tool denied"
                if tool == "AskUserQuestion" and outcome.get("behavior") == "allow":
                    output = "synthetic answer " + json.dumps(outcome["updatedInput"]["answers"])
                result_message = {"role": "user", "content": [{"type": "tool_result", "tool_use_id": call_id,
                    "is_error": outcome.get("behavior") != "allow", "content": output}]}
                result_id = str(uuid.uuid4())
                record("user", result_id, result_message)
                send({"type": "user", "uuid": result_id, "message": result_message})
            stream_text("Claude interrupted" if interrupted.is_set() else "Claude synthetic final " + content, "msg-final-" + message_id)
            send({"type": "result", "subtype": "success", "is_error": False, "uuid": str(uuid.uuid4()), "result": "final"})
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
                response = {"models": [{"value": "fake-claude", "displayName": "Synthetic Claude", "supportsEffort": True, "supportedEffortLevels": ["low", "high"]}], "commands": [], "pid": os.getpid()}
            elif subtype == "set_model":
                state["model"] = request["model"]
            elif subtype == "apply_flag_settings":
                state["effort"] = request["settings"]["effortLevel"]
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
