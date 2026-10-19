# 五种 SDK 功能与漏实现审计

2026-10-19，最初对照代码快照 `25178c71`，现已更新本轮补实现状态，以 SessionDock 已实现的 Codex SDK
视图为基准，核对 Grok、Agy、OpenCode、Claude 的桥接、HTTP、网页和浏览器用例，
再用官方文档、官方源码及固定版本接口证据判断可实现范围。
本轮已交付部分原生接线与网页交互；仍未完成的项目统一登记在
[TODO](../TODO.md)。表中“部/漏/待/限”保留真实边界，不表示已全量对齐。

结论：四种新 SDK 已有主要会话闭环，但没有完整对齐 Codex。用量、队列管理、
会话操作和设置中仍存在遗漏。Agy 用量底栏、effort、命令分流已补齐；
Grok/Claude 问答、三家用量及部分原生会话操作也已接通。不能把 capability=false
一概解释成产品永久不支持。图片按用户决定统一沿用原版附件路径，五种 SDK 的
专用图像转换均已移除，这属于产品取舍，不再列为待补齐功能。

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
| A16 | 无页面、无任务、无审批后释放闲置订阅/运行资源 | 已；10 分钟 unsubscribe | 已；10 分钟，未确认引导/后台任务保留 | 已；每轮 EOF 退出，不是常驻订阅 | 已；无人订阅且空闲停止观察 | 已；10 分钟，审批/控制/后台任务保留 |

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
| B05 | 原生多模态图片输入 | 已按产品决定移除；统一附件路径 | 同左 | 同左 | 同左 | 同左 |
| B06 | 发布附件一次；相同提交 ID 重试不重复上传发布/原生输入 | 已 | 已；私有持久提交账本 | 已；私有持久提交账本 | 已；持久桥接回执及原生消息 ID | 已；私有持久提交账本 |
| B07 | 原生准入后清本次草稿；保留发送期间新增编辑、失败时保留输入 | 已 | 已；持久排队准入清本次草稿 | 已；同左 | 已；同左 | 已；同左 |
| B08 | 确认输入真正被原生用户消息消费，而非仅 RPC/write 接受；压缩中断时可找回未消费输入 | 已；队列状态和原生 echo 对账 | 部；准入与消费分开，正常完成仍标未验证消费 | 部；按关联 user_input 确认，未知结果保留供核对 | 部；区分原生 inbox 与已消费消息 | 部；按原生 user UUID echo 确认，未知结果保留供核对 |
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
| C01 | 活跃时继续接收输入；按明确的队列语义等待，不隐式 steer | 已；节点 FIFO | 已；节点持久 FIFO | 已；节点持久 FIFO | 已；prompt 显式 delivery:queue | 已；节点串行提交，避免默认插入当前轮 |
| C02 | 网页显示待发送/发送中/失败项；不同页面同步同一队列 | 已 | 已；共享持久队列视图 | 已；同左 | 已；原生 inbox 与桥接失败/未知回执 | 已；共享持久队列视图 |
| C03 | 未消费项显式撤销，不误取消当前任务 | 已 | 已；尚未投递队列可撤销 | 已；同左 | 已；原生 inbox 与本地尚未投递项 | 已；尚未投递队列可撤销 |
| C04 | 失败项显式重试；失败阻塞后续项；不确定投递不自动重放 | 已 | 已；失败/未知阻塞，显式重试 | 已；同左 | 部；桥接未确认准入可处理，已入原生 inbox 由原生调度 | 已；失败/未知阻塞，显式重试 |
| C05 | 用户显式“立即引导”当前 turn，避免默认插入当前任务 | 已；`turn/steer` | 部；显式 `_x.ai/interject`，回执不证明消费 | 待；未发现同义 headless 控制接口 | 已；显式修改原生 inbox delivery:steer | 待；尚无同等显式 steer 控件/确认语义 |
| C06 | 队列、提交 ID、失败状态持久化，节点重启不静默丢失 | 已；有 state dir 时 | 已；有 state dir 时，含原提交与重试操作 ID | 已；同左 | 已；桥接准入账本与原生持久 inbox | 已；有 state dir 时，含原提交与重试操作 ID |
| C07 | 重启后不确定在途输入标记待人工核实，不自动再发 | 已 | 已；在途转 unknown，不自动重发 | 已；同左 | 已；未确认准入转 unknown，原生工作保留 | 已；在途转 unknown，不自动重发 |
| C08 | 停止当前任务；不把页面关闭当停止，不杀外部会话 | 已；原生 interrupt | 已；`session/cancel` | 已；仅自己 child 的 SIGINT | 已；原生 interrupt | 已；原生 interrupt，原生回执保留 |

代码依据：[Codex 队列](../crates/sessiondock/src/codex_sdk/queue.rs)、
[Codex 队列 UI](../web/codex-sdk.js)、[NativeSdkView.send](../web/native-sdk.js)。
四来源另有[持久输入账本](../crates/sessiondock/src/native_sdk_queue.rs)，Grok/Agy/Claude
由节点串行提交；OpenCode 保留原生 inbox 调度。正常完成、用户消息消费和节点重启
恢复是不同保证；Grok 普通 prompt 没有逐项消费证据，显式引导回执也不能冒充已消费。

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
| D08 | 长正文/思考/工具展开与折叠，流更新保留用户展开选择 | 已 | 已；共用工具展开/参数/换行状态保留 | 已；同左 | 已；同左 | 已；同左 |
| D09 | SSE 首快照与订阅原子取得、心跳、lag/reset 后重连 | 已 | 已 | 已 | 已；原生观察为轮询，非原生 SSE 直转 | 已 |
| D10 | 就绪/处理中/待批准/待回答/连接失败与完整错误可见 | 已 | 已；状态种类较少 | 已；状态种类较少 | 已；状态种类较少 | 已；状态种类较少 |
| D11 | 恢复中可重试错误、willRetry 提示、用量已满等专门状态 | 已 | 漏；扩展 retry/status 未接 | 待；尚无对等结构化事件证据 | 漏；assistant.retry/原生错误可投影 | 部；api_retry、rate_limit_event 与错误已投影 |
| D12 | 压缩专用进度卡，快照恢复；不编造百分比 | 已 | 部；原生压缩通知/状态与计数失效 | 待；须先确认 headless 压缩接口 | 部；原生 compaction 状态消息 | 部；compacting/boundary 已接，状态栏展示 |

代码依据：[Codex itemMessage/applyEvent](../web/codex-sdk.js)、
[共享 nativeSdkMessage/paintHistory](../web/native-sdk.js)、各来源投影模块。
“调用了同一 renderer”不能替代针对原生输入形状、折叠选择和历史竞态的验证。
具体地，Codex.paintItem 保存并恢复工具输出展开、参数展开和换行开关；
NativeSdkView 现已复用同样的三类控件状态保留，避免快照刷新重置工具卡。

### E．审批、问题和表单

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| E01 | 工具审批：显示原生操作、参数和允许/拒绝选项 | 已；命令、文件、权限请求 | 已；ACP permission options | 限；headless 不支持 control_request/response | 已；once/always/reject | 已；本次 allow/deny |
| E02 | 会话内批准或记住规则等原生选项，用户明确选择才应用 | 已；按 availableDecisions | 已；仅原生实际给的 options | 限；无网页回传 | 已；原生 always | 漏；当前只 allow/deny，未用 permission suggestions/updatedPermissions |
| E03 | 阻塞问题卡：多题、单选/多选/自由输入、原生键值回答 | 已 | 已；原生问题键、多选数组及取消/失效 | 限；当前 headless 无可回答控制协议 | 部；基本 fields 可填，复杂字段见 E06 | 已；AskUserQuestion，按问题全文编码 |
| E04 | 异步问题卡：输出继续、显式答复、保存选项、刷新恢复、显式 steer | 已；Codex async questions | 待；问答扩展与此异步语义不同 | 限；当前 headless 无该接口 | 待；表单不等价于 Codex async question | 待；user_dialog/AskUserQuestion 与此语义不同 |
| E05 | MCP elicitation：表单接受/拒绝/取消，非法 JSON 保留并提示 | 已 | 待；须确认实际 Grok 客户端扩展 | 限；当前 headless 无回传协议 | 待；已有 form，MCP 映射/完整 schema 需验证 | 已；form/URL 接受、拒绝、取消，原生 schema 字段 |
| E06 | 复杂表单 schema 的完整处理；不是把所有字段都当文本 | 部；MCP 用 JSON 编辑，不是完整 schema 表单生成器 | 待 | 限 | 部；条件/external 等未完整处理，数值/布尔输入也需专测 | 部；MCP 基础字段及已核实 refusal dialog；不是任意 schema/dialog 渲染器 |
| E07 | 原生取消/别处已回答后撤卡；旧 key 不可误答另一个请求 | 已 | 已；generation key/断线清理 | — | 已；原生列表观察撤卡 | 已；control_cancel_request |
| E08 | 刷新/重开恢复仍未回答的原生请求，不重复发起审批 | 已 | 已；同一桥接运行时 | —；没有可回答请求 | 已；从原生服务重读 | 已；同一桥接运行时 |

Grok 已接原生问题与退出计划批准；真实安装版核验确认扩展 wire 名需 `_x.ai/`
前缀，逻辑方法名则为 `x.ai/`，夹具也按实际 wire 编码。
[官方问答类型](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-tools/src/implementations/grok_build/ask_user_question/types.rs)
定义了选项、答案和 mode，
[计划批准调用](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/session/acp_session_impl/tool_calls.rs)
定义了反向请求。

Agy 的限制则是官方 headless 文档明确规定的，不应用自动放宽权限来掩盖。
Claude 的完整输入能力还包含 user dialog 与 elicitation，不能把 AskUserQuestion
接通等同于全部接通。[Claude 用户输入文档](https://code.claude.com/docs/en/agent-sdk/user-input)
和 [权限文档](https://code.claude.com/docs/en/agent-sdk/permissions)给出了相关机制。
Grok 的专门计划退出批准已接；Codex 目前是模式切换与通用请求卡，不宣称
实现了与 Grok `x.ai/exit_plan_mode` 一一对应的专门协议。

### F．模型、强度、模式、权限和命令

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| F01 | 原生模型目录；能选模型；目录失败仍可沿用当前会话模型 | 已 | 已 | 已 | 已 | 已 |
| F02 | 独立 effort 选择，并映射本来源真正的原生字段 | 已 | 已；reasoning_effort | 已；独立 low/medium/high/xhigh/max，逐轮 --effort | 已；本来源是 model variant | 已；effortLevel |
| F03 | 显示原生有效模型/effort；保留目录外当前值和未发送用户选择 | 已；settings 通知 | 部；仅 configOptions 的 model/effort，其他模型事件未接 | 部；init 有 model，但没有完整有效 effort 回读 | 部；跟随 assistant model，未完整跟随 model_selected 等状态 | 部；model 从 init/assistant 更新，未完整跟随 settings/effort 通知 |
| F04 | 执行/计划模式切换及原生生效反馈 | 已 | 已；session/set_mode，原生 modes 与 currentModeId | 部；网页选择后逐轮 --mode；标明下一轮生效，非实时回读 | 已；原生 agent 目录和切换 | 已；原生 permission_mode 的 plan/default |
| F05 | 审批策略切换，显示原生确认值，失败恢复实际状态 | 已 | 部；已接原生 modes；额外 auto/yolo 控制仍待补 | 部；显式 mode 逐轮传入，headless 仍无法回答审批 | 已；原生 permissions JSON 显式保存 | 已；set_permission_mode，失败保留原值 |
| F06 | 沙箱策略选择 | 已；Codex 原生 policy | 待；CLI 有 sandbox，ACP 中途等价变更尚待核实 | 部；已有原生 `--sandbox`，网页/逐轮选项未接；无 Codex 三档等价承诺 | 待；当前 v2 无 Codex 三档对等证据 | 漏；官方 SDK sandbox 设置可用，在线变更/重启边界需适配 |
| F07 | 服务档位 Normal/Fast 等，使用原生 ID 并立即更新 | 已 | 待；未证明同义档位接口 | 待；boost 不等于 Codex serviceTier | 待；模型 variant 不等于通用 serviceTier | 部；fastMode 设置和原生状态，账号资格/冷却仍由原生控制 |
| F08 | 明确的 slash 映射：model/effort/status/stop/new/help | 已 | 部；model/effort/status/usage/stop/help 明确分流；new 未接 | 部；同左；usage 独立 print 查询 | 部；网页映射及原生命令目录；new 未接 | 部；网页明确映射；new 未接 |
| F09 | fork/revert/compact/review/plan/default/permissions/approval/sandbox 的网页命令映射 | 已 | 部；fork/compact/revert 原生动作已接，其他按能力开放 | 部；用量/设置已接；高级原生命令待核实 | 部；fork/revert/compact/shell/command 已接，review 待核实 | 部；compact/settings/fork 已接；revert/review 未接 |
| F10 | `!command`、`/shell` 直接执行并显示原生输出；输入使用工具色 | 已 | 已；原生 TextContent._meta.bash_command | 待；未找到同义 headless 原生入口 | 已；原生 session.shell，输出工具卡 | 待；需确认 headless 的本地命令语义，不能用“让模型执行”替代 |
| F11 | 未知 slash/命令保留草稿并解释，不意外当推理提示词 | 已 | 已；共用网页命令分流并保留未识别输入 | 已；同左，不进入 NDJSON | 已；同左，原生命令另有显式入口 | 已；同左 |
| F12 | 新建前无需现有 UID 即可查询 SDK 原生模型目录 | 已；models 支持 node | 部；新建沿用 term/models，未开放共享无 UID native models 路由 | 部；同左 | 部；同左 | 部；同左 |

代码依据：[Codex controls](../web/codex-sdk-controls.js)、
[Codex command](../web/codex-sdk.js)、[四来源 pickers/send](../web/native-sdk.js)。
Agy 目录现提供 CLI 接受的 effort 档位，网页实际选择后按每轮 argv 传入。
新建弹窗已有 `agyModelCatalog` 合并具体模型/强度组合，但 SDK 内的选择器没有
复用它；选到带强度后缀的完整 model ID 不等于已有独立 effort 控件。
新建目录的协议差异可见 [fetchModelCatalog](../web/term.js)：目前只有 Codex
切换到 SDK models 入口，另外四种仍走既有 terminal models 入口。
Agy 官方还明确要求 `/model`、`/usage` 独立 print 执行，不能送进 streaming stdin。
[headless 命令边界](https://www.antigravity.google/docs/cli/headless/#common-mistakes)

### G．底栏用量、额度和恢复

| ID | Codex 已实现的功能与口径 | Codex | Grok | Agy | OpenCode | Claude |
| --- | --- | --- | --- | --- | --- | --- |
| G01 | 账号额度窗口：已用百分比、原生窗口名、重置时间 | 已；rateLimits | 已；原生 billing 额度与周期 | 已；既有 source-specific quota 查询、时间与重置 | 待；多 provider，没有统一账号额度 API 证据 | 部；原生 rate_limits 详情及既有 quota 快照，非全部限额条归一 |
| G02 | 当前上下文占用百分比、tokens/窗口；压缩后不沿用旧计数 | 已；last usage / context window | 已；原生 session/info 上下文；无数据不猜 | 待；TUI statusLine 有字段，但 headless 是否触发未验证 | 已；最近原生请求估计，压缩后失效 | 已；原生 summary total/max/percentage |
| G03 | 重开立即显示已有用量，不等下一条模型输出 | 已；原生尾部恢复 context/recorded limits | 部；重开主动读取；ledger 是当前 ACP 进程累计 | 部；账号额度快照可恢复；token live 不跨服务重启 | 已；原生 session/message/模型目录读取 | 已；主动读取原生 context/usage，失败保留已知值 |
| G04 | 独立读取失败保留旧值及采集时间；retry，不因失败变成 0 | 已 | 部；失败可见与重试；没有完整陈旧时间标记 | 已；quota 保留旧值/采集时间，累计与步骤分开 | 部；读取失败保留快照，完整陈旧标记待补 | 部；错误/实验接口降级可见；没有完整陈旧时间标记 |
| G05 | 余额/credits：按本来源单位显示，缺字段不猜 | 已；balance/unlimited | 已；prepaidBalance 按美分换算，字段缺失隐藏 | 待；当前 collector 不采 credits，需核对独立 credits JSON | 待；没有跨 provider 统一余额语义 | 待；API cost/overage 不等于余额，未确认等价 credits |
| G06 | 重置卡数量、到期、选择与确认兑换、幂等结果、刷新额度 | 已 | —；无同义重置卡证据 | —；无同义重置卡证据 | —；无同义重置卡证据 | —；无同义重置卡证据 |
| G07 | 通知/每轮完成刷新额度；已知字段合并、不被空值擦除 | 已 | 部；回合完成主动读 usage/billing；采集失败保留 | 部；回合结果更新 Token；额度打开/显式刷新 | 待；账号额度未确认；token 刷新可补 | 已；rate_limit_event、每轮原生查询 |
| G08 | 底栏 gauge、阈值色、tooltip；无数据时隐藏 | 已 | 已；使用原生单位与上下文 | 已；累计 Token/额度；不伪造上下文 | 已；Token/上下文；不伪造账号额度 | 已；Token/上下文及限额详情 |
| G09 | 关键用量常显，窄屏自动换行；原生明细/刷新放入更多菜单 | 已 | 已；共用用量底栏 | 已；同左 | 已；同左 | 已；同左 |

四种新视图现共用 [Native SDK controls](../web/native-sdk-controls.js) 的用量底栏、
明细与窄屏换行；各桥接仍保留本来源计量口径。缺数据不显示 0，不把累计值当上下文。
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
| H01 | 整会话原生 fork，保留来源，返回新 UID 并导航 | 已 | 已；原生 fork，持久网页回执和新 UID | 待；TUI 有 /fork，headless 返回新身份/落盘语义未确认 | 已；原生 fork，持久网页回执和新 UID | 已；精确路径 resume/fork-session，首次输入原生落盘 |
| H02 | 从指定用户消息之前分支；恢复该输入，不自动发送 | 已 | 部；原生 targetPromptIndex 已接；输入恢复/完整消息入口待补 | 待；未确认 headless 按消息分叉接口 | 部；原生 before ID 与 live 消息入口；历史消息菜单/原输入恢复待补 | 部；原生 user.parentUuid 冻结边界；完整历史入口/输入恢复待补 |
| H03 | 原生回合分页选择，回退指定消息及之后对话，确认后执行 | 已 | 已；原生 rewind_points 选择与 conversation_only | 待；未确认同义原生机器接口 | 部；可见消息选择和 stage/commit；完整分页选择待补 | 待；resumeSessionAt 是截断恢复，不是已证明的同 SID 原地 revert |
| H04 | 回退只改对话，不回滚工作区文件；其他页面同步失效历史 | 已 | 部；明确 conversation_only；跨页面历史失效需专项验收 | 待 | 部；明确 files:false，stage/撤销/commit；跨页需专项验收 | 待；rewindFiles 是文件回滚，不能用它代替对话回退 |
| H05 | 压缩上下文：原生执行、进度、错误和历史/计数更新 | 已 | 已；原生 compact，计数失效/刷新 | 待；未确认 print 模式命令能力及回执 | 已；原生 compact 消息/状态 | 已；原生 /compact、status、boundary、错误 |
| H06 | 原生代码审查：未提交/base branch/commit/custom，inline 显示 | 已；review/start | 待；源码 x.ai/review/* 是记录评论，不是启动 reviewer | 待；未确认同义审查接口 | 待；自定义 command/agent 可以审查，但不是已证明的同义原生 API | 待；CLI ultrareview/技能不是本会话 inline review/start 的同义物 |
| H07 | 用户消息 ⋯ 回退/分支；保留已有草稿并追加原输入 | 已 | 部；有原生分支/回退；完整消息操作/草稿恢复待补 | 待；先确认底层能力 | 部；live 消息分支入口；历史菜单/回退入口与输入恢复待补 | 部；live 用户消息之前分支；原地回退、完整历史入口/输入恢复待核 |
| H08 | 会话操作图标、对话框、取消不改历史、响应错误可见 | 已 | 已；按能力显示菜单/对话框/错误 | 部；设置与用量；高级动作待核实 | 已；菜单/对话框/显式永久回退确认 | 部；设置/压缩/分支入口；原地回退未接 |

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
| 图片 | ACP 原生具有 image content 能力 | 产品决定不接，统一附件路径 |
| 问题卡和计划批准 | 官方 AskUserQuestionExtRequest/Response、`x.ai/exit_plan_mode` | 现已接反向 RPC 与取消/失效请求；只宣告已验证的类型 |
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
| 独立 effort 选择 | CLI `--effort` 和本桥接既有参数 | 现已给网页可用档位；原生有效 effort 回读仍有边界 |
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
| 图片/文件结构化输入 | prompt.files：uri/name/description/mention | 产品决定不接，统一附件路径 |
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
| 图片 | streaming user content 的 image/base64 block | 产品决定不接，统一附件路径 |
| MCP elicitation、user dialog、更完整权限建议 | 官方回调、控制类型、updatedPermissions | 现已接 elicitation 和核实的 refusal dialog；完整权限建议仍待补，不能自动批准 |
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

Claude Code / Agent SDK 也可接 OpenRouter 的 Anthropic 兼容入口：
`ANTHROPIC_BASE_URL=https://openrouter.ai/api`，用 OpenRouter key 设置
`ANTHROPIC_AUTH_TOKEN`，并显式留空 `ANTHROPIC_API_KEY`。仅覆盖 URL 不能让
订阅认证变成 OpenRouter key；OpenRouter 文档只保证 Anthropic 1P 兼容性。
本轮保留现有 Claude 路由，未把此文档能力记为 Claude 经 OpenRouter 实测通过。
依据：[Claude Code 接入](https://openrouter.ai/docs/cookbook/coding-agents/claude-code-integration)、
[Agent SDK 接入](https://openrouter.ai/docs/guides/community/anthropic-agent-sdk)。

## Codex 自己也没有接全的能力

Codex 是本次基准，不是“全部官方 API 都已实现”。以下独立于上面的已有功能基准：

| 功能 | Codex 当前状态 | 其他来源的相近状态 |
| --- | --- | --- |
| 未落盘新建回执跨节点服务重启保留 | 漏；目前仅内存 | 四来源共享创建回执已落盘，但 live/运行队列恢复仍不同 |
| 子代理独立服务端草稿/附件身份 | 漏；目前文本-only，刻意不借父草稿 | Grok/Agy/OpenCode 同样未提供完整独立草稿附件；Claude 不独立恢复子代理 |
| 输入/输出/缓存/思考 tokens 的详细面板和会话成本 | 部；usage 数据可得，底栏只有 context tooltip 等，没有完整细分面板 | 四来源已接 Token 明细，Grok/OpenCode/Claude 接本来源成本；范围/缺字段/陈旧时间仍各异 |
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
  来源都经过独立 Hub 实机验收。[额度 suite](../tests/quotas_browser.py)验证额度页；
  四来源 SDK 底栏由各来源浏览器套件验证，不能用额度页测试代替。

既有真实协议实验的范围也不同：Grok 验证未认证 initialize、创建认证失败及只读扩展 wire 识别；
Agy 用真实 CLI 对 loopback 合成模型验证协议；OpenCode 用隔离原生服务验证
OpenAPI 与空会话接口；Claude 用临时官方固定版本 CLI 对合成网关验证协议。
历史实验中 Grok 1.0.62 的 initialize 确认 image:false；当时测试的结构化图片适配
现已按产品决定移除，当前浏览器验收统一检查附件发布与文本路径。它们均不能扩大为真实账号、全部工具、后台任务和高级功能
已验收，详情见各来源合同。

第一批补实现扩展并重跑四来源 Chromium、共享 Hub、quota 回归，验证真实网页
点击/输入/上传/回答/队列/会话操作，使用隔离合成原生对端；宽窄屏与明暗截图
已人工查看。它们验证桥接与 UI，不替代真实账号/全部模型的生产协议验收。
第二批完成四来源、共享 Hub、Grok 生命周期和持久队列七套 Chromium 验收，
另通过历史读取/分页两套回归。覆盖双标签页真实撤销、失败与未知结果的明确重试、
节点重启不重发、附件只发布一次、新草稿保留、Claude 原生分支及闲置重开。
SDK 视图暂停额外历史长连接，修复 HTTP/1 双标签页操作阻塞；真实点击验证了
离开 SDK 恢复历史 watch、跨会话不恢复旧连接。宽窄屏明暗截图已人工查看。
已补持久队列/失败恢复、Grok/Claude 闲置回收与 Claude 原生分支；原生消费证据、
完整历史消息操作/输入恢复、子代理专项验收及若干高级能力仍有边界。

## 真实会话验收流程

2026-10-19 本轮按用户明确要求，使用各机器现有 CLI、认证、配置和服务创建
专属测试会话，不复制认证、不迁移登录、不更改日常默认模型。测试工作文件放在
专属临时目录；这不等于隔离 CLI HOME。每次新建和 fork 都立即登记节点、来源、
UID、原生 SID 与 cwd，失败创建留下的原生孤儿也纳入清理清单。

1. 先只读核对该节点实际 CLI、版本、网页能力和模型目录。网页选择指定低成本
   模型/effort，检查创建请求及原生实际模型；不自动换成更贵模型。目录错误、
   启动失败、提供商拒绝分别记录，不能只凭 HTTP 200 认定可用。
2. 从正常网页新建 SDK 会话，发送唯一标记与记忆口令，等真实模型回复；刷新后
   保持同一原生 SID，再要求工具读取测试输入文件并生成含文件内容与记忆口令的
   输出文件。对照原生历史、工具卡及磁盘结果，验证上下文、执行和展示。
3. 新编辑草稿后刷新，检查草稿、历史和模型保持；查看真实 Token/上下文/额度
   底栏与详情。分别检查宽屏、窄屏、明暗主题，缺原生字段不能当作数值零。
4. 让真实工具执行一个有自动超时的测试命令，期间排入第二条输入。第二个
   标签页撤销未投递项，检查两页同步、当前工具未误停、被撤销文件未生成；
   显式 Stop 后检查真实任务结束、发送期间新增草稿保留。仅操作登记的测试任务。
5. 对有能力的来源分别检查审批/问答、普通附件/图片、分支、对话回退、压缩及
   引导。必须出现真实原生事件/结果才记通过；例如 compact 的准入成功不能
   替代压缩完成，fork 返回 SID 也不能替代分支首次对话。原生不触发的项目记
   未覆盖，不让合成夹具代替真实模型结果。
6. 实测故障先留证，补针对性 Chromium 回归，审查、提交、发布后再走正常网页
   复测。路由改写只可用于定位，不能计入正常路径验收。共享服务不为测试故意
   崩溃或杀死其他会话；节点崩溃/断网等破坏性场景在临时服务回归中验证，明确
   标为合成验证。
7. 结束时停止自有任务，先删分支再删父会话，清除失败创建回执/孤儿；仅永久
   清理这些 UID 返回的回收站条目。核对原生记录、网页列表及待落盘行都已消失，
   再删除测试工作目录。保留脱敏结果和截图，不保留认证副本。

每项结果分别标记“真实通过”“合成回归通过”“原生限制”“环境阻断”或
“未覆盖”。本节流程不代表所有步骤、所有来源已经完成；最终实测结果另列，
不能把前面的代码实现状态直接转换成真实验收通过。

## 2026-10-19 当前环境实测结果

本轮使用 Chromium 操作正式页面、现有原生 CLI 与真实提供商，未复制认证。
下表只计实际出现的结果，不把协议夹具或请求准入替代模型执行完成。

| 来源与实际模型 | 真实通过 | 限制、阻断与未覆盖 |
| --- | --- | --- |
| Codex `gpt-5.6-luna` / low | 正常网页新建、多轮记忆、刷新恢复、文件工具、用量、草稿；后续四轮双标签页撤销和 Stop 成功，被撤销输入未执行、新草稿保留 | 两次撤销无响应超时未能重现，保留待查；本轮未测审批、图片、分支/回退/压缩 |
| Grok `grok-4.6` / low | 修复后正常模型目录/新建、多轮记忆、刷新、真实文件审批一次允许、Write/Read、Token/成本/上下文/额度、双标签页队列撤销、Stop、显式引导、原生 fork、压缩完成计数 | 引导只证明原生接收，consumed 仍为空；分支返回原生复制数量，本轮未验证分支首次模型回复；未测问答、图片及回退 |
| Agy `gemini-3.8-flash-low` | 正常网页新建、多轮记忆、刷新、真实文件工具、草稿、双标签页队列撤销/Stop；当次 Token 底栏及真实额度刷新；异步用量菜单修复后按钮可点击 | Token 快照未承诺跨服务重启恢复；headless 审批/图片限制仍按合同；高级操作不因其他 SDK 通过而算通过 |
| OpenCode `openrouter/google/gemini-2.5-flash-lite` / none | 模型目录修复后完整返回 765 个模型；真实 shell、fork、conversation-only revert stage/unrevert、原生 inbox 撤销；已 steer 项不再提供重复引导，窄屏用量可用 | 最初 free 模型拒绝非官方客户端；显式改选 Flash Lite 后原生报额度不足。未获得成功模型回复/模型工具；compact 虽准入但最终 quota error；STOP、审批/问答未覆盖 |
| Claude `claude-haiku-4-5-20251001` / low | 使用有 CLI 的节点现有认证，原生报告 firstParty；真实多轮、Write/Read、刷新、AskUserQuestion、用量、队列撤销/Stop、完整 fork 和选点 fork 的真实后续回复、压缩边界 | 另一节点 CLI 缺失已正确展示启动诊断；普通工具审批未触发；sleep 被原生策略拒绝，Stop 验证不声称成功运行该命令；图片/MCP 未覆盖 |

修复项包括 Grok 配置值格式与新建目录、OpenCode 大型 stdout 截断、Claude
启动错误丢失/恢复前 SessionStart 遥测误判、SDK 额外历史 SSE、异步用量菜单
定位、被替代历史读取的取消误报，以及运行中工具被误标完成。针对这些输入形状
增加 Chromium 回归；共享服务恢复另走发布后的真实会话复验。

未在共享生产环境故意杀 daemon、制造节点崩溃、断网或耗尽真实账号额度。
重启/未知投递等故障注入保留为临时服务合成回归。未覆盖项与两类待定位问题
见 [TODO](../TODO.md)，本轮结果不能解释为五种 SDK 的每个功能均已验收。

交付代码为 `3f6d9b0b`、`e167c0f8`，已推送；三台 Linux 节点与 Hub 均发布、
重启并通过健康检查。相关 Chromium 回归覆盖四来源 SDK、Grok 生命周期、
新建模型选择、持久队列、双标签页、普通历史/分页及 SDK 历史分页；另通过
Rust 构建/格式、JavaScript 语法及文档检查。发布后 Claude 原 SID 恢复得到
真实 `CLAUDE_LIVE_RESUME_OK`，实际模型仍为 Haiku / low；Grok 当前压缩历史
与原生记录一致，分支保留压缩前历史；本轮刷新/重选复验未再出现历史加载的
`AbortError`。

清理按登记身份完成：10 个原生测试会话（含分支与失败创建的 Grok 孤儿）及
1 个未落盘 Claude 创建回执均已移除；仅清空这些会话返回的回收站条目。
OpenCode 原生数据库、Agy 原生索引/数据库/brain、Claude/Grok/Codex 原生
文件及网页列表/待落盘行均核对不存在。Codex 使用原生精确 SID 永久删除；
Agy 网页删除超时后使用原生当前会话确认删除。两机测试工作目录已清理，
本轮脱敏结果、失败记录和截图作为验收证据保留在仓库外。
