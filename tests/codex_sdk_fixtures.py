"""Scripted Codex app-server over a Unix socket for the SDK-mode browser suite.

Speaks the same transport as `codex app-server --listen unix://`: an HTTP
Upgrade to WebSocket, then one JSON-RPC message (without "jsonrpc") per text
frame. Standard library only; it never starts or contacts a real Codex. Each
turn appends user/assistant rows to the synthetic rollout, like the daemon.
"""
# run_validation: skip
from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import struct
import threading
import time

from history_fixtures import codex_message, encoded

GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


class FakeAppServer:
    def __init__(self, socket_path, rollout, thread_id):
        self.socket_path = str(socket_path)
        self.rollout = rollout
        self.thread_id = thread_id
        self.received = []          # every client message, in order
        self.loop = asyncio.new_event_loop()
        self.ready = threading.Event()
        self.clients = []
        self.turn = None
        self.request_seq = 0
        self.waiting = {}           # server request id -> continuation
        self.thread = threading.Thread(target=self._run, daemon=True)

    # ------------------------------------------------------------ lifecycle
    def __enter__(self):
        self.thread.start()
        assert self.ready.wait(10), "fake app-server did not start"
        return self

    def __exit__(self, *exc):
        self.loop.call_soon_threadsafe(self.loop.stop)
        self.thread.join(5)

    def _run(self):
        asyncio.set_event_loop(self.loop)
        self.loop.run_until_complete(asyncio.start_unix_server(self._client, path=self.socket_path))
        self.ready.set()
        self.loop.run_forever()

    def methods(self, name):
        return [m for m in list(self.received) if m.get("method") == name]

    def responses(self):
        return [m for m in list(self.received) if "method" not in m and "id" in m]

    def wait_for(self, predicate, timeout=15):
        deadline = time.time() + timeout
        while time.time() < deadline:
            found = predicate()
            if found:
                return found
            time.sleep(0.05)
        raise AssertionError("fake app-server condition not met; received=" + json.dumps(self.received)[-2000:])

    # ------------------------------------------------------------ transport
    async def _client(self, reader, writer):
        head = await reader.readuntil(b"\r\n\r\n")
        key = ""
        for line in head.decode("latin1").split("\r\n"):
            if line.lower().startswith("sec-websocket-key:"):
                key = line.split(":", 1)[1].strip()
        accept = base64.b64encode(hashlib.sha1((key + GUID).encode()).digest()).decode()
        writer.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                      f"Sec-WebSocket-Accept: {accept}\r\n\r\n").encode())
        await writer.drain()
        self.clients.append(writer)
        try:
            while True:
                text = await self._frame(reader, writer)
                if text is None:
                    break
                message = json.loads(text)
                self.received.append(message)
                await self._handle(writer, message)
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        finally:
            if writer in self.clients:
                self.clients.remove(writer)

    async def _frame(self, reader, writer):
        payload = b""
        while True:
            b1, b2 = await reader.readexactly(2)
            opcode, length = b1 & 0x0F, b2 & 0x7F
            if length == 126:
                length = struct.unpack(">H", await reader.readexactly(2))[0]
            elif length == 127:
                length = struct.unpack(">Q", await reader.readexactly(8))[0]
            mask = await reader.readexactly(4) if b2 & 0x80 else b"\0\0\0\0"
            data = bytes(c ^ mask[i % 4] for i, c in enumerate(await reader.readexactly(length)))
            if opcode == 8:
                return None
            if opcode == 9:
                await self._write(writer, data, opcode=10)
                continue
            payload += data
            if b1 & 0x80:
                return payload.decode()

    async def _write(self, writer, data, opcode=1):
        header = bytes([0x80 | opcode])
        if len(data) < 126:
            header += bytes([len(data)])
        elif len(data) < 65536:
            header += bytes([126]) + struct.pack(">H", len(data))
        else:
            header += bytes([127]) + struct.pack(">Q", len(data))
        writer.write(header + data)
        await writer.drain()

    async def send(self, writer, message):
        await self._write(writer, json.dumps(message).encode())

    async def notify(self, method, params):
        params = {"threadId": self.thread_id, **params}
        for writer in list(self.clients):
            await self.send(writer, {"method": method, "params": params})

    # ------------------------------------------------------------ protocol
    async def _handle(self, writer, message):
        method, mid, params = message.get("method"), message.get("id"), message.get("params") or {}
        if method is None:
            continuation = self.waiting.pop(mid, None)
            if continuation:
                await continuation(message.get("result") or {})
            return
        if mid is None:
            return
        reply = lambda result: self.send(writer, {"id": mid, "result": result})
        if method == "initialize":
            await reply({"userAgent": "fake/0", "codexHome": "/synthetic", "platformFamily": "unix", "platformOs": "linux"})
        elif method == "thread/resume":
            if params.get("threadId") != self.thread_id:
                await self.send(writer, {"id": mid, "error": {"code": -32602, "message": "no rollout found"}})
                return
            await reply({"thread": {"id": self.thread_id, "status": {"type": "idle"}, "turns": [], "name": None}})
        elif method == "model/list":
            await reply({"data": [
                {"id": "fake-sol", "model": "fake-sol", "displayName": "Fake Sol", "hidden": False, "isDefault": True,
                 "supportedReasoningEfforts": [{"reasoningEffort": "low", "description": "fast"},
                                               {"reasoningEffort": "high", "description": "deep"}],
                 "defaultReasoningEffort": "low"},
                {"id": "fake-luna", "model": "fake-luna", "displayName": "Fake Luna", "hidden": False, "isDefault": False,
                 "supportedReasoningEfforts": [{"reasoningEffort": "medium", "description": "mid"}],
                 "defaultReasoningEffort": "medium"}], "nextCursor": None})
        elif method == "turn/start":
            text = params["input"][0]["text"]
            turn_id = f"turn-{len(self.methods('turn/start'))}"
            self.turn = turn_id
            await reply({"turn": {"id": turn_id, "status": "inProgress", "items": []}})
            asyncio.ensure_future(self._script(turn_id, text))
        elif method == "turn/steer":
            assert params.get("expectedTurnId") == self.turn, params
            await reply({"turnId": self.turn})
            text = params["input"][0]["text"]
            await self._user_item(self.turn, text, suffix="steer")
        elif method == "turn/interrupt":
            await reply({})
            turn_id, self.turn = self.turn, None
            await self.notify("thread/status/changed", {"status": {"type": "idle"}})
            await self.notify("turn/completed", {"turn": {"id": turn_id, "status": "interrupted", "items": []}})
        elif method == "thread/unsubscribe":
            await reply({"status": "unsubscribed"})
        else:
            await self.send(writer, {"id": mid, "error": {"code": -32601, "message": f"unknown {method}"}})

    async def _user_item(self, turn_id, text, suffix="user"):
        item = {"type": "userMessage", "id": f"{turn_id}-{suffix}", "content": [{"type": "text", "text": text}]}
        await self.notify("item/started", {"turnId": turn_id, "item": item})
        await self.notify("item/completed", {"turnId": turn_id, "item": item})

    async def _agent(self, turn_id, item_id, text, phase):
        await self.notify("item/started", {"turnId": turn_id, "item": {"type": "agentMessage", "id": item_id, "text": "", "phase": phase}})
        for word in text.split(" "):
            await self.notify("item/agentMessage/delta", {"turnId": turn_id, "itemId": item_id, "delta": word + " "})
            await asyncio.sleep(0.03)
        await self.notify("item/completed", {"turnId": turn_id, "item": {"type": "agentMessage", "id": item_id, "text": text + " ", "phase": phase}})

    async def _request(self, method, params, continuation):
        request_id = self.request_seq
        self.request_seq += 1
        self.waiting[request_id] = continuation
        for writer in list(self.clients):
            await self.send(writer, {"method": method, "id": request_id, "params": {"threadId": self.thread_id, **params}})
        return request_id

    async def _script(self, turn_id, text):
        await self.notify("thread/status/changed", {"status": {"type": "active", "activeFlags": []}})
        await self.notify("turn/started", {"turn": {"id": turn_id, "status": "inProgress", "items": []}})
        await self._user_item(turn_id, text)
        if text.startswith("approve"):
            await self._agent(turn_id, f"{turn_id}-c1", "Working on it", "commentary")
            command = {"type": "commandExecution", "id": f"{turn_id}-cmd", "command": "/bin/sh -lc 'echo synthetic'",
                       "cwd": "/synthetic/sdk", "status": "inProgress", "commandActions": [{"type": "unknown", "command": "echo synthetic"}],
                       "aggregatedOutput": None, "exitCode": None, "durationMs": None}
            await self.notify("item/started", {"turnId": turn_id, "item": command})
            await self.notify("thread/status/changed", {"status": {"type": "active", "activeFlags": ["waitingOnApproval"]}})

            async def approved(result, request_id=None):
                decision = result.get("decision")
                rid = self.request_seq - 1
                await self.notify("serverRequest/resolved", {"requestId": rid})
                await self.notify("thread/status/changed", {"status": {"type": "active", "activeFlags": []}})
                ok = decision == "accept"
                if ok:
                    await self.notify("item/commandExecution/outputDelta", {"turnId": turn_id, "itemId": command["id"], "delta": "synthetic\n"})
                await self.notify("item/completed", {"turnId": turn_id, "item": {**command, "status": "completed" if ok else "declined",
                                  "aggregatedOutput": "synthetic\n" if ok else None, "exitCode": 0 if ok else None, "durationMs": 12}})
                final = "All done after approval" if ok else "Command was declined"
                await self._agent(turn_id, f"{turn_id}-final", final, "final_answer")
                await self.notify("thread/tokenUsage/updated", {"turnId": turn_id, "tokenUsage": {
                    "total": {"totalTokens": 2000}, "last": {"totalTokens": 1000}, "modelContextWindow": 4000}})
                self._persist(text, final)
                self.turn = None
                await self.notify("thread/status/changed", {"status": {"type": "idle"}})
                await self.notify("turn/completed", {"turn": {"id": turn_id, "status": "completed", "items": []}})

            await self._request("item/commandExecution/requestApproval", {
                "turnId": turn_id, "itemId": command["id"], "reason": "Synthetic approval reason",
                "command": command["command"], "cwd": "/synthetic/sdk",
                "availableDecisions": ["accept", "acceptForSession", "decline", "cancel"]}, approved)
        elif text.startswith("ask"):
            async def answered(result):
                rid = self.request_seq - 1
                await self.notify("serverRequest/resolved", {"requestId": rid})
                choice = result["answers"]["color"]["answers"][0]
                await self._agent(turn_id, f"{turn_id}-answer", f"You picked {choice}", "commentary")
            await self._request("item/tool/requestUserInput", {
                "turnId": turn_id, "itemId": f"{turn_id}-ask", "isBlocking": True,
                "questions": [{"id": "color", "header": "Color", "question": "Which color?", "isOther": False,
                               "options": [{"label": "Red", "description": "warm"}, {"label": "Blue", "description": "cool"}]}]},
                answered)
            # The turn stays in flight until the page interrupts it.

    def _persist(self, user, assistant):
        with open(self.rollout, "ab") as handle:
            handle.write(encoded(codex_message("user", user)))
            handle.write(encoded(codex_message("assistant", assistant)))
