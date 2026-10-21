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
思考只显示原生记录提供的内容，空通知不生成助手气泡；系统消息沿用普通历史的
信封解析与来源标签。未知步骤保留为系统消息；`result` 保留终态错误，缺少流式正文时使用
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

接收后的进程不依赖 SSE 连接或网页存活。配置持久 state dir 时，通过
[独立 SDK 宿主](sdkhost.md)持有进程和管道，节点服务重启后重新附着并回放输出。读写失败、native ERROR、进程异常退出和
中断保留可见错误，不自动重发已交给 CLI 的正文。节点 shutdown 只断开宿主连接；显式停止才终结任务。
stderr 保存在实时快照的 `diagnostics`；已知敏感环境值及常见凭据字段先做遮盖，
不把原始 stderr 打到服务日志。启动、模型目录、关闭空输入和中断清理有生命周期
超时；正文、NDJSON 行长、队列条数与消息总量没有新增拒绝上限。

DELTA（BUG-20261020-085320-73b6ac）：原生可能在流中断后自行恢复、写出完整回复，
却仍在最终 `result.status=ERROR` 携带先前的中断错误。桥接只在当前输入的原生
transcript 含同一中断、最后记录为后续 DONE 回复且与 result.response 尾部一致时，
将其标为回执矛盾：保留原始错误诊断、完整回复及持久运行时，不追加重复系统错误，
不当成发送失败或伪造 SUCCESS。运行状态显示“结果待核对”；已接收的发送回执保持完成，后续输入无需移除旧发送记录。没有这些证据的真实错误保持可见。
同会话后续输入的 `result` 也可能继续携带历史中断：只在当前原生 user_input 之前的
最近错误与回执一致、当前输入之后没有任何 ERROR_MESSAGE、最后原生记录为有正文的
DONE 回复且与回执正文尾部一致时，按本轮原生完成处理，保留原始回执诊断与运行时。
结构化日志同时记录原生和有效状态、历史错误步骤及当前输入／末尾步骤。
缺少历史证据、出现新的错误或后来输入时均不套用此处理；不会仅凭回复措辞猜测成功。
发送与执行阶段、旧账本迁移统一遵守[公共输入合同](native-sdk.md#持久输入与失败恢复)。
原生消费或完成证据补充运行诊断，不把执行错误变成发送失败。
已结束宿主的回放保留同一回执判断；后续输出 EOF／读取失败只清除可复用运行时，
不覆盖已有回合错误或“结果待核对”。输出结束原因、可取得的退出码／信号及
脱敏 stderr 留在诊断中；没有既有回合错误时仍显示运行时关闭错误。
节点再次重启不重发旧输入；只有新的明确发送才创建同 SID 的新运行实例。

同一缺陷的生命周期修复：合法的 `result.status=ERROR` 只结束当前回合，
不再触发对持久 CLI 的 SIGKILL。保留错误、消费证据和待核对队列，也保留运行时绑定；
节点重启可重新附着，用户确认移除记录后，后续输入复用同一进程。不能因此将真实
错误改为成功或自动重试。显式中断／停止、读写或协议失败仍按原有路径清理；
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
没有原生证据时继续保留未知。观测值不写入启动 override，不因补齐显示重建进程。
顶部与列表从同一原生设置通知生成会话摘要；用户正文、工具输出里的引用不是设置证据。

`items` 仅为当前桥接接受/观察的实时消息，与普通历史区独立。原生步骤消息的
稳定 ID 为 `agy:<SID>:step:<step_index>`；接受的用户输入 ID 使用 request_id，
收到原生 user_input 时另加 native_id。序号在当前 live session 内递增。
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
审批和问题按本来源声明的能力验收。
共享 Hub、草稿恢复与网页请求卡路径见
[native_sdk_hub_browser.py](../tests/native_sdk_hub_browser.py)。
完整验证命令与套件入口见[验证清单](validation.md)，协议实验的实机范围仍以上述证据为准。

## 用量与设置

DELTA：底栏展示原生 `result.usage` 累计 Token；`step_update.usage` 在明细单独
标注，不相加，也不伪装为上下文百分比。额度使用独立 print `/usage` collector，
不发送模型 turn；与设置页共享持久快照和查询，缓存与刷新条件见[额度合同](quotas.md)。
打开先显示已有数据，不等待原生启动；明细及菜单条件见[SDK 界面合同](sdk-ui.md)。

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
