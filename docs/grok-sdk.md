# Grok 原生 SDK 桥接

`grok_sdk::Bridge` 使用 Grok Build 的 ACP JSON-RPC stdio，不依赖 PTY。
桥接只运行配置中的 CLI；普通历史仍由 [Grok read model](grok-metadata.md) 读取。
本模块不实现 HTTP、Hub 或 UI；共享 facade 负责这些接入及 pending create 列表。

## 原生协议与证据

2026-10-18 核实本机 Grok Build `1.0.62 (1474f9cbdd91) [stable]`：

- `grok --help`、`grok agent --help`、`grok agent stdio --help` 确认
  `grok agent --no-leader stdio` 及模型、effort 参数。
- 安装包附带的 `docs/user-guide/15-agent-mode.md`、`17-sessions.md`、
  `26-config-reference.md` 明确规定 ACP 会话方法、configOptions、
  `GROK_HOME` 及原生存储目录。
- 独立临时 HOME、临时 GROK_HOME、loopback 合成网关、清空继承环境的
  无账号实验实际取得 `initialize` 响应：协议版本 1、`loadSession: true`、
  `sessionCapabilities.resume/close/list`、`_meta.modelState`。
  modelState 包含 `currentModelId`、`availableModels`。
  随后的无 prompt `session/new` 返回 `-32000 Authentication required`
  和 `no auth method id provided`。没有发送 `session/prompt`、模型请求或读取
  日常凭据。CLI 显式选择 `grok-4.6` / `low`，没有修改日常默认值。

协议来源为 [ACP v1 会话](https://agentclientprotocol.com/protocol/v1/session-setup)、
[prompt turn](https://agentclientprotocol.com/protocol/v1/prompt-turn)、
[工具与审批](https://agentclientprotocol.com/protocol/v1/tool-calls)。
另核对官方公开源码
[ACP 实现](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs)、
[会话创建与恢复](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/agent/mvp_agent/session_setup.rs)、
[路径编码](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-config/src/paths.rs)。
该源码快照与已安装二进制的 commit 不同；无账号实验只证明上述握手与认证失败路径，
不能替代认证后的 create/load/prompt 验收。

2026-10-19 再次用清空继承环境、临时 HOME/GROK_HOME/cwd 的安装版 1.0.62
执行无账号 initialize；命令显式选择 grok-4.6 / low，返回 currentModelId 也断言为
`grok-4.6`。原生明确广告 `promptCapabilities.image:false`、`audio:false`、
`embeddedContext:true`。因此本安装版不能按 ACP 能力承诺结构化图片；桥接保留
路径引用；共享 HTTP 在协商 image:false 时只发布已有路径引用，并在网页明确提示不传图像内容。
直接向 bridge 提供非空 images 仍明确 unsupported。合成 `image:true` 对端只验证后续
原生版本广告支持时的图片适配，不扩大为本机真实多模态验收。

下表及 facade 说明使用 Grok handler 的逻辑扩展名 `x.ai/...`。
ACP JSON-RPC wire 的自定义方法必须带前导下划线，即 `_x.ai/...`；桥接在 transport
统一编码，并将原生反向 `_x.ai/ask_user_question`、`_x.ai/exit_plan_mode` 规范为逻辑名。
2026-10-19 隔离安装版无账号只读核验：无下划线的上述扩展均返回 -32601；
正确 wire 的 `_x.ai/session/usage`、`_x.ai/rewind/points` 对不存在 SID 返回 -32002，
`_x.ai/billing` 返回 -32000（未认证），`_x.ai/session/info` 返回 `{result:{}}`。
这些结果确认安装版识别了扩展，不证明认证后账号数据和会话 mutation 已验收。

| 原生操作 | 桥接行为 |
| --- | --- |
| `initialize` | 协议版本 1；不声明客户端文件读写或 terminal 能力 |
| `session/new` | 由 Grok 生成 SID、建立原生记录；取返回的 cwd 元数据 |
| `session/load` | 使用 inventory 已解析的同一 SID/cwd，不 fork、不 restore-code |
| `session/prompt` | 文本及已协商的 image content block；异步完成，实时接收 session/update |
| `session/cancel` | 不带 RPC id 的原生通知；不结束 ACP 进程 |
| `session/request_permission` | 显示原生选项，明确选择后回送原生 outcome |
| `session/set_config_option` | model / reasoning_effort；保留原生完整 configOptions |
| `session/set_mode` | 只使用原生 modes.availableModes 的 ID |
| `x.ai/ask_user_question` / `x.ai/exit_plan_mode` | 原生问题及计划批准反向请求，网页明确提交后才回复 |
| `x.ai/session/usage` / `x.ai/session/info` / `x.ai/billing` | 进程累计用量、独立的原生上下文估计、账号额度 |
| `x.ai/compact_conversation` / `x.ai/session/fork` | 原生压缩及分支，不用模型提示词模拟 |
| `x.ai/rewind/points` / `x.ai/rewind/execute` | 原生 prompt index；明确 conversation_only、force:false |

## 启用与进程归属

`new(&Config, CancellationToken)` 只读取配置，返回
`io::Result<Option<Arc<Bridge>>>`，不启动进程。需要 Grok root 和唯一的
Grok launcher profile；只有 root、没有对应 profile 时返回 None。
执行文件和固定 argv/env 复用 [launcher profile](lifecycle-launcher.md)，
不使用 new_args/resume_args，不接受浏览器提供的 executable 或额外启动参数。
启动前移除 launcher 的 lineage 环境变量，设置与 inventory 对应的 GROK_HOME。
原生 root 必须解析到该 home 下的 sessions；自定义 root 别名需要正确的 GROK_HOME。

每个 selected native session 持有独立的 `--no-leader` ACP 进程。
创建、恢复和 models 查询按需启动；没有后台启动全量会话。
create/load 明确发送 `_meta.yoloMode: false`、`autoMode: false`。
不会添加 always-approve、静默登录、读取认证文件或改写日常模型默认。
Grok 自身的原生权限规则仍适用；桥接只转发实际收到的审批。

浏览器/SSE 离开不会取消已接受的工作。桥接持有进程、快照及请求，重新打开可继续
查看或回答。共享 shutdown token 取消时清理本桥接启动的 ACP 子进程；
不触碰外部 TUI、共享 leader 或其他 CLI 进程。

独立 TUI 无法加入本桥接的 ACP runtime，`shared_tui: false`。
open/load 和每次新 turn 前，使用现有只读 procscan 与配置 home 的 active_sessions
检查原生 SID、打开文件和 TUI 注册。有外部持有者时返回
`grok_sdk_session_busy`，不会强杀、接管或自动重试发送。
PID 为空的匹配原生活动标记也会报告冲突；已退出或 zombie PID 不占用。
该发现机制无法消除另一进程恰好在检查后开始写入的竞态，也不能在不提供 proc
观测的系统上证明外部会话空闲；不宣称具备原生跨进程写锁或共享 TUI 能力。

## facade 接口

接口是 `Bridge::call(&Arc<Self>, method, target, input)` 和
`Bridge::subscribe(key)`；模块无需修改 lib.rs 或 transport handler。

- target 由 inventory 验证后提供 `{uid, session_id, cwd, agent?}`。
  该 SID 必须属于当前选择的 native session，不能只传父会话 SID。
- create 使用 target `{source: "grok", cwd}`，input 可带
  `{request_id, model, effort}`。返回 `{uid, session_id, cwd, session, capabilities, raw}`；
  session 包含 `uid/source/session_id/sid/cwd/title/path`。
- UID 使用原生实际目录和 `crate::sessions::uid_for("grok", path)`。
  桥接扫描 native runtime 发布的目录，包括原生长 cwd 的 slug/hash 编码，
  不自造文件、猜测 hash、重序列化记录或向只读 mirror 写占位记录。
  若 Grok 尚未发布可定位目录，create 返回明确的 identity-unavailable 错误，
  缓存该次结果，不因同 request_id 重试而再创建一份。
- open 返回 `{uid, session_id, cwd, capabilities, raw}`。raw 是快照内部 raw。
  同一存活进程重复 open 不重新 load；原生回放标记 `isReplay` 的消息不放入实时区。
- turn 的 input 为 `{text, request_id?, model?, effort?, images?}`。
  运行中相同 request_id 返回原收据；已接受的 id 在本服务生命周期内也不会再次送出。
  不同输入按会话串行等待前一原生 turn 结束，HTTP 请求等待本次准入；网页断开不撤销提交，不提供隐式 steer。
  model/effort 设置失败会使该收据对应的实时快照出错，不偷偷改用另一个模型。
- interrupt 仅向运行中的会话发送 `session/cancel`；闲置返回 `grok_sdk_idle`。
- respond 使用 `{key, result}`。选项可为 `{optionId: "原生 optionId"}`，
  或原生 `{outcome: {outcome: "selected", optionId}}`；取消为
  `{outcome: "cancelled"}`。未知选项和失效 key 明确报错。
- models 对已打开会话返回 `{models: availableModels, native, configOptions, capabilities}`。
  创建前可使用 target `{source: "grok", cwd}`，仅读 initialize.modelState，
  随后关闭该探测进程，不创建会话。没有原生模型数据时明确报 unsupported。
- discard 只释放空闲桥接进程和本地连接；原生文件始终保留。
  正在运行或仍有审批时拒绝 discard。

新增 facade 方法及边界：

- `usage` 原样保留原生 input/output/cache/reasoning/modelUsage 计数；原生输入包含缓存，
  思考是输出的子集，不能把这些桶再相加。`raw.usage` 为当前 ACP 进程的累计 ledger，
  新进程恢复会重置；每次读取替换累计值，不重复相加。`last_turn` 单独保存原生 prompt
  `_meta.usage` 或 `turn_completed.usage`。缺成本、`usageIsIncomplete` 或 `costIsPartial`
  不显示完整成本；`costUsdTicks` 的单位为 1e10 ticks/USD。
- `usage` 同时尝试 `x.ai/session/info` 获取独立上下文，支持 `{result:{context:{used,total,usagePct}}}`。
  `raw.context_usage` 明确标为原生估计；读取失败/total=0 保持未知，不把累计 tokens 当占用。
  `billing` 只通过原生 `x.ai/billing` 读取，不直接访问账号或凭据文件。
- `settings` 接 `{request_id,mode?,model?,effort?}`，空闲时应用；mode 必须来自原生
  `modes.availableModes[].id`。不把执行/计划模式映射为自动批准。
- `compact`、`fork`、`rewind` 必带 request_id。空闲会话串行执行，并缓存已派发的成功和
  不确定失败结果；同一 ID 不重复执行。活跃工作/待答审批时明确 busy。
  `fork` 由原生创建新 SID、复制并记录父子关系；返回与 create 同结构的
  uid/session_id/cwd/session，不自动发 prompt。可传原生 `prompt_index`，转为
  `targetPromptIndex`；不自写或重序列化历史。分支仍使用原会话 cwd。
- `rewind_points` 保留原生 `rewind_points` 数组的 `prompt_index`、`prompt_preview`；
  `rewind` 用精确 prompt_index，始终显式 `mode:"conversation_only"`、`force:false`，
  不回滚工作区文件，不猜文本匹配点。回退成功清除旧实时投影，历史由现有 read model 刷新。
- `shell` 接 `{command,request_id?}`，通过原生 `session/prompt` 文本块
  `_meta.bash_command` 直接执行，使用 CLI 的实际执行权限；不经过模型。其回执沿用 turn 去重。
- `images:[{data,mimeType}]` 仅由共享 HTTP 层从已发布、可信附件生成；
  initialize 的 `agentCapabilities.promptCapabilities.image` 必须明确为 true 才能作为
  ACP 图像块发送。共享 HTTP 在 image:false 时继续发布附件及发送已有路径引用，
  不生成结构化 images；网页明确显示“仅发送图片路径引用，不传递图像内容”。
  直接向 bridge 传非空 images 则返回 images_unsupported。不会把路径文字冒充多模态输入。

扩展支持以原生结果为准。只读用量/额度/回退点及显式操作可尝试已有官方方法；
原生 `-32601` 记录到 `raw.extension_support` 并关闭对应 capability；认证、账号、模型、
参数错误仍明确失败，不假报成功或回退成普通 prompt。桥接声明处理能力不代表本机
1.0.62 已认证实测这些扩展；安装版与官方源码快照的版本差异仍以上述证据边界为准。

订阅 key 为主会话 `target.uid`；agent 非空时为 `uid + ":" + agent`。
snapshot 与 broadcast receiver 在同一 live mutex 下建立，没有丢事件窗口。
每次更新广播完整 JSON：

```json
{"seq":1,"type":"snapshot","raw":{"session_id":"native SID","running":true,"items":[],"requests":[],"configOptions":[]}}
```

items 使用稳定 id、role、content，并提供现有 renderer 的 text、name、result 等字段。
工具参数、原生 rawOutput、ACP content、status、error 及完整 native 更新均保留。
普通历史与实时 items 分开；桥接不读取历史来伪造实时回复。
requests 的 key 带连接 generation，晚到的旧网页回答无法批准重连后的另一个请求。
审批选项保留原生 kind，只有用户明确选择才发送 outcome。
question 为 true，表示桥接能处理实际收到的 `x.ai/ask_user_question`，不是原生每个模型均会出题。
问题按原文索引，选项使用原生 label；多选传数组，自填文字编码为 `Other` 加 annotations.notes。
`accepted`、`cancelled` 以及计划模式的 `chat_about_this`、`skip_interview` 保持原生 outcome。
计划批准保留 `approved`、`cancelled`（可带 feedback）、`abandoned`。
未实现的客户端 RPC 返回 `-32601`。

facade 应将 snapshots 作为 SSE `sdk` 事件转发；broadcast lag 时重新订阅取得快照，
不能把 lag/HTTP 断开解释成 interrupt。无需把 native 错误当作 Codex app-server 事件。
原生错误只公开静态认证错误或数字错误码，redacted stderr 留在该连接私有内存，
不通过 HTTP 或日志转发任意 provider 错误正文。

连接意外结束会清理运行标记和过期审批，保留已接受输入及诊断消息；再次 open
重新 load 同 SID，但从不自动 replay prompt。断线时原生工作可能已产生结果，
用户应结合普通历史判断。进程重启后的内存收据/去重缓存不持久化。

## 验证

[grok_sdk_browser.py](../tests/grok_sdk_browser.py) 使用临时原生目录和合成协议对端，
通过 Chromium 实际操作 SDK 页面，覆盖本来源的新建/恢复、发送、流式消息和工具、
去重、中断、重开及外部 writer 冲突；新增原生问题多选、计划批准、用量/额度、
模式、压缩、直接 shell、对话回退和分支的真实点击路径及 mutation 丢 ACK 重试断言。
另覆盖先 image:true、断线后新进程 image:false 时的实际图片上传、路径发布及可见降级说明。
合成对端证明桥接行为，不替代已认证安装版的真实模型/账号验收。
共享 Hub、草稿恢复与网页请求卡路径见
[native_sdk_hub_browser.py](../tests/native_sdk_hub_browser.py)。
完整验证命令与套件入口见[验证清单](validation.md)，协议实验的实机范围仍以上述证据为准。
