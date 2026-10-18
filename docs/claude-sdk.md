# Claude 原生 SDK 桥接

`claude_sdk::Bridge` 使用 Claude Code 的持久双向 `stream-json` 子进程，
不发送 PTY 按键，不模拟 Codex app-server，不写原生 JSONL 或只读镜像。
普通历史仍由现有 Claude read-model 提供；桥接快照只包含本进程接受或观测的实时消息。

## 启用与接入

- `new(&Config, CancellationToken) -> io::Result<Option<Arc<Bridge>>>`
  只读取配置，不执行 CLI。必须同时配置 Claude 原生根和唯一的 Claude launcher
  **profile**；只有 root、只有 fixed adapter 或没有 Claude profile 时返回 `None`。
- 使用 profile 的 `executable`、固定 `args`、`env` 和 `env_remove`；
  不使用 TUI 专用 `new_args/resume_args`。清除 launcher 已定义的会话血缘变量。
  CLI 启动失败在具体请求中返回错误，不假装 SDK 已连接。
- 原生 root 必须是 `CLAUDE_CONFIG_DIR/projects`，CLI 启动时将
  `CLAUDE_CONFIG_DIR` 使用 profile 中与 root 对应的目录，未配置时从
  `projects` 父目录推导；支持 root 的原生目录软链接。这是关联原生身份的必要条件；
  不支持把任意镜像根当作 Claude 可写原生根。配置不会被改写。
- facade 调用 `call(method, target, input)`，其中已有会话的可信 target 为
  `{uid,session_id,cwd,agent}`；create 为 `{source:"claude",cwd}`。
  `open/create` 的返回值含 `session_id,cwd,capabilities`；create 还含
  `uid,session`，交给共享服务保存 pending 列表与草稿身份。
- facade 负责 inventory 授权、pending 回执解析、HTTP/Hub 路由、SSE 心跳及
  broadcast lag 后重新订阅。此模块不改 `lib.rs`、AppState 或共享 HTTP/UI 文件。

公共方法：

| method | 原生操作及返回 |
| --- | --- |
| `create` | 生成 CLI 原生 UUID 和路径派生 UID；不启动 CLI、不创建目录或 JSONL。相同 `request_id` 与相同输入返回同一回执 |
| `open` | 启动或复用本桥接拥有的持久 CLI，等待 `initialize`；已有文件按**完整原生路径** `--resume=…` 恢复，避免跨项目同 SID 搜索歧义 |
| `turn` | 把 `{type:"user",uuid,session_id,message:{role:"user",content:text}}` 写入原生 stdin；返回 `{accepted,request_id,session_id,message_id}` |
| `interrupt` | 原生 `{subtype:"interrupt"}` 控制请求；回执说明原生控制已确认，实际结束由原生 result/state 事件更新；保留原生 `still_queued` 等结果于 `raw` |
| `respond` | 按原生 request ID 回送一条 `control_response`；已取消或已回答的卡片返回 request-gone 错误 |
| `models` | 使用 `initialize.response.models`；保留原生字段并添加 `id/name/efforts` 对应 `value/displayName/supportedEffortLevels`。无 uid 时用仅初始化的临时原生进程查询，之后清理该进程 |
| `discard` | 仅空闲时释放桥接运行时与 pending create 缓存；保留所有原生历史 |

`turn` 的模型变化通过原生 `set_model`，effort 通过原生
`apply_flag_settings {settings:{effortLevel}}`。首次启动的选择用
`--model/--effort`。不改日常默认值、不配置 fallback、不自动批准。
多个接受的输入交给 Claude 自己的 streaming input 队列；桥接不添加输入长度、
行长、总量或排队数量拒绝规则。相同 `request_id` 在本桥接进程生命周期内
只发送一次；断线和不确定的写入结果不自动重放。

## 快照与回答

主会话订阅 key 是完整 `target.uid`。`uid+":"+agent` 保留为共享 facade 的
子代理命名约定，但 Claude 不支持独立恢复子代理 transcript，明确返回
`claude_sdk_unsupported`，`subagents:false`，不会误发给父会话。

`subscribe(key)` 在同一把状态锁里建立快照和 broadcast receiver。每次变更广播
完整快照，序号单调递增：

```json
{"seq":1,"type":"snapshot","raw":{"session_id":"native-uuid","running":true,"items":[],"requests":[],"model":null,"effort":null,"error":null,"capabilities":{}}}
```

消息包含 `id,role,content,text`；`text` 兼容现有 `renderMessage`。
文本和 thinking 的稳定 ID 使用原生 API message ID 与 block index，
工具用原生 tool-use ID；完整 assistant 帧到达后补充 `native_id`（JSONL UUID）。
发送的 user UUID 直接作为实时 ID，便于普通历史对账。
工具参数保留于 `input/native_block`，参数碎片在流中累计，工具结果和
`is_error` 保留于同一工具消息的 `result`；不会把失败工具当作成功回答。

`can_use_tool` 映射为审批卡，`AskUserQuestion` 映射为问题卡，保留完整原生
`input/questions`、tool-use ID 和请求内容。原生取消请求会撤掉对应卡片。

- 审批：`respond {key,result:{behavior:"allow"}}` 或
  `{behavior:"deny",message?:"…"}`。allow 仅批准该次原始工具输入；
  不持久化 permission suggestions，不创建统一允许规则。
- 问题：`respond {key,result:{answers:{"原生问题全文":"回答"}}}`，
  写入原生 `updatedInput.answers`。多选按 Claude 的字符串答案约定由 UI 组合；
  不把自由文本或缺失回答冒充 allow。问题也可以明确 deny。
- 其他原生控制请求明确回送 unsupported 错误；未实现新的
  `request_user_dialog`、MCP elicitation、OAuth 回调或 MCP SDK server。
  本实现不向 initialize 宣告这些能力。

## 生命周期与限制

启动 argv 在固定 profile 参数后追加 `--print --input-format stream-json
--output-format stream-json --verbose --include-partial-messages
--replay-user-messages --permission-prompt-tool=stdio` 和精确身份。
stdin 在每个 result 后保持打开。HTTP future 和 SSE receiver 都不拥有子进程，
因此离页、切换会话、刷新和单个 SSE 断开不会结束已接受工作。
按官方 SDK 设置进程内 `CLAUDE_CODE_SDK_READS_SESSION_STATE=1`，
使用原生 session-state 通知区分 result 与仍在执行的后台后续工作。
桥接 shutdown、明确 discard 或传输故障只清理自己启动的隔离进程组。
stderr 的诊断尾部经过配置/环境中的 secret 值及 token 形态脱敏，
不返回 argv、环境或凭据；原生退出错误会附带已脱敏的末条诊断。

能力包括 send、interrupt、respond、approval、question、models、create、resume、
effort；`shared_tui:false`、`subagents:false`。这是独立原生 runtime，
无法加入一个独立 TUI 正在使用的 runtime。Linux 在首次连接和每次发送前
复用既有 procscan，检查 SID、完整原生文件持有者及同 cwd 尚未明确身份的裸
Claude 进程，排除本桥接的进程树。发现冲突返回
`claude_sdk_session_busy`，不接管、不发信号。其他平台因缺少这项观察能力，
现有原生文件的首次恢复明确拒绝；新建桥接会话仍可使用。

进程观察是操作前采样，**不是 Claude 原生跨进程排他锁**，不能阻止另一个
外部 TUI 在采样后启动。共享终端接入不得把本桥接进程当作可强杀的旧 TUI；
需要切换到独立 TUI 时先显式结束 SDK 工作并 discard。
节点重启会丢失内存中的 pending 回执、请求去重缓存与实时快照；已存在的原生
历史仍可按同一 SID 恢复。不承诺从桥接崩溃中重放未确认输入。

新建路径使用 Claude Code 2.1.302 包内 `sanitizePath` 的确切算法：
按 JS UTF-16 把非 ASCII 字母数字替为 `-`，超过 200 个编码单元时取前
200 个并追加 `Math.abs(simpleHash(original)).toString(36)`。
这是原生命名算法，没有新增 cwd 长度限制。最终通过
`crate::sessions::uid_for("claude", native_path)` 生成 UID。
原生会话路径从未被桥接自造文件、重序列化或改写。

## 接口证据与验证状态

2026-10-18：本机 PATH 及已配置的 shell 包装器都找不到 `claude`，
因此没有声称验证本机已安装版本。为了核实接口，只在临时目录下载官方
npm Claude Code **2.1.302** Linux 包并执行 `--version/--help`，
另检查官方 Agent SDK **0.3.302** 包及官方 Python SDK 源码。

在临时 HOME、显式 Haiku low 和只返回合成错误的 loopback 网关下，
发送 initialize、effort 更新及 interrupt 控制请求：三者都收到原生 success，
initialize 确实返回 models，首次 user 输入前没有 JSONL。
另在只返回合成 Anthropic SSE 消息的 loopback 网关下发送一条合成 prompt：
实际请求模型确认为 `claude-haiku-4-5-20251001`，收到 user/partial/assistant/result，
SID 全部一致；包含非 BMP 字符且超过 200 个 UTF-16 单元的 cwd 所生成的原生
JSONL 路径与桥接算法完全一致。没有调用真实账号或真实模型，也没有读取或修改日常配置。

依据：

- [Claude CLI reference](https://code.claude.com/docs/en/cli-reference)：机器输入输出、恢复、指定 SID、partial messages 和用户回显参数。
- [官方 streaming input 说明](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode)：持久多轮输入、中断和 SDK 控制。
- [官方 Python query 实现](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/query.py)：initialize、control_request/response、can_use_tool 和 interrupt 封包。
- [官方 subprocess transport](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/transport/subprocess_cli.py)：stream-json argv、resume/session-id、effort 和 partial messages。
- [官方 Claude Code 2.1.302 包元数据](https://registry.npmjs.org/@anthropic-ai/claude-code-linux-x64/2.1.302) 及 [Agent SDK 0.3.302 包元数据](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/0.3.302)：版本固定的 CLI 帮助、包内命名与控制协议实现。
- [官方 session path 实现](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/sessions.py)：projects 根与路径命名。最终长路径算法以本次检查的 CLI 2.1.302 为准。

合成夹具在 [claude_sdk_fixtures.py](../tests/claude_sdk_fixtures.py)，
浏览器验收在 [claude_sdk_browser.py](../tests/claude_sdk_browser.py)。
该套件通过 Chromium 操作发送、停止、流式内容、工具失败、审批和问题回传、
刷新恢复、去重、新建及独立 writer 冲突。共享 Hub 的真实审批/问题控件和草稿恢复
见 [native_sdk_hub_browser.py](../tests/native_sdk_hub_browser.py)。
验证命令见[验证清单](validation.md)；合成协议验收不扩大上述实机实验的证据范围。
