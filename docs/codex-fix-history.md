# Codex 历史修复传播与五种状态栏核验

2026-10-19，以已提交版本 `51e77ba9` 为检查基线。本文记录本次检查和补修，
不把共享工作区其他未提交改动计为已交付，也不恢复已被后续提交替代的设计。

## 历史范围与判定

检索所有 refs 的提交消息（包括正文）中的 Codex，并补查 Codex SDK 模块、HTTP、
前端控件及所有名称含 Codex 的路径历史。并集 **378 条**，其中 **174 条**属于
基线的祖先，**204 条**属于其他分支/旧整合历史；全部保留在本文索引。
这不是 378 个独立缺陷：包含功能、合并、重复整合和已退休实现的历史记录。
下面按当前代码的实际共享边界复核；旧分支补丁不能直接覆盖当前实现。

复现检索口径：

```sh
git log --all --regexp-ignore-case --grep=codex
git log --all -- ':(icase,glob)**/*codex*'
git log --all -- web/codex-sdk.js web/codex-sdk-actions.js web/codex-sdk-controls.js web/codex-sdk-compose.js crates/sessiondock/src/codex_sdk crates/sessiondock/src/api/codex_sdk.rs crates/sessiondock/src/api/codex_sdk_actions.rs
```

结论不是“都已同步”。两个共享行为遗漏为用量自动重试和队列操作期间保护；
还发现下面的界面差异。本次补齐这些可复用修复。原生协议未接完的项目仍按
[功能审计](sdk-feature-audit.md)和 [TODO](../TODO.md)记录，不能冒充已完成。

## 状态栏曾经遗漏、本次补齐的完整清单

“四来源”均指 Claude、Grok、OpenCode、Agy；只在来源实际提供对应能力时显示控件。

| ID | 基线中没有同步的地方 | 影响来源 | 本次结果 |
| --- | --- | --- | --- |
| V01 | 进入 ⋯ 的隐藏控件被 display:flex 重新显示，出现空模型/强度行 | 四来源 | 显式保留 hidden，空数据不出现假控件 |
| V02 | 当前模型既出现在空值默认项、又出现在目录中 | 四来源 | 当前值用原生 ID，目录去重 |
| V03 | 当前强度既出现在空值默认项、又出现在强度列表中 | 四来源 | 当前强度只列一次，中文名称共用 |
| V04 | 模型目录失败就隐藏已知模型 | 四来源 | 保留目录外当前模型和用户未发送选择 |
| V05 | 没有目录时隐藏已知原生强度；切模型时沿用不支持的旧强度 | 四来源 | 保留有效当前值；切换模型时不冒用不支持的值 |
| V06 | 动作始终藏在 ⋯，也没有 Codex 同类图标 | 四来源 | 共用图标、单行布局和按剩余空间移入 ⋯ 的规则 |
| V07 | 模式/权限/Fast/代理选择器即使宽屏也始终藏在 ⋯ | 四来源，按能力 | 同类原生下拉直接显示，窄屏移入 ⋯ |
| V08 | 选择器进入菜单后没有名称，只剩下值；间距与原生菜单不同 | Codex | 共用名称/值两列、行高和菜单内边距 |
| V09 | 设置里的“跟随原生”是可提交的空值，Fast 空值还会变成 false | 四来源，按能力 | 无有效值显示不可选“未返回”；已知值保留，原生 ID 不改 |
| V10 | 原生任务运行中按钮仍叫“发送”，没有反映排队语义 | 四来源 | 同 Codex 显示“排队”，空闲显示“发送” |
| V11 | 重试详情只显示次数，丢失已提供的原因 | 四来源，原生有字段时 | 状态保持短名，完整原因进入已有详情浮层 |
| V12 | 服务档位空值暴露内部英文 Tier | Codex | 改为“档位”；原生服务名称和 ID 保留 |
| V13 | 额度悬停文字舍去小数，与其他来源精确明细不一致 | Codex | 详情使用共用百分比格式，底栏仍用简洁整数 |
| V14 | Grok 已返回 currentModeId，状态栏设置却没有读取 | Grok | 显示原生有效模式 |
| V15 | 发送后始终保留临时模型/强度选择，遮住后续原生有效值；发送中选择变动没有与本次请求隔离 | 四来源 | 与 Codex 一样固定本次参数，成功后释放本次选择，保留期间新选择 |
| V16 | 发送按钮没有与 Codex 一致的 aria-busy | 四来源 | 提交期间标记忙碌，结束后恢复 |
| V17 | 设置移回宽屏后只有“默认/关闭”等笼统值，或暴露 build 等可翻译原始名 | 四来源，按能力 | 显示“默认权限/Fast 关闭/自动模式/构建”等明确选项，原生 ID 保留 |
| V18 | Codex、原生 SDK、普通对话各自构造进度条，条长/无障碍值的取整不同 | 五种来源 | 统一复用 UsageUI.gauge；文字简洁取整，条长和 aria 保留同一精确值 |
| R01 | 读取用量失败后只能手动刷新，未移植 2e64237a 的自动恢复 | 四来源 | 1/2/4…30 秒退避，成功归零；关闭取消读取和定时器 |
| R02 | 分开刷新 billing 时，失败会丢失先前余额/周期快照 | 四来源，原生有 billing 时 | 保留最近成功快照，显示错误并重试 |
| R03 | 撤销/重试请求期间新快照重建并重新启用队列按钮 | 四来源 | 保留操作中的禁用按钮；返回后绘制最新快照 |

这些修改使用 [共用底栏算法](../web/codex-sdk.js)、[原生控件](../web/native-sdk-controls.js)、
[原生视图](../web/native-sdk.js)和[共享样式](../web/style.css)。模型/设置提交仍使用原生 ID，
不把各家的权限模式伪装成 Codex 沙箱；保留最近提交的单行底栏、原生权限 select、
有内容才显示的 ⋯、有额外信息才可开的状态详情、余额及重置卡。

## 已传播的历史修复与实际边界

下列是当前代码核对结果；“已有覆盖”指仓库已有浏览器用例，不表示本次重跑了全部历史用例。

| 历史修复主题/代表提交 | 当前其他 agent 的接法 | 代码/合同证据与边界 |
| --- | --- | --- |
| 单行、最多 6 字状态、用量优先、窄屏滚动；e85e6ca0、8d2f1159、92f00110 | 五种 SDK 共用底栏算法和状态详情判定；普通对话五来源共用渲染 | [共用前端](../web/codex-sdk.js)、[普通对话](../web/app.js)、[底栏浏览器](../tests/native_sdk_footer_browser.py)、[状态浏览器](../tests/sdk_status_disclosure_browser.py) |
| 用量常显、人类可读窗口与分组；e61b8873、51e77ba9 | 四来源复用 UsageUI；普通对话也共用；本次补失败自动恢复 | [用量格式](../web/usage-ui.js)、[额度页](quotas.md)、[恢复浏览器](../tests/native_sdk_recovery_browser.py) |
| 草稿 CAS、取消回执恢复、附件/引用、保留新编辑；8fc2d721、44f7615f、ea933b38 | SDK 共用 composer 和 accepted/sent；普通对话共用草稿修订逻辑 | [composer](../web/codex-sdk-compose.js)、[输入合同](conversation.md)、[原生输入 HTTP](../crates/sessiondock/src/api/native_sdk.rs) |
| Tab/Shift+Tab、输入时过滤补全、IME/过期响应隔离；2af115d2、c648497b | 五种 SDK 及普通对话使用同一个补全组件 | [补全组件](../web/path-completion.js)、[浏览器夹具](../tests/path_completion_fixtures.py) |
| 队列按显式消费证据、失败/未知不自动重放；6b30d787、44f7615f | 四来源有持久输入账本，准入/消费分开；本次补按钮操作保护 | [持久账本](../crates/sessiondock/src/native_sdk_queue.rs)、[队列回归](../tests/native_sdk_queue_browser.py)；Grok 接收回执不证明消费 |
| 临时断线重连、历史与 SDK 并行打开；069355a1、30171a94、4b232911 | 原生视图有 reset/CLOSED 重连和重连失败再试；直接复用 refreshHistory/useHistory | [原生视图](../web/native-sdk.js)、[原生合同](native-sdk.md) |
| 保留部分历史、复用 head/tail、中间页显式加载；cb1dd1e0、27acd39d | 复用历史入口、折叠与分页渲染 | [原生 paintHistory](../web/native-sdk.js)、[分页合同](history-pages.md)；完整 turn 身份对账差距见后表 |
| 全文助手正文、流更新保留展开选择、工具未结束不标完成；19e26b22、e2d618f6、e167c0f8 | 共用 msgNode，原生视图保留工具展开状态及运行状态 | [渲染器](../web/app.js)、[原生视图](../web/native-sdk.js)、[五来源功能表 D04–D08](sdk-feature-audit.md) |
| 审批/异步问答、MCP 表单、错误保留；747fbe14、44f7615f | Grok/Claude/OpenCode 按原生问题/审批类型映射；Claude MCP 用表单校验 | [原生请求卡](../web/native-sdk.js)、[elicitation 表单](../web/native-sdk-controls.js)；Agy headless 无 respond 能力 |
| 精确恢复 SID/cwd、未落盘创建行、独立 writer 不强接管；65a6caf8、b7487ec8、e167c0f8 | 四来源有创建回执及 pending 行、精确身份恢复和 writer 诊断 | [桥接入口](../crates/sessiondock/src/native_sdk.rs)、[节点发现](../web/nodes.js)、[原生合同](native-sdk.md) |
| Stop 不杀共享 daemon、不打开缺失历史；06a7c8f9、6f7efb88 | OpenCode 只 interrupt 目标 session；Claude 原生 interrupt、Grok cancel、Agy 只中断自有 child | 各来源 [合同](native-sdk.md)；不能复制 Codex app-server RPC 到其他协议 |
| 独立分支不跟随父终端、源会话保留；1dee01f1、30ffda53 | 共享时间线/导航处理；原生 fork 仍由各家接口产生身份 | [生命周期](lifecycle-binding.md)、[分组移动](session-move.md)、[功能表 H](sdk-feature-audit.md) |
| 终端输入非空保护、Esc 退回、粘贴确认；b8ca13ea、eec6e961 | Claude/Codex 共享识别器保护；Grok/OpenCode 有自己的已确认 editor 接口 | [输入判定](../crates/sessiondock/src/conversation/input.rs)、[CLI 状态](cli-state.md)；SDK 机器接口无需移植 PTY 光标/字符匹配 |
| cwd 引用碰撞、私有附件、Android 通用选择器；35924164、ca4c4320 | 文件解析/上传与 composer 共用，适用于各来源 | [文件合同](files.md)、[附件合同](conversation.md) |
| 树关系、近端 CLI 身份、隐藏/嵌套/平铺、移动进度、终端渲染与恢复 | 共享索引、process-links、生命周期、传输和终端，不按 Codex 单独分支复制 | [读模型](read-model.md)、[进程关系](process-links.md)、[传输](session-transfer-tool.md)、[终端](terminal-grid.md) |
| Codex rollout 轮换、rewind 继承、rename index、bundled context、app-server tier/策略 | 按原生记录/协议专属处理 | Claude parentUuid、Grok 自身历史、OpenCode 数据库、Agy 原生索引不能套 Codex 文件规则；各合同分别约束 |
| 已退休 executor、迁移基线、工程/发布检查、早期整合提交 | 历史证据，现行实现按当前合同检查 | 不恢复退休协议、旧前端或旧测试；[验证规则](validation.md)和[部署合同](deployment.md)为准 |

## 尚未等同 Codex 的协议能力

以下不是本次状态栏设计遗漏，也没有记成已修好；现状和后续工作仍在同一份 TODO 中。

| 项目 | 实际差距 |
| --- | --- |
| 完整活跃 turn 恢复与落盘/live 对账（25cf456d） | 四来源前端仍以消息 ID/文本裁剪为主，不能承诺 Codex 等价的完整 turn 重建；见 D03 |
| 原生重试事件投影 | Grok retry_state 仍是普通 system 项；OpenCode retry 状态未完整归一；Claude 已有 api_retry。前端保留原因不等于原生接线完成；见 D11 |
| 最后中断结果跨重开保持（0bcaf6d3） | Codex 有 last_turn。其他来源的取消回执、aborted 项、Agy interrupted 错误还不是同义持久回合状态；不能把 ready/DONE 推断为成功或中断 |
| 原生模型/effort/设置通知、完整消息分支与原输入恢复 | 部分来源事件尚未接全，不能用下拉样式一致替代原生生效确认；见 F03、H02、H07 |
| 子代理、review/revert、共享 TUI runtime、credits/reset 卡 | 按来源能力判断；Claude 子代理不可独立 resume，Agy headless 部分动作未证实。无同义协议的能力不制造假控件 |

## 本次验收口径

行为回归必须操作实际页面：用量首读失败、退避后恢复、保留旧 Token/billing、
切页取消读取和重试、迟到响应隔离、操作中收到新队列快照；四来源逐一覆盖。
布局和状态回归覆盖五种 SDK、普通对话、宽/窄屏及明暗主题；Codex 另覆盖
100%/125% 与 15 个宽度，确认最近权限 select 与空菜单行为继续成立。
执行者需亲自查看完整页面截图，核对页头/侧栏/消息/状态栏/菜单之间的关系、
对齐、留白、层级与可读性。

实际结果：13 项 Chromium 套件通过：native_sdk_footer、native_sdk_recovery、
sdk_status_disclosure、codex_sdk_layout、sdk_overflow、codex_sdk_controls、
conversation_status、claude_sdk、grok_sdk、opencode_sdk、agy_sdk、agy_usage、
native_sdk_queue。夹具均为隔离原生协议/假 CLI，不调用付费真实模型。
四来源的目录失败、当前值去重、发送期间改选和新快照中的队列按钮均实际交互验收；
Codex 另验 40.5% 的条长、完整提示和无障碍值一致，四来源验 25.4% 的上下文条。
已亲自查看五家 SDK 和普通对话的整页截图，并对照旧版菜单/底栏检查。
文档链接、AGENTS 和品牌检查通过。原生协议尚未接齐的项目仍见前表及 TODO。

## 完整历史索引

以下提交标题保留历史原文，可能包含退休名称，仅作历史审计，不能据此恢复旧品牌或实现。
类别用于归并检查主题，逐项传播结论见上表；分支记录不冒充当前版本已应用的提交。

### 当前基线祖先（174 条）

| 提交 | 日期 | 检查主题 | 历史标题 |
| --- | --- | --- | --- |
| `ea933b38` | 2026-10-19 | 输入与队列 | fix: use ordinary attachment paths for all SDK inputs |
| `e85e6ca0` | 2026-10-19 | 界面 | fix: keep a single-line footer with direct approval and sandbox controls |
| `e61b8873` | 2026-10-19 | 状态与用量 | fix: keep SDK usage visible without opening a menu |
| `e167c0f8` | 2026-10-19 | 共享基础与工程记录 | fix: preserve SDK resume identity and live tool state |
| `be005ae3` | 2026-10-19 | 输入与队列 | Fix Codex SDK capacity error display and queued sends |
| `ab2d8a9f` | 2026-10-19 | 状态与用量 | Show interrupted Codex status when CLI returns to ready |
| `9d34a395` | 2026-10-19 | 状态与用量 | feat: add semantic SDK status dots and animated activity |
| `9cd65393` | 2026-10-19 | 共享基础与工程记录 | fix: use native system selects for SDK permissions |
| `92f00110` | 2026-10-19 | 状态与用量 | fix: make status details useful and size floating notices to content |
| `91c19fc9` | 2026-10-19 | 模型与原生命令 | fix: support native Codex SDK rename commands |
| `8d2f1159` | 2026-10-19 | 界面 | fix: hide empty SDK overflow menus and orphan separators |
| `7b0241ec` | 2026-10-19 | 历史与消息 | test: cover forking a middle message with later conversation |
| `6f7efb88` | 2026-10-19 | 身份与生命周期 | fix: stop Codex threads without loading missing histories |
| `65a6caf8` | 2026-10-19 | 共享基础与工程记录 | fix: retain empty Codex SDK sessions across node updates |
| `5942bdbe` | 2026-10-19 | 身份与生命周期 | fix: treat inherited Codex rewinds as one forked thread |
| `51e77ba9` | 2026-10-19 | 界面 | Unify SDK usage labels and simplify native conversation menus |
| `4ce722ed` | 2026-10-19 | 共享基础与工程记录 | docs: audit feature parity and gaps across five SDK modes |
| `3f6d9b0b` | 2026-10-19 | 共享基础与工程记录 | fix: resolve SDK failures found in live browser sessions |
| `25178c71` | 2026-10-19 | 共享基础与工程记录 | feat: add native SDK modes for Grok, Agy, OpenCode and Claude |
| `20ccaf0d` | 2026-10-19 | 界面 | Compact SDK model selector and usage labels |
| `1dee01f1` | 2026-10-19 | 身份与生命周期 | fix: keep independent forks on their own terminal timeline |
| `0bcaf6d3` | 2026-10-19 | 状态与用量 | fix: preserve interrupted Codex SDK turn status |
| `06a7c8f9` | 2026-10-19 | 身份与生命周期 | fix: stop Codex threads without terminating their shared daemon |
| `fdc13e27` | 2026-10-18 | 状态与用量 | fix: seed Codex usage before live quota refresh |
| `e2d618f6` | 2026-10-18 | 历史与消息 | fix: retain live tool disclosure choices across output updates |
| `d8314f3f` | 2026-10-18 | 状态与用量 | Show managed Codex SDK sessions with blue status markers |
| `cb1dd1e0` | 2026-10-18 | 历史与消息 | fix: preserve partial native history in the SDK view |
| `c648497b` | 2026-10-18 | 输入与队列 | fix: retain composer focus and refresh path suggestions while typing |
| `c53bce4a` | 2026-10-18 | 界面 | fix: expose native Codex service tiers in composer toolbar |
| `c19a1d1a` | 2026-10-18 | 界面 | fix: reserve readable SDK status width and shorten labels |
| `b7976bbe` | 2026-10-18 | 共享基础与工程记录 | fix: discover daemon SDK sessions before opening them |
| `b7487ec8` | 2026-10-18 | 身份与生命周期 | Recover Codex SDK view after an independent writer exits |
| `82dfd9c0` | 2026-10-18 | 界面 | fix: consolidate SDK composer toolbar layout and typography |
| `7bdb69d3` | 2026-10-18 | 状态与用量 | Keep managed status blue when fleet observations lag |
| `77360c59` | 2026-10-18 | 模型与原生命令 | Render native shell cards and apply policies from checked submenus |
| `747fbe14` | 2026-10-18 | 历史与消息 | fix: render and submit asynchronous SDK questions |
| `6fb0238a` | 2026-10-18 | 输入与队列 | feat: highlight SDK shell input and offer Tab command suggestions |
| `6b30d787` | 2026-10-18 | 输入与队列 | Preserve SDK inputs until native consumption confirms delivery |
| `4b232911` | 2026-10-18 | 身份与生命周期 | fix: retry SDK reconnect through temporary server outages |
| `4afeb9e9` | 2026-10-18 | 界面 | fix: vertically center SDK user bubble action button |
| `4577ae44` | 2026-10-18 | 状态与用量 | fix: show ready status in idle Codex SDK composer |
| `44f7615f` | 2026-10-18 | 输入与队列 | fix: recover Codex SDK drafts and native protocol state |
| `4386b77d` | 2026-10-18 | 输入与队列 | Recover shared Codex worker identity and active turn steering |
| `395626ca` | 2026-10-18 | 历史与消息 | fix: restore height-clipped SDK bubble expansion |
| `30171a94` | 2026-10-18 | 共享基础与工程记录 | fix: reduce collector contention and restore closed SDK streams |
| `2af9b3f1` | 2026-10-18 | 历史与消息 | fix: open SDK before waiting for native history |
| `2af115d2` | 2026-10-18 | 输入与队列 | feat: complete real filesystem paths in both conversation composers |
| `25cf456d` | 2026-10-18 | 历史与消息 | fix: restore full SDK turn history after reconnect |
| `19e26b22` | 2026-10-18 | 历史与消息 | fix: preserve streaming disclosure interaction and show assistant prose in full |
| `069355a1` | 2026-10-18 | 历史与消息 | Show Codex SDK history without waiting for app-server startup |
| `e8ba9a30` | 2026-10-17 | 状态与用量 | Restore SDK context usage on resume and keep shell input in composer |
| `df2814af` | 2026-10-17 | 输入与队列 | Add SDK attachments, shared drafts and native conversation controls |
| `c9de1c1c` | 2026-10-17 | 模型与原生命令 | Enable SDK views and controls for Codex subagents |
| `b7101783` | 2026-10-17 | 共享基础与工程记录 | Add Codex SDK view backed by the shared app-server daemon |
| `9c4f317b` | 2026-10-17 | 界面 | Show compact quota dialog with installed clients and Codex credits |
| `9984dccf` | 2026-10-17 | 状态与用量 | Show running SDK compaction with indeterminate progress |
| `66b29a15` | 2026-10-17 | 身份与生命周期 | Join managed Codex terminals to the shared app-server |
| `641f5bfa` | 2026-10-17 | 界面 | Refine SDK composer overflow, queue inputs, and add message actions |
| `5f38b51c` | 2026-10-17 | 状态与用量 | Show used percentage (100% full) for SDK view usage and context bars |
| `5ddde00f` | 2026-10-17 | 状态与用量 | Show 5h/7d limits, credits and reset cards in the Codex SDK view |
| `45c13d1c` | 2026-10-17 | 界面 | Customize agent quota tables and show countdowns with fixed Codex credits |
| `30ffda53` | 2026-10-17 | 身份与生命周期 | Keep source sessions visible after explicit SDK forks |
| `2e64237a` | 2026-10-17 | 状态与用量 | Retry failed Codex SDK usage reads automatically |
| `2c8d19ec` | 2026-10-17 | 输入与队列 | Keep SDK composer controls compact across screen widths |
| `2c10f121` | 2026-10-17 | 身份与生命周期 | Show effective SDK settings and attach terminals to shared threads |
| `27acd39d` | 2026-10-17 | 历史与消息 | Connect SDK history to shared process fold controls |
| `1da7b707` | 2026-10-17 | 状态与用量 | Compact the SDK view usage bar after visual review |
| `18db6b05` | 2026-10-17 | 历史与消息 | fix: retain Codex terminal binding after empty-history rewind |
| `0a9c335e` | 2026-10-17 | 模型与原生命令 | Make SDK the default Codex view with lazy session creation |
| `709bace9` | 2026-10-16 | 共享基础与工程记录 | fix: attach native Agy subagents to their parent sessions |
| `323cbc90` | 2026-10-16 | 输入与队列 | Capture queued input receipts and rendered bubble state in bug reports |
| `f4319ac8` | 2026-10-15 | 界面 | Fix four-part session view controls and center resource dialog |
| `eea1763d` | 2026-10-14 | 共享基础与工程记录 | feat: make external integrations optional and export public source snapshots |
| `8fc2d721` | 2026-10-13 | 输入与队列 | Fix draft revision recovery after Codex rollout rotation |
| `eec6e961` | 2026-10-12 | 输入与队列 | Fix SEND paste confirmation for matching prompt suggestions |
| `8fb3fba1` | 2026-10-11 | 状态与用量 | fix: show live Codex compaction in conversation mode |
| `f9b95d6d` | 2026-10-08 | 界面 | Make toolbar views, agents and folding modes configurable in ellipsis submenus |
| `dcf7d925` | 2026-10-08 | 身份与生命周期 | Recheck spawn parents when native birth metadata completes |
| `910c95cf` | 2026-10-07 | 共享基础与工程记录 | Reduce Hub aggregation cost and isolate frontend search control |
| `d9f75ad8` | 2026-10-06 | 共享基础与工程记录 | test: align stale backend tests with current documented behavior |
| `7fdf7254` | 2026-10-06 | 共享基础与工程记录 | test: keep only headless browser suites; drop HTTP, oracle and bench scripts |
| `7753ea9e` | 2026-10-06 | 共享基础与工程记录 | fix(tests): reap private hosts on every browser-suite exit path |
| `4ddf4134` | 2026-10-06 | 身份与生命周期 | test: remove all unit tests and their descriptions |
| `2fb839d4` | 2026-10-06 | 共享基础与工程记录 | Add reproducible releases with cross-platform CI and real-CLI acceptance |
| `08720552` | 2026-10-06 | 共享基础与工程记录 | Merge branch 'worktree-agent-ab059d58dd7cd610a' into integrate-n |
| `ec713e36` | 2026-10-05 | 共享基础与工程记录 | feat(sidebar): add hidden, nested and flat child session modes |
| `c424cb63` | 2026-10-05 | 身份与生命周期 | Show continuous fine-grained session transfer progress |
| `c2894bdb` | 2026-10-05 | 输入与队列 | fix(legacy): keep queued-send state text separate from its dismiss button |
| `716e81db` | 2026-10-05 | 身份与生命周期 | chore: remove the retired Vue frontend |
| `2c8004ae` | 2026-10-05 | 状态与用量 | fix(sessions): reconcile restored Codex subagent activity |
| `d53ee7ce` | 2026-10-04 | 输入与队列 | fix(send): reconcile merged Codex steer echoes |
| `4fc86814` | 2026-10-04 | 共享基础与工程记录 | fix(sidebar): explain inactive children in active-only view |
| `4eb38204` | 2026-10-04 | 共享基础与工程记录 | refactor(frontend): unify search state and static Vue tree |
| `36466534` | 2026-10-04 | 身份与生命周期 | fix(transfer): distinguish historical UUID text from agent references |
| `02bf4d6d` | 2026-10-04 | 身份与生命周期 | fix(list): resolve exec nesting before first publication |
| `ca4c4320` | 2026-10-03 | 身份与生命周期 | Fix generic Android file picker and remove the PDF-only entry |
| `ab49ee7d` | 2026-10-03 | 模型与原生命令 | fix(sessions): show latest native model and refresh session titles |
| `901a2c67` | 2026-10-03 | 身份与生命周期 | fix(terminal): follow same-thread Codex rollout rotations |
| `780eb999` | 2026-10-03 | 共享基础与工程记录 | Prefer nearer child CLIs over inherited launcher identity and skip SSH resumes |
| `4e092f94` | 2026-10-03 | 共享基础与工程记录 | refactor(frontend): finish scoped Vue application migration |
| `32d9d031` | 2026-10-03 | 历史与消息 | Remove idle history scans and production test-session filtering |
| `252545f4` | 2026-10-03 | 输入与队列 | Fix Android PDF attachment selection with an explicit picker |
| `a08c9d3b` | 2026-10-02 | 身份与生命周期 | Persist transfer relationships and defer staging until explicit confirmation |
| `eaaa75a0` | 2026-10-01 | 身份与生命周期 | fix: initialize session nesting from observed CLI launches |
| `b2125b9d` | 2026-10-01 | 状态与用量 | fix: recognize Codex background terminal activity |
| `ac1a6484` | 2026-10-01 | 身份与生命周期 | fix: restore nesting regressions across discovery and transfer |
| `a3d65d2d` | 2026-10-01 | 界面 | Retain Codex background activity through quota banners and wrapped footers |
| `9e397ad3` | 2026-10-01 | 状态与用量 | Keep Codex agent liveness across long native turns |
| `7cd3e427` | 2026-10-01 | 状态与用量 | Repair validation gates and preserve live Codex turn summaries |
| `3180e908` | 2026-10-01 | 输入与队列 | Reconcile interrupted Codex queues without native echo |
| `12fe39bc` | 2026-10-01 | 身份与生命周期 | feat: move and clone complete Codex, Claude and Grok session groups |
| `f6adea45` | 2026-09-30 | 共享基础与工程记录 | Show CLI versions and manual updates in a machine-by-client matrix |
| `e2f6dd18` | 2026-09-30 | 身份与生命周期 | Confirm report rename before native rollout exists |
| `dd5e6867` | 2026-09-30 | 模型与原生命令 | fix: show effective Codex default reasoning effort |
| `c5b87b02` | 2026-09-30 | 输入与队列 | Answer Codex and Claude startup folder trust through shared composer cards |
| `b5657601` | 2026-09-30 | 身份与生命周期 | Rename Codex report threads before sending the first task |
| `a3d57765` | 2026-09-30 | 输入与队列 | Observe native Codex queued input without losing interrupted queue state |
| `9ed8af3b` | 2026-09-30 | 输入与队列 | fix: retain report attachments after Codex trust prompt |
| `9cb569a0` | 2026-09-30 | 输入与队列 | Track Codex built-in command dispatch without waiting for nonexistent echoes |
| `9214f768` | 2026-09-30 | 界面 | Fix Codex composer detection with quoted Ready footer |
| `78911eab` | 2026-09-30 | 共享基础与工程记录 | Use description-based BUG titles for report workers |
| `6c00cf0f` | 2026-09-30 | 输入与队列 | Project native CLI menus into shared composer cards |
| `42aa0a8b` | 2026-09-30 | 模型与原生命令 | Fix Codex picker catalog after CLI upgrades |
| `0ea2e965` | 2026-09-30 | 历史与消息 | Simplify Codex question cards and prioritize attention markers |
| `b8ca13ea` | 2026-09-29 | 输入与队列 | fix: refuse a send into a nonempty CLI editor and flag an Esc-returned prompt |
| `b85719e7` | 2026-09-29 | 状态与用量 | fix: keep a session turning while its background tasks run |
| `2fb79ec7` | 2026-09-29 | 共享基础与工程记录 | feat: centered in-page popups replace native alert/confirm |
| `0cb1e458` | 2026-09-29 | 身份与生命周期 | fix: autobind never binds a pending launch to a record that predates it |
| `f4e8ad37` | 2026-09-28 | 输入与队列 | Integrate OpenCode launch, composer, native history and lifecycle operations |
| `d0c9f3d6` | 2026-09-28 | 模型与原生命令 | feat: choose the model and effort when creating a session |
| `60ba1bfb` | 2026-09-28 | 共享基础与工程记录 | refactor: retire the delivery ledger |
| `4d15a005` | 2026-09-28 | 共享基础与工程记录 | refactor: retire the legacy reliable-send executor and browser outbox |
| `26f9f8b5` | 2026-09-28 | 状态与用量 | feat: show whether a running session is turning, waiting or idle |
| `056b8688` | 2026-09-28 | 共享基础与工程记录 | feat: let OpenCode handle a bug report |
| `738f3555` | 2026-09-27 | 身份与生命周期 | fix: show one list row per rotated Codex rollout thread |
| `424c2376` | 2026-09-27 | 输入与队列 | Recognize Codex busy composer above warning hints |
| `35924164` | 2026-09-27 | 共享基础与工程记录 | Fix cwd file references shadowed by historical basename collisions |
| `c827a4c3` | 2026-09-26 | 输入与队列 | fix: recognize scrolled Codex report composers before Enter |
| `c6975132` | 2026-09-26 | 输入与队列 | Reduce composer send round trips and release UI after delivery |
| `5e29c134` | 2026-09-26 | 共享基础与工程记录 | Preserve managed Codex sessions across startup update exits |
| `4e48038a` | 2026-09-26 | 模型与原生命令 | fix: load native Codex rename index beside configured sessions |
| `24bc55b8` | 2026-09-26 | 共享基础与工程记录 | Avoid repeated native target resolution on conversation sends |
| `9166143e` | 2026-09-25 | 历史与消息 | Fix same-thread Codex rollout continuation history and resume |
| `4ffeea50` | 2026-09-25 | 身份与生命周期 | Preserve running terminal guard identity across Codex rollout rotation |
| `4d010b7d` | 2026-09-22 | 状态与用量 | fix: classify bundled Codex context per native text block |
| `48e32b37` | 2026-09-22 | 历史与消息 | fix: interpret audited native history tag envelopes |
| `c621671b` | 2026-09-21 | 身份与生命周期 | Fix Codex report trailing-newline submission and remove blanket delivery stop rule |
| `bc8596e4` | 2026-09-20 | 共享基础与工程记录 | Require validated changes to be pushed and deployed immediately |
| `af099574` | 2026-09-20 | 共享基础与工程记录 | Wait for Codex startup before submitting report tasks |
| `4567ced3` | 2026-09-20 | 界面 | Recognize stable Codex paste cells and footerless report editors |
| `a2845495` | 2026-09-19 | 共享基础与工程记录 | Add a replay timeline: seek, play/pause and speed for recorded sessions |
| `9ae1500f` | 2026-09-19 | 共享基础与工程记录 | Probe the host for --no-record instead of assuming it |
| `76eda448` | 2026-09-19 | 身份与生命周期 | Offer delete, not stop, on an unused new-assigned native row |
| `61cede0f` | 2026-09-19 | 共享基础与工程记录 | Merge branch 'wp-record' into main |
| `5a4bb206` | 2026-09-19 | 身份与生命周期 | Stop default unit tests in the runner and agent contracts. |
| `4eca3b57` | 2026-09-19 | 输入与队列 | Classify composer SEND from the recognized PTY editor |
| `3138d6a1` | 2026-09-19 | 输入与队列 | Serve the staged attachment route again, relist three suites |
| `72f12ea3` | 2026-09-17 | 输入与队列 | Keep the first native final on top when folding, hide the composer of an ended launch |
| `36006773` | 2026-09-16 | 输入与队列 | Keep agent conversation; SSH gets PTY above a simple composer |
| `f52cacca` | 2026-09-15 | 共享基础与工程记录 | Accelerate gigabyte session reads and record measured performance |
| `c03e456c` | 2026-09-15 | 历史与消息 | Splice hot session reads from retained message bytes |
| `ae345a11` | 2026-09-15 | 身份与生命周期 | Attribute long main-thread frames and stop registering a service worker |
| `79e2d8cb` | 2026-09-15 | 身份与生命周期 | Render terminals with the DOM renderer only |
| `6e7240f9` | 2026-09-15 | 身份与生命周期 | Follow a Codex rollback branch in the conversation view and console |
| `6683c09c` | 2026-09-15 | 身份与生命周期 | Explain Codex side threads in conversation view |
| `fb0ad98b` | 2026-09-14 | 身份与生命周期 | Remove migration provenance and restate standalone project rules |
| `e3583998` | 2026-09-14 | 共享基础与工程记录 | Write console output straight into xterm |
| `dd9794c9` | 2026-09-14 | 共享基础与工程记录 | Support macOS nodes |
| `d244d767` | 2026-09-14 | 身份与生命周期 | Harden lifecycle and terminal recovery |
| `8d924705` | 2026-09-14 | 身份与生命周期 | fix lifecycle fork handling and clarify project docs |
| `558ae506` | 2026-09-14 | 身份与生命周期 | remove remaining AgentHub identifiers |
| `ac028ad2` | 2026-09-13 | 共享基础与工程记录 | SessionDock: initial import |
| `81a3cdee` | 2026-09-13 | 输入与队列 | Launcher parity with Python: home-directory cwd, symlinks, 1 MiB input |
| `7f6738f0` | 2026-09-13 | 共享基础与工程记录 | Align SessionDock runtime behavior with Python |

### 其他分支及旧整合记录（204 条）

| 提交 | 日期 | 检查主题 | 历史标题 |
| --- | --- | --- | --- |
| `7c3b0a32` | 2026-10-19 | 界面 | fix: restore ordered footer control folding |
| `78c05d0c` | 2026-10-19 | 身份与生命周期 | fix: treat inherited Codex rewinds as one forked thread |
| `52507fd5` | 2026-10-19 | 状态与用量 | fix: align usage before model and expand controls by available space |
| `45232dcd` | 2026-10-19 | 界面 | fix: keep status footer on one line with short status details |
| `fba5b8c3` | 2026-10-18 | 状态与用量 | fix: place SDK usage before the model selector |
| `f097c4d6` | 2026-10-18 | 状态与用量 | Align SDK composer status and usage and use icon actions |
| `ef97c976` | 2026-10-18 | 历史与消息 | fix: open SDK before waiting for native history |
| `c9206b39` | 2026-10-18 | 状态与用量 | fix: align native picker text line boxes with composer usage |
| `c720425a` | 2026-10-18 | 状态与用量 | fix: align SDK usage tightly before model controls |
| `c244e1ac` | 2026-10-18 | 界面 | fix: preserve full collaboration mode labels in compact toolbar |
| `905cd5a9` | 2026-10-18 | 界面 | Keep all SDK composer controls on one toolbar row |
| `7dac7b71` | 2026-10-18 | 历史与消息 | fix: restore height-clipped SDK bubble expansion |
| `5c23593e` | 2026-10-18 | 输入与队列 | feat: highlight SDK shell input and offer Tab command suggestions |
| `593320f6` | 2026-10-18 | 状态与用量 | Keep managed status blue when fleet observations lag |
| `54bab498` | 2026-10-18 | 输入与队列 | Revert "fix: recover permanently closed SDK event streams without losing drafts" |
| `3aa680cb` | 2026-10-18 | 界面 | fix: align composer picker typography with status and usage |
| `35e84d97` | 2026-10-18 | 界面 | fix: consolidate SDK composer toolbar layout and typography |
| `2c931274` | 2026-10-18 | 输入与队列 | fix: recover permanently closed SDK event streams without losing drafts |
| `23ca761a` | 2026-10-18 | 界面 | fix: let SDK status text occupy available toolbar space |
| `e0335b0e` | 2026-10-08 | 身份与生命周期 | Recheck spawn parents when native birth metadata completes |
| `e8f4535c` | 2026-10-07 | 共享基础与工程记录 | Reduce Hub aggregation cost and isolate frontend search control |
| `ea72cd47` | 2026-10-06 | 共享基础与工程记录 | test: align stale backend tests with current documented behavior |
| `d05b12a2` | 2026-10-06 | 共享基础与工程记录 | Merge branch 'worktree-agent-ab059d58dd7cd610a' into integrate-n |
| `c0678fd3` | 2026-10-06 | 身份与生命周期 | test: remove all unit tests and their descriptions |
| `950c6a95` | 2026-10-06 | 共享基础与工程记录 | feat: real-CLI release acceptance step |
| `94b425aa` | 2026-10-06 | 共享基础与工程记录 | test: keep only headless browser suites; drop HTTP, oracle and bench scripts |
| `0425dd99` | 2026-10-06 | 共享基础与工程记录 | fix(tests): reap private hosts on every browser-suite exit path |
| `b44d44c6` | 2026-10-05 | 身份与生命周期 | feat: show fine-grained progress within session transfers |
| `89f20710` | 2026-10-05 | 输入与队列 | fix(legacy): keep queued-send state text separate from its dismiss button |
| `697718b8` | 2026-10-05 | 身份与生命周期 | chore: remove the retired Vue frontend |
| `3b0f2134` | 2026-10-05 | 共享基础与工程记录 | feat(sidebar): add hidden, nested and flat child session modes |
| `34c6183e` | 2026-10-05 | 状态与用量 | fix(sessions): reconcile restored Codex subagent activity |
| `df05915d` | 2026-10-04 | 共享基础与工程记录 | refactor(frontend): unify search state and static Vue tree |
| `79af49c7` | 2026-10-04 | 输入与队列 | fix(send): reconcile merged Codex steer echoes |
| `797f462e` | 2026-10-04 | 输入与队列 | fix(send): reconcile merged Codex steer echoes |
| `78423cab` | 2026-10-04 | 身份与生命周期 | fix(list): resolve exec nesting before first publication |
| `5c5b9d2b` | 2026-10-04 | 身份与生命周期 | fix(transfer): distinguish historical UUID text from agent references |
| `50b2af35` | 2026-10-04 | 身份与生命周期 | fix(list): resolve exec nesting before first publication |
| `4728410f` | 2026-10-04 | 共享基础与工程记录 | refactor(frontend): unify search state and static Vue tree |
| `4655ef37` | 2026-10-04 | 共享基础与工程记录 | fix(sidebar): explain inactive children in active-only view |
| `38db6194` | 2026-10-04 | 身份与生命周期 | fix(transfer): distinguish historical UUID text from agent references |
| `2b8872f6` | 2026-10-04 | 共享基础与工程记录 | fix(sidebar): explain inactive children in active-only view |
| `a8fb5d7b` | 2026-10-03 | 身份与生命周期 | fix(terminal): follow same-thread Codex rollout rotations |
| `9ecc4a7c` | 2026-10-03 | 身份与生命周期 | Fix generic Android file picker and remove the PDF-only entry |
| `836fc704` | 2026-10-03 | 共享基础与工程记录 | refactor(frontend): finish scoped Vue application migration |
| `812e347e` | 2026-10-03 | 历史与消息 | Remove idle history scans and production test-session filtering |
| `796ee905` | 2026-10-03 | 模型与原生命令 | fix(sessions): show latest native model and refresh session titles |
| `770d17de` | 2026-10-03 | 共享基础与工程记录 | refactor(frontend): finish scoped Vue application migration |
| `3c942ac5` | 2026-10-03 | 模型与原生命令 | fix(sessions): show latest native model and refresh session titles |
| `245b91bd` | 2026-10-03 | 共享基础与工程记录 | Prefer nearer child CLIs over inherited launcher identity and skip SSH resumes |
| `21d3d13f` | 2026-10-03 | 身份与生命周期 | fix(terminal): follow same-thread Codex rollout rotations |
| `1e931a8d` | 2026-10-03 | 输入与队列 | Fix Android PDF attachment selection with an explicit picker |
| `17be68c4` | 2026-10-02 | 身份与生命周期 | fix: persist transfer relationships and defer staging until confirmation |
| `e9b9765f` | 2026-10-01 | 界面 | fix: retain Codex activity with wrapped mobile footer |
| `e36fd573` | 2026-10-01 | 身份与生命周期 | Keep Codex generation redirects source-scoped and verify mixed-family clones |
| `cec66079` | 2026-10-01 | 共享基础与工程记录 | fix: preserve native session bytes during cloning |
| `ca1c4ec7` | 2026-10-01 | 身份与生命周期 | feat: move and clone complete Codex, Claude and Grok session groups |
| `c39aad80` | 2026-10-01 | 输入与队列 | Reconcile interrupted Codex queues without native echo |
| `abad1699` | 2026-10-01 | 状态与用量 | Keep Codex agent liveness across long native turns |
| `794967b6` | 2026-10-01 | 输入与队列 | Reconcile interrupted Codex queues without native echo |
| `6bb7cf93` | 2026-10-01 | 身份与生命周期 | fix: initialize session nesting from observed CLI launches |
| `64c337a7` | 2026-10-01 | 状态与用量 | Repair validation gates and preserve live Codex turn summaries |
| `46258d48` | 2026-10-01 | 状态与用量 | fix: keep Codex background activity through quota banners |
| `3e43d616` | 2026-10-01 | 身份与生命周期 | Fix transfer isolation, cancellation and redundant preparation |
| `2a5437aa` | 2026-10-01 | 身份与生命周期 | fix: restore nesting regressions across discovery and transfer |
| `1f74499e` | 2026-10-01 | 状态与用量 | fix: recognize Codex background terminal activity |
| `1ebf6031` | 2026-10-01 | 状态与用量 | Keep Codex agent liveness across long native turns |
| `0d63fa74` | 2026-10-01 | 状态与用量 | Repair validation gates and preserve live Codex turn summaries |
| `f2143dc1` | 2026-09-30 | 历史与消息 | Simplify Codex question cards and prioritize attention markers |
| `f207de15` | 2026-09-30 | 界面 | Fix Codex composer detection with quoted Ready footer |
| `e54a664b` | 2026-09-30 | 输入与队列 | Migrate native Codex attachment memberships without rewriting client metadata |
| `e3f132bb` | 2026-09-30 | 输入与队列 | Match Codex built-in command dispatch when tracking SEND echoes |
| `d13bc946` | 2026-09-30 | 身份与生命周期 | Implement connected session groups and Codex transfer staging |
| `c7ec7b73` | 2026-09-30 | 身份与生命周期 | docs: design the cross-machine session move |
| `c2f37e29` | 2026-09-30 | 历史与消息 | Migrate Codex goal snapshots and structured history identities |
| `b0872d5c` | 2026-09-30 | 身份与生命周期 | Confirm report rename before native rollout exists |
| `acb91f37` | 2026-09-30 | 输入与队列 | Match Codex built-in command dispatch when tracking SEND echoes |
| `a8f4fc81` | 2026-09-30 | 身份与生命周期 | Stream durable cross-node family clones for Codex Claude and Grok |
| `a7990cfb` | 2026-09-30 | 界面 | Fix Codex composer detection with quoted Ready footer |
| `a0bb829b` | 2026-09-30 | 历史与消息 | Support native content-array agent results when cloning Codex groups |
| `996c0874` | 2026-09-30 | 身份与生命周期 | Confirm report rename before native rollout exists |
| `89bb8f3e` | 2026-09-30 | 身份与生命周期 | Remap native clone record identities and verify independent Codex resume |
| `86dfc1fd` | 2026-09-30 | 身份与生命周期 | Verify complex Codex moves through Chromium and native resume |
| `7eaa141c` | 2026-09-30 | 共享基础与工程记录 | Use description-based BUG titles for report workers |
| `76ff0263` | 2026-09-30 | 输入与队列 | Fix Codex /model waiting forever for a native echo |
| `6ebf943b` | 2026-09-30 | 历史与消息 | Simplify Codex question cards and prioritize attention markers |
| `6b1dbce4` | 2026-09-30 | 模型与原生命令 | fix: show effective Codex default reasoning effort |
| `668cb035` | 2026-09-30 | 输入与队列 | Project native CLI menus into shared composer cards |
| `6296a95e` | 2026-09-30 | 共享基础与工程记录 | Show agent CLI versions and manual updates in machine settings |
| `5fbc510b` | 2026-09-30 | 共享基础与工程记录 | Capture Codex project associations and compensate only inserted native rows |
| `4b2d5f1a` | 2026-09-30 | 身份与生命周期 | Publish same-node Codex group clones with native recovery and browser validation |
| `4ae21b0e` | 2026-09-30 | 共享基础与工程记录 | Show AI clients as a machine-by-client matrix with newest versions |
| `467e0879` | 2026-09-30 | 身份与生命周期 | Rename Codex report threads before sending the first task |
| `4520e0ca` | 2026-09-30 | 输入与队列 | Show Codex startup folder trust choices in the composer |
| `3732f3f9` | 2026-09-30 | 输入与队列 | Fix Codex /model waiting forever for a native echo |
| `32388e5e` | 2026-09-30 | 身份与生命周期 | Prepare session migration with native Codex probes and rollout identity reads |
| `20339b01` | 2026-09-30 | 输入与队列 | Finalize Codex queue observation for BUG-20260930-123331-237778 |
| `1de7b43c` | 2026-09-30 | 共享基础与工程记录 | Show agent CLI versions and manual updates in machine settings |
| `1cada00e` | 2026-09-30 | 输入与队列 | Project native CLI menus into shared composer cards |
| `1b788e14` | 2026-09-30 | 历史与消息 | Transfer Codex realtime history with consistent native identities |
| `1928fe81` | 2026-09-30 | 输入与队列 | Finalize Codex queue observation for BUG-20260930-123331-237778 |
| `18c2e08d` | 2026-09-30 | 输入与队列 | Show Codex startup folder trust choices in the composer |
| `14781f1b` | 2026-09-30 | 身份与生命周期 | Rename Codex report threads before sending the first task |
| `1437516f` | 2026-09-30 | 输入与队列 | fix: retain report attachments after Codex trust prompt |
| `0ff94999` | 2026-09-30 | 共享基础与工程记录 | Show AI clients as a machine-by-client matrix with newest versions |
| `0cf0d617` | 2026-09-30 | 共享基础与工程记录 | Use description-based BUG titles for report workers |
| `0ce3c6a1` | 2026-09-30 | 模型与原生命令 | Fix Codex picker catalog after CLI upgrades |
| `0b57c953` | 2026-09-30 | 模型与原生命令 | Fix Codex picker catalog after CLI upgrades |
| `05aa0e60` | 2026-09-30 | 身份与生命周期 | Recover pre-handoff moves with durable withdrawal and browser retry |
| `03faadf8` | 2026-09-30 | 输入与队列 | Preserve native Codex queue observation from mixed transfer commit |
| `d6951991` | 2026-09-29 | 输入与队列 | fix: refuse a send into a nonempty CLI editor and flag an Esc-returned prompt |
| `c99545bc` | 2026-09-29 | 身份与生命周期 | fix: autobind never binds a pending launch to a record that predates it |
| `b5a9fb65` | 2026-09-29 | 共享基础与工程记录 | feat: centered in-page popups replace native alert/confirm |
| `75e83127` | 2026-09-29 | 状态与用量 | fix: keep a session turning while its background tasks run |
| `fb008cb2` | 2026-09-28 | 共享基础与工程记录 | refactor: retire the delivery ledger |
| `e4a61934` | 2026-09-28 | 共享基础与工程记录 | feat: launch OpenCode as an AI CLI source |
| `df554c31` | 2026-09-28 | 模型与原生命令 | feat: choose the model and effort when creating a session |
| `b3835183` | 2026-09-28 | 共享基础与工程记录 | refactor: retire the legacy reliable-send executor and browser outbox |
| `9069ec80` | 2026-09-28 | 共享基础与工程记录 | feat: let OpenCode handle a bug report |
| `0b684dc8` | 2026-09-28 | 状态与用量 | feat: show whether a running session is turning, waiting or idle |
| `bbcbbd11` | 2026-09-27 | 共享基础与工程记录 | Fix cwd file references shadowed by historical basename collisions |
| `b4c84bc7` | 2026-09-27 | 身份与生命周期 | fix: show one list row per rotated Codex rollout thread |
| `0b0f35f2` | 2026-09-27 | 输入与队列 | Recognize Codex busy composer above warning hints |
| `d43a9ff1` | 2026-09-26 | 输入与队列 | Reduce composer send round trips and release UI after delivery |
| `bbfe8998` | 2026-09-26 | 共享基础与工程记录 | Preserve managed Codex sessions across startup update exits |
| `4056b9cf` | 2026-09-26 | 模型与原生命令 | fix: load native Codex rename index beside configured sessions |
| `3f8d4fcf` | 2026-09-26 | 输入与队列 | fix: recognize scrolled Codex report composers before Enter |
| `1db05cb7` | 2026-09-26 | 共享基础与工程记录 | Avoid repeated native target resolution on conversation sends |
| `db408c77` | 2026-09-25 | 历史与消息 | Fix same-thread Codex rollout continuation history and resume |
| `2acab6c6` | 2026-09-25 | 身份与生命周期 | Preserve running terminal guard identity across Codex rollout rotation |
| `9edd6983` | 2026-09-22 | 状态与用量 | fix: classify bundled Codex context per native text block |
| `0c033d2e` | 2026-09-22 | 历史与消息 | fix: interpret audited native history tag envelopes |
| `71af3174` | 2026-09-21 | 身份与生命周期 | Fix Codex report trailing-newline submission and remove blanket delivery stop rule |
| `f39453e9` | 2026-09-20 | 界面 | Recognize stable footerless Codex report editors |
| `eae265e5` | 2026-09-20 | 输入与队列 | Preserve Codex particle cells when checking paste stability |
| `b9eff15d` | 2026-09-20 | 共享基础与工程记录 | Wait for Codex startup before submitting report tasks |
| `5f9ba5c9` | 2026-09-20 | 输入与队列 | Keep the report regression above the Codex paste collapse threshold |
| `57edef54` | 2026-09-20 | 共享基础与工程记录 | Require validated changes to be pushed and deployed immediately |
| `f0b1a978` | 2026-09-19 | 输入与队列 | Classify composer SEND from the recognized PTY editor |
| `d98eb5eb` | 2026-09-19 | 身份与生命周期 | Offer delete, not stop, on an unused new-assigned native row |
| `a1b83977` | 2026-09-19 | 身份与生命周期 | Stop default unit tests in the runner and agent contracts. |
| `5725da4c` | 2026-09-19 | 共享基础与工程记录 | Probe the host for --no-record instead of assuming it |
| `394fe15d` | 2026-09-19 | 共享基础与工程记录 | Add a replay timeline: seek, play/pause and speed for recorded sessions |
| `10b156b3` | 2026-09-19 | 输入与队列 | Serve the staged attachment route again, relist three suites |
| `0ce7d5f4` | 2026-09-19 | 共享基础与工程记录 | Merge branch 'wp-record' into main |
| `af218a8c` | 2026-09-17 | 输入与队列 | Keep the first native final on top when folding, hide the composer of an ended launch |
| `605626e5` | 2026-09-16 | 输入与队列 | Keep agent conversation; SSH gets PTY above a simple composer |
| `f2e14e0e` | 2026-09-15 | 身份与生命周期 | Attribute long main-thread frames and stop registering a service worker |
| `ade53936` | 2026-09-15 | 身份与生命周期 | Explain Codex side threads in conversation view |
| `402fdba4` | 2026-09-15 | 共享基础与工程记录 | Read gigabyte sessions at I/O speed: fingerprints, 64 KiB spans, shared prefixes |
| `36a2243d` | 2026-09-15 | 身份与生命周期 | Follow a Codex rollback branch in the conversation view and console |
| `18b87cfe` | 2026-09-15 | 历史与消息 | Splice hot session reads from retained message bytes |
| `186be9ea` | 2026-09-15 | 身份与生命周期 | Render terminals with the DOM renderer only |
| `c6b32889` | 2026-09-14 | 共享基础与工程记录 | Write console output straight into xterm |
| `aa346585` | 2026-09-14 | 身份与生命周期 | fix lifecycle fork handling and clarify project docs |
| `a7b68762` | 2026-09-14 | 共享基础与工程记录 | Support macOS nodes |
| `96e80bbd` | 2026-09-14 | 身份与生命周期 | remove remaining AgentHub identifiers |
| `8df85efc` | 2026-09-14 | 共享基础与工程记录 | Drop mechanical Python provenance markers |
| `79079bc5` | 2026-09-14 | 界面 | Remove migration ledger and batch labels |
| `78f325f1` | 2026-09-14 | 身份与生命周期 | Harden lifecycle and terminal recovery |
| `0ddbe8bf` | 2026-09-14 | 共享基础与工程记录 | Restate predecessor-derived rules in their own words |
| `d007606b` | 2026-09-13 | 共享基础与工程记录 | files/media: read hard links, Python-shaped media discovery, degrading reference budget, state-dir file trash (WP-D) |
| `aaeeb030` | 2026-09-13 | 共享基础与工程记录 | concurrency and steady state: bounded-wait pools, 2000-event pages, cache budgets, trim, stage=replacement |
| `a2c963ed` | 2026-09-13 | 历史与消息 | Merge wp-g: live question cards and Codex approvals (claude-hook, bridge settings, prompt in messages/watch) |
| `8283d292` | 2026-09-13 | 共享基础与工程记录 | windows: launch, process identity, ledgers and env for a SessionDock node on Windows (WP-W) |
| `6b4e7baa` | 2026-09-13 | 历史与消息 | question cards and approvals: claude-hook subcommand, bridge settings, prompt field, Codex approval parser (WP-G) |
| `4a820730` | 2026-09-13 | 模型与原生命令 | Rename the project to sessiondock |
| `27d4822f` | 2026-09-13 | 历史与消息 | tests: suites for the WP-C read-model parity items |
| `225b8ff4` | 2026-09-13 | 身份与生命周期 | lifecycle/terminal (WP-E): process-evidence binding, pending archive/discard, Grok trash dir, worker injection, exit UX |
| `216af228` | 2026-09-13 | 共享基础与工程记录 | fonts: route CJK inside code, cwd and option stacks to AgentHub CJK Sans; sweep fixes |
| `0d8fae9a` | 2026-09-13 | 历史与消息 | read model: debug-run views, symlinked subagents, Grok dir size, /rename event, system hostname (WP-C) |
| `0d184281` | 2026-09-13 | 共享基础与工程记录 | search: persistent per-session search-text cache with a bounded parse budget (WP-B) |
| `0abde579` | 2026-09-13 | 历史与消息 | Merge wp-a: bounded read admission, larger history pages, transient-failure resilience, memory budgets, no dev banner |
| `f3bc058a` | 2026-09-12 | 身份与生命周期 | Batch 36 WP-AB: explicit /proc scan of external CLIs and write-once spawned_by (Python live.py e5b023a) |
| `ea69ca5d` | 2026-09-12 | 共享基础与工程记录 | Resync to Python 16cc89c: Grok in-flight envelopes, CLI barrier in pane walks, continued-in pane inheritance |
| `dd7f9aae` | 2026-09-12 | 历史与消息 | Batch 35 WP-C: torn/invalid JSONL lines skipped, broken Claude lineage truncated like Python |
| `d63b2ef1` | 2026-09-12 | 共享基础与工程记录 | Batch 36 WP-C: agent_items[].active and Claude continued_in on the list rows |
| `d097315a` | 2026-09-12 | 共享基础与工程记录 | Add tests/send_http_suite.py (reliable-send HTTP contract, both fake CLIs) |
| `d00c86ff` | 2026-09-12 | 共享基础与工程记录 | Batch 24: real CLI creation/resume argument contract |
| `bd9ad4ba` | 2026-09-12 | 输入与队列 | Headless-drafted tooling, fourth wave: HTTP contract suites and references |
| `b6cbbd76` | 2026-09-12 | 共享基础与工程记录 | Batch 34 WP-D: real_roots_bench.py and inventory_live_append_suite.py |
| `aa6a32c1` | 2026-09-12 | 历史与消息 | Batch 21: advanced Python parity, three read-model fixes, validation runner |
| `a690e8f9` | 2026-09-12 | 共享基础与工程记录 | Batch 32: Codex reliable send through the shared executor |
| `7855fe64` | 2026-09-12 | 身份与生命周期 | Batch 31: Claude reliable send — executor, terminal driver, native ack, HTTP |
| `73df1b9d` | 2026-09-12 | 身份与生命周期 | WP-AB follow-up: open *.jsonl under a configured read root counts; spawned_by_suite aligned with live.py |
| `72288ac4` | 2026-09-12 | 共享基础与工程记录 | Batch 35: real-root shapes Python reads — contract/parity suites, operator tools, docs |
| `70e18d82` | 2026-09-12 | 共享基础与工程记录 | Batch 35 WP-B: lifecycle_http expects the orphan Codex subagent as a catalogued non-row |
| `66c5a915` | 2026-09-12 | 共享基础与工程记录 | Batch 34 WP-D: list_rows_parity.py (grok q83, reviewed) — pins /api/sessions rows to Python list_sessions; 2 edge DIFFs and the Codex updated=mtime DELTA are the new index's targets |
| `5d24bc27` | 2026-09-12 | 共享基础与工程记录 | agent_active_real_check: finalize Claude rows like Python's index (continued_in), Codex rows are already final |
| `4d09a86c` | 2026-09-12 | 共享基础与工程记录 | Import Rust backend migration workspace through batch 19 |
| `3efee9e9` | 2026-09-12 | 共享基础与工程记录 | Replace the paid-CLI authorization rule with a cheapest-configuration default |
| `3ef6b157` | 2026-09-12 | 历史与消息 | Batch 35 WP-C integration: invalid-line and lineage warnings reach the detail meta, stale hard-failure tests converted |
| `3e912696` | 2026-09-12 | 共享基础与工程记录 | Batch 30: Codex native acknowledgment adapter (library only) |
| `3e811b4f` | 2026-09-12 | 共享基础与工程记录 | Batch 35 WP-D: Python-suite oracles for the real-root shapes Python reads but Rust rejects |
| `323f7a31` | 2026-09-12 | 共享基础与工程记录 | Add tests/check_config_suite.py (--check-config validation matrix) |
| `2991bc6f` | 2026-09-12 | 身份与生命周期 | Batch 34 WP-C: lazy index wired into SessionStore, frozen full-parse path removed |
| `28784f85` | 2026-09-12 | 身份与生命周期 | Add tests/session_stop_http_suite.py (batch 29 HTTP contract) |
| `24d92442` | 2026-09-12 | 共享基础与工程记录 | Batch 34 WP-A + WP-B: lazy index module and on-demand views (transitional tree) |
| `1eee87e3` | 2026-09-12 | 历史与消息 | Batch 35 WP-B: index graph follows Python fork lineage, orphan agents are no rows |
| `19217c86` | 2026-09-12 | 共享基础与工程记录 | Batch 36 WP-G: Codex multi-chunk tool-output envelopes show every chunk |
| `16a6b5a1` | 2026-09-12 | 模型与原生命令 | Batch 41 WP-B1: bug-report bundles and cheapest-model CLI workers (Python e5b023a bug_report.py / server._bug_report) |
| `0dd77a7e` | 2026-09-12 | 历史与消息 | Batch 33: read model tolerates current CLI record kinds (Python parity) |
| `0679d25e` | 2026-09-12 | 历史与消息 | Batch 35 WP-A: Codex identity = first session_meta; null history_base inherits nothing; thread_id wins over forked_from_id |
| `054751ab` | 2026-09-12 | 身份与生命周期 | Batch 35 WP-A: catalog/binding tests follow first-meta Codex identity |
