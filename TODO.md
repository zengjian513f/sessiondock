# SessionDock TODO

这里只记录当前尚未完成或尚未决定的工作。当前行为以代码、测试和 `docs/` 下的
合同为准。

## Kiro integration

- [ ] 补真实非空分页历史、审批／问答、取消、fork/delete/compact、设置回读与消费
  对账证据。当前能力来自已认证 initialize，相关恢复路径有合成 Chromium 验证；
  不能以合成事件替代原生高级能力验收。普通历史暂只读自有缓存，显式 SDK load
  同步 replay；默认 history 空响应不作为完整历史证明。见 [Kiro](docs/kiro.md)。

## Runtime and platform coverage

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

- [ ] 按部署报告补发仍未交付的目标及产物，恢复联机后用官方部署工具验证；
  连通性以现场舰队状态为准，不沿用旧报告的离线机器数。见[部署](docs/deployment.md)。
  当前前端快照 `7e122589`（滚动漏画修复，含此前重连及外部 CLI 启动提示修复）
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

- [ ] Codex SDK 完成实际使用验收后再退役旧对话/终端交互模式；非 Unix 接入仍未实现。
  子代理独立服务端草稿/附件身份待补，不能复用主会话草稿；当前行为见[Codex SDK](docs/codex-sdk.md)。

## Public distribution

- [ ] 补齐公共源码导出清单的 workspace 依赖：`public/manifest.json` 未包含
  `crates/sdkhost/`、`crates/sdkhost-client/` 及共享的 `crates/runtime_log.rs`。
  在临时导出树验证 Cargo 元数据、构建和文档；同步核对独立 SDK 宿主的二进制打包与安装说明。
  未完成前不把公共导出视为可构建发行物。实际缺项以所选提交的依赖闭包为准。

## Architecture follow-up

- [ ] 搜索控制器已独立管理请求、取消和计时器；继续按职责拆分侧栏、会话同步、
  草稿发送与终端连接模块，用现有浏览器路径保持 DOM、草稿和连接生命周期语义。
- [ ] 测量 Hub→节点的连接/握手开销后评估短请求连接复用；保留字面 IP、认证、
  证书验证、空闲超时及禁止重定向的合同，SSE/WS 独立管理。
- [ ] 扩展现有浏览器性能记录到大文件首屏、追加与冷搜索，并补充排队、解析、
  缓存命中和流完成的分阶段观测；通过 Chromium 用户操作路径验证。

## Read-path optimization follow-up

- [ ] 评估搜索正文与独立模型/回合扫描的按需字段投影。列表头/尾的大记录投影
  已接入；后续仍须保持 [read-model](docs/read-model.md) 的坏行、分页与源字节语义，
  不以新增输入限制代替优化。
- [ ] 评估 Grok 等来源的追加投影，处理跨记录工具状态并证明等价。Agy 无媒体且
  summary 未变的已验证前缀已支持续算，边界见 [append-cache](docs/append-cache.md)；
  Claude/Codex 可能回改旧消息，继续完整投影，不直接拼接旧事件。

## Validation stability

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
