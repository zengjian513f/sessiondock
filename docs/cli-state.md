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
    {"request_id": "…", "text": "…", "echo_hash": "…", "sent_at": 1759055990.5, "state": "queued", "cli_queued_at": null}
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
| `queued[].cli_queued_at` | 原生历史记录该正文进入 CLI 自己输入队列的时间（Claude `queue-operation` enqueue）；尚未看到为 `null` |

题卡/审批仍是同级的 `prompt` 字段，不重复放进对象。

## 谁写

- **SEND** 成功写入 Enter 后把正文（去首尾空白）、`echo_hash` 和发送时间压入 `queued`，落在 conversation 账本，重启后仍在。多个页面、多台设备发出的消息都在同一个列表里。
- **观察**：每个订阅主会话视图的 watcher 每秒请求一次读取；1 秒内的读取由所有 watcher 和 CHECK 共用，一个会话每秒最多截一次屏。读失败后 3 秒内不重试。读取更新 `instance`、`input`、`editor`；CHECK 总是即时读取并同样写入对象。
- **回显对账**：watcher 每次拿到含正文的数据包，把其中 user/command 记录的正文摘要（与 SEND 回执相同的 SHA-256）与 `queued` 比对；摘要相同且记录时间不早于发送时间 5 秒的记录退掉一条排队项，一条记录只能退一条。没有时间的记录（Grok）只按摘要匹配。
- **CLI 入队**：Claude 忙碌时收到输入先写 `queue-operation` enqueue，到当前步骤结束才写 remove 与 `queued_command`。同一对账把摘要相同、时间不早于发送 5 秒的 enqueue 记录写进 `cli_queued_at`，排队项保留到 user/command 记录出现（BUG-20260928-231633-9a7610）。一条 enqueue 记录只配一条排队项，已标记的项仍占用它。
- **退回**：回显退掉排队项时记住最后一条的正文（仅内存）。之后读到的编辑区文字与它或仍在排队的正文忽略空白后相同，且分类为 `cli_input_pending`，则 `input` 改为 `cli_input_returned`。原生历史里这条输入之后已有回答（assistant、thinking、工具）时不算退回（见下条），记忆随之清掉。
- **终端回滚**：Claude Code 2.1.284 的双 Esc "恢复对话"只改进程内的叶子，下一条输入之前 JSONL 一字不写；被回滚的那条输入回到编辑区，画面重画到它之前（BUG-20260929-075643-bc6def）。watcher 每次读屏后（编辑区文字、分类或主视图变化时）用编辑区上方的画面核对 `conversation/rewind.rs`：编辑区文字与视图里最后一条同文的已回答用户输入 X 相同；X 及其后的记录都不在画面上；X 之前 64 条内至少一条在画面上。可见的判定只看字母和数字：长文（≥48 个）按 48 个一窗、每半窗取一段在画面里找；短的用户输入要与画面上某个 `❯` 提示行（连同缩进续行）完全相同；短的助手文字不参与。三条都满足即为终端回滚，服务端以 X 为 target 写一个 `cli: true` 的 timeline pin（与"回到此处"同一校验与固定机制，[metadata](metadata.md)），并记一条 `cli.rewind.followed` 审计事件；X 本身或其后仍在画面上（上键调出历史）只清掉退回记忆，不固定。下一条原生输入写入后 pin 按原有规则失效，以原生分支为准。
- **丢失**：实例连续 30 秒读不到画面（已退出、宿主不可达）时，`queued` 全部标为 `lost`。用户可用 `POST /api/session/conversation/queued/dismiss {uid, request_id}` 关闭一条；不自动重发。

## 前端

- 排队项在对话末尾（活动状态行之后）按用户气泡显示，标注"已发送，等待 CLI 处理"；有 `cli_queued_at` 时改为"已进入 CLI 队列，当前步骤结束后处理"；`lost` 项标注"未送达，请到终端查看"并提供关闭。新建会话的等待页把它们放在等待文案之下。
- `input.code` 为 `cli_input_returned`（编辑区里是被 Esc 退回的上一条，[对话输入就绪](composer-input.md)）时，对话里最后一条用户气泡标注"已被 Esc 退回终端输入框，CLI 未处理"；编辑区清空或重发后标注随状态撤掉。
- 终端回滚写下的 `cli: true` pin 让视图只显示到回滚点，会话头下一行提示"已同步终端里的回滚，显示到回滚点为止"，不提供取消按钮；失效后提示消失。输入框的双 Esc 按钮只把终端揭示出来，回滚由同一观察跟上。
- 会话运行中时，左栏与会话头的运行点按回合状态显示：轮转中外圈扩散、等待回答为琥珀色、空闲静止。正在看的会话以 `instance.busy` 为准（对话 `activity` 为 `waiting` 时优先显示等待；有子代理或列表行 `background` 后台任务在跑时不显示空闲），其他会话用列表行的 `turn`（[read-model.md](read-model.md)）。
- 发送按钮不因等待回显而转圈；对象里 `input` 非空时按它更新输入就绪提示，与 CHECK 轮询结果同源。
- 前端拒绝较旧 `observed_at` 的推送覆盖较新的 CLI 状态。只带 `cli.input` 的推送不表示 CHECK 题卡消失：状态仍为 `cli_question` 时保留现有 composer 题卡与待处理答案；CHECK 显式返回 `prompt: null` 或输入状态离开选择界面时才撤下。回答控件仍必须自行再检当前画面。
- `cli` 只随数据包变化时推送（`{"cli_only": true, "cli": …}`），页面不轮询它；没有该字段的旧节点退化为只看 CHECK 响应。

## 测试

| 范围 | 文件 |
| --- | --- |
| 忙碌 SEND 排队气泡、CHECK 响应携带 `cli.queued`、回显退掉 | [send_browser.py](../tests/send_browser.py) |
| 终端回滚同步到对话、刷新保留、下一条输入后以原生分支为准、调出已回答的输入既不回滚也不算 Esc 退回 | [rewind_cli_browser.py](../tests/rewind_cli_browser.py) |
| 刷新后排队气泡由服务端恢复、Grok 无时间记录按摘要退掉 | [grok_send_echo_browser.py](../tests/grok_send_echo_browser.py) |
