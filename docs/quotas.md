# Agent 用量与额度

设置 → 机器 → AI 客户端下方的 Agent 用量打开弹窗，关闭后保留设置页。
独立入口 `quotas.html` 继续可用。只显示 `/api/clients` 确认已安装的客户端；
未安装的机器行和空分组隐藏，缺失数据统一显示 `N/A`。
Codex、Kiro、Agy、Claude Code、Droid 按此顺序各一个表格，复用客户端矩阵的表格样式；
Agy、Droid 保留额度组，Claude 不显示额度组，Codex 的特殊模型额度在窗口列注明。
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
SDK 与普通对话按模型选择内联额度组，其他组收进 ⋯；完整额度表仍显示全部组，
不受模型筛选影响。具体分组与未知模型行为见 [SDK 界面合同](sdk-ui.md#用量与菜单的显示条件)。

## 周期 API 等价与套餐等值

DELTA：费用统计集中在设置弹窗和独立页上半部；底栏与 ⋯ 不再展示累计 Token、
累计会话 Credits／成本及对应悬停明细。上下文、官方窗口、余额和原生额度错误保留。
各节点、来源、额度组和窗口分别展示“本周期 API 等价”“已耗费”“预计套餐等值”。
公式为 `本周期 API 等价消耗 / (官方已耗费百分比 / 100)`，例如 $120 / 30% = $400。
这不是除以套餐价格，也不是按时间预测月末消费；5 小时估值不能称为整月价值。
当前模型组合、窗口和缓存命中率改变，估值也会改变。

金额只覆盖本机原生记录，不把多机快照相加；跨设备、网页或 IDE 的遗漏消费可能
使样本估值偏低。没有请求明细、未定价、计量不完整、未知额度组、零消耗、
过期／失败的官方快照或已结束的周期不显示套餐估值，保留已知费用并解释原因。
相同账号在多节点的记录仍是分项，不承诺未经身份验证的全账号合计。
Token 计价采用[随版本发布的当前标准价格](../tools/agent_usage_prices.json)，不是实际账单；
不含工具、图片等单独收费项目。原始计量和未定价模型可以展开查看，不以零补齐。
价格可由管理员在私有统计目录的 `prices.json` 配置；默认价目只在首次运行创建，
之后不会覆盖本地修订。页面显示价目版本和官方来源，价格变更不需要重读正文。

固定窗口以原生重置时间减去窗口时长求起点；使用官方采集时间作为费用截止水位，
不会拿更晚的消费除以更早的百分比。月度缺少起点时由“统计设置”填写带时区的
本周期起点，绑定当次重置时间；下一账期不沿用旧起点。无官方窗口的来源显示
本月 UTC 已知费用，不推算百分比。金额、已耗费和估值来自同一统计快照，
不将后续额度广播拼接到旧金额上。

## 增量脚本与周期更新

服务将内嵌的[统计脚本](../tools/agent_usage.py)发布到
`<SESSIONDOCK_STATE_DIR>/agent-usage/collector.py`，Python 3.9+、无需第三方包。
解释器默认 POSIX `python3`、Windows `python`；可用 `SESSIONDOCK_USAGE_PYTHON`
指定可执行文件。服务内调度器在启动后延迟 30 秒开始，默认每 300 秒刷新官方额度
并增量采集，同一节点不重叠执行；不需要额外安装 systemd／launchd／计划任务。
“统计设置”可关闭或调整间隔；服务停止时停止自己的采集进程，不影响原生 Agent。

- GET `/api/usage` 只读小快照，返回采集状态与设置，不扫描日志。
- POST `/api/usage/refresh` 启动后台采集并立即返回；同时发生的请求合并，关闭页面不取消。
- POST `/api/usage/settings` 保存启用状态、间隔和月度起点，重算当前报告；
  浏览器不能通过此接口改变原生读根、Python 命令或输出目录。
- Hub 沿用节点代理逐台读取；一个节点失败不阻塞其他节点。页面可见时每分钟读取
  小快照，采集进行中每秒读取进度；隐藏或关闭停止读取。刷新额度和更新统计是独立按钮。

独立执行同一实现（`CONFIG` 取私有目录的 `config.json`，也可在临时目录自行配置）：

```sh
python3 tools/agent_usage.py collect --config CONFIG --agent all
python3 tools/agent_usage.py collect --config CONFIG --agent codex --format table
python3 tools/agent_usage.py report --config CONFIG
python3 tools/agent_usage.py reprice --config CONFIG
python3 tools/agent_usage.py rebuild --config CONFIG --agent claude
```

配置含 `directory`、`quota_dir`、`prices` 和 `roots`（source → 原生根；OpenCode 为数据库路径）。
运行目录持有 SQLite 计量账本、文件偏移和 JSON 快照；只存计量、模型及原生身份，
不存会话正文或凭据，不改写原生文件。JSONL 续读完整行，半行下次重读；
文件缩短／替换或大小不变而修改时间变化时重新读取，以事件身份去重；
同一文件增长默认按追加续读，若同时修改过已读前缀，需执行 `rebuild` 重读。
删除或回退对话不扣减已发生消费。
无变化文件不读正文；OpenCode 同时检查数据库与 WAL 的变化，不只检查主库 mtime。
首次需要建立全量计量索引，后续输出 `metrics.bytes_read/changed_files` 供核对开销。
任务失败保留旧账本和成功时间；外部脚本与服务共享操作系统锁。

| Agent | 已接入的原生来源 | 缺失与边界 |
| --- | --- | --- |
| Codex | `token_usage_record.payload.usage`，按 response_id 去重，turn_context 提供模型；旧 token_count 用累计差分 | 缓存输入从总输入扣除；思考不再加到输出；旧计数缺期初基线、缓存写入 TTL 不明、多额度组归属不明时不估套餐 |
| Claude Code | assistant message.usage 与原生消息 ID，包含子代理 | 同一消息修订不重复计费，输入／缓存分开，分别计 5m／1h 写入 |
| Agy | 完整 transcript 的 PLANNER_RESPONSE，按原生会话与 step_index 去重 | input_tokens 与 cache_read_tokens 分列；缺少实际模型的旧步骤未定价 |
| Grok | usage.json 的 turns[].modelUsage 与 endedAt | 不叠加 session 总量；没有官方额度百分比时仅显示费用；未知 CLI 模型别名不套近似价格；回合聚合超过长上下文价阶但无法拆回请求时不定价 |
| CodeBuddy | providerData.rawUsage，按 messageId 与 stepSeq 去重 | message／function_call 中重复 usage 只计一次；缺价或缓存写入口径不明不估套餐 |
| OpenCode | 只读 session_message 的已完成 assistant 记录 | 不叠加 session 累计；cache.write 缺 TTL 时不估完整费用 |
| Kiro | 官方额度中的月度 Credits | 没有可定价请求明细，显示原因，不将 Credits 当成 API 美元 |
| Droid | 原生 settings 的会话计量基线及现有官方窗口 | 累计量缺逐请求时间／模型归属，保留为不可归期，不将旧累计倒灌到本周期 |

[周期用量 Chromium 回归](../tests/agent_usage_browser.py)实际打开节点与 Hub 页面、
点击更新、明细和设置，覆盖八来源、去重、尾行、无变化快路径、零消耗、缺价、过期
与真实周期调度；宽窄屏及明暗主题分别检查。原生统计来源依据为只读样本字段核对，
未启动付费模型；测试使用合成私有数据。

## 采集与快照

- GET `/api/quotas` 只读 `<SESSIONDOCK_STATE_DIR>/quotas/{source}.json`。
- POST `/api/quotas/refresh` 并行查询本节点配置的 Codex、Agy、Kiro、Droid profile；Claude 只重读快照。
  同节点按来源合并同时发生的查询，不同来源互不阻塞；不打开会话、不发送推理请求。
  周期用量任务按其配置间隔复用此采集器，额度页面本身不额外定时查询。
  可用 `source` 参数只查询当前来源。自动查询使用 `?cached=true`，复用一分钟内的采集或失败尝试；显式刷新跳过有效期。
- Hub 页面通过已有 `/api/nodes/{nid}/api/...` 代理逐节点读取；一台失败不阻塞其它结果。
- 复用 launcher profile 的 executable、固定参数与环境，清除会话身份环境变量；工作目录为临时目录。
  CLI 查询最长 25 秒，结束或取消时回收子进程。stderr 与原始认证错误不返回网页。
  Windows 查询复用后台 CLI 的无窗口启动方式，打开额度页及刷新不弹出控制台窗口。
- 快照只存归一化额度字段，原子写入，文件权限 0600。不持久化或向网页返回账号凭据、邮件、会话正文。
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

DELTA：任一入口取得新额度快照后，按节点和来源同步当前 SDK、普通对话底栏及额度表；
同站点已打开的标签页通过浏览器通知同步，不增加轮询或推理请求。旧采集时间的响应
不能覆盖新值，切换来源或节点不串用额度，更新不清除输入草稿。
验收见[额度同步回归](../tests/quota_sync_browser.py)与外部会话额度回归。

DELTA：Codex 回合额度通知没有可靠采集版本，可能携带旧请求或旧套餐的计量口径；
即使重置窗口相同，通知中的上升和下降都不能直接当作账号快照。通知仅触发完整账号读取，
确认前保留现值，不广播通知中的百分比。并发 SDK 读取共享进行中的账号查询（包括失败），
失败保留现值并退避重试；真实下降与重置以完整查询为准，不取历史最大值。
SDK 查询保留服务端查询开始时间，持久化时丢弃被较新采集超越的响应，浏览器不以响应
到达时间重新标新；接口携带当前有效快照，使尚未收到跨页广播的页面也能丢弃旧原始响应。
已有账号快照时，原生 JSONL 额度仅作为缺失时的兜底；完整账号读取不再保留已消失的额度组。
账号快照广播使用新版通道，避免尚未重载的旧页面继续注入未经核实的通知数值。
见[通知顺序回归](../tests/codex_quota_order_browser.py)。

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

## Droid 接入

DELTA：Droid 1.6.0 原生账户使用 GET `https://api.factory.ai/api/billing/limits`，
公开 SDK 暂无账单方法。后端只读 profile 的 `FACTORY_API_KEY`，或
`FACTORY_HOME_OVERRIDE`（否则 HOME）下 `.factory/auth.v2.key`、`auth.v2.file`，
按原生 AES-256-GCM 格式解密 access token；不刷新、改写凭据或将其放入快照。
profile 可用 `FACTORY_API_BASE` 指定接口基址；HTTP 禁止重定向，失败只返回通用提示。
映射 Standard/Core 的 fiveHour、weekly、monthly；剩余为 100 − usedPercent，
windowEnd 为重置时间，null 保持未知。SDK、普通对话和额度页共用一分钟缓存，
查询失败保留上次结果；不会启动 Droid 或发送推理请求。额外付费余额暂不展示。
验收见 [Droid 用量浏览器回归](../tests/droid_usage_browser.py)。
