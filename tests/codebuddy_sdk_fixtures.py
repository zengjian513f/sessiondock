"""Private, synthetic CodeBuddy stream-json peer for browser integration tests.

Run through a launcher profile: python3 codebuddy_sdk_fixtures.py --fake-cli.
The peer speaks the CodeBuddy 2.175.0 NDJSON control protocol observed from the
real CLI (docs/codebuddy-sdk.md), including the full-history replay that follows
`initialize` on resume. It never imports an SDK, contacts a provider or reads
the user's CLI home.
"""
# run_validation: skip
from __future__ import annotations

from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import queue
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from urllib.request import ProxyHandler, build_opener

SECRET = "SYNTHETIC-ACCOUNT-TOKEN-MUST-NOT-LEAK"


def project_key(cwd):
    """Observed 2.175.0 naming: '/' becomes '-', leading '-' dropped."""
    return str(cwd).replace("/", "-").lstrip("-")


def native_uid(path):
    return "codebuddy:" + hashlib.sha1(str(path).encode()).hexdigest()[:16]


def native_records(sid, cwd, turns):
    """Native OpenAI-Responses style records for already finished turns."""
    rows, previous, stamp = [], None, 1792900000000
    for index, (question, answer) in enumerate(turns):
        for row in (
            {"type": "message", "role": "user", "id": f"seed-u{index}",
             "content": [{"type": "input_text", "text": question}],
             "providerData": {"agent": "cli", "conversationRequestId": f"req{index}"}},
            {"type": "reasoning", "id": f"seed-r{index}", "content": [],
             "rawContent": [{"type": "reasoning_text", "text": f"thinking about {question}"}],
             "providerData": {"model": "fake-buddy", "conversationRequestId": f"req{index}"}},
            {"type": "message", "role": "assistant", "id": f"seed-a{index}", "status": "completed",
             "content": [{"type": "output_text", "text": answer, "providerData": {"annotations": []}}],
             "providerData": {"model": "fake-buddy", "conversationRequestId": f"req{index}"}},
        ):
            stamp += 1000
            row.update(parentId=previous, timestamp=stamp, sessionId=sid, cwd=str(cwd))
            rows.append(row)
            previous = row["id"]
    return rows


class CodebuddyFixture:
    def __init__(self, root):
        self.root = Path(root)
        self.lifecycle_initialized = False
        self.home = self.root / "cli-home"
        self.config = self.home / ".codebuddy"
        self.projects = self.config / "projects"
        self.cwd = self.root / "workspace"
        self.received = self.root / "codebuddy-native-received.jsonl"
        self.sid = "3e4eb4fd-8678-467e-a8a3-34728c7c7e26"
        for directory in (self.projects, self.cwd, self.root / "host", self.root / "lifecycle",
                          self.root / "state", self.root / "proc"):
            directory.mkdir(parents=True, exist_ok=True)
        (self.root / "proc/stat").write_text("btime 1700000000\n")
        self.path = self.projects / project_key(self.cwd) / (self.sid + ".jsonl")
        self.path.parent.mkdir()
        self.path.write_text("".join(json.dumps(row) + "\n" for row in native_records(
            self.sid, self.cwd, [("earlier synthetic CodeBuddy prompt", "earlier synthetic CodeBuddy answer")])))
        self.uid = native_uid(self.path)
        self.launcher = self.root / "launcher.json"
        self.launcher.write_text(json.dumps({"host_binary": "/bin/true", "host_dir": str(self.root / "host"),
            "profiles": [{"id": "codebuddy-fixture", "source": "codebuddy", "executable": sys.executable,
                "args": [str(Path(__file__).resolve()), "--fake-cli"], "env": {"HOME": str(self.home),
                    "CODEBUDDY_CONFIG_DIR": str(self.config), "SESSIONDOCK_FAKE_CODEBUDDY_LOG": str(self.received)},
                "env_remove": ["LD_PRELOAD", "SESSIONDOCK_TEST_CLOCK_OFFSET"]}]}))

    def frames(self, kind=None):
        rows = [json.loads(line) for line in self.received.read_text().splitlines()] if self.received.exists() else []
        return [row for row in rows if kind is None or row.get("type") == kind]

    @contextmanager
    def server(self, binary, extra_env=None, *, preserve_sdkhosts=False):
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
            SESSIONDOCK_CODEBUDDY_ROOT=str(self.projects), SESSIONDOCK_LAUNCHER_CONFIG=str(self.launcher),
            SESSIONDOCK_PROC_ROOT=str(self.root / "proc"), SESSIONDOCK_PTYHOST_DIR=str(self.root / "host"),
            SESSIONDOCK_LIFECYCLE_DIR=str(self.root / "lifecycle"), SESSIONDOCK_STATE_DIR=str(self.root / "state"),
            SESSIONDOCK_CODEX_APP_SERVER="off", HOME=str(self.home))
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
                if not preserve_sdkhosts:
                    from sdkhost_fixtures import stop_private_sdkhosts
                    stop_private_sdkhosts(self.root)


def terminal_mode(args):
    """Interactive CLI stand-in: the PTY terminal mode launches without stream-json."""
    log = Path(os.environ["SESSIONDOCK_FAKE_CODEBUDDY_LOG"])
    with log.open("a") as stream:
        stream.write(json.dumps({"type": "cli_start", "pid": os.getpid(), "args": args, "cwd": str(Path.cwd())}) + "\n")
    print("CodeBuddy fake terminal ready", flush=True)
    for line in sys.stdin:
        print("CodeBuddy CLI echoed: " + line.strip(), flush=True)


def fake_cli():
    args = sys.argv[2:]
    if "--version" in args:
        print("2.175.0")
        return
    if "update" in args:
        raise SystemExit("synthetic peer never updates real software")
    if "--help" in args:
        # Same shape as the real `--model` line: the catalog is parsed from it.
        print("Options:\n  --model <model>    Model for the current session. Please provide the model ID.\n"
              "                     Currently supported: (default-model, fake-buddy, fake-buddy-2)\n"
              "  --effort <level>   Reasoning effort level (minimal, low, medium, high, xhigh, max)")
        return
    if "--input-format=stream-json" not in args:
        return terminal_mode(args)
    assert "--input-format=stream-json" in args and "--output-format=stream-json" in args, args
    assert "--print" not in args and not any(arg.startswith("--permission-prompt-tool") for arg in args), args

    def value(flag):
        return args[args.index(flag) + 1] if flag in args else None

    resume, assigned = value("--resume"), value("--session-id")
    sid = resume or assigned
    assert sid and not (resume and assigned), "exactly one explicit native session identity required"
    assert not any(arg.startswith("--resume=") for arg in args), args
    cwd = str(Path.cwd())
    config = Path(os.environ["CODEBUDDY_CONFIG_DIR"])
    path = config / "projects" / project_key(cwd) / (sid + ".jsonl")
    assert not any(os.environ.get(key) for key in ("CLAUDECODE", "CLAUDE_CONFIG_DIR", "CODEBUDDY_CODE_SESSION_ID"))
    log = Path(os.environ["SESSIONDOCK_FAKE_CODEBUDDY_LOG"])
    mutex = threading.Lock()
    output_lock = threading.Lock()
    state = {"model": value("--model") or "fake-buddy", "effort": value("--effort") or "medium",
             "permission_mode": "default", "completed": 0}
    pending = {}
    inputs = queue.Queue()
    interrupted = threading.Event()
    previous = None
    if path.is_file():
        rows = [json.loads(line) for line in path.read_text().splitlines()]
        previous = next((row["id"] for row in reversed(rows) if row.get("id")), None)
        assert rows[0]["sessionId"] == sid and rows[0]["cwd"] == cwd, "resume found another project's transcript"
    elif resume:
        print("No conversation found with session ID: " + sid, file=sys.stderr, flush=True)
        os._exit(1)

    def send(frame):
        with output_lock:
            print(json.dumps({**frame, "session_id": sid}), flush=True)

    def record(row):
        nonlocal previous
        path.parent.mkdir(parents=True, exist_ok=True)
        row = {**row, "id": row.get("id") or str(uuid.uuid4()), "parentId": previous,
               "timestamp": int(time.time() * 1000), "sessionId": sid, "cwd": cwd}
        with path.open("a") as stream:
            stream.write(json.dumps(row) + "\n")
        previous = row["id"]
        return row["id"]

    def init_frame(model):
        return {"type": "system", "subtype": "init", "uuid": sid, "cwd": cwd, "model": model,
                "permissionMode": state["permission_mode"], "tools": ["Bash"], "mcp_servers": []}

    def replay_history():
        send(init_frame("unknown"))
        for row in [json.loads(line) for line in path.read_text().splitlines()]:
            if row["type"] == "message" and row["role"] == "user":
                text = row["content"][0]["text"]
                send({"type": "user", "uuid": row["id"], "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
            elif row["type"] == "message":
                text = row["content"][0]["text"]
                send({"type": "assistant", "uuid": row["id"], "message": {
                    "id": "replay-" + row["id"], "role": "assistant", "model": row["providerData"]["model"],
                    "content": [{"type": "text", "text": text}], "usage": {"input_tokens": 1, "output_tokens": 1}}})
                send({"type": "result", "subtype": "success", "is_error": False, "uuid": "replay-result-" + row["id"],
                      "result": text, "usage": {"input_tokens": 1, "output_tokens": 1}, "modelUsage": {}, "total_cost_usd": 0})

    def stream_text(content, message_id):
        send({"type": "stream_event", "event": {"type": "message_start", "message": {"id": message_id, "model": state["model"]}}})
        send({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}})
        for part in (content[:8], content[8:]):
            send({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": part}}})
        send({"type": "stream_event", "event": {"type": "content_block_stop", "index": 0}})
        frame_id = record({"type": "message", "role": "assistant", "status": "completed",
            "content": [{"type": "output_text", "text": content, "providerData": {"annotations": []}}],
            "providerData": {"model": state["model"]}})
        send({"type": "assistant", "uuid": frame_id, "message": {"id": message_id, "role": "assistant", "model": state["model"],
            "content": [{"type": "text", "text": content}], "usage": {"input_tokens": 100, "output_tokens": 20}}})

    def request_control(request):
        key = "perm_" + str(uuid.uuid4())
        event = threading.Event()
        with mutex:
            pending[key] = [event, None]
        send({"type": "control_request", "request_id": key, "request": request})
        while not event.wait(.05):
            if interrupted.is_set():
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
            user_id = record({"type": "message", "role": "user", "content": [{"type": "input_text", "text": content}],
                              "providerData": {"agent": "cli"}})
            send(init_frame(state["model"]))
            # CodeBuddy mints its own uuid for the echoed user message.
            send({"type": "user", "uuid": user_id, "message": {"role": "user", "content": [{"type": "text", "text": content}]},
                  "parent_tool_use_id": None})
            send({"type": "system", "subtype": "status", "status": None})
            if content == "crash":
                print("synthetic stderr token=should-not-leak", file=sys.stderr, flush=True)
                os._exit(7)
            if content == "error":
                send({"type": "result", "subtype": "error_during_execution", "is_error": True,
                      "uuid": str(uuid.uuid4()), "errors": ["synthetic native execution failure"],
                      "modelUsage": {}, "total_cost_usd": 0})
                continue
            stream_text("streaming CodeBuddy progress", message_id)
            if content == "hold":
                interrupted.wait(30)
            elif content in ("approval", "question"):
                call_id = "call_" + uuid.uuid4().hex[:24]
                tool = "Bash" if content == "approval" else "AskUserQuestion"
                tool_input = {"command": "printf synthetic", "description": "synthetic"} if content == "approval" else {
                    "questions": [{"question": "Choose a synthetic answer?", "header": "Choice", "multiSelect": False,
                                   "options": [{"label": "Alpha", "description": "synthetic Alpha"},
                                               {"label": "Beta", "description": "synthetic Beta"}]}]}
                record({"type": "function_call", "callId": call_id, "name": tool, "arguments": json.dumps(tool_input)})
                send({"type": "assistant", "uuid": str(uuid.uuid4()), "message": {"id": "msg-tool-" + call_id, "role": "assistant",
                      "model": state["model"], "content": [{"type": "tool_use", "id": call_id, "name": tool, "input": tool_input}],
                      "usage": {"input_tokens": 1, "output_tokens": 1}}})
                response = request_control({"subtype": "can_use_tool", "tool_name": tool, "input": tool_input, "tool_use_id": call_id})
                outcome = ((response or {}).get("response") or {}).get("response", {})
                allowed = outcome.get("allowed") is True
                output = "synthetic tool allowed" if allowed else "synthetic tool denied"
                if tool == "AskUserQuestion" and allowed:
                    output = "synthetic answer " + json.dumps(outcome["updatedInput"]["answers"])
                record({"type": "function_call_result", "callId": call_id, "name": tool,
                        "status": "completed" if allowed else "failed", "output": {"type": "text", "text": output}})
                send({"type": "user", "uuid": str(uuid.uuid4()), "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": call_id, "is_error": not allowed, "content": [{"type": "text", "text": output}]}]}})
            stream_text("CodeBuddy interrupted" if interrupted.is_set() else "CodeBuddy synthetic final " + content,
                        "msg-final-" + message_id)
            state["completed"] += 1
            send({"type": "result", "subtype": "success", "is_error": False, "uuid": str(uuid.uuid4()), "result": "final",
                  "usage": {"input_tokens": 100, "output_tokens": 20},
                  "modelUsage": {state["model"]: {"inputTokens": state["completed"] * 100, "outputTokens": state["completed"] * 20,
                                                  "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0, "costUSD": 0}},
                  "total_cost_usd": 0})

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
                assert request["capabilities"]["askUserQuestion"] is True, request
                response = {"commands": [{"name": "/help", "description": "synthetic"}], "output_style": "default",
                            "models": [{"id": "fake-buddy", "name": "Synthetic Buddy"}, {"id": "fake-buddy-2", "name": "Synthetic Buddy 2"}],
                            "account": {"userName": "synthetic", "token": SECRET}, "currentModelId": state["model"]}
            elif subtype == "set_model":
                assert request["session_id"] == sid, request
                state["model"] = request["model"]
                response = {"session_id": sid, "model": request["model"]}
            elif subtype == "set_permission_mode":
                assert request["session_id"] == sid, request
                state["permission_mode"] = request["mode"]
                response = {"session_id": sid, "mode": request["mode"]}
            elif subtype == "set_config":
                assert request["session_id"] == sid, request
                state["effort"] = request["config"]["reasoningEffort"]
                response = {"session_id": sid, "updated": request["config"]}
            elif subtype == "interrupt":
                interrupted.set()
                response = {"session_id": sid, "interrupted": True, "reason": "SDK request"}
            else:
                send({"type": "control_response", "response": {"subtype": "error", "request_id": frame["request_id"],
                                                               "error": "unsupported synthetic control"}})
                continue
            send({"type": "control_response", "response": {"subtype": "success", "request_id": frame["request_id"], "response": response}})
            if subtype == "initialize" and path.is_file():
                replay_history()


if __name__ == "__main__":
    if "--fake-cli" not in sys.argv:
        raise SystemExit("Use as an imported browser fixture or with --fake-cli")
    fake_cli()
