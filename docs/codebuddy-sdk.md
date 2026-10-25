# CodeBuddy 原生 SDK 桥接

`codebuddy_sdk::Bridge` 使用 CodeBuddy 的持久双向 `stream-json` 子进程，不发送 PTY 按键，
不写原生 JSONL。历史由 [CodeBuddy 读模型](codebuddy.md)提供；配置持久 state dir 时由
[独立 SDK 宿主](sdkhost.md)持有原生进程，桥接快照和待回答请求可跨节点服务重启恢复。
结构沿用 [Claude SDK](claude-sdk.md)，下面只记录与它不同的、已用真实 2.175.0 核实的协议事实。

## 启用与接入

- `new(&Config, CancellationToken)` 只读配置：需要 `roots.codebuddy` 和唯一的 `codebuddy`
  launcher profile，否则返回 `None`。使用 profile 的 `executable`、固定 `args`、`env`。
- 原生配置目录：profile 的 `CODEBUDDY_CONFIG_DIR`，否则取根（目录名必须是 `projects`）的父目录，
  两者必须与配置的根一致。桥接不导出该变量，也不改写配置。
- 启动 argv：`--input-format=stream-json --output-format=stream-json --verbose
  --include-partial-messages --replay-user-messages`，新会话 `--session-id <sid>`，
  已有文件 `--resume <sid>`（工作目录必须是会话原 cwd），可选 `--model`、`--effort`。
  不带 `--print`，权限请求默认以 `can_use_tool` 控制请求到达。

## 方法与能力

| method | 原生操作 |
| --- | --- |
| `create` | 生成 UUID 和路径派生 UID，不启动 CLI、不写文件；相同 `request_id` 返回同一回执 |
| `open` | 启动或复用持久 CLI，发送 `initialize`（带 `askUserQuestion` 能力） |
| `turn` | 写入 `{type:user,session_id,message:{role,content}}`；相同 `request_id` 只发一次 |
| `interrupt` | `{subtype:interrupt,session_id}`；实际结束由后续 `result` 更新 |
| `respond` | 回送 `control_response`；审批 `{allowed,updatedInput,tool_use_id}`，拒绝 `{allowed:false,reason,interrupt,tool_use_id}`（与 Claude 的 `behavior` 不同） |
| `models` | `initialize.response.models`（`id/name`），强度为固定六档 |
| `settings` | 权限模式 `set_permission_mode`、模型 `set_model`、强度 `set_config {reasoningEffort}`，均带 `session_id`；必填 `request_id`，相同请求不重放 |
| `discard` | 仅空闲时释放桥接运行时与未落盘回执，保留历史 |

`rename`、`compact`、`fork`、`usage` 均不提供：CLI 没有对应控制请求，stream-json 中的
`/rename` 会被当作普通提示词交给模型（已实测），因此桥接明确返回 `codebuddy_sdk_unsupported`，
不把命令文本发给模型。`fast_mode`、MCP elicitation、user dialog、子代理均不支持。
`shared_tui:false`：无法加入独立 TUI 正在使用的运行时；Linux 在首次连接和每次发送前复用
procscan 检查 SID 与完整原生文件持有者，冲突返回 `codebuddy_sdk_session_busy`，
不接管、不发信号。SDK 运行期间启动同一会话的 CLI 返回 `sdk_session_busy`。

## 协议事实（2.175.0 实测）

- `initialize` 响应含 `models`、`commands`、`currentModelId` 和 **`account`（含登录令牌）**。
  桥接只读取 `models`；账号字段不进入快照、检查点、日志或浏览器。
- `system/init` 在每次运行开始时发出（首条用户消息之后），`session_id` 与 `--session-id` 一致。
- 恢复时 CLI 在 `initialize` 之后把**整段历史**重放到 stdout（`system/init`（`model:"unknown"`）、
  历史 `user`/`assistant` 帧以及每轮一个 `result`）。历史属于读模型，桥接在 `replaying`
  期间丢弃这些帧（仍处理控制消息），直到已接受回合引出模型不是 `unknown` 的下一条
  `system/init`。`replaying` 写入检查点，重启后同样生效。
- `--replay-user-messages` 回显的 `user` 帧 `uuid` 由 CLI 生成，**不保留**客户端 `uuid`。
  桥接把回显按文本 FIFO 对应到最早未确认的已接受输入，写入 `native_id`（等于原生记录 `id`）
  并标 `native_echo:true`；此后输入账本的消费证据为 `native_echo && request_id == delivery_id`。
- 流式帧（`stream_event`）、`assistant`（每块一帧，共享 `message.id`）、`result`
  （`usage`、`modelUsage`、`total_cost_usd`）与 Claude 同形，投影共用同一实现。
- `set_model`、`set_config` 实测不修改 `~/.codebuddy/settings.json`；强度对后续请求的实际影响
  只验证了 CLI 接受（`updated` 回执），没有独立观察模型请求。
- 未观察到 `session_state_changed`、`control_cancel_request`、`get_usage` 等 Claude 专有帧。

## 验证状态

[codebuddy_sdk_browser.py](../tests/codebuddy_sdk_browser.py) 用复现上述协议的假 CLI
（[codebuddy_sdk_fixtures.py](../tests/codebuddy_sdk_fixtures.py)）覆盖：读模型列表和历史、
设置立即生效且不发模型提示、发送／去重／历史与实时对齐、关页后继续与停止、审批允许／拒绝／
问题回答、令牌不外泄、创建的目录命名、不支持的控制、同 SID 独立进程冲突、节点重启重附着。
真实 2.175.0 的证据来自隔离探测（`glm-5.3-flash`，临时工作目录）：审批和问题回答、中断、
恢复重放、目录命名；假 CLI 与真实 CLI 的结果分别记录，真实 CLI 套件尚未纳入仓库。
