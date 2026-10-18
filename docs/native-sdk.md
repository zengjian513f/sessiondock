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

输入共用服务端草稿。失败保留输入，成功只清本次提交，发送期间的新编辑保留。
文件和图片经现有上传/发布边界放到本会话 cwd。Claude、OpenCode 将可信发布的
图片转为原生多模态内容；Grok 按 initialize 的 image 能力协商，当前安装版 false
时与 Agy 一样仅发路径引用，网页明确提示。客户端自报 images/files/path 不授予读取权限。
已加载的原生 SDK 会话通过 `/api/live.sdk_uids` 标记为受管 SDK，沿用蓝色状态点。
模型目录来自原生接口，目录失败不妨碍沿用会话当前模型。现已接用量明细/底栏、
各来源可用设置、OpenCode inbox、Grok/OpenCode 会话动作和 Claude 压缩。
完整持久队列与输入消费对账尚未对齐 Codex。已实现范围、可补齐遗漏与
协议限制见[五种 SDK 功能审计](sdk-feature-audit.md)。

网页离开、SSE 断开或切换视图不取消已接受工作；重开取桥接快照，不重发输入。
提交 ID 在各桥接进程内去重，完整节点重启后的实时恢复边界见各来源合同。
新建回执保存在私有 `native-sdk-sessions.json`，同一创建请求可恢复其 UID/SID。
原生准入后沿用既有 SEND 回执及草稿修订规则，服务端持久清除本次提交字段；
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
| POST `/api/native-sdk/turn` | `{uid, agent?, request_id, text, attachments?, quotes?, model?, effort?}`；原生输入 |
| POST `/api/native-sdk/interrupt` | `{uid, agent?}`；原生中断，只针对桥接所属任务 |
| POST `/api/native-sdk/respond` | `{uid, agent?, key, result}`；回答该会话仍未解决的原生审批或问题 |
| POST `/api/native-sdk/models` | `{uid, agent?}`；原生模型目录 |
| POST `/api/native-sdk/usage` | 原生 Token/上下文；Agy/Claude 合并已有 quota；`refresh:true` 仅查询本来源 |
| POST `/api/native-sdk/settings` | 来源自己的模式/权限/effort/Fast；OpenCode `native_agent` 与身份字段 `agent` 分开 |
| POST `/api/native-sdk/fork` | Grok/OpenCode 原生分支，返回稳定身份；成功网页回执持久化、Hub 命名空间处理 |
| POST `/api/native-sdk/compact`、`shell` | 按 capability 走原生操作；mutation 带 request_id |
| POST `/api/native-sdk/queue_cancel`、`queue_update` | OpenCode 未投递 inbox 撤销与显式 steer |
| POST `/api/native-sdk/revert`、`unrevert`、`revert_commit` | OpenCode 对话-only stage/撤销/永久提交，固定 files:false |
| POST `/api/native-sdk/rewind_points`、`rewind`、`billing` | Grok 原生选择点、conversation_only 回退和额度 |
| POST `/api/native-sdk/agents`、`commands`、`command` | OpenCode 原生目录与命令；不把命令伪装成普通提示词 |
| POST `/api/native-sdk/discard` | `{uid}`；释放未落盘创建回执，保留原生历史；能力与拒绝条件由来源声明 |

普通操作响应为 `{ok, raw, ...原生桥接结果}`，创建结果直接位于顶层。
SSE 包为 `{seq, type:"snapshot", raw:{session_id,running,items,requests,...}}`。
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
