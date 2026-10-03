# SessionDock TODO

这里只记录当前尚未完成或尚未决定的工作。当前行为以代码、测试和 `docs/` 下的
合同为准。

## Runtime and platform coverage

- [ ] 扩展公共进程归属层的平台覆盖与完整计量：Linux `resource-agent` 已提供
  生命周期事件、CPU/PSS/GPU/proc storage 指标及可选 60 秒 VFS/TCP/NFS 诊断。
  macOS/Windows 采集器、短命任务无遗漏的累计账本仍待实现；连接复用、跳板、NAT
  的完整关联需要额外证据，不能将部分观测视为完整计费。
  现行接口与后续计量口径见 [process-links.md](docs/process-links.md)。

- [ ] 为 Windows/macOS 的外部（非 ptyhost 管理）CLI 补齐进程发现与强身份验证。
  Windows 的受管 ptyhost 路径已经过实机验证，不应与此外部进程缺口混为一谈。
- [ ] 在 macOS 实机验证 sessiondock、ptyhost、文件原子替换、进程身份和终端生命周期；
  Linux 测试或 MSVC 交叉编译不能代替该验证。
- [ ] 为外部会话实现原生 rename；现有历史中的 `/rename` 展示不等于进程控制操作。

## Native interaction semantics

- [ ] 决定并实现真正的 native rewind/rollback。现有 timeline pin 只改变 SessionDock
  的展示视图，不改 CLI 原生历史，也不向 CLI 发送回滚动作。
- [ ] 若仍需要 activity stop 覆盖，先定义原生确认和退役语义，再接入读模型；不得仅凭
  HTTP 或终端写入成功声明完成。

## Operations and release engineering

- [ ] 评估是否仍需独立合成语料统计工具，以及由源码生成的预算参考表；仅维护
  仍有效的性能/协议数字，不恢复已删除的输入拒绝规则。

- [ ] 增加服务端请求/响应 tracing 与结构化日志，同时保持凭据、正文和原生记录默认不落盘。
- [ ] 产出可复现发布包和 Linux/Windows/macOS CI 矩阵；Windows OpenSSH 原生构建遵循
  `docs/deploy-windows.md`。
- [ ] 将真实 Claude/Codex/Grok CLI 套件纳入明确的发布验收步骤；继续使用临时配置和
  `AGENTS.md` 规定的低成本测试模型，不进入普通 `cargo test`。

## Frontend direction

- [ ] 为全部展开的大侧栏增加可见区渲染，并保持分组多选、深链定位和文本选择语义。
  当前已复用未变化行、按需创建折叠组；全部展开时 DOM 数量仍随会话数增长。
- [ ] 将 Grid 历史折行改为逻辑行惰性计算或可取消分批计算，同时保持选择坐标和完整历史。
  当前拖动已合并、临时 cell 不常驻，但最后一次变窄仍同步遍历历史。

### Vue 重构计划（2026-10-03）

用户已选择分阶段迁移到现有 `web/` 的 Vue 3 / TypeScript / Pinia / Vite。
本节只登记未完成的工作；完成批次从这里移除，当前实现和审阅结果写入相应合同。

2026-10-03 最新指令：设立持续 goal，完成全部重构；生产环境暂不切换引用。
以已发布的 `2552436` 为生产基线，后续使用独立 Vue 入口和构建目录，在临时
服务上验证。完成后的源码提交、推送，但不部署或修改生产资源/入口引用。
保留生产所需的 `legacy-web/`；B8 清理的是新入口中的旧业务渲染与过渡接线，
生产入口目录的最终删除/切换留待用户另行明确要求。

#### 固定范围

- 功能原样搬运：以开始迁移时的生产 `legacy-web/` 代码及现行 `docs/` 合同为准，
  包括默认值、缺失字段降级、能力开关、错误说明、键盘、触摸和偏好持久化。
  不借重构增加、删除功能或修复无关问题。
- 界面保持一致：复用原 CSS、图标、字体、文案、布局、断点和交互状态；
  不为像素级一致增加测量、补偿、兼容分支或截图基线系统。
- 不新增产品检查、安全机制、输入拒绝、容量限制、权限、确认步骤或后台探测。
  已有机制原样保留，不因迁移改变其语义。
- 不新增或运行 unit test。验证采用实际 Chromium 页面操作；优先复用已有
  `*_browser.py`，仅补迁移后尚无覆盖的用户操作。不以函数断言、静态快照替代操作。
- Rust API、发送、授权、原生记录和 ptyhost 协议保持现行合同；前端转换不创造新协议。
  不修改冻结参考目录、其它项目或日常 CLI 默认模型。

#### 目标边界

- `web/src/api/`：现行 DTO 和传输适配；只搬运已有错误处理，不新增校验体系。
- `web/src/domain/`：从现有实现提取列表增量、历史游标、消息合并等纯逻辑。
- `web/src/services/`：连接、任务和缓存的生命周期；终端字节、Grid、WebSocket
  实例留在这里，不放进深层响应式对象。
- `web/src/stores/`：按列表、选中会话、搜索、偏好和机器拆分 UI 状态，避免复制全局 `S`。
- `web/src/components/`：组件渲染和用户事件，不承接同步、发送确认或协议解释。
- 过渡接线只提供每一批确实需要的读取和操作。每份状态和每块 DOM 只有一个所有者。
  替换完成即删除对应旧渲染/事件绑定，过渡接线随模块迁移删除，不建设通用迁移框架。
- 构建产生静态资源，由现有 Rust 静态服务发布，Node 只在构建时运行。
  生成产物不提交；生产发布继续使用现有 fleet deploy 工具。

#### 执行分工

| 难度/范围 | 执行会话 | 主审职责 |
| --- | --- | --- |
| 中高：多文件迁移、同步状态、构建发布、终端生命周期 | Sol 6.1 medium（`gpt-6.1-sol`） | 审阅边界、语义、旧代码删除和集成；由主审验证、提交、推送；生产切换暂缓 |
| 边界明确的独立文档、夹具或用户操作脚本 | Grok 4.7 high（`grok-4.7`） | 对照实际代码审阅，记录实际模型与人工审阅结果；不委派发送、授权、原生语义或 ptyhost 协议 |

每个 exec 会话有任务书、独占文件范围、日志、超时、退出状态和产出说明。
子会话不提交、不推送、不部署、不再分派；并行任务不修改同一个文件。
模型和 effort 显式传参，不允许替代模型或修改默认配置。

#### 批次和依赖

B0 构建接线和 B1 外观/功能设置试点已实现并通过 Chromium 验收，
当前接线见 [架构合同](docs/architecture.md)。以下保留待完成的批次。
逐项功能与现有操作锚点见 [迁移行为基线](docs/frontend-migration-surfaces.md)。

| 批次 | 工作包与交付 | 执行/难度 | 依赖 | Chromium 用户路径与完成标准 |
| --- | --- | --- | --- | --- |
| B2 页面框架与机器 | 顶栏、手机列表/详情切换、更多菜单、弹窗外壳；迁移机器设置、排序、客户端矩阵和既有更新操作；删除 B1 相应接线 | Sol / 中高；Grok 独立操作脚本 | B1 | `tests/header_fold_browser.py`、`tests/session_deep_link_browser.py`、`tests/client_update_browser.py`、`tests/hub_browser.py`；原入口、操作、文案和断点一致 |
| B3 列表和导航 | 提取列表增量、树/平铺分组、机器/客户端筛选、星标、多选、未读、排序、附属关系和主/子会话深链；Vue 接管侧栏 | Sol / 高；Grok 独立夹具/操作脚本 | B2 | `tests/nest_tree_browser.py`、`tests/session_titles_browser.py`、`tests/session_resources_browser.py` 及对应现有多选/未读/分组套件；刷新、跳转、增量后选中和展开状态不变 |
| B4 搜索 | 搬运字面/正则、大小写、全词、AND/OR、命中跳转和原错误说明；列表过滤与完整搜索保持现行区别 | Sol / 中高 | B3 | `tests/search_no_fold_browser.py`、`tests/search_browser.py` 及现有搜索套件；输入、切换选项、打开结果、上下命中跳转 |
| B5 历史与消息 | 提取 SSE/增量/缓存/分页状态；组件化文本、Markdown、公式、语法、工具组、差异、媒体、耗时、压缩/回退/子代理；保留锚点和滚动位置 | Sol / 高；Grok 独立媒体/工具操作脚本 | B3、B4 | `tests/history_browser.py`、`tests/tool_group_fold_browser.py`、`tests/media_continuation_browser.py`、`tests/conversation_performance_browser.py` 等；补历史不移动实时 checkpoint，增量不重置折叠/选择/阅读位置 |
| B6 输入与会话操作 | 搬运草稿、输入历史、CHECK/SEND 展示、问题/审批卡片、新建、停止、重命名、移动/复制/回收站和反馈入口；服务端逻辑不变 | Sol / 高；主审负责语义裁决 | B5 | `tests/draft_sync_browser.py`、`tests/send_browser.py`、`tests/question_browser.py`、`tests/new_session_model_browser.py`、`tests/session_transfer_environment_browser.py` 及对应现有套件；实际输入/提交/取消/重开，对应请求和可见结果不变 |
| B7 终端和辅助页 | 提取终端管理模块；Vue 只管理容器和工具栏，复用 Grid 与 xterm；保持视图切换、连接、租约、恢复、键鼠/触摸/粘贴、录制回放；迁移现有 Grid/录制/兼容文件页面 | Sol / 高；Grok 仅独立页面走查 | B6 | `tests/terminal_input_browser.py`、`tests/terminal_grid_browser.py`、`tests/terminal_records_browser.py` 等；切会话不误断连接，关闭/重连和既有抢占提示保持原行为 |
| B8 完成替换准备 | 独立 Vue 构建可作为完整入口；新入口删除旧业务渲染、全局状态和过渡接线，保留继续使用的 Grid/vendor/font/PWA 静态资产；更新当前合同 | Sol / 高；主审集成 | B7 | 对全部迁移表面做浏览器验收、子路径访问和临时服务重启后恢复；完整产物已构建、提交和推送；生产仍引用当前版本 |

表中的套件是工作包定位依据；每批开工前核对当前存在的文件和覆盖范围。
若名称不存在，选覆盖同一操作的已有浏览器套件；只补缺失的用户操作。
历史迁移同时按仓库规定运行 `tests/history_parity.py`；不改 oracle 或新增拒绝策略。
侧栏可见区渲染、Grid 惰性折行等上方独立性能 TODO 不并入本次功能原样搬运。

#### 每批审阅和发布

1. 开工前列出该批实际控件、事件、默认值、存储键、请求和已有可见状态；
   对照源码与对应合同，不据旧文档猜测行为。
2. exec 会话只完成被分配的工作包；依赖未合入的包先做独立部分，不先写占位产品功能。
3. 主审逐文件读 diff，确认功能无增减、CSS 复用、DOM/状态单一所有者、无新增产品检查，
   以及旧事件绑定和渲染确实退出。
4. 编辑完成后构建，在私有临时数据和 loopback 服务上运行覆盖该批的 Chromium 操作。
   不运行 unit test，不接触生产会话；结果写明真正运行的套件和命令。
5. 文档改动运行 `python3 tests/check_docs_links.py`；部署接线用实际构建产物启动
   私有服务并操作页面验证，不用 mock 命令断言代替实际构建/页面操作。
6. 只暂存本批文件，主审提交并从有凭据的机器推送。按最新用户指令，
   本 goal 内不运行生产部署、不重启生产服务、不修改生产入口或资源引用。
   最终记录完整可切换产物及生产仍使用原版本的状态。

#### 下一批派发

- [ ] B6 输入区已合并并通过十套 Chromium 操作；B5 消息区和 B7 主终端
  已交回，待主审集成；B6 会话标题栏/操作窗口及辅助浮层正在迁移；
  独占模块和集成副本避免并行覆盖，主审负责逐批审阅和浏览器验收。
- [ ] B7 的独立网格、录制和文件页已完成 Vue 迁移，三套实际 Chromium 操作通过；
  主页面终端及工具栏仍需迁移。
- [ ] B3–B8：按依赖顺序细化独占文件任务书，每个完成批次审阅、验证、发布后再推进。
