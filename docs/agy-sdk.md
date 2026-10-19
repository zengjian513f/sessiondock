# Agy 原生 SDK 模式

`agy_sdk::Bridge` 使用 Agy 的 headless NDJSON 协议，不使用 PTY、不模拟
Codex app-server、不写原生数据库或只读镜像。原生读模型合同见 [agy.md](agy.md)。
当前实现范围为 Linux；其他平台返回未启用，避免宣称具备尚未验证的外部占用检查。

## 原生机制与证据

2026-10-18 核对本机 `agy --version` 为 **1.4.2**，`agy --help` 提供
`--input-format stream-json`、`--output-format stream-json`、`--conversation`、
`--model`、`--effort`。协议字段依据官方
[headless 文档](https://www.antigravity.google/docs/cli/headless/)，
身份与存储依据 [会话管理](https://www.antigravity.google/docs/cli/conversations/)
和现有 Agy 合同。官方 Python SDK 用于自定义代理，不能据此假设它可接管 CLI 原生会话。

已用临时 HOME、显式合成模型、仅监听 loopback 的 OpenAI 兼容网关核对：

- CLI 在首条输入前发出 `init`，包含原生 `conversation_id`、`init.cwd`、
  显式选择的 `init.model`、工具目录与 permission mode。
- `init` 后进程确实持有 `conversations/<SID>.db`，没有由桥接生成 SID 或数据库。
- 输入为 `{"event":"user","message":{"content":"正文"}}`，每条独立一行。
  两轮输出使用同一 SID，回复通过 `step_update.text_delta` 到达并收到 `SUCCESS`。
  网关实际请求的 model 为显式传入的合成模型，没有账号或付费模型调用。
- 关闭 stdin 后进程结束；新进程 `--conversation <SID>` 返回相同 SID。
  对保持 stdin 打开的闲置进程发送 SIGINT 后观察到退出码 1，仍可恢复同一 SID。
- 活跃的合成网关请求被阻塞时发送 SIGINT：CLI 约 64 ms 后以码 1 退出，
  发出 `result.status=ERROR`、`error=interrupted`，随后能恢复同一 SID。
  不假设 SIGINT 一定输出 INTERRUPTED，也不将网络请求中断等同于工具子进程清理验收。

桥接每轮启动 `agy --input-format stream-json --output-format stream-json
--conversation <SID>`，写入一条原生 user 消息并关闭 stdin。原生协议规定 EOF
在当前回合完成后结束进程；下一轮用同一 SID 恢复。这样 model/effort 可以逐轮通过
独立 argv 传入，无需虚构在线设置 RPC。CLI 的持久多轮协议也已观察到，但桥接采用
逐轮恢复，承担启动开销，不宣称共享一个常驻 TUI 运行时。

输出处理：`step_update` 按原生 SID/step index 累积 `text_delta`；工具保留
`tool_info.parameters/output/error`，后续稀疏 DONE 事件不会擦掉先前参数。
工具提供读模型使用的 `input`、`result`、`error`，并另保留 `output`、`tool_error`
及原始事件。未知步骤保留为系统消息；`result` 保留终态错误，缺少流式正文时使用
原生 result.response。协议未提供独立 thinking 正文时不制造 thinking 消息。

## 能力和限制

| 能力 | 行为 |
| --- | --- |
| open | 使用 inventory 已解析的 SID/cwd；原生数据库缺失时不从镜像伪造恢复载体 |
| create | 无提示词启动，读取原生 init 的 SID/cwd，关闭空输入；不调用模型来换取身份 |
| turn | 服务端逐会话 FIFO；重复 request_id 返回原回执，不重复发送 |
| interrupt | SIGINT 只发送给本桥接持有的 Child；5 秒未退出才结束该桥接进程 |
| respond / 审批 / 问题卡 | capability 为 false，respond 明确报 unsupported |
| models | profile 环境中执行原生 `agy models`，解析 TSV；失败不填猜测模型 |
| discard | 仅确认可丢弃网页创建回执；保留原生文件及已经接受的任务 |

官方 headless 文档的 Unsupported messages 明确说明，`control_request` 和
`control_response` 会以 ERROR 结束进程。审批按 CLI 自己配置的策略执行；需要交互
批准而无法取得的工具会被软拒绝。桥接不增加 `--dangerously-skip-permissions`，
不改权限配置，不将工具目录里的 ask_permission/ask_question 冒充可回答的请求卡。
`requests` 因此为空。已有 profile 的固定参数和原生权限配置仍由管理员负责。

独立 TUI 不共享这个 headless 运行时。打开及每轮发送前检查原生数据库的 fd/inode
占用，排除本桥接持有的子进程；发现外部持有者即报 `agy_sdk_session_busy`，
不杀进程、不接管。该检查没有跨独立 CLI 的原子租约，无法防止外部 TUI 在检查后
同时打开数据库；使用 SDK 时应先正常退出同一 SID 的独立 TUI。此限制不能表述为
支持原生共享写入或完整互斥保证。其他 SID 不受影响。

接收后的进程不依赖 SSE 连接或网页存活。读写失败、native ERROR、进程异常退出和
中断保留可见错误，不自动重发已交给 CLI 的正文。shutdown 只清理自己启动的进程。
stderr 保存在实时快照的 `diagnostics`；已知敏感环境值及常见凭据字段先做遮盖，
不把原始 stderr 打到服务日志。启动、模型目录、关闭空输入和中断清理有生命周期
超时；正文、NDJSON 行长、队列条数与消息总量没有新增拒绝上限。

## 共享接入

公开入口：`new(&Config, CancellationToken) -> io::Result<Option<Arc<Bridge>>>`，
`Bridge::call(method, target, input)`，`Bridge::subscribe(key)`。
`lib.rs` 注册该模块，由共享 `native_sdk` facade 和 HTTP/SSE 路由调用。

`new` 仅读取配置。必须同时存在 agy root、agy_home、Agy launcher profile 才启用；
只配置 root 而没有 profile 不运行 CLI。CLI HOME 必须对应配置的原生数据目录，
否则返回明确配置错误，由 facade 降级该能力；不回退到另一个 HOME。

`target` 使用 inventory 已解析的 `{uid,session_id,cwd,agent}`，create 使用
`{source:"agy",cwd}`。`call` 支持 open/create/turn/interrupt/respond/models/discard/usage/settings。
create 必须提供 request_id，成功及失败回执在桥接进程内去重；持久 pending 列表、
草稿身份与跨重启创建回执由共享 facade 保存。discard 不删除原生数据库。

**身份注意**：现有 Agy inventory 的 UID 是
`uid_for("agy", <AGY_ROOT>/cli/<native SID>)`，而不是原生二进制 DB 路径的 UID。
create 沿用这一现有身份算法，避免第一条原生索引出现后换 UID；SID 来自真实 init，
另返回 `session.native_path` 指明真实 `conversations/<SID>.db`。桥接不创建 UID 所用的
镜像目录或文件。该 UID 与现有 inventory 合同保持一致。

订阅 key：主会话为 target.uid；agent 非空时为 `target.uid + ":" + target.agent`。
不同 key 指向同一个原生 SID 时复用同一 actor，子代理绝不替换成父会话 SID。
同 SID/cwd 身份发生不一致时返回 identity_changed。

snapshot 与 receiver 在同一发布锁中取得；每次发布完整快照：

```json
{"seq":1,"type":"snapshot","raw":{"session_id":"native SID","cwd":"workspace","running":true,"items":[],"requests":[],"model":null,"effort":null,"error":null,"diagnostics":"","capabilities":{}}}
```

`items` 仅为当前桥接接受/观察的实时消息，与普通历史区独立。原生步骤消息的
稳定 ID 为 `agy:<SID>:step:<step_index>`；接受的用户输入 ID 使用 request_id，
收到原生 user_input 时另加 native_id。序号在当前 live session 内递增。
广播 lag 后 facade 应使用 bridge/reset 触发重新订阅，不能把缺口解释为任务结束。
实时消息与正在执行的 child 不跨节点服务重启。共享 facade 的
[持久输入账本](native-sdk.md#持久输入与失败恢复) 保存尚未投递的队列、提交去重与
失败/未知回执；在途工作重启后须核对，不自动重发。关联的原生 user_input 提供
消费证据，不能把准入当消费。原生历史仍由既有 inventory/read-model 读取。

## 验证

[agy_sdk_browser.py](../tests/agy_sdk_browser.py) 使用临时原生目录和合成协议对端，
通过 Chromium 实际操作 SDK 页面，覆盖本来源的新建/恢复、发送、流式消息和工具、
去重、中断、重开及外部 writer 冲突；审批和问题按本来源声明的能力验收。
共享 Hub、草稿恢复与网页请求卡路径见
[native_sdk_hub_browser.py](../tests/native_sdk_hub_browser.py)。
完整验证命令与套件入口见[验证清单](validation.md)，协议实验的实机范围仍以上述证据为准。

## 本轮补齐

DELTA：SDK 底栏展示原生 result.usage 累计 Token，step_update.usage 在明细单独
标注，不相加，也不伪装为上下文百分比。额度复用独立 print /usage collector，
只查询 Agy；打开/手动刷新取额度，失败保留旧快照和时间，不开启模型 turn。
原生目录提供 low/medium/high/xhigh/max 独立 effort。settings.mode 可显式选择
accept-edits/auto/plan，显示下一轮生效，并以独立 --mode argv 传入，不制造在线控制 RPC。
网页 /usage、/model、/effort 与正文分流，未知 slash 保留草稿，不破坏 NDJSON。
图片维持路径引用，明确提示不传图像内容；headless 审批回传限制仍有效。

扩展 Chromium 覆盖 effort/mode 的实际 argv、累计用量/额度查询、slash 不入原生
user 流，及 1280/390/320 明暗默认、用量和设置菜单；合成 CLI 不访问真实账户。
