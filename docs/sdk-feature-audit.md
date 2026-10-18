# 五种 SDK 功能与漏实现审计

2026-10-19，对照代码快照 `25178c71`，以 SessionDock 已实现的 Codex SDK
视图为基准，核对 Grok、Agy、OpenCode、Claude 的桥接、HTTP、网页和浏览器用例，
再用官方文档、官方源码及固定版本接口证据判断可实现范围。
这是功能审计，不是新增功能交付或一次新的运行验收；后续工作统一登记在
[TODO](../TODO.md)。

结论：四种新 SDK 已有主要会话闭环，但没有完整对齐 Codex。用量、队列管理、
会话操作和设置中存在可补齐的遗漏；不能把当前 capability=false 或没有按钮
直接解释成原生协议不支持。Agy 的用量底栏属于明确遗漏。

## 范围、版本和判定口径

本文的“SDK”指 SessionDock 的机器接口视图。Codex 实际走 app-server，Grok
走 ACP，Agy 走 CLI headless，OpenCode 走 v2 server API，Claude 走 Claude Code
stream-json。官方同名 SDK 包不一定使用同一协议，不能把别的 SDK 的能力直接
移植为现有 CLI 会话能力。

| 来源 | 本仓库核对版本 | 主要代码与合同 |
| --- | --- | --- |
| Codex | CLI 0.163.0；本次复读官方 app-server 文档 | [桥接](../crates/sessiondock/src/codex_sdk/mod.rs)、[视图](../web/codex-sdk.js)、[合同](codex-sdk.md) |
| Grok | Build 1.0.62；官方源码固定到 `2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8`，与安装版 build commit 不同 | [桥接](../crates/sessiondock/src/grok_sdk/mod.rs)、[投影](../crates/sessiondock/src/grok_sdk/live.rs)、[合同](grok-sdk.md) |
| Agy | CLI 1.4.2；本次复读 headless、statusline、usage、conversation、SDK 文档及本机帮助 | [桥接](../crates/sessiondock/src/agy_sdk/mod.rs)、[传输](../crates/sessiondock/src/agy_sdk/transport.rs)、[合同](agy-sdk.md) |
| OpenCode | CLI 2.0.32；核对该安装版隔离服务导出的 OpenAPI，官网页面仍混有 v1 API | [桥接](../crates/sessiondock/src/opencode_sdk/mod.rs)、[投影](../crates/sessiondock/src/opencode_sdk/project.rs)、[合同](opencode-sdk.md) |
| Claude | CLI 2.1.302 / Agent SDK 0.3.302 的官方包及类型；本次复读官方专题文档 | [桥接](../crates/sessiondock/src/claude_sdk/mod.rs)、[投影](../crates/sessiondock/src/claude_sdk/protocol.rs)、[合同](claude-sdk.md) |

符号含义：

- **已**：代码接通；具体实现和限制见对应行。已有浏览器用例不等于所有生产账号均实测。
- **部**：只实现一部分，或实现语义弱于该行的 Codex 基准。
- **漏**：尚未实现；已有原生接口或可复用的 SessionDock 实现证据。
- **待**：尚未实现/确认；存在候选路径，但当前证据不足以承诺同等语义。
- **限**：当前选定的原生机器接口明确不支持该能力，或当前拓扑不能提供它。
- **—**：没有相同概念，不要求制造一个 Codex 特有功能。

“漏”是工程可行性判断，尚未经过新增适配和验收。尤其 Grok 扩展必须按安装版
能力协商、错误返回再验证；不能以源码快照直接承诺该 build 一定可用。
表内“已”也不扩大平台范围：Codex 当前 Unix，Agy 当前 Linux；其他来源的平台
和外部 writer 发现限制沿用各自合同。

## Codex 基准清单及五种 SDK 对照

下面各表构成同一张功能表；ID 用于定位遗漏，不把一项协议能力重复算成多个
“完成率”。没有给百分比，因为队列持久化和一个状态按钮的工作量、语义不同。

### A．入口、会话身份和生命周期

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| A01 | 按节点能力显示 SDK，默认打开；手动切换对话/终端后尊重本页选择 | 已 | 已 | 已 | 已 | 已 |
| A02 | 原生机器接口收发，不依赖 PTY 按键 | 已 | 已 | 已 | 已 | 已 |
| A03 | 网页新建：cwd、目录缺失确认、模型/effort，返回稳定 UID/SID | 已 | 已 | 已 | 已 | 已；SID 由本地原生 UUID 规则预留 |
| A04 | 无模型提示词换身份；不写 placeholder 或重序列化原生历史 | 已 | 已 | 已 | 已 | 已 |
| A05 | 未落盘会话在侧栏显示，网页刷新可重开；落盘后交给 inventory | 已 | 已 | 已 | 已 | 已 |
| A06 | 同一创建 request_id / 丢 HTTP 响应重试不重复新建 | 已；进程内 | 已；共享回执落盘 | 已；共享回执落盘 | 已；共享回执落盘 | 已；共享回执落盘 |
| A07 | 精确恢复已选主会话 SID/cwd，不按标题猜配 | 已 | 已 | 已 | 已 | 已 |
| A08 | 精确选择子代理自身会话并交互，不误发给父会话 | 已；独立文本输入 | 部；仅可恢复的原生 SID，缺专门子代理验收 | 部；同 SID 解析约定，缺专门子代理验收 | 部；独立子 SID 路径，缺专门子代理验收 | 限；子代理 transcript 不能独立 resume |
| A09 | 与日常 TUI 共享一个存活 runtime、同一上下文和任务 | 已；同 daemon 才成立 | 限；当前 `--no-leader` 独立 runtime | 限；当前 headless 独立 runtime | 已；默认共享服务，standalone 不适用 | 限；当前 stdio 子进程独立 runtime |
| A10 | 页面离开、切换、SSE 断开不取消已经接受的任务 | 已 | 已 | 已 | 已 | 已 |
| A11 | 重开取实时快照，不自动重发原提示词 | 已 | 已 | 已 | 已 | 已 |
| A12 | 节点重启后，已运行的原生任务仍由独立 daemon 持有 | 已；daemon 不重启时 | 限；桥接子进程没有此持久宿主 | 限；逐轮 child 没有此持久宿主 | 已；已交给原生 daemon 的任务 | 限；桥接 child 没有此持久宿主 |
| A13 | 独立 writer 冲突可见；不强杀、不接管；退出后可重试打开 | 已 | 已；观察有竞态 | 已；观察有竞态 | 已；默认服务以外有发现边界 | 已；观察有竞态/平台边界 |
| A14 | 丢弃空新建回执、释放桥接，保留已出现的原生记录 | 已 | 已 | 已 | 限；未提供同义可撤销 discard，不能用永久删除代替 | 已 |
| A15 | 受管 SDK 蓝点；未成功连接不冒充 SDK 已加载 | 已；另能只读发现 daemon 已加载会话 | 部；只有本桥接已加载证据 | 部；只有本桥接已加载证据 | 部；未做 Codex 式全局已加载发现 | 部；只有本桥接已加载证据 |
| A16 | 无页面、无任务、无审批后释放闲置订阅/运行资源 | 已；10 分钟 unsubscribe | 漏；没有同等闲置回收 | 已；每轮 EOF 退出，不是常驻订阅 | 已；无人订阅且空闲停止观察 | 漏；持久 child 未做同等闲置回收 |

代码依据：[Codex 生命周期](../crates/sessiondock/src/codex_sdk/mod.rs)、
[只读 daemon 发现](../crates/sessiondock/src/codex_sdk/observation.rs)、
[四来源创建回执](../crates/sessiondock/src/native_sdk.rs)、
[共享视图 facade](../web/native-sdk.js)。新建回执落盘不代表 live items、原生任务、
输入队列或整套 runtime 能跨重启恢复。Grok/Agy 的“双写检查”是采样，不是原生锁。

### B．编辑、附件和发送

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| B01 | 文本、多行、Enter 发送、Shift+Enter 换行、手机按钮发送 | 已 | 已 | 已 | 已 | 已 |
| B02 | 服务端草稿、CAS 保存、刷新恢复、跨页跟随；不覆盖未保存编辑 | 已；主会话 | 已；主会话 | 已；主会话 | 已；主会话 | 已；主会话 |
| B03 | 文件选择/粘贴/拖放、私有暂存、续传、附件卡片与移除 | 已；主会话 | 已；共用 composer | 已；共用 composer | 已；共用 composer | 已；共用 composer |
| B04 | 引用文字、附件或引用单独发送；普通文件作为路径引用发布 | 已 | 已 | 已 | 已 | 已 |
| B05 | 图片作为原生多模态内容传给模型，不只发送路径文本 | 已；`localImage` | 漏；支持 ACP image，桥接只发 text | 限；此 stream-json 输入只支持 text block | 漏；v2 prompt 有原生 files，适配仍需验证模型媒体能力 | 漏；streaming user content 支持 image，桥接只发字符串 |
| B06 | 发布附件一次；相同提交 ID 重试不重复上传发布/原生输入 | 已 | 已；当前进程回执 | 已；当前进程回执 | 已；桥接回执，原生消息另有 ID | 已；当前进程回执 |
| B07 | 原生准入后清本次草稿；保留发送期间新增编辑、失败时保留输入 | 已 | 已 | 已 | 已 | 已 |
| B08 | 确认输入真正被原生用户消息消费，而非仅 RPC/write 接受；压缩中断时可找回未消费输入 | 已；队列状态和原生 echo 对账 | 漏；没有同等消费确认状态机 | 漏；无同等消费确认状态机 | 部；有持久 inbox，网页未区分准入/消费 | 部；有 replay UUID，但清草稿仍以写入准入为准 |
| B09 | 主会话无草稿存储能力时仍可纯文本；子代理不读写父草稿 | 已 | 已；共享 composer 降级 | 已；共享 composer 降级 | 已；共享 composer 降级 | 已；不开放独立子代理 SDK |
| B10 | Tab 路径补全 | 已 | 已 | 已 | 已 | 已 |

代码依据：[composer](../web/codex-sdk-compose.js)、
[Codex 原生提交](../crates/sessiondock/src/api/codex_sdk.rs)、
[其他四种提交与附件发布](../crates/sessiondock/src/api/native_sdk.rs)。B03/B04 的
复用不代表 B05 已接。Agy 文档明确只接收文本块，不能因独立 Python SDK 支持图片
而承诺现有 CLI stream-json 可传图片。[Agy headless](https://www.antigravity.google/docs/cli/headless/)
和 [Claude streaming input](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode)
分别规定了这两种输入边界。

### C．排队、中断和投递可靠性

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| C01 | 活跃时继续接收输入；按明确的队列语义等待，不隐式 steer | 已；节点 FIFO | 部；后续 HTTP 等待 turn gate，没有已准入队列快照 | 已；桥接 FIFO | 部；原生 inbox，但未显式传 `delivery:queue`，默认语义还需核实 | 部；原生 streaming 队列，可能合并/在中途消费，不等于独立 FIFO |
| C02 | 网页显示待发送/发送中/失败项；不同页面同步同一队列 | 已 | 漏 | 漏 | 漏；inbox 已查询但未展示成队列控件 | 漏 |
| C03 | 未消费项显式撤销，不误取消当前任务 | 已 | 漏；先建本地队列再撤销 | 漏；已有本地 FIFO 可扩展 | 漏；v2 `DELETE inbox/{id}` | 漏；本地队列可做，原生取消还要处理合并边界 |
| C04 | 失败项显式重试；失败阻塞后续项；不确定投递不自动重放 | 已 | 部；不重放，但没有失败项管理/阻塞界面 | 部；不重放，无同等失败队列 | 部；有原生 inbox，无同等失败队列 | 部；不重放，无同等失败队列 |
| C05 | 用户显式“立即引导”当前 turn，避免默认插入当前任务 | 已；`turn/steer` | 漏；`x.ai/interject` 候选扩展已存在 | 待；未发现同义 headless 控制接口 | 漏；v2 inbox delivery 可改 `steer` | 待；原生队列可能中途合并，尚无等价显式 steer 控件/确认语义 |
| C06 | 队列、提交 ID、失败状态持久化，节点重启不静默丢失 | 已；有 state dir 时 | 漏 | 漏 | 部；已进入原生 inbox 的项目持久，桥接请求 ID 映射/在途任务不持久 | 漏 |
| C07 | 重启后不确定在途输入标记待人工核实，不自动再发 | 已 | 漏；目前没有可恢复的在途账本 | 漏 | 部；有原生持久记录，缺桥接恢复和可见状态 | 漏 |
| C08 | 停止当前任务；不把页面关闭当停止，不杀外部会话 | 已；原生 interrupt | 已；`session/cancel` | 已；仅自己 child 的 SIGINT | 已；原生 interrupt | 已；原生 interrupt，原生回执保留 |

代码依据：[Codex 队列](../crates/sessiondock/src/codex_sdk/queue.rs)、
[Codex 队列 UI](../web/codex-sdk.js)、[NativeSdkView.send](../web/native-sdk.js)。
四种新 SDK 虽然模板含 `.sdk-queue`，却没有 `paintQueue` 或 queue 操作路由，不能
据空容器认定已经实现。OpenCode prompt 当前只发 `{id,text}`，本次不能确认它保证
默认排队；需要按 v2 原生 delivery 定义显式实现和验收。

### D．历史、流式消息和状态

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| D01 | SDK 打开与原生历史并行；SDK 失败时仍可读历史 | 已 | 已；复用 history 方法 | 已 | 已 | 已 |
| D02 | 复用已有历史 head/tail 缓存，不自动填中间；点击加载/停止中间页 | 已 | 已；共享分页入口，缺专门 SDK 竞态验收 | 已；同左 | 已；同左 | 已；同左 |
| D03 | persisted/live 去重，完成后增量追上历史，不重复展示整轮 | 已；优先原生 turn 身份 | 部；live ID/text 裁剪，未做等价 turn 对账/清退 | 部；同左 | 部；有 native message ID，前端裁剪仍简化 | 部；有 native UUID，前端裁剪仍简化 |
| D04 | 流式正文、计划文本、最终答案；沿用 Markdown/代码等消息渲染 | 已 | 已；plan 为普通 system 展示 | 已；原生给的正文 | 已；投影轮询 | 已 |
| D05 | 独立 reasoning/thinking 气泡 | 已；summary delta | 已；thought chunk | 待；当前协议/实验没有独立 thinking 正文 | 已；reasoning part | 已；thinking block/delta |
| D06 | 工具参数、输出、错误、运行状态；命令/文件 diff/MCP 等卡片 | 已；按原生类型细分 | 部；通用工具卡接通，未覆盖全部扩展类型 | 部；通用工具卡接通 | 部；通用 tool part 接通 | 部；tool_use/result 接通，专用状态更少 |
| D07 | 子代理工具结果/状态、web search、image view 等原生项目投影 | 已；有对应 mapper | 部；通用 tool 原文，缺等价专门投影 | 部；subagent_info 未做等价状态界面 | 部；通用 tool，缺等价专门投影 | 部；保留 parent_tool_use_id，缺完整任务状态投影 |
| D08 | 长正文/思考/工具展开与折叠，流更新保留用户展开选择 | 已 | 部；文本可复用，工具重绘未保留展开/换行状态 | 部；同左 | 部；同左 | 部；同左 |
| D09 | SSE 首快照与订阅原子取得、心跳、lag/reset 后重连 | 已 | 已 | 已 | 已；原生观察为轮询，非原生 SSE 直转 | 已 |
| D10 | 就绪/处理中/待批准/待回答/连接失败与完整错误可见 | 已 | 已；状态种类较少 | 已；状态种类较少 | 已；状态种类较少 | 已；状态种类较少 |
| D11 | 恢复中可重试错误、willRetry 提示、用量已满等专门状态 | 已 | 漏；扩展 retry/status 未接 | 待；尚无对等结构化事件证据 | 漏；assistant.retry/原生错误可投影 | 漏；api_retry/rate_limit_event 未投影 |
| D12 | 压缩专用进度卡，快照恢复；不编造百分比 | 已 | 漏；有原生压缩与通知，未接 | 待；须先确认 headless 压缩接口 | 漏；v2 compaction 消息未投影 | 漏；compacting/compact_boundary 未完整投影 |

代码依据：[Codex itemMessage/applyEvent](../web/codex-sdk.js)、
[共享 nativeSdkMessage/paintHistory](../web/native-sdk.js)、各来源投影模块。
“调用了同一 renderer”不能替代针对原生输入形状、折叠选择和历史竞态的验证。
具体地，Codex.paintItem 保存并恢复工具输出展开、参数展开和换行开关；
NativeSdkView.applySnapshot 对没有 `_updateMessage` 的工具节点直接替换，尚未
复用这段状态保留逻辑。D08 的“部”有代码差异依据，不只是缺少测试。

### E．审批、问题和表单

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| E01 | 工具审批：显示原生操作、参数和允许/拒绝选项 | 已；命令、文件、权限请求 | 已；ACP permission options | 限；headless 不支持 control_request/response | 已；once/always/reject | 已；本次 allow/deny |
| E02 | 会话内批准或记住规则等原生选项，用户明确选择才应用 | 已；按 availableDecisions | 已；仅原生实际给的 options | 限；无网页回传 | 已；原生 always | 漏；当前只 allow/deny，未用 permission suggestions/updatedPermissions |
| E03 | 阻塞问题卡：多题、单选/多选/自由输入、原生键值回答 | 已 | 漏；官方问答反向请求存在，当前一律 unsupported | 限；当前 headless 无可回答控制协议 | 部；基本 fields 可填，复杂字段见 E06 | 已；AskUserQuestion，按问题全文编码 |
| E04 | 异步问题卡：输出继续、显式答复、保存选项、刷新恢复、显式 steer | 已；Codex async questions | 待；问答扩展与此异步语义不同 | 限；当前 headless 无该接口 | 待；表单不等价于 Codex async question | 待；user_dialog/AskUserQuestion 与此语义不同 |
| E05 | MCP elicitation：表单接受/拒绝/取消，非法 JSON 保留并提示 | 已 | 待；须确认实际 Grok 客户端扩展 | 限；当前 headless 无回传协议 | 待；已有 form，MCP 映射/完整 schema 需验证 | 漏；官方 elicitation 回调与控制类型存在，当前未宣告 |
| E06 | 复杂表单 schema 的完整处理；不是把所有字段都当文本 | 部；MCP 用 JSON 编辑，不是完整 schema 表单生成器 | 待 | 限 | 部；条件/external 等未完整处理，数值/布尔输入也需专测 | 部；AskUserQuestion 已接，request_user_dialog 未接 |
| E07 | 原生取消/别处已回答后撤卡；旧 key 不可误答另一个请求 | 已 | 已；generation key/断线清理 | — | 已；原生列表观察撤卡 | 已；control_cancel_request |
| E08 | 刷新/重开恢复仍未回答的原生请求，不重复发起审批 | 已 | 已；同一桥接运行时 | —；没有可回答请求 | 已；从原生服务重读 | 已；同一桥接运行时 |

Grok 当前 `Live.receive` 只接受 `session/request_permission`，其他反向 RPC
都回 `-32601`。这证明的是桥接未实现，不是 Grok 无问题协议。
[官方问答类型](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-tools/src/implementations/grok_build/ask_user_question/types.rs)
定义了选项、答案和 mode，
[计划批准调用](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/session/acp_session_impl/tool_calls.rs)
定义了反向请求。

Agy 的限制则是官方 headless 文档明确规定的，不应用自动放宽权限来掩盖。
Claude 的完整输入能力还包含 user dialog 与 elicitation，不能把 AskUserQuestion
接通等同于全部接通。[Claude 用户输入文档](https://code.claude.com/docs/en/agent-sdk/user-input)
和 [权限文档](https://code.claude.com/docs/en/agent-sdk/permissions)给出了相关机制。
Grok 的专门计划退出批准也未接；Codex 目前是模式切换与通用请求卡，不宣称
实现了与 Grok `x.ai/exit_plan_mode` 一一对应的专门协议。

### F．模型、强度、模式、权限和命令

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| F01 | 原生模型目录；能选模型；目录失败仍可沿用当前会话模型 | 已 | 已 | 已 | 已 | 已 |
| F02 | 独立 effort 选择，并映射本来源真正的原生字段 | 已 | 已；reasoning_effort | 部；后端接受，目录 `efforts:[]` 导致网页隐藏选择器 | 已；本来源是 model variant | 已；effortLevel |
| F03 | 显示原生有效模型/effort；保留目录外当前值和未发送用户选择 | 已；settings 通知 | 部；仅 configOptions 的 model/effort，其他模型事件未接 | 部；init 有 model，但没有完整有效 effort 回读 | 部；跟随 assistant model，未完整跟随 model_selected 等状态 | 部；model 从 init/assistant 更新，未完整跟随 settings/effort 通知 |
| F04 | 执行/计划模式切换及原生生效反馈 | 已 | 漏；`session/set_mode` | 漏；可在逐轮 argv 用 `--mode`，须显示实际模式 | 漏；可用原生 agent 目录与 switchAgent，不能硬造同名通用 mode | 漏；`setPermissionMode('plan'/'default')` |
| F05 | 审批策略切换，显示原生确认值，失败恢复实际状态 | 已 | 漏；本来源 ask/auto/yolo，不能照抄 Codex 枚举 | 部；沿用 profile，网页无显式策略入口；可用 CLI mode/参数 | 漏；session.update.permissions，不能照抄 Codex 枚举 | 漏；setPermissionMode/会话 settings |
| F06 | 沙箱策略选择 | 已；Codex 原生 policy | 待；CLI 有 sandbox，ACP 中途等价变更尚待核实 | 部；已有原生 `--sandbox`，网页/逐轮选项未接；无 Codex 三档等价承诺 | 待；当前 v2 无 Codex 三档对等证据 | 漏；官方 SDK sandbox 设置可用，在线变更/重启边界需适配 |
| F07 | 服务档位 Normal/Fast 等，使用原生 ID 并立即更新 | 已 | 待；未证明同义档位接口 | 待；boost 不等于 Codex serviceTier | 待；模型 variant 不等于通用 serviceTier | 漏；本来源 fastMode 有设置，需能力/账号限制处理，不冒充相同档位 |
| F08 | 明确的 slash 映射：model/effort/status/stop/new/help | 已 | 部；直接发给原生 prompt，不是网页控件映射 | 漏；直接进入 stream，`/model`、`/usage` 会破坏 JSON 流 | 部；当前只发 prompt.text，未接原生 command API | 部；原生可能处理部分 slash，但无网页明确映射/目录 |
| F09 | fork/revert/compact/review/plan/default/permissions/approval/sandbox 的网页命令映射 | 已 | 漏/待；必须逐命令用 ACP 扩展，不能用普通 text 冒充 | 漏/待；须独立 print 命令验证，不能塞 stream | 漏；已有多项 v2 原生接口 | 漏/待；已有多项原生能力，不能全照抄 Codex 命令名 |
| F10 | `!command`、`/shell` 直接执行并显示原生输出；输入使用工具色 | 已 | 漏；官方 TextContent `_meta.bash_command` 入口 | 待；未找到同义 headless 原生入口 | 漏；v2 session.shell | 待；需确认 headless 的本地命令语义，不能用“让模型执行”替代 |
| F11 | 未知 slash/命令保留草稿并解释，不意外当推理提示词 | 已 | 漏；当前直接转发全部正文 | 漏；同左，且有原生流破坏风险 | 漏；当前未区分 prompt/command | 漏；当前未区分网页支持/不支持的命令 |
| F12 | 新建前无需现有 UID 即可查询 SDK 原生模型目录 | 已；models 支持 node | 部；新建沿用 term/models，未开放共享无 UID native models 路由 | 部；同左 | 部；同左 | 部；同左 |

代码依据：[Codex controls](../web/codex-sdk-controls.js)、
[Codex command](../web/codex-sdk.js)、[四来源 pickers/send](../web/native-sdk.js)。
Agy 模型目录解析器明确生成 `efforts:[]`；`NativeSdkView.paintPickers` 仅在 efforts
非空时显示强度选择。这不是 CLI 不接受 `--effort`。
新建弹窗已有 `agyModelCatalog` 合并具体模型/强度组合，但 SDK 内的选择器没有
复用它；选到带强度后缀的完整 model ID 不等于已有独立 effort 控件。
新建目录的协议差异可见 [fetchModelCatalog](../web/term.js)：目前只有 Codex
切换到 SDK models 入口，另外四种仍走既有 terminal models 入口。
Agy 官方还明确要求 `/model`、`/usage` 独立 print 执行，不能送进 streaming stdin。
[headless 命令边界](https://www.antigravity.google/docs/cli/headless/#common-mistakes)

### G．底栏用量、额度和恢复

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| G01 | 账号额度窗口：已用百分比、原生窗口名、重置时间 | 已；rateLimits | 漏；billing 有原生 credits 使用率/周期，显示本来源口径 | 漏；现成 quotas collector 未接 SDK 底栏 | 待；多 provider，没有统一账号额度 API 证据 | 漏；已有 statusLine 快照，SDK 另有 rate_limit_event/get_usage 候选 |
| G02 | 当前上下文占用百分比、tokens/窗口；压缩后不沿用旧计数 | 已；last usage / context window | 漏；原生通知/模型信息有上下文数据，需选对分母 | 待；TUI statusLine 有字段，但 headless 是否触发未验证 | 漏；assistant.tokens + Model.Info.limit，需核对缓存及压缩口径 | 漏；原生 get_context_usage 返回 total/max/percentage |
| G03 | 重开立即显示已有用量，不等下一条模型输出 | 已；原生尾部恢复 context/recorded limits | 漏；可查询原生 usage/上下文并保存快照 | 部；额度页有快照，SDK 不展示；上下文恢复待核 | 漏；可读原生 messages/模型限制 | 漏；可主动查询上下文/额度或复用已有额度快照 |
| G04 | 独立读取失败保留旧值及采集时间；retry，不因失败变成 0 | 已 | 漏 | 部；collector 已保留旧值，SDK 未接 | 漏；可先实现已有 token 数据的陈旧标记 | 部；已有 quota 快照，SDK 未接 |
| G05 | 余额/credits：按本来源单位显示，缺字段不猜 | 已；balance/unlimited | 漏；billing.prepaidBalance 等原生字段 | 待；当前 collector 不采 credits，需核对独立 credits JSON | 待；没有跨 provider 统一余额语义 | 待；API cost/overage 不等于余额，未确认等价 credits |
| G06 | 重置卡数量、到期、选择与确认兑换、幂等结果、刷新额度 | 已 | —；无同义重置卡证据 | —；无同义重置卡证据 | —；无同义重置卡证据 | —；无同义重置卡证据 |
| G07 | 通知/每轮完成刷新额度；已知字段合并、不被空值擦除 | 已 | 漏 | 漏；可复用 collector，刷新频率按来源开销设计 | 待；账号额度未确认；token 刷新可补 | 漏；rate_limit_event/每轮查询未接 |
| G08 | 底栏 gauge、阈值色、tooltip；无数据时隐藏 | 已 | 漏 | 漏 | 漏；至少上下文可做 | 漏 |
| G09 | 窄屏 Usage popover/更多菜单，按实测宽度让位状态、模型/effort | 已 | 漏 | 漏 | 漏 | 漏 |

四种新视图的模板没有 `.sdk-gauges`，没有 loadLimits/paintGauges；各桥接也没有
统一 usage 状态。即便通用工具/消息保留了某条 native 记录，也不等于底栏已接。
对照 [Codex 用量读取](../crates/sessiondock/src/codex_sdk/usage.rs)、
[现有账号额度采集](../crates/sessiondock/src/quotas.rs)和[额度合同](quotas.md)。

Agy 的 `step_update.usage` 是步骤统计，`result.usage` 是累计统计；两者不能再次
累加，也不能把累计 total_tokens 当作当前上下文。statusLine 文档给出
`context_window_size/used_percentage/current_usage`，但明确是 TUI 脚本回调，
本次没有证明它在 headless 中触发。[Agy 用量输出](https://www.antigravity.google/docs/cli/headless/)
及 [statusLine 字段](https://www.antigravity.google/docs/cli/statusline/)支持这个区分。

### H．会话操作和一致性

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| H01 | 整会话原生 fork，保留来源，返回新 UID 并导航 | 已 | 漏；x.ai/session/fork | 待；TUI 有 /fork，headless 返回新身份/落盘语义未确认 | 漏；v2 session.fork | 漏；官方 forkSession / resume+forkSession |
| H02 | 从指定用户消息之前分支；恢复该输入，不自动发送 | 已 | 漏；fork+子会话 conversation-only rewind 可组合，需验证身份 | 待；未确认 headless 按消息分叉接口 | 漏；fork.before 接原生 message ID | 漏；官方 upToMessageId/resumeSessionAt，须正确选保留边界 |
| H03 | 原生回合分页选择，回退指定消息及之后对话，确认后执行 | 已 | 漏；rewind/points 与 execute | 待；未确认同义原生机器接口 | 漏；revert stage/commit | 待；resumeSessionAt 是截断恢复，不是已证明的同 SID 原地 revert |
| H04 | 回退只改对话，不回滚工作区文件；其他页面同步失效历史 | 已 | 漏；必须显式选 conversation 模式，不能默认 All | 待 | 漏；必须明确 `files:false` 并验证 commit/通知 | 待；rewindFiles 是文件回滚，不能用它代替对话回退 |
| H05 | 压缩上下文：原生执行、进度、错误和历史/计数更新 | 已 | 漏；x.ai/compact_conversation | 待；未确认 print 模式命令能力及回执 | 漏；v2 session.compact | 漏；官方支持 /compact，需接命令输出/compact_boundary |
| H06 | 原生代码审查：未提交/base branch/commit/custom，inline 显示 | 已；review/start | 待；源码 x.ai/review/* 是记录评论，不是启动 reviewer | 待；未确认同义审查接口 | 待；自定义 command/agent 可以审查，但不是已证明的同义原生 API | 待；CLI ultrareview/技能不是本会话 inline review/start 的同义物 |
| H07 | 用户消息 ⋯ 回退/分支；保留已有草稿并追加原输入 | 已 | 漏；底层 H02/H03 接通后可复用 | 待；先确认底层能力 | 漏；底层接口存在 | 部/待；分支可补，原地回退待核 |
| H08 | 会话操作图标、对话框、取消不改历史、响应错误可见 | 已 | 漏/待；按实际支持操作显示 | 漏/待 | 漏 | 漏/待 |

代码依据：[Codex actions](../web/codex-sdk-actions.js)、
[原生动作](../crates/sessiondock/src/codex_sdk/actions.rs)、
[HTTP 动作](../crates/sessiondock/src/api/codex_sdk_actions.rs)。
Grok 回退默认 `RewindMode::All`，OpenCode revert 可涉及文件；实现时必须保持
该表的 conversation-only 口径。审查尤其不能只因接口名含 review 就认定可启动审查。

### I．界面与路由

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| I01 | Hub UID 命名空间、按所属节点代理、创建选节点 | 已 | 已 | 已 | 已 | 已 |
| I02 | 窄屏/明暗主题、消息滚动跟随、同类按钮样式 | 已 | 已；基础视图已有覆盖 | 已 | 已 | 已 |
| I03 | 按整行可用宽度移动动作/用量/模式/权限到 overflow | 已 | 漏；只有较简单的模型宽度限制 | 漏 | 漏 | 漏 |
| I04 | 状态完整文字 popover、可访问标签和键盘操作 | 已；只在截断时可开 | 部；总可开，未复用相同判定 | 部 | 部 | 部 |

## 官方资料确认的可实现项

本节列工程路径，不表示已经实现。共用前端能力可复用，但请求名称、原生身份、
审批策略、用量单位和历史边界必须逐来源适配。

### Grok

| 可补齐功能 | 原生依据/建议接法 | 证据边界 |
| --- | --- | --- |
| Token、缓存、思考、成本及模型细分 | `x.ai/session/usage` / `grok usage SID` / prompt response `_meta` | 内存 ledger 在新 runtime resume 后重置；持久 usage 才包含既往记录；不完整成本不能当 0 |
| 额度/周期/余额底栏 | `x.ai/billing` 的 creditUsagePercent、currentPeriod、prepaidBalance | 按本来源单位；不伪造 Codex 5h/7d 或重置卡 |
| 上下文条 | 原生上下文通知、模型窗口元数据 | 与整会话累计 tokens 分开；安装版字段需确认 |
| 模式/审批设置 | `session/set_mode`、原生 yolo/auto 通知 | 提供本来源模式；显式操作后以原生确认状态显示 |
| 中途引导 | `x.ai/interject`，有 text/content 与 interjectionId | 原生是安全点插入，仍需实现可见回执和去重 |
| 分支/回退/压缩 | `x.ai/session/fork`、`x.ai/rewind/points`、`x.ai/rewind/execute`、`x.ai/compact_conversation` | 回退显式 conversation-only；安装版扩展需协商 |
| 图片 | ACP image content，包括原生 interject 的 structured content | 只在原生/模型声明支持时发送 |
| 问题卡和计划批准 | 官方 AskUserQuestionExtRequest/Response、`x.ai/exit_plan_mode` | 当前桥接拒绝这些反向 RPC，必须完整处理取消/失效请求 |
| 直接 shell | TextContent `_meta.bash_command` | 不用普通提示词让模型代跑；显示其实际权限口径 |
| 队列管理、持久化、闲置回收、完整 toolbar | SessionDock 层，复用 Codex 模式 | 不依赖 Grok 新增 API；不能把原生在途调用不确定性隐藏掉 |

官方证据：[ACP/扩展指南](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-pager/docs/user-guide/15-agent-mode.md)、
[会话与持久用量](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-pager/docs/user-guide/17-sessions.md)、
[usage](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/extensions/usage.rs)、
[billing](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/extensions/billing.rs)、
[interject](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/extensions/interject.rs)、
[rewind](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/extensions/rewind.rs)、
[compaction](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/extensions/memory.rs)、
[直接 shell metadata](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/extensions/prompt_meta.rs)。
ACP 通用 [session modes](https://agentclientprotocol.com/protocol/v1/session-modes)
仅证明协议形状；判断 Grok 扩展仍以 Grok 自己的实现为依据。

### Agy

| 可补齐功能 | 已有依据/建议接法 | 证据边界 |
| --- | --- | --- |
| Token 用量显示 | 提取 step_update.usage/result.usage，快照和底栏展示 | 步骤值与累计值分别标注，不相加 |
| 账号额度、重置时间 | 复用现有 `Quotas` 的独立 `-p /usage --output-format json` 查询和快照 | 不开模型 turn；沿用 SUCCESS、num_turns=0、command.name 校验 |
| 独立 effort 选择 | CLI `--effort` 和本桥接既有参数 | 需要给网页可用档位/当前值；不再返回空列表后宣称已支持 UI |
| 执行/计划及权限模式 | 逐轮进程可显式传 `--mode accept-edits/auto/plan` | 每轮生效，不伪造实时设置 RPC；权限放宽只由明确选择发起 |
| 持久队列、撤销/重试、输入消费状态、统一 toolbar | 既有 FIFO 与共用服务可扩展 | 仍不等同原生中途 steer |
| 安全的网页 slash 映射 | 把网页命令与 stream user 消息分开，查询走独立 print | 不把 `/usage` 塞进 stream 破坏 NDJSON |

仍需研究：上下文百分比（statusLine 是否在 headless 运行）、原生 fork 的机器
调用及新 SID 回执、压缩、同 SID 对话回退、credits 结构化字段、直接 shell、
中途 steer。官方 TUI 有某命令，只能证明有该功能，不能证明它适合当前 stream。
[会话文档](https://www.antigravity.google/docs/cli/conversations/)描述 `/fork`，
[statusLine](https://www.antigravity.google/docs/cli/statusline/)描述上下文回调，
[SDK overview](https://www.antigravity.google/docs/sdk/overview/)描述另外一套 Python
Agent runtime；后者不能作为 CLI 原生 DB 会话可直接恢复、审批或图片输入的证明。
本次也读了 SDK 的 [持久化](https://www.antigravity.google/docs/sdk/lifecycle)、
[策略/审批](https://www.antigravity.google/docs/sdk/policies)和
[多模态输入](https://www.antigravity.google/docs/sdk/structured-output)：SDK 本身支持
审批和图片，持久化可指定 conversation_id/save_dir/app_data_dir。切换底层到这套
SDK 是候选路线，但它是否能完整兼容现有 CLI DB、身份和多模型配置仍待验证；
表中的“限”只指当前 headless 接口，不代表 Agy 产品永久没有这些能力。

### OpenCode

| 可补齐功能 | 已核对的 2.0.32 API / schema | 证据边界 |
| --- | --- | --- |
| Token/缓存/思考/成本、上下文条 | assistant.tokens/cost、Model.Info.limit.context/input/output | 不是账号额度；以原生上下文/压缩口径计算 |
| 新分支、按消息分支 | `POST /api/session/{sessionID}/fork`，`before?` | before 是原生 message ID，不是文本搜索结果 |
| 对话回退 | `POST .../revert/stage`、`POST .../revert/commit`、`DELETE .../revert` | 显式 `files:false`，区分 stage、撤销 stage、永久提交边界 |
| 压缩 | `POST .../compact`，`id?`、`delivery?` | 默认原生可在安全点引导；与默认普通输入排队分开 |
| 队列展示、取消、立即引导 | `GET .../inbox`、`DELETE .../inbox/{id}`、`PATCH .../inbox/{id}` | delivery 枚举为 queue/steer；应明确传入选择 |
| 模式/agent、权限设置 | `POST .../agent`、`PATCH ...` 的 permissions | 用原生 agent ID/permissions，不套 Codex policy 枚举 |
| 图片/文件结构化输入 | prompt.files：uri/name/description/mention | 当前只发 `{id,text}`；需补模型媒体能力和 URI 语义验证 |
| 直接 shell、原生命令 | `POST .../shell`、`POST .../command`、`GET /api/command` | shell 输出用原生消息/事件，不经过模型 |
| 完整表单、错误重试/压缩状态、toolbar | 既有 form/assistant.retry/compaction 与共用 UI | 不能把条件字段/external 简化成所有字段文本框 |

官网 [SDK](https://opencode.ai/docs/sdk/)和 [Server](https://opencode.ai/docs/server/)
说明了 server/client 与会话操作，但本次页面仍列 v1 `/session/:id/abort` 等接口。
本表明确使用仓库既有协议实验导出的 **2.0.32 OpenAPI**：不要直接复制官网 v1
路径覆盖当前 v2 桥接。原生 SSE 直连另需解决默认共享服务的受控连接/认证；
当前 CLI `api` 会读完整 response.text，不应声称已实时转发原生 SSE。

### Claude

| 可补齐功能 | 官方 CLI 2.1.302 / SDK 0.3.302 能力 | 证据边界 |
| --- | --- | --- |
| Token/缓存/思考/成本细分 | assistant.message.usage、result.usage/modelUsage/total_cost_usd | 同一 API message ID 的多个 block 只计一次；累计值不重复相加 |
| 上下文条 | `get_context_usage` / Query.getContextUsage，返回 totalTokens/maxTokens/percentage | 用 summary detail 取已有估计时注明口径；不拿累计 modelUsage 当占用 |
| 账号额度/重置 | rate_limit_event、既有 quota statusLine 快照；原生 `get_usage` | get_usage 属实验接口；可用账号/提供商范围须能力判断，skip_behaviors 避免无关全历史扫描 |
| 执行/计划、权限模式 | Query.setPermissionMode / set_permission_mode | 原生 mode 与 Codex 的 sandbox/approval 不一一对应 |
| Fast 设置 | applyFlagSettings 的 fastMode/相关原生状态 | 模型、账号和服务端资格限制，保留失败/冷却真实状态 |
| 分支、按历史点分支 | forkSession、upToMessageId、resume+forkSession/resumeSessionAt | 应调用官方原生实现；本仓库不能自行改写 transcript |
| 压缩及进度 | 原生 /compact，status.compacting、compact_boundary | 须接命令完成和计数失效；不是普通提示词“请总结” |
| 图片 | streaming user content 的 image/base64 block | 当前桥接仅字符串正文，未接 |
| MCP elicitation、user dialog、更完整权限建议 | 官方回调、控制类型、updatedPermissions | 当前 capabilities 显式为 false/未声明；不能自动批准或改永久规则 |
| 队列管理和消费确认 | SDKUserMessage.uuid、用户回显、原生 consumed UUID/取消控制；SessionDock 可持久排队 | 原生队列可合并，撤销和 Stop 的 still_queued 必须对账，不能宣称普通 FIFO |
| toolbar、闲置回收、错误/限流状态 | 共用 UI 和原生 api_retry/rate_limit_event | 属接线与状态投影工作 |

官方依据：[CLI reference](https://code.claude.com/docs/en/cli-reference)、
[Sessions](https://code.claude.com/docs/en/agent-sdk/sessions)、
[Streaming input](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode)、
[Cost/usage](https://code.claude.com/docs/en/agent-sdk/cost-tracking)、
[Permissions](https://code.claude.com/docs/en/agent-sdk/permissions)、
[User input](https://code.claude.com/docs/en/agent-sdk/user-input)，
以及固定版本 [Agent SDK 包](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/0.3.302)
的 sdk.d.ts 和 [CLI 包](https://registry.npmjs.org/@anthropic-ai/claude-code-linux-x64/2.1.302)。
TypeScript 大页面本次抓取超长失败，具体控制字段改以该官方固定包类型核对；
没有把未读成功的页面当作证据。

`rewindFiles` 回滚文件，不是 Codex 的 conversation-only revert；`resumeSessionAt`
是截断恢复入口，还不能据此承诺活跃同 SID 的原地回退。CLI 的独立 ultrareview
也不自动等同本会话 inline reviewer。这两项保留“待”，不算已证实可完整对齐。

## Codex 自己也没有接全的能力

Codex 是本次基准，不是“全部官方 API 都已实现”。以下独立于上面的已有功能基准：

| 功能 | Codex 当前状态 | 其他来源的相近状态 |
| --- | --- | --- |
| 未落盘新建回执跨节点服务重启保留 | 漏；目前仅内存 | 四来源共享创建回执已落盘，但 live/运行队列恢复仍不同 |
| 子代理独立服务端草稿/附件身份 | 漏；目前文本-only，刻意不借父草稿 | Grok/Agy/OpenCode 同样未提供完整独立草稿附件；Claude 不独立恢复子代理 |
| 输入/输出/缓存/思考 tokens 的详细面板和会话成本 | 部；usage 数据可得，底栏只有 context tooltip 等，没有完整细分面板 | Grok/Agy/OpenCode/Claude 均有不同程度原生 token 数据；Grok/OpenCode/Claude 有成本数据；全部未接完整 SDK 面板 |
| 账号累计 Token 活动/每日用量 | 漏；官方 account/usage/read，项目未接 | 不是 G01 的限额，也不是 G02 的上下文；其他来源未证明统一同义 API |
| 后台 terminal 列表、逐个 terminate、clean | 漏；官方实验 API 可用，当前 SDK 未接 | 其他来源任务管理需分别设计，不把一般停止按钮当逐任务管理 |
| 登录/账号切换、插件/MCP 管理、完整 skills 工具面板 | 未作为现有 SDK 视图功能实现，属于另一个产品范围 | 原生 CLI/SDK 有不同能力，不以继承 CLI 配置冒充网页管理面板 |
| 目标 goal 控件和 /goal | 已明确移除，不能算本次漏实现 | 不应为追求表格对齐重新添加 |
| 非 Unix 的 Codex SDK 接入 | 未实现；现有 Unix socket 拓扑限制 | 其他来源平台范围不同，不以编译成功替代实机验证 |

官方 [app-server 文档](https://learn.chatgpt.com/docs/app-server)列出上述候选 API；
项目实际已接范围以 [Codex 合同](codex-sdk.md)与代码为准。官方 TypeScript
[Codex SDK](https://learn.chatgpt.com/docs/codex-sdk)偏自动化入口，不能混作当前
app-server bridge 的完整方法表。

## 已有验证与本次审计的边界

本次复读了六份项目 SDK 合同、共用/各来源实现、Codex compose/controls/actions、
quota collector，以及与各功能对应的浏览器套件。已有覆盖包括：

- Codex：基础/创建/composer、队列与消费确认、压缩、恢复与断线、历史窗口和分页、
  消息展开、审批/问题、状态/布局、设置/服务档位、会话动作/消息动作、子代理及终端并存。
  入口见 [validation](validation.md) 中各 `codex_sdk_*_browser.py`。
- 其他来源：[Grok](../tests/grok_sdk_browser.py)、[Agy](../tests/agy_sdk_browser.py)、
  [OpenCode](../tests/opencode_sdk_browser.py)、[Claude](../tests/claude_sdk_browser.py)
  主要覆盖 create/open/send/stream/tools/stop/reopen/去重/writer 冲突；审批和问题
  仅按当前实现覆盖。
- [共享 Hub suite](../tests/native_sdk_hub_browser.py)验证 namespace、创建、草稿、
  附件、丢 ACK 和审批/问题控件，使用的是 Claude 合成对端；不能扩大为另外三个
  来源都经过独立 Hub 实机验收。[额度 suite](../tests/quotas_browser.py)验证额度页，
  不验证目前不存在的四来源 SDK gauge。

既有真实协议实验的范围也不同：Grok 只验证未认证 initialize 和创建认证失败；
Agy 用真实 CLI 对 loopback 合成模型验证协议；OpenCode 用隔离原生服务验证
OpenAPI 与空会话接口；Claude 用临时官方固定版本 CLI 对合成网关验证协议。
它们均不能扩大为真实账号、全部工具、后台任务和高级功能已验收，详情见各来源合同。

“已有套件”表示代码中存在相关回归，本次未重跑这些 runtime suites。上一轮通过
的基础 SDK 用例也不能证明本表标为“漏/待/部”的高级功能已经实现。
本次只新增/纠正文档，运行文档链接和 AGENTS 一致性检查，不发布生产运行产物。
