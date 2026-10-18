# Agent 额度

设置 → 机器 → AI 客户端下方的 Agent 额度打开弹窗，关闭后保留设置页。
独立入口 `quotas.html` 继续可用。只显示 `/api/clients` 确认已安装的客户端；
未安装的机器行和空分组隐藏，缺失数据统一显示 `N/A`。
Codex、Claude Code、Agy 各一个表格，复用客户端矩阵的表格样式；
Agy 保留额度组，Claude 不显示额度组，Codex 的特殊模型额度在窗口列注明。
Codex 固定显示 Credits 和重置卡列，即使旧快照尚无这些字段也显示 N/A。
各表独立列宽；同一机器的多个窗口合并机器与采集时间单元格，窄屏内部横向滚动。
重置和卡片到期显示倒计时，每秒只更新本地时钟，不请求接口；绝对时间放在悬停提示中。
同一账号可能跨机器共享额度，页面不相加，也不根据相同百分比猜测账号身份。
Agy 中的 Claude/GPT 组属于 Agy，与独立 Claude Code、Codex 分开。

## 采集与快照

- GET `/api/quotas` 只读 `<SESSIONDOCK_STATE_DIR>/quotas/{source}.json`。
- POST `/api/quotas/refresh` 并行查询本节点配置的 Codex、Agy profile；Claude 只重读快照。
  同节点刷新串行，不打开会话、不发送推理请求，不做定时刷新。
- Hub 页面通过已有 `/api/nodes/{nid}/api/...` 代理逐节点读取；一台失败不阻塞其它结果。
- 复用 launcher profile 的 executable、固定参数与环境，清除会话身份环境变量；工作目录为临时目录。
  CLI 查询最长 25 秒，结束或取消时回收子进程。stderr 与原始认证错误不返回网页。
- 快照只存归一化额度字段，原子写入，文件权限 0600。不采集账号凭据、邮件、会话正文。
  没有快照或缺字段显示 N/A。
- 查询失败保留旧快照并标记失败；重置时间经过不推断额度变成 100%。
  时间按浏览器本地时区显示。打开弹窗、独立页及页面重载先读取快照，再自动查询一次；
  每个节点独立推进，慢节点不延迟其它节点采集。点击刷新也主动查询，页面停留时不轮询。
  DELTA（BUG-20261018-030544-8d1fff）：原先打开页面只读快照，可能持续显示前一天的额度；
  现在打开即采集。查询期间及失败后保留原快照及真实采集时间，不推算余额。

Codex：启动 `app-server --stdio`，先 `initialize` / `initialized`，再
`account/rateLimits/read`。优先 `rateLimitsByLimitId`，兼容单桶 `rateLimits`；
解析 primary/secondary 的 usedPercent、windowDurationMins、resetsAt。

Agy：`-p /usage --output-format json`，要求 SUCCESS、num_turns=0、command.name=usage；
只读 `command.data.groups[].buckets[]` 的 id、window、remaining_fraction、reset_time。
不解析 response 文本。配置中禁用 slash commands 时不能调用，以免命令成为推理提示。

Claude：原生 statusLine JSON 中的 `rate_limits.five_hour`、`seven_day`、`spend_limit`，
提取 used_percentage 与 resets_at。没有额度字段的回调保留已有快照。

## Claude 接入

显式安装（路径由部署环境提供，不在仓库保存个人配置）：

```sh
sessiondock quota-install STATE_DIRECTORY CLAUDE_SETTINGS_FILE
```

安装会备份已有 settings 文件，将原 statusLine 配置保存在 state dir 的
`quota-statusline.json`，仅替换 statusLine.command，保留其它设置和 padding。
重复安装相同 wrapper 不修改文件；遇到另一套额度 wrapper 会报错，避免循环调用。
CLI 调用 `sessiondock quota-statusline STATE_DIRECTORY` 时先保存额度，再将相同 stdin
传给原 command，原 stdout 原样输出；没有原 command 时无输出。采集不读取凭据文件。
原命令最多等待 10 秒。Windows 的安装命令需另行适配 shell quoting；当前安装支持 POSIX。
恢复时还原 settings 备份即可，不影响会话数据。现有 Claude 会话是否即时重载 settings
由 CLI 控制；尚未收到回调时页面显示 N/A。

## 验收

`python3 tests/quotas_browser.py`：真实隔离 Node/Hub 与合成 CLI，点击设置入口、刷新、返回；
验证 Codex 握手/多桶、Agy 零回合 JSON、Claude wrapper 原输出及字段过滤，
缺字段/零额度/已过重置时间、失败保留快照、宽窄屏和明暗主题。

Codex 同时展示结构化 credits 余额与重置卡：数量取 availableCount，
到期时间取已返回的可用卡片中最近的一次，不用明细条数推算总数，不提供兑换操作。
Claude statusline 尚无 Usage credits 余额，不生成虚构的额度组占位行。
接口依据：[Codex App Server](https://learn.chatgpt.com/docs/app-server)。
