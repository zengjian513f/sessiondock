"""Private rollouts and a daemon that writes settings when a live thread is attached."""
# run_validation: skip
from datetime import datetime
import os

from codex_sdk_browser import THREAD
from codex_sdk_fixtures import FakeAppServer
from history_fixtures import Corpus, batch35_agent_meta, codex_message, codex_row, encoded

OLD = "2026-09-11T10:00:00.000Z"
NEW = "2026-09-14T10:00:00.000Z"
SETTINGS = "2026-10-20T08:52:34.186Z"
CHILD = "activity-child"


def at(row, stamp):
    return {**row, "timestamp": stamp}


def maintenance():
    return [
        at(codex_row("event_msg", {"type": "thread_settings_applied"}), SETTINGS),
        at(codex_row("turn_context", {"model": "fake-sol"}), SETTINGS),
        at(codex_row("event_msg", {"type": "token_count", "padding": "x" * 600000}), SETTINGS),
        at(codex_row("token_usage_record", {"total_tokens": 20}), SETTINGS),
    ]


def corpus(root):
    data = Corpus(root)
    for source in ("claude", "codex", "grok"):
        (root / source).mkdir()
    for sid, native, stamp, title, cwd in [
        ("old", THREAD, OLD, "synthetic 旧会话：检查服务流量与长期运行状态", "/synthetic/sdk"),
        ("neighbour", "activity-neighbour", "2026-09-12T10:00:00.000Z",
         "synthetic 较新会话：迁移计划与风险复核", "/synthetic/sdk"),
        ("other", "activity-other", "2026-09-13T10:00:00.000Z",
         "synthetic 另一项目：记录发布检查结果", "/synthetic/other"),
    ]:
        rows = [
            at(codex_row("session_meta", {"id": native, "cwd": cwd, "timestamp": stamp}), stamp),
            at(codex_row("turn_context", {"model": "fake-sol"}), stamp),
            at(codex_row("event_msg", {"type": "task_started"}), stamp),
            at(codex_message("user", title), stamp),
            at(codex_message("assistant", "synthetic 已完成的历史答复"), stamp),
            at(codex_row("event_msg", {"type": "task_complete"}), stamp),
        ]
        data.put(sid, "codex", rows, [])
        seconds = datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp()
        os.utime(data.paths[sid], (seconds, seconds))
    meta = batch35_agent_meta(CHILD, THREAD, "2026-09-10T10:00:00Z")
    data.put(CHILD, "codex", [
        meta, at(codex_message("assistant", "synthetic 子代理历史"), "2026-09-10T10:00:00Z"),
    ], [], parent="old")
    return data


class ActivityDaemon(FakeAppServer):
    async def _handle(self, writer, message):
        if message.get("method") == "thread/loaded/list":
            await self.send(writer, {"id": message["id"], "result": {"data": [THREAD]}})
            return
        if message.get("method") == "thread/resume":
            # Actual daemon reconnects append thread_settings_applied without
            # user input. Telemetry may put the real activity beyond tail windows.
            with self.rollout.open("ab") as stream:
                stream.write(b"".join(map(encoded, maintenance())))
        await super()._handle(writer, message)
