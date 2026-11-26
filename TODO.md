# SessionDock TODO

这里只记录当前尚未完成或尚未决定的工作。当前行为以代码、测试和 `docs/` 下的
合同为准。

- [ ] Orion 恢复 SSH 后补发前端 `5f4e4676` 和节点服务 `211d62f5`，正式部署均因 SSH 超时未完成。
  前端包含顶栏文件夹筛选（BUG-20261126-095139-fa70df）、
  机器和 Agent 筛选右键恢复多选（BUG-20261126-094636-c012e0）、
  迁移任务的 ⋯ 菜单及一小时快捷图标（BUG-20261126-065008-fb7d7b）、
  用户绿色气泡的明暗主题修复（BUG-20261126-063413-4c1ccb）、
  迁移目标缺目录置灰、关闭窗口后找回迁移任务（BUG-20261126-053423-52de47）、
  停止会话树后的灰色未读角标（BUG-20261126-030145-19ff02）
  与 `33605d22` 的全页卡顿监测、Hub 节点状态和输入框 Tab 路径补全
  （BUG-20261123-082202-117ff4）。节点服务修复 Agy 回退后旧回复混入新回合
  （BUG-20261126-051132-9b88af）。其余六个目标均已更新并通过健康检查。

- [ ] Orion、Cetus 补发节点服务与前端 `20f18a07`，包含迁移后恢复研究关联
  （BUG-20261122-010737-9c9f24）、`5442f1c3` 的八种 SDK 回合阶段、字符数和思考折叠
  （BUG-20261121-111133-589aa4）、`46007545` 的 Kiro API 价值粗估（系数 0.15），
  以及 `7b20331f` 的 Codex API 价值修复与 Standard、Fast、Ultrafast 价目同步
  （BUG-20261121-080412-a6b7ee）。两台正式部署均 SSH 超时，联网后通过部署工具补发。

- [ ] Orion、Cetus 补发第二轮组件修复的节点服务 `caca8dbb`（后端版本
  `20261121T044547546279Z`）。两台在正式发布时均 SSH 超时。

- [ ] Orion、Cetus 补发七项组件审查后的节点服务和前端 `38d20736`。本次发布中两台均在 SSH 连接时超时。

- [ ] Orion、Cetus 补发 CodeBuddy 输出恢复修复 `5c140eb2`（BUG-20261121-015748-3bae52，
  后端版本 `20261121T034709610086Z`，包含子代理启动、重开状态及子代理历史修复）。两台正式
  部署均 SSH 超时。Lyra、Cygnus、Pavo、Vela 和 Hub 已更新服务二进制，重启并通过健康检查。

- [ ] Orion、Cetus 补发 Agy 启动取消修复 `226adf2a`（BUG-20261120-122309-518082，
  版本 `20261120T124544358967Z`）。两台正式部署均 SSH 超时；Lyra、Cygnus、Pavo、
  Vela 和 Hub 已更新服务与前端，重启并通过健康检查。

- [ ] Orion、Cetus 补发 Droid 回退分支历史修复 `d47c7312`
  （BUG-20261120-103452-af158e，后端版本 `20261120T104556404314Z`）。
  两台正式部署均 SSH 超时。Lyra、Cygnus、Pavo、Vela 和 Hub 已更新后端并通过健康检查。

- [ ] Orion、Cetus 补发 Droid Core 7 天 API 价值统计修复 `129f95ab`
  （BUG-20261120-062654-05c99e，后端版本 `20261120T063511809877Z`）。
  两台正式部署均 SSH 超时；Lyra、Cygnus、Pavo、Vela 已更新后端并通过健康检查。

- [ ] Orion、Cetus 补发 Agy 回退后输入对账的后端修复 `5b394a6a`
  （BUG-20261120-050041-623923，版本 `20261120T051625733454Z`），包含 Agy
  停止状态与回退冲突修复 `f0920068`。两台正式部署均 SSH 超时。
  Lyra、Cygnus、Pavo、Vela 和 Hub 的后端已更新、重启并通过健康检查。

- [ ] Orion、Cetus 补发 Agy 子代理后台任务与只读底栏状态修复 `7e7b622c`
  （BUG-20261118-043820-6e33ab，版本 `20261118T045815532353Z`）：正式部署均已
  尝试并因 SSH 超时未连接；Lyra、Cygnus、Pavo、Hub、Vela 已部署、重启并健康。

- [ ] Orion、Cetus 补发后台迁移与会话冻结修复 `682d8efe`
  （BUG-20261117-030514-c3841a，版本 `20261117T034714038936Z`）：正式部署已逐项
  尝试，两台均 SSH 超时；联网后通过部署工具补发服务和前端。

- [ ] Orion、Cetus 补发 Agy 延迟回显身份对账修复 `3f03173b`
  （BUG-20261115-234238-df94ad，版本 `20261115T235145526458Z`）：正式 SSH 尝试超时；
  Lyra、Cygnus、Pavo、Hub、Vela 已部署、重启并健康，现有会话宿主保留。

- [ ] Orion、Cetus 补发原生分支/回退修复 `217e8359`（BUG-20261115-155602-e82f81，
  版本 `20261115T165812622765Z`）：正式 SSH 尝试超时；Lyra、Cygnus、Pavo、Hub、
  Vela 已部署服务与前端、重启并健康，原有会话宿主保留。

- [ ] Orion、Cetus 补发前端 `770db621`（气泡滚动渐隐，BUG-20261120-125152-47b539，
  版本 `20261120T125841355046Z`）。两台正式部署均 SSH 超时，联网后用 `--web-only` 补发。
  此快照包含引导回执修复 `aa23045f`、Agy 历史尾回合修复 `46411c12`，以及此前的
  气泡横向滚动、目录后退、路径误判和额度底栏修复。

- [ ] 核对 `session_global_actions_browser.py` 刷新后侧栏仍应收起的断言。
  修复前基线与目录后退修复版均失败。

- [ ] Orion、Cetus 补发 Agent 周期用量、Agy/Droid API 价值及查询错误修正 `ec4f5458`
  （发布快照 `ac3ea3d6`，版本 `20261115T162422064809Z`）：正式 SSH 尝试超时；Lyra、Cygnus、Pavo、
  Hub、Vela 已部署、重启并健康，原生会话宿主保持运行。

- [ ] Orion、Cetus 补发周期性读取及 Hub 快照修复的后端快照 `db78edd0`
  （版本 `20261115T053112055239Z`）：正式 SSH 尝试超时；Lyra、Cygnus、Pavo、
  Hub、Vela 已部署、重启并健康。验收见 [事故记录](docs/history-incidents.md)。

- [ ] 核对 `codex_sdk_stalled_browser.py:74` 的静默原生连接恢复：
  BUG-20261114-233657-ab4cea 验收中，已发布基线与修复版均在等待重新连接后的
  空闲快照时超时；迟到创建、创建重启保留、调用者附属及节点/Hub 新建专项通过。
  未改写该断言，不能将迟到回执恢复等同于无响应原生连接恢复。

- [ ] BUG-20261114-233657-ab4cea 的存量会话尚未补迁：旧创建回执已丢失父关系，
  本次只读核对生产证据，未改写关系或重跑原迁移。后续确认关系与实际选择后，
  通过正式附属及迁移入口处理，不根据标题自动推断。

- [ ] Orion、Cetus 补发 BUG-20261114-233657-ab4cea 的迟到创建修复 `ce8c24a8`
  （已包含于发布快照 `bba2b5d3`）：正式 SSH 尝试超时；Lyra、Cygnus、Pavo、Hub、
  Vela 已部署、重启并健康。

- [ ] Orion 补发 Codex 中断回执修复 `f0ae5a5e` 的服务与前端
  （BUG-20261114-053237-efca19）：正式部署 SSH 超时；其余六目标已发布、重启并健康。

- [ ] BUG-20261113-231335-4ef293：额度计量变化原因与 Gemini App 0% 的差异仍待核对。
  原生周已用在重置时间不变时从 1.358% 降至 1.060%，服务快照还曾显示 0.154%。
  消息内容回退、载入等待及重复切换的多余渲染已修复；Orion 仍需补发服务修复
  `549c4e79`，Orion/Cetus 待补发前端 `497de694`（包含快速切换期间的历史复用及
  隐藏输入布局优化，正式 SSH 超时）。本次 Lyra、Cygnus、Pavo、Hub、Vela 已发布并健康。
  见[诊断记录](docs/history-incidents.md#bug-20261113-231335-4ef293agy-额度核对与首条消息位置)。

- [ ] 核对 `agy_history_browser.py:1023` 的原生目录删除／侧栏刷新：
  2026-11-12 附件修复验收中，当前调试二进制两次、旧 release 二进制一次均在
  删除目录行后等待侧栏移除会话超时；此前历史、图片、增量和分页步骤通过。
  附件专项由 `agy_media_browser.py` 的节点／Hub 普通历史与 SDK 八组合覆盖。

- [ ] 核对 `codex_sdk_stop_browser.py --case unreadable` 的冷入口自动打开预期：
  BUG-20261115-043012-a301cc 验收中，当前调试二进制与旧 release 二进制均在
  首次选择后等待 `/api/codex-sdk/open` 超时，尚未到达停止断言。
  本次 `--case normal` 的节点/Hub、宽窄屏停止、保留蓝灯与卸载灭灯回归通过。

- [ ] Orion、Cetus 补发 BUG-20261115-043012-a301cc 的自动换行、输出末尾滚动区与
  待卸载蓝灯修复 `a3275115`、气泡内滚动光标条件前端 `3f9de930`，以及
  全部正文预览修复 `aec95fe2`、去除蓝框前端 `4f5ed0f4` 与气泡滚动留白前端
  `676e1a36`（BUG-20261115-160737-2a51fa）：正式部署 SSH 尝试均超时；
  最新前端已在 Lyra、Cygnus、Pavo、Hub、Vela 部署、重启并健康。

## Frontend delivery

- [ ] Orion、Cetus 补发服务端展示计划 `347b563e`（版本
  `20261116T025637998799Z`，包含活动会话切换与大快照分批前端 `095c41a3`）：
  正式 SSH 尝试超时；Lyra、Cygnus、Pavo、Hub、Vela 的服务端和前端已更新、
  重启并健康；资源代理及 ptyhost 未更新，现有宿主保留。

- [ ] 核对 Grok 假 CLI 夹具在窄屏重入后的历史／实时衔接：同一次合成发送出现
  两个同文用户气泡，改动前 `5ca71a0b` 前端亦为两个。先核对夹具的原生身份与
  历史字段，不按正文去重；展示计划本轮未修改该衔接算法。

- [ ] 对齐 Agy/Grok/OpenCode SDK 用量套件的既有断言：Agy 额度页刷新按钮的
  disabled、Grok/OpenCode 底栏 title 中 Token 详情在 `5ca71a0b` 前端和展示计划
  修复版均失败。保留断言，独立核对当前用量合同；本轮未修改用量逻辑。

- [ ] 继续将 Codex 实时事件的展示投影、跨批历史尾部合并与 Markdown／代码高亮
  的可共享计算移到服务端；当前服务端计划覆盖历史批次、分页及七种通用原生 SDK，
  Codex 实时增量与旧节点兼容路径仍在浏览器规划，不将本轮算作全部计算迁移完成。

- [ ] 继续缩短大 SDK 快照提交后的分组／布局：消息构建已分批且切走可取消，
  2026-11-16 续修实测 Native 最终提交仍约 162 ms、六会话切换长帧最高约 366 ms；
  这轮已消除整段节点构建占用主线程，但尚未达到全程流畅帧率。
  见[诊断记录](docs/history-incidents.md#2026-11-16快速切换与延迟快照的前端阻塞)。

- [ ] 核对 Codex 底栏 125% 缩放时的窄栏溢出：`codex_sdk_layout_browser.py`
  在“不支持的命令”状态出现约 4 px 横向溢出；批处理前后前端及本次任务前的
  `b74c7a2b` 前端均复现。100% 明暗主题、十五种宽度通过，未将全套记为通过。

- [ ] Orion 补发 Agy 后台任务通知前端 `d1506cb3`（BUG-20261113-132137-89196c）：
  正式部署 SSH 探测超时；Lyra、Cygnus、Pavo、Hub、Vela、Cetus 均已仅发布前端、
  重启并健康。专项 Chromium 节点／Hub 八组合与既有消息呈现六组合通过，已完成视觉验收。

- [ ] Orion 补发 Agy 非图片附件修复 `27782c8e` 的服务与前端（BUG-20261112-075842-b01ef4）：
  正式 SSH 探测超时；Lyra、Cygnus、Pavo、Hub、Vela、Cetus 均已部署、重启并健康。
  此版本也包含 Codex 后台状态修复 `3d83ac59`；Cetus 已同步，不再待补发。
- [ ] 核对 `codex_sdk_status_browser.py` 的旧外部 CLI 提示与 daemon 卸载／重连假设：
  本次扩展尝试在这些原有断言失败，未将该综合套件记为通过，原文件保持不变。
  本次后台状态由独立 `codex_sdk_background_browser.py` 的节点／Hub 八组合验证。

- [ ] Orion 补发工具过程警示修复 `c3ddda91`（BUG-20261111-113726-0d1e21）：
  正式部署 SSH 探测超时，前端待恢复后补发。

## Droid integration

- [ ] Orion、Cetus 补发 BUG-20261115-160646-a893db 的 Droid 内部提醒投影修复
  `0e045cf1`（已包含于服务发布 `ac3ea3d6`）：正式 SSH 尝试超时；其余五目标
  已部署、重启并健康。验收见 [事故记录](docs/history-incidents.md#bug-20261115-160646-a893dbdroid-内部提醒在连接前闪现)。

- [ ] Orion 补发 Droid 接入 `b9159b17` 的 ptyhost，以及服务／前端
  最新版本（含文件回退、子代理控制、活动通知、CLI 设置及普通会话移动）：正式 SSH 尝试超时。
  Lyra、Cygnus、Pavo、Hub、Vela、Cetus 均已部署、重启并健康；Lyra 已启用 Droid。
  三个资源代理不涉及本次组件，按范围排除。
- [ ] macOS／Windows 安装 Droid 后补真实恢复验收；跨平台进程采集及原生 SID 匹配已接，
  当前只有系统命令与合成 Chromium 证据，不能以编译成功替代原生恢复验证。

- [ ] Droid mission 原生迁移：核实 missions/missionsV2 的 worker、handoff、云端与工作树依赖闭包，并分离 BYOK／runtime model 凭据。当前普通会话、settings 和文件回退快照可迁移；已识别 mission 指针明确提示未适配，不整目录盲拷。

## Kiro integration

- [ ] BUG-20261108-114059-330a36 存量关系仍未恢复：Kiro
  `sess_f9ff3f5c-a5f1-4539-8f8c-d7279aa02152` 的直接发起者为 Codex
  `01a1bae4-42d1-7963-b01d-21f66fb02a04`，其上级为报告指定的
  `01a1bae0-3592-7651-a677-82f7b2e03453`。原生调用与 SSH 记录已核对；旧采集器
  将累计丢失反复应用，连接已持久标为不确定，进程已退出。本次禁止手工修改生产数据，
  未补写父关系；后续经授权可通过正式附属接口恢复，不清除历史不确定标记。

- [ ] BUG-20261106-021317-6b1897 补充的 Kiro SSH 后台启动：原生发起记录已确认父会话，
  但源 SSH 已退出、当前跨节点账本没有该启动映射；无法证明当时哪次采样首先缺失。
  现有 socket 采样可能错过短连接，需要保留启动时的跨节点身份，而非按目录或提示词猜配。
  本机 SDK HTTP 创建与临时回执附属关系修复不覆盖此路径；三条已核实的存量关系已按用户确认通过正式接口补回。

- [ ] BUG-20261103-112752-cb1c9d：原生上游三次返回 `ServerConnectionResetError`
  的具体服务端／网络原因缺少日志，仍待定位。SessionDock 错误误记为取消已修复；
  原生 `TRANSIENT` 错误接入退避续跑，保留已完成工作，不重放原始消息。

- [ ] 补真实非空分页历史、审批／问答、取消、fork/delete/compact、设置回读与消费
  对账证据。当前能力来自已认证 initialize，相关恢复路径有合成 Chromium 验证；
  不能以合成事件替代原生高级能力验收。普通历史暂只读自有缓存，显式 SDK load
  同步 replay；默认 history 空响应不作为完整历史证明。见 [Kiro](docs/kiro.md)。

## Runtime and platform coverage

- [ ] `agy_history_browser.py` 最后的目录行删除检查超时：2026-11-11 在平台补齐基线
  与备用正文修复版均复现于等待 `S.sessions` 移除 SID；此前正文、搜索、分页、
  追加、改写、缺失恢复及服务重启断言通过。待单独定位列表移除刷新，未改写断言。

- [ ] Orion 补发服务 `388e839a`（含 SDK 平台补齐及 Agy 备用正文读取）与 sdkhost `5ea9eabc`：
  2026-11-11 两轮正式部署的 SSH 探测均超时；Lyra、Cygnus、Pavo、Hub、Vela、Cetus
  均已部署、重启并健康。这两轮无前端、ptyhost 或资源代理变更。

- [ ] 核对既有综合测试的阻点：`agy_sdk_browser.py` 已按当前入口显式点击启动，
  但额度刷新按钮禁用断言仍失败；`agy_steer_browser.py` 截取 DOM 前 80 字符
  查找消息的断言失败。2026-11-15 两者在修复版与修复前已发布二进制均复现，
  周期读取和 Agy 用量专项通过不等于这两个完整套件通过。
  `lifecycle_cli_browser.py` 的 `resume_sources` 精确字典漏了 `droid:false`，在 Codex 分支前失败。
  `claude_sdk_browser.py` 的旧 Token footer 文案、`sdk_generalization_ui_browser.py` 的
  Codex 更多按钮断言，在历史操作修复版和修复前发布二进制均失败；新增原生历史操作
  与 SDK 加载专项通过，未改写上述旧断言。

- [ ] Codex 活跃回合的重复渲染：先验证只变物理游标时跳过实时区重组、SDK 可见时延迟隐藏普通气泡，再验证工具完成与累计输出的局部更新。下次整页卡住时按页 ID 关联 watchdog 与浏览器线程栈；原现场尚未定因。见[活跃回合审查](docs/history-incidents.md#2026-11-23codex-活跃回合整页卡住)。

- [ ] BUG-20261110-135836-3fb287：前端整页无响应、偶尔触发浏览器结束页面提示，
  根因未定。报告内 7 次长帧最长 27.363 秒，多数缺少脚本归因；原生会话期间仍有输出。
  隔离历史读取和 SDK 回放诊断未复现秒级阻塞。已补应用阶段耗时及 SDK 规模记录，
  下次现场需核对 `main_thread.long_frame.work`；未结束的永久阻塞仍需浏览器性能栈。
  诊断增强 `25852c26` 已部署六目标并健康；Orion 正式连接超时，前端待补发。
  弱网模拟中等待网络时仍可操作，但积压的 120 工具／2400 增量恢复时观察到约一秒
  停顿，采样涉及 SSE 事件的状态栏／底栏布局与工具扫描；尚未重现现场 27 秒，需继续核实。
  源码定向探针进一步复现：300 条相同状态事件在 120／360 工具项下分别阻塞
  2.844／7.410 秒。优先修正无变化事件仍拆装工具组、重复状态绘制和布局读写交错；
  观察器扩大扫描和长正文重解析为次级风险，未证明原现场走了完全相同的触发序列。
  不能将诊断增强视为卡死修复。证据见[事故记录](docs/history-incidents.md#bug-20261110-135836-3fb287前端长帧归因缺失)。

- [ ] `tool_progress_browser.py` 在上述诊断验证的一轮中于窄屏 Codex SDK 参数收起
  断言失败；未修改运行时代码的完整 44 组合重跑通过。待核实偶发交互或测试时序原因，
  现有证据不足以判断与报告中整页卡死的关系。

- [ ] Orion 补发 SDK 节点复用、流式更新优化与双向历史分页 `0473c540`（服务与前端）：
  本轮正式 SSH 尝试超时，待恢复后补发；ptyhost、sdkhost 和资源代理不在本次变更范围。

- [ ] Orion 补发额度修复 `85d871ca`（BUG-20261107-104739-ad88f6，包含此前额度同步修复）。
  Lyra、Cygnus、Pavo、Hub、Vela、Cetus 已部署、重启并健康；Orion 正式 SSH 尝试超时，
  待恢复后补发服务与前端。三个资源代理、ptyhost 按组件范围排除。

- [ ] `kiro_sdk_browser.py` 在 BUG-20261106-143936-9d8a6d 排查中于 stop/reopen 后重载，
  等待原会话侧栏行超时（第 406 行），未完成综合套件；待定位列表恢复路径。
  额度同步另由 `quota_sync_browser.py` 的节点／Hub、已停止／已启动 SDK 实际操作覆盖。

- [ ] BUG-20261106-021317-6b1897：SDK 自动附属及临时回执修复 `6ce04eea`
  已部署、重启并健康于 Lyra、Cygnus、Pavo、Hub、Vela、Cetus；原有 ptyhost 保留。
  Orion 正式 SSH 尝试超时，待恢复后补发服务二进制；资源代理与 ptyhost 按组件范围排除。

- [ ] `nest_tree_browser.py` 的 `Grandchild C renamed` 列表刷新等待超时：
  BUG-20261106-021317-6b1897 验证中，新构建与未修改的 `aa1ab8a8` 基线均在同一断言失败；
  四组宽窄屏／明暗主题详情父关系同步断言通过。待定位这条既存刷新路径。

- [ ] BUG-20261105-075749-d52ddc：后台接收时间及隐藏缺失占位修复 `100ef0ae`
  已部署、重启并健康于 Lyra、Cygnus、Pavo、Vela，Hub 前端已同步且健康。
  后续移除旧分块时间条 `29e72863`、分支／回退确认 `665f0347` 均已以 web-only
  同步上述五个目标且健康。
  Orion、Cetus 正式 SSH 尝试超时，待恢复连接后补发后端、sdkhost 及前端；
  三个资源代理按组件范围排除，现有宿主进程保持运行。


- [ ] BUG-20261104-102740-283f30：SDK 查看入口修复 `f2c178bc` 已以 web-only
  部署、重启并健康于 Lyra、Cygnus、Pavo、Hub、Cetus。Orion SSH 超时待补发；
  Vela 仍报 `Store(Invalid)`，发布和恢复旧文件后的健康检查均失败（`ROLLBACK_FAILED`），
  18 个独立 ptyhost 保留，待恢复服务后补发。

- [ ] 跟进 Kiro 重试浏览器一次窄屏刷新中的 `takenOver is not defined`。
  后续完整节点／Hub 重跑通过，已增强页面错误堆栈采集；原失败无堆栈，原因尚未确认。

- [ ] BUG-20261103-112752-cb1c9d：错误分类及临时错误退避续跑 `b20fdfed` 已部署并健康于 Lyra、Cygnus、Pavo、Hub、Cetus。
  Orion 正式 SSH 尝试超时，待补发；Vela 原生构建成功但启动失败，恢复旧文件仍不健康
  （`ROLLBACK_FAILED`），18 个独立 ptyhost 保留，待恢复服务后补发。

- [ ] BUG-20261103-104023-0c1a1a：SDK 停止状态修复 `12e0d6d3` 已上线
  Lyra、Cygnus、Pavo、Hub、Cetus 并重启、健康。Orion SSH 超时待补发；
  Vela 已尝试发布并恢复旧文件，但既有启动异常仍在，结果 `ROLLBACK_FAILED`，
  18 个独立 ptyhost 保留；恢复服务后补发。

- [ ] BUG-20261102-162048-b98299：长消息列表输入修复 `5b03e212` 已以 web-only
  上线 Lyra、Cygnus、Pavo、Hub、Cetus 并完成重启和健康检查。Orion 连接超时待补发；
  Vela 部署前服务已不可用，本次发布和自动回滚后的健康检查均失败，待恢复后补发。
  本次未替换后端或 ptyhost，资源代理不在发布范围。

- [ ] BUG-20261102-104958-712bf3：搜索修复 `6c6aa872` 已上线 Lyra、Cygnus、Pavo、Hub。
  Orion、Cetus 正式部署连接超时，待补发；Vela 构建通过但启动报 `Store(Invalid)`，
  自动恢复旧版本后同样启动失败，目前服务不可用，18 个独立 ptyhost 保留。
  需只读定位持久状态校验失败并按恢复合同处理，不能清空或手改账本绕过。
  现场首次搜索的完整耗时无法由旧日志还原；已补搜索全程回执并合并进度流，
  若仍慢，以新增 trace 的响应头、扫描进度、完成及可见数定位。

- [ ] 更新 `codex_sdk_browser.py` 与 `codex_sdk_status_browser.py` 的旧自动启动假设：
  选择只读会话后需按当前显式启动流程操作；两套件当前分别停在历史额度和外部状态断言。
  外部额度、普通对话与显式启动后的额度写回由 `external_quota_browser.py` 覆盖。

- [ ] 根据 [外部程序启动审计](docs/process-launch-audit.md) 补足共享 tmux/systemd、
  容器/调度器/常驻 RPC 服务的每作业身份或因果证据；无证据时保持未归属。
  不能把启动请求、业务租约释放或自然语言“完成”当作 OS 资源归属/释放证明。
  节点覆盖与留存坏行的审计仍需补齐。

- [ ] 扩展公共进程归属层的平台覆盖与完整计量：Linux `resource-agent` 已提供
  生命周期事件、CPU/PSS/GPU/proc storage 指标及可选 60 秒 VFS/TCP/NFS 诊断。
  macOS/Windows 采集器、短命任务无遗漏的累计账本仍待实现；连接复用、跳板、NAT
  的完整关联需要额外证据，不能将部分观测视为完整计费。
  现行接口与后续计量口径见 [process-links.md](docs/process-links.md)。

- [ ] 为 Windows/macOS 的外部（非 ptyhost 管理）CLI 补齐进程发现与强身份验证。
  Windows 的受管 ptyhost 路径已经过实机验证，不应与此外部进程缺口混为一谈。
- [ ] 在 macOS 实机验证 sessiondock、ptyhost、文件原子替换、进程身份和终端生命周期；
  Linux 测试或 MSVC 交叉编译不能代替该验证。

## Native interaction semantics

- [ ] BUG-20261119-000133-abcaed 的 SDK 事件流静默触发点待核对。Lyra 账本确认
  消息即时入队，页面约 54 秒后才从历史取得正文。前端已能补读队列和重连；复发时
  用 `sdk.stream.reconnect` 的 `missing_input`／`heartbeat_timeout` 与入口日志核对。

- [ ] BUG-20261029-001953-cbb6c1、BUG-20261029-071653-e3316c：恢复链路已补齐
  历史请求替换与 SDK 浏览器订阅，但手机和桌面均有浏览器至入口的传输停顿。
  后一次约 35 秒后自动恢复，原生任务正常完成，未点击手动重连。现有入口日志
  缺少请求开始及上游耗时，仍不能区分网络／浏览器／连接层原因。下一步结合
  客户端实际连接记录定位。用户有 VPN、预期 DIRECT，实际路径未核实；07:44
  再发约 20 秒停顿，点击重连后新历史请求 152ms 返回。私有 Chromium 已复现
  HTTP/2 请求数轮换后多个旧 SSE 占据连接导致后续读取超时，生产是否同机制
  仍需连接／标签页证据；不能以合成机制替代根因确认，也不能把重连或自动恢复
  等同于消除根因。见[最新诊断](docs/history-incidents.md#bug-20261029-071653-e3316c桌面读取链路再次停顿)。

- [ ] BUG-20261027-082713-1f9886：已修复 Codex RPC 超时后仍复用失活连接、
  HTTP 取消使超时检测丢失的问题；首次原生事件接收阻塞的触发条件仍未确定。
  新增 `codex_sdk.rpc_timeout` 元数据日志，复发时结合 native 与桥接时间线核对。
  同次验证的 `codex_sdk_cancel_recovery_browser.py` 在原版与修复版均停在
  `legacy_attachment_queue` 的首次 connected 等待，需单独修复其夹具/打开路径。
  见[诊断记录](docs/history-incidents.md#bug-20261027-082713-1f9886)。

- [ ] 排查 `codex_sdk_browser.py` 的 `usage_opening_check`：2026-10-24 在
  原前端与长回合分页修复前端均因 7d 用量的 `9%` 元素缺失而失败；长回合缺陷
  回归、历史分页与 Codex 过程折叠另已通过，不能把该用量检查记为通过。

- [ ] BUG-20261022-124950-cf8e22：Agy 原生取消已去掉信号终止路径，但 1.4.5
  headless 的最终回执仍等待后台任务，后续 FIFO 输入也等待。需取得原生支持的
  即时回合结束／续发通道，达到 TUI Esc 的完整交互；不能伪造 result 或杀任务。
  见 [Agy SDK](docs/agy-sdk.md)。

- [ ] 排查 `sdk_mode_browser.py` 的 OpenCode 暖缓存重开间歇超时：运行归属已确认，
  但 SDK 仍显示“连接中”，5 秒内 SEND 未启用；节点和 Hub 均出现，具体等待环节待查。
  BUG-20261021-125735-d03f36 冷读取修复前的 `7a474ae3` 前端对照也复现，
  独立复跑曾通过；未放宽断言，不算本次冷历史读取修复已覆盖的成功项。

- [ ] Agy 后台任务/子代理完成后的原生续跑语义仍需专项验收。
  BUG-20261020-033847-a2d757 的节点重启存活由[独立 SDK 宿主](docs/sdkhost.md)承接；
  宿主保留进程与字节通道，不替原生 CLI 决定后台任务完成后的续跑。
  见[诊断证据](docs/history-incidents.md#bug-20261020-033847-a2d757)。

- [ ] 补齐[SDK 能力对照](docs/sdk-feature-audit.md)中的剩余差距：子代理专项验收、
  原生完整 turn 消费对账（包括 Grok ACP prompt 与历史 prompt index 的关联）、
  尚无原生证据的高级动作和有效设置回读。候选能力先核实原生语义，不为界面对齐伪造协议支持。

- [ ] 跟进 BUG-20261018-122932-0e7a3c 另报的 SDK 等待现象：原生回复和工具调用
  均早于 PTY 点击/连接，尚不能确认执行停滞；旧审计缺少 SDK 包，无法还原准确显示延迟。
  [共存契约与回归](docs/codex-sdk.md#terminal-coexistence)明确无 PTY
  也能执行并显示进展。此项独立于已补齐的服务档位控件，不能标为已修复。

- [ ] 定位 BUG-20261013-232935-441af3 的具体 PTY 上行中断环节：四条连接首屏正常，
  42 次浏览器输入没有节点写入回执，首个心跳均超时；第五条连接随后自行恢复。
  [诊断记录](docs/history-incidents.md#bug-20261013-232935-441af3-repeated-pty-upstream-stalls)
  已核对原生记录、账本及节点/Hub 日志。同一连接的浏览器发送、Hub 双向转发、
  节点接收/等待/宿主写入及取消最终状态已补充审计，Hub 记录自动复制进节点诊断包。
  再次发生时按这些边界定位；旧包仍不能指定故障代理/网络跳，隔离浏览器回归通过
  不代表历史生产问题已修复。

- [ ] 后续统一将 `openai_catalog_cache` 会话迁移到 `openai`：当前保留双 provider
  路由与原生运行会话；另行安排停止、卸载、备份与迁移，不在普通启动时自动转换。
  Cetus 已于 2026-11-11 完成默认配置和全部会话 provider 维护，其余机器另行安排。

- [ ] 核对 Cetus 原生历史分页的既有索引偏移：2026-11-11 provider 维护备份中，
  3 个会话共 20 个回合起止偏移已不指向 JSONL 记录边界；其余 64 个有效偏移迁移后
  仍有效。保留原生历史，由原生投影重建语义定位，勿直接移动实时检查点。
  现场与备份范围见[维护记录](docs/history-incidents.md#cetus-provider-统一维护2026-11-11)。

- [ ] 按部署报告补发仍未交付的目标及产物，恢复联机后用官方部署工具验证；
  连通性以现场舰队状态为准，不沿用旧报告的离线机器数。见[部署](docs/deployment.md)。
  Goal 状态图标前端 `d5c13fc2` 待补发 orion、vela；orion 正式尝试连接超时，
  vela 发布前已不健康，恢复旧文件后仍不可用（`ROLLBACK_FAILED`）。lyra、cygnus、pavo、hub、
  cetus 已发布、重启且健康；报告在 `target/deploy/20261103-045429-d5c13fc2/`。
  选择会话树并固定身份迁移 `cc040207` 已交付 lyra、cygnus、pavo、hub，重启且健康；
  orion、cetus SSH 超时；vela 构建通过，恢复旧文件后服务仍不可用（`ROLLBACK_FAILED`），
  三者待补发。完整报告在未跟踪的 `target/deploy/20261102-074719-cc040207/`。
  此前 Kiro 首次 headless／ACP 启动归属修复 `0a5dbf45`（包含 `f4652a8f`）已交付
  lyra、cygnus、pavo、hub，重启且健康；前端保留已交付的 `c51a54b8`。
  orion、cetus 正式发布尝试仍 SSH 超时；vela 在原生构建阶段停止发布，
  旧服务拒绝连接导致身份保全失败，后端未替换。三者待恢复后补发，vela 必须先成功保全身份。
  本轮逐目标日志及收尾记录在未跟踪的 `target/deploy/20261102-061906-0a5dbf45/`；
  部署主进程在 vela 切换前终止，结果以 `completion.json` 和逐目标日志为准。
  此前 Kiro 发布报告在 `target/deploy/20261102-015352-4bb9919b/`，vela 网页尝试恢复旧文件后
  仍不健康（`ROLLBACK_FAILED`），见 `target/deploy/20261102-015155-4bb9919b/`。
  此前后端快照 `6c1dad6c`（含 Codex 共享终端功能开关修复及 provider 恢复修复）
  已交付 lyra、cygnus、pavo、hub 且健康；
  orion、cetus 连接超时，vela 既有 `Store(Invalid)` 故障导致旧文件恢复后仍不健康，
  三者待补发。本轮报告在 `target/deploy/20261030-182455-6c1dad6c/`。
  此前前端快照 `7e122589`（滚动漏画修复，含此前重连及外部 CLI 启动提示修复）
  待补发 orion、cetus、vela：前两者连接超时；vela 部署前即未正常运行，恢复旧文件后
  服务仍不可用（`ROLLBACK_FAILED`）。lyra、cygnus、pavo、hub 已部署且健康；
  cygnus 首次 45 秒健康检查超时并回滚，同快照以 90 秒窗口重试成功。
  两份部署报告位于未跟踪的 `target/deploy/20261029-232108-7e122589/`。
  Codex 推理禁用退避修复 `374daabf` 已交付 lyra、cygnus、pavo、hub；orion、cetus
  离线待补发，vela 启动失败，文件自动回滚后服务仍未恢复（`ROLLBACK_FAILED`）。
  显式启动/接管按钮及模式绑定 `3178d1eb`（仅前端）已交付上述四个在线目标；
  orion、cetus 连接超时，vela 自动恢复旧文件后服务仍不可用，待恢复后补发。
  浏览器验证及视觉证据在未跟踪的 `target/explicit-composer-start/`，部署报告在
  `target/deploy/20261026-070212-3178d1eb/`。
  分模式启动／停止与互斥切换确认 `fd99ce5d`（服务端及前端）已交付 lyra、cygnus、
  pavo、hub 并通过健康检查；orion、cetus 连接超时，vela 文件自动回滚后服务仍未恢复
  （`ROLLBACK_FAILED`），待恢复后补发。验证和视觉证据在未跟踪的
  `target/session-mode-controls/`，部署报告在 `target/deploy/20261026-072055-fd99ce5d/`。
  菜单按 SDK／CLI 配成两个启动停止开关 `77d8708b`（仅前端）已交付同四个在线目标；
  orion、cetus 离线，vela 自动回滚后服务仍未恢复，待补发。证据在未跟踪的
  `target/paired-mode-menu/`，部署报告在 `target/deploy/20261026-073659-77d8708b/`。

- [ ] 恢复 Vela 的服务启动：气泡修复 `89b45c19` 发布及自动恢复旧文件后，launchd
  均退出 1，stderr 为 `Store(Invalid)`，部署结果 `ROLLBACK_FAILED`，健康端点不可用。
  只读定位无效存储及恢复方案，保留原生会话与状态；恢复后补发该修复。
  本轮 Orion、Cetus 连接超时，亦待联机补发；报告在未跟踪的 `target/bug-ddd959/`。

- [ ] Codex SDK 完成实际使用验收后再退役旧对话/终端交互模式；Windows 原生接入已有独立验证。
  子代理独立服务端草稿/附件身份待补，不能复用主会话草稿；当前行为见[Codex SDK](docs/codex-sdk.md)。

## Public distribution

- [ ] 补齐公共源码导出清单的 workspace 依赖：`public/manifest.json` 未包含
  `crates/sdkhost/`、`crates/sdkhost-client/` 及共享的 `crates/runtime_log.rs`。
  在临时导出树验证 Cargo 元数据、构建和文档；同步核对独立 SDK 宿主的二进制打包与安装说明。
  未完成前不把公共导出视为可构建发行物。实际缺项以所选提交的依赖闭包为准。

## Architecture follow-up

- [ ] 搜索控制器已独立管理请求、取消和计时器；继续按职责拆分侧栏、会话同步、
  草稿发送与终端连接模块，用现有浏览器路径保持 DOM、草稿和连接生命周期语义。
- [ ] 扩展现有浏览器性能记录到大文件首屏、追加与冷搜索，并补充排队、解析、
  缓存命中和流完成的分阶段观测；通过 Chromium 用户操作路径验证。

## Read-path optimization follow-up

- [ ] 评估搜索正文的按需字段投影。列表头/尾与独立模型/回合扫描的大记录投影
  已接入；后续仍须保持 [read-model](docs/read-model.md) 的坏行、分页与源字节语义，
  不以新增输入限制代替优化。
- [ ] 评估 Grok 等来源的追加投影，处理跨记录工具状态并证明等价。Agy 无媒体且
  summary 未变的已验证前缀已支持续算，边界见 [append-cache](docs/append-cache.md)；
  Claude/Codex 可能回改旧消息，继续完整投影，不直接拼接旧事件。

## Validation stability

- [ ] 核对 `kiro_sdk_browser.py` 的停止和重载后历史容器断言：周期用量发布版
  `3534d55a` 两次在第 392 行等待停止按钮可见时超时；发布前服务 `39616151`
  配前端 `838214ec` 越过停止后，在第 430 行查找 `#msgs` 正文失败，实际正文位于
  SDK 区域。两者不能认定为同一原因；此前额度、底栏和菜单步骤已执行，
  周期用量专项通过不等于该综合套件完整通过。

- [ ] 清理 sessiondock 严格 Clippy 与格式基线：`ec4f5458` 的发布门禁
  `--all-targets --locked -- -D warnings` 因 43 个既有告警失败，额度修改文件无告警；
  fmt 另在 api/codex_sdk.rs、api/lifecycle.rs、lifecycle/service.rs、model_cache.rs 失败。
  普通构建与浏览器通过不能代替严格 lint。

- [ ] 补齐发布门禁浏览器夹具的测试宿主定位：不发布宿主的 stage/bin 缺 ptyhost/sdkhost，
  quotas_browser 启动报 Launcher(UnsafePath)，droid_usage_browser 等待发送可用失败。
  `ec4f5458` 发布二进制放入独立测试目录、配已有测试宿主后，两套节点/Hub Chromium 通过；
  agent_usage_browser 同样通过，已验宽窄屏/明暗主题及错误状态。Droid 夹具需 cryptography，
  在 Lyra p311 验收；Cygnus 缺该依赖。该次按独立专项结果以 `--test none` 发布，未宣称全库门禁通过。

- [ ] 更新 `header_fold_browser.py` 对已有前后导航按钮的折叠预期：780px 时菜单包含
  `m-prev`、`m-next`，动作顺序断言遗漏二者；本次默认视图修改前的 `e6dffc16`
  与修改后均在相同断言失败，尚未完成该套件全程验收。

- [ ] 修复测试型号更新时暴露的验收阻点：`draft_rotation_browser` 的 current_uid 断言、
  `kiro_hub_browser` 的 SDK 连接等待均在未改测试上复现；`cli_menus_browser`、
  `send_codex_attachments_browser`、`startup_question_browser` 和真实 Codex 冷启动
  在客户端入口禁用处失败，`kiro_startup_browser` 等待响应超时。先修测试前提／定位
  行为，不将本次型号替换记为这些套件通过。Kiro 原生目录尚未返回 Haiku 5.5；
  Claude 真实套件缺少隔离测试所需的登录凭据，未完成新型号真实验收。

- [ ] `codex_sdk_layout_browser.py` 在 125% 缩放、窄侧栏组合的 blocked 状态下，
  状态栏溢出 4px、额度区域宽度变成零。BUG-20261107-104739-ad88f6 排查中，
  当前前端与未修改的 `f124ba57` 均在同一断言、同一尺寸失败；100% 明暗主题
  各 15 种宽度通过。保留断言，单独修复布局，不将额度数据修复记为此项已解决。

- [ ] 核对 `kiro_sdk_browser.py` 完整分支后的原会话侧栏可见性：2026-10-23
  在停止并刷新后，原 UID 的侧栏点击超时；按 UID 链接仍能打开完整缓存且不启动 SDK。
  保留原套件断言，独立定位目录／分支可见性，不把外部 ACP 观测专项通过当作整套 SDK 通过。

- [ ] 定位 `kiro_sdk_browser` 审批完成后请求卡偶发残留：2026-10-21 验收中
  消退断言曾失败，随后独立及三路并发复跑通过，原因未确认。保留严格断言，
  失败时同时记录原生请求、SSE、前端请求表与实际卡片，不能把选择器修正当作
  产品竞态已经解决。
- [ ] 跟进 `native_sdk_hub_browser` 刷新后偶发选中普通对话的现象。2026-10-21
  最终四组宽度/主题流程与 12 次审批中刷新诊断通过，但未确认原失败原因；
  不把状态栏测试的压缩完成等待、原子样式读取当作刷新问题已修复。
- [ ] 定位 2026-10-19 真实 Codex 双标签页撤销的两次无响应超时：修复额外历史
  SSE 后确认每页只剩两条流，后续四轮真实模型复测均成功；尚未重现原失败，
  不能把连接池修复视作全部原因。保留原失败、请求与 pointer 事件记录。
- [ ] 复现旧报 Agy `/resume` 菜单或 SID 搜索差异；当前 1.4.2 自动删除通过，
  菜单差异未重现，不凭旧描述改动选择器。信任提示的处理见 [Agy 删除](docs/agy.md#操作更新与问题报告)。

- [ ] 继续观察 `turn_state_browser` 持续追加时的瞬态读失败。已稳定复现并修复
  mtime 更新导致标量扫描丢弃完整记录的问题；上轮没有保留底层错误，不能断定
  所有追加偶发失败均由此引起。现有严格断言保留，失败时收集结构化读错误。
- [ ] 跟进完整浏览器验收的偶发停止/丢响应恢复失败，利用新增失败响应和
  删除进度诊断定位原因；保留成功数量、恢复结果与原生字节断言。
  评估发布门收集全部失败后统一修复，减少长菜单套件被重复执行的成本。

- [ ] 补发 CodeBuddy 接入（`9210138e`）到离线节点 orion、cetus，以及启动失败中的
  vela（部署时 launchd 未运行，旧文件已回滚，与此前记录的 Vela 启动问题相同）。
  只有 lyra 安装了 CodeBuddy 并配置了 `codebuddy-cli-v1` profile 与
  `SESSIONDOCK_CODEBUDDY_ROOT`；其他节点安装后按 [CodeBuddy](docs/codebuddy.md)
  补 profile。
- [ ] CodeBuddy 未接入项（证据见 [CodeBuddy](docs/codebuddy.md)、[SDK](docs/codebuddy-sdk.md)）：
  真实 CLI 浏览器套件（`*_real`，需隔离登录）、输入就绪／菜单题卡、问题报告处理会话、
  移动与克隆、SDK 用量与上下文（CLI 无对应控制请求）、`steer` 引导（CLI 有
  `steer` 控制请求，尚未接入网页）、强度运行中变更对后续请求的实际影响尚未独立观察。
