"""Synthetic Grok ACP stdio peer and isolated launcher setup; no real CLI/account.

Run as a configured CLI prefix: python grok_sdk_fixtures.py --fake-grok agent
--no-leader stdio. The browser suite imports GrokFixture. Persistence below is
owned by the fake native peer, never by the SessionDock bridge.
"""
# run_validation: skip
from __future__ import annotations

import json
import os
import shutil
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
        self.capability_marker = self.root / "image-capability.json"
        self.capability_marker.write_text(json.dumps({"image":True}))
        self.safe_point = self.root / "native-safe-point"
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
                "executable": sys.executable, "env_remove":["LD_PRELOAD","SESSIONDOCK_TEST_CLOCK_FILE"], "args": [str(Path(__file__).resolve()), "--fake-grok"],
                "env": {"HOME": str(self.root / "private-home"), "GROK_HOME": str(self.home),
                        "GROK_SDK_FAKE_LOG": str(self.log),
                        "GROK_SDK_FAKE_CAPABILITY":str(self.capability_marker),
                        "GROK_SDK_FAKE_SAFE_POINT":str(self.safe_point)}}]}))

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
        wire_method = "_" + method if method and method.startswith("x.ai/") else method
        return [row for row in rows if method is None or row.get("method") == wire_method]


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
        self.mode = "default"
        self.extensions = True
        self.prompt_count = 0
        self.cancel = threading.Event()
        self.permission = threading.Event()
        self.permission_result = None
        self.permission_id = None
        self.turn = None
        self.interjections = {}
        with self.log.open("a") as log:
            log.write(json.dumps({"fixture_start":{"pid":os.getpid()}})+"\n")

    def send(self, message):
        with self.lock:
            sys.stdout.write(json.dumps(message, ensure_ascii=False) + "\n")
            sys.stdout.flush()

    def reply(self, request, result):
        self.send({"jsonrpc": "2.0", "id": request["id"], "result": result})

    def error(self, request, code, message):
        self.send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": code, "message": message}})

    def update(self, update_type, **fields):
        native = update_type not in {"agent_message_chunk","agent_thought_chunk","tool_call","tool_call_update","config_option_update","current_mode_update"}
        self.send({"jsonrpc": "2.0", "method": "_x.ai/session/update" if native else "session/update",
                   "params": {"sessionId": self.sid, "update": {"sessionUpdate": update_type, **fields}}})

    def settings(self):
        return {"modes":{"currentModeId":self.mode,"availableModes":[
                    {"id":"default","name":"执行"},{"id":"plan","name":"计划"}]},"models": {"currentModelId": self.model, "availableModels": [
                    {"modelId": "fake-grok", "name": "Fake Grok", "_meta":{"supportsReasoningEffort":True,"reasoningEfforts":[{"id":"low","value":"low","default":True},{"id":"high","value":"high","default":False}]}},
                    {"modelId": "fake-other", "name": "Fake Other", "_meta":{"supportsReasoningEffort":True,"reasoningEfforts":[{"id":"low","value":"low","default":True},{"id":"high","value":"high","default":False}]}}]},
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
        if method and method.startswith("x.ai/"):
            self.error(message,-32601,"ACP extension wire methods require underscore prefix")
            return
        if method and method.startswith("_x.ai/"):
            method = method[1:]
        if method is None:
            if message.get("id") == self.permission_id:
                self.permission_result = message.get("result")
                self.permission.set()
            return
        if method == "initialize":
            assert sys.argv[-3:] == ["agent", "--no-leader", "stdio"], sys.argv
            self.reply(message, {"protocolVersion": 1, "agentCapabilities": {"loadSession": True,"promptCapabilities":{"image":json.loads(Path(os.environ["GROK_SDK_FAKE_CAPABILITY"]).read_text())["image"]}},
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
            settings = {} if json.loads(Path(os.environ["GROK_SDK_FAKE_CAPABILITY"]).read_text()).get("omit_settings") else self.settings()
            if json.loads(Path(os.environ["GROK_SDK_FAKE_CAPABILITY"]).read_text()).get("omit_model_option") and settings:
                settings["configOptions"] = [option for option in settings["configOptions"] if option["id"] != "model"]
            self.reply(message, {**settings, "sessionId": self.sid,
                                "_meta": {"currentWorkingDirectory": self.cwd}})
        elif method == "x.ai/interject":
            assert params["sessionId"] == self.sid and self.turn and self.turn.is_alive()
            ident = params["interjectionId"]
            assert ident not in self.interjections,"bridge replayed native interjection"
            blocks = params.get("content",[])
            if blocks:
                assert blocks[0] == {"type":"text","text":params["text"]}
                assert all(block["type"] == "image" and block["mimeType"] == "image/png" for block in blocks[1:])
            self.interjections[ident] = params
            if params["text"] == "steer-native-reject":
                self.error(message,-32601,"Synthetic unsupported interject")
                return
            self.send({"jsonrpc":"2.0","method":"_x.ai/session/interjection",
                "params":{"sessionId":self.sid,"text":params["text"],"interjectionId":ident}})
            if params["text"] == "steer-native-disconnect":
                os._exit(19)
            self.reply(message,{"result":{"status":"queued"}})
        elif method == "session/set_mode":
            assert params["sessionId"] == self.sid and params["modeId"] in {"default","plan"}
            self.mode = params["modeId"]
            self.reply(message, self.settings())
            self.update("current_mode_update",currentModeId=self.mode)
        elif method == "x.ai/session/rename":
            summary=json.loads((self.path/"summary.json").read_text())
            summary["generated_title"]=params["title"]
            (self.path/"summary.json").write_text(json.dumps(summary))
            self.reply(message,{})
        elif method in {"x.ai/session/usage","x.ai/session/info","x.ai/billing","x.ai/rewind/points","x.ai/compact_conversation","x.ai/rewind/execute","x.ai/session/fork"}:
            if not self.extensions:
                self.error(message,-32601,"Synthetic unsupported extension")
                return
            if method == "x.ai/session/usage":
                self.reply(message,{"usage":{"inputTokens":1200,"outputTokens":300,"totalTokens":1500,
                    "cachedReadTokens":200,"cacheCreationTokens":50,"reasoningTokens":100,"numTurns":self.prompt_count,
                    "costUsdTicks":250000000,"costIsPartial":False,"usageIsIncomplete":False}})
            elif method == "x.ai/session/info":
                self.reply(message,{"result":{"sessionId":self.sid,"cwd":self.cwd,"context":{"used":32000,"total":128000,"usagePct":25}}})
            elif method == "x.ai/billing":
                self.reply(message,{"config":{"creditUsagePercent":42.5,"currentPeriod":{"start":"2026-10-19T00:00:00Z","end":"2026-10-26T00:00:00Z"}},"onDemandEnabled":False})
            elif method == "x.ai/rewind/points":
                self.reply(message,{"rewind_points":[{"prompt_index":0,"created_at":"2026-10-19T00:00:00Z","num_file_snapshots":0,"has_file_changes":False,"prompt_preview":"first synthetic prompt"}]})
            elif method == "x.ai/compact_conversation":
                assert params["sessionId"] == self.sid
                self.update("auto_compact_started",tokens_used=1500,context_window=128000)
                self.update("auto_compact_completed",tokens_after=300)
                self.reply(message,{})
            elif method == "x.ai/rewind/execute":
                assert params["sessionId"] == self.sid and params["mode"] == "conversation_only" and params["force"] is False
                assert params["targetPromptIndex"] == 0
                (self.path / "chat_history.jsonl").write_text("")
                self.reply(message,{"success":True,"mode":"conversation_only"})
            elif method == "x.ai/session/fork":
                assert params["sourceSessionId"] == self.sid and params["sourceCwd"] == self.cwd
                sid = str(uuid.uuid4())
                path = self.home / "sessions" / quote(params["newCwd"],safe="-_.~") / sid
                shutil.copytree(self.path,path)
                GrokFixture.write_summary(path,sid,params["newCwd"],"Synthetic Grok fork")
                if "targetPromptIndex" in params:
                    assert params["targetPromptIndex"] == 0
                    (path / "chat_history.jsonl").write_text("")
                self.reply(message,{"newSessionId":sid,"newCwd":params["newCwd"],"parentSessionId":self.sid,
                    "chatMessagesCopied":0,"updatesCopied":0,"planStateCopied":False})
        elif method == "session/set_config_option":
            assert params["sessionId"] == self.sid
            value = params["value"]
            assert isinstance(value,str), "installed ACP SessionConfigOptionValue is a scalar string"
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
        if text == "generalize-error-exit":
            self.error(message, -32000, "Synthetic retained native failure")
            time.sleep(.1)
            os._exit(17)
        if text == "generalize-progress":
            gate=self.log.with_name("generalize-release")
            for phase in ("retry","compact","model"):
                if phase=="retry":self.update("retry_state",type="retrying",attempt=2,maxRetries=4,reason="synthetic capacity retry")
                elif phase=="compact":self.update("auto_compact_started",tokens_used=1500,context_window=128000,reason="synthetic threshold")
                else:
                    self.update("auto_compact_completed",tokens_after=300)
                    self.update("model_changed",model_id="fake-other",reasoning_effort="high")
                deadline=time.monotonic()+20
                while time.monotonic()<deadline and not self.cancel.wait(.02):
                    if gate.exists() and gate.read_text()==phase:break
        if text == "native-error":
            self.error(message, -32000, "Synthetic model failure")
            return
        if text == "disconnect":
            os._exit(17)  # Only this test's fake native child.
        self.prompt_count += 1
        blocks = message["params"]["prompt"]
        if blocks[0].get("_meta",{}).get("bash_command"):
            assert blocks[0]["_meta"]["bash_command"] == text
            self.update("tool_call",toolCallId="shell-"+uuid.uuid4().hex,title="shell",kind="execute",status="completed",
                        rawInput={"command":text},rawOutput={"output":"Grok direct shell output"})
            self.reply(message,{"stopReason":"end_turn"})
            return
        if len(blocks)>1:
            assert blocks[1]["type"] == "image" and blocks[1]["mimeType"] == "image/png" and blocks[1]["data"]
        if text == "disable-extensions":
            self.extensions = False
        self.append("user", text)
        if text == "background-task":
            self.update("task_backgrounded",task_id="synthetic-bg",tool_call_id="bg-tool",command="synthetic daemon",cwd=self.cwd,output_file="private-synthetic-output")
            self.update("background_tasks",tasks=[{"task_id":"synthetic-bg","status":"running"}],truncated=False)
        elif text == "clear-background":
            self.update("background_tasks",tasks=[],truncated=False)
        self.update("agent_thought_chunk", content={"type": "text", "text": "Grok synthetic reasoning"})
        self.update("agent_message_chunk", content={"type": "text", "text": "Grok streaming "})
        if text in {"question","question-plan","plan-approval"}:
            self.permission_id = "interaction-" + uuid.uuid4().hex
            if text == "plan-approval":
                method = "_x.ai/exit_plan_mode"
                params = {"sessionId":self.sid,"toolCallId":"plan-exit","planContent":"# Synthetic native plan\nKeep files unchanged"}
            else:
                method = "_x.ai/ask_user_question"
                params = {"sessionId":self.sid,"toolCallId":"question","mode":"plan" if text == "question-plan" else "default",
                    "questions":[{"question":"Which database?","options":[{"label":"Redis","description":"Memory"},{"label":"Postgres","description":"Relational"}],"multiSelect":False},
                                 {"question":"Which features?","options":[{"label":"Search","description":"Find"},{"label":"Cache","description":"Reuse"}],"multiSelect":True}]}
            self.send({"jsonrpc":"2.0","id":self.permission_id,"method":method,"params":params})
            self.permission.wait(30)
            assert self.permission_result is not None,"bridge never answered question/plan"
            outcome = self.permission_result["outcome"]
            if text == "plan-approval":
                assert outcome in {"approved","cancelled","abandoned"}
            elif outcome == "accepted":
                assert self.permission_result["answers"]["Which database?"] == ["Postgres"]
                assert self.permission_result["answers"]["Which features?"] == ["Search","Cache"]
            else:
                assert outcome in {"cancelled","chat_about_this","skip_interview"}
            self.update("agent_message_chunk",content={"type":"text","text":"Native interaction " + outcome})
        if text.startswith("approval"):
            call_id = "tool-" + uuid.uuid4().hex
            self.update("tool_call", toolCallId=call_id, title="Synthetic command", kind="execute",
                        status="in_progress", rawInput={"command": "printf synthetic"},
                        rawOutput={"output":["partial native stdout"],"exit_code":0},
                        content=[{"type":"content","content":{"type":"text","text":"partial native stdout"}}])
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
        if text == "steer-active":
            deadline = time.monotonic() + 60
            marker = Path(os.environ["GROK_SDK_FAKE_SAFE_POINT"])
            while not marker.exists() and not self.cancel.is_set() and time.monotonic() < deadline:
                self.cancel.wait(0.02)
            if not self.cancel.is_set():
                for ident,interjection in list(self.interjections.items()):
                    if interjection["text"] in {"steer-native-reject","steer-native-disconnect"}: continue
                    self.append("user",interjection["text"])
                    self.update("agent_message_chunk",content={"type":"text","text":"Native safe point consumed: " + interjection["text"]})
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
            self.reply(message, {"stopReason": "end_turn","_meta":{"usage":{"inputTokens":100,"outputTokens":20,
                "totalTokens":120,"costUsdTicks":999,"usageIsIncomplete":True,"costIsPartial":True}}})
            if text=="generalize-success-exit":
                time.sleep(.1)
                os._exit(17)

    def run(self):
        for line in sys.stdin:
            self.handle(json.loads(line))


if __name__ == "__main__":
    assert "--fake-grok" in sys.argv
    FakeGrok().run()
