# 现有前端用户可见功能清单

`grok-4.7 high headless 产出，人工审阅`

这是迁移开始时的行为基线，只盘点当时用户能看见、能操作的行为，当时已发布提交为 `2552436`。生产前端仍引用 `legacy-web/`，本目标不切换生产入口；其它并行任务已接受的修复也同步到迁移入口。独立迁移前端的当前实现见 [架构合同](architecture.md)，本文不把尚未验收的脚手架或过渡接线写成产品表面。

`legacy-web/framework/settings.js` 只把「外观」「功能」两个设置页挂进现有对话框（`SessionDockSettings.mount`）。主会话页、侧栏、对话、终端和辅助页仍是现有 DOM。`tests/frontend_framework_browser.py` 覆盖的是这两页设置在现有页面里还能用，不是整站 Vue 替换。

浏览器套件是定位既有操作的锚点，不是完整覆盖统计。生产行为以列出的实际代码为准。

相对链接都指向当前仓库里已有的文件。

## 怎么用这份清单

按批对照时，每一节都给出：用户看到什么、默认是什么、出错或不可用时页面怎么说、关键函数、相关存储键或 API、已有的 `tests/*browser.py`。函数名是锚点，不是新接口。

能力开关来自页面上的 `<meta name="sessiondock-capabilities">`，解析在 [`legacy-web/capabilities.js`](../legacy-web/capabilities.js)。没有这张标签时 `declared:false`，行为保持放开，存储前缀仍是 `sessiondock.`。标签存在但 JSON 不是对象时失败关闭：`read_only`、关掉 live/outbox/audit/search/files，并显示「能力配置无效，请检查服务配置。」（`#backend-notice`，[`app.js` `configuration_error` 分支](../legacy-web/app.js)）。健康页面没有常驻横幅。`allows(name)` 的含义是 `config[name] !== false`，缺键仍允许。完整旗标表见 [`docs/capabilities.md`](capabilities.md)。

存储前缀：节点 `sessiondock.`；Hub 为能力里的 `storage_namespace`，缺省时页面用 `sessiondock.hub.` 加上当前 `location.pathname`。读写只使用 `sessiondock.*`。Hub 模式由 `<meta name="sessiondock-mode">` 为 `hub` 决定。

---

## 1. 页面壳、设置、机器

主页面是 [`legacy-web/index.html`](../legacy-web/index.html)。标题是主机名加「会话管理」。语言 `zh-CN`。视口带 `interactive-widget=resizes-content`。首屏样式加载前就根据 `theme` 设置 `documentElement.dataset.theme`，避免闪色。窄屏（≤720px）若 URL 带 `sid`，或上次 `mobilePage === 'detail'` 且有 `sel`，则 `data-boot-page=detail`，直接从会话页起步；列表恢复失败再退回列表（`leaveBootDetail`）。

### 顶栏

从左到右：

| 控件 | 行为 |
| --- | --- |
| `#side-toggle` | 收起/展开左栏。`setSideCollapsed`，键 `sideCollapsed`，默认展开 |
| `#session-scope` | 「只显示活跃会话」/「显示全部会话」。计数在 `#session-active`、`#session-total`。`selectSessionScope`；`activeOnly` 默认关，且 `live` 能力为 false 时不按空集合筛选 |
| `#sidebar-resources-toggle` | 见第 2 节资源列 |
| `#nest-toggle` | 分层显示，默认关。`S.nest` |
| `#view` | 项目树 `tree`（默认）、会话分组 `group`、时间轴 `date`。`renderView` |
| `#node-chips` | 仅 Hub。见第 2 节 |
| `#chips` | Claude / Codex / Grok / OpenCode / SSH 筛选 |
| `#new-session` | 新建。`terminal_create` 未开时保持隐藏 |
| `#page-reload` | 刷新页面。独立应用模式等条件下才出现，见 `syncPageReload` |
| `#transfer-tasks` | 未完成的移动与复制，带计数。见第 5 节 |
| `#trash` | 回收站 |
| `#report-bug` | 报告问题 |
| `#settings` | 设置 |
| `#header-more` | 空间不够时，右侧按钮从末尾折进「更多操作」 |

`layoutHeader` 的折叠顺序：先收 Agent / 组织方式 / 分层的文字标签，再收主机标题，再把机器 chip 收成首字母（撞车用前两个字母，[`nodeAbbrs`](../legacy-web/nodes.js)），最后把右侧按钮折进 ⋯。放得下就按相反顺序展开。隐藏的能力按钮不会被折进菜单。窄屏、中屏（≤1199）、宽屏三级，与 `style.css` 断点一致（`layoutTier`）。

左栏被收起，或手机正打开会话详情时，新建、设置，以及独立应用下的刷新，会挪到会话标题栏。展开列表后回到顶栏。[`tests/session_global_actions_browser.py`](../tests/session_global_actions_browser.py)。

空详情：「从左侧选择一个会话」。加载中用 `#prog` 进度条（`progress` / `progressDone`）。

### 外观

设置对话框 `#settings-dialog`，三个标签：外观、功能、机器。上次标签在 `settingsTab`，默认 `appearance`（`showSettingsTab` / `openSettings`）。副标题：「界面偏好保存在浏览器」。

外观（`applyTheme`、`applyFont`、`applyInterfaceScale`，以及 Vue 挂载的 `SettingsAppearance`）：

- 界面缩放 `interfaceScale`：50–150，步长 1，默认 100。滑块、重置、主页面双指捏合共用同一数值。旧的 30–49 读出来按 50。捏合开始派发 `sessiondock-pinch-start`，取消进行中的终端选区且不复制。单指滚动保持浏览器原生。`#app` 使用 `touch-action: pan-x pan-y`。带 `data-pinch-owner` 的区域页面不接管。浏览器自己的页面缩放（`visualViewport.scale !== 1`）不当成键盘，也不因此改 PTY 行列。手势期间 `#scale-indicator` 显示百分比，松手后淡出，不挡输入。设置对话框尺寸不跟着滑块变。
- 等宽字体 `font`：`ubuntu`（默认，Ubuntu Sans Mono）、`cascadia`、`system`、`consolas`。终端和工具输出共用。字体文件在 `legacy-web/fonts/`。`typography.js` 的 `apply` 在样式表之后执行。
- 颜色 `theme`：`system`（默认，跟随 `prefers-color-scheme`）、`light`、`dark`。系统主题变化时若选择仍是 `system`，`applyTheme('system')` 再刷一次。浅色终端会改写 ANSI/OSC 颜色，见第 6 节。
- 「安装到桌面」按钮。见第 8 节。

### 功能

- 自动休眠 `sleepMinutes`：0、5、15、30、60（默认）、120、240。见第 8 节。
- 历史缓存 `cacheMb`：64 / 128 / 256（默认）/ 512 / 1024 / 0（不限制）。当前打开的会话固定驻留，不计入上限。改完立刻 `trimCache`。
- 停止并发 `stopConcurrency`：1、2、4、6（默认）、8、12、16。非法值回到 6。下一次批量停止才采样，不改已经在跑的一批（`sessionStopConcurrency`）。
- 控制台粘贴文件 `consolePasteFiles`：默认关。见第 6 节。

[`tests/frontend_framework_browser.py`](../tests/frontend_framework_browser.py)、[`tests/prefs_migration_browser.py`](../tests/prefs_migration_browser.py)、[`tests/metadata_browser.py`](../tests/metadata_browser.py)、[`tests/page_sleep_browser.py`](../tests/page_sleep_browser.py)。双指捏合的触摸路由在能力说明里有 Chromium 触摸仿真；完整手势覆盖待逐项核对。

### 机器

机器名称、颜色、启用、顺序和控制台渲染存在中央服务端，不进本机 `localStorage`。单机没有注册表，「本机」的渲染器存在 `consoleRenderer`（默认 `grid`）。接入和移除机器仍是服务器操作，页面不提供。

Hub 每一行（`machineRow` / `renderMachineSettings`）：

- 拖动手柄或 ↑↓ 调顺序，`POST api/nodes/order`（`saveMachineOrder`）。筛选条和新建会话下拉都按这个顺序。
- 勾选启用。取消后这台机器不显示、不检查。`saveMachine`。
- 名称输入，最长 80，回车失焦后保存。
- 颜色：默认、蓝、紫、琥珀、青、玫红、青柠、天蓝、品红（`MACHINE_COLORS`）。
- 控制台渲染：`服务端网格（默认）` 或 `xterm.js（浏览器解析）`。重新打开控制台后生效。旧宿主报 `grid:false` 时自动用 xterm。停用时下拉不可用；离线时悬停说明原因，保留中央展示属性的现有修改行为。

AI 客户端矩阵（`renderClientMatrix` / `loadMachineClients` / `updateMachineClient`）：机器 × Claude/Codex/Grok/OpenCode。单元格显示版本、是否最新，以及「更新」。更新走该机器自己的 `api/clients/update`，页面每 2 秒轮询到结束。首次读取失败显示离线单元格，错误保留在悬停提示；更新失败显示在 `#machine-note`。

[`tests/client_update_browser.py`](../tests/client_update_browser.py) 覆盖版本矩阵和手动更新。[`tests/hub_browser.py`](../tests/hub_browser.py) 覆盖改名、启用/停用、拖动及方向键排序。调色板和保存失败回退另做迁移验收。

Hub 顶栏另有机器 chip，见第 2 节。节点离线、列表/运行状态/终端列表失败时，chip 上有原因（`nodeChipReason`、`nodeOfflineReason`），不弹自定义浮层。

### 登录环境提示

后端比对登录 shell 环境变化后，页面按机器列一张表（`renderShellEnvNotice`）：变量名、重启、忽略，以及「全部重启」。重启 `POST api/shell-env/restart`，最多等约 120 秒看启动时间是否变化。忽略只对当前页（`shellEnvIgnored`），不写入存储。[`tests/shell_env_browser.py`](../tests/shell_env_browser.py) 覆盖这张表的忽略、重启和 Hub 操作。

### 弹窗外观

`appAlert` / `appConfirm`（[`legacy-web/popup.js`](../legacy-web/popup.js)）用与新建会话、回收站相同的居中 `dialog.app-dialog`。消息含空行时第一行当标题。Esc 或 × 等于取消。浮动卡（版本更新、登录环境、操作状态）进 `#float-stack`，不抢焦点。[`tests/popup_browser.py`](../tests/popup_browser.py)。

---

## 2. 侧栏：嵌套、筛选、分组、多选、未读、深链、资源

渲染入口是 `renderSide` / `patchSide` / `groupBy`。列表数据来自 `GET api/sessions`，增量签名 `GET api/sessions?sig=`，能力 `list_delta` 时走 [`legacy-web/list-sync.js`](../legacy-web/list-sync.js) 的 `expand`。`ui_events` 时用 `EventSource api/events`（`startUiEvents`），页面隐藏会停。空闲页不拉未选中会话的正文。[`tests/list_delta_browser.py`](../tests/list_delta_browser.py)、[`tests/ui_events_browser.py`](../tests/ui_events_browser.py)。

### 三种列表

- **项目树**（默认）：按目录分组。同名目录用颜色区分（`timelineDirectoryColors`，会写回存储）。路径过长时 `fitTimelineDirectories` 压缩。
- **时间轴**：按时间分组，标题可钉住显示（`session_titles` 的点查，不是热发现）。[`tests/session_titles_browser.py`](../tests/session_titles_browser.py)。
- **会话分组**：只有 `metadata` 能力允许时 `#view [data-v=group]` 才出现。否则若存的是 `group` 会退回 `tree`。见下文分组。

分组头可折叠。普通浏览的折叠在 `closed`（持久）。搜索态用另一套 `searchClosed`，不写回 `closed`。[`tests/search_no_fold_browser.py`](../tests/search_no_fold_browser.py)。大量同名目录下点击折叠和筛选：[`tests/sidebar_path_performance_browser.py`](../tests/sidebar_path_performance_browser.py)。

左栏宽度 `width`，默认 340，最小 200。拖 `#drag`，双击回到默认。资源列打开时视觉宽度再加 144，存的仍是不含这 144 的值（`setSideWidth`）。

### 行上能看见的东西

每一行：来源图标、运行/冻结角标、标题、目录、可选星标、可选分组名、可选资源格。子代理行文案是「子代理 · 类型 · 时间」（`agentMeta`）。子代理在平铺和分层两种模式下都挂在所属会话下面；三角折叠的是子代理行或整棵子树，分层开关只决定「由会话发起的会话」是否缩进。打开子代理深链不会因此打开分层。[`docs/external-links.md`](external-links.md)、[`tests/session_deep_link_browser.py`](../tests/session_deep_link_browser.py)、[`tests/nest_tree_browser.py`](../tests/nest_tree_browser.py)。

运行角标：`paintItemStatus` / `paintStatusMarker`。冻结用暂停图标。有未读时显示数字；等待回答时是 `?`，标题带「等待回答」。颜色按当前 tmux/运行态现算，不把旧计数的颜色写进存储。后台命令的呼吸点：[`tests/process_activity_browser.py`](../tests/process_activity_browser.py)。

星标：`toggleSessionStar` → `POST api/session/star`。忙碌时不重复发，避免多页面乱序。需要 `metadata`。父会话可见性：`POST api/sessions/fork-visibility`。Codex 回退后的父会话默认从左栏隐藏，标题栏有父会话链菜单（`renderForkChainMenu`）：打开、显示、隐藏。记录不存在时显示「记录已不存在」。

点已选中的行不改它的子树折叠；只有三角切换折叠。刷新后恢复选中的父行，保留它自己的折叠。深链目标被折叠挡住时，只展开能看见它的祖先，并清掉会把它藏住的筛选。

### 筛选

来源 chip：点击切换，右键或长按「只选此类型」（`selectOnlySource`）。当前机器上数量为 0 时 `aria-disabled`，原因是「在当前选择的机器上没有会话」，仍可聚焦，点击不改变筛选。不弹自定义 toast。

Hub 机器 chip：点击切换，右键或长按「只选这台」。`app.js` 的新交互层拦截旧双击直选回调。离线不可选，原因来自 `nodeOfflineReason`。`nodesOff` 持久。筛选变化后若正在全文搜索，会重跑 `runSearch`。

活跃/全部是单选分段，不是 chip。分层列表在「活跃」模式下，展开父会话会提示被活跃筛选隐藏的直接子会话数量（「有 X 个不活跃会话」）；所有子会话都已退出时仍保留折叠入口。计数遵守来源、机器、分组及隐藏父会话筛选，不计入分组数量或多选；搜索、平铺和会话分组视图不显示此提示，切回「全部」则展示实际子会话。

### 嵌套与附属

`S.nest` 默认关。分层时，由会话发起的会话缩进在发起者下（`nestParentOf` / `nestTree`）。手动收起的发起者在 `nestClosed`（持久）。搜索态用 `searchNestClosed`，不写回。

上下文菜单和多选条有「附属到…」「解除附属」。点选父会话期间 `S.nestAttach` 不持久，Esc 取消。`POST api/session/nest`。成功且当前未开分层时会打开分层。跨节点手动嵌套：[`tests/hub_nest_browser.py`](../tests/hub_nest_browser.py)。Codex exec 扇出初始化 `nest_parent` 且选择在扫描/重启后仍在：[`tests/codex_exec_nest_browser.py`](../tests/codex_exec_nest_browser.py)。OpenCode 由工具 shell 发起的会话挂到发起者下：[`tests/opencode_spawn_browser.py`](../tests/opencode_spawn_browser.py)。

### 分组

[`legacy-web/groups.js`](../legacy-web/groups.js)，需要 `metadata`。

- `GET/POST api/groups`：创建、删除。删除确认就是按钮本身，没有第二套确认框。离线节点时状态是「已创建/已删除；离线节点恢复连接后同步。」
- 分组视图底部「＋ 新建分组」，Esc 退出编辑并焦点回到按钮。
- 行菜单「分组 ›」和多选「分组」打开 `#session-group-menu`：未分组或某个组名，单选勾。`POST api/session/group`。悬停（鼠标）展开，→ 打开，← 或 Esc 关闭。
- 行上显示「分组：名称」。目录里没有的组名不当成当前组。
- 待落盘的新建行不能分组。

[`tests/groups_browser.py`](../tests/groups_browser.py)。

### 多选

`S.picking` 不持久，刷新回到普通浏览。入口：行菜单「多选…」，或在多选态拖过行（`pickDragTo`，只认无修饰的主键）。

条上有：已选数量、全选/全不选、停止、分组、附属到…、删除、取消，以及停止结果的 `<details>`（失败、未确认、状态刷新失败）。Esc 退出多选。

- 停止：只对选中的运行中会话，并发数用 `stopConcurrency`。进行中按钮显示已返回/失败/未确认。
- 删除：见第 5 节。Hub 上一次多选附属和慢批量删除：[`tests/hub_bulk_browser.py`](../tests/hub_bulk_browser.py)。

长按开行菜单时，若用户正在选侧栏文字，会取消长按（`sidebarTextSelectionProtected`），避免抢走选择。

行菜单（`#item-menu`，`openItemMenu`）按固定顺序分三段：复制会话标识、多选；分组、附属到…、解除附属、移动/复制整组…、隐藏父会话；停止会话、删除会话。段间使用现有边框色的分隔线。不可用项保持可聚焦并带 `aria-disabled` 与原因，点击不发请求。复制标识成功后静默完成，不显示 toast，也不覆盖已有停止结果；失败仍显示错误。复制标识：[`tests/session_identity_browser.py`](../tests/session_identity_browser.py)。

### 未读

未选中的会话用未读摘要，不拉已缓存的消息正文。`POST api/sessions/unread`（Hub 为 `api/nodes/{id}/api/sessions/unread`），`fetchUnreadSummary` / `flushUnreadBatch`。只计代理产生的新内容。存 `unread`：`uid → {count, tmux}`，计数为 0 的项不写回。打开会话 `clearUnread`。

[`tests/sidebar_unread_browser.py`](../tests/sidebar_unread_browser.py) 覆盖节点和 Hub。

### 深链与地址栏

`?sid=` 最长取 128 字符。形式：

- `source:nativeId`
- 裸 native id 或旧的内部 uid（旧链仍能落到那一行，新分享不用 uid）
- `source:ownerNativeId/agent:agentId`
- 单独的子代理 native id，经 `agent_items` 打开所属根会话下的那个子代理
- 可选 `node=`，用来区分多机副本

同一 native id 有多行时，只有它们经同一机器、同一来源的 `continued_in` 收敛到一行才打开；独立副本、断链或环保持不确定，不随便挑。深链优先于上次 `sel`。`openSession` 用 `push` 或 `replace` 更新地址栏；子代理写成 `?sid=source:owner/agent:id`。Back/Forward 走 `popstate` → `routeSession`。其它查询参数和反代前缀保留。

Boot 时若上次 `sel` 是 `tmux:` 前缀的新建会话，会等 `api/term/list` 再打开（`openPendingSession`），避免列表先回来把导航冲掉。

### 资源列与会话资源

资源列默认关（`sidebarResources`）。打开后每行一个按钮，六格：CPU 核数、进程数、内存 PSS、GPU 张数、磁盘读、磁盘写（[`legacy-web/sidebar-resources.js`](../legacy-web/sidebar-resources.js)）。只画视口内的行（含 160px 预读）。采样超过约 15 秒或子代理行显示「—」。数字用 zh-CN 紧凑格式。每 5 秒，以及回到前台时，`GET api/resources/summary`。失败则清空数字，不把缺失显示成 0。点格子打开会话资源，不选中行。

[`tests/sidebar_toggle_browser.py`](../tests/sidebar_toggle_browser.py)。

会话资源对话框由 [`legacy-web/session-resources.js`](../legacy-web/session-resources.js) 现建：标题「会话资源」，范围「仅当前会话」/「包含子会话」（默认 inclusive），刷新，关闭。合计说明：只加当前会话相关机器的已知数，缺失不是零，同一进程只计一次。子代理若没有可识别进程归属，不会被拆进共享 CLI。`GET api/session/resources?uid&scope`。404 文案：「此服务尚未提供资源统计，请更新服务端。」

抽屉打开且约 60 秒内有可信操作、页面未休眠时，`POST api/session/resources/probe` 申请短租约；关闭、休眠或空闲会释放。状态：未探测 / 启动中 / 探测中 / 失败 / 关闭。探测失败按节点列在对话框里。

[`tests/session_resources_browser.py`](../tests/session_resources_browser.py)、[`tests/resource_probe_browser.py`](../tests/resource_probe_browser.py)、[`tests/process_links_browser.py`](../tests/process_links_browser.py)。

### 侧栏缺口

以下操作的覆盖尚需逐项核对：时间轴目录颜色分配、星标与父会话链的全部菜单路径（星标/可见性在 metadata 套件里有一部分）、单机（非 Hub）多选拖选、资源列六格各自的数值格式。不据套件名称判断这些操作是否已有覆盖。

---

## 3. 搜索

输入框 `#q`，占位：「快速筛选… Enter 搜正文」。无需回车即筛选标题、UUID/UID、目录、机器名、Agent 名和模型名。空格分词，双引号是短语。

打字：`cancelSearch()` 退出全文结果，120ms 后 `renderSide` 做本地筛选（`matchesSearch` / `searchTerms`）。Esc 清空并退出。大小写、全词、正则、AND/OR 存在 `opts`，默认全关、`mode: 'all'`（按钮显示 AND）。

- AND：全部词，可以落在不同消息。OR：任一词。
- 正则：整段输入当一个表达式，不拆词，AND/OR 按钮禁用。本地先 `reTerm`，无效则「正则无效」，不发全盘请求。正则匹配在 worker（[`legacy-web/regex-worker.js`](../legacy-web/regex-worker.js)）里，可取消。[`tests/conversation_performance_browser.py`](../tests/conversation_performance_browser.py)。
- 改选项：若已经在全文结果里，立刻重搜；否则只刷新本地筛选。

Enter：`runSearch` → `GET api/search`，NDJSON 进度。进度条 `#search-progress` 显示已扫描会话数、百分比、每台机器的准备中/扫描中/离线跳过/搜索失败/达到上限/已完成，以及「取消」。结果边到边画进左栏（约 50ms 合并一次），并给键盘一个任务间隙，使 Backspace/Esc 能中止。完成后左栏是命中列表，带「退出搜索 ×」。截断时提示细化条件。部分节点离线或失败会写进 `#stat`，不把不完整结果说成完整。

`search` 能力为 false 时不发 NDJSON，状态是「当前只能筛选标题和目录。」

全文搜索只换左栏。右侧已打开会话的内容、滚动和展开保持原样。搜索态的分组折叠和嵌套折叠与平时分开，开始时是展开的。[`tests/search_no_fold_browser.py`](../tests/search_no_fold_browser.py)、[`tests/search_uuid_browser.py`](../tests/search_uuid_browser.py)。

会话内高亮：全文词也会标到已打开的消息上（`markMatches` / `markRegexMatches`）。标题栏出现上一处/下一处（`jumpMark`）和计数。高亮数量有上限时 `markCapped`，导航停在已标出的范围内。自动展开有上限（`autoOpen`）。

右侧正文搜索与左栏全文是两条路径。审阅到的脚本覆盖了 UUID/UID 回车导航、搜索折叠隔离、可取消正则。AND/OR、大小写、全词在会话正文里的逐项点击，脚本头里尚需逐项核对。

---

## 4. 消息、工具、媒体、历史、分页

打开会话：`openSession` → `fetchMessages` `GET api/messages/{uid}`，可选 `start`/`head`/`agent`。随后 `EventSource api/watch`。增量对不上时 `scheduleDiffRecovery`，而不是沿用旧偏移重连。迁移读失败有可见重试（`renderMigrationReadFailure` / `retryMigrationRead`），不静默清空。

渲染：`renderSession` → `planTurns` / `appendMessages` / `msgNode`。角色包括 `user`、`user·subagent`、`assistant`、`assistant·subagent`、`thinking`、`tool`、`tool_result`、`question`、`answer`、`command`，以及事件行（`eventNode`）。子代理视图从标题栏下拉进入（`sessionViewRows`：主会话在前，运行中的子代理靠前并带绿点）。

### 回合与工具

`compactTurns` 默认开：已完成回合只留过程合集和最终结论。标题栏按钮在「展开所有过程」和「折叠已完成过程」之间切换，并写回存储。进行中的尾段保持展开，后面出现普通对话或任务结束后再封口（`sealTurnTail`）。

连续工具并成一组（`groupNode`），至少 2 条。用户打开过的组，后续追加仍保持打开。[`tests/tool_group_fold_browser.py`](../tests/tool_group_fold_browser.py)。大组默认折叠，展开才建 DOM；再折叠再打开内容还在。[`tests/conversation_performance_browser.py`](../tests/conversation_performance_browser.py)。单组不必再包一层过程外壳。

工具行（`toolEntry`）：命令/参数、输出预览、统计、可折叠全文。输出用 [`legacy-web/syntax.js`](../legacy-web/syntax.js) 和 [`syntax-worker.js`](../legacy-web/syntax-worker.js)，大段高亮不占 UI 线程。[`tests/render_assets_browser.py`](../tests/render_assets_browser.py)。Diff 行着色（`paintToolOutputDiff`）。带 `changes` 的文件修改走左右/行内 diff（`fileDiffMarkup` / `paintInlineFileDiff`），可切换换行。围栏图保持终端列宽，不因中文等宽字体挤乱。[`tests/code_diagram_browser.py`](../tests/code_diagram_browser.py)。

Markdown（`md` / `blocks` / `inline`）和公式（`renderFormulae`，按需加载 KaTeX）。公式库失败时正文仍在。审阅到的脚本头没有单独点名 KaTeX 渲染，标为缺口。原生标签族：[`tests/native_tags_browser.py`](../tests/native_tags_browser.py)、[`tests/history_browser.py`](../tests/history_browser.py)。

时间：相邻消息间隔 ≥5 分钟，或距上一条分隔 ≥20 分钟，插入时间分隔（`messageTimeDivider`）。折叠过程会重算分隔。

活动行 `renderActivity`：`working` / `waiting`。旧进程的 working 在 CLI 已不在时不显示。等待回答参与侧栏 `?`。

中断回合有单独标记（`markInterruptedTurn`）。Codex 侧线程在终端里可见时，对话页说明仍跟随 main thread，并提示终端里 Ctrl+/ 可切回（`renderTerminalThreadNotice`）。[`tests/legacy_browser.py`](../tests/legacy_browser.py) 覆盖提示出现和清除。

### 媒体

图片类型：PNG、JPEG、GIF、WebP、AVIF、BMP。无法识别的候选略过，周围文字仍在。消息 JSON 不带原始历史字节。

`imageHtml`：懒加载、`no-referrer`、固定宽高（若有）、失败显示「图片不可用」并可诊断重试（`mediaImageFailed` / `diagnoseMedia` / `reloadMediaSession`）。外链 HTTP(S) 由浏览器直接拉，页面不代理。本地引用走会话文件服务。

一条消息先显示有限张，其余是「还有 N 张图片，加载下一批」（`mediaMoreHtml`）。游标不可用时按钮禁用：「还有 N 张图片暂不可加载」。续页 `GET api/messages/{uid}/media-page`，校验不过则「图片分页响应与当前消息不匹配；请重新载入当前会话。」续页不推进实时 checkpoint。

[`tests/media_browser.py`](../tests/media_browser.py)、[`tests/media_files_browser.py`](../tests/media_files_browser.py)、[`tests/media_continuation_browser.py`](../tests/media_continuation_browser.py)。

### 历史分页

能力 `history_pages` 时，长会话中间是缺口按钮：「加载中间 N 条消息」（`historyGapNode`）。点击 `loadHistoryPage`，默认连翻直到填满（`HISTORY_PAGE_CHAIN`）；再点中止。按钮上能看到进度。没有该能力时，同一按钮走 `loadFullHistory`。

`GET api/messages/{uid}?window=1` 先给头部少量加尾部一窗。`GET api/messages/{uid}/page?cursor=` 把中间插到已有尾部之前，不推进、不回滚实时尾，也不停 watch。响应和当前缺口对不上时保留已有历史，提示重新载入，不静默清空，也不改去拉无界全文。授权丢失时用 `partial.resume` 再要一页。[`tests/history_pages_resume_browser.py`](../tests/history_pages_resume_browser.py)。合同在 [`docs/history-pages.md`](history-pages.md)。

滚动：贴底跟随（`stickBottom`），用户上翻后不抢。增量插入尽量保住当前消息锚点（`mutateKeepingMessageAnchor`）。打开会话、搜索跳转用 `jumpWithinConversation`。

### 文件引用

消息里的路径、`file://`、带行号的引用变成链接（`referenceLink`）。右键 `#file-menu`：复制完整路径、复制所在目录、下载、复制链接、在新标签页打开。解析在点击时 `POST api/session/resolve-files`，不在每次 SSE 上做。网页链接另走「复制链接 / 在新标签页打开」。`files` 能力关闭时菜单会说明当前不能解析。

左键打开 `file.html?...&open=1`，再跳到 FileDock。见第 7 节。[`tests/files_browser.py`](../tests/files_browser.py)。

### Claude 时间线固定

仅 Claude、非子代理、用户消息带 `turn_id`，且 `timeline_pin` 能力开时，消息上有「回到此处」（`timelinePinAction`）。`POST api/session/rewind`。固定后标题下通知：「已固定显示到所选输入之前，CLI 未回滚」。可「取消固定」。失效时说明原因，按钮变成「清除记录」。终端里做出的回滚若已同步，通知是「已同步终端里的回滚」，没有取消按钮。失败写在左栏 `#stat`，约 2.4 秒后恢复计数。

[`tests/rewind_browser.py`](../tests/rewind_browser.py) 覆盖桌面和 390px。[`tests/rewind_cli_browser.py`](../tests/rewind_cli_browser.py) 覆盖 CLI 自己回滚后对话跟着变。

---

## 5. 输入、草稿、问题、审批、动作、新建、克隆、移动、回收站、反馈

输入区 `#composer`。SSH/纯 PTY 会话没有对话归档：终端在输入框上方，输入框只有文字和发送，没有附件和 Esc。Agent 会话默认对话页，终端可切换。

### 草稿与发送

服务端每个逻辑会话一份草稿：文字、引用、附件元数据、提交 ID、修订号。`GET/POST api/session/conversation`。浏览器里的旧 `composerDraft.*` 和 IndexedDB `composer-drafts` 只在导入时读一次（`importLegacyComposer`），成功后删掉本地副本。

行为（[`docs/conversation.md`](conversation.md)，实现在 [`legacy-web/term.js`](../legacy-web/term.js) 的 `composerDraft` / `persistComposerDraft` / `submitComposer`）：

- 打开或切换会话只读草稿，不立刻保存。输入框在第一次读取返回前禁用。
- 正在编辑的页面赢；空闲页面跟随服务端修订。不做字段级合并。保存冲突重读再写，最多两次，仍失败才显示错误。
- 每次编辑立刻标未保存，150ms 合并后写入。刷新只能恢复服务端已确认的内容。未保存文字或未上传完的附件会触发离页提醒。
- 附件先暂存再在发送时发布到会话目录 `sessiondock_attachments`。选择时和上传时都按 512 MiB 检查。每个草稿最多两路并发上传，卡片有排队、进度、取消、失败重试。图片预览超过 32 MiB 或读不回时用类型图标。
- 一次粘贴超过 5 个文件或合计超过 50 MB，先确认（输入框、报告、控制台同一规则，`confirmPastedFiles`）。取消则不暂存。文件夹不能直接粘贴，提示先压缩；若同时有普通文件，确认那些文件后再提示文件夹。只有 `text/csv`、没有 File 时，做成一个 csv 附件。带文件的粘贴会吃掉同剪贴板的 text/plain，避免贴两次。
- 拖放文件到输入区（`bindFileDrop`）。
- 引用：附件菜单「引用文字」，或对话里选中的文字（最多 16000 字）带进引用卡。
- 桌面 Enter 发送，Shift+Enter 换行。窄屏 Enter 只换行，只能点「发送」。IME 组字期间不发送。
- 输入框为空时 ↑ 打开本会话输入历史（`openComposerHistory`），↑↓ 选择，Enter 填入，Esc 关闭。`GET api/session/input-history`。
- 发送按钮以服务端 `input` 为准（`renderComposerInputStatus`）。非 ready 的原因显示在输入框旁，草稿仍可编辑。恢复 ready 后启用发送，不自动提交。受阻时按 Enter 不弹「发送失败」。CHECK 超过 5 秒显示超时并继续轮询。纯终端或输入框被藏起时停止 CHECK。
- 发送中按钮显示「上传 i/n」「发送中」。失败：「发送失败，输入保留：…」，焦点回到输入框（若用户没去点别处）。成功后已发送内容从草稿清掉。
- 排队气泡在对话末尾：「已发送，等待 CLI 处理」，随后可变成「已进入 CLI 队列，当前步骤结束后处理」。可关闭一条排队记录（`POST api/session/conversation/queued/dismiss`），不重发。原生记录对上后气泡消失，没有成功 toast。Codex 内置命令发送成功后不进这个等待队列。Esc 退回编辑区的那条会标成「已被 Esc 退回终端输入框，CLI 未处理」。
- 版本过期时发送和添加附件禁用，草稿仍可编辑保存。见第 8 节。

[`tests/send_browser.py`](../tests/send_browser.py)、[`tests/send_codex_attachments_browser.py`](../tests/send_codex_attachments_browser.py)、[`tests/send_native_codex_browser.py`](../tests/send_native_codex_browser.py)、[`tests/grok_send_echo_browser.py`](../tests/grok_send_echo_browser.py)、[`tests/hub_draft_recovery_browser.py`](../tests/hub_draft_recovery_browser.py)。Hub 启动时按节点发现保留草稿：成功节点本页只读一次，失败节点退避，离线跳过，上线再试；当前会话的修订同步不受这个发现缓存影响。

输入历史的 ↑ 面板、512 MiB 边界提示、离页提醒的文案，审阅到的脚本头尚需逐项核对。

### 问题、信任、审批、屏幕菜单

历史里的问题渲染成题卡（`questionNode`）。正在问的卡可以点。单选题：Claude 用方向键把光标夹到真实选项再按数字；Codex 用数字直选；审批用选项上的助记键，不用数字菜单（[`legacy-web/cli.js`](../legacy-web/cli.js)）。Grok、OpenCode 走基类，基类不猜测按键。多题单选（Claude）在网页上每题选一项再「提交答案」，按键分组发送，组间约 50ms。多选或无法直答时文案是「多选或多题请在原生终端回答」，并提供「打开终端」和「取消」。

画面菜单（`kind: screen_menu`）复用同一张卡，另外有：多选同步勾选、当前页文字、下一页/返回/提交等 `actions`、取消（仅当给了 `cancel_keys`）。文字草稿存在页面内存（`screenMenuTextDrafts`），轮询不把正在打的字清掉。点击前重新 CHECK 同一语义 ID，再按新画面算按键。菜单消失、换了会话或选项变了，就不写。密钥类字段不在网页里填。

目录信任：Codex 预启动文件夹信任走 composer 题卡。[`tests/startup_question_browser.py`](../tests/startup_question_browser.py)。Claude 工作区信任在 hook/历史之前，同一套卡。[`tests/startup_claude_browser.py`](../tests/startup_claude_browser.py)。Claude 题卡点到正确原生选项：[``tests/question_browser.py``](../tests/question_browser.py)。四个 CLI 的源码菜单清单在 Chromium 里点：[``tests/cli_menus_browser.py``](../tests/cli_menus_browser.py)。

发送在 `cli_question` 时被拒绝，输入保留。回答走终端按键，不走 SEND 正文。

Esc 按钮 `#cesc`：`sendComposerEscape`。Claude/Codex 在忙碌或输入非空时，Esc 是中断或取消，不是回滚。空输入 650ms 内第二次 Esc 才打开原生 TUI 的回滚/编辑（`revealNativeTerminal`）。Grok/OpenCode 的基类不把双 Esc 当成回滚。

用户主动打开终端后，当前这张题卡记为已看过；后台刷新终端列表不能再因为它把终端关掉。新的问题 ID 仍可自动把对话拉回来。

### 标题栏动作

`head` 里可见：

- 手机返回列表。
- 有子代理时，标题是下拉，切换主会话/子代理。
- 父会话链或子会话菜单。
- 控制台按钮（`bindConsoleButton`）。不可用时 toast 说明原因（`showConsoleToast`），不打开空终端。Hub 上按节点能力灰掉：[`tests/hub_console_availability_browser.py`](../tests/hub_console_availability_browser.py)。
- 星标、展开/折叠过程、搜索导航。
- 冻结现场 / 恢复运行（`renderSessionFreeze`）。需要 `session_freeze`、已加载的终端列表、以及可验证的 `instance_id`。否则按钮在，但是不可用，原因分别是：正在读取、当前节点不支持、状态已失效、没有可验证实例。成功后会话区覆盖「会话已暂停」和恢复按钮，不放进全局浮层。`POST api/session/freeze`。[`tests/session_freeze_browser.py`](../tests/session_freeze_browser.py)。
- 报告当前会话问题。
- 移动/复制整组。仅当 `session_clone_local_codex === true` 才出现。运行中灰色，原因：「会话正在运行，请先停止后再移动或复制整组。」待落盘行或没有该旗标：「此会话当前不支持移动或复制整组。」
- 停止或删除。运行中（`S.live` 或列表里有未过期的托管 `instance_id`）是停止，否则是删除。父会话行这里是显示/隐藏父会话。尚未被对话占用的新建启动是删除这次启动。

停止：`POST api/session/stop`。结果写在会话内通知（`showSessionStopNotice`），不是浏览器 `alert`。停止失败保留会话。

删除确认见下。

更窄时这些动作收进会话头的 ⋯（`bindSessionActions` / `layoutSessionHead`）。Esc 关菜单。

### 新建会话

`#new-session-dialog`（`openNewSessionDialog` / `createNewSession`）。来源：Claude（默认）、Codex、Grok、OpenCode、SSH。Hub 上先选机器。模型和推理强度按该 CLI 自己的目录（`createModelPicker`，`GET api/term/models`），可搜索。记忆的模型和 effort 按 `storeKey` 写入浏览器。工作目录：最近 8 条（`newDirs` 或 Hub 的 `newDirs.<nodeId>`）、常用目录、Tab 补全（`GET api/term/complete-dir`）。创建后侧栏出现待落盘行，阶段文案来自 `pendingStageMessage` / `workerStatusMessage`。可停止、删除或丢弃。记录还没出现时有明确缺失文案（`pendingRecordMissing`）。启动回执在 Hub 列表不完整时仍以真实退出为准。[`tests/hub_pending_state_browser.py`](../tests/hub_pending_state_browser.py)、[`tests/pending_create_discard_browser.py`](../tests/pending_create_discard_browser.py)、[`tests/new_session_model_browser.py`](../tests/new_session_model_browser.py)、[`tests/new_session_follow_browser.py`](../tests/new_session_follow_browser.py)、[`tests/lifecycle_cli_browser.py`](../tests/lifecycle_cli_browser.py)。

丢失根目录时，其它来源和新建仍可用。[`tests/missing_roots_browser.py`](../tests/missing_roots_browser.py)。

### 移动与复制

对话框由 `cloneSessionGroup` 现建，标题「移动或复制会话组」。

- 源机器只读，目标机器下拉。离线或停用的目标标「不可用」且不可选（源机器自己除外）。
- 操作：复制（默认）或移动。同机不能移动，提示「移动需要选择另一台机器。」跨机需要 `session_clone_remote` 或 `session_move_remote`，否则「跨机器传输尚未接入。」
- 跨机才出现「生成新 UID」，复制默认勾选，移动默认不勾。两种操作各自记住这一勾。
- 清单表：会话、来源、关联（所选会话 / 子代理 / 分支关联 / 关联历史）、历史文件数、大小。状态如「整组 N 个会话 · N 份历史 · 大小」。
- 环境行：核对目标 CLI 是否安装、版本是否更旧、动态工具是否未核验。未核验会写出来，不伪装成已通过。
- 按钮随状态变成「复制整组 / 移动整组 / 正在复制… / 重试同一次复制 / 撤回本次移动」。结果不确定时不能改目标、操作和 UID 选择。
- 计划 `POST api/session/clone/plan`，执行同机 `api/session/clone`、跨机 `api/session/transfer/clone`，进度 `api/session/clone/progress` 或 `api/session/transfer/progress`，取消 `api/session/clone/cancel` 或 `api/session/transfer/cancel`。
- 完成后关闭对话框，打开目标会话，通知「整组复制完成，原会话已保留。」或「整组移动完成。」
- 顶栏 `#transfer-tasks` 列出未完成任务，可继续。`GET api/session/transfers`，`refreshTransferTasks`。

[`tests/session_clone_browser.py`](../tests/session_clone_browser.py)、[`tests/session_transfer_browser.py`](../tests/session_transfer_browser.py)、[`tests/session_transfer_cache_browser.py`](../tests/session_transfer_cache_browser.py)、[`tests/session_transfer_environment_browser.py`](../tests/session_transfer_environment_browser.py)、[`tests/session_transfer_isolation_browser.py`](../tests/session_transfer_isolation_browser.py)、[`tests/session_files_clone_browser.py`](../tests/session_files_clone_browser.py)、[`tests/session_mixed_clone_browser.py`](../tests/session_mixed_clone_browser.py)、[`tests/session_mixed_bundle_browser.py`](../tests/session_mixed_bundle_browser.py)、[`tests/session_local_recovery_browser.py`](../tests/session_local_recovery_browser.py)、[`tests/session_prefix_browser.py`](../tests/session_prefix_browser.py)、[`tests/session_files_browser.py`](../tests/session_files_browser.py)、[`tests/session_grok_checkpoint_browser.py`](../tests/session_grok_checkpoint_browser.py)、[`tests/session_goals_native_browser.py`](../tests/session_goals_native_browser.py)。

### 回收站与删除

删除确认（`del` / `deleteSessions`）说明：

- 一般会话：「文件会移入服务端回收站，不会永久删除。」
- OpenCode：「会从 OpenCode 直接删除（连同子会话），不进回收站，无法恢复。」删除后空状态是「会话已从 OpenCode 删除」。
- 未落盘的新建：停止并丢弃，未发送草稿清除；若已经生成记录，记录保留。未落盘项可以不经确认直接丢弃；只要选择里有已记录会话，仍弹确认。
- 运行中的已记录会话会被跳过，需要先停止。
- 运行状态未知且后端返回 `run_state_unknown` / `needs_force` 时，再问一次：未知不等于已停止，确认 CLI 已退出才 `DELETE ?force=1`。

成功后详情显示回收站路径和「打开回收站」。`DELETE api/session/{uid}`，批量 `POST api/sessions/delete`。

回收站对话框：列表、恢复、彻底删除、刷新、清空、完成。副标题含数量、大小、目录；有 `next_cursor` 时说明只显示最近若干条。不能恢复的行显示原因并禁用恢复。彻底删除和清空都要再确认，文案写明不可恢复。`GET api/trash`，`POST api/trash/restore`，`POST api/trash/purge`。Hub 列表和清空带当前选中的 `nodes`。恢复后退出搜索并刷新左栏。

[`docs/trash.md`](trash.md)。审阅到的 `*browser.py` 头没有名为回收站的套件。恢复、清空等点击路径的覆盖尚需逐项核对。批量删除的一部分在 [`tests/hub_bulk_browser.py`](../tests/hub_bulk_browser.py)。

### 报告问题

`#bug-report-dialog`（`openBugReportDialog`）。来源默认 Codex（`bugReportSource`），可选 Claude/Codex/Grok/OpenCode，无 SSH。Hub 上选处理节点。模型与推理强度用同一套选择器。描述最多 50000 字。附件：图片、视频、音频、文件，无「引用文字」。草稿用临时身份 `BUG_REPORT_DRAFT_UID`，提交后绑到处理会话。可附带当前会话和终端上下文（`captureBugReportContext`）。发送按钮「提交并启动处理会话」。失败保留草稿。已退出且未绑定的处理会话可以重新启动。提交后浮动条指向该会话（`showBugReportToast`）。

节点记忆：`bugReportNode`、`bugReportDraftNode`、`reportDraftId.<node>`。

[`tests/bug_report_upload_browser.py`](../tests/bug_report_upload_browser.py) 覆盖丢失回复后的补传，不重复上传或启动。报告对话框的模型选择和描述键入尚需逐项核对。合同 [`docs/bug-report.md`](bug-report.md)。

---

## 6. 终端：归属、网格、xterm、键盘、触摸、剪贴板、录制

实现在 [`legacy-web/term.js`](../legacy-web/term.js)，菜单在 [`legacy-web/term-menu.js`](../legacy-web/term-menu.js)。网格外观层 [`legacy-web/grid/facade.js`](../legacy-web/grid/facade.js) 导出 `GridTerm`，由 `index.html` 在 `term.js` 之前挂到 `globalThis.GridTerm`。

### 打开方式与布局

会话跑在 tmux 里，关页面或重启服务不打断它。`T.mode`：`full` 纯终端（默认）、`collapsed` 对话加输入、`normal` 手动分屏。旧存储若把 `normal` 当默认，只迁移一次到 `full`（`termLayoutPolicyVersion` 2）。此后 `normal` 只由拖 `#tgrip` 产生，并按 tmux 名记在 `termviews`（含高度 `termh`，默认 320）。顶栏控制台按钮只在纯对话和纯终端之间切（`toggleTermPane`）。桌面新打开且没有保存过布局时用 `full`。SSH 的 PTY 在输入框上方（`sessionTerminalFirst`）。

吸附高度为 0 时保留对象和连接，但不 fit、不新连，避免把 tmux 压成很小。字体就绪后再 fit，连接前先定尺寸。

浅色主题改写终端颜色（`lightTerminalAnsi` 等），并丢掉会把配色设回深色的 OSC。网格渲染器在宿主侧画，不走这层 SGR 改写。[`tests/terminal_grid_theme_browser.py`](../tests/terminal_grid_theme_browser.py)。

### 两种渲染器

默认网格。机器设置或单机 `consoleRenderer` 可选 xterm。`ensureTerm`：网格加 `mode=grid`，不用 xterm 的 fit/webgl/unicode 插件；xterm 路径才加载那些插件，并在符合条件时用 WebGL（`shouldUseTermWebgl`）。宿主太旧则强制 xterm。切换后要重新打开控制台。

网格在主控制台右侧留 12px 滚动条。拖动或点击轨道只改本地视口，不把滚轮发给 PTY。焦点在滚动条上时可用方向键、PgUp/PgDn、Home/End。新输出不抢走已上翻的位置；回到底部再跟随。备用屏幕上滚动条隐藏。xterm 用它自己的滚动条。[`tests/terminal_scrollback_browser.py`](../tests/terminal_scrollback_browser.py)。

独立 `grid.html` 见第 7 节。主控制台和它共用网格模型，但主控制台还有归属、租约和录制。

### 归属与连接

`POST api/term/claim`，然后 WebSocket `api/term/attach`。页面级 `TERM_PAGE_ID` 不进存储，复制标签页不复制归属。连接超时约 15 秒，心跳 3 秒，10 秒无心跳则恢复，且不重放含义不明的输入。[`tests/terminal_heartbeat_browser.py`](../tests/terminal_heartbeat_browser.py)。Claim 超时约 20 秒。

别人拿着终端时，按钮是接管（`takeover` / `renderTakeoverBtn`）。被收回时说明是谁、从哪（`handleTermRevoked` / `describeTermTaker`），并给输出通知（`renderTermOutputNotice`）。只有用户主动进入 PTY 且终端在别的页面手上，才问是否接管。对话页的 CHECK、SEND 和答题不抢已打开的终端。

同一会话可有关联终端（`linkedTermSession` / `toggleLinkedTermSession`）。列表不确定时合并上次行，不把整表清空（`mergeUnavailableTermRows`）。宿主退出记在 `recordHostExit`，结束后可进录制回放。

[`tests/managed_terminal_browser.py`](../tests/managed_terminal_browser.py)、[`tests/terminal_diagnostics_browser.py`](../tests/terminal_diagnostics_browser.py)、[`tests/terminal_exit_browser.py`](../tests/terminal_exit_browser.py)、[`tests/terminal_input_browser.py`](../tests/terminal_input_browser.py)。

原始输入也可以走 `POST api/term/send`，滚动 `POST api/term/scroll`。合同 [`docs/terminal-input.md`](terminal-input.md)、[`docs/terminal-ownership.md`](terminal-ownership.md)。

### 键盘、触摸、剪贴板

桌面：键落到当前终端。Ctrl+V / ⌘V 在允许时把选区贴回 PTY。右 Ctrl 只修饰下一次输入（`setTermCtrl`）。

窄屏底栏：Ctrl、Alt、Shift、Esc、Tab、方向键、PgUp、PgDn。Ctrl/Alt 是「下一键」，Shift 锁住本地选字、绕过 CLI 鼠标捕获，再点关闭。状态 `#term-ctrl-lock` 显示「Ctrl（下一键）」。Alt 的物理键字节在两种渲染器里都进 PTY。[`tests/terminal_alt_browser.py`](../tests/terminal_alt_browser.py)。

软键盘只压缩页面可视高度，不向 PTY 发 SIGWINCH。行列按键盘收起时的布局量；键盘开着时改界面缩放，行列仍会跟着变。[`tests/terminal_keyboard_browser.py`](../tests/terminal_keyboard_browser.py)。

选字与复制：鼠标拖选、双击单词。CLI 开了鼠标捕获时，Shift 拖选走本地。OSC 52 写入剪贴板（`handleOsc52Clipboard`）。复制失败时菜单提示聚焦后 Ctrl+V（macOS ⌘V）。触摸选区：`installTermTouchSelection`。两种渲染器：[`tests/terminal_selection_browser.py`](../tests/terminal_selection_browser.py)。

上下文菜单和「⋯」：复制、粘贴、全选、查找已加载输出（上一个/下一个/关闭）。输出在查找过程中变了，状态是「输出已变化，请重试查找」，不无限重开。Ctrl/⌘+点击网格上的 OSC 8 链接，新标签打开。

控制台粘贴文件默认关。打开后，粘贴的图片或文件存进会话目录 `sessiondock_attachments`，路径写入终端（`consolePasteFiles` / `publishConsolePaste`）。大批粘贴先确认。审阅到的脚本头没有单独覆盖这个开关打开后的粘贴，标为缺口。

Codex 侧线程：终端视口里出现 side thread 时记下状态（`setCodexSideThreadState`）。用户用 Ctrl+/ 在 CLI 里切回 main。页面不替用户发送这个切换。

### 录制回放

会话结束后，主控制台用同一条录制流在原地回放（`startShellRecordingReplay` / `renderTimeline`）。条上：状态「会话已结束 · 只读回放」、播放、进度、时间、倍速 1/2/4/8/16、跳到最新并跟随。`timelineSeekTo` 不把按键发给已退出的 PTY。进行中的录制可跟随。独立录制页见第 7 节。

[`tests/terminal_records_browser.py`](../tests/terminal_records_browser.py) 覆盖录制页的列表、跟随、尺寸、只读、退出、刷新和「适应窗口」。主控制台里的倍速和刻度点击尚需逐项核对。

---

## 7. 辅助页：网格、录制、文件

这些页和主站共用主题前缀与字体，采用独立 HTML。迁移时它们仍是用户能打开的入口，不是内部调试页。

### 网格终端 `grid.html`

[`legacy-web/grid.html`](../legacy-web/grid.html) + [`legacy-web/grid.js`](../legacy-web/grid.js)。标题「网格终端」。工具条：会话下拉、连接、抢占（需要时）、状态、尺寸、复制选区、粘贴、回到底部、从录制返回（`?record=` 时）。画面是 canvas 加隐藏输入框。

连接：`term/list`、`term/claim`、WebSocket `term/attach?mode=grid`。录制：`term/records/attach?mode=grid`，只读，无 claim、无输入、无 pty resize。滚轮在应用鼠标模式里发给 PTY，否则本地翻页并按需拉历史（`term/grid/history`）。Ctrl/⌘+点击开 OSC 8 链接。双击选词。主题跟随主站 `theme`。

主控制台的网格套件覆盖了画、滚动和主题的一部分。[`tests/terminal_grid_render_browser.py`](../tests/terminal_grid_render_browser.py) 看的是画完之后的格子，不能单独代替把 `grid.html` 的连接、抢占、粘贴、历史当作用户路径点一遍。独立 `grid.html` 的点击流在审阅到的脚本头里尚需逐项核对。模块分工见 [`docs/terminal-grid.md`](terminal-grid.md)。

### 录制页 `records.html`

[`legacy-web/records.html`](../legacy-web/records.html) + [`legacy-web/records.js`](../legacy-web/records.js)。标题「终端录制」。左列表、右 xterm。刷新、只看进行中、状态、大小、「适应窗口」（`records-fit`，默认关）、复制选区、回到底部。空态「选择左侧的录制」。嵌在主页面里（`embedded=1` 或 iframe）时去掉自己的标题栏。

Hub：`?nodes=` 聚合多机，单条用 `?node=`。列表 `term/records`，观看 WebSocket `term/records/attach`。键盘：↑↓ Home End Enter/Space 在列表里移动。断线在可见时重连；`code === 1000` 或已经结束则不重连。缺口在画面里写成「（录制有缺口，已从下一个快照继续）」。能力关掉时状态是「此机器未启用终端录制」。

[`tests/terminal_records_browser.py`](../tests/terminal_records_browser.py)。

### 文件入口 `file.html` 与 `files.html`

两页标记相同，都加载 [`legacy-web/file.js`](../legacy-web/file.js)。它们不浏览目录、不预览，只解析后跳到 FileDock。

- 有 `node` 和绝对 `path`：直接 `location.replace` 到 `filedock_url`（默认 `/files/`）的 `?node&path`，并带上原来的 hash。
- 只有会话 `uid` 和 `ref`：先 `POST api/session/resolve-files`。相对名先对当前 cwd，不行再看记录里的路径和这些目录的直接子项。多个不同目标保持不确定。
- 失败停在本页：标题「无法打开文件」，正文是实际错误，按钮「刷新」。未登录时：「请登录后刷新页面」。

目录浏览、预览、下载、上传属于 FileDock，不在本仓库页面里。对话侧仍负责识别引用、行内媒体、附件上传和控制台粘贴。

[`tests/files_browser.py`](../tests/files_browser.py) 覆盖对话链接、node/path、解析失败和直接目录入口，含桌面和手机。FileDock 自己的套件不在本文范围。

---

## 8. PWA、更新、休眠、审计

### 安装与更新

[`legacy-web/manifest.webmanifest`](../legacy-web/manifest.webmanifest)：名称 SessionDock，`display: standalone`，`start_url`/`scope` 为 `./`，图标 192 与 512。`theme-color` `#245de8`。

[`legacy-web/pwa-install.js`](../legacy-web/pwa-install.js) 接 `beforeinstallprompt`。按钮文案：「安装到桌面」；已是独立窗口或已安装时「已安装」并禁用；浏览器还没给安装事件时禁用，标题「浏览器尚未提供安装能力」。脚本标签上有 `data-storage-key="sessiondock.pwa-install-dismissed"`，当前这个脚本不读、不写该键。没有单独的「以后再说」存储。

页面不再注册 Service Worker。`index.html` 末尾会注销已有 registration，并删掉名前缀 `sessiondock-shell-` 的 Cache Storage。原因写在页面注释里：避免旧页面卡死时新标签也被 Service Worker 拖住。离线壳不是当前行为。

版本：每 30 秒 `GET api/meta`。`build` 与页面 `sessiondock-build` 不一致时 `markStaleBuild`：暂停自动同步，浮动卡「SessionDock 已更新」，「仍可编辑并自动保存草稿；发送前请重新加载。」「重新加载」先 `prepareComposerReload`，草稿或附件未完成则取消刷新并说明。「稍后」只藏起卡片，发送和添加附件保持禁用，回到前台再显示。

[`tests/popup_browser.py`](../tests/popup_browser.py) 覆盖版本卡片及「稍后」，[`tests/draft_sync_browser.py`](../tests/draft_sync_browser.py) 覆盖保存失败时取消重新加载。PWA 安装流程覆盖待核对。设置页按钮的禁用态随 `frontend_framework_browser` 的外观页一起存在，但那不是安装流程。

### 休眠

[`legacy-web/page-sleep.js`](../legacy-web/page-sleep.js)。无可信操作达到 `sleepMinutes`（默认 60，0 为不休眠）后，全屏对话框「页面已休眠」，只有 Resume。`SessionDockNetwork.pause('idle')`。回到标签、焦点、`pageshow` 只重新检查时间，不关掉对话框。休眠期间点对话框以外的地方会被吞掉，避免点到旧按钮。鼠标移动、按键、滚轮、触摸、输入都算活动，且必须是 `isTrusted`。其它标签改了 `sleepMinutes` 时，本页 `storage` 事件跟着变。休眠不停止后端 CLI。

[`tests/page_sleep_browser.py`](../tests/page_sleep_browser.py)。

### 审计

`audit` 能力开时，页面把 UI 事件排队，`POST api/audit/browser`，离开时 `sendBeacon`。约 5 秒或满批（约 48KB / 100 条）再发。单条内容有上限。失败会退避，不阻断操作。事件包括长帧、控制台按钮状态、详情渲染、终端面板开关、头部布局、冻结、构建过期、HTTP 开始等（`browserAuditEvent`）。页面 ID 与终端页 ID 相同，不持久。

[`tests/audit_browser.py`](../tests/audit_browser.py)、[`tests/renderer_fd_browser.py`](../tests/renderer_fd_browser.py)。审计是现有诊断，不是迁移要新加的检查。

### 其它现有浏览器锚点

不重复归类、但迁移时仍会碰到的脚本：

- [`tests/legacy_browser.py`](../tests/legacy_browser.py)：现有页面垂直切片。
- [`tests/hub_browser.py`](../tests/hub_browser.py)：Hub 与多节点。
- [`tests/sessions_visibility_browser.py`](../tests/sessions_visibility_browser.py)：过时的测试注册表/URL 不藏起普通会话。
- [`tests/opencode_mirror_browser.py`](../tests/opencode_mirror_browser.py)：OpenCode 镜像在真实页面上的交互。
- [`tests/bench_term_echo_browser.py`](../tests/bench_term_echo_browser.py)：控制台按键回显。这是性能观察，不是功能清单的验收政策。

---

## 存储键与默认值

前缀见文首。值经 `store.set` 的是 JSON。下表是页面会读的键，不是建议新键。

| 键 | 默认 | 谁写 |
| --- | --- | --- |
| `theme` | `system` | 外观 |
| `font` | `ubuntu` | 外观 |
| `interfaceScale` | `100`（读到 30–49 时按 50） | 滑块、捏合 |
| `settingsTab` | `appearance` | 设置标签 |
| `sleepMinutes` | `60` | 功能；合法值 0/5/15/30/60/120/240 |
| `cacheMb` | `256`；`0` 不限制 | 功能 |
| `stopConcurrency` | `6` | 功能 |
| `consolePasteFiles` | `false` | 功能 |
| `consoleRenderer` | `grid` | 仅单机「本机」；Hub 用服务端 |
| `view` | `tree` | 顶栏 |
| `nest` | `false` | 顶栏 |
| `nestClosed` | `[]` | 分层折叠；搜索态不写 |
| `closed` | `[]` | 分组折叠；搜索态不写 |
| `off` | `[]` | 来源筛选 |
| `nodesOff` | `[]` | Hub 机器筛选 |
| `activeOnly` | `false` | 活跃/全部 |
| `compactTurns` | `true` | 会话头 |
| `unread` | `[]` | 未读；0 不保留 |
| `width` | `340` | 左栏，不含资源列加宽 |
| `sideCollapsed` | `false` | 左栏 |
| `mobilePage` | `list` | 窄屏列表/详情 |
| `sel` / `agent` | 无 | 上次会话 |
| `opts` | case/word/regex 关，`mode: all` | 搜索 |
| `termh` | `320` | 分屏高度 |
| `termmode` | `full` | 终端布局 |
| `termviews` | `[]` | 按 tmux 名记 mode/height |
| `termLayoutPolicyVersion` | 迁到 `2` | 一次性 |
| `sidebarResources` | `false` | 资源列 |
| `timelineDirectoryColors` | `[]` | 时间轴同名目录颜色 |
| `newDirs` 或 `newDirs.<node>` | `[]` | 最近工作目录，最多 8 |
| `newNode` | `''` | Hub 新建默认机器 |
| `bugReportSource` | `codex` | 报告 |
| `bugReportNode` / `bugReportDraftNode` | `''` | 报告节点 |
| `reportDraftId.<node>` | 新 UUID | 报告提交身份 |
| 模型与 effort | 按 CLI 键 | 新建和报告的选择器 |
| `records-fit` | 关（裸字符串 `'true'`） | 仅录制页 |

一次性导入后删除：`queuedMessages`、`composerDraft.<uid>`、`composerDraftUids`，以及 IndexedDB `<prefix>composer-drafts`。当前草稿在服务端。

`sessiondock.pwa-install-dismissed` 出现在安装脚本的属性上，当前 `pwa-install.js` 不使用它。

终端归属、审计页面 ID 不进 localStorage / sessionStorage。

## 页面会调用的路径

只列现有前端实际请求的路径。Hub 对节点的转发是 `api/nodes/{id}/api/...`。搜索和回收站在 Hub 上附加当前选中的 `nodes`。

会话与列表：`api/meta`、`api/sessions`、`api/sessions?sig=`、`api/sessions/unread`、`api/sessions/delete`、`api/sessions/fork-visibility`、`api/events`、`api/live`、`api/watch`、`api/messages/{uid}`、`api/messages/{uid}/page`、`api/messages/{uid}/media-page`、`api/search`、`api/session/star`、`api/session/nest`、`api/session/stop`、`api/session/freeze`、`api/session/{uid}` DELETE、`api/session/rewind`、`api/session/file`、`api/session/resolve-files`、`api/groups`、`api/session/group`。

对话：`api/session/conversation`、`.../attachment`、`.../attachment/discard`、`.../check`、`.../send`、`.../queued/dismiss`、`.../restart`、`api/session/input-history`、`api/session/conversation/drafts`。

终端：`api/term/list`、`api/term/claim`、`api/term/attach`、`api/term/send`、`api/term/scroll`、`api/term/models`、`api/term/complete-dir`、`api/term/new-status`、`api/term/grid/history`、`api/term/records`、`api/term/records/attach`。

转移与回收站：`api/session/clone/plan`、`api/session/clone`、`api/session/clone/progress`、`api/session/clone/cancel`、`api/session/transfer/clone`、`api/session/transfer/progress`、`api/session/transfer/cancel`、`api/session/transfers`、`api/trash`、`api/trash/restore`、`api/trash/purge`。

机器与其它：`api/nodes`、`api/nodes/order`、`api/clients`、`api/clients/update`、`api/shell-env`、`api/shell-env/restart`、`api/resources/summary`、`api/session/resources`、`api/session/resources/probe`、`api/audit/browser`、`api/bug-report`。

文件跳转目标默认 `/files/?node&path`，可由 `filedock_url` 换源。

## 基线盘点时的浏览器覆盖核对项

以下记录迁移开始时仅凭套件名称或文件头无法判断覆盖的用户路径，属于基线盘点；当前实现和验收边界见文末。这里不宣称既有套件缺少全部覆盖。

- 双指界面缩放的完整手势（滑块本身在设置套件里）。
- 机器调色板、保存失败回退；改名、排序和启用操作已有 Hub 浏览器覆盖。
- 登录环境变化表的分支覆盖；重启和忽略已有专门浏览器套件。
- 时间轴同名目录颜色、单机多选拖选、资源六格的数字格式。
- 搜索的 AND/OR、大小写、全词在会话正文高亮上的逐项操作。
- KaTeX 公式、输入历史 ↑ 面板、512 MiB 与离页提醒文案；Codex 侧线程提示已有垂直切片覆盖。
- 回收站的恢复、彻底删除、清空。
- 报告对话框里的模型、推理强度和描述键入（上传补救已有套件）。
- 控制台「粘贴文件」打开后的粘贴。
- 主控制台录制条的倍速和拖动进度。
- 独立 `grid.html` 的连接、抢占、粘贴、历史加载。
- PWA 安装提示；版本卡片已有 popup/draft 浏览器覆盖。

`tests/terminal_grid_render_browser.py` 含像素断言。迁移验收沿用会点击、输入、提交的现有套件，不把截图像素当成新标准。

## 入口与验收边界

`web/` 为完整 Vue 独立入口，组件和具名服务的当前接线见
[架构合同](architecture.md#独立-vue-前端)。新入口已移除 `web/src/compat/`。
生产仍使用 `legacy-web/` 及其 `framework/settings.js`，本重构不切换生产引用。
`2552436` 记录目标开始时的行为基线，后续已接受的旧入口修复同步到独立入口。

视觉复用现有 `style.css`、`typography.css`、`grid.css`、`records.css`、
`session-resources.css`；不引入像素补偿。验收锚点为本文中的浏览器操作与临时 loopback，
不增加单元测试或产品检查。FileDock、ptyhost 线协议及克隆/移动的字节身份改写
继续按各自合同执行；已注销的 Service Worker 离线壳不会因重构恢复。
