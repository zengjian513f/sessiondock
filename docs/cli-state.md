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
| `instance.busy` | 最近一次读画面成功时画面是否显示 CLI 的忙碌指示（spinner、`esc to interrupt`；Claude、Codex），或 Codex 当前编辑区上方的 `N background terminal(s) running · /ps to view · /stop to close`（N > 0）；两者之间可有 Codex 的额度提示（包括折行），其他正文仍隔断后台状态识别。后台终端可在模型回合结束后继续运行，仍算活动中，但不改变输入就绪或发送队列中断判定。历史引用和编辑区文字不算后台状态；Agy 根据已识别编辑区下方的原生页脚判断（见下文）；读失败、尚未读到、未知布局或 Grok、OpenCode 为 `null` |
| `input` | 与 CHECK 相同的分类结果（`ready`/`starting`/`blocked`/`unknown` 及 `code`/`message`）；最近一次读失败时为 `null` |
| `editor.text` | 识别到编辑区时的可见文字（Claude、Codex、Agy）；Grok、OpenCode 尚无提取，为 `null` |
| `queued` | 终端已接受、原生记录尚未出现的 SEND，按发送顺序；`state` 为 `queued`、`interrupted`、`rejected` 或 `lost` |
| `queued[].cli_queued_at` | 确认该正文进入 CLI 自己输入队列的时间：Claude 原生 enqueue 时间，或 Codex 画面首次观察时间；尚未看到为 `null` |
| `queued[].error` | `rejected` 时保存 CLI 的命令拒绝原文；其他状态可省略 |
| `queued[].command_rejections_before` | Claude 命令发送前，识别到的画面中同名未知命令警告数量；无基线的旧回执省略，不用旧画面推断结果 |

题卡/审批仍是同级的 `prompt` 字段，不重复放进对象。

Agy 1.2.16 提取已识别编辑区正文，并用只读镜像中的 native user 正文及时间参与
普通 SEND 回显对账。编辑区下方的 `esc to cancel` 页脚表示忙碌，`? for shortcuts`
及非空草稿下仅有的右对齐模型标签表示闲置；正文和草稿里的同名文本不算，未知
画面为 `null`。1.2.17 编辑区下方的已识别 running 任务栏优先表示忙碌，
即使页脚同时显示 `? for shortcuts`；任务结束回到普通页脚后按原规则判断。
行摘要不输出 `turn`；原生 DONE/IDLE 不能区分中断和正常完成。
菜单与验收边界见 [Agy](agy.md)。

## 谁写

- **SEND** 成功写入 Enter 后把正文、`echo_hash` 和发送时间压入 `queued`，落在 conversation 账本，重启后仍在。正文保留前导空白，回显摘要仍去首尾空白。多个页面、多台设备发出的消息都在同一个列表里。
- **Codex 内置命令**：按 Codex 的首行裸命令和可带参数命令规则识别实际发送正文；这些命令通过 TUI 分派，不以原命令正文写入 user/command 记录，因此不进入等待同文原生回显的 `queued`。包括 `/model` 等本地操作，以及 `/init`、`/plan 正文`、`/review 正文` 等转成其它输入或操作的命令。前导空白转义、非参数命令后同一行的正文、路径仍按普通输入对账。`sent` 只表示终端接受了发送；可用性、成功或失败、会话切换及退出仍由 CLI 决定。完整范围和解析边界见 [Codex 命令回显](codex-commands.md)。CHECK/观察通过正常账本操作撤掉旧版本误记的命令排队项（包括 `lost`），但必须有 `sent` 回执，且正文能复算出原提交摘要；旧版本已裁掉空白、包含附件/引用或没有回执而无法证明的记录保留。不重发、不修改原生历史；其它 CLI 沿用原有对账。
- **观察**：每个订阅主会话视图的 watcher 每秒请求一次读取；1 秒内的读取由所有 watcher 和 CHECK 共用，一个会话每秒最多截一次屏。读失败后 3 秒内不重试。读取更新 `instance`、`input`、`editor`；CHECK 总是即时读取并同样写入对象。
- **通知独立消费**：终端回滚检查只查看最新历史快照，不消费历史变更通知。读屏期间出现的原生记录仍须由历史分支推送并对账，不能只发送 `cli_only` 后等 HTTP 补拉或下一条回复（BUG-20261005-151728-007777）。
- **回显对账**：watcher 每次拿到含正文的数据包，把其中 user/command 记录的正文摘要（与 SEND 回执相同的 SHA-256）与 `queued` 比对；摘要相同且记录时间不早于发送时间 5 秒的记录退掉一条排队项，一条记录通常只退一条。Codex 把多条待处理输入合并为一个 user 记录时，完整正文须等于连续发送项按顺序用换行连接的正文，且每项都满足时间边界，才一起结清；不使用子串或忽略排版空白的匹配，也不要求每项都曾在 TUI 可见。已消费的合并回显与结清结果一起持久化，刷新或重启后重复读到它不能结清下一批同文输入（BUG-20261003-231725-8dad6b）。没有时间的记录（Grok）只按摘要匹配。
- **CLI 入队**：Claude 忙碌时收到输入先写 `queue-operation` enqueue，到当前步骤结束才写 remove 与 `queued_command`。同一对账把摘要相同、时间不早于发送 5 秒的 enqueue 记录写进 `cli_queued_at`，排队项保留到 user/command 记录出现（BUG-20260928-231633-9a7610）。一条 enqueue 记录只配一条排队项，已标记的项仍占用它。
- **Claude 未知命令**：Claude Code 2.1.289 的交互分派可只显示 `Unknown command: /name`（可附 `Did you mean`），不写 JSONL，不能永远等待原生回显（BUG-20261005-081802-e81d57）。SEND 在粘贴前记录同名警告数量，CHECK/观察仅从无滞后、已识别编辑区上方读取完整匹配的新警告，一条警告只配一条发送，持久化为 `rejected` 并保留错误原文；不把斜杠命令一概当失败，也不按超时或空闲推断成功。旧警告、引用行、编辑区文字不作为新增警告；实例退出后也保留明确拒绝结果。旧回执缺少发送前基线，或警告已离屏/被重画覆盖而无法证明新增时，仍保留未确认状态，用户可关闭提示。关闭只撤下回执，不取消或重发 CLI 输入。
- **Codex CLI 入队**：Codex 忙碌队列在下一次工具调用之前仅存在于 TUI，JSONL 不写入队事件。观察和 CHECK 从已识别编辑区正上方的 `Messages to be submitted after next tool call` 区块读取 `↳` 消息，与待确认发送的完整正文忽略排版空白后逐条匹配，持久化首次观察时间到 `cli_queued_at`（BUG-20260930-123331-237778）。引用的工具输出、无匹配正文、未知编辑区或有 lag 的画面不算证据；重复正文一对一匹配，已确认项仍占用可见条目。队列离屏不撤销已获得的证据，也不据此移除消息；仍等原生 user 记录退掉排队项。
- **退回**：回显退掉排队项时记住最后一条的正文（仅内存）。之后读到的编辑区文字与它或仍在排队的正文忽略空白后相同，且分类为 `cli_input_pending`，则 `input` 改为 `cli_input_returned`。原生历史里这条输入之后已有回答（assistant、thinking、工具）时不算退回（见下条），记忆随之清掉。
- **终端回滚**：Claude Code 2.1.284 的双 Esc "恢复对话"只改进程内的叶子，下一条输入之前 JSONL 一字不写；被回滚的那条输入回到编辑区，画面重画到它之前（BUG-20260929-075643-bc6def）。watcher 每次读屏后（编辑区文字、分类或主视图变化时）用编辑区上方的画面核对 `conversation/rewind.rs`：编辑区文字与视图里最后一条同文的已回答用户输入 X 相同；X 及其后的记录都不在画面上；X 之前 64 条内至少一条在画面上。可见的判定只看字母和数字：长文（≥48 个）按 48 个一窗、每半窗取一段在画面里找；短的用户输入要与画面上某个 `❯` 提示行（连同缩进续行）完全相同；短的助手文字不参与。三条都满足即为终端回滚，服务端以 X 为 target 写一个 `cli: true` 的 timeline pin（与"回到此处"同一校验与固定机制，[metadata](metadata.md)），并记一条 `cli.rewind.followed` 审计事件；X 本身或其后仍在画面上（上键调出历史）只清掉退回记忆，不固定。下一条原生输入写入后 pin 按原有规则失效，以原生分支为准。
- **丢失**：实例连续 30 秒读不到画面（已退出、宿主不可达）时，`queued` 全部标为 `lost`。用户可用 `POST /api/session/conversation/queued/dismiss {uid, request_id}` 关闭一条；不自动重发。
- **Codex 中断后未确认处理**：TUI 已确认入队的正文可能被 Esc 消费为 steer，再被 Esc 中断，原生 user 记录尚未写入（BUG-20260930-172002-bc4cf4）。观察和 CHECK 同时确认当前画面无滞后、编辑区就绪且为空、CLI 空闲、无可见队列，并读取同一会话的完整原生视图：先对账 user/command 回显；最新原生状态须为入队确认之后的 `aborted`，且不晚于读屏时间。仍无回显的对应项持久化为 `interrupted`，保留正文，标注“CLI 已中断，未确认处理，请到终端查看”并提供关闭。队列离屏、仍在工作、未知画面、缺少中断时间或发送之前的中断都不能触发；新发送不受旧中断影响，后续同文原生回显仍可退掉该项。不自动重发，也不宣称模型从未收到。

## 前端

- 排队项在对话末尾（活动状态行之后）按用户气泡显示，标注"已发送，等待 CLI 处理"；有 `cli_queued_at` 时改为"已进入 CLI 队列，当前步骤结束后处理"；`lost` 项标注"未送达，请到终端查看"；`rejected` 项显示“CLI 已拒绝命令”及原文。未确认的斜杠命令与上述终止状态可关闭提示，按钮说明明确不取消或重发 CLI 输入。新建会话的等待页把它们放在等待文案之下。
- `input.code` 为 `cli_input_returned`（编辑区里是被 Esc 退回的上一条，[对话输入就绪](composer-input.md)）时，对话里最后一条用户气泡标注"已被 Esc 退回终端输入框，CLI 未处理"；编辑区清空或重发后标注随状态撤掉。
- 终端回滚写下的 `cli: true` pin 让视图只显示到回滚点，会话头下一行提示"已同步终端里的回滚，显示到回滚点为止"，不提供取消按钮；失效后提示消失。输入框的双 Esc 按钮只把终端揭示出来，回滚由同一观察跟上。
- 会话运行中时，左栏与会话头的运行点按回合状态显示：轮转中外圈扩散、等待回答为琥珀色、空闲静止。正在看的会话以 `instance.busy` 为准（对话 `activity` 为 `waiting` 时优先显示等待；有子代理或列表行 `background` 后台任务在跑时不显示空闲），其他会话用列表行的 `turn`（[read-model.md](read-model.md)）。
- 未绑定原生记录的 SSH/代理会话以 `term/list.pending` 的运行回执同步受管状态，与普通终端列表一起判断；刷新 live、重载和服务重启后，运行中的左栏与会话头仍为蓝点，已结束的回执不亮点。回归由 [lifecycle_browser.py](../tests/lifecycle_browser.py) 默认运行的 SSH 路径实际新建、输入、重启和停止验证。
- `/api/live.working_uids` 补充明确归属的命令进程活动，优先于空闲画面与已结束回合，但不覆盖等待回答。Linux 用最近的已归属 CLI 祖先进程，或脱离父子树后保留的原生会话环境 ID，匹配当前活着的会话；不按 CPU 使用率判断。CLI 本身、空命令行的僵尸进程、`codex-code-mode-host`、`codex app-server`、`opencode serve`、`mcp serve` / `mcp-server-*` 常驻服务不计入；code-mode 下启动的命令仍计入，MCP 与 OpenCode 服务的子进程不计入。OpenCode 常驻服务继承旧会话环境 ID 也不算旧会话的活动，普通 `opencode run` 命令仍按归属计入（BUG-20261003-060744-e01ddb）。经 SSH 在其他机器上运行、归属本会话的命令（`/api/live.remote_working`，见 [liveness](liveness.md#apilive-with-the-scan)）同样计入，本机 SSH 客户端退出后仍保持运行点。Hub 保留节点命名空间，进程退出后的新采样撤销活动，旧节点或不支持进程采样的平台沿用原状态。它只影响显示，不更改 `instance.busy`、输入就绪、发送中断或进程控制权限。
- 发送按钮不因等待回显而转圈；对象里 `input` 非空时按它更新输入就绪提示，与 CHECK 轮询结果同源。
- 左栏、会话头和 composer 提示只用黄色背景问号标记等待答题，不显示感叹号。原生编辑区已有文字、Esc 退回输入、未知画面、检查失败、初始化、同步、粘贴、正常处理中及中止/退出都沿用原有状态和提示文案，不加告警图标。暂停会话的左栏和会话头显示暂停图标，与等待答题的问号共用黄色背景，悬停说明标明“会话已暂停”；暂停图标或问号优先于未读数字，未读计数仍保留在悬停说明中。
- 前端拒绝较旧 `observed_at` 的推送覆盖较新的 CLI 状态。只带 `cli.input` 的推送不表示 CHECK 题卡消失：状态仍为 `cli_question` 时保留现有 composer 题卡与待处理答案；CHECK 显式返回 `prompt: null` 或输入状态离开选择界面时才撤下。回答控件仍必须自行再检当前画面。
- `cli` 只随数据包变化时推送（`{"cli_only": true, "cli": …}`），页面不轮询它；没有该字段的旧节点退化为只看 CHECK 响应。

终端宿主 `ptyhost` 及仅继承它归属的后代也不作为启动者的命令活动
（BUG-20261006-231836-0d5270）：SSH 测试留下的终端宿主和等待输入的假 CLI
不能让已结束的发起会话持续呼吸。该排除同时用于本机 `working_uids` 与
跨机 `remote_working`，不撤销进程归属或改变宿主寿命。终端内另有明确归属的
CLI 时，它自己启动的命令仍按最近的 CLI 归属计入。相对原先只排除 MCP 等
常驻服务，这是活动判定的 **DELTA**。回归覆盖本机、Hub、刷新和服务重启。

## 测试

| 范围 | 文件 |
| --- | --- |
| 命令活动、终端宿主隔离、SSH 远端归属与刷新/重启恢复 | [process_activity_browser.py](../tests/process_activity_browser.py)、[process_links_browser.py](../tests/process_links_browser.py) |
| 忙碌 SEND 排队气泡、CHECK 响应携带 `cli.queued`、回显退掉 | [send_browser.py](../tests/send_browser.py) |
| 原生输入在读屏期间落盘，现有 SSE 在模型回复前结清队列；后续回复同样不丢通知 | [send_echo_browser.py](../tests/send_echo_browser.py) |
| Codex TUI 入队确认、折行与同文多次发送、引用画面不误认、中断后未确认处理、离屏与旧中断不误判、刷新恢复与迟到原生回显退掉、Esc 合并回显及重复快照隔离 | [send_native_codex_browser.py](../tests/send_native_codex_browser.py) |
| 终端回滚同步到对话、刷新保留、下一条输入后以原生分支为准、调出已回答的输入既不回滚也不算 Esc 退回 | [rewind_cli_browser.py](../tests/rewind_cli_browser.py) |
| 刷新后排队气泡由服务端恢复、Grok 无时间记录按摘要退掉 | [grok_send_echo_browser.py](../tests/grok_send_echo_browser.py) |
