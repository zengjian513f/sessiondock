# OpenCode 原生 SDK 桥接

`crates/sessiondock/src/opencode_sdk/` 使用 OpenCode **v2** 的原生机器 API。
普通历史仍由[只读数据库 mirror](opencode.md)提供；桥接不写 SQLite、mirror
或 TUI，不自造原生记录。共享 HTTP、Hub 与 UI 由 `native_sdk` facade 接入。

## 配置与服务生命周期

`new(&Config, CancellationToken)` 只读取配置。只有 OpenCode root 和
[launcher](lifecycle-launcher.md) 中恰好一个 OpenCode profile 同时存在才返回
`Some(Arc<Bridge>)`；只有 root 或没有 profile 返回 `None`，不运行 CLI。
执行使用 profile 的 executable、固定 args、env、env_remove，清除 launcher
规定的旧会话环境变量；没有浏览器可控的可执行文件或 shell 拼接。

桥接连接**已经运行的默认原生共享服务**。每次 API 请求先通过原生
`service get disabled` 和 `service status` 检查；禁用或未启动时明确报错，
`autostart:false`。服务由用户正常启动 OpenCode 或 `opencode service start`
管理。SDK 不启动、停止或升级共享 daemon，不使用临时 standalone server。
profile 固定参数中的 `--standalone`、`--server` 明确不支持。

桥接自己启动的短命 API 客户端使用独立进程组、关闭 stdin、并行收集
stdout/stderr。shutdown 或 30 秒 API 超时终止并回收这些客户端；已交给原生
daemon 的工作不因 SSE 断开、页面离开或客户端退出而取消。API 超时不重送
已接受输入，原生记录继续用于观察和对账。stderr 的已知环境密钥、Basic、
Bearer 和含密码 URL 脱敏后用于错误诊断，不打印原始 stdout/stderr。

注意原生 CLI 的连接实现仍有边界：`api` 自身在检查与执行之间使用
`service.ensure`；如果原生服务恰在预检后消失，CLI 可能执行自己的恢复。
桥接没有调用 service stop/restart，也不持有或终止独立 TUI。若交付要求连
这个原生恢复窗口也必须消除，需要 OpenCode 提供使用其受控认证的
connect-only 客户端入口；不能把 `api --server URL` 的认证行为凭空等同于
默认服务发现。

## 原生接口证据

2026-10-18 本机 `opencode --version` 返回 `opencode v2.0.32`。
`opencode --help`、`opencode serve --help`、`opencode api --help` 及安装程序
内嵌的 CLI/server 路由定义确认：

| 用途 | v2 API |
| --- | --- |
| 新建、恢复身份 | `POST /api/session`、`GET /api/session/{sessionID}` |
| 发送 | `POST /api/session/{sessionID}/prompt`，`{id?,text,...}`，返回持久 inbox 接收记录 |
| 会话模型 | `POST /api/session/{sessionID}/model`，`{model:{providerID,id,variant?}}` |
| 运行状态、待处理输入 | `GET /api/session/active`、`GET /api/session/{sessionID}/inbox` |
| 消息与流中投影 | `GET /api/session/{sessionID}/message`，`{data,cursor:{previous,next}}` |
| 中断 | `POST /api/session/{sessionID}/interrupt?resume=false` |
| 审批 | `GET /api/session/{sessionID}/permission`、`POST .../permission/{requestID}/reply`，`{decision:"once"|"always"|"reject"}` |
| 问题表单 | `GET /api/session/{sessionID}/form`、`POST .../form/{formID}/reply`，`{answer:{fieldKey:value,...}}` |
| 模型目录 | `GET /api/model?location[directory]=...` |

使用 `serve --stdio --hostname 127.0.0.1 --port 0` 在临时 HOME、临时 XDG
目录启动隔离原生服务，设置临时服务密码和仅指向 loopback 的模型目录 URL，
读取 `/openapi.json` 验证上述 schema。另以合成 SID 创建空会话，验证返回的
`projectID`、`location.directory`，以及 message、permission、form、active、
inbox 的空响应结构；没有提交 prompt 或调用账号/模型。服务 stdin 关闭后
退出并回收，未接触日常配置或生产数据。

官方[SDK 文档](https://opencode.ai/docs/sdk/)说明 SDK 通过原生 server 工作；
[Server 文档](https://opencode.ai/docs/server/)和
[CLI 文档](https://opencode.ai/docs/cli/)提供背景。核实时官网 Server 页面
仍列有 v1 `/session/:id/message` 等路径；本桥接以安装版 v2 OpenAPI 为准，
不把旧版 `/prompt_async`、`/abort` 或 Codex app-server 协议套到 v2。
内嵌 `cli.api` 实现先读取整个 `response.text()`，不能转发原生 SSE；因此桥接
分页读取原生消息投影并广播完整快照。活跃/有订阅时两次观察间隔 350ms，
无人订阅且空闲时停止轮询，由下次订阅或输入唤醒，实际延迟另含原生 CLI/API 开销；从 newest-first 页读取当前原生回合，首次读到最近 user、后续读到已知尾部即停止；只在尚未触及边界时跟随 next cursor，普通历史分页不被 SDK 展开。

## facade 接口

模块提供约定的 `Bridge`、`new`、`call`、`subscribe`，不修改 `lib.rs`。
共享 `native_sdk` dispatcher 分派来源；target 已由 inventory
解析为 `{uid,session_id,cwd,agent}`，create 为 `{source:"opencode",cwd}`。

`call` 支持 `open/create/turn/interrupt/respond/models/discard`，以及下面的原生控制；原生不支持
可撤销 discard，明确返回错误，`capabilities.discard:false`。原生删除属于
既有已确认的删除路径，不借 discard 悄悄执行不可撤销删除。

open 返回 `{uid,session_id,cwd,key,capabilities}`。create 返回同样身份以及
`session:{uid,source:"opencode",session_id,sid,cwd,title,model}`。原生 SID 由
原生 create API 接受，UID 使用 `sessions::uid_for("opencode",
root/projectID/sessionID)`，与 inventory 的原生目录身份完全相同；bridge
不等待 mirror、也不代写草稿。共享服务负责 pending 列表与原生尚未可见时的草稿。
create 的 request_id 保留同一原生 SID，模糊失败后的重试先 get 同一 SID。

订阅 key 为 target.uid；target.agent 非空时是 `uid + ":" + agent`。
这是已选原生 SID 的别名，不能将父 SID 加上 agent 当作子会话；OpenCode
子 agent 是独立 SID，facade 必须先解析到那个 native session。别名复用同一
Session 状态和 observer。open 校验 native SID、cwd、UID 与 target 相同。

snapshot 与 receiver 在同一个状态锁中建立，更新始终广播完整内容：

```json
{"seq":1,"type":"snapshot","raw":{"session_id":"ses_…","running":true,"items":[],"requests":[],"model":{"providerID":"…","id":"…"},"effort":"…","error":null}}
```

facade 把初始 snapshot 和后续 receiver 内容作为 `sdk` SSE 事件发送；遇到
broadcast lag 时重新 `subscribe(key)` 取最新 snapshot 即可。普通历史作为
baseline 保留在历史区；只有桥接接受或打开后观测变化/新增的消息进入 live
items。user 保留原生 message ID；assistant/thinking/tool 使用原生 ID 加
内容索引作为稳定 ID，另有 `native_id` 供对账。tool 保留 input、arguments、
result、error、status 和原生 part。原生 provider error 独立呈现；aborted
保留部分文本与 interrupted 标记。

turn 以原生消息 ID 去重，正在运行的同 request_id 返回原接收回执；不同
request_id 使用原生持久 inbox 排队，普通输入显式发送 `delivery:"queue"`，不通过 PTY。请求独立后台任务运行，
操作锁在回执之前取得，后续 interrupt 不会抢在已接受 prompt 之前执行。
input.model 接受 `provider/model` 或 `{providerID,id,variant?}`；effort 是
原生 **variant** 字符串，不假装通用推理档位。模型设置仅通过原生 session
switchModel 影响会话后续 provider turn，不改全局日常默认值。

审批 key 为 `permission:<原生 request ID>`，respond.result 接受
`once/always/reject` 或 `{decision:...}`。问题 key 为 `form:<原生 form ID>`，
result 是按原生 field key 编码的对象，也接受 `{answer:{...}}`；单字段表单
可直接传该字段值。多个字段、条件字段和 external 类型需要共享 UI 按原生
fields 渲染或提交完整 JSON。桥接不自动批准任何审批，不自动回答表单。
models 只返回模型标识、名称、variants、enabled，不回传 provider 凭据设置。

## 用量、队列与原生控制

`raw.usage.tokens` 和 `cost_usd` 直接取原生 `Session.Info.tokens/cost`，包括
输入、输出、思考、缓存读写细分；观察流重复更新不会反复累加。它们是会话累计
用量与美元成本，不是账号额度。`quota:null` 表示没有经验证的账号额度接口。
`models` 只额外暴露无凭据的 `limit` 和 `capabilities`，不回传模型 headers/body。

`usage.context` 是明确标记 `estimated:true` 的最近 provider 请求估计：
`input + cache.read + cache.write + output` 对原生 `Model.Info.limit.context`
的比例；思考 token 不重复加到 output。它不把会话累计 tokens 当作上下文。
已完成 compaction 晚于最近带 usage 的 assistant、切换模型，或缺少原生 limit
时返回 null，等待下一次 provider usage。此估计不含之后尚未计量的新输入，
也不冒充原生账号或精确即时占用仪表。

`raw.queue` 投影原生持久 inbox，保留 `id/type/delivery/native` 和 user text；
这是尚未投递的工作，不能把 HTTP 已准入回执等同于已被模型消费。
`queue_cancel({id})` 使用 DELETE inbox，原生已不可用项目为 no-op；
`queue_update({id,delivery})` 使用 PATCH，支持 queue/steer。Steer 在原生
安全点引导，不保证立即打断当前工具。中断保持 `resume=false`，不主动清队列。
共享 facade 另持久保存提交 ID 和未确认准入，显示在 `raw.input_queue`，不取代原生
inbox。原生 POST 成功标记 native_accepted，已看到原生 inbox 的回执标记
native_queued，看到原生 user message 才标记 observed 消费。原生多次 GET 轮询可能
跨越提交，单个陈旧的空闲快照不证明投递失败。重启后尚未确认的在途输入显示
unknown，不自动重发。
恢复和显式重试的边界见[持久输入账本](native-sdk.md#持久输入与失败恢复)。

| facade 操作 | 原生语义 |
| --- | --- |
| `fork({request_id,before?})` | POST fork；省略 before 复制全部投影历史，before 为原生消息 ID，复制该消息之前的历史；返回独立 SID/UID 和 create 同形的 session |
| `revert({message_id})` | POST revert/stage，始终 `files:false`，只暂存可撤销的对话边界，不恢复工作区文件 |
| `unrevert` / `revert_commit` | DELETE revert 撤销暂存 / POST revert/commit 永久提交边界；提交须在 UI 明确区分 |
| `compact({request_id,delivery?})` | POST compact，默认显式 steer；原生持久压缩请求，不发送“请总结”提示词 |
| `shell({request_id,command})` | POST session shell；直接执行命令，原生 shell 消息投影保留输出、状态与退出码，不调用模型 |
| `commands` / `command({request_id,name,text,files?,delivery?})` | 查询原生 command 目录 / 执行原生回调；默认 delivery queue，不把任意斜杠文本假装成已注册命令 |
| `agents` / `settings({native_agent?,permissions?})` | 查询原生 agent 目录，POST agent 切换后续 provider turn；PATCH session 的原生 Permission.Ruleset，不套用 Codex policy 枚举 |

fork、shell、command、compact 在调用前按 request_id 在当前桥接进程预留回执，
成功后保存结果；失败或响应丢失时保留未确认状态，同 ID 重试明确报错，不重发
原生操作。原生 fork/command 没有可预留的幂等请求身份，未确认状态须核对原生
记录，不能自动推断原生没有执行。进程重启后只有共享 facade 已保存的成功 fork
回执继续去重；不承诺未确认操作跨节点重启的 exactly-once。原生删除、永久回退
提交与可撤销 discard 仍是不同操作。

`turn.files` 与 `command.files` 使用 v2 `PromptInput.FileAttachment` 的
`{uri,name?,description?,mention?}`；共享 HTTP 只从已授权发布的附件生成
URI，不接受客户端任意路径。OpenAPI 只约束 uri 为字符串；file URI 和 data URL
的实际读取、图片格式及模型媒体能力由原生服务决定并保留其错误，合成测试不代表
所有 provider/模型都经过真实图片验收。普通文本路径引用仍可降级工作。

## 并存限制

默认 TUI 与桥接通过同一 HOME/profile 下的默认原生 daemon 操作同一个 SID。
`session.active` 只报告那个 OpenCode 进程拥有的运行任务，不代表所有独立
runtime。Linux 用配置的 proc_root 拒绝同 SID 或同 cwd 的
`--standalone/--server` OpenCode 进程，不终止或接管它们。跨机器的独立
runtime、进程启动后才选择的其他 SID、以及非 Linux 的外部 writer 不能靠
这个 proc 观察完整识别；这些情形不承诺原生共享，须关闭独立 runtime 后
再使用 SDK。原生 API 返回的冲突保持可见，不绕过原生拒绝。

## 验证

[opencode_sdk_browser.py](../tests/opencode_sdk_browser.py) 使用临时原生目录和合成协议对端，
通过 Chromium 实际操作 SDK 页面，覆盖新建/恢复、发送、流式消息和工具、
去重、中断、重开、外部 writer 冲突、审批/问题，以及用量/上下文、原生 inbox
撤销/steer、图片上传、agent/permissions、原生命令、shell/compact、分支、
暂存/撤销/提交对话回退。原生执行后故意断开 ACK，验证 shell/fork 同 request_id
重试不重复执行。加 `--hub-binary` 使用真实 Hub 验证 fork 命名空间、重新打开
新身份、SSE/SEND 路由和成功回执重试。共享 Hub、草稿恢复与网页请求卡路径见
[native_sdk_hub_browser.py](../tests/native_sdk_hub_browser.py)。
完整验证命令与套件入口见[验证清单](validation.md)，协议实验的实机范围仍以上述证据为准。
