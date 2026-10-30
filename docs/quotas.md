# Agent 额度

设置 → 机器 → AI 客户端下方的 Agent 额度打开弹窗，关闭后保留设置页。
独立入口 `quotas.html` 继续可用。只显示 `/api/clients` 确认已安装的客户端；
未安装的机器行和空分组隐藏，缺失数据统一显示 `N/A`。
Codex、Kiro、Agy、Claude Code 按此顺序各一个表格，复用客户端矩阵的表格样式；
Agy 保留额度组，Claude 不显示额度组，Codex 的特殊模型额度在窗口列注明。
窗口与 SDK、普通对话状态栏统一显示“5小时 / 7天 / 月度”，不直接显示 `weekly`、`monthly` 等原生字段。
Codex 固定显示 Credits 和重置卡列，即使旧快照尚无这些字段也显示 N/A。
百分比与进度条统一显示已使用（100% − 剩余百分比），缺字段不推算。
复用 composer 的 sdk-gauge/sdk-bar：40×6 圆角条与百分比同行，达到 80% 时按 composer 规则标红。
Credits 余额右对齐，以逗号分隔千位，直接去掉小数部分；N/A 和不限量保留原文。
各表独立列宽；同一机器的多个窗口合并机器与采集时间单元格，窄屏内部横向滚动。
重置和卡片到期显示 `3d 05:33` 格式倒计时（不足一天为 `05:33`，小时/分钟补零，
不足一分钟为 `00:00`，到期为“已到期”）；每秒只更新本地时钟，不请求接口，绝对时间放在悬停提示中。
DELTA：SDK 与普通对话底栏的额度提示、原生限额和账期重置提示也共用上述剩余时长格式，
统一标为“重置剩余”，并每秒更新提示及无障碍说明；采集/记录时间仍显示日期时间。
时钟更新不重绘菜单或输入框，不推算剩余额度，不触发额度查询。
同一账号可能跨机器共享额度，页面不相加，也不根据相同百分比猜测账号身份。
Agy 中的 Claude/GPT 组属于 Agy，与独立 Claude Code、Codex 分开。

## 采集与快照

- GET `/api/quotas` 只读 `<SESSIONDOCK_STATE_DIR>/quotas/{source}.json`。
- POST `/api/quotas/refresh` 并行查询本节点配置的 Codex、Agy、Kiro profile；Claude 只重读快照。
  同节点按来源合并同时发生的查询，不同来源互不阻塞；不打开会话、不发送推理请求，不做定时刷新。
  可用 `source` 参数只查询当前来源。自动查询使用 `?cached=true`，复用一分钟内的采集或失败尝试；显式刷新跳过有效期。
- Hub 页面通过已有 `/api/nodes/{nid}/api/...` 代理逐节点读取；一台失败不阻塞其它结果。
- 复用 launcher profile 的 executable、固定参数与环境，清除会话身份环境变量；工作目录为临时目录。
  CLI 查询最长 25 秒，结束或取消时回收子进程。stderr 与原始认证错误不返回网页。
  Windows 查询复用后台 CLI 的无窗口启动方式，打开额度页及刷新不弹出控制台窗口。
- 快照只存归一化额度字段，原子写入，文件权限 0600。不采集账号凭据、邮件、会话正文。
  没有快照或缺字段显示 N/A。
- 查询失败保留旧快照并标记失败；重置时间经过不推断额度变成 100%。
  时间按浏览器本地时区显示。打开弹窗、独立页及页面重载先读取快照，再按有效期自动查询一次；
  每个节点独立推进，慢节点不延迟其它节点采集。点击刷新也主动查询，页面停留时不轮询。
  DELTA：打开页面从只读旧快照改为先显示快照、再检查有效期；Agy、Kiro SDK 与设置页
  共用持久快照、有效期及进行中的查询。普通对话与外部只读 Codex 入口也先读快照、
  再按当前来源检查有效期；Codex SDK 的完整账号读取和兑换后读取写回同一快照。失败尝试同样缓存一分钟，手动刷新及网页
  `/usage` 可立即重试。SDK 首屏与菜单条件见[SDK 界面合同](sdk-ui.md)。

Codex：启动 `app-server --stdio`，先 `initialize` / `initialized`，再
`account/rateLimits/read`。优先 `rateLimitsByLimitId`，兼容单桶 `rateLimits`；
解析 primary/secondary 的 usedPercent、windowDurationMins、resetsAt。

Agy：`-p /usage --output-format json`，要求 SUCCESS、num_turns=0、command.name=usage；
只读 `command.data.groups[].buckets[]` 的 id、window、remaining_fraction、reset_time。
不解析 response 文本。配置中禁用 slash commands 时不能调用，以免命令成为推理提示。

Claude：原生 statusLine JSON 中的 `rate_limits.five_hour`、`seven_day`、`spend_limit`，
提取 used_percentage 与 resets_at。没有额度字段的回调保留已有快照。

Kiro：与 SDK 相同的命令、HOME 作用域及 `acp --agent-engine=v3 --auth-method=cli`，
只发 `initialize` 与 `_kiro/account/getUsage`，不建会话、不推理。DELTA：该方法未列入
`extensionMethods`，`kiro-cli 2.24.1` 实测可用；不支持时按查询失败保留旧快照。
要求 `success=true`；每个 `usageBreakdowns[]` 一个 `monthly` 窗口，额度组为 `planName`，
剩余为 `100 − used×100/limit`（原生 `percentage` 取整，不用于显示），附 used/limit/单位。
`hasLimit=false` 显示“不限量”。`billingCycleReset` 只有日期，按该日 UTC 00:00 倒计时。
bonus、add-on 与超额 credits 暂不展示。SDK 打开时按一分钟有效期自动查询，
网页 `/usage` 和刷新按钮立即查询；回合完成只重读快照。

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

[外部会话额度回归](../tests/external_quota_browser.py)：节点与 Hub 的外部 Codex、普通对话切换，
旧快照自动刷新、同机缓存复用与 SDK 账号查询写回。

`python3 tests/quotas_browser.py`：真实隔离 Node/Hub 与合成 CLI，点击设置入口、刷新、返回；
验证 Codex 握手/多桶、Agy 零回合 JSON、Claude wrapper 原输出及字段过滤，
缺字段/零额度/已过重置时间、失败保留快照、宽窄屏和明暗主题。
`python3 tests/kiro_sdk_browser.py`：Kiro SDK 底栏月度 credits、`/usage` 刷新、额度页 Kiro 表。

Codex 同时展示结构化 credits 余额与重置卡：数量取 availableCount，
到期时间取已返回的可用卡片中最近的一次，不用明细条数推算总数，不提供兑换操作。
Claude statusline 尚无 Usage credits 余额，不生成虚构的额度组占位行。
接口依据：[Codex App Server](https://learn.chatgpt.com/docs/app-server)。
