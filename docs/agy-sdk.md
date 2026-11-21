# Agy 原生 SDK 模式

`agy_sdk::Bridge` 使用 Agy 的 headless NDJSON 协议，不使用 PTY、不模拟
Codex app-server、不写原生数据库或只读镜像。原生读模型合同见 [agy.md](agy.md)。
Linux、macOS 与 Windows 使用同一协议适配器；统一由独立 SDK 宿主持有进程，
重连附着原实例。外部数据库占用使用各平台的只读文件持有者证据。

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

桥接启动 `agy --input-format stream-json --output-format stream-json
--conversation <SID>` 后保持 stdin 打开，以原生 `result` 分隔回合。普通连续输入
复用同一进程及子代理／后台任务运行时，不再因每轮 EOF 产生原生服务重启通知。
显式更改 model/effort/mode 在当前轮完成后关闭旧进程、按新 argv 启动下一轮；
这是原生没有在线设置协议的边界，会重建子代理／后台任务运行时。独立 TUI 仍不共享此进程。

输出处理：`step_update` 按原生 SID/step index 累积 `text_delta`；工具保留
`tool_info.parameters/output/error`，后续稀疏 DONE 事件不会擦掉先前参数。
工具提供读模型使用的 `input`、`result`、`error`，并另保留 `output`、`tool_error`
及原始事件。已知 `agent_response`、`system_message` 只有完成通知而没有正文时，
按当前原生 SID 和 step index 从完整 transcript 读取正文；落盘稍迟时继续只读补齐。
思考只显示原生记录提供的内容，空通知及纯空白回复不生成助手气泡。流式分块累积时
保留换行与缩进，正文出现后一起显示。系统消息沿用普通历史的信封解析与来源标签。
未知步骤保留为系统消息；`result` 保留终态错误，缺少有效步骤正文时使用原生
`result.response` 中的有效正文。协议未提供独立 thinking 正文时不制造 thinking 消息。
DELTA：对话中的原生思考正文以无边框的「思考记录」显示一行预览，可展开全文；
收录于已完成回合时沿用「本轮过程」。思考记录的出现不改变底栏的运行阶段。

## 能力和限制

| 能力 | 行为 |
| --- | --- |
| open | 使用 inventory 已解析的 SID/cwd；原生数据库缺失时不从镜像伪造恢复载体 |
| create | 无提示词启动，读取原生 init 的 SID/cwd，关闭空输入；不调用模型来换取身份 |
| turn | 服务端逐会话 FIFO；重复 request_id 返回原回执，不重复发送 |
| steer | 调用本实例的原生 `SendUserCascadeMessage`（`MESSAGE_DELIVERY_STRATEGY_NEXT_INVOCATION`），`clientMessageId` 使用独立 `request_id`；不中断当前工具或进程，结果不确定绝不自动重发；配合持久队列支持 `queue_steer` |
| interrupt | 调用本实例的原生 `CancelCascadeInvocation`，`killBackgroundTasks=false`、`notifyParent=false`；不发送 SIGINT/SIGKILL，不因取消超时终结进程；重复请求幂等 |
| rename | 本实例的 `UpdateConversationAnnotations`，`annotations.title`、`mergeAnnotations=true`；等待读取原生摘要确认标题 |
| fork | 本实例 Connect `ForkConversation`，整会话或原生 `step:N` 用户消息之前分支，保留源会话与文件 |
| revert | 本实例 `RevertToCascadeStep`，显式 `conversationOnly:true`，保留 SID 与工作区文件 |
| respond / 审批 / 问题卡 | capability 为 false，respond 明确报 unsupported |
| models | profile 环境中执行原生 `agy models`，解析 TSV；失败不填猜测模型 |
| discard | 仅确认可丢弃网页创建回执；保留原生文件及已经接受的任务 |

DELTA（BUG-20261115-155602-e82f81）：消息操作按原生步骤身份开放分支／回退；助手回复使用下一条用户消息作为分支边界，末条回复分支整个会话。
接口由 Agy CLI 1.5.9 本地 Connect 服务执行，不向 stream-json 发送控制帧或斜杠提示词。
`ForkConversation.forkAtStepIndex` 是保留的末步；`-1` 表示整会话，因此首条输入之前分支使用原生零回合创建。
回退的 `stepIndex=-1` 则清空当前会话。写操作要求空闲、原生用户步骤及 `request_id`，持久回执防止重复创建；结果不确定不重发。
原生数据库 `steps` 是当前对话，诊断 transcript 保留已回退的后缀并复用步骤索引；私有历史投影按有效步骤及最新记录更新，数据库/WAL 变化也使缓存失效，不改写原生文件。

官方 headless 文档的 Unsupported messages 明确说明，`control_request` 和
`control_response` 会以 ERROR 结束进程。审批按 CLI 自己配置的策略执行；需要交互
批准而无法取得的工具会被软拒绝。桥接不增加 `--dangerously-skip-permissions`，
不改权限配置，不将工具目录里的 ask_permission/ask_question 冒充可回答的请求卡。
`requests` 因此为空。已有 profile 的固定参数和原生权限配置仍由管理员负责。

DELTA（BUG-20261022-124950-cf8e22）：停止按钮只取消当前原生 invocation，
保留同一进程、子代理与后台任务。此前 SIGINT 退出 CLI、随后 SIGKILL 清理的
实现违反这个语义；仅延长退出宽限期不能修复它。Agy 1.4.5 已用隔离真实 CLI
验证本机 Connect `CancelCascadeInvocation`，这不是公开 NDJSON 控制消息。
启动时给此实例传入独立随机 `--csrf_token` 和私有 `--log-file`；凭据保存在
桥接 state 的私有 control 目录，日志端口必须匹配所持有的 PID，凭据不进入诊断。
取消请求失败、超时或旧运行实例缺少凭据时显示明确错误，保留任务，不退回信号或重启。
节点服务重启复用原有凭据及取消回执；取消不是自动重放输入的理由。

DELTA（BUG-20261120-122309-518082）：stdin 写入成功后，Agy 可能尚未把输入交给会话，
此时原生取消可以返回成功而当前 invocation 仍未启动。启动期间点击停止绑定当前投递 ID，
立即返回等待接收的回执，界面显示“等待接收输入”，用户可继续排队。桥接收到该输入的原生
消费证据后才发送一次取消请求，原生拒绝时可再次点击停止。若本轮先结束或桥接退出，
等待中的停止请求随本轮结束。

DELTA（BUG-20261023-025934-a5d53c）：`/rename` 使用本机原生管理接口，
不把命令作为 print 模式提示词（原生明确不支持）。运行中及保留的闲置实例
复用相同 PID/SID；没有运行实例时零回合打开同一 SID、改名、正常关闭空输入。
只读核对原生摘要落盘后刷新目录，不直接写数据库或制造网页别名。
已验证的 Agy 1.4.6 原生接口保留 `for HTTP` 启动标签却使用自签名 HTTPS；仅在本实例端口明确
返回 HTTP-to-HTTPS 拒绝时改用本机 TLS，不重试结果未知的 mutation。
此通道只接受桥接拥有的 PID、loopback 端口和独立 CSRF 授权，不改变 Hub TLS 校验。
验收：[真实 Agy 改名浏览器](../tests/agy_rename_real_browser.py)，临时 HOME、
合成 loopback 模型，覆盖空闲／运行中改名、原生落盘、同 PID 和刷新。

DELTA（BUG-20261110-162211-7507b6）：Agy 1.5.4 已用临时 HOME 与 loopback 合成模型核对本机
Connect `SendUserCascadeMessage`（`deliveryStrategy=MESSAGE_DELIVERY_STRATEGY_NEXT_INVOCATION`）：
运行中通过 `Ctrl+Enter`、按住 `Ctrl` 点击发送、手机长按发送、会话菜单“引导当前任务”以及排队气泡
“立即引导”（`queue_steer`，保留已发布附件路径与无关草稿）提交后，原生在当前工具或模型步骤结束后进入下一
invocation，保持同一 `stream-json` 回合、同一 PID/SID 及显式 `--model` 参数。原生不按
`clientMessageId` 去重：桥接对同一 `request_id` 保存回执，连接前失败／HTTP 非 200 记为确定失败，
超时或响应体截断记为 `unknown` 且绝不自动重发。已接收未回显的引导随 `Binding.steers` 跨节点重启保留，
原生 `user_input` 按 FIFO 绑定 `native_id` 并保持步骤时序，与 `/api/messages` 历史去重对齐。
重开正在运行的会话时，事件快照同时提供持久队列和 `queue_steer` 能力，直接恢复排队气泡的
“立即引导”，无需再次调用 `/open`。

DELTA（BUG-20261118-122549-7f8965）：原生前台已空闲时，后台任务仍可能让
headless 暂缓输出 `result` 和读取下一行 stdin。原输入已回显，且本实例
`StreamAgentStateUpdates` 的会话、执行器和执行循环状态均确认空闲时，
普通 FIFO 在同一 PID/SID 上通过 `NEXT_INVOCATION` 继续投递。执行前再次
确认状态，仅复用当前的模型、强度和模式；需要重建进程的设置仍等待 headless
回执。普通输入沿用队列顺序、原投递 ID 和原生 `clientMessageId` 对账，经
原生消费及前台空闲确认后继续下一条。原生接收与最终执行回执各自保留，
结果未知不自动重发。判断错误回执时（包括节点重启后的回放），先从原生数据库
按 `clientMessageId` 补齐已消费的 FIFO 步骤，再核对原生回复归属和回执。

DELTA：Agy 确认前台空闲且回合结束回执仍待接收时，SDK 底栏按原生记录显示
“后台定时任务等待中”或“等待回执”。点击状态可查看已核实的本会话和所属子代理定时任务，
包括任务说明、延时和设置时间。能由设置时间和延时计算时，显示预计触发时间，到时自动移除条目。
列表较长时只滚动任务区域。原生完成、取消或唤醒通知也会按任务 ID 移除对应条目；
状态显示沿用上面的投递和回执语义。

**取消后的 headless 限制**：原生接受取消后，最终 NDJSON `result` 仍可能等待后台任务
完成。SDK 快照发布 `cancel_requested`，底栏显示“停止已受理”，暂时禁用停止按钮。
节点重启后从本回合的持久取消回执恢复该状态。桥接与 FIFO 后续输入继续等待原生结束回执。
此时回退和分支返回 `409 agy_sdk_history_busy`，说明需等待回执，回执到达前不会修改会话历史。
桥接不会伪造完成回执或通过结束后台任务解除等待。
因此当前尚不等同于 TUI Esc 后立即开始下一轮。此差距记录在 [TODO](../TODO.md)。
取消终态保留运行时，待原生回执到达后新输入复用同 PID/SID。离开页面不停止任务。

独立 TUI 不共享这个 headless 运行时。打开及每轮发送前检查原生数据库的文件占用：
Linux 使用 fd/inode，macOS 使用 lsof，Windows 使用 Restart Manager。排除本桥接
持有的进程（macOS/Windows 同时排除其后代）；发现外部持有者即报 `agy_sdk_session_busy`，
不杀进程、不接管。该检查没有跨独立 CLI 的原子租约，无法防止外部 TUI 在检查后
同时打开数据库；使用 SDK 时应先正常退出同一 SID 的独立 TUI。此限制不能表述为
支持原生共享写入或完整互斥保证。其他 SID 不受影响。

接收后的进程不依赖 SSE 连接或网页存活。统一通过
[独立 SDK 宿主](sdkhost.md)持有进程和管道，节点服务重启后重新附着并回放输出。读写失败、native ERROR、进程异常退出和
中断保留可见错误，不自动重发已交给 CLI 的正文。节点 shutdown 只断开宿主连接；回合取消不终结运行时。
stderr 保存在实时快照的 `diagnostics`；已知敏感环境值及常见凭据字段先做遮盖，
不把原始 stderr 打到服务日志。启动、模型目录、关闭空输入及取消 RPC 有超时；取消超时不清理进程；正文、NDJSON 行长、队列条数与消息总量没有新增拒绝上限。

DELTA（BUG-20261020-085320-73b6ac）：原生可能在流中断后自行恢复、写出完整回复，
却仍在最终 `result.status=ERROR` 携带先前的中断错误。桥接只在当前输入的原生
transcript 含同一中断、最后记录为后续 DONE 回复且与 result.response 尾部一致时，
将其标为回执矛盾：保留原始错误诊断、完整回复及持久运行时，不追加重复系统错误，
不当成发送失败或伪造 SUCCESS。运行状态显示“结果待核对”；已接收的发送回执保持完成，后续输入无需移除旧发送记录。没有这些证据的真实错误保持可见。
DELTA（BUG-20261022-025356-038499）：同会话后续输入的 `result` 也可能继续携带历史错误（包括空函数调用），或本轮中途发生内置 HTTP 重试（`API error (attempt N): ...`）后已自动重试完成：
- 历史错误：当前原生 `user_input` 之前的最近非同批重试错误与回执错误文本完全一致、当前输入之后没有任何 `ERROR_MESSAGE`、最后原生记录为有正文的 `DONE` 回复且与回执正文尾部一致时，按本轮原生完成处理。
- 本轮瞬时重试恢复：回执错误为 `API error (attempt N): ...`、本轮原生 transcript 含该重试记录且不存在非重试 `ERROR_MESSAGE`、最后原生记录为后续有正文的 `DONE` 回复且与回执正文尾部一致时，同样按本轮原生完成处理，保留原始回执诊断与运行时，不追加重复 `agy_sdk_native_ERROR:`；仅含元数据的 `error_message` 通知从原生 `ERROR_MESSAGE` 记录补齐正文并按 `phase=progress` 归入回合过程，与 `/api/messages` 对齐。
结构化日志同时记录原生和有效状态、历史／重试错误步骤及当前输入／末尾步骤；缺少完成证据、重试耗尽或夹杂其他错误时保持真实错误可见，不会仅凭回复措辞猜测成功。
发送与执行阶段、旧账本迁移统一遵守[公共输入合同](native-sdk.md#持久输入与失败恢复)。
原生消费或完成证据补充运行诊断，不把执行错误变成发送失败。
已结束宿主的回放保留同一回执判断；后续输出 EOF／读取失败只清除可复用运行时，
不覆盖已有回合错误或“结果待核对”。输出结束原因、可取得的退出码／信号及
脱敏 stderr 留在诊断中；没有既有回合错误时仍显示运行时关闭错误。
节点再次重启不重发旧输入；只有新的明确发送才创建同 SID 的新运行实例。

同一缺陷的生命周期修复：合法的 `result.status=ERROR` 只结束当前回合，
不再触发对持久 CLI 的 SIGKILL。保留错误、消费证据和待核对队列，也保留运行时绑定；
节点重启可重新附着，用户确认移除记录后，后续输入复用同一进程。不能因此将真实
错误改为成功或自动重试。读写或协议失败仍按原有路径清理；回合取消保留运行时；
原生自行退出时仍报告实际退出原因。宿主 stderr 在回合错误时读取脱敏快照，
没有宿主时保留直接子进程的 stderr 收集器，待退出后读取。

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

DELTA（BUG-20261020-082043-e49ea2）：未显式选模型时，恢复读取该 SID 完整原生
transcript 中最近一次模型设置通知；运行后消费原生 `init.model`（若提供）和后续
user_input 对应的设置通知。真实 headless 在未传 `--model` 时可能省略 `init.model`，
不能据此清空已知模型。底栏按目录名称匹配 ID，目录失败仍显示已确认名称；
没有原生证据时继续保留未知。启动已有 `--conversation <sid>` 且未显式指定 `--model` 时，
若该会话已有原生模型设置通知且 profile 未预设 `--model`／`--settings`，则以会话级
`--settings {"model":"…"}` 传入该模型（不改动用户全局 `settings.json`，也不把观测值当作
`--model` override 或因运行中补齐显示而重建进程）。
顶部与列表从同一原生设置通知生成会话摘要；用户正文、工具输出里的引用不是设置证据。

`items` 仅为当前桥接接受/观察的实时消息，与普通历史区独立。原生步骤消息的
稳定 ID 为 `agy:<SID>:step:<step_index>`；接受的用户输入 ID 使用 request_id，
收到原生 user_input 时另加 native_id。序号在当前 live session 内递增。
DELTA（BUG-20261115-234238-df94ad）：后台工具未完成时，Agy 可能先写入用户历史，
稍后才输出 headless `user_input`。对尚未回显的引导，桥接只读核对当前会话原生数据库中
已完成的 `USER_INPUT`，以 `clientMessageId` 精确匹配投递 request_id，补齐原生步骤身份与
消费回执。Agy 1.5.9 的未压缩 `step_payload` 使用 CascadeStep 字段 19 下的
UserInput 字段 19 保存该身份。数据库不可读、格式未知或身份未匹配时继续等待原生回显；
不按正文或时间猜配，不重发、不改写原生数据。迟到的 NDJSON 更新同一消息；相同正文的
不同请求仍各自保留。浏览器验收覆盖历史先落盘、回显延迟及两次同文引导。
DELTA（BUG-20261120-050041-623923）：回退后重用原生步骤号时，Agy 可能写入新用户
消息却省略 headless 回显。stdin 投递发送前记录当前原生步骤边界，随宿主绑定保存。
缺少回显时，桥接仅用边界之后已完成、无 `clientMessageId`、原生 `userResponse` 与
本轮发送正文完全一致且唯一的步骤补齐消费和消息身份。旧绑定没有该边界时继续使用
原生回显。这样新消息落盘后只保留历史中的气泡，已回退的步骤和相同正文的旧输入
不会被当成本轮消费证据。
DELTA：普通历史保留每条投影的 `native_step_index`、`native_step_status`。
SDK 连接后保留已完成原生步骤的完整历史（含工具结果、设置说明和分页缺口），
只接入该边界之后的实时步骤；未知步骤身份的待回显输入仍保留。
历史更新时同时重算实时尾部，避免旧快照覆盖已落盘的新答复。缺少步骤字段的旧节点
沿用原有按用户身份衔接。验收见 [Agy 历史衔接浏览器](../tests/agy_sdk_history_join_browser.py)。
DELTA（BUG-20261120-084823-98cbce）：首次空闲快照保留载入时已展开的历史尾回合，
包括只有工具结果的回合。重连继续使用最近一次收到的快照衔接历史；页面实际观察到
运行中的回合结束，或用户操作标题栏的过程按钮时，恢复按已完成回合处理。
DELTA（BUG-20261115-023419-7b8c75）：桥接生成的取消／错误通知没有原生步骤编号，
按 SDK 快照中前序原生步骤定位并插入历史，保留通知本身及原始时间；历史增长、补页、
重复快照和重新进入页面不把旧通知追加到新回复之后。没有前序步骤证据的通知及待回显
用户输入继续保留在实时区，不按时间戳猜测原生顺序，也不修改历史读取游标。

浏览器从当前运行状态确认 Agy 实例仍在线时，重新进入视图直接订阅，不重发 open。
显式启动仍调用 open，未确认运行时不凭历史缓存启动。
消息订阅直接观察已有桥接实例；实例不在内存中时只尝试恢复持久绑定，不启动新的原生 CLI。
打开和发送仍核对原生数据库占用，消息订阅不重复执行打开时的全进程占用扫描。
广播 lag 后 facade 应使用 bridge/reset 触发重新订阅，不能把缺口解释为任务结束。
配置持久 state dir 时，实时消息从宿主输出重建，正在执行及闲置的 child 跨节点服务重启。
绑定记录运行时及当前输入的输出起点；各输入使用独立传输 ID，恢复只回放该输入的
既有输出并核对原回执，不重新提交。旧的每轮 EOF 绑定仍按原语义完成，之后的新轮使用持久进程。共享 facade 的
[持久输入账本](native-sdk.md#持久输入与失败恢复) 保存尚未投递的队列、提交去重与
失败/未知回执；在途工作重启后先核对原宿主绑定，失败才标未知，不自动重发。关联的原生 user_input 提供
消费证据，不能把准入当消费。原生历史仍由既有 inventory/read-model 读取。

## 验证

[agy_sdk_browser.py](../tests/agy_sdk_browser.py) 使用临时原生目录和合成协议对端，
通过 Chromium 实际操作 SDK 页面，覆盖本来源的新建/恢复、发送、流式消息和工具、
去重、中断、重开及外部 writer 冲突；错误回合后验证同 PID 继续且没有终止请求。
[持久队列浏览器](../tests/native_sdk_queue_browser.py)还覆盖真实错误后的节点重启、
待核对阻塞后续输入、确认移除后同 PID 继续且不重发旧输入。
[历史错误回执浏览器](../tests/agy_historical_error_browser.py)覆盖旧空函数调用错误、
本轮新错误、历史证据缺失／不匹配、回复不匹配与重启回放后同 PID 继续且不重发。
[原生取消浏览器](../tests/agy_cancel_real_browser.py)在隔离 HOME 和 loopback 合成网关
启动真实子代理及有心跳的后台进程，通过网页取消后等待超过原有 5 秒期限，核对
它们继续存活；放行这些任务后验证子代理完成、同 PID/SID 下一轮成功。该用例
不宣称后台任务仍活跃时 headless 已能立即接收下一轮。审批和问题按本来源声明的能力验收。
[Agy 引导浏览器](../tests/agy_steer_browser.py)覆盖节点／Hub 的发送手势、排队转引导、
附件路径、回执去重、确定拒绝、断线待核对与服务重启复接，并在前台空闲、
headless 回执被后台任务延迟时核对普通 FIFO 自动投递和延迟回显；
[真实 Agy 引导浏览器](../tests/agy_steer_real_browser.py)在临时 HOME 和 loopback 合成网关
验证 `Ctrl+Enter` 与 `queue_steer` 在同一回合、同一 PID/SID 下继续执行并保持显式 `--model`。
共享 Hub、草稿恢复与网页请求卡路径见
[native_sdk_hub_browser.py](../tests/native_sdk_hub_browser.py)。
完整验证命令与套件入口见[验证清单](validation.md)，协议实验的实机范围仍以上述证据为准。

## 用量与设置

DELTA：底栏用量提示保留原生 `result.usage` 累计 Token（底栏内联仅展示上下文与账号额度）；`step_update.usage` 在明细单独
标注，不相加，也不伪装为上下文百分比。额度使用独立 print `/usage` collector，
不发送模型 turn；与设置页共享持久快照和查询，缓存与刷新条件见[额度合同](quotas.md)。
打开先显示已有数据，不等待原生启动；明细及菜单条件见[SDK 界面合同](sdk-ui.md)。

DELTA（BUG-20261114-141520-4b1627）：上下文从当前 SDK 所拥有实例的
Connect `StreamAgentStateUpdates` 初始快照读取 `contextWindowUsage`，
使用原生 `estimatedTokensUsed / maxContextTokens`，复用底栏上下文进度条。
只查询本实例 PID/CSRF 授权和对应 SID；关闭历史分页区间，读取一帧即关闭订阅。
有网页订阅时每五秒刷新，网页 `/context` 和 `/usage` 也刷新；不启动原生进程、
不发送模型回合、不取累计 Token 推算。原生压缩后的估计按新测量更新；
字段缺失、窗口未知或实例退出时隐藏，查询失败显示用量错误并撤下旧百分比。
原生 NDJSON 与 transcript 不含这个测量，不能从它们替代读取。
[Agy 用量浏览器](../tests/agy_usage_browser.py)覆盖刷新、压缩估计、缺失、错误与身份不匹配；
[真实上下文浏览器](../tests/agy_context_real_browser.py)使用隔离 HOME 与 loopback 合成模型，
核对原生测量、同 PID/SID、刷新及零额外模型回合。


模型目录与新建会话共用解析：带强度后缀的原生模型已固定 effort。SDK 模型框将这些
变体合并为一个模型，强度框只提供目录中实际存在的档位；选择后传对应原生模型 ID，
并清除旧的独立 effort。包含基础模型的分组可选择“CLI 默认”。无变体的普通模型保留
low/medium/high/xhigh/max 独立 `--effort`。恢复旧设置或排队任务时，具体变体不会再
附带冲突的 `--effort`，模型目录不可用时不猜测可选档位。
`settings.mode` 可显式选择
accept-edits/auto/plan，标明下一轮生效，以独立 `--mode` argv 传入，不制造在线 RPC。
网页 `/usage`、`/model`、`/effort` 与正文分流，未知 slash 保留草稿。
图片沿用普通附件路径；headless 审批回传限制仍有效。

[用量浏览器](../tests/agy_usage_browser.py)覆盖真实发送、零模型回合刷新、
无 Token 与双额度组的组合、阻塞刷新、失败保留及宽窄屏明暗主题。
[模型浏览器](../tests/agy_model_browser.py)覆盖顶部、列表和底栏的模型名称、
原生默认值、实时更新、刷新、服务重启复接、目录失败与显式切换。
[强度浏览器](../tests/agy_effort_browser.py)覆盖分组、真实可选组合、基础模型默认档、
普通模型独立 flag、旧冲突设置、运行中切换与实际 CLI argv。

本实例管理端口从原生日志增量解析；上下文刷新与控制请求共享读取位置，只读取
新增字节。日志替换、截断或请求 PID 变化时重新解析，仍要求对应 PID 的启动标记，
不把另一实例的端口作为回退。未完成的末行在下一次追加时继续解析。
