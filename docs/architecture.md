# 架构边界

## 当前可运行链路

当前生产链路：`legacy-web/` → Axum API → 有界 blocking 工作池 →
`sessions` 原生记录解析 / 版本缓存；详情增量通过 SSE 返回。
静态资源在启动时读取为内存快照，HTML 注入模式、build 与能力；
请求不访问静态目录中的动态路径。无需 Node.js 服务；发布时用现有
Vue / Vite 工具链构建设置面板，生成资源随同一个静态快照发布。

`ptyhost-client` 是独立的异步本地协议库；仅配置私有 host 目录时，HTTP才接入
claim/WS传输。受控host匹配full SID/UID，缺少/冲突证据保留unknown；
关联快照本身不授权控制，claim重新观察唯一关联并固定instance，随后只使用
guarded attach。legacy已接手动控制台；受控创建需要额外显式配置，发送走服务端会话服务。
生产 `legacy-web/` 的外观和功能设置面板使用现有 Vue 构建，保留其调用接口。
`npm --prefix web run build:legacy` 将该面板打包到
`legacy-web/framework/settings.js`，生成目录不提交。部署工具仍显式构建这一路径。

## 独立 Vue 前端

完整入口为 `web/migration/index.html`。`npm --prefix web run build`（或
`build:migration`）生成 `web/dist-migration/`，同时包含主页面、`grid.html`、
`records.html`、`file.html`、`files.html` 和正则/语法 Worker。构建复用现有 CSS、
字体、vendor、Grid 和 PWA 资源。无需 Node.js 运行服务。
`npm --prefix web run dev` 监听源码并重建静态资源；HTML 能力注入及资源快照仍由
Rust 提供，开发时在重新构建后重启自己的临时服务。

`components/` 管理页面框架、设置、机器、侧栏/分组/右键菜单、搜索、消息、输入区、
会话操作、弹窗/报告、资源/文件/媒体提示和终端控件。保留原文案、图标、样式、
布局断点、键盘与触摸操作、能力开关和错误说明；顶栏按原顺序折叠并保持菜单可达。
辅助页面在 `pages/` 内管理其工具条、列表、目录及可见状态。

主页面静态组件树从 `App.vue` 挂载：页面壳直接包含输入区，设置、常驻会话弹窗和
提示通过 Teleport 放入原容器，保持原有 DOM 层级及样式。DOM 挂载后同步创建控制器，
再启动服务。资源、休眠、文件及会话正文等动态表面的独立挂载仍由现有服务管理。

`stores/` 使用 Pinia 按目录、选中视图、未读、搜索、偏好、机器和各 UI 表面拆分。
`services/runtime/` 组合列表读取、缓存、历史分页、SSE、未读、节点、分组、事件和
轮询生命周期；消息与请求原对象保持身份，界面只投影需要显示的状态。
`domain/` 保留列表增量、树/分组/时间计算、消息规划与索引、分页合并和原协议逻辑。
正文格式化、语法高亮、公式及懒展开继续使用原实现。

搜索由一个 Pinia store 同时供组件和业务服务使用；保留输入原文，`term` 从其去除
首尾空白后派生，结果摘要使用 computed。结果数组保持浅响应式，保留原会话对象
身份；`null` 表示本地筛选，空数组表示尚无命中的全文搜索。选项、进度及状态提示
直接更新共享状态，不复制运行时/UI 两份状态，也不模拟 DOM 的 textContent/classList。
搜索服务保留请求代际、AbortController、Worker 取消和分批结果时序；侧栏列表仍由
现有控制器刷新，搜索不会重新渲染已打开的正文。

`services/composer/` 保留草稿同步、附件 staging、CHECK/SEND、排队回执及题卡写入；
`services/session-ui/` 管理新建、停止/冻结、模型/目录、报告/上传、移动/复制及回收站。
`services/terminal/` 管理 Grid/xterm、WebSocket、租约、同步帧缓冲、录制、回放与重连；
终端字节和实例不进入深层响应式状态，切换会话保留原连接和视图规则。
媒体和文件服务保留原请求、视图身份、诊断、重试和重载时序。

新入口直接组合上述具名服务，不加载旧业务脚本，不使用全局 `S`/`T` 或过渡函数别名。
已删除 `web/src/compat/` 和未使用的演示页面、演示健康状态及 Vitest 依赖。
主页面的 `window.SessionDockRuntime` 指向实际应用服务对象；浏览器夹具由此访问
实际状态和操作。Grid/录制页仍保留其原渲染实例引用。
功能、默认值、存储键、请求和原有检查保持基线，不因重构增加产品检查或拒绝策略。

浏览器夹具通过 `SESSIONDOCK_TEST_WEB_DIR` 选择完整构建；未设置时验证生产旧入口。
验收使用 Chromium 中实际点击、输入、提交、终端连接、文件选择及历史/媒体竞态，
并覆盖 `/sessiondock/` 子路径和临时服务重启后的会话、偏好恢复；不运行 unit test。
操作范围见 [迁移行为基线](frontend-migration-surfaces.md)，运行方式见
[验证合同](validation.md)。

独立 Vue 入口通过隔离构建验收，生产继续使用 `legacy-web/`；常规发布保留现有入口、
代理和资源引用。生产切换与旧入口退役作为单独的最终阶段，不随中间重构批次执行。
同一工作区内已接受的终端同步帧、同 SID rollout 跟随、Windows 路径及 CLI 说明修复
同步到新入口。

## 后端与宿主

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
3. 消息同步、队列确认、重复/乱序处理由独立服务管理，以实际浏览器事件路径验证。
4. 独立前端由 Vue 按模块管理视图，Pinia 按会话、机器、偏好等拆分；同步、
   缓存和终端协议由具名服务管理。生产旧入口保持当前引用。
5. xterm 和 WebSocket 的生命周期由终端管理模块负责。终端字节不经过
   全局深层响应式状态；切换视图不等于销毁连接。
6. ptyhost 继续每会话一个独立进程。Web 服务重启不得终止 CLI 会话。
7. Rust 浏览器接口可与新前端一起演化；旧 Hub 兼容性需要单独的
   适配层和契约测试，当前健康检查的版本字段不代表旧节点协议兼容。

当前实现合同以本文件及相应模块文档为准；未完成工作只记录在根目录
[TODO.md](../TODO.md)，路由清单见 [route-ledger.md](route-ledger.md)。
