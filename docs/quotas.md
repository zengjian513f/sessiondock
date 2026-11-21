# Agent 用量与额度

设置 → 机器 → AI 客户端下方的 Agent 用量打开弹窗，关闭后保留设置页。
独立入口 `quotas.html` 继续可用。只显示 `/api/clients` 确认已安装的客户端；
未安装的机器行和空分组隐藏，缺失数据统一显示 `N/A`。
DELTA：顶部以 Codex、Kiro、Agy、Claude Code、Droid 标签切换，每次只显示一个 Agent 的
原有额度表，复用设置页标签及客户端矩阵样式；只展示已安装的 Agent 标签。
标签位于滚动区外，支持左右方向键及 Home/End；切换保留各 Agent 的纵向和表内横向位置。
独立页面与设置弹窗使用相同布局。后台增量、跨页额度通知、延迟响应和手动刷新
按机器与窗口身份原位更新，保留当前标签、表格滚动节点及焦点，不清空整页再重建。
Agy、Droid 保留额度组，Claude 不显示额度组，Codex 的特殊模型额度在窗口列注明。
额度表窗口显示“5小时 / 7天 / 月度”；SDK、普通对话底栏及其他额度菜单缩写为 `5h / 7d / 1m`，提示保留完整含义，不直接显示 `weekly`、`monthly` 等原生字段。
Codex 固定显示 Credits 和重置卡列，即使旧快照尚无这些字段也显示 N/A。
百分比与进度条统一显示已使用（100% − 剩余百分比），缺字段不推算。
复用 composer 的 sdk-gauge/sdk-bar：40×6 圆角条与百分比同行，达到 80% 时按 composer 规则标红。
Credits 余额右对齐，以逗号分隔千位，直接去掉小数部分；N/A 和不限量保留原文。
各表独立列宽；同一机器的多个窗口合并机器与采集时间单元格，窄屏内部横向滚动。
DELTA：节点连接失败显示“节点暂不可达”，页首按机器说明；配置的 CLI 消失显示
“CLI 文件缺失”，超时与额度查询失败分开。保留上次快照和原始采集时间，错误详情在提示中。
重置和卡片到期显示 `3d 05:33` 格式倒计时（不足一天为 `05:33`，小时/分钟补零，
不足一分钟为 `00:00`，到期为“已到期”）；每秒只更新本地时钟，不请求接口，绝对时间放在悬停提示中。
DELTA：SDK 与普通对话底栏的额度提示、原生限额和账期重置提示也共用上述剩余时长格式，
统一标为“重置剩余”，并每秒更新提示及无障碍说明；采集/记录时间仍显示日期时间。
时钟更新不重绘菜单或输入框，不推算剩余额度，不触发额度查询。
同一账号可能跨机器共享额度，页面不相加，也不根据相同百分比猜测账号身份。
Agy 中的 Claude/GPT 组属于 Agy，与独立 Claude Code、Codex 分开。
SDK 与普通对话按模型选择内联额度组，其他组收进 ⋯；无法匹配时全部组内联显示。完整额度表仍显示全部组，
不受模型筛选影响。具体分组与未知模型行为见 [SDK 界面合同](sdk-ui.md#用量与菜单的显示条件)。

## 周期 API 等价与套餐等值

DELTA：在原有额度表的“已使用”之后增加一列“API 价值”，不另建卡片或表格。
同一格第一行是本周期已知 API 价值，第二行是“整轮 ≈ $…”。窗口仍由原表说明，
5 小时整轮不能称为整月。悬停及无障碍说明包含公式、采样时刻、范围、模型明细、
价目版本和缺失原因。底栏与 ⋯ 不显示累计 Token、会话 Credits／成本及其明细；
上下文、官方窗口、余额和原生额度错误保留。

公式为 `同一采样时刻的本周期 API 价值 / (该时刻已耗费百分比 / 100)`，
不是除以套餐价格，也不是按时间预测月末消费。Codex 采用原生 token_count 的
观测口径：首次显示整数百分比时，在四舍五入假设下用下边界估算，例如首次 5%
使用 4.5%，首次 15% 使用 14.5%；提示明确标注假设。金额也截止该次观测，
不拿后来的消费除以之前的百分比。自动压缩请求不属于此累计差分口径，单独排除。已有累计计数的会话中，只有逐请求
记录而未被 token_count 确认的中断调用也不计入；纯逐请求格式仍可单独统计。
首次到达 100% 时截止本轮金额，后续 Credits 消费不计入；延迟到达的较低读数
不重开已结束的历史窗口。表内官方额度仍使用现有账号快照，不被这些历史采样覆盖。
非整数原生百分比使用原值；缺跳档记录不猜整数边界。其他 Agent 使用同步采集的
官方百分比。当前调用组合、上下文和缓存命中率改变，估值也会改变。

DELTA：费用按迁移账本记录的本机执行区间归属，不能按文件现在位于哪台机器判断。
读取成功 move 的时间和方向，包含迁出保留在 trash 的原始记录及 Codex 归档，
排除迁入前、迁出后在其他机器执行的调用；克隆及失败的迁移不切换归属。
同一请求去重。迁出原始记录缺失时只保留已知金额并标为不完整，不能估算整轮。
跨设备、网页或 IDE 未被记录的消费可能使本机样本偏低；不相加多机快照、
不承诺未经核对的全账号合计。缺明细、价格、计量、额度组、有效百分比或周期边界时，
保留已知费用并解释原因，隐藏估值；过期／失败的官方快照也不估算。
部分统计显示 `≥` 和“记录不完整”；缓存写入有效期未知但两档价格都可核实时，
显示金额范围，整轮估值使用相同范围除以已用百分比。

采用[当前 Standard API 价格](../tools/agent_usage_prices.json)，非实际扣款，不含工具、
图片等另收费项目。支持型号的单次输入超过 272K 时，整次输入和缓存按两倍、
输出按 1.5 倍计价；提示另列不含长上下文加价的金额。管理员可以修改私有目录的
`prices.json`；服务更新 `default-prices.json`，合并新增型号及缺失价阶字段，保留本地
已配置的同名字段。自定义价阶应与自定义基础单价一起维护。重新计价无需重读正文。
内置价目随服务发布更新，不自动联网同步。GPT-6.2
[Sol](https://developers.openai.com/api/docs/models/gpt-6.2-sol) 和
[Astra](https://developers.openai.com/api/docs/models/gpt-6.2-astra) 使用官方 Standard
输入、输出、缓存读写及超过 272K 的完整价阶；旧私有价目没有的新型号从内置表补齐。

固定窗口以原生重置时间减去窗口时长求起点，费用不晚于官方采集时刻。
月度缺少起点时通过私有 `settings.json` 或设置 API 配置带时区的起点并绑定当次
重置时间；新账期不沿用旧起点。无官方窗口的来源仍由脚本输出本月 UTC 已知费用，
不因此在页面增加另一张表。当前金额和整轮估值的采样时刻可不同，提示分别列出。
Droid 优先使用原生 windowStart；缺失时按[官方 30 天窗口](https://docs.factory.com/pricing/individuals)
从 windowEnd 回推，不能套自然月。

## 增量脚本与周期更新

服务将内嵌的[统计脚本](../tools/agent_usage.py)发布到
`<SESSIONDOCK_STATE_DIR>/agent-usage/collector.py`，Python 3.9+、无需第三方包。
解释器默认 POSIX `python3`、Windows `python`；可用 `SESSIONDOCK_USAGE_PYTHON`
指定可执行文件。服务内调度器在启动后延迟 30 秒开始，默认每 300 秒刷新官方额度
并增量采集，同一节点不重叠执行；不需要额外安装 systemd／launchd／计划任务。
私有 `settings.json` 或设置 API 可关闭或调整间隔；服务停止时停止自己的采集进程，不影响原生 Agent。

- GET `/api/usage` 只读小快照，返回采集状态与设置，不扫描日志。
- POST `/api/usage/refresh` 启动后台采集并立即返回；同时发生的请求合并，关闭页面不取消。
- POST `/api/usage/settings` 保存启用状态、间隔和月度起点，重算当前报告；
  浏览器不能通过此接口改变原生读根、Python 命令或输出目录。
- Hub 沿用节点代理逐台读取；一个节点失败不阻塞其他节点。页面可见时每分钟读取
  小快照，采集进行中每秒读取进度；隐藏或关闭停止读取。原有“刷新”按钮同时更新额度和统计，不增加第二个按钮。

独立执行同一实现（`CONFIG` 取私有目录的 `config.json`，也可在临时目录自行配置）：

```sh
python3 tools/agent_usage.py collect --config CONFIG --agent all
python3 tools/agent_usage.py collect --config CONFIG --agent codex --format table
python3 tools/agent_usage.py report --config CONFIG
python3 tools/agent_usage.py reprice --config CONFIG
python3 tools/agent_usage.py rebuild --config CONFIG --agent claude
```

配置含 `directory`、`quota_dir`、`prices` 和 `roots`（source → 原生根；OpenCode 为数据库路径），
以及 `default_prices`、`transfer_dir`、`trash_dir`、`codex_archive`。新版建立 `ledger-v2.sqlite`，
保留旧索引；首次升级重新读取原始计量，以补齐归属与额度观测。
运行目录持有 SQLite 计量账本、文件偏移和 JSON 快照；只存计量、模型及原生身份，
不存会话正文或凭据，不改写原生文件。JSONL 续读完整行，半行下次重读；
文件缩短／替换或大小不变而修改时间变化时重新读取，以事件身份去重；
同一文件增长默认按追加续读，若同时修改过已读前缀，需执行 `rebuild` 重读。
删除或回退对话不扣减已发生消费。
无变化文件不读正文；OpenCode 同时检查数据库与 WAL 的变化，不只检查主库 mtime。
首次需要建立全量计量索引，后续输出 `metrics.bytes_read/changed_files` 供核对开销。
任务失败保留旧账本和成功时间；外部脚本与服务共享操作系统锁。
解析器版本按来源保存；Agy/Droid 升级只重读对应来源，不触发其他 Agent 的全量扫描。

| Agent | 已接入的原生来源 | 缺失与边界 |
| --- | --- | --- |
| Codex | `token_usage_record.payload.usage`，按 response_id 去重，turn_context 提供模型；旧 token_count 用累计差分 | 缓存输入从总输入扣除；思考不再加到输出；旧计数缺期初基线、缓存写入 TTL 不明、多额度组归属不明时不估套餐 |
| Claude Code | assistant message.usage 与原生消息 ID，包含子代理 | 同一消息修订不重复计费，输入／缓存分开，分别计 5m／1h 写入 |
| Agy | 完整 transcript 的 PLANNER_RESPONSE，按原生会话与 step_index 去重 | input_tokens 与 cache_read_tokens 分列；旧步骤按此前原生 USER_SETTINGS_CHANGE 恢复型号，不采信用户正文中的同名片段；Carbon/Argon/Hydrogen 思考档位映射相应 API 价格 |
| Grok | usage.json 的 turns[].modelUsage 与 endedAt | 不叠加 session 总量；没有官方额度百分比时仅显示费用；未知 CLI 模型别名不套近似价格；回合聚合超过长上下文价阶但无法拆回请求时不定价 |
| CodeBuddy | providerData.rawUsage，按 messageId 与 stepSeq 去重 | message／function_call 中重复 usage 只计一次；缺价或缓存写入口径不明不估套餐 |
| OpenCode | 只读 session_message 的已完成 assistant 记录 | 不叠加 session 累计；cache.write 缺 TTL 时不估完整费用 |
| Kiro | 官方额度中的月度 Credits | 没有可定价请求明细，显示原因，不将 Credits 当成 API 美元 |
| Droid | sessions 同级 logs/droid-log-single.log 及轮转日志的 Agent Streaming result、Model-Router Model transition | 按请求时间、tags.modelId 和 response/request ID 核算去重；不叠加 settings 总量、totalInputTokens 或 reasoningTokens；BYOK 排除；优先使用请求的额度组，闭源模型默认归 Standard；同一进程和会话在 402 后自动切换至 Kimi K3，后续匹配请求归 Core，直至下一次主模型切换；其余额度组缺失的记录仍标为不完整；缓存写入 TTL 未知显示 5m–1h 价格范围，日志未覆盖周期起点时仅列已知下限 |

[周期用量 Chromium 回归](../tests/agent_usage_browser.py)实际打开节点与 Hub 页面、
点击原表刷新、打开设置弹窗，覆盖八来源、历史检查点、迁移归属、自动压缩、
长上下文、耗尽截止、去重、尾行、无变化快路径、零消耗、缺价、过期与周期调度；宽窄屏及明暗主题分别检查。原生统计来源依据为只读样本字段核对，
未启动付费模型；测试使用合成私有数据。
[标签与滚动 Chromium 回归](../tests/quota_tabs_browser.py)覆盖节点/Hub、独立页/弹窗、
全部 Agent、宽窄屏、键盘、延迟返回、周期刷新及额度广播，验证阅读位置与焦点不被重置。

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
windowEnd 为重置时间，null 保持未知；保留 windowStart，monthly 缺起点时回推 30 天。SDK、普通对话和额度页共用一分钟缓存，
查询失败保留上次结果；不会启动 Droid 或发送推理请求。额外付费余额暂不展示。
验收见 [Droid 用量浏览器回归](../tests/droid_usage_browser.py)。
