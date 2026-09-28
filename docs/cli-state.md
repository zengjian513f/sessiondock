# 会话 CLI 状态对象

父合同：[会话草稿与 SEND](conversation.md)、[对话输入就绪](composer-input.md)。本文定义服务端为每个 AI 会话维护的 **CLI 状态对象**：原生历史（JSONL）之外、只有 CLI 进程和它的画面才知道的事。前端只读原生历史和这个对象，不直接解释 PTY 画面；对象对所有 agent 同一形状，页面不按 source 分支。

## 形状

`cli` 字段随主会话视图一起下发（`GET /api/messages/{uid}`、`/api/watch` 数据包、`POST /api/session/conversation/check` 响应）；子代理视图没有该字段。服务尚未识别过该会话（没有 SEND、CHECK 或观察）时为 JSON `null`。

```json
{
  "observed_at": 1759056000.123,
  "instance": {"running": true, "busy": false},
  "input": {"state": "ready", "code": "", "message": ""},
  "editor": {"text": "粘贴的时候…"},
  "queued": [
    {"request_id": "…", "text": "…", "echo_hash": "…", "sent_at": 1759055990.5, "state": "queued"}
  ]
}
```

| 字段 | 含义 |
| --- | --- |
| `observed_at` | 最近一次成功读到画面的 Unix 秒；从未读到为 `null` |
| `instance.running` | 最近一次读画面成功为 `true`，失败（实例已退出、宿主不可达）为 `false`，尚未尝试为 `null` |
| `instance.busy` | 最近一次读画面成功时画面是否显示 CLI 的忙碌指示（spinner、`esc to interrupt`，与投递前判忙同一规则；Claude、Codex）；读失败、尚未读到或 Grok、OpenCode 为 `null` |
| `input` | 与 CHECK 相同的分类结果（`ready`/`starting`/`blocked`/`unknown` 及 `code`/`message`）；最近一次读失败时为 `null` |
| `editor.text` | 识别到编辑区时的可见文字（Claude、Codex）；Grok、OpenCode 尚无提取，为 `null` |
| `queued` | 终端已接受、原生记录尚未出现的 SEND，按发送顺序；`state` 为 `queued` 或 `lost` |

题卡/审批仍是同级的 `prompt` 字段，不重复放进对象。

## 谁写

- **SEND** 成功写入 Enter 后把正文（去首尾空白）、`echo_hash` 和发送时间压入 `queued`，落在 conversation 账本，重启后仍在。多个页面、多台设备发出的消息都在同一个列表里。
- **观察**：每个订阅主会话视图的 watcher 每秒请求一次读取；1 秒内的读取由所有 watcher 和 CHECK 共用，一个会话每秒最多截一次屏。读失败后 3 秒内不重试。读取更新 `instance`、`input`、`editor`；CHECK 总是即时读取并同样写入对象。
- **回显对账**：watcher 每次拿到含正文的数据包，把其中 user/command 记录的正文摘要（与 SEND 回执相同的 SHA-256）与 `queued` 比对；摘要相同且记录时间不早于发送时间 5 秒的记录退掉一条排队项，一条记录只能退一条。没有时间的记录（Grok）只按摘要匹配。
- **丢失**：实例连续 30 秒读不到画面（已退出、宿主不可达）时，`queued` 全部标为 `lost`。用户可用 `POST /api/session/conversation/queued/dismiss {uid, request_id}` 关闭一条；不自动重发。

## 前端

- 排队项在对话末尾（活动状态行之后）按用户气泡显示，标注"已发送，等待 CLI 处理"；`lost` 项标注"未送达，请到终端查看"并提供关闭。新建会话的等待页把它们放在等待文案之下。
- 会话运行中时，左栏与会话头的运行点按回合状态显示：轮转中外圈扩散、等待回答为琥珀色、空闲静止。正在看的会话以 `instance.busy` 为准（对话 `activity` 为 `waiting` 时优先显示等待），其他会话用列表行的 `turn`（[read-model.md](read-model.md)）。
- 发送按钮不因等待回显而转圈；对象里 `input` 非空时按它更新输入就绪提示，与 CHECK 轮询结果同源。
- `cli` 只随数据包变化时推送（`{"cli_only": true, "cli": …}`），页面不轮询它；没有该字段的旧节点退化为只看 CHECK 响应。

## 测试

| 范围 | 文件 |
| --- | --- |
| 忙碌 SEND 排队气泡、CHECK 响应携带 `cli.queued`、回显退掉 | [send_browser.py](../tests/send_browser.py) |
| 刷新后排队气泡由服务端恢复、Grok 无时间记录按摘要退掉 | [grok_send_echo_browser.py](../tests/grok_send_echo_browser.py) |
