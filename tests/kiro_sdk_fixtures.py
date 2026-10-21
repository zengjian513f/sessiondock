"""Synthetic Kiro ACP stdio & CLI peer and isolated launcher setup; no real CLI/account.

Run as a configured CLI prefix:
- ACP SDK mode: python kiro_sdk_fixtures.py --fake-kiro acp --agent-engine=v3 --auth-method=cli
- CLI PTY mode: python kiro_sdk_fixtures.py --fake-kiro chat --agent-engine=v3 [--resume-id <sid>]
"""
# run_validation: skip
from __future__ import annotations

import json
import os
from pathlib import Path
import signal
import shutil
import sys
import threading
import time
import uuid


class KiroFixture:
    def __init__(self, root: Path, host_binary: Path | None = None):
        self.root = Path(root)
        self.home = self.root / "kiro-home"
        self.sessions = self.root / "fake-native-store"
        self.sessions.mkdir(parents=True)
        (self.sessions / "cli").mkdir(parents=True)
        self.work = self.root / "work-kiro"
        self.work.mkdir()
        self.log = self.root / "kiro-acp-received.jsonl"
        self.launcher = self.root / "launcher.json"
        self.host_dir = self.root / "ptyhost-dir"
        self.host_dir.mkdir()
        self.proc = self.root / "proc"
        self.proc.mkdir()
        (self.proc / "stat").write_text("btime 1700000000\n")

        # 1. Primary V3 session (<sessions>/bucket_v3/<sid>/session.json + messages.jsonl)
        self.sid = "019a0000-0000-7000-8000-000000000301"
        self.v3_dir = self.sessions / "bucket_v3" / self.sid
        self.v3_dir.mkdir(parents=True)
        self.write_v3_session(
            self.v3_dir,
            self.sid,
            str(self.work),
            "Synthetic Kiro V3 Session",
            model="claude-haiku-4.5",
            effort="low",
        )
        (self.v3_dir / "messages.jsonl").write_text(
            json.dumps({
                "id": "msg-u1",
                "timestamp": "2026-10-18T00:01:00Z",
                "message": {
                    "kind": "user",
                    "data": {"content": [{"kind": "text", "data": "Explain Kiro V3 ACP architecture"}]},
                },
            }) + "\n" +
            json.dumps({
                "id": "msg-a1",
                "timestamp": "2026-10-18T00:01:05Z",
                "message": {
                    "kind": "assistant",
                    "data": {"content": [{"kind": "text", "data": "Kiro V3 uses ACP over JSON-RPC 2.0."}]},
                },
            }) + "\n"
        )

        # 2. Legacy V2 session (<sessions>/cli/<v2_sid>.json)
        self.v2_sid = "019a0000-0000-7000-8000-000000000202"
        self.v2_json = self.sessions / "cli" / f"{self.v2_sid}.json"
        self.v2_json.write_text(json.dumps({
            "session_id": self.v2_sid,
            "cwd": str(self.work),
            "title": "Synthetic Kiro V2 Legacy",
            "created_at": "2026-10-17T12:00:00Z",
            "updated_at": "2026-10-17T12:05:00Z",
            "history": [
                {
                    "user": {"content": {"Prompt": {"prompt": "Hello from Kiro V2 legacy"}}},
                    "assistant": {"Response": {"message_id": "v2-a1", "content": "Kiro V2 legacy response ready."}},
                }
            ],
            "session_state": {
                "rts_model_state": {"model_info": {"model_id": "claude-sonnet-4.5"}},
                "conversation_metadata": {
                    "user_turn_metadatas": [
                        {"timestamp": "2026-10-17T12:05:00Z", "model_id": "claude-sonnet-4.5"}
                    ]
                },
            },
        }))

        # 3. Empty V3 session (0-byte messages.jsonl) for empty title & delete checks
        self.empty_sid = "019a0000-0000-7000-8000-000000000399"
        self.empty_dir = self.sessions / "bucket_v3" / self.empty_sid
        self.empty_dir.mkdir(parents=True)
        self.write_v3_session(self.empty_dir, self.empty_sid, str(self.work), "")
        (self.empty_dir / "messages.jsonl").write_text("")

        host_bin = str(host_binary) if host_binary else sys.executable
        self.launcher.write_text(json.dumps({
            "schema": 2,
            "host_binary": host_bin,
            "host_dir": str(self.host_dir),
            "profiles": [
                {
                    "id": "kiro",
                    "source": "kiro",
                    "label": "Kiro",
                    "executable": sys.executable,
                    "env_remove": ["LD_PRELOAD", "SESSIONDOCK_TEST_CLOCK_FILE"],
                    "args": [str(Path(__file__).resolve()), "--fake-kiro"],
                    "new_args": ["chat", "--agent-engine=v3"],
                    "resume_args": ["chat", "--agent-engine=v3", "--resume-id", "{sid}"],
                    "env": {
                        "HOME": str(self.home),
                        "KIRO_SDK_FAKE_ROOT": str(self.sessions),
                        "KIRO_SDK_FAKE_LOG": str(self.log),
                    },
                }
            ],
        }))

    @staticmethod
    def write_v3_session(
        path: Path,
        sid: str,
        cwd: str,
        title: str,
        model: str = "claude-haiku-4.5",
        effort: str = "low",
    ):
        doc = {
            "schemaVersion": 1,
            "id": sid,
            "sessionId": sid,
            "cwd": cwd,
            "workspaceDirectory": cwd,
            "createdAt": "2026-10-18T00:00:00Z",
            "updatedAt": "2026-10-18T00:05:00Z",
            "modelId": model,
            "effortLevel": effort,
        }
        if title:
            doc["title"] = title
        (path / "session.json").write_text(json.dumps(doc))

    @property
    def env(self):
        return {
            # An account-scope identifier only. This path intentionally does not exist.
            "SESSIONDOCK_KIRO_ROOT": str(self.home / ".kiro" / "sessions"),
            "KIRO_SDK_FAKE_ROOT": str(self.sessions),
            "SESSIONDOCK_LAUNCHER_CONFIG": str(self.launcher),
            "SESSIONDOCK_PROC_ROOT": str(self.proc),
        }

    def received(self, method: str | None = None):
        if not self.log.exists():
            return []
        rows = [json.loads(line) for line in self.log.read_text().splitlines() if line.strip()]
        return [row for row in rows if method is None or row.get("method") == method]


class FakeKiroAcp:
    def __init__(self):
        self.sessions_root = Path(os.environ["KIRO_SDK_FAKE_ROOT"])
        self.log = Path(os.environ["KIRO_SDK_FAKE_LOG"])
        self.lock = threading.Lock()
        self.sid = None
        self.session_dir = None
        self.cwd = None
        self.model = "claude-haiku-4.5"
        self.effort = "low"
        self.mode = "vibe"
        self.autopilot = "off"
        self.cancel = threading.Event()
        self.pending_rpc = {}
        self.turn_thread = None
        with self.log.open("a") as f:
            f.write(json.dumps({"fixture_start": {"pid": os.getpid(), "argv": sys.argv[1:]}}) + "\n")

    def send(self, message):
        with self.lock:
            sys.stdout.write(json.dumps(message, ensure_ascii=False) + "\n")
            sys.stdout.flush()

    def reply(self, request, result):
        self.send({"jsonrpc": "2.0", "id": request["id"], "result": result})

    def error(self, request, code, message):
        self.send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": code, "message": message}})

    def update(self, update_type, **fields):
        self.send({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": self.sid,
                "update": {"sessionUpdate": update_type, **fields},
            },
        })

    def config_options(self):
        return [
            {
                "id": "model",
                "name": "Model",
                "category": "model",
                "type": "select",
                "currentValue": self.model,
                "options": [
                    {"value": "auto", "name": "auto", "description": "Auto model"},
                    {"value": "claude-haiku-4.5", "name": "claude-haiku-4.5", "description": "Claude Haiku 4.5 (0.4x credit)"},
                    {"value": "qwen3-coder-next", "name": "qwen3-coder-next", "description": "Qwen3 Coder Next (0.05x credit)"},
                ],
            },
            {
                "id": "effortLevel",
                "name": "Thinking Effort",
                "category": "thought_level",
                "type": "select",
                "currentValue": self.effort,
                "options": [
                    {"value": "low", "name": "Low"},
                    {"value": "medium", "name": "Medium"},
                    {"value": "high", "name": "High"},
                    {"value": "max", "name": "Max"},
                ],
            },
            {
                "id": "mode",
                "name": "Mode",
                "category": "mode",
                "type": "select",
                "currentValue": self.mode,
                "options": [
                    {"value": "vibe", "name": "vibe"},
                    {"value": "plan", "name": "Plan"},
                ],
            },
            {
                "id": "autopilot",
                "name": "Autopilot",
                "category": "autopilot",
                "type": "select",
                "currentValue": self.autopilot,
                "options": [
                    {"value": "on", "name": "Enabled"},
                    {"value": "off", "name": "Disabled"},
                ],
            },
        ]

    def open_payload(self):
        return {
            "sessionId": self.sid,
            "configOptions": self.config_options(),
            "modes": {
                "currentModeId": self.mode,
                "availableModes": [
                    {"id": "vibe", "name": "Default"},
                    {"id": "plan", "name": "Plan"},
                ],
            },
        }

    def handle(self, message):
        with self.log.open("a") as f:
            f.write(json.dumps(message, ensure_ascii=False) + "\n")
        method = message.get("method")
        params = message.get("params") or {}
        if method is None:
            req_id = message.get("id")
            if req_id in self.pending_rpc:
                ev, box = self.pending_rpc.pop(req_id)
                box["result"] = message.get("result")
                ev.set()
            return

        if method == "initialize":
            self.reply(message, {
                "protocolVersion": 1,
                "agentInfo": {"name": "kiro-cli", "version": "2.24.1"},
                "agentCapabilities": {
                    "loadSession": True,
                    "promptCapabilities": {"image": True, "embeddedContext": True},
                    "sessionCapabilities": {"fork": {}, "list": {}, "delete": {}},
                    "_meta": {"kiro": {"extensionMethods": ["_kiro/session/compact", "_kiro/session/context", "_kiro/session/history"], "replayMarking": True, "sessionSources": ["local"], "sessionListScopes": ["workspace", "user"]}},
                },
            })
        elif method == "session/list":
            rows = []
            for path in sorted((self.sessions_root / "bucket_v3").glob("*/session.json")):
                meta = json.loads(path.read_text())
                if params.get("cwd") and params["cwd"] != meta["cwd"]:
                    continue
                rows.append({"sessionId": meta["id"], "cwd": meta["cwd"],
                             "title": meta.get("title", ""), "updatedAt": meta["updatedAt"],
                             "_meta": {"kiro": {"modelId": meta["modelId"],
                             "effortLevel": meta["effortLevel"], "agentMode": "vibe",
                             "createdAt": meta["createdAt"], "source": "local",
                             "executionTarget": {"kind": "local"}, "status": "idle"}}})
            # Force the real catalog through opaque cursor pagination.
            offset = 1 if params.get("cursor") == "fixture-second-page" else 0
            result = {"sessions": rows[offset:offset + 1]}
            if offset == 0 and len(rows) > 1:
                result["nextCursor"] = "fixture-second-page"
            if offset:
                result["sessions"] = rows[offset:]
            self.reply(message, result)
        elif method == "session/new":
            self.cwd = params.get("cwd") or os.getcwd()
            self.sid = "sess_" + str(uuid.uuid4())
            initial = params.get("_meta", {}).get("kiro", {})
            self.model = initial.get("modelId", self.model)
            self.effort = initial.get("effortLevel", self.effort)
            self.mode = initial.get("modeId", self.mode)
            self.session_dir = self.sessions_root / "bucket_v3" / self.sid
            self.session_dir.mkdir(parents=True, exist_ok=True)
            KiroFixture.write_v3_session(self.session_dir, self.sid, self.cwd, "", self.model, self.effort)
            self.reply(message, self.open_payload())
        elif method == "session/load":
            self.sid = params["sessionId"]
            self.session_dir = self.sessions_root / "bucket_v3" / self.sid
            if not (self.session_dir / "session.json").is_file():
                self.error(message, -32000, "Session not found")
                return
            meta = json.loads((self.session_dir / "session.json").read_text())
            self.cwd = meta["cwd"]
            self.model = meta["modelId"]
            self.effort = meta["effortLevel"]
            for update in self.history_updates(self.session_dir):
                self.send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": self.sid, "update": update}})
            result = self.open_payload()
            # Real V3 load omits sessionId: the client retains the requested SID.
            result.pop("sessionId")
            self.reply(message, result)
        elif method == "_kiro/session/history":
            # Real default history returns older pages, not the initial load replay.
            self.reply(message, {"updates": [], "hasMore": False})
        elif method == "session/delete":
            directory = self.sessions_root / "bucket_v3" / params["sessionId"]
            if not directory.is_dir():
                self.error(message, -32000, "Session not found")
                return
            shutil.rmtree(directory)
            self.reply(message, {})
        elif method == "session/fork":
            source = self.sessions_root / "bucket_v3" / params["sessionId"]
            if not source.is_dir():
                self.error(message, -32000, "Session not found")
                return
            fork_sid = "sess_" + str(uuid.uuid4())
            target = self.sessions_root / "bucket_v3" / fork_sid
            shutil.copytree(source, target)
            meta = json.loads((target / "session.json").read_text())
            meta["id"] = meta["sessionId"] = fork_sid
            (target / "session.json").write_text(json.dumps(meta))
            self.reply(message, {"sessionId": fork_sid, "configOptions": self.config_options()})
        elif method == "session/set_config_option":
            cid = params.get("configId")
            val = str(params.get("value", ""))
            if cid == "model":
                self.model = val
            elif cid in ("effortLevel", "effort"):
                self.effort = val
            elif cid == "mode":
                self.mode = val
            elif cid == "autopilot":
                self.autopilot = val
            if self.session_dir and (self.session_dir / "session.json").is_file():
                meta = json.loads((self.session_dir / "session.json").read_text())
                meta["modelId"] = self.model
                meta["effortLevel"] = self.effort
                (self.session_dir / "session.json").write_text(json.dumps(meta))
            self.reply(message, {"configOptions": self.config_options()})
            self.update("config_option_update", configOptions=self.config_options())
        elif method == "session/set_mode":
            self.mode = params.get("modeId", "vibe")
            self.reply(message, {})
            self.update("current_mode_update", currentModeId=self.mode)
        elif method == "_kiro/session/compact":
            self.reply(message, {"compacted": True, "contextUsagePercentage": 6.5})
        elif method == "_kiro/session/context":
            self.reply(message, {
                "contextUsagePercentage": 18.5,
                "usedTokens": 37000,
                "maxTokens": 200000,
            })
        elif method == "session/cancel":
            self.cancel.set()
            for ev, _ in list(self.pending_rpc.values()):
                ev.set()
        elif method == "session/prompt":
            self.cancel.clear()
            self.turn_thread = threading.Thread(target=self.run_prompt, args=(message,), daemon=True)
            self.turn_thread.start()
        else:
            self.error(message, -32601, f"Unsupported method {method}")

    @staticmethod
    def history_updates(directory):
        path = directory / "messages.jsonl"
        if not path.exists():
            return []
        result = []
        for line in path.read_text().splitlines():
            row = json.loads(line)
            msg = row["message"]
            result.append({"sessionUpdate": "user_message_chunk" if msg["kind"] == "user" else "agent_message_chunk",
                           "content": {"type": "text", "text": msg["data"]["content"][0]["data"]},
                           "_meta": {"kiro": {"messageId": row["id"], "timestamp": row["timestamp"], "replay": True}}})
        return result

    def append_v3_message(self, kind: str, text: str):
        if not self.session_dir:
            return
        self.session_dir.mkdir(parents=True, exist_ok=True)
        now = "2026-10-18T00:10:00Z"
        rec = {
            "id": f"msg-{uuid.uuid4().hex[:8]}",
            "timestamp": now,
            "message": {
                "kind": kind,
                "data": {"content": [{"kind": "text", "data": text}]},
            },
        }
        with (self.session_dir / "messages.jsonl").open("a") as f:
            f.write(json.dumps(rec, ensure_ascii=False) + "\n")

    def call_client(self, req_id: str, method: str, params: dict) -> dict | None:
        ev = threading.Event()
        box = {"result": None}
        self.pending_rpc[req_id] = (ev, box)
        self.send({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params,
        })
        ev.wait(timeout=15.0)
        return box["result"]

    def run_prompt(self, message):
        blocks = message["params"].get("prompt") or []
        text = " ".join(b.get("text", "") for b in blocks if isinstance(b, dict)).strip()
        self.append_v3_message("user", text)

        if text == "wait-for-cancel":
            self.update(
                "agent_thought_chunk",
                content={"type": "text", "text": "Thinking before cancellation..."},
            )
            self.update(
                "agent_message_chunk",
                content={"type": "text", "text": "Partial response before stop"},
            )
            self.cancel.wait(timeout=15.0)
            self.reply(message, {"stopReason": "cancelled"})
            return

        if text == "permission-test":
            tc_id = "tc-perm-1"
            self.update(
                "tool_call",
                toolCallId=tc_id,
                title="Execute bash command",
                kind="execute",
                status="in_progress",
                rawInput={"command": "echo kiro-approved"},
            )
            res = self.call_client("perm-req-1", "session/request_permission", {
                "sessionId": self.sid,
                "toolCall": {
                    "toolCallId": tc_id,
                    "title": "Execute bash command",
                    "kind": "execute",
                    "rawInput": {"command": "echo kiro-approved"},
                },
                "options": [
                    {"optionId": "allow_once", "name": "Allow once", "kind": "allow_once"},
                    {"optionId": "reject_once", "name": "Reject", "kind": "reject_once"},
                ],
            })
            outcome = (res or {}).get("outcome", {})
            if outcome.get("optionId") == "allow_once":
                self.update(
                    "tool_call_update",
                    toolCallId=tc_id,
                    status="completed",
                    rawOutput={"output": "kiro-approved\n", "exitCode": 0},
                )
                ans = "Permission granted and command executed."
                self.update("agent_message_chunk", content={"type": "text", "text": ans})
                self.append_v3_message("assistant", ans)
            else:
                self.update(
                    "tool_call_update",
                    toolCallId=tc_id,
                    status="failed",
                    rawOutput={"error": "Rejected by user"},
                )
                ans = "Permission rejected by user."
                self.update("agent_message_chunk", content={"type": "text", "text": ans})
                self.append_v3_message("assistant", ans)
            self.reply(message, {"stopReason": "end_turn"})
            return

        if text == "question-test":
            res = self.call_client("ask-req-1", "_kiro/userInput", {
                "sessionId": self.sid,
                "title": "Kiro Question",
                "questions": [
                    {
                        "id": "q1",
                        "header": "Engine",
                        "question": "Which Kiro mode should we verify?",
                        "options": [
                            {"label": "ACP V3", "description": "Native JSON-RPC SDK"},
                            {"label": "CLI PTY", "description": "Terminal xterm.js"},
                        ],
                    }
                ],
            })
            chosen = (res or {}).get("answer") or json.dumps((res or {}).get("answers", {}))
            ans = f"User selected Kiro option: {chosen}"
            self.update("agent_message_chunk", content={"type": "text", "text": ans})
            self.append_v3_message("assistant", ans)
            self.reply(message, {"stopReason": "end_turn"})
            return

        # Standard turn: thought + tool_call + message + metadata
        self.update(
            "agent_thought_chunk",
            content={"type": "text", "text": "Analyzing Kiro request with low effort..."},
        )
        tc_id = f"tc-{uuid.uuid4().hex[:6]}"
        self.update(
            "tool_call",
            toolCallId=tc_id,
            title="Read workspace file",
            kind="read",
            status="in_progress",
            rawInput={"path": "README.md"},
        )
        self.update(
            "tool_call_update",
            toolCallId=tc_id,
            status="completed",
            rawOutput={"content": "# Kiro Workspace\n"},
        )
        reply_text = f"Kiro ACP V3 final answer for: {text}"
        self.update(
            "agent_message_chunk",
            content={"type": "text", "text": reply_text},
        )
        self.update("session_info_update", _meta={"kiro": {
            "kind": "context_usage", "contextUsage": {"usagePercentage": 18.5}, "usagePercentage": 18.5,
            "breakdown": {"yourPrompts": {"tokens": 1200, "percent": 12}, "kiroResponses": {"tokens": 650, "percent": 6.5}},
        }})
        self.append_v3_message("assistant", reply_text)
        self.reply(message, {"stopReason": "end_turn"})


def run_fake_cli_chat(args: list[str]):
    sessions_root = Path(os.environ["KIRO_SDK_FAKE_ROOT"])
    sid = None
    for i, arg in enumerate(args):
        if arg in ("--resume-id", "--resume") and i + 1 < len(args):
            sid = args[i + 1]
        elif arg.startswith("--resume-id="):
            sid = arg.split("=", 1)[1]
    if not sid:
        sid = f"019a0000-0000-7000-8000-{uuid.uuid4().hex[:12]}"
        sdir = sessions_root / "bucket_v3" / sid
        sdir.mkdir(parents=True, exist_ok=True)
        KiroFixture.write_v3_session(sdir, sid, os.getcwd(), "CLI Created Kiro Session")
        (sdir / "messages.jsonl").write_text(
            json.dumps({
                "id": "msg-cli-1",
                "timestamp": "2026-10-18T01:00:00Z",
                "message": {
                    "kind": "user",
                    "data": {"content": [{"kind": "text", "data": "CLI session started"}]},
                },
            }) + "\n"
        )

    session_dir = sessions_root / "bucket_v3" / sid
    if not (session_dir / "session.json").is_file():
        raise SystemExit("Unknown native session ID")
    log = Path(os.environ["KIRO_SDK_FAKE_LOG"])
    with log.open("a") as f:
        f.write(json.dumps({"fixture_cli_start": {"pid": os.getpid(), "argv": args, "sessionId": sid}}) + "\n")

    def cleanup(*_):
        with log.open("a") as f:
            f.write(json.dumps({"fixture_cli_exit": {"pid": os.getpid(), "sessionId": sid}}) + "\n")
        sys.exit(0)

    signal.signal(signal.SIGTERM, cleanup)
    signal.signal(signal.SIGINT, cleanup)

    sys.stdout.write(f"Kiro CLI v3 Interactive Terminal (session={sid})\r\n> ")
    sys.stdout.flush()
    try:
        for line in sys.stdin:
            cmd = line.strip()
            if cmd in ("exit", "/quit", "/exit"):
                break
            with (session_dir / "messages.jsonl").open("a") as f:
                f.write(json.dumps({"id": "cli-" + uuid.uuid4().hex, "timestamp": "2026-10-18T02:00:00Z",
                                   "message": {"kind": "user", "data": {"content": [{"kind": "text", "data": cmd}]}}}) + "\n")
            sys.stdout.write(f"Kiro CLI echoed: {cmd}\r\n> ")
            sys.stdout.flush()
    finally:
        cleanup()


def main():
    args = sys.argv[1:]
    if args and args[0] == "--fake-kiro":
        args = args[1:]
    if any(a in ("--version", "-V", "--help", "-h") for a in args):
        sys.stdout.write("kiro-cli 2.24.1\n")
        sys.stdout.flush()
        return
    if "--list-models" in args:
        sys.stdout.write(json.dumps({
            "models": [
                {"model_id": "auto", "model_name": "Auto", "default_effort": "medium", "efforts": ["low", "medium", "high", "max"]},
                {"model_id": "claude-haiku-4.5", "model_name": "Claude Haiku 4.5", "default_effort": "low", "efforts": ["low", "medium", "high", "max"]},
                {"model_id": "qwen3-coder-next", "model_name": "Qwen3 Coder Next", "default_effort": "low", "efforts": ["low", "medium", "high", "max"]},
            ],
            "default_model": "claude-haiku-4.5",
        }) + "\n")
        sys.stdout.flush()
        return
    if "acp" in args:
        peer = FakeKiroAcp()
        for line in sys.stdin:
            if not line.strip():
                continue
            peer.handle(json.loads(line))
    else:
        run_fake_cli_chat(args)


if __name__ == "__main__":
    main()
