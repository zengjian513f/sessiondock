# 架构边界

## 当前可运行链路

第一阶段默认链路：`legacy-web/` → Axum API → 有界 blocking 工作池 →
`sessions` 原生记录解析 / 版本缓存；详情增量通过 SSE 返回。
静态资源在启动时读取为内存快照，HTML 注入模式、build 与能力；
请求不访问静态目录中的动态路径。无需 Node.js 服务；发布时用现有
Vue / Vite 工具链构建设置面板，生成资源随同一个静态快照发布。

`ptyhost-client` 是独立的异步本地协议库；仅配置私有 host 目录时，HTTP才接入
claim/WS传输。受控host匹配full SID/UID，缺少/冲突证据保留unknown；
关联快照本身不授权控制，claim重新观察唯一关联并固定instance，随后只使用
guarded attach。legacy已接手动控制台；受控创建需要额外显式配置，发送走服务端会话服务。
Vue `web/` 已接管设置中的外观和功能面板：组件复用现有 CSS、ID、文案和控件选项，
通过小型 bridge 调用原偏好、缓存、PWA 和休眠逻辑。旧控件事件绑定已移除；
dialog、标签及机器面板仍由 legacy 管理。后续批次及依赖见
[前端重构计划](../TODO.md#vue-重构计划2026-10-03)。

`npm --prefix web run build:legacy` 将 Vue 和组件打包为
`legacy-web/framework/settings.js`，供本地浏览器验证使用，生成目录不提交。
生产部署从与页面相同来源的 `web/` 源码快照构建到 stage 的 `web/framework/`，
不依赖开发目录里残留的生成文件。现有 `web/` 独立演示入口暂不参与生产。

完整重构使用独立的 `web/migration/index.html` 入口：
`npm --prefix web run build:migration` 生成 `web/dist-migration/` 静态目录，
由临时 Rust 服务的 `SESSIONDOCK_WEB_DIR` 指向该目录。`WorkspaceShell` 提供
原页面结构，机器设置及客户端矩阵由 Vue 组件、独立状态和请求 service 管理。
独立入口的顶栏按钮/更多菜单、列表视图切换、侧栏收起和拖动、手机列表/详情切换、
完整设置对话框及标签页已由 Vue 管理。缩放、捏合和可见视口逻辑移入 shell service，
保持原断点、测量和偏好键；业务对话框暂通过具名操作打开。
相应旧渲染和事件绑定已退出新入口；其它尚未迁移区域暂由
`web/src/compat/` 独占。构建复用现有 CSS、字体、vendor 和 Grid 资产。

浏览器夹具通过 `SESSIONDOCK_TEST_WEB_DIR` 选择这个目录，验证运行器相应
构建新入口；未设置时仍验证现有生产前端。第一批独立入口的设置、客户端
更新、顶栏折叠、深链、身份复制、资源列、Hub 机器操作及原生会话/SSE/手机
恢复共十套 Chromium
验收通过，未运行 unit test。实际操作范围见
[迁移行为基线](frontend-migration-surfaces.md)。

顶栏/设置交互迁移后，十套相关 Chromium 验收通过，包括顶栏逐宽折叠、收起左栏和
手机详情页的全局操作、深链恢复、机器控件、缩放/捏合与 Grid/xterm 软键盘适配。
拖动脚本原有的取消/中途宽度断言在旧入口也失败；按当前行为改为拖动期间只移动
引导线、松手提交宽度、取消保留已保存宽度，并在两入口验证。

独立入口的侧栏行、分组标题、机器/客户端筛选、计数、多选条、分组编辑/成员菜单和
资源格已由 Vue 组件管理；树/时间/分组计算在 domain，局部 UI 快照在侧栏 store。
保留关闭组按需构建、子树局部展开、节点复用、文字选择保护和滚动位置。
运行状态更新保留临时会话新建时的运行展示，后续跟随终端列表；停止后不保留运行标记。
相应旧 DOM 构造、chip/资源格渲染与控件绑定已删除，列表同步和请求暂保留具名接线。
十七套侧栏相关 Chromium 操作验收通过，涵盖单机/Hub 嵌套、分组、多选、停止、
未读、资源、深链、增量列表、UI 事件、折叠组及路径布局。
验收脚本修正了已存在的复制标识菜单项、Vue 片段标记选择器、无查询参数列表请求
夹具，以及 Hub 乐观星标先于目录增量返回的等待时序；后三类原行为均已对照旧入口。

独立入口的快速筛选/全文搜索控件、选项、AND/OR、进度和机器进度、结果摘要、退出及
会话内命中导航已由 Vue 管理。词法/全词计算在 search domain，NDJSON 请求、取消、
批次绘制和正则 Worker 在 search service，保持原默认值、请求、提示与偏好键。
全文搜索仍只筛左栏，保留右侧正文、折叠和阅读位置；旧实现与相应事件绑定已移除。
六套 Chromium 操作通过：`search_browser`、`search_no_fold_browser`、
`search_uuid_browser`、`hub_browser`、`groups_browser` 和 `header_fold_browser`。

按 2026-10-03 最新用户指令，本重构 goal 只提交和推送代码，不部署、不重启
生产，也不切换生产入口或资源引用。goal 开始时的生产基线为 `2552436`；完整替换及过渡
脚本清理仍按根目录计划继续执行。

独立构建中的 `grid.html`、`records.html`、`file.html` 与 `files.html`
已分别使用 `web/src/pages/` 下的 Vue 页面。Vue 管理工具条、列表和可见状态；
Grid/xterm 实例、连接、字节解析及回放仍由页面 controller 管理，复用既有
Grid 模块。原辅助页的三份过渡脚本已删除，生产目录仍保留原页面。
`build:migration` 同时构建这些辅助页并放入同一静态目录。
`terminal_grid_browser`、`terminal_records_browser` 和 `files_browser`
在独立构建上通过实际连接、输入、滚动、回放及文件跳转验收。

独立入口的消息主体由 `components/conversation` 管理文本、工具组、过程、差异、
题卡、媒体、历史缺口和排队回执。规划、消息索引、分页合并与原校验在 domain，
折叠和懒展开状态按视图保存；已有 Markdown 内文仍使用原格式化与 Worker 高亮。
请求、缓存与 SSE 暂保留具名服务边界，B8 会继续移除过渡全局接线。
展开全文按内容更新内文片段；分页渲染期间仍保留已接受的实时后缀和 checkpoint。
历史合成语料及 Chromium 历史/分页、工具折叠、长代码、媒体、搜索、题卡、发送与
性能路径经过验收。媒体脚本按现有窗口大小补足竞态触发和图片解码后的滚动测量，
同一脚本在旧入口也通过；附件删除先等待该批上传完成，保留既有“取消上传”语义。

独立前端的输入区由 `components/composer` 接管，草稿同步、附件 staging、
CHECK/SEND、回执和题卡写入保留在 `services/composer` 原服务逻辑中。
十套 Chromium 操作覆盖发送、跨页草稿、附件重试、启动选择题、Esc 和终端粘贴。
会话操作、弹窗、报告与附件、模型/目录选择、停止/冻结、移动/复制及回收站
由 `components/session-ui` 管理；UI 状态绑定在 store，请求与任务时序在 scoped service。
输入草稿回填经过 Vue 状态再测量，协议对象与草稿仍保持原身份。
新建、停止、冻结、回收站、传输/环境提示、跨机器报告、短屏滚动、草稿同步与
发送路径经过 Chromium 验收。顶栏验收保留操作和元信息顺序、菜单可达和不裁剪，
不要求相同的像素填满阈值。过渡调用桥接在 B8 收尾移除。
生产引用暂不切换。

主页面终端的容器、分界拖动、输出提示、回放栏、快捷键和粘贴/查找菜单由
`components/terminal` 管理。Grid/xterm 实例、WebSocket、租约、同步帧缓冲、
录制和重连时序留在 `services/terminal` 的非响应式控制器中，复用原协议和样式。
十八套 Chromium 路径通过，覆盖连接、键盘/粘贴、选择、抢占、心跳、退出、诊断、
滚动、回放、Grid 主题/绘制、输入区衔接、偏好和休眠。仍保留临时具名调用别名，
与其余过渡脚本一起在 B8 收尾清理；生产入口未切换。

资源弹窗、探测展示、休眠、文件菜单与媒体错误面板由 `components/overlays` 管理。
原资源租约、活动计时、休眠网络暂停、文件请求与媒体诊断/重载时序留在对应 service。
gallery 与 Markdown 内嵌图片共享原视图身份规则，失败、重试和重新载入的入口保留。
资源、探测、偏好、休眠、文件和媒体解码/隔离经过 Chromium 操作验证。
新入口也同步当前已接受的 Windows 路径识别和终端同步帧延迟输出修复。

host 输出由每客户端独立有界队列隔离慢读者，退出完整性与进程身份分别验证。

独立的 `LaunchTarget`/host launch guard 与同步 `lifecycle` 回执库：
前者在没有SID/UID时验证真实启动实例，后者在持久化Prepared/Starting后才返回
一次性授权，并将崩溃后的Starting恢复为Uncertain。已接显式版本化
launcher、排队 coordinator 和 pending HTTP/WS；独立 child reaper 保留
进程句柄，Web退出不杀host。取消先保存意图、退休输入租约，再一次guarded kill，
未确认退出仍是Uncertain；4002退休通知不冒充进程退出。
native BoundTarget不接受launch身份替代，详见
[生命周期接线合同](lifecycle-integration.md)。

host内有独立的write-once native binding，不修改immutable meta/record。
服务schema3保存操作者确认的关联意图，guarded Info才能确认完整原生身份；
恢复与离线先降Uncertain。NativeScope证明记录身份，不证明进程归属，因此禁止
自动猜测；新绑定使用同snapshot的真实ID目录，旧metadata目录与其分离。
pending租约不升级，衍生native租约仍受launch退休及持久取消约束。

读工作池 `SESSIONDOCK_READ_WORKERS`（默认 `clamp(核数/2, 8, 32)`）个 blocking
worker；请求排队直到取得许可或自身取消。探测、
历史页/媒体/文件写/生命周期响应池按比例派生（表见
[performance.md](performance.md#并发预算)）；搜索不占读池。
同一文件版本复用解析结果。`/api/meta` 声明 `stage:"replacement"`、
`read_only:false`——这是替代服务，前端没有常驻横幅。
`observe::WatchHub` 为每个 `(uid,agent)` 共享一次500ms版本读取，最多2个后台
工作准入；Tokio watch只保留最新不可变快照，每个浏览器按自己的checkpoint
生成增量，慢读者不堆积旧版本。首次并发订阅合并、最后订阅回收、错误重试
使用代际标识防止旧任务清理新任务。共享槽的语义见
[Tokio watch](https://docs.rs/tokio/latest/tokio/sync/watch/index.html)。
完整checkpoint与原始前缀digest按视图缓存；追加从上次已提交偏移续读，较早
checkpoint仍核对投影。

## 历史读模型

设计以 [read-model.md](read-model.md) 为准（惰性索引 + 按需视图）：

- `sessions/index`：列表只做目录遍历、`stat` 与每文件有界头/尾摘要（头 96 KiB
  ≤ 40 条、尾 512 KiB，与 `list_sessions` 同一推导），按 stamp 缓存，
  并行读取；没有启动解析，没有会话数/总字节上限，单个文件的变化只影响它自己。
- `sessions/views`：只在打开会话时经 `records`/`native_input`/`providers` 流式
  解析这一个文件，增量续读，进有界 LRU；游标、锚点、pin、原生输入证据等每视图
  契约不变（[history-pages.md](history-pages.md)、[native-input.md](native-input.md)）。
- `providers/` 只投影传入的原生记录，不打开文件；`index/graph.rs` 的归属图从
  摘要推导父/子关系，请求的 `agent` 和记录中的父 ID 从不拼接成任意磁盘路径。
- `sessions/mod.rs` 的 `SessionStore` 只是门面：列表 = 索引行 + 元数据装饰 +
  重签名（打开会话不改变 `sig`）；打开 = 从索引取候选文件，经 `views` 打开；
  运行时目录、回收站文件集、续接身份都取自索引，不解析文件。
- 普通读模型不修改原生数据。用户确认的整组复制、移动和回收站操作按各自合同
  发布、移动或清理文件；克隆保留源组，只重写目标必需的身份、路径和依赖偏移字节。
  见 [移动与克隆](session-move.md)、[回收站](trash.md)。

## 搜索与工具展示

- 搜索对完整语义正文匹配（按 `updated` 倒序），不对原始 JSONL 匹配；正文来自
  按文件版本持久化的搜索文本缓存（[read-model.md](read-model.md#搜索)：
  显式 `SESSIONDOCK_SEARCH_CACHE_DIR`，LRU 上限），只有版本变了的会话才重新投影，
  投影不留驻；不可读视图产生显式部分失败（并按版本缓存），不能把跳过的会话算作
  完整搜索成功。
- 搜索请求等待自己的准入许可和解析 worker
  （`SESSIONDOCK_SEARCH_WORKERS`，不占普通读池）；
  取消响应会取消工作并释放阻塞发送，blocking permit保持到实际工作结束。JSON大对象
  序列化也在worker。
- 字面搜索按空白拆词、双引号保留短语，默认 AND（整个会话内全部词出现），
  `mode=any` 选择 OR。每个词用字面匹配器和 Unicode 全词边界（汉字、假名、谚文与其它字母之间断开），复用缓存预筛，
  不合成为回溯正则；前端搜索框内的 AND / OR 按钮切换“全部词 / 任一词”，
  与大小写、全词、正则按钮保持单排布局。
  `regex=1` 不拆词、忽略 `mode`，由 `fancy-regex` 支持 lookaround/backreference。
  详见 [read-model.md](read-model.md#搜索)。
- 工具changes是原生参数的纯投影，不读取被编辑文件。完整Write/片段Edit和
  patch保持原有before/after可信程度；生成差异超预算时保留原始参数并解释原因。

## 后续实现原则

文件读取由当前会话引用或已签发路径引用确定目标；兼容配置中的 file roots
不充当授权边界。路径解析保留目录能力句柄，读响应保留已检查文件；按 HTTP 消费者
需求每次只在blocking任务读取64KiB。未轮询Body不开始读，取消后尚未结束的
读取继续持有permit，避免慢下载挤占普通历史worker。文件浏览及独立预览由
[FileDock](files.md) 提供；SessionDock 保留兼容文件作业 API，`file_thumbnails` 为 false。

可选的 `MetadataStore`：每次操作重读当前文件，并以原子替换持久化。
列表、详情、搜索和 SSE 从同一元数据版本装饰读模型；偏好变化不修改消息 anchor。
legacy增量补接元数据变化，只更新缓存/标题栏，不重绘已有消息正文；子代理名称
更新已经过无列表刷新、无reload的实际SSE/DOM回归。

1. HTTP / SSE / WebSocket 是传输层，不承担 CLI 历史解释或发送确认逻辑。
2. 接口 DTO 和业务状态分开；缺失字段与显式 null 必须有明确含义。
3. 消息同步、队列确认、重复/乱序处理置于独立状态机，并用事件序列测试。
4. Vue 按模块接管视图，Pinia 按会话、机器、偏好等拆分；未迁移区域继续由
   legacy 独占。已迁移控件删除旧绑定，不复制旧全局大对象到 Vue。
5. xterm 和 WebSocket 的生命周期由终端管理模块负责。终端字节不经过
   全局深层响应式状态；切换视图不等于销毁连接。
6. ptyhost 继续每会话一个独立进程。Web 服务重启不得终止 CLI 会话。
7. Rust 浏览器接口可与新前端一起演化；旧 Hub 兼容性需要单独的
   适配层和契约测试，当前健康检查的版本字段不代表旧节点协议兼容。

当前实现合同以本文件及相应模块文档为准；未完成工作只记录在根目录
[TODO.md](../TODO.md)，路由清单见 [route-ledger.md](route-ledger.md)。
