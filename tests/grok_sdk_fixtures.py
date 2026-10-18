"""Synthetic Grok ACP stdio peer and isolated launcher setup; no real CLI/account.

Run as a configured CLI prefix: python grok_sdk_fixtures.py --fake-grok agent
--no-leader stdio. The browser suite imports GrokFixture. Persistence below is
owned by the fake native peer, never by the SessionDock bridge.
"""
# run_validation: skip
from __future__ import annotations

import json
import os
from pathlib import Path
import sys
import threading
import time
from urllib.parse import quote
import uuid


class GrokFixture:
    def __init__(self, root):
        self.root = Path(root)
        self.home = self.root / "grok-home"
        self.sessions = self.home / "sessions"
        self.sessions.mkdir(parents=True)
        self.work = self.root / "work-中文"
        self.work.mkdir()
        self.log = self.root / "acp-received.jsonl"
        self.active = self.home / "active_sessions.json"
        self.launcher = self.root / "launcher.json"
        (self.root / "unused-host").mkdir()
        self.proc = self.root / "proc"
        self.proc.mkdir()
        (self.proc / "stat").write_text("btime 1700000000\n")
        self.sid = "019a0000-0000-7000-8000-000000000062"
        self.path = self.sessions / quote(str(self.work), safe="-_.~") / self.sid
        self.path.mkdir(parents=True)
        self.write_summary(self.path, self.sid, str(self.work), "Synthetic Grok SDK")
        (self.path / "chat_history.jsonl").write_text(
            json.dumps({"type": "assistant", "content": "Grok earlier synthetic answer"}) + "\n")
        self.launcher.write_text(json.dumps({"schema": 2, "host_binary": sys.executable,
            "host_dir": str(self.root / "unused-host"), "profiles": [{"id": "grok", "source": "grok",
                "executable": sys.executable, "args": [str(Path(__file__).resolve()), "--fake-grok"],
                "env": {"HOME": str(self.root / "private-home"), "GROK_HOME": str(self.home),
                        "GROK_SDK_FAKE_LOG": str(self.log)}}]}))

    @staticmethod
    def write_summary(path, sid, cwd, title):
        (path / "summary.json").write_text(json.dumps({"info": {"id": sid, "cwd": cwd},
            "generated_title": title, "created_at": "2026-10-18T00:00:00Z",
            "updated_at": "2026-10-18T00:00:00Z", "current_model_id": "fake-grok"}))

    @property
    def env(self):
        return {"SESSIONDOCK_GROK_ROOT": str(self.sessions),
                "SESSIONDOCK_GROK_ACTIVE": str(self.active),
                "SESSIONDOCK_LAUNCHER_CONFIG": str(self.launcher),
                "SESSIONDOCK_PROC_ROOT": str(self.proc)}

    def received(self, method=None):
        if not self.log.exists():
            return []
        rows = [json.loads(line) for line in self.log.read_text().splitlines() if line]
        return [row for row in rows if method is None or row.get("method") == method]


class FakeGrok:
    def __init__(self):
        self.home = Path(os.environ["GROK_HOME"])
        self.log = Path(os.environ["GROK_SDK_FAKE_LOG"])
        self.lock = threading.Lock()
        self.sid = None
        self.path = None
        self.cwd = None
        self.model = "fake-grok"
        self.effort = "low"
        self.cancel = threading.Event()
        self.permission = threading.Event()
        self.permission_result = None
        self.permission_id = None
        self.turn = None

    def send(self, message):
        with self.lock:
            sys.stdout.write(json.dumps(message, ensure_ascii=False) + "\n")
            sys.stdout.flush()

    def reply(self, request, result):
        self.send({"jsonrpc": "2.0", "id": request["id"], "result": result})

    def error(self, request, code, message):
        self.send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": code, "message": message}})

    def update(self, update_type, **fields):
        self.send({"jsonrpc": "2.0", "method": "session/update",
                   "params": {"sessionId": self.sid, "update": {"sessionUpdate": update_type, **fields}}})

    def settings(self):
        return {"models": {"currentModelId": self.model, "availableModels": [
                    {"modelId": "fake-grok", "name": "Fake Grok"},
                    {"modelId": "fake-other", "name": "Fake Other"}]},
                "configOptions": [{"id": "model", "name": "Model", "category": "model", "type": "select",
                    "currentValue": self.model, "options": [{"value": "fake-grok", "name": "Fake Grok"},
                        {"value": "fake-other", "name": "Fake Other"}]},
                    {"id": "reasoning_effort", "name": "Effort", "category": "thought_level", "type": "select",
                     "currentValue": self.effort, "options": [{"value": "low", "name": "Low"},
                        {"value": "high", "name": "High"}]}]}

    def handle(self, message):
        # Record exact protocol plus child launch argv, for assertions. Never
        # record the process environment or read any credentials.
        with self.log.open("a") as log:
            log.write(json.dumps(message) + "\n")
        method, params = message.get("method"), message.get("params", {})
        if method is None:
            if message.get("id") == self.permission_id:
                self.permission_result = message.get("result")
                self.permission.set()
            return
        if method == "initialize":
            assert sys.argv[-3:] == ["agent", "--no-leader", "stdio"], sys.argv
            self.reply(message, {"protocolVersion": 1, "agentCapabilities": {"loadSession": True},
                "_meta": {"modelState": self.settings()["models"]}})
        elif method in {"session/new", "session/load"}:
            assert params.get("_meta", {}).get("yoloMode") is False
            assert params.get("_meta", {}).get("autoMode") is False
            self.cwd = params["cwd"]
            self.sid = params.get("sessionId") or str(uuid.uuid4())
            self.path = self.home / "sessions" / quote(self.cwd, safe="-_.~") / self.sid
            if method == "session/new":
                self.path.mkdir(parents=True)
                GrokFixture.write_summary(self.path, self.sid, self.cwd, "新建 Grok 会话")
            elif not (self.path / "summary.json").exists():
                self.error(message, -32602, "Synthetic native session not found")
                return
            if method == "session/load":
                self.update("agent_message_chunk", content={"type": "text", "text": "Replay is ordinary history"},
                            _meta={"isReplay": True})
            self.reply(message, {**self.settings(), "sessionId": self.sid,
                                "_meta": {"currentWorkingDirectory": self.cwd}})
        elif method == "session/set_config_option":
            assert params["sessionId"] == self.sid
            value = params["value"]["value"]
            if params["configId"] == "model":
                self.model = value
            else:
                assert params["configId"] == "reasoning_effort"
                self.effort = value
                if value == "high":
                    time.sleep(0.25)  # Exercises interrupt before prompt dispatch.
            self.reply(message, self.settings())
            self.update("config_option_update", configOptions=self.settings()["configOptions"])
        elif method == "session/prompt":
            assert params["sessionId"] == self.sid
            assert self.turn is None or not self.turn.is_alive(), "duplicate native prompt"
            self.cancel.clear()
            self.permission.clear()
            self.turn = threading.Thread(target=self.safe_prompt, args=(message,), daemon=True)
            self.turn.start()
        elif method == "session/cancel":
            assert "id" not in message and params["sessionId"] == self.sid
            self.cancel.set()
            self.permission.set()
        else:
            self.error(message, -32601, "Synthetic unsupported method")

    def append(self, kind, content):
        with (self.path / "chat_history.jsonl").open("a") as out:
            out.write(json.dumps({"type": kind, "content": content}) + "\n")

    def safe_prompt(self, message):
        try:
            self.prompt(message)
        except Exception as error:
            with self.log.open('a') as log:
                log.write(json.dumps({'fixture_error':repr(error)})+'\n')
            raise

    def prompt(self, message):
        text = message["params"]["prompt"][0]["text"]
        if text == "native-error":
            self.error(message, -32000, "Synthetic model failure")
            return
        if text == "disconnect":
            os._exit(17)  # Only this test's fake native child.
        self.append("user", text)
        self.update("agent_thought_chunk", content={"type": "text", "text": "Grok synthetic reasoning"})
        self.update("agent_message_chunk", content={"type": "text", "text": "Grok streaming "})
        if text.startswith("approval"):
            call_id = "tool-" + uuid.uuid4().hex
            self.update("tool_call", toolCallId=call_id, title="Synthetic command", kind="execute",
                        status="pending", rawInput={"command": "printf synthetic"})
            self.permission_id = "permission-" + uuid.uuid4().hex
            self.send({"jsonrpc": "2.0", "id": self.permission_id, "method": "session/request_permission",
                "params": {"sessionId": self.sid, "toolCall": {"toolCallId": call_id, "title": "Synthetic permission"},
                    "options": [{"optionId": "once", "name": "Allow once", "kind": "allow_once"},
                                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]}})
            self.permission.wait(30)
            if not self.cancel.is_set():
                assert self.permission_result is not None, "bridge never answered permission"
                outcome = self.permission_result["outcome"]
                allowed = outcome.get("optionId") == "once" and outcome["outcome"] == "selected"
                output = "Grok native tool result" if allowed else "Grok tool denied"
                self.update("tool_call_update", toolCallId=call_id, status="completed" if allowed else "failed",
                            rawOutput={"output": output}, content=[{"type": "content", "content": {"type": "text", "text": output}}])
        if text == "wait-for-cancel":
            self.cancel.wait(30)
        elif text == "detached":
            self.cancel.wait(0.6)
        if self.cancel.is_set():
            self.update("agent_message_chunk", content={"type": "text", "text": "Grok interrupted"})
            self.reply(message, {"stopReason": "cancelled"})
        else:
            self.update("agent_message_chunk", content={"type": "text", "text": "Grok final answer"})
            self.append("assistant", "Grok final answer")
            self.reply(message, {"stopReason": "end_turn"})

    def run(self):
        for line in sys.stdin:
            self.handle(json.loads(line))


if __name__ == "__main__":
    assert "--fake-grok" in sys.argv
    FakeGrok().run()
