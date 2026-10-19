# 多来源 SDK 模式

Grok、Agy、OpenCode 和 Claude 使用各自的原生机器接口，复用 Codex SDK
视图的消息样式、历史分页、草稿、附件暂存和会话导航，不通过 PTY 输入。
各来源的协议、版本证据与能力边界分别见：

| 来源 | 原生接口 | 合同 |
| --- | --- | --- |
| Grok | ACP JSON-RPC stdio | [Grok SDK](grok-sdk.md) |
| Agy | headless stream-json | [Agy SDK](agy-sdk.md) |
| OpenCode 2 | 原生服务 API，经 CLI `api` 调用 | [OpenCode SDK](opencode-sdk.md) |
| Claude | 双向 stream-json | [Claude SDK](claude-sdk.md) |
| Codex | 共享 app-server | [Codex SDK](codex-sdk.md) |

## 配置与页面

节点必须配置该来源的原生根目录及 launcher profile；启动节点只装配桥接，
首次打开 SDK 才连接或启动原生客户端。`capabilities.native_sdks` 按来源声明
可用性，Hub 根据所属节点的能力显示入口。SDK 配置不兼容时记录在
`native_sdk_unavailable`，不阻止原有读模型和终端模式启动。
`SESSIONDOCK_NATIVE_SDKS=off` 关闭这四种 SDK 桥接；Codex 保留自己的配置。
普通 PTY 浏览器夹具显式关闭 SDK，SDK 夹具显式启用，避免假 CLI 被当成 SDK。

DELTA：SDK 可用的来源默认打开 SDK；新建直接调用原生接口取得 SID，不创建
PTY。原生 CLI 是否立即落盘由各协议决定。已配置的普通对话和终端入口仍可手动
选择。恢复只使用 inventory 或创建回执中的原生 SID/cwd，不按标题或目录猜配。
Claude 的原生子代理归档不能作为独立 SDK 会话，隐藏其入口。
DELTA（Claude 视图恢复）：按主会话 UID 在浏览器保存最近选择的 SDK、对话或终端，
切换会话及刷新后恢复，Hub 的节点命名空间分别保存。没有记录时先等待终端发现；
已有受管终端沿用其布局，已观察到独立 CLI 时显示对话，其余仍默认 SDK。
恢复视图不自动接管、结束 CLI 或释放 SDK；终端已退出时保留普通历史和终端入口。

输入共用服务端草稿。失败保留输入，成功只清本次提交，发送期间的新编辑保留。
DELTA：五种 SDK 的文件与图片统一沿用原版 SessionDock 附件逻辑：发送时发布到
会话 cwd 的 `sessiondock_attachments/<批次>/<文件>`，正文追加带编号的相对路径。
不再追加原生图片块、base64 或 data URI，不读取图片字节来构造 SDK 输入，也不提供
原生图片能力开关或常驻协议提示。旧准备队列中的图片字段在原生投递时不再使用。
草稿、上传暂存、发布去重及普通附件卡片沿用既有合同。
客户端自报 images/files/path 不授予读取权限。
已加载的原生 SDK 会话通过 `/api/live.sdk_uids` 标记为受管 SDK，沿用蓝色状态点。
模型目录来自原生接口，目录失败不妨碍沿用会话当前模型。现已接用量明细/底栏、
各来源可用设置、OpenCode inbox、Grok/OpenCode 会话动作和 Claude 压缩。
持久队列、失败恢复与原生消费证据分开显示。已实现范围、可补齐遗漏与
协议限制见[五种 SDK 功能审计](sdk-feature-audit.md)。

DELTA（SDK 状态呈现）：五种 SDK 的输入框上方统一使用小圆点加同色文字，
复用现有主题色，保留状态全文与详情入口：

| 颜色 | 状态 | 圆点动画 |
| --- | --- | --- |
| 绿 | 就绪、压缩完成 | 静止 |
| 蓝 | 处理中 | 呼吸 |
| 琥珀 | 待批准、待回答、等待发送、队列暂停 | 静止；正在重试时呼吸 |
| 紫 | 正在压缩上下文 | 呼吸 |
| 红 | 请求或原生操作失败 | 静止 |
| 灰 | 连接、重连、普通提示 | 连接与重连时呼吸，普通提示静止 |

动画只作用于圆点，系统要求减少动态效果时关闭。压缩进度与结果使用状态栏，
SDK 实时区不再创建压缩气泡，SDK 历史区也不重复展示 `event_kind:compact`
的提示记录；保留原生历史、分页位置和普通对话页的记录。
覆盖：[SDK 布局](../tests/codex_sdk_layout_browser.py)、
[压缩动作](../tests/codex_sdk_actions_browser.py)、
[原生 SDK Hub](../tests/native_sdk_hub_browser.py)。

网页离开、SSE 断开或切换视图不取消已接受工作；重开取桥接快照，不重发输入。
全部五种 SDK 视图使用 SDK 实时流与既有增量轮询同步落盘历史，暂不占用额外的
历史 watch 长连接，避免两个标签页耗尽 HTTP/1 连接池而阻塞发送/撤销。
离开 SDK 视图后恢复历史 watch，切换会话不会恢复旧会话的连接。
共享 facade 负责关闭/恢复 watch；Codex 双标签页撤销另由
[浏览器回归](../tests/sdk_multitab_browser.py)覆盖。
DELTA：用量底栏直接展示已知 Token、上下文及额度；窄屏放不下时换行，不收进
“用量”按钮。模型框适配列表最长名称，最多 160px，窄屏继续收缩；上下文显示 `Ctx`，
累计用量使用 `tok` 和大数缩写，额度长名称省略显示；完整名称与精确数值保留在悬停或明细。
覆盖：[四种原生 SDK 底栏](../tests/native_sdk_footer_browser.py)。
输入/输出明细与手动刷新从 ⋯ → 用量明细进入。没有数据时不伪造数值。
用量/设置菜单在异步数据改变高度后重新定位，保持在视口内，长内容可滚动，
刷新按钮仍可点击；沿用已有 Codex 菜单定位与样式。
同一会话再次打开时，SDK 历史跟随新的读取请求；已被替代的旧请求取消不显示
历史错误，也不覆盖新请求成功得到的内容。
提交 ID 和明确的重试/撤销操作 ID 保存在私有输入账本，完整节点重启后的
原生任务恢复边界仍见各来源合同。
新建回执保存在私有 `native-sdk-sessions.json`，同一创建请求可恢复其 UID/SID。
持久排队准入后沿用既有 SEND 回执及草稿修订规则，服务端持久清除本次提交字段；
响应丢失或立即刷新不恢复已发送的输入，发送期间的新编辑仍保留。
原生记录进入 inventory 后，回执停止充当侧栏待落盘行，后续原生删除不会复活它。

## HTTP 与事件

所有请求均由节点校验所选 UID/agent 对应的 inventory 身份。Hub 沿用 UID
命名空间和单节点代理；新建使用 `_node` 指定节点。

| 接口 | 输入与行为 |
| --- | --- |
| POST `/api/native-sdk/create` | `{source, request_id, cwd, create_cwd?, model?, effort?}`；原生创建，返回 UID、SID、session 与能力 |
| POST `/api/native-sdk/open` | `{uid, agent?}`；恢复原生会话，返回能力 |
| GET `/api/native-sdk/events?uid=&agent=` | SSE `sdk` 事件：原子快照与订阅，之后持续发布快照；滞后时要求重连 |
| POST `/api/native-sdk/turn` | `{uid, agent?, request_id, text, attachments?, quotes?, model?, effort?}`；先持久排队再调用原生 |
| POST `/api/native-sdk/interrupt` | `{uid, agent?}`；原生中断，只针对桥接所属任务 |
| POST `/api/native-sdk/respond` | `{uid, agent?, key, result}`；回答该会话仍未解决的原生审批或问题 |
| POST `/api/native-sdk/models` | `{uid, agent?}`；原生模型目录 |
| POST `/api/native-sdk/usage` | 原生 Token/上下文；Agy/Claude 合并已有 quota；`refresh:true` 仅查询本来源 |
| POST `/api/native-sdk/settings` | 来源自己的模式/权限/effort/Fast；OpenCode `native_agent` 与身份字段 `agent` 分开 |
| POST `/api/native-sdk/fork` | Grok/OpenCode/Claude 原生分支，返回稳定身份；成功网页回执持久化、Hub 命名空间处理 |
| POST `/api/native-sdk/compact`、`shell` | 按 capability 走原生操作；mutation 带 request_id |
| POST `/api/native-sdk/queue_cancel`、`queue_retry` | 本地队列撤销/重试，`id` 以 bridge: 开头；未知结果须 confirmed:true；操作带 request_id |
| POST `/api/native-sdk/queue_update` | OpenCode 未投递 inbox 显式 steer；其 queue_cancel 仍只撤销原生未投递项 |
| POST `/api/native-sdk/steer` | Grok 显式原生引导，独立 request_id；不进入普通 FIFO，不把接收回执当消费 |
| POST `/api/native-sdk/revert`、`unrevert`、`revert_commit` | OpenCode 对话-only stage/撤销/永久提交，固定 files:false |
| POST `/api/native-sdk/rewind_points`、`rewind`、`billing` | Grok 原生选择点、conversation_only 回退和额度 |
| POST `/api/native-sdk/agents`、`commands`、`command` | OpenCode 原生目录与命令；不把命令伪装成普通提示词 |
| POST `/api/native-sdk/discard` | `{uid}`；释放未落盘创建回执，保留原生历史；能力与拒绝条件由来源声明 |

普通操作响应为 `{ok, raw, ...原生桥接结果}`，创建结果直接位于顶层。
SSE 包为 `{seq, type:"snapshot", raw:{session_id,running,items,requests,...}}`。
队列更新另发 `{type:"input_queue",raw:{input_queue:[...]}}`，不改变原生 seq；
首快照与队列订阅原子取得，普通原生快照也附带当前 input_queue。
消息及请求有稳定 ID；工具参数、输出、错误和审批选项经原生协议保留。
快照不会推进历史分页的 live checkpoint，也不会自动补齐省略的中间历史。
SDK 连接错误显示原生原因并保留草稿；再次点击已选 SDK 按钮重试打开。

Agy headless 没有审批回答协议，按 CLI 自身策略允许或软拒绝；界面不伪造批准。
Grok、Agy、Claude 的独立 TUI 不能像 Codex daemon 那样共享实时上下文；
桥接报告原生占用冲突，不结束或接管独立 TUI。OpenCode 共享服务与 standalone
运行时的边界见其合同。

网页未知 slash 保留输入并说明；`/usage` 与聊天发送分开。原生操作失败/不支持
明确显示，不自动放宽权限。用量详情区分累计 Token、步骤用量、上下文估计和
额度窗口；不可用的实验接口保留旧值与错误。创建/分支回执持久化不意味着所有
在途操作具有跨进程 exactly-once 保证。

## 持久输入与失败恢复

有 state dir 时，`native-sdk-inputs.json` 保存原始提交、可信发布后的实际输入、
投递 ID、准入结果与重试操作回执；目录/文件权限沿用私有状态目录约定。
无 state dir 时仅内存保存。账本不改写原生 transcript，也不作为原生任务宿主。

Grok/Agy/Claude 按所选 UID/agent 串行调度，前一任务、审批或问题未结束时继续排队。
OpenCode 保留原生 inbox 的 queue/steer 语义，本地账本只跟踪未确认准入。
网页区分 preparing、queued、sending、accepted、failed 和 unknown；完成/撤销回执
隐藏但继续去重。accepted 只表示接收，Agy 的关联 user_input、Claude 的真实 user UUID
回显和 OpenCode 的已消费原生消息才提供 observed 消费证据。Grok 正常完成仍保留
consumption:unverified，不把乐观用户气泡当原生回显。

失败或未知投递阻塞本地后续输入。重启把 sending/accepted 转为 unknown，绝不自动
重发；尚未发送的 queued 输入可以继续调度。明确重试生成新的原生投递 ID，原始提交
ID 保持去重；同一个重试操作 ID 也不会再次执行。未知结果重试前，网页要求核对历史
并确认重复风险。移除未知记录只解除本地阻塞，不停止原生任务；停止须显式中断。
“恢复文字”追加到当前输入框，保留新编辑；附件/引用仍由已有草稿与发布路径管理。
Grok 引导单独记录，不会在重启或失败后变为普通输入自动重试。

排队及已接受工作持有原生订阅，避免闲置回收误杀任务。Grok/Claude 无订阅、任务、
审批和原生控制等活动后经过十分钟才回收桥接子进程，重开恢复相同身份且不重发输入。
Grok 尚未证明终结的引导和已观测后台任务保持租约；各来源的细节见其合同。
