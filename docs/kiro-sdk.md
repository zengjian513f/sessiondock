# Kiro 原生 SDK 桥接

`kiro_sdk::Bridge` 使用 `kiro-cli acp --agent-engine=v3 --auth-method=cli` 的 ACP JSON-RPC 2.0 over stdio 协议，由 `sdkhost` 托管进程生命周期，不依赖 PTY 屏幕解析。普通历史仍由 [Kiro read model](kiro.md) 读取。

## 协议与引擎选择

Kiro CLI（`1.29.0`）提供两套 ACP 引擎：

- **V3 KAS 引擎（默认主用，`--agent-engine=v3 --auth-method=cli`）**：
  - 原生支持 `configOptions`（`model`、`effortLevel`、`mode`、`autopilot`）及 `session/set_config_option`。
  - 原生支持 `session/fork`、`session/list`、`session/delete`、`_kiro/userInput`（结构化提问卡）、`_kiro/session/compact`、`_kiro/session/context`、`_kiro/account/getUsage`。
  - 进程退出不遗留陈旧 `.lock` 文件。
- **V2 旧引擎（兼容回退）**：
  - 使用 `modes` / `models`、`session/set_mode` 及 `_kiro.dev/commands/execute`（`model`、`effort`、`compact`）。

## 桥接操作映射

| 原生操作 | 桥接行为 |
| --- | --- |
| `initialize` | `protocolVersion: 1`，声明 `_meta.kiro.userInput: true` 以启用结构化问题卡 |
| `session/new` | 创建新 V3 会话，定位原生 `session.json` 并计算与索引一致的 `kiro:<hash>` UID |
| `session/load` | 恢复已有会话；`session/load` 期间的重放流（`loading` 或 `_meta.isReplay`）去重，不与磁盘历史重复 |
| `session/prompt` | 异步发送 prompt content blocks，实时转发 `session/update`（`agent_thought_chunk`、`agent_message_chunk`、`tool_call`、`tool_call_update`） |
| `session/cancel` | JSON-RPC notification，中断当前轮次并保留 `last_turn.status = "interrupted"`，不杀死 ACP 进程 |
| `session/request_permission` | 映射为前端 `approval` 请求卡，用户点击后回传 `outcome: { outcome: "selected", optionId }` |
| `_kiro/userInput` | 映射为前端 `question` 请求卡，用户提交后回传所选答案 |
| `session/set_config_option` | 实时切换 `model`、`effortLevel`、`mode`、`autopilot`（V2 回退 `session/set_mode` 与 `_kiro.dev/commands/execute`） |
| `_kiro/session/compact` | 原生上下文压缩（V2 回退 `_kiro.dev/commands/execute` `compact`） |
| `_kiro/session/context` / `_kiro/account/getUsage` / `_kiro.dev/metadata` | 实时更新上下文占比、Token 用量与 Credits 消耗 |
