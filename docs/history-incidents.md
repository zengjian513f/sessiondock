# 历史事故与验收记录

本文件专门保存特定版本的诊断经过、修复背景和验收回执。内容保留当时口径，
不作为当前规则、部署指令或当前产物的验证结果。当前合同见各来源文档。

## BUG-20261030-013132-fb51f9：旧会话 provider 与 daemon 默认值不一致

诊断包的 manifest、browser-state、environment、220 条 events 与附件已核对；
没有 terminal.txt，页面终端也未连接。原生 session_meta 的 model_provider 为
`openai`，本机配置与报告 worker 回执为 `openai_catalog_cache`。
浏览器在 01:30:54.994Z、01:31:15.815Z 点击启动；01:31:16.504Z 的状态按钮
记录原生拒绝：旧会话使用 openai，当前 provider 是 openai_catalog_cache。
历史读取成功，游标保持 11230910；SDK 队列既有四项均 sent，CLI lifecycle
没有该会话记录。最初将问题归因于 thread/resume 未指定会话 provider，
但后续原生验证否定了“仅补参数即可恢复”的结论，见下文更正。

桥接现在先 thread/read（不读 turns），再把原生 modelProvider 传给 resume；
不修改全局配置或原生文件。provider 浏览器回归覆盖节点 openai、Hub 自定义
provider、缺失值兼容，以及发送、刷新和断线重连不重放。修复版和 SDK controls
浏览器套件通过，旧部署二进制在隔离 provider 夹具中无法连接。
SDK 综合套件的 usage_opening_check 在新旧二进制上都因缺少 9% 额度元素失败，
对应 TODO 已登记的既有问题；未声称综合套件通过。构建、文档链接和 AGENTS
检查通过。亲自查看节点/Hub 整页截图：历史、状态栏和输入框可读且对齐，
状态恢复为就绪；本次无前端样式修改。未运行真实模型回合。

运行修复提交 `b6bc5dd1` 已推送。全部七个节点/Hub 均经发布工具尝试，仅更新
后端二进制；资源代理和 ptyhost 不在范围。lyra、cygnus、pavo、hub 部署重启
并健康；cygnus 首次 45 秒检查超时自动回滚，同产物以 120 秒窗口重试成功，
六个 ptyhost 保留。orion、cetus 连接超时；vela 恢复旧二进制后仍不可用，
stderr 为既有 `Store(Invalid)`，结果 ROLLBACK_FAILED。待办见 TODO；两份
工具报告在未跟踪的 `target/deploy/20261030-014212-b6bc5dd1/`。

后续更正（附件 188）：Cygnus 确实运行上述后端，01:58:03.332Z 的启动点击
之后，01:58:08.032Z 仍记录同一 provider 错误；用户新开页面后也重试了启动。
直接只读 thread/read 确认原生返回 openai、notLoaded。使用正在运行 daemon
所对应的 Codex 0.166.0 二进制，在临时 HOME/CODEX_HOME、合成 JSONL、
命令行 gpt-5.6-luna/low 下仅执行 initialize/resume，不发送模型输入：默认
provider 为 cache 时，省略覆盖、modelProvider=openai、config 覆盖和 path
恢复均返回结构化 modelProviderMismatch；进程启动参数改为 openai 时恢复成功，
返回模型为 gpt-5.6-luna。上一轮假 CLI 未模拟进程级限制，不能证明
用户问题已解决。当时未改生产配置、原生历史或重启共享 daemon。

随后用户选择调整会话 provider。确认两个标识均使用 OpenAI 推理后，新增显式
单会话维护脚本 `deploy/codex_provider_migrate.py`（提交 `69e41008`），默认只读
计划，应用时遵守原生写入锁并先保存私有备份。仅修改数据库 provider、JSONL
首条元数据和最后一条本线程设置快照中的 provider，按字节增量同步分页索引。
真实 Codex 0.166.0 的隔离 Chromium 回归先重现拒绝，迁移后通过页面启动、刷新、
两轮历史及分页偏移检查，并验证已加载线程拒绝迁移；无模型输入。亲自查看
完整页面截图，历史和就绪状态正常。文档链接、AGENTS 与 diff 检查通过。

仅对报告线程执行上述维护：修改两处 provider，文件增长 28 字节，更新并检查
21 个回合的偏移，投影仍位于文件末尾。原生只读 thread/read 随即返回
`openai_catalog_cache`，模型仍为 `gpt-6-astra`，状态仍为 notLoaded。原文件及
数据库原字段已保存在服务私有备份目录。没有发送生产模型请求，也未替用户启动
实际会话；实际页面重试仍待用户确认。其他会话、全局配置和共享 daemon 均未变。
本轮只增加维护脚本、测试和文档，不发布运行产物或重启服务。


2026-10-31：用户选择“新会话默认 openai，已有会话保留原 provider”。实现
provider 进程路由，保留原共享 daemon，按需启动独立 listener；发送、控制、
订阅恢复、停止与模型列表使用所属连接。连接代次全局唯一，断开只重置本连接。
终端新建和接管使用同一选择，并在生命周期记录规范化时保留内部 socket 字段。
子会话模型查询传递 agent，避免误用主会话的 provider。

真实 Codex 0.166.0、临时 home、Luna/low 的 Chromium 验证通过：两种历史
分别恢复，新建默认 openai，终止一个测试 provider 不影响另一个，单独重新启动，
刷新及服务重启恢复且不重复创建进程；没有模型输入。合成 provider（节点/Hub）、
订阅保留、控制操作、provider 终端接管、CLI 新建以及子会话节点/Hub 桌面/手机
明暗组合通过。旧测试已跟随当前显式启动入口、回复落盘位置与接管文案更新。
亲自检查双 provider、终端和 Hub 子会话整页截图，布局和可读性正常。编译、文档
链接、AGENTS、品牌与 diff 检查通过。本轮不修改生产会话字段或全局 provider。

## BUG-20261029-235311-41269a：迁移后的停顿与浏览器离开

本次只读核对报告 manifest、browser-state、相关 events、原生主会话 JSONL、
SDK 输入队列、保留订阅记录、CLI lifecycle 账本及服务 journal。以下均为 UTC，
对应北京时间次日 07 时；会话为 `01a18698-c405-74b0-84b3-f6418da454b9`。

- 23:17:19 迁移通知进入原生回合，23:17:53 完成后同毫秒段启动 goal 续作。
  两条迁移输入的 SDK 回执均为 `sent`，各关联原生 turn，无投递错误。
  更早的 23:15:52、23:16:53 有 `turn_aborted: interrupted`，现有证据不能
  确定发起方；不能将它们归因于后来的浏览器离开。
- 页面 23:23:05.669 变为 hidden，23:38:56.694 回到 visible；隐藏事件直到
  返回时才上传，必须按 `client_ts` 而非 `received_at` 对齐。期间主会话每分钟
  均有原生记录，包含工具执行和进度答复。返回时页面仍持有游标 40281291，
  随后新页面在 23:39:02 已读到 41137925。页面离开时的旧画面不代表后台停工。
- 23:47:00.455–23:50:55.966 的原生 `ContextCompaction` 耗时 235511ms；
  `item_completed.started_at_ms/completed_at_ms` 给出完整时间，且同刻写入
  `compacted`。这是本段唯一超过一分钟的原生记录空档所在阶段，不能误判为
  会话退出。23:51:02 同一回合继续答复，23:51:45 完成并立即进入下一次 goal
  回合；报告提交前 23:52、23:53 仍有工具和消息记录。
- SessionDock 于 23:21:18–20 曾重启，但原生工作随后持续；当前保留订阅记录
  包含该线程，CLI lifecycle 没有该 UID/SID 的记录（本次走 SDK，不能据此称
  启动失败）。这些证据不支持“关闭浏览器杀死会话”。

可定位的长停顿是原生上下文压缩；未发现需要修改 SessionDock 生命周期的偏差。
报告没有该压缩期间的 SDK 状态截图或完整事件快照，不能断言当时页面正确显示了
压缩状态，也不能将其他报告的传输停顿根因套到本次。既有压缩与订阅保留合同见
[Codex SDK](codex-sdk.md)。本次仅登记诊断，不修改运行代码、生产会话或配置；
验证为文档链接及 AGENTS 检查，不将已有浏览器测试结果冒充本次运行或部署。

## BUG-20261029-161823-199df8：补充指令后的提前收尾

用户要求从催促语反查停工原因，而不是把每次“继续”都视为故障。只读检索三机
2026-10-23 至本次调查时的近期 Codex 消息，包含本月较早创建但近期仍有消息的
会话；从 5989 个本月 JSONL 文件中得到 230 条关键词候选、49 个会话。
候选包含内部续作通知、委派指令、正常暂停恢复和对结果的质疑，不是故障次数，
也不作为发生率统计。未覆盖已归档、删除或不在三机上的会话。以下时间均为 UTC。

- Pavo `01a18698-c405-74b0-84b3-f6418da454b9`：浏览器 16:03:46 点击立即引导，
  原生同秒收到“放宽到5个”，随后仍有多次工具调用；16:05:21 输出最终答复和
  `task_complete`，16:13:43 用户以问号催促后才继续。SDK 队列中原输入和引导
  均为 `sent`，关联同一 turn 且各有 native item。没有相应 CLI lifecycle 记录。
  首个明确偏差是未完成总任务却主动结束回合，不是 steer 丢失或立即杀死执行。
  下一轮模型承认五次接续实际报 `no rollout found`，先前“已恢复负责人”没有
  验证实际启动结果。16:03:18 和 16:13:53 的 `get_goal` 均实际返回 `paused`；
  用户自然语言要求恢复后，当次 turn 已启动，但持久 goal 仍未恢复，不能期待
  正常 final 后自动续作。现有 SDK `/goal resume`／目标面板“恢复”走原生
  `thread/goal/set` 的 `status: active`；本次没有代用户恢复生产目标。
  16:14 的后续回合另有原生 `turn_aborted`，不能混为正常收尾。
- Lyra `01a169c0-8e82-7963-abbb-9eae80085301`：10月24日 02:50 在进度答复中
  明确还有30项未收尾，却发出 `task_complete`；05:10 用户问“你就这样不做了”。
  回答进度代替了继续执行。
- Pavo `01a16c52-9515-74c1-8d40-165e8cef0048`：10月24日 18:44 正常结束，
  同时写明总任务未完成、训练终点后仍需验收；次日用户再次要求继续。
  后台计算继续不证明主会话仍在验收和调度，阶段性交接不能冒充全任务闭环。
- Lyra `01a171b5-5dd8-7b70-8721-6ae00721b9f3`：10月29日的追问揭示模型将
  “不自动重训”扩大为禁止回放、统计，实际只修改评价文字却报告重判完成。
  这是目标被文书工作替代及授权范围误读，不是传输停顿。
- Lyra `01a17c73-fce1-73b2-8e85-ebace22054fc`：10月28日 10:05 和 12:22
  在 commentary/工具调用后出现 `turn_aborted`；Cygnus
  `01a16eaf-538f-7392-98e9-9b78f7596b7f` 在10月24日 16:16 同样被中断。
  这些后接“继续”的片段不能算成模型自行 final；中断发起方尚未逐例核实。
  另有原生继续、页面请求超时的案例，见下方桌面读取链路记录。

改进应保留原目标与已授权边界：中途追问和参数调整处理完继续剩余工作；结束前
核对完整目标；实际验证启动、进展及交付；局部失败先修复或推进独立事项；用户
恢复后从断点接续，不重复启动仍在跑的任务。上述约定已写入共享 Codex 专属规则。
恢复任务还须核对已有 goal 的实际状态，不能假定自然语言“继续”等于恢复自动续作。
规则只能约束后续模型决策，不能恢复已中断的回合、解决网络/运行时故障，或保证
已加载规则的旧会话自动刷新。不能通过无限轮询或无授权自动发送“继续”来掩盖问题。

报告提示词另作最小修复：诊断包由强制完整阅读改为按问题需要选择证据，开发文档
同步；Chromium `optional_report_browser.py` 实际提交缺陷/功能报告，覆盖桌面、
手机及明暗主题，检查生成提示词，全部通过。文档链接、AGENTS 检查及 release
构建通过；运行修复提交 `4e423187`。这些检查验证提示词交付，不证明停工频率已降低。
该提交按 bin-only 尝试全部七个节点/Hub 目标：Lyra、Pavo、Hub 部署、重启和
健康检查成功；Orion、Cetus 不可达；Cygnus 健康检查超时后已回滚；Vela 发布
后 launchd/HTTP 检查失败，旧产物恢复后服务仍不健康（部署前也非 running）。
后三类目标待补发/恢复，三个无关 resource-agent 排除，未替换 ptyhost。

## BUG-20261029-071653-e3316c：桌面读取链路再次停顿

本次仅完成诊断，未确认可修复的代码根因。诊断包的 manifest、browser-state、
environment 和全部 3569 条 events 已读取；无附件或 terminal.txt。报告来自桌面
Edge SDK 页面，构建为 `20261029T005109042311Z`，仓库 HEAD 为 `c09fdb18`，
包含此前重连修复。以下时间均为 UTC。

- 最后正常草稿响应为 07:16:11.372。第一个异常读取是 07:16:11.617 发起的
  历史请求（trace `e4b99483-e7c0-432d-a739-b83113713ef8`），12 秒后超时；
  07:16:12.583 的草稿读取也在响应头前超时。两者下一次读取再次超时，审计
  上传同时累计两次超时，页面 online=true，不能据此认定公网连接正常。
- 入口代理在 07:16:12–44 没有任何 access 完成记录，07:16:45 恢复记录；
  同分钟 error 日志没有相关错误。access 记录没有请求开始时间或上游耗时，
  因此不能将“没有完成记录”写成“请求未到达”。节点与 Hub 的运行日志未见
  对应长耗时业务请求；该日志仅保留慢请求和错误，也不能证明请求从未进入服务。
- 原生 JSONL 在 07:16:17.111 完成工具调用，07:16:28.294 写入正常
  task_complete。SDK 队列中该回合为 sent，带原生 turn/item 身份；CLI
  lifecycle 账本没有该线程的记录。没有本次任务退出或重复投递的证据。
- 07:16:46.848 历史读取恢复，游标从 411732 前进到 453697，补入 3 条消息；
  DOM 随后从 working 更新为 idle。用户在 07:16:43 点击报告问题，期间没有
  点击立即重连。这里观察到的是自动恢复，不能当作手动重连失效的复现。

首次可见偏差位于浏览器读取与入口响应之间，停顿约 35 秒；原生执行在此期间
继续。仍缺少浏览器连接级记录及入口请求开始／上游耗时，不能进一步归因于
浏览器、客户端网络或入口连接层，也不能宣称此前重连改动消除了传输停顿。
本次不修改运行代码或生产配置，不重启服务、不改会话数据；后续定位合并到
[待办](../TODO.md#native-interaction-semantics)。

用户随后补充：客户端有 VPN／代理，该站点按预期应走 DIRECT；未注意其他网站
是否同步卡住。实际匹配规则和是否使用 TUN 尚未核实，不能据此归因于代理。

同一诊断会话于 07:44 再次停顿，直接核对当日浏览器审计与入口日志：
07:44:04.715 历史读取仍正常；07:44:05.406 草稿读取、07:44:07.815 历史读取
各在 12 秒后超时。07:44:25.260 用户点击立即重连，旧历史请求被取消，新请求
在 151.8ms 后成功，草稿于 07:44:25.681 恢复。入口在 07:44:05–23 没有完成
记录，24 秒起记录全局及 SDK 事件流结束，随后业务请求恢复。客户端与入口
时钟有小幅偏差，不能据秒级先后证明点击是网络恢复的唯一原因。节点仍持续
完成资源查询，Hub 没有该时段的长耗时业务请求。

两次停顿恢复点之间，同桌面浏览器的 SessionDock access 记录恰好为 4000 条，
提示检查 HTTP/2 连接轮换。现场 nginx 1.24.0 未显式设置 keepalive_requests，
[官方默认值](https://nginx.org/en/docs/http/ngx_http_core_module.html#keepalive_requests)
为 1000；仅凭整倍数不能证明这就是生产根因。私有 loopback nginx 1.24.0＋
headless Chromium 合成页面中，将上限缩至 20 并通过按钮发起 250 次读取：
单条 SSE 全部成功；每 20 次额外保留一条 SSE 时，第 113 次读取达到 3 秒超时，
日志显示多条长流占据已达到请求上限的旧连接。此实验复现一种连接耗尽机制，
不证明生产页面存在相同数量的遗留订阅。当前 SDK 切页及全局重连路径均有
close 调用；还需客户端实际连接／标签页证据。实验日志保留在本机临时诊断文件
`sessiondock-h2-probe.log`、`sessiondock-h2-many-probe.log`，不作为修复验收。

## BUG-20261029-001953-cbb6c1：同步停顿与重复重连

诊断包含 1282 条审计记录（含重发副本），无附件或终端文本。页面为手机 SDK，
报告时 online=true。以下为 UTC：00:07 和 00:19 的历史、草稿请求在响应头前
达到 12 秒期限，触发“立即重连”提示；原生 JSONL 在此期间继续产出，SDK 投递
均为 sent，没有对应的 CLI lifecycle 记录。唯一的 turn_aborted（00:06:40）
对应浏览器点击停止，不能视为自行断联。

跨层核对：00:18:56.507 的草稿请求和 00:18:57.890 的历史请求没有及时返回，
随后客户端重试也超时；入口代理在 00:19:25 集中记录这些请求的 499，以及
19 条被取消的通知流。同期审计上传在 Hub 等待约 29.5 秒后以 400 结束。
节点和 Hub 没有对应的长耗时业务请求，其他慢历史读取为约 2–4 秒。
这将停顿定位在浏览器至入口的传输链路，现有记录不能进一步区分移动网络、
浏览器或连接层原因。00:18:05 的单次“会话在读取期间变化”503 耗时约 2.5 秒，
是另外一次读取失败，不能解释上述整段停顿。

已确认的恢复缺陷有三处：每次点击都会关闭、新建全局通知流，连续点击会撤销
刚建立的连接；历史已有读取时直接复用旧请求，不能立即重试；SDK 模式的
`watchSession` 本来就不创建正文 watch，而按钮也没有重建独立的 SDK 事件流。
因此用户补充“点击重连从来没有发生过作用”后，仅合并点击的第一版修复被确认
不完整。合成 Chromium 在该版本复现：点击后 SDK 输出仍是旧内容。

完整修复合并进行中的恢复，取消旧增量历史读取并从已接受游标立即重读，同时
重新订阅当前 SDK 的事件流；保持既有快照和草稿，不增加 SDK open 请求，不重放
输入。按钮显示“正在重连…”，成功读取后才清除相应网络错误。此修复恢复浏览器
订阅与读取路径，不宣称消除了最初的传输停顿；剩余定位见 [TODO](../TODO.md)。

补充验证：七种 SDK 的节点／Hub 共 14 个合成 Chromium 场景中，建立后不再推送的
流只在重新订阅时提供新快照；旧历史请求保持挂起。点击按钮后 3 秒内出现新输出、
完成新的历史读取，草稿保留且没有额外 open 请求。手机 SDK／桌面对话连续点击
仍只建立一条全局通知流；后续 offline／online 恢复也通过。
执行者亲自查看 `target/cbb6c1-manual-final/` 的七来源 472px 浅色节点、1280px 深色
Hub 整页，以及 SDK／对话重连中与恢复后的整页：提示与按钮沿用现有风格，
“正在重连…”可读，历史、输出与底栏草稿没有被清空，恢复后提示消失。
最终弱网套件完整运行通过（CodeBuddy 两个场景独立补跑通过）；首次历史超时、
通知流静默检测、草稿响应体超时与保存恢复、发送回执丢失后仅投递一次均通过。
文档链接、AGENTS 检查和 JavaScript 语法检查通过。

交付：第一版 `aab3e54f` 与完整修复 `89780dc6` 均已提交、推送。完整修复仅发布前端，
lyra、cygnus、pavo、hub 均重启且健康，分别保留 27、13、0、0 个受管 ptyhost；
orion、cetus 正式尝试后连接超时。vela 发布前已有启动故障，工具恢复旧文件后仍
无健康响应（`ROLLBACK_FAILED`），18 个 ptyhost 保留。三个 resource-agent 没有
受影响产物，按组件范围排除。首次 cygnus 的默认健康窗口过短、回滚旧文件后也
未及时就绪，之后旧服务自行恢复；完整修复使用 300 秒检查窗口，发布用时约 88 秒
并验证成功，未替换后端或 ptyhost 二进制。
发布使用已提交快照，部署测试门因独立验证已完成使用 `--test none`；该标志本身
不算验证。完整修复的两份报告是未跟踪的
`target/deploy/20261029-005109-89780dc6/report-20261029-005303.json` 与
`report-20261029-005457.json`，待补发目标已记入 [TODO](../TODO.md)。


## BUG-20261028-004342-1d2ae4：迁移提示与已停止状态矛盾

诊断包含 1276 条浏览器事件，无附件或终端文本。以下为 UTC：用户在
00:40 停止父会话 SDK，随后多次打开迁移会话树；节点日志确认 00:40:48、
00:41:52、00:42:23 的计划请求返回 409。报告所选子会话的原生最后一轮在
00:23:09 完成，55 条投递均为 sent，没有待发送项或对应 CLI lifecycle 记录；
报告时页面显示不活动。

已确认的语义缺陷：迁移把 SDK 原生 loaded 证据统一描述成“会话仍在运行”，
而停止 SDK 仅取消本连接订阅，页面会隐藏已停止线程；原生 loaded 仍可能保留。
修复按证据说明原生线程尚未卸载、可能继续写入及重试条件，保持原有写入保护，
并记录具体阻碍 UID、SID、操作及证据。旧日志没有记录这次拒绝的具体证据，
不能回溯证明当时是哪一个连接保留线程，也没有对生产会话执行停止或迁移。

后续只读查询 Cygnus 原生日志数据库补齐了时间线：00:40:25 收到 Interrupt，
01:10:25 才执行线程 teardown、Shutdown 并退出 agent loop，间隔恰好 30 分钟；
teardown 记录 `had_active_turn=false`。随后直接查询原生接口，该线程已不在
loaded 列表，`thread/read` 返回 `notLoaded`；daemon 本身未重启。
因此三分钟后仍不能迁移是真实的延迟卸载，不是模型仍运行。30 分钟间隔与 Codex
历史固定卸载延迟一致；本地源码已改成可配置、默认 60 秒，不能用它代替运行二进制
的实际行为。原生 `config/read` 未返回显式卸载延迟，具体订阅者及倒计时起点仍未
由接口暴露，不能将“其他客户端一直占用”当作本事故已确认原因。

合成 Chromium 在旧版复现笼统提示，修复版通过保留订阅、重连、重启、停止后
阻止迁移及原生卸载后允许预检。执行者亲自检查 1280 浅色与 390 深色完整页面，
提示在既有错误区域换行，关闭与取消按钮可见，没有新增控件或样式。构建、文档
链接及 AGENTS 检查通过。

## BUG-20261027-082713-1f9886

诊断包无附件或终端文本，包含 1377 条浏览器审计事件。以下为 UTC：原生
JSONL 在 08:24:35.308 写入 interrupted；浏览器点击停止后却收到 Hub 的连接
中断提示，随后回答入队和立即引导也失败。节点读取历史正常，但 SDK 快照仍
保留旧 inProgress 回合；原生 thread/read 返回 idle。SDK 队列中一条输入长期
sending，另一条 failed；该 SDK 会话无对应 CLI lifecycle 记录。

现场只读检查确认桥接 Unix socket 曾积压约 140 KB 未读数据，线程栈中有
实时状态锁等待；随后自行追上 idle。不能据此确定最初阻塞由哪个事件触发。
已确认的恢复缺陷是 RPC 超时只移除等待者，连接继续标为可用，保留订阅也不
重连；调用方取消 HTTP 时，原有 timeout future 一并取消。修复让 RPC 回执
等待独立于 HTTP 生命周期，期限到达后关闭桥接连接、释放待决 RPC，并沿用
保留订阅恢复；不重启原生 daemon，不重放结果不确定的输入。

合成 Chromium 的静默 socket 场景在原版因旧回合未消失失败；修复后通过取消
HTTP、中断恢复、草稿保留、显式续发，以及未知发送失败后的撤销与不重放。
保留订阅/重启回归通过。通用 SDK 用量套件与取消恢复套件在原版及修复版均有
同点失败，未记为通过；首次阻塞触发条件和这些既有测试失败仍列于 TODO。

## BUG-20261025-073120-ddd959：SDK 发送短暂出现两个相同气泡

完整读取处理机诊断包的 manifest、browser-state、environment 和 544 条浏览器事件；
没有 terminal.txt 或附件。浏览器处于 Lyra 的 Kiro SDK 视图。以下为 UTC：
节点投递日志在 07:30:54.867 进入 sending，07:30:55.864 才确认投递完成；
原生 JSONL 在 07:30:55.086 记录该输入，正文只出现一次。投递账本、桥接检查点
和历史投影使用同一提交身份，原生 prompt RPC 仅一条且写入为 written。
该 SDK 创建会话没有对应的普通 CLI lifecycle 记录；运行归属由 SDK 检查点及宿主保存。

首个偏差在浏览器合并：Kiro begin_turn 在投递回执之前广播用户消息，
前端同时绘制实时正文与 sending 队列正文；队列渲染缓存也不依赖实时消息身份。
隔离 Chromium 实际发送后控制 SSE 顺序，旧版稳定得到两个气泡，计数断言失败。
修复为快照提供当前 delivery_id，前端按它关联用户消息 request_id；正文先到时
保留一份正文与发送进度，不冒充原生接收或消费。明确重试使用新投递身份，
同文独立提交不合并；失败、待核对和撤销行为保持原合同。

验证：Rust 节点及 Hub debug 构建通过；[发送回执浏览器](../tests/native_sdk_receipt_browser.py)
覆盖 Kiro、Agy、Grok、Claude、OpenCode 的 1440px 暗色、390px／675px 明色，
全部通过，包含正文先到、独立同文输入、新投递重试及失败恢复。
[Kiro 引导浏览器](../tests/kiro_steer_browser.py)在实际隔离节点和 Hub 通过，
核对真实队列的投递身份及发送、排队、引导、撤销、丢回执、刷新和草稿保留。
均使用合成 CLI／SSE，没有调用真实模型或修改生产会话。
执行者亲自查看旧版及修复后的五来源整页截图：重复正文消失，发送提示仍可读，
气泡间距、正文换行、页头和输入栏在宽窄屏保持一致。
日志和截图保存于未跟踪的 `target/bug-ddd959/`；文档链接、规则及品牌检查通过。

## BUG-20261020-222858-6b521a：手机切换会话先露出上一会话

核对 manifest、browser-state、environment、1090 条事件与 terminal.txt。以下时间为
2026-10-20 UTC：事件 999 在 22:28:34.477 打开目标会话，缓存命中；事件 1003
已经记录新 selected 和 215 条目标缓存，但展开的详情仍留着此前的 16 个消息节点。
直到事件 1019（22:28:40.873）才替换详情。其他来源切换也有相同顺序。

`openSession` 先展开手机详情、更新选择，再等待 SDK／CLI 归属查询，缓存渲染和
冷加载占位都在等待之后。因此首个偏差是浏览器显示身份与已选会话不同。
只读核对生命周期账本、会话投递存储、终端快照及对应原生 JSONL：目标 SID 一致，
生命周期为 running、无 failure，原生记录在该时间窗继续正常追加。没有独立 delivery
目录；本修复不修改原生文件、投递或会话状态。

修复将旧详情与观察器的清理提前到首次异步等待之前，立即建立目标标题和现有加载提示，
再展开手机详情。沿用正式标题的 span 截断样式，防止长标题挤占窄屏按钮。
保留查询归属、缓存、迟到响应检查及新建会话自动跟随语义。

隔离 Chromium 用真实点击和阻塞归属响应复现旧版：侧栏已选 Claude，正文仍显示上一
Codex 的标题与消息。回归入口为 [会话切换](../tests/session_switch_browser.py)、
[历史加载](../tests/history_loading_browser.py)和[新建自动跟随](../tests/new_session_follow_browser.py)。
切换夹具覆盖节点六来源、Hub 已声明 SDK 能力的五来源、冷开、缓存及等待中另选会话；
当前 Hub 不声明 Kiro SDK 能力。使用合成历史、假 SDK 和现有测试二进制，不调用真实模型。
三项 Chromium 回归与文档检查通过。亲自检查 1280px 明色、390px 暗色六来源整页，
以及 675px、390px 明暗主题的历史加载、失败重试与长历史；目标标题、正文和操作一致，
长标题正常截断，加载提示不遮挡返回和视图按钮。自动跟随另覆盖键盘高度 472×287。
本地日志与前后整页截图在 `target/bug-6b521a/`，不入库。

## BUG-20261020-222442-40bae0：历史读取成功但 SDK 装饰用户消息时抛错

报告的 manifest、browser-state、environment 和 128 条浏览器事件均已核对；无附件或
terminal.txt。以下时间为 2026-10-20 UTC。22:24:05.372 的事件 11 已收到 HTTP 200
及 717 条历史消息，end 为 3536465；事件 23 首次记录
`SdkUI.decorateUserBubble is not a function`，事件 69 在刷新后再次记录同一错误。
页面缓存已有历史，SDK 渲染却进入“读取历史失败”分支，是读取成功后的首个明确异常。

只读核对原生完整 JSONL 与 Agy 镜像：均为 897 条步骤，末步 896，文件长度与浏览器
游标一致；原生 SDK 输入账本的六次输入均 completed、consumption=observed，无错误。
该路径不走 PTY，lifecycle 无对应启动记录，也没有 delivery 目录，未修改生产数据。

已发布的 native-sdk-controls 调用了缺失的共享方法。无分支/回退能力的 Agy 也会传入
空操作列表，因此普通历史用户消息即可触发错误。补齐公共方法，空操作列表不生成
菜单，有能力的来源沿用既有消息菜单、回调和布局；不隐藏读取错误或改变历史协议。
补丁仅提取共享工作区已有实现中这一个方法，其余未提交重构保留。

隔离 Chromium 在原发布源码上复现 historyFailed=true 和红色“读取历史失败”。
回归入口为 [SDK 历史与消息菜单](../tests/sdk_generalization_ui_browser.py)，新增初次
打开、刷新后进入 SDK、无能力菜单及失败状态断言；使用合成消息和假服务。
修复快照上该套件的 Agy、Grok、OpenCode、Claude、Codex 全部 PASS，
[Kiro SDK 套件](../tests/kiro_sdk_browser.py)也 PASS；复用现有测试二进制，未调用真实模型。
亲自检查六来源整页：五来源 1440px 明色与 390px 暗色，Kiro 1280px 明色与 390px 暗色。
Agy 初次打开与刷新均恢复“未运行”状态，历史正文和输入区保持完整；有操作能力的
消息菜单沿用原有紧凑行、字号及边距，窄屏菜单未越界。日志与前后截图位于本地
`target/bug-40bae0/`，不入库。

## BUG-20261020-122705-101231：修复前死亡的 Agy 运行时在下一次输入时报重启

完整读取本报告 manifest、browser-state、environment 及 815 条审计事件；无附件和
terminal.txt。报告保存在处理节点，origin 为 Lyra，服务端与原生证据均在 Lyra
只读核对。SDK 不走 PTY 投递，lifecycle 无对应记录；采用 native-sdk-inputs、
sdk-adapters、sdkhost 输出/回执/退出状态及原生 transcript，未改生产状态。
以下时间均为 2026-10-20 UTC+8。

- 原生最终回复 step 433 的 created_at 为 17:46:20，但宿主输出的 result
  仍携带此前 stream interrupted 错误。旧宿主 status.json 在 17:46:40.987
  落下 PID 1379552、signal 9 的退出结果。对照当时 ERROR 终结进程的代码，
  根因判断为旧桥接错误清理；17:52 的恢复日志也记录 turn_error 终止请求，
  但此时进程已死，signal sent=false。首次 kill 尚无关联日志，不虚构其调用记录。
  已确定进程早于本报告和最后一次修复死亡。
- 19:45:10 部署标记确认 Lyra 已发布 `89d3f138`。恢复只重放旧宿主输出和回执，
  无法复活先前已死的进程/子代理。`turn_finished.runtime_retained=true` 是
  result 处理后的暂态，后续 EOF 路径清除运行时并保留退出诊断，不能据此断言进程存活。
- 浏览器 20:24:24.735 点击“停止会话”、20:24:26.970 确认，
  20:24:29.206 点击“启动会话”。这些操作晚于旧宿主死亡，不是旧进程退出的起因。
- 20:25:06.651 接收新的审计输入，20:25:15.024 宿主写入回执为 `reused=false`，
  新原生 PID 2186898；消费证据随后为 observed。原生 step 434 是新输入，
  step 435 于 20:25:15 新写出 subagents/server restart 通知。它不是浏览器重复渲染，
  而是死亡后首次恢复原生会话的通知。此报告没有证据证明新版本再次无故杀进程。

回查该问题链路的九个直接相关实现/诊断提交（不包含模型、正文展示等旁支）：

| 提交 | 实际修改 |
| --- | --- |
| `1cf6bc2a` | 部署时保护尚未完成的原生 SDK 输入，先行止血 |
| `9619939c` | 独立 sdkhost 持有进程/管道，节点重启只重连 |
| `717477c2` | Agy 连续回合保持 stdin，复用运行时，避免逐轮 EOF |
| `7baffd64` | 区分已消费但回执矛盾与发送失败，保留此类运行时 |
| `02cee2fa` | 旧宿主 EOF 保留回合结果、退出原因和待核对状态 |
| `629aa4d3` | 增加投递、回执与进程终止的关联诊断 |
| `168317b0` | 本轮完成证据与历史错误分开判断 |
| `e19c3bb6` | 恢复完成证据同步持久输入账本，消除旧 unknown |
| `89d3f138` | 所有合法 ERROR 回执只结束回合，不再杀持久 CLI |

另有 `30dd34ea` 加固宿主发布验证。前面的错误分类修复没有完整移除 ERROR
终结进程的路径，直到 `89d3f138` 才补全；不能把九个提交都表述成九次根因修复。

本次从已部署产物复制二进制到私有测试目录，使用 `89d3f138` 的 tests/web 快照、
合成 CLI 和 loopback，调用现有 `native_sdk_queue_browser.py` 的四个 Chromium
场景：两种 ERROR 的 `restart_conflicting_result`、非冲突
`closed_runtime_result`、Agy `restart_idle_registry`，全部 PASS。实际覆盖发送、
节点重启、移除待核对记录后同 PID 继续、已死宿主只在新输入时建新 PID、
旧输入不重发及空闲重连不重复初始化。未调用付费模型，未在生产会话上试发。
本次仅补充事故记录，无新增运行时补丁或生产重启；已死亡的旧子代理不会因修复自动恢复。

## BUG-20261013-232935-441af3: repeated PTY upstream stalls

The Cygnus report was captured on build `20261013T223823185417Z`, commit
`afbe3f7c`, with a clean checkout. The complete manifest, browser state,
environment, 842 audit events and terminal capture were reviewed alongside
the lifecycle/conversation ledgers, the native Claude JSONL and the node/Hub
service journals. Times below are UTC on 2026-10-13.

- At 23:28:40 the first claim completed in 52 ms and the browser painted the
  initial grid within 167 ms. The first recorded input at 23:28:44.093
  (event 513, 27 bytes) had no corresponding node write receipt. This is the
  first observable input-path divergence, before the report dialog existed.
- Across the first four connections the browser recorded 42 inputs. Each
  received its initial screen but timed out its first bidirectional heartbeat
  after 10 seconds (events 533, 625, 697 and 792). None had an input-write
  receipt. Replacement claims completed in 49–72 ms, and ordinary HTTP/SSE
  traffic continued. The browser reported zero buffered WebSocket bytes at
  timeout; this is not a remote-delivery acknowledgement.
- The fifth connection opened at 23:29:27.254. Its first input receipt was
  two zero-byte resize frames (event 820), not the user's text. The terminal
  capture still showed an unsubmitted draft; the native history checkpoint
  stayed at 9277741 throughout the report window. PTY keystrokes do not use a
  SEND delivery ledger; the deployment had no `delivery/` directory, and its
  current conversation ledger and running lifecycle record were inspected.
- The subsequent daily audit shows new input on the fifth connection at
  23:29:37.784 and a node write window at 23:29:38.810 (39 bytes).
  At 23:29:51.028 native JSONL recorded a new user message. The same managed
  instance remained running; this recovery preceded this investigation and
  involved no repair or restart by the investigator.

The evidence identifies the browser-input → node-write gap and failed
bidirectional transport checks. It does not identify whether browser/network,
HTTPS proxy, Hub forwarding or node receive processing caused the gap.
The journals contain slow successful HTTP requests but no event establishing
a WebSocket failure cause. Header-button errors after opening the report are
occlusion by that dialog, not evidence that the terminal keyboard was blocked.
No speculative runtime change, input replay or production-data edit was made.

On 2026-10-14, `python3 tests/terminal_heartbeat_browser.py` passed in headless
Chromium with private synthetic fixtures: separately stalled downstream/upstream
sockets recovered, the host survived, and uncertain input was not replayed.
`python3 tests/terminal_rtt_browser.py --binary target/debug/sessiondock` also
passed: node and authenticated Hub paths each returned and painted all 85
keys (5 warmup, 40 individual, 40 grouped). Both runs used existing debug
binaries and the current frontend; they were not production-network tests.
These checks exercise recovery and input delivery, not the cause of this
intermittent production incident. The unresolved investigation remains in
[TODO](../TODO.md).

## BUG-20261015-135638-c5bbf6

For BUG-20261015-135638-c5bbf6, all 917 bundled events, the manifest, browser
state, environment, terminal capture, lifecycle binding and native JSONL
metadata were reviewed. The first observable diagnostic divergence is snapshot
587 after information-button click 583: it still reports the CSS-hidden terminal
as visible. Resource click 701 and information click 730 confirm the active
surface; no iframe wheel evidence existed. All 26 heartbeat replies matched,
HTTP response receipts were 200, native progress continued, and the lifecycle
binding was running. The four button errors followed report-modal occlusion.
No delivery directory existed and none was created.

The private Chromium baseline reproduced the information root's `overflow:hidden`:
2853px content in a 714px viewport stayed at scrollTop 0 after mouse wheel input.
The resource pane scrolled from 0 to 426 normally. The fix gives the information
root vertical scrolling; resource layout remains unchanged. The
[content scroll suite](../tests/content_scroll_browser.py) checks real wheels,
view changes, retry, geometry, themes/zoom and persisted audit records, including
an HTTP-served old-CSS fault to verify blocked-scroll diagnostics.

## BUG-20261015-154632-c5f7ce

BUG-20261015-154632-c5f7ce: all 3539 bundled audit rows, browser state,
environment and terminal capture were inspected, then correlated with the
origin's conversation/lifecycle ledgers and native Codex records. At 15:12:14 UTC
SEND succeeded and the CLI queue was observed. Native interruptions at 15:13:01
and 15:13:07 followed; the retained receipt is `interrupted`. The native user
record at 15:15:12 has edited wording, not the original submitted text. A later
answer cannot prove consumption of that original input. Keeping this explicit,
dismissible uncertainty follows the CLI-state contract; fuzzy text matching or
an automatic timeout would discard evidence. No production receipt was edited.

The report's first observable evidence gap is that none of its snapshots
identified the bubble or its state: the initial SEND also predates the bundled
window. Thus the old bundle alone cannot prove the precise label rendered at
report time. The new paired snapshots make browser/server disagreement directly
inspectable. The native Codex browser suite exercises interruption, reload,
report submission and persisted metadata without logging message text.

## BUG-20261019-095333-0cd031

BUG-20261019-095333-0cd031: the manifest, browser state, environment, all 1,218
bundled audit rows and terminal capture were correlated with the conversation
and lifecycle ledgers, native Codex JSONL, native runtime logs and the earlier
browser audit. No delivery directory existed. The current turn began at
08:08:44 UTC; its last output was at 08:10:55, with no native ending. At
08:10:54 the browser selected Stop on a different thread, confirmed at
08:10:56; the server recorded successful `/api/session/stop` at 08:11:03.
The shared daemon restarted at 08:11:04 and the affected TUI resumed its thread
at 08:11:10. At 08:11:38 CHECK observed ready/idle. This sequence and the old
process-stop implementation support collateral daemon termination, not a
manual interruption of the reported thread. The process ownership fix already
landed in `06a7c8f9`; the interruption predates that fix's deployment.

The first retained divergence is loss of the active native turn across that
daemon restart. The SDK reducer additionally discarded `turn/completed`'s
interrupted status and painted daemon idle as 就绪; its historical fallback
also omitted the conversation view's missing-ending inference. The isolated
[SDK interruption suite](../tests/codex_sdk_interrupted_browser.py) reproduces
the reducer fault with native RPC events, without restarting production.
The bridge now retains the last observed terminal outcome, and both views use
the same CLI observation fallback. No native records or production receipts
are rewritten. At report time the terminal's first snapshot already contained
the interruption; a later three-byte browser input cannot explain its origin,
and byte-count-only auditing does not identify those keys.

## BUG-20261020-033847-a2d757

2026-10-20 的 Agy SDK 报告已读取 manifest、browser-state、environment 和全部
980 条浏览器审计事件；没有 terminal.txt 或附件。浏览器 03:28 UTC 显示 SDK 活跃，
03:31:57 已不活跃，相关 HTTP 请求成功，没有用户停止事件；ResizeObserver 通知
不能解释执行中断。该 SDK 会话不走 PTY lifecycle 账本。

原生 transcript 与镜像最后保留 03:30:04 的“等待子代理与后台任务”回复，前一步
工具仍为 RUNNING；镜像 03:30:24 更新为 IDLE。持久输入账本保留消费证据
`observed`、`stop_requested=false`，最终为 `unknown`，错误明确记录服务重启后
投递结果未确认。systemd 记录 03:30:35 停止、03:30:37 启动，随后 03:31:25
再次停止、03:31:32 启动，与部署备份时间相符。

首个已证实的跨层问题是部署在尚未确认 SDK 输入结束时重启了节点服务。
Agy bridge 的 shutdown 会结束自己持有的 Child；实时快照不跨服务重启。
现有证据不能还原原生进程的精确退出时刻/退出码，也不能把原生 IDLE 等同于
审计任务和子代理汇总完成，因此不宣称已证实 CLI 自身崩溃或 OOM。

最初止血补丁的[部署检查](deployment.md)在 Linux staging 和 backup 前检查在途原生输入，
阻止已知活跃工作被正常发布打断。临时目录的隔离 run_push 验收覆盖 sending、
accepted、staging 后新增工作、终态/排队回执、缺失/损坏账本、Hub 和 dry-run；
拒绝路径没有 swap、restart 或 rollback。部署语法、文档链接、AGENTS 检查及
deploy dry-run 通过。本次仅改部署脚本和文档，未做生产重启或付费 CLI 重放。
后续架构修复将进程和原始通道移入[独立 SDK 宿主](sdkhost.md)，具体 SDK 协议
仍在节点服务。节点 shutdown 仅断开通道，重启复接原宿主，补读输出并恢复审批与
输入去重；明确停止仍终结原进程组。普通发布不替换宿主。旧进程内任务无法原地
迁移，部署检查继续保护它们，新宿主任务允许节点重启。原生后台任务完成后的
续跑语义与节点生命周期分开验收，见 [TODO](../TODO.md)。

### Incident BUG-20260927-020547-dd0ff9

The first observed transport failure began with the input-status request at
02:04:15.597 UTC, which exceeded its five-second deadline. Repeated status
checks then timed out in both the headers and body phases; message refreshes
and the report draft read also failed. The screenshot's raw `signal is aborted
without reason` came from the draft GET's local deadline. That GET previously
had no request audit, preventing precise correlation of its failure phase.

The native session completed at 01:02:53 UTC, matching the captured terminal
and displayed answer. No new delivery for the selected session was found.
Proxy access logs showed cancelled requests (499) and an empty 200 response
from the affected Windows client during the failure window, while other
clients continued receiving responses. Requests subsequently recovered and
the report was submitted successfully. These observations establish an
interrupted client request path, but do not distinguish browser, client network
or intermediary transport failure; they do not establish a node outage.

The fix makes the draft timeout explicit and adds missing transport evidence.
It does not claim to fix the unlocalized interruption. The Hub/node Chromium
acceptance test stalls the initial draft read through its real deadline, types
after failure, verifies automatic recovery without creating a worker, and
then submits once with exact attachment bytes.

## BUG-20261018-040613-165ffa

BUG-20261018-040613-165ffa: the browser sent the opening request 8 ms after
clicking and received its parsed body after 5902.6 ms, then rendered within
9 ms. The matching node trace recorded 5830 ms in the previously combined
read phase, 0 ms retiring echoes and 4 ms probing the prompt. The native turn
had completed before the click, and the lifecycle binding still named its
running instance; no delivery directory existed. A synthetic five-screenshot
Chromium open took approximately 0.4 seconds and did not reproduce the stall.
Follow-up phase logs captured 1519–2271 ms in snapshots with no admission delay.
The collector returned 71.8 MB / 103k boot-long connection records: liveness,
catalog acknowledgements and coordinator reads expanded them into a generic JSON
tree, cloned it and converted it again. Typed decoding on blocking workers and
bindings-only decoding for liveness remove those unnecessary allocations; the
coordinator still receives full evidence. In the synthetic 92 MB / 100k-record
Chromium regression, the old release peaked at 8776 MiB and cold opens took
468–1838 ms; the fixed release peaked at 285 MiB and took 34–343 ms.
This reproduces and fixes contention under collector load; the old combined
trace cannot retrospectively assign every millisecond of its 5830 ms stall.
See [collector regression](../tests/history_collector_browser.py).

## 同节点克隆原生验收

`session_clone_service_real.py`（已于 2026-10-06 随非浏览器测试删除）当时直接调用生产接口，在临时 home 使用 Codex 0.159.0、
`gpt-5.6-luna` low，创建真实分页分支、revert、归档兄弟和 code-mode 子代理。验证完整分页
内容、原生列表、当前 rollout、名称/置顶/项目关联，随后分别继续克隆分支、克隆子代理和
原分支，确认新旧互不追加。日常配置保持原样。该脚本只在显式直接调用或真实 CLI 验证时运行。

本节结果验证同节点 Codex；跨节点与其他 CLI 的独立证据见[迁移验收记录](#会话迁移交付验收2026-10-01)，
不能从此单项结果推导任意跨版本兼容。

## 跨节点迁移原生验收

`session_clone_service_real.py --production-peer <SSH 别名>`（已于 2026-10-06 随非浏览器测试删除）当时从真实 Codex 的可见孙分支经 Chromium
迁移整个五会话组；加 `--new-ids` 验证身份重写。Codex 0.159.2 / Luna low 实测包含分页 fork、
归档兄弟、revert 多代 rollout 和 code-mode 子代理，两种身份模式均通过目标原生列表、完整
分页历史、当前文件、名称/置顶/项目/归档元数据及分支/子代理续聊检查。目标 CLI 自行初始化
临时数据库，生产导入不调用测试 SQL 导入器；两端无关会话和日常默认配置保持不变，源端会话行
与历史清理完成。该验证不代表跨版本或所有工具/附件格式已经完成；运行回执使用下述合成浏览器验证。

## 会话迁移交付验收（2026-10-01）

## 交付验收

以下是 2026-10-01 收尾时核对的验收结果。浏览器套件使用临时合成会话和真实节点/Hub，
实际点击入口、选择操作、确认并打开目标历史；原生测试另使用隔离 home 和规定的低成本模型。
未用生产会话做测试。早期章节中的单项实验限制只描述当时样本，不替代本表后续证据。

本表保留当次验收回执。当前 `session_mixed_clone_browser.py` 的损坏暂存样例会在发布前
返回 `move_plan_stale` 并撤回旧计划，不再进入历史版本所测的发布后补偿分支；
现行套件覆盖范围见上面的[验证](session-move.md#验证)，发布后故障与恢复继续由相应 transfer 套件覆盖。

| 要求 | 已核对证据与结果 |
|---|---|
| 同机整组复制，Codex 分页 fork、多代 rollout、归档和 code-mode 子代理 | [session_clone_browser.py](../tests/session_clone_browser.py) 页面操作通过；`session_clone_service_real.py`（已于 2026-10-06 随非浏览器测试删除） 原生完整分页、列表、当前版本、元数据及新旧组分别续聊通过 |
| Claude/Grok 分支、子代理、跨父会话代理引用 | [session_files_clone_browser.py](../tests/session_files_clone_browser.py) 页面整组复制、历史打开、源文件不变、失败补偿和重启重试通过；最后一次包含 Grok 检查点依赖，共 9 项 PASS |
| 混合来源连通组、同 SID 不同来源、跨来源归属 | [session_mixed_clone_browser.py](../tests/session_mixed_clone_browser.py) 十四会话组、发布失败补偿和重启重试通过 |
| 跨机移动/复制 × 保留/改变身份 | [session_mixed_bundle_browser.py](../tests/session_mixed_bundle_browser.py) 在两台主机的独立临时存储完成四种组合；目标三种历史与关系、源端保留/清理、Codex 同 SID 两代 rollout 均通过 |
| 真实复杂会话迁移后恢复 | [session_native_transfer.py](../tests/session_native_transfer.py) 的原生迁移路径：Codex 五会话组、Claude/Grok 各三会话组，分别完成保留与重写身份；原生恢复与续聊、源端清理、无关会话保留、日常配置不变通过。Claude 为跨机往返后在最终目标续聊 |
| 目标已有副本、前缀扩展和失败恢复 | [session_prefix_browser.py](../tests/session_prefix_browser.py) 三种来源复制/移动及文件、原生行、显示设置恢复通过；[session_bundle_browser.py](../tests/session_bundle_browser.py) 相同目标复用与续写后重试不覆盖通过 |
| 停写、交接、清理与中断恢复 | [session_bundle_browser.py](../tests/session_bundle_browser.py) 覆盖源锁、部分清理、新外部引用、移回、旧回执、草稿别名、Hub/节点重启；保留和重写身份的移动均通过 |
| cwd 存在、外部附件及共享存储 | 同一双机套件 `--dependencies` 覆盖三种来源外部图片/结果/链接的缺失与异内容；独立本地存储和共享 NFS 探针实测通过；共享会话存储拒绝移动，不执行源端删除 |
| 原生 goal、附件关联、实时历史 | [session_clone_browser.py](../tests/session_clone_browser.py) 结构与映射验证通过；[session_goals_native_browser.py](../tests/session_goals_native_browser.py) 原生读取 goal/附件并独立修改克隆通过。实时连接不迁移 |
| Claude 原生文件回退 | [session_claude_rewind_browser.py](../tests/session_claude_rewind_browser.py) 原生 Edit、浏览器复制、新检查点回退及文件原字节恢复通过 |
| Grok 原生压缩检查点 | [session_grok_checkpoint_browser.py](../tests/session_grok_checkpoint_browser.py) Grok 1.0.44 原生压缩、浏览器复制、加载、回退和续聊读回标记通过；源文件和日常配置不变 |
| 目标 CLI 与动态工具提示 | [session_transfer_environment_browser.py](../tests/session_transfer_environment_browser.py) 缺 CLI、版本较旧/未知、切换目标后迟到响应及执行器未核验提示通过 |
| 步骤内部实时计数与进度条 | [session_large_manifest_browser.py](../tests/session_large_manifest_browser.py) 的 `--progress --peer` 模式，真实跨机移动、关联大历史与超 64 MiB 原生清单；记录页面更新间隔并核对源清理、目标原生数据 |
| 表格、机器与身份选择、禁用提示、窄屏及任务进度 | [session_clone_browser.py](../tests/session_clone_browser.py) 控件与 tooltip 通过；双机套件覆盖持久进度、刷新恢复和窄屏入口 |
| 交付 | 实现及测试已提交、推送；官方 `deploy --all` 已更新六个在线应用目标并通过健康检查，现有 ptyhost 保留。Cetus 离线跳过，未宣称该目标已更新 |

原生 Codex 证据覆盖 0.159.0 的克隆与 0.159.2 的复杂迁移；Grok 检查点证据为 1.0.44。
这些结果不构成所有 CLI 版本组合的兼容承诺。导入按实际原生 schema 核对，不创建或升级
CLI 数据库，不覆盖目标整库；不匹配时保留现场并报错。动态工具定义可迁移，但执行器、
认证及外部服务仍属于目标环境，页面会明确标为未核验。第一版 Linux、相同 CLI 根和 cwd
范围，以及不迁移进程/PTY、不复制工作目录的边界保持不变。

## Grok 文件集原生隔离实验

`session_files_grok_real.py`（已于 2026-10-06 随非浏览器测试删除）曾是显式运行的原生隔离实验：使用 `grok-4.6` low
创建父会话、fork 和子代理，将暂存输出实验性导入同一个临时 home，验证克隆后的父会话及
子代理按新 ID 续聊，再继续原组并核对两组互不追加。断言原生 chat 记录中的实际模型
`grok-4.6-build` 和 low 强度、原生关系以及日常配置不变。结果写入
`target/session-files-grok-report.json`；不代表生产发布或跨机验收通过。

## Agy 原生删除与索引回写

本次现场在原生删除成功后约 56 秒重新出现旧索引行，但数据库与正文仍不存在。
已确认异常从原生索引重新出现开始；未确定具体哪个 Agy 进程回写。
真实隔离双 CLI 保持另一会话运行并继续发送，观察 70 秒未重现该回写；常规浏览器
测试显式重放旧索引重新出现，验证刷新、服务重启、空壳删除与实际原生载体恢复。

真实 Agy 1.4.0、1.4.2 在私有 HOME、合成 loopback 模型网关中验证上述路径；常规浏览器测试
使用假 CLI，覆盖节点、Hub、批量、取消、身份不符与原生失败。更早版本、修改过的
快捷键和未知菜单尚未获得本次实机验收，不绕过原生确认。移动、克隆及整树删除仍不
支持，整组操作保留阻碍项，不能忽略 Agy 成员或仅迁移文本镜像。

## BUG-20260926-025909-1ee07a

BUG-20260926-025909-1ee07a exposed the missing OPEN-socket liveness check:
node output receipts continued while the browser had an approximately 14-second
receive gap, and browser inputs later arrived at the node in a burst with
negligible host-write latency. Other page traffic and native session work
continued. The bundle places the stall in the terminal transport but does not
identify which network/proxy hop caused it.

## BUG-20261015-030106-0869b4

BUG-20261015-030106-0869b4: browser history finished rendering at 03:00:43.152,
but the first input CHECK started at 03:00:45.791 and took only 160.6 ms.
The Hub recorded 4012 ms for the initial `/api/term/list`. The running lifecycle
record already had a confirmed native binding, and the captured terminal and
native JSONL showed ongoing work. The first mismatch was making this one
conversation wait for whole-fleet discovery. The available logs do not identify
which upstream caused that particular aggregation delay. No delivery directory
was present in that deployment; opening the conversation did not submit input.

## BUG-20261018-012536-a4c05c

BUG-20261018-012536-a4c05c: the earlier report's worker started at 01:14:59 UTC
and completed SEND at 01:15:04. Its native Codex history existed, but the
lifecycle receipt retained `binding: null`. At 01:25:03 the browser opened its
same-title pending row, still labelled “等待首条消息”, then at 01:25:08 opened
the separate native history. The rollout writer was the shared app-server,
outside the TUI ancestry required by process autobinding. Naming had already
confirmed the guarded TUI's full `/status` SID but discarded it. The
[binding recovery](lifecycle-binding.md) now preserves that identity and also
recovers old submitted workers whose status card has disappeared. No duplicate
worker launch or duplicate task SEND was found.

## BUG-20261013-154315-ac4fbd

BUG-20261013-154315-ac4fbd exposed a Grok registry entry whose PID was gone
and whose managed receipt was already Exited. The old unconditional hint made
cleanup repeatedly report running although there was no process to stop.

## BUG-20261018-050342-5d4448

The report's native event at 05:02:55 UTC contained the structured questions,
while the SDK item-to-message conversion discarded them; its terminal capture
showed one queued question. Browser audit had no SDK reply attempt.

## BUG-20261014-124811-809df3

BUG-20261014-124811-809df3：两次异常断联对应 Hub 在 12:40:50 / 12:47:44 UTC
达到 2 GiB cgroup 上限后被 OOM killer 终止；节点 PTY 与原生会话继续运行。
浏览器 WebSocket 1006、SSE 错误与 HTTP 502 同时出现，节点笼统的 1009 收帧错误
是上游骤断后的记录，不能据此认定用户发送了超大输入。另一次 4001 是显式接管。

## BUG-20261006-072955-02f968

BUG-20261006-072955-02f968 exposed this distinction: native command results
confirmed GPU workloads, while live collector samples contained their device
UUIDs but no session bindings. The jobs used short SSH launches of detached
workers; their launch clients had exited before diagnosis. Polling can miss
that connection evidence, and historical prose cannot safely recreate it.
This correction preserves the observed attribution gap in server aggregation;
it does not retroactively restore missing launch links. The event-driven connection
mechanism described in [process-links.md](process-links.md) closes the short-client capture gap for new observed
launches; it cannot recreate already-missing historical evidence.

## BUG-20261007-083559-acc010

BUG-20261007-083559-acc010 exposed a local CLI spawner that was present in
attribution but absent from `launch_chain`: child GPU samples existed, while
the parent's inclusive view omitted them. The process launch-chain correction fixes the
causal chain, preserving the sidebar's direct ownership scope. The accompanying
[historical launch audit](process-launch-audit.md) covers other program launches
and resource-release boundaries.

## BUG-20261008-120649-69fc59

BUG-20261008-120649-69fc59 的终端、原生历史和账本绑定一致，首个偏差是紧凑目录缺少
子会话时，浏览器把已绑定的启动重新投影成等待行，点击后却打开原生历史。

## Agy 编辑区与自定义状态栏识别

BUG-20261005-150140-776952：后台命令已在原生记录中进入 RUNNING，终端仍有空编辑区，
旧识别器因下方超过一行而使 CHECK 返回 409。任务栏现参与同一 CHECK/SEND 识别，
并令 `instance.busy` 为 true；任意正文、缺失分隔线及框外光标仍不恢复编辑区。
[菜单浏览器套件](../tests/cli_menus_browser.py) 用私有假 CLI 重放该布局，覆盖网页发送、
终端已有草稿、任务结束及菜单拒发。
BUG-20261006-041817-f7cf23：原生最后一条回复已 DONE、宿主仍 running，报告终端保留
完整空编辑框及默认页脚，其下多出自定义额度栏。浏览器恢复会话后的首个 CHECK
（2026-10-06 04:07:37 UTC）返回 409；旧识别器把所有多行页脚当作后台任务面板，
导致编辑区变成 unknown。现在从任务行结构识别面板，再独立定位默认页脚；其后的
自定义文本不参与输入正文或 busy 判断。同一浏览器套件覆盖叠加状态栏的 CHECK、
实际点击 SEND、忙碌/后台任务、原生草稿和框外光标；不修改原生会话或状态栏脚本。

## 前端清单来源记录

`grok-4.7 high headless 产出，人工审阅`

## BUG-20261024-022003-6dda5d：长回合回放跨越历史缺口

历史记录：诊断包的 browser-state 为 SDK 视图；审计 6106 在刷新时收到 605 条
窗口消息，之后快照 DOM 只有 8 条消息。用户图中缺口为 3,507 条，但后面出现
很早的“不是，你要进行系统复核”。来源原生 JSONL 的该输入写于
2026-10-23 17:38:04 UTC，所属回合一直进行中；SDK 快照仍从该输入回放整个
回合。原生历史窗口的尾部则已在数小时之后。投递队列没有这条旧输入，生命周期
账本没有对应记录，诊断包没有 terminal.txt；没有重发或修改生产数据的证据。
首个错误在前端拼接：按 turn_id 截掉窗口尾部，再把整个回合回放接在历史缺口后。

修正保留原生 item 身份，按加载尾部的重叠 item 衔接；无重叠保留原生历史，
补页后恢复完整回合且维持实时检查点。合成 Chromium 用约 4 MiB 的长回合、
重复回复文字和不同 SDK/原生工具身份复现；原前端在旧输入越过缺口的断言失败，
修复前端在节点及 Hub、1440 深色及 390 浅色通过，含流式追加、无重叠回放、
补页、顺序与刷新。执行者亲自检查缺口/补齐后的整页截图：缺口后是近期过程，
补齐后旧输入回到早期位置；气泡、工具分组、留白和移动端换行沿用现有样式。
历史、历史分页、Codex 过程折叠与文档检查通过；SDK 用量检查的独立失败见 TODO。
