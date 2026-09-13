# 导入基线

- 来源仓库：`agenthub`
- 来源提交：`ee2e373c134f4f16a3452ba35fa5346ea441fb9f`（host 导入基线）；前端冻结快照自第三十七批起为
  `e5b023a0c1e552286341878b35b92ae03799eb13`（2026-09-12 21:23，见下文"第三十七批"）。
- `host-rs/src/` → `crates/ptyhost/src/`：源码原样导入。
- `host-rs/Cargo.lock`：作为根 workspace 锁文件的初始基线，再解析新增服务依赖。
- ptyhost manifest：移除成员级 release profile，统一使用根 workspace profile；
  设置 `publish = false`，不发布包。未改变 host 的运行协议或默认路径。
- `agenthub/static/` → `reference/legacy-web/`：完整静态资源快照（当前为 `e5b023a`），含原有第三方许可
  （`fonts/LICENSE-Cascadia-Code.txt` 按本仓库 `text=auto` 规范化为 LF）。此目录不会进入新前端构建，不由新服务对外提供。
- 未导入 Git 历史、Python 后端、部署文件、凭据、运行数据或生产配置。

`web/` Vue 健康页保留为第二阶段骨架；第一阶段不继续框架重构。
冻结快照不是永久同步副本，重要修复按需移植并记录来源。

第十四批新增Rust `history_pages`能力：legacy中间历史按钮按200事件/媒体与JSON
预算逐页补齐，不再为Rust一次请求所有省略历史。页游标不覆盖实时SSE游标，
失败保留快照并提供显式恢复；Python无此能力继续原行为。详见
[历史页合同](history-pages.md)。这不是Vue重构或大原生图片支持。

## 第一阶段：legacy 接 Rust

- `reference/legacy-web/` → `legacy-web/`，第一阶段只服务后者，前者未修改。
- `capabilities.js` + 服务端 meta 声明只读能力与 `sessiondock.` 存储命名空间。
- 旧 HTML 模板/资源顺序保留；仅添加能力脚本、只读提示及主题存储命名空间。
- 关闭未支持的自动 audit/live/outbox/文件解析；运行状态显示未知，不清理旧队列。
- SSE/读取失败保留旧快照并明确提示，通过显式成功读取才恢复观察。
- 控制台原来的灰色、可点击与错误解释逻辑保持；未接入能力不伪装可用。
- 新后端有意使用独立 schema 的 byte cursor/完整前缀 anchor，避免与 Python
  的历史缓存误续读；不是旧 Hub 协议的节点替代。
- Vue 只扩大健康响应的 stage 校验以接受 `read_only`，未新增视图或业务功能。

## 第二批：原生历史语义

- 后端补 Claude 活动树/last-prompt/compact，以及 Codex 固定前缀分叉和两家
  子代理；使用现有 messages/watch 的 agent 参数，无前端框架或业务源码改动。
- cursor schema 提升为 `rs-m1-2`，anchor 包含语义投影和固定继承前缀。
  换枝或父前缀改变时 reset；旧游标有意失效，正常叶追加仍增量。
- 无法确定的关系不猜测：缺父/坏切点等 501，歧义 ID 409，错误 agent 404。
  循环/歧义子代理公开为 unsupported 项；父会话不在索引里的孤儿子代理自第三十五批起
  与 Python 一样不列出（按 uid 打开仍是 501）。
- 复杂 fork 列表暂不返回不正确的 leaf-only cursor；已打开详情有完整逻辑
  cursor，后台精确未读计数待后续索引阶段补齐。
- 固定前缀严格要求完整 JSONL 行（行边界 / 越界 cut 仍 501）；这是明确的安全收紧。
  未知的记录/attachment/事件/内容块**类型**（第三十三批）、坏行、重复 `session_meta`、
  Claude 缺失祖先/环/缺失叶子（第三十五批）都与 Python 同样跳过或截断并计入非致命
  `migration_warnings`。
- Claude 真正回滚涉及 CLI 屏幕观测和持久 pin，本批只支持已落原生日志的
  分支信号。没有开放 rewind/send/terminal 写操作。
- 回归工具仅用自建临时记录；原 Python adapter 只作为可选差分 oracle，
  不成为 Rust 的运行时依赖。

## 第三批：共享观察、搜索和工具展示

- `rs-m2-1` 将叶投影与固定祖先前缀身份组合成语义anchor，fork列表与详情
  使用同一公式，不需要为了列表重解析全部父正文。旧Rust游标会正常reset。
- 搜索直接沿用现有UI/JSON/NDJSON接口。仅对Rust正则按钮增加方言说明：
  lookaround/backreference不支持，Unicode大小写/字符类可能与Python不同。
- 工具摘要/changes继续使用原有卡片；新增 `changes_unavailable_reason` 时
  用textContent显示预算说明，参数仍能展开；不含该字段时Python行为不变。
- 共享观察补测发现，纯子代理名称变化会被旧SSE包过滤且legacy忽略增量meta。
  HTTP与Chromium复现后，后端改为发布完整revision变化，Rust页接收权威meta
  并只刷新标题/菜单；不重绘消息，不改变Python页面的处理策略。
- 不能用原项目最新 checkout 的差异误判冻结 reference 被修改：以 `reference/legacy-web` 记录的快照提交为准。

## 第四批（进行中）：隔离偏好与终端传输

- 新 `metadata` / `terminal_transport` 独立能力按实际配置注入HTML和meta，
  也参与build标识；不会因能保存星标就声称已支持原生写操作或可靠发送。
- 星标/父会话显示沿用原API与控件；有效visibility子集一次原子提交，无效项
  单独报errors，写盘失败整体返回失败。这比原先逐行写入具有更强的批次边界。
- 开发通知明确区分“原生记录只读”与“独立偏好可保存”。双页星标SSE、移动
  控制台、偏好重启恢复和原生文件不变已通过隔离浏览器验收。

迁移计划与各里程碑状态见
[BACKEND_MIRGRATION_PLAN.md](../BACKEND_MIRGRATION_PLAN.md)。

## 第五批（进行中）：受限文件、受控进程与Claude domain

- `files`只在显式独立授权根启用；原Python的全文件系统浏览权限没有导入。
  目录入口和每次导航重核对所选会话分支，永远`writable:false`。
- `files_jobs`和`file_thumbnails`单独声明false。实际Chromium先复现旧页面仍发
  jobs/thumbnail请求，再加最小能力门控；传输任务按钮仍可打开具体未实现说明。
- resolve返回的逐项错误用于手动打开/右键菜单，使用文本展示，不再吞掉越界/
  未引用/歧义等具体原因。Markdown和下载验证均使用临时合成文件。
- `/api/live`仅添加`managed`部分观察，旧全局状态仍unknown；这不启用旧console
  或根据name前缀/cwd猜UID。既有host为空meta时仍可观察，但无法关联。
- Claude发送状态机与Codex独立；当前缺可靠原生request关联，未开放HTTP发送。

## 第六批（进行中）：host实例闸门

导入host首次有意扩展：`guarded_v1`为新增可选操作，保留旧操作和独立进程生命周期。
它先校验实例/原生身份，再执行同一Session上的内层命令。只添加expected字段到
旧操作不安全，因为旧host会忽略字段后先执行；新操作让旧host明确拒绝且无降级。
Info能力和ACK格式详见 [终端实例合同](terminal-identity.md)。
4项纯测试、真实临时shell的错误实例拒绝/合法回放输入/同名重建，以及原Web重启
和xterm场景已验证。没有升级生产host；除main/session/新guard外保持导入源码原样。

legacy 控制台现沿用原按钮、xterm、抢占确认框，但 Rust 能力分支必须使用完整
UID/instance，禁止名称或短 SID 猜测关联。持久布局附带绑定身份，同名替换不会
自动恢复旧视图。新建/可靠发送仍分别关闭；不影响无能力声明的 Python 页面。
真实隔离浏览器已覆盖正常输入、两页面抢占、移动导航、Web 重启和原生字节不变。

显式 Codex 名称索引及 Grok summary-only 元数据分别见
[名称索引合同](codex-names.md) 和 [Grok 元数据合同](grok-metadata.md)。

## 第七批：输出隔离与只读账本接线

host 有意扩展范围新增 `output.rs`、`transport.rs`，并修改 `session.rs` 的
订阅/发布/退出路径。每客户端独立有界队列与写线程，ACK→replay→live→exit
保持顺序；慢客户端不阻塞 PTY 发布。退出等待实际 EOF，超时显式报告输出
不完整，不把子进程退出等同于输出读完。具体预算见 [host 输出](host-output.md)。

旧页面的退出误重连已在真实 Chromium 复现并修复：保留尾部 xterm 输出，
停止对已退出实例自动申请租约，灰色控制台按钮继续显示具体原因；不清理草稿
或 outbox，也不把仅断流误判为进程退出。桌面/移动端均已隔离验收。

账本增加异步启动恢复和有界只读 outbox API，见 [HTTP 合同](delivery-http.md)。
原生操作身份来自独立 NativeScope 证据，不复用展示 SID/文件名 fallback。
`outbox_read` 不意味着可靠发送、CLI 创建或 M5 整体完成。

第八批补测发现 `live:false` 还会让旧页面停止终端列表刷新。Rust 现对已配置的
手动终端独立每3秒观察一次（等待前次完成、隐藏页面暂停），不启用全局进程
发现、不发自动claim。同名新实例通过真实浏览器验收：退出说明解除，但旧布局
和租约不会继承，用户明确点击才创建新的xterm/实例绑定。

## 第九批：受控创建与 pending 控制台

沿用 legacy 创建弹窗、pending 侧栏和移动端停止菜单；仅 Rust 能力分支使用
record/launch/instance 三重身份，不伪造原生 UID、不轮询猜测新文件归属、不自动
删除 pending 回执。显式私有 launcher allowlist 与已初始化账本均配置后才启用；
浏览器不能传入 argv/env/可执行程序。协议见 [生命周期 HTTP](lifecycle-http.md)。

取消使用独立 4002 `launch retired` 通知，保留 xterm 尾部并停止自动 reclaim；
不把权限退休当作进程退出。停止与弃置分离，不能以清理页面为由删除持久回执。
真实免费 shell 的创建、幂等重放、Web 重启保活和移动取消均已隔离验证；
原生绑定与可靠发送仍未开放。host 停止额外在同一 child 锁内检查存活再发信号，
已回收或查询失败时不使用旧 PID；不枚举或杀未知后代进程。

## 第十批：操作者确认的原生绑定

新增小型关联确认弹窗及“释放本页控制台”操作，复用现有dialog样式，不引入框架。
pending与native同名时，记忆布局/重连继续固定原种类及完整身份；新绑定出现不
自动升级连接、不迁移草稿。source/真实SID由后端验证，页面只提交UID及明确确认。
绑定后取消同时退休衍生native租约，Web重启也须校验持久取消；与native确认/发送
状态机分离。详见 [绑定合同](lifecycle-binding.md)。

移动Chromium检查同时发现开发提示在已配置create后仍说“创建未启用”，已改成
能力对应的文案；未配置时继续明确禁用。确认弹窗的窄屏边界及现有主题样式已验证。
测试使用独立浏览器存储区分页面自动恢复接管与另一页面取消，不借此隐藏正常行为。

额外浏览器回归先复现“跨种类打开被拒绝但已写错持久布局”，修复为在任何布局或
pane变更之前检查既有连接类型；拒绝后旧pending socket及其完整保存身份均保留。

## 第十一批：本地原生 PNG/JPEG

沿用原有`media`/gallery渲染，不引入新框架。Rust页面声明`media_remote:false`：
正文Markdown里的外链图片不自动联网，保留图片文字占位；普通显式外链导航不变。
没有此能力声明的Python页面保留原行为。原生内嵌数据只由后端转换为进程内随机
token，不把data URL或base64直接交给浏览器；历史中的磁盘路径不是读取授权。

消息窗口选择先于媒体注册，搜索和输入历史只消费文本。图片载荷作为typed私有
事件字段参与视图预算和语义摘要；随机token不参与cursor，以免重解析造成假reset。
媒体缓存不是永久文件仓库：淘汰/服务重启后旧token可404，重新读取消息重新注册。
完整的三家媒体格式与授权磁盘路径在第十二批继续实现；本批不代表读模型完整兼容。

三家结构化工具结果和明确MCP根content包装沿用原gallery，isError与原call/turn身份
保留。普通字符串JSON教程、工具参数和业务content数组不按图片猜测；已知Codex
执行信封使用候选/扫描预算而不是逐行反复解析。最终构建已通过真实Chromium图片
解码、分叉/子代理隔离、刷新/SSE/移动、外链零自动请求和native字节不变检查。
窗口中省略的图片、搜索和已消费追加前缀均不触发解码；接口及预算见[媒体合同](media.md)。

## 第十二批：扩展图片与授权磁盘来源

格式扩展为GIF/WebP/APNG/静态AVIF/BMP，保留原legacy渲染和容器/像素预算；
不引入前端框架或系统图片解码器。并非所有AVIF/JPEG/BMP编码特性均支持，具体
子类型与动画限制见媒体合同。

显式结构化本地图片引用、正文Markdown图片和聊天裸路径可经开发文件根读取。
完整当前分支/子代理索引授予引用范围，分页窗口只决定实际读取哪些图片；cwd、
扩展名和历史里出现路径本身不是磁盘权限。文件名不作为媒体URL，私有结构化路径
不进入公开消息；正文已展示的引用仅用于inline替换。

磁盘token在重新投影时重新产生，GET/HEAD再次读取当前会话授权、核验文件身份和
版本，文件替换使旧token返回409。已交付的字节不能追溯撤销；本批不承诺自动监听
图片文件变化，需显式重载消息取得新token。未配置、不可访问或格式无效的磁盘图片
在原位置显示解释，保留文字和其它图片。外链仍不自动请求。

格式浏览器回归先复现移动错误页没有返回列表按钮，最小修复只补loading/error
占位头部的返回按钮；控制台持续可见且可点击解释，不改变正常终端能力或页面架构。

## 第十三批：追加JSON复用与媒体语义差分

追加优化仅复用完整相等committed字节对应的JSON AST，旧partial尾部重读；provider
仍全量投影。新增缓存独立计费、可淘汰、不被历史View Arc长期持有。ctime仍用于
失效检测，稳定file identity单独比较。改写/截断/错误/超预算回退冷解析；没有将旧
Event拼接作为时间线真值。性能收益与冷启动成本并列记录于[追加缓存说明](append-cache.md)。

真实Python adapter/media差分发现并修复Claude混合块被合并成一条消息、多图计数
错误，以及Codex/Grok多图或混合文本的额外占位。先在合成会话浏览器DOM重现count/
折叠文本差异，再恢复原生分组语义；保留Rust工具/MCP支持和全部文件授权限制。
具体覆盖与有意安全差异见[媒体差分](media-parity.md)。

大内嵌图与多图历史不是只放宽常量即可实现，后续按[分页设计](media-pagination-design.md)
拆分历史页游标、按需媒体、私有native-span扫描器；本批不宣称其已完成。

## 第十五批：按需媒体与加载错误

消息中的媒体对象改为`src/alt/lazy:true`，不再预先返回MIME或尺寸。历史接口
登记来源，浏览器原有lazy img实际GET时才读图、解base64、检查容器；浏览器
natural尺寸及GET实际bytes/MIME仍受验收。文件版本在登记时固定，冷/热GET
都重新授权；内嵌源独立有界强引用不pin历史View。

`media_lazy:true`仅给Rust能力分支启用局部错误和受控重试。先在真实Chromium
复现GET503只剩破图、没有原因/恢复控件，再增加有界错误诊断；诊断成功返回
image时立即取消body，不在JavaScript再读一份大图。503同token手动重试，
404/409可显式重载有限窗口；不自动清历史或全量重载、不修改Python页面行为。
错误处理、显式重载及并发SSE均须保留实时checkpoint和控制台访问。

独立审查发现`applyDiff(reset)`在分批render期间收到SSE时，缓存顺序正确但DOM
可能把新尾排在旧历史前。实际Chromium复现后，Rust reset统一复用已有缓存entry
协调发布，合并同期新尾/活动状态；Python仍走原默认render路径。此前分页测试的
timer hook只搜索完整调用栈，可能误停在嵌套helper而未暂停真实render；本批收窄
到直接Promise/render调用帧，另断言fragment尚未发布及render代际未变化后重验。
旧宽泛hook结果不能单独作为render-yield竞态已覆盖的证据。

坏容器/帧/像素/MIME不符现在在图片GET返回422/413而非阻断历史文字；结构化
原生source/alias/编码长度错误仍保持早期拒绝。格式和文件边界没有放宽。
这不是大内嵌图、native span或完整读模型兼容完成，后续事项仍在迁移计划。

## 第十六批：可信原生输入与结构扫描基础

原生记录读取现经过 retained checked handle，按块建立每个物理换行的完整前缀
摘要索引；游标schema、字节end、半行和父cut含义不变。索引独立32MiB进程预算
包含旧快照及扩容峰值。生产暂时仍保留原16MiB有界bytes供缓存/父前缀解析，
不能把这一步称为消除整文件内存或支持32MiB图片。

结构扫描器验证完整JSON、UTF8/转义/重复键和独立节点/驻留预算；普通记录移动
到Value，大字符串只得到未授权私有区段，不伪造image或空正文。重复键及超结构
预算记录、新的hardlink/LF边界明确拒绝，具体[输入契约](native-input.md)。

真实工具差分和Chromium发现初版BTreeMap重排参数，改变了摘要选取顺序；初版
Value相等测试没有检查对象顺序，不能作为全语义一致证明。已改保序IndexMap，
复用锁文件已有版本，并补嵌套乱序key/序列化字节完全一致和DOM摘要顺序回归。
不更改legacy JS，不把此修复转成前端框架重构。

## 第十七批：移除原文副本与直接 JSON 构造

生产`Parsed.bytes`已移除。普通记录在读取流中逐条有界解码，缓存复用前对完整
旧committed前缀计算SHA256；不匹配先丢弃推测结果再同stamp冷读一次，读失败
不回退旧记录。SHA1公共游标仍保持兼容。父固定cut改走retained checked前缀
读取，仍校验整个源文件stamp，半行/空行及leaf-only end不变。

`scan_value`与私有span scanner共享同一语法/保序/重复键/预算实现，只改变输出
构造，避免普通记录先建Node树再转Value。没有放宽16MiB原生文件、2MiB单行和
现有内嵌图片大小限制；provider-aware span及授权GET仍未接入。

新`native_streaming.py`用真实HTTP检查三家行边界、同长恢复mtime重写、损坏修复、
工具保序和父cut，逐请求核对合成原生文件未被服务改写。性能/RSS对照记录在
[原生输入说明](native-input.md#batch-17-direct-construction-and-streaming-comparison)：
1万条冷读p50改善约3–15%、追加0–6%、峰值RSS减少4–6MiB；不是全链路提速倍数
或大图验收，也未完全抵消上一批成本。

## 第十八批：结构化原生大图与当前分支授权

真实结构化图片现在走私有span：三家消息及结构化工具/MCP结果只保留来源范围、
规范MIME和完整摘要，不在Value中保存大base64；未知正文、参数和教程扩展不会
因此获得权限。普通字段仍有2MiB限制，剥离图载荷后的普通JSON也独立计数。
原生扫描工作量现为文件/总清单各256MiB，AST/index/descriptor/blob预算各自独立，
不是把全部内存上限一起放大。单图支持32MiB，AVIF仍保留独立1.5MiB解析限制。

GET用canonical UID/精确agent的完整当前分支授权，核对原生candidate stamp后
只读该物理字符串范围，流式JSON解转义/base64，发布前必须完成摘要和checked
handle验证。热缓存同样重新授权/读源验证；不需要、也不扩张external file roots。
普通append保留旧图，固定cut外变动不改变继承内容，cut内改写和换枝撤销旧token。

inline/span/dataURL采用统一image-semantic-v2：跨版本旧含图cursor会有一次明确
reset，但随机token与物理位置仍不作为图片内容语义。公共字节checkpoint/schema
不变。现有legacy懒加载协议直接承接，本批无JS框架重构。

这仅交付结构化大图路径。巨型字符串嵌套工具JSON、单消息超过16图的continuation、
完整后端其余模块仍在计划内，不因本批验收而缩减，也不表示Windows/macOS实机
或生产迁移完成。

本批实测又定位到PNG/APNG校验器整图临时副本，已改为原slice两遍校验并保留CRC。
新增旧filtered对照测试，并明确拒绝以前可能被过滤隐藏的IEND后动画块。32MiB合成
PNG场景服务器峰值约74→48MiB；普通1万条历史冷读p50仍增加约1–6%，无整体提速。
最终完整数据和测试范围见[本批测量](native-input.md#final-batch-18-large-image-measurement)。

## 第十九批：嵌套字符串工具输出与共享回放预算

Codex 工具输出里超过 2 MiB 的字符串（`function_call_output`/`custom_tool_call_output`/
`local_shell_call_output` 的 `output`、MCP `{content,isError}` 包装中的单个文本块、
单元素文本数组）现在先成为私有 `TextSpan`，再由 `tool_envelopes` 按旧小字符串
parser 的候选顺序（最后一个 `Output:\n`、首个非空白、最多 32 个对象行）流式重扫；
只有带 `output`/`wall_time_seconds` 且含 `exit_code`/`session_id`/`chunk_id` 的完整
对象才被接受为信封。普通消息正文、工具参数、未知巨型对象、重复键、第九层嵌套
和跨多个文本块的组合仍明确失败（501），不会被当成图片或空文本。

被接受的信封替换为已校验的私有结构树，继续沿 `output` 向内查找结构化图片；
图片保留一条不可变的 `DecodePlan`：一段真实外层物理范围加至多八段内层
`StringRange`，内层偏移只针对上一层解码后的文本，绝不是文件字节偏移。
一次记录投影中的全部候选扫描与重开共享 512 MiB 工作预算，GET 回放另有独立的
512 MiB 预算；层数不会补充预算，两层 32 MiB 图片会让历史请求明确返回 413。
每次候选打开都必须读完并 finish 整个 checked 来源，预算用尽的判定发生在失败
读取或 finish（含父层尾部排空）之后，不会把 409 和 413 混淆。

GET 用生成当前分支的 candidate stamp 打开外层范围，按计划逐层重建 JSON 解转义
读取器，流式 base64 解码最内层后，再排空每个父层未读尾部、校验各层解码长度/
SHA-1 与外层 EOF，最后完成 retained checked handle 才发布 HTTP。图片字节相同
不构成授权：外层非图片字段在文件大小与 mtime 不变的情况下被改写，冷/热 token
都会 409；普通 append 保留旧图。固定父前缀与 Codex 子代理保持真实外层记录
范围和 leaf-only end，scope token 仍按所选视图独立。

本批未实现单消息超过 16 图的 continuation，也未支持跨多个字符串拼接的巨型工具
JSON；原 Python 项目不变，未调用付费 CLI，未提交生产部署或跨平台实机验收。
测量与合同细节见 [native-input.md](native-input.md#nested-stringified-tool-envelopes-batch-19)。

## 第二十批：单消息媒体 continuation

一条逻辑消息含 17 张以上 typed 图片以前会让整个会话 501。现在 provider 上限
提高到 256 张（超出仍明确失败），所有公开投影（首屏窗口、SSE 增量、历史页）
每条消息只内联前 16 张描述符，其余通过 `media_more{remaining,total,cursor}` 和
`GET /api/messages/{uid}/media-page` 逐批（≤16）取回；窗口/页预算只计内联前缀。
搜索与输入历史不带 `media`/`media_more`。

grant 复用历史页存储（1024 项、10 分钟、最旧淘汰、32 hex token），绑定 canonical
UID、精确 agent、生成视图的 live checkpoint、事件在非 status 序列中的绝对下标、
消息 JSON 的 SHA-1 身份、图片偏移与总数；同一页重读返回同一 `next`。checkpoint
失效、事件身份或图片总数变化 → 409；跨类型 token → 404。分页描述符走与内联相同
的懒加载登记路径，GET 物化、描述符/blob 预算不变；读取分页从不触碰 live cursor。

legacy 仅在 Rust `media_continuation` 能力下于 gallery 末尾渲染 `.media-more`
按钮；点击原地追加下一批、更新缓存消息，失败保留已加载图片并给出带 HTTP 状态
的错误与重试；视图切换/reset/cursor 不匹配时丢弃结果。Python 页面不变。

未实现：跨多字符串拼接的巨型工具 JSON；256 张以上仍明确失败。原 Python 项目
未改，无付费 CLI、提交生产或跨平台实机验收。

## 第二十一批：高级差分与读模型对齐

`tests/advanced_parity.py` 用合成 fixture 把 Claude 多级 compact/rewind/sidechain、
Codex 三级 fork/rollback/子代理、三家工具/问答/错误/超大输出和 Grok 信封逐字段
对照 Python adapter。由此修正三处 Rust 偏差：Grok `<image_files>`+`<user_query>`
信封按参考实现整体匹配且只去一个首尾换行；Claude/Grok tool_result 过滤空文本块；
Codex `web_search_call` 无参数渲染 `{}`。其余差异均为计划已声明的安全差异
（结构异常明确 501、超预算 Write 提示、不伪造 `[图片]`、Grok `ts` 保留、嗅探
退出码为额外字段）。另新增 `tests/run_validation.py` 串行验证运行器。

## 第二十二批：进程身份与三态运行状态

`/api/live` 在配置了显式 host 目录时给出证据化的三态：`running` 要求 host Info
可达且子进程/宿主进程的 pid + `/proc` 启动 ticks 与首次观察一致；`exited` 来自
host 退出、已确认的生命周期退出回执或本进程内验证过的身份消失；其余全部是带
原因的 `unknown`。`uids/started_at` 只列 running，未列出不表示已停止，legacy 的
`live` 能力仍关闭。非 Linux 平台不读 `/proc`，状态恒为 unknown。合同细节见
[processes.md](processes.md#process-identity-and-the-three-run-states-batch-22)。

## 第二十三批：浏览器审计接收

legacy 页面在 `audit` 能力开启时会把浏览器侧回执 POST 到 `/api/audit/browser`。
Rust 只在显式私有 `SESSIONDOCK_AUDIT_DIR` 下接收：先准入后拷贝、字节与条数双限
队列、饱和时不读 body 直接 202 `dropped:true`（旧页对非 2xx 会无限重发）、只存
脱敏后的结构化元数据、按大小轮转与总量保留、`/api/health` 暴露计数。未配置时
路由保持 501、能力为 false，页面零请求。合同见 [diagnostics.md](diagnostics.md)。

## 第二十四批：CLI 创建/续接参数契约

launcher 配置升到 schema 2，按来源声明 CLI profile：可执行文件、固定前缀、
`new_args`/`resume_args`（整参数占位符 `{session_id}`/`{sid}`）、env 白名单与
黑名单、profile 级 cwd 根。Claude 新建由服务端预先铸造 `--session-id` 并作为
launch 身份信封传给 ptyhost，Codex/Grok 新建保持 pending 走操作者绑定；续接
（`resume_uid`/takeover）从冻结库存解析完整 SID，同一原生身份只允许一个受管
实例。新增 `complete-dir`、`backend`（仅 ptyhost）；`session/stop`、强制接管、
rename 仍 501。legacy 仅按能力门控最小改动。全部验证只用假 CLI 脚本。合同见
[lifecycle-launcher.md](lifecycle-launcher.md#cli-profiles-batch-24-schema-2)、
[lifecycle-http.md](lifecycle-http.md#launch-identity-kinds-batch-24)。

## 第二十五批：原始终端输入

`POST /api/term/send`/`term/scroll` 以与 WebSocket 输入完全相同的租约授权把原始
文本/命名键交给 host；成功只表示 host 已确认写入，不表示 CLI 已处理，也不涉及
发送账本（`outbox` 仍 false）。`scroll` 与 Python 的 ptyhost 后端一样是空操作
（浏览器 xterm 自己滚动）。legacy 仅在 `terminal_input` 能力下为两条原有 HTTP
路径附带租约。合同见 [terminal-input.md](terminal-input.md)。

## 第二十六批：会话回收站

软删除只移动冻结库存命名的文件到显式私有 `SESSIONDOCK_TRASH_DIR`（同文件系统
rename、移动前复验 stamp、失败回滚），fork 父会话受保护，`running` 永远拒绝、
`unknown` 需显式 force；恢复不覆盖、purge 不碰原生目录；批量删除以 200 报告
部分成功。legacy 在能力开启时增加 force 确认与回收站分页提示。合同见
[trash.md](trash.md)。

## 第二十七批：持久化时间线 pin

`POST /api/session/rewind` 把 Claude 主会话的显示 pin 写入元数据存储，读模型以
纯 parser 选项（`declared_tip`/`abandoned_after`）应用，anchor 带 pin 戳因此
pin/unpin/退役都是 reset；`stale_end` 之后出现任何 lineage 信号即退役并报原因
（`native_continued`/`native_advanced`/…）。不写原生文件、不向 CLI 发信号，响应
恒为 `native_rewind:false`。legacy 在 `timeline_pin` 能力下提供"回到此处"与
说明。合同见 [metadata.md](metadata.md#timeline-pins-batch-27)。

## 第二十八批：显式写根下的文件作业

上传/新建/重命名/移动/删除只在 `SESSIONDOCK_FILE_WRITE_ROOTS` 下开放，写根必须
位于读根内且与所有私有路径不相交；每个分块都重新经会话 scope 解析，发布用
`linkat`/`O_EXCL` 绝不覆盖，删除只移入回收目录（第二十八批放在根内
`.agenthub-trash`；WP-D 起对齐 Python 的私有 `file-manager/trash`，改为
`<SESSIONDOCK_STATE_DIR>/file-trash/`，不再落入项目树；暂存目录改名
`.sessiondock-upload`），作业绑定 scope 并
有并发/字节/过期限额。legacy 按 `files_write` 能力隐藏不支持的动作；缩略图与
copy/compress 等仍 501。合同见 [files.md](files.md#write-operations-under-explicit-write-roots-batch-28)。

## 第三十三批：读模型对当前 CLI 版本的兼容（未知类型跳过）

2026-09-12 实跑 `tests/send_claude_real.py` 发现：Rust 投影对任何未知
记录类型整会话判 `supported:false`（空消息列表），而 Python adapter 的 `if/elif`
链只是落空、什么也不输出。当前 CLI 版本写入白名单之外的类型（Claude Code 2.1.269：
`mode`/`permission-mode`/`atis-latch`/`bridge-session`/`file-history-delta`/
`agent-name`/`cost-state` 记录与 `environment`/`hook_success`/`model`/`diagnostics`
等 20 种 attachment；Codex rollout：`token_usage_record`/
`inter_agent_communication_metadata` 记录、`event_msg item_completed`、
`response_item agent_message`），因此每个真实新会话都不可读、不可续接。

**策略（三家 provider 相同）**：未知的记录类型、Claude attachment 类型、Codex
`event_msg`/`response_item` 类型、Grok 记录类型，以及可读记录内未知的非图片
内容块类型（含 content 数组里的非字符串/对象元素）一律**跳过**，与 Python 一致；
绝不因此 `supported:false`、绝不返回空消息列表。跳过的种类按首次出现顺序计数，
写进会话行的非致命 `migration_warnings`，`supported` 保持 true，列表/详情/分页/
SSE/搜索/冻结 inventory/lifecycle 续接与发送确认都按可读会话处理。条目格式：
`跳过未知的<provider> <类别>：<kind> ×<count>`，例如
`跳过未知的Claude 记录类型：atis-latch ×264`、`跳过未知的Claude attachment 类型：environment ×3`、
`跳过未知的Codex event_msg：item_completed ×12`、`跳过未知的Codex response_item：agent_message ×1`、
`跳过未知的Grok 记录类型：usage ×2`、`跳过未知的内容块类型：audio ×1`。最多列 32 种，
超出部分合并为一行 `另有 N 条其他未知类型已跳过（超过 32 种，未逐一列出）`；kind 超过
64 字符截断。硬失败仍是硬失败并保持 `migration_warnings == [原因]`：`content` 为标量、
文本块 `text` 非字符串、Codex `history_base` 非对象、无法解码的图片块/外链、100000
条与其他预算（第三十五批起，坏 JSON/重复键的行、重复 `session_meta`、Claude 声明叶子
缺失/祖先缺失/环都改为与 Python 相同的非致命处理，见该批）。Codex 工具输出信封（`tools::output_text`）保持第十九批的严格语法，
未知块仍拒绝。已渲染的 attachment（`queued_command` 人类提示）行为不变；
`atis-latch` 等新类型不生成消息。attachment 记录带 uuid/parentUuid，是 Claude
图节点：assistant 的 parentUuid 指向最后一条 attachment，祖先链经 attachment 回到
user 记录，`turn_id`/已回答判定/last-prompt 都按原逻辑成立（单测与 parity 夹具
按此形状构造）。

夹具：`tests/history_parity.py` 提供 `claude_control_rows`/`claude_attachment_chain`/
`claude_turn_tail_rows`/`codex_telemetry_rows` 与 `CLAUDE_ATTACHMENT_KINDS`（20 种），
新增 `claude-cli-current`（`-p` 一次性 + `--resume` 第二轮）与 `codex-cli-current`；
`advanced_parity.py` 的 `claude-events`/`codex-l2`/`grok-chat` 混入同样的类型；
`fixture_gen.py` 默认把这些类型按观察到的位置写进生成语料（`--plain` 关闭）；
`sessions_list_suite.py` 断言列表行 `supported:true` + 精确警告 + 详情可读。Python
adapter 读同一夹具结果完全一致（0 DIFF，无新增 DELTA）。原先用"未知记录类型"
触发不支持的测试（`legacy_browser`/`search_suite`/`search_browser`/`tests/search.rs`
/`tests/files.rs`/`history.rs`/`sessions/tests.rs`/`history_parity`）改用 Python 同样
读不了的形状（`content: 42`；第三十五批起重复 `session_meta` 不再是不支持形状）。

## 第三十四批：读模型重做为惰性索引 + 按需视图

设计以 [read-model.md](read-model.md) 为准（旧"冻结库存"归档于
[superseded/frozen-inventory.md](superseded/frozen-inventory.md)）。列表只做目录
遍历 + `stat` + 每文件 96 KiB 头 / 512 KiB 尾的有界摘要（`sessions/index/`，按
`dev/ino/size/mtime` 缓存，16 路并行，500 ms TTL，`force=1` 重扫），会话只在打开时
经 `sessions/views/` 流式解析这一个文件（追加续读、重写重建、64 项 / 2 GiB LRU），
搜索按需流式扫描不留驻。`SessionStore` 只是门面：`list` = 索引行 + names/metadata
装饰 + 重签名；`snapshot` = 从索引取候选文件与所属子代理、从元数据取 pin、从已
发布行取 `meta`，经 `Views` 打开；`search_snapshot`/`native_catalog` 直接是索引
的行与原生目录（lifecycle 续接/接管、回收站文件集、`/api/live` 都不再解析任何文件）。
删除的东西：启动/刷新全量解析、"任何文件在解析期间变化即整份 503"、1000 会话 /
256 MiB 总量 / 20000 目录条目上限、旧 `sessions/names.rs`（由 `index/names.rs`
取代）、`history::Graph` 的生产用途（仅剩 views 测试的参照实现）。

对外契约变化（均已进套件与 [history-pages.md](history-pages.md)）：

- 列表行 `cursor` 只带物理部分 `{end, head}`（索引可知），语义 `anchor` 只在服务端
  缓存着该文件当前版本的视图时出现；`sig` 只覆盖索引行 + 元数据，打开会话不改变
  签名。legacy `syncSidebarUpdates` 只在两边都有 anchor 时比较它（Python 服务的页面
  行为不变）；后果是该进程里从未打开过的会话第一次增长只刷新游标不累加未读。
- `timeline_pin` 行字段来自元数据，`retired:false` 只在文件未越过 `stale_end` 时
  由索引断言，完整退役原因来自缓存视图或详情读取。
- Claude 谱系警告（缺失祖先、环、缺失叶子；第三十五批起非致命）与内容块警告只在
  打开时进入详情 `meta.migration_warnings`，行按 Python 列出 `supported:true`；
  > 512 KiB 文件的未知类型计数只来自头尾；Codex 主行 `updated` 是文件 mtime，
  无提示的 rollout 标题为 `(无标题) <stem[:16]>`。
- 单条记录 64 MiB / 单文件 4 GiB / 2,000,000 LF / 1,000,000 条等物理预算只在打开时
  对该会话生效（413/501），列表照常列出该行（`budget_boundaries_suite` 改为
  "列出 + 打开失败"）；`sessions_list_suite` 的 1001 会话改为必须全部列出。
- Grok 会话的 `chat_history.jsonl` 若是链接/目录/不可读，行为 `supported:false`
  （不再是"没有聊天文件"的空成功）；权限类失败不进摘要缓存，修好即恢复。
- 视图打开时若发现文件比索引发布的版本新（追加、重写、Grok 聊天文件刚创建），
  门面强制重扫一次再复用缓存视图，使 `meta`（size/updated/title/chat_exists）与
  视图一致；SSE 在这种情况下可能多发一次只改元数据的事件。

验收套件：`inventory_scale_suite`（合成 1500 会话 / 1 GiB）、
`inventory_live_append_suite`（并发追加 30 次列表 0 失败）、`list_rows_parity`
（与 Python `list_sessions` 逐字段 0 DIFF）、操作者手动的 `real_roots_bench`
（只读真实根，结果见 [performance.md](performance.md)）。

## 第三十六批（D）：Claude 中断回合保持可见

- Python `d16c5e1`（基线 e5b023a）的 `_active_lineage`/`_read_one` 规则全部对齐：Esc 后助手已写
  文本/工具/thinking、随后新输入挂回上一个 `turn_duration` 的那一轮不再当成"已完成旧分支"
  丢掉——从原生中断记录（`interruptedMessageId` 或正文恰为 `[Request interrupted by user]`/
  `…for tool use]`）上溯到当前链旁的第一条输入，连同其下全部记录一起显示；其下未回答的
  输入逐层同样标记；被标记的输入带 `interrupted:true`/`interrupt_reason`，自开 `turn_id`、
  不发 `working`，`aborted` 在其下有原生记录时由中断记录给出（否则紧跟输入）；中断记录
  本身从不开回合。旧的"未回答 sibling"规则与 `abandoned_after` 边界不变，第三十五批的
  谱系警告不变。规则见 [history-pages.md](history-pages.md#opening-a-session-on-demand-views)；
  `history_parity`（`claude-esc-*` 四例）与 `advanced_parity`（工具链/compact/中断即 sibling）
  对 Python 0 DIFF，真实读根 `shadow_compare.py --sample 30` 的 claude 0 DIFF。

## 第三十六批（G）：Codex 多块工具信封按序拼接

- Codex 把 `exec` 脚本结果记成 `output` 数组：块 0 是 `Script completed\nWall time …\nOutput:\n`
  头，之后每个流式 chunk 一块字符串化信封（`{"chunk_id","wall_time_seconds","exit_code"?,
  "session_id"?,"output"}`），有时再跟一个空块或 `input_image` 块，脚本自身的 print
  （`--- 1 ---`、`{}`、`{"i":0,"status":"fulfilled"}`）可夹在中间。Rust 现在把**每个**
  chunk 的 `output` 按块序原样拼接（不加分隔符，chunk 自带换行），`exit_code` 取最后一个带
  该字段的 chunk，`duration_s` 为各 chunk `wall_time_seconds` 之和，图片块与 chunk 内图片照旧
  按块序登记为媒体；头、print、空块、`{"i","status","value":{…}}` 包装不进正文（后者与 Python
  一样显示原始拼接文本）。只有一个 chunk 时结果与第十九批的单信封搜索完全相同。原生路径上
  多块数组的每一块是独立的候选位置：巨型 chunk 经共享回放预算就地解码（不再读回后二次解析），
  巨型普通块仍按原样读回，MCP content 数组内多个巨型块仍 501。
- **有意差异（DELTA）**：Python `_tool_output` 先把各块拼成文本再解码，只显示一个 chunk（拼接
  文本以 chunk 结尾时取最后一个，尾块是 `[图片]`/`{}`/print 时取第一个信封块），`duration_s`
  只是该 chunk 的。真实读根中此形状约 2600 条结果，第三十五批影子比对的 4 处 DIFF 全部属于
  此类；`tests/advanced_parity.py` 的 `codex-envelope-chunks` 按字段声明该 DELTA，
  `tests/shadow_compare.py` 把"Python 正文是 Rust 更长正文中完整一段"的 tool_result 归为
  DELTA 并打印两边长度。规则见 [native-input.md](native-input.md#multi-part-outputs-every-chunk-batch-36)。

## 第三十七批：前端重同步到 Python `e5b023a`

`legacy-web` 对每个文件做 3-way 合并（ours = 带 Rust 门控的 `legacy-web`，base = 冻结的 `ee2e373c`
快照，theirs = Python `e5b023a` 的 `agenthub/static`），然后把 `reference/legacy-web` 整目录换成
`e5b023a` 快照作为新的冻结基线。`git diff --no-index reference/legacy-web legacy-web` 现在只剩 Rust
自己的东西（10 个文件 +1355/−84 与 `capabilities.js`）。吸收的 Python 变化：左栏分层树
（`#nest-toggle`，`spawned_by` + `agent_items` 建树、`.nest-caret` 折叠、`--depth` 缩进、子代理行）、
标题下拉按结束时间排序并带运行点、`followContinuedSession`、顶栏/标题栏按实测折叠
（`layoutHeader`/`layoutSessionHead`，三档同高 43px）、`#dlive` 运行点移到标题图标右上、批量浏览器审计
（`auditPayload`、pagehide 用 sendBeacon 剥 content、新事件名）、工具分组复用节点并记住用户展开
（`syncGroupNode`/`appendToolResult`）、侧栏分割线 pointer 事件、设置→机器的启用/排序（本机模式不发请求）、
亮色主题 PTY 颜色重映射、Grok 排队气泡后缀匹配（`cli.js`）。

8 处冲突的解法：① `flushBrowserAudit`：采用 Python 的 `auditPayload` + 批量拆分，`allows('audit')`
门控放进新的 `flushBrowserAudit` 与 `flushBrowserAuditBeacon` 函数头（先于任何计时器/beacon 访问）；
② `paintLive`：Python 去掉 `●` 字形，Rust 的"运行状态未知，尚未实现进程探测"标题保留，抽成
`liveStatusTitle(tmux)`；③ `openSession` 失败：`reportMigrationReadFailure` 与
`auditDetailRendered('load-failed')` 都留；④ `renderSession`：先 `layoutSessionHead()` 再
`renderTimelinePinNotice(meta)`；⑤ `head()`：Python 把 `#dlive` 从 dmeta 挪进 `sessionIconMarkup`，Rust 的
未知态标题跟着挪过去（同一个 `liveStatusTitle`）；⑥ `toolEntry`：采用 Python 抽出的 `appendToolResult`，
`mediaGallery(m.media, m.media_more)` / `mediaGallery(r.media, r.media_more)`（媒体 continuation）进新函数；
term.js `loadTermList`：新建按钮 = `T.enabled && allows('terminal_create')`，显隐变化时按 Python 重跑
`layoutHeader()`；nodes.js：`STORAGE_PREFIX` 取 namespace 与 `Nodes.machines` 同时保留。另外
`applyMigrationMeta`（Rust SSE 权威 meta）的标题键补上 `spawned_by` 并在换头后 `layoutSessionHead()`，
与 Python `refreshSessionMeta` 一致。58 处 `AgentHubCapabilities` 门控全部保留（两处 `#dlive` 标题表达式
合并为一个 helper，新增 beacon 门控，现为 57 处引用），`capabilities.js`、`#backend-notice`、
`#native-bind-dialog`、历史分页、媒体 lazy/continuation、pending 行、时间线 pin、回收站/停止门控、
`storage_namespace` 均在。唯一的样式修正：窄屏详情页隐藏 `#backend-notice`（Python 在 mobile-detail
隐藏 header，标题下拉按 `position:fixed; top:45px` 定位；通知条留着会盖住标题栏）。

后端尚未发布的字段按 Python 同样的缺省退化：没有 `spawned_by` 就没有嵌套（每行都是根，子代理行照常缩进），
没有 `agent_items[].active` 就没有子代理运行点（`agentRunning = active && S.live.has(uid)`），没有
`continued_in` 就不跟到续写会话，`live:false` 时 `S.live` 只含本页启动/接管的实例、活跃筛选仍拒绝。
浏览器套件 `nest_tree_browser`/`agent_menu_browser` 先按当前后端断言这种退化，再在 HTTP 边界注入这些字段
断言完整行为；`header_fold_browser`、`side_drag_browser`、`tool_group_fold_browser` 是对应 Python e2e 的
移植。Node 合同改为加载重构后的 `auditPayload`/`flushBrowserAudit`/`flushBrowserAuditBeacon` 与分层树纯函数。

同步到 `16cc89c`（2026-09-12 22:52）：同样的 3-way 合并（base = `e5b023a`），`app.js`/`index.html`/`style.css`
零冲突，`reference/legacy-web` 换成 `16cc89c` 快照。吸收的变化：分层开关用自己的图标 `#i-tree`（⑂ 字形从
前端消失）；未读角标按当前状态着色（`idle` 灰 = 已退出但有没看的新内容、`tmux` 蓝、运行绿），localStorage 旧记录里
的 tmux 标志不再读；子代理行改用来源图标加右下角 `.agent-mark`；树的引导区 `nestLeadMarkup`（每级祖先一根竖线加一个
三角槽位，与分组标题的三角同列），`--depth` 缩进删除、行加 `.item.tree`；`sessionHidden` 加 `sessionContinued`——Claude
`continued-in` 的旧文件在续写会话仍在列表时不进左栏，`nestTree` 不把续写会话当成旧会话派出的孩子（新进程继承旧会话
的环境变量，`spawned_by` 会指向它）；标题栏去掉 `spawnerMarkup` 发起者链接与 `⑂ branch` 项，列表行元信息去掉 `⑂N`
子代理数。Rust 侧唯一跟随修改：`applyMigrationMeta` 的标题键去掉 `m.branch`（标题栏不再渲染它）。57 处
`AgentHubCapabilities` 门控与上一段列出的 Rust 专有部分全部保留；Grok 在途 `user_query` 信封的剥离在后端。
后端已发布 `spawned_by`/`continued_in`/`agent_items[].active`（第三十六批），浏览器套件不再在 HTTP 边界注入：
`nest_tree_browser` 把 `spawned_by` 种进 `session-metadata.json`（进程扫描写的同一份记录）、子代理 `active` 由 sidecar
未收尾的回合给出、`continued_in` 由尾部 `continued-in` 记录给出，断言续写前的父行不在左栏而续写会话在、点旧 uid 跟到
新会话、`.agent-mark`、根行三角与分组三角同列；`agent_menu_browser` 的完成/恢复改成改写子代理 transcript 后
`loadSessions(true)`（`?force=1` 让索引先重扫）。仍由页面手工设置的只有 `S.live`：合成语料没有 CLI 进程可扫，Python e2e
同样在节点桩上给 live。`header_fold_browser` 的元信息顺序去掉 branch；Node 合同补 continued-in 隐藏/不嵌套与角标状态类。


## 第四十四批（WP-A）：并发与稳态

Rust 侧：只读工作池从固定 4 个 `try_acquire` 改为可配置的有界等待
（`SESSIONDOCK_READ_WORKERS`、`SESSIONDOCK_ADMISSION_WAIT_MS`，探测/响应池按比例派生，
搜索不再占读 worker），历史页页长 200 → `SESSIONDOCK_HISTORY_PAGE_EVENTS`（默认 2000），
`capabilities.stage` 改 `"replacement"`、`read_only:false`，`/api/meta` 去掉 `migration` 字段；
常驻内存与空闲 CPU 的预算、实测与剩余项见 [performance.md](performance.md) 与
[read-model.md](read-model.md#常驻内存预算第四十四批-wp-a)。

`legacy-web` 相对 Python `16cc89c` 的有意差异（都在 Rust 分支内，Python 路径不变）：

- 读取失败分两类：瞬时（网络错误、abort、408/429、5xx 非 501）只影响本次请求——
  `syncSession` 与首次打开走 Python 同款退避重试，SSE `onerror` 在 Rust 分支按 1.5 s 起、
  15 s 封顶的指数退避重开（流从未打开过时先用一次增量读取探原因），子代理 `append=1`
  只对瞬时失败退避重试三次；不可恢复（`migration-error` 事件、501、4xx）才
  `reportMigrationReadFailure` 暂停并保留快照。横幅只在有快照时出现，4xx 不给"重试读取"，
  无快照时正文"读取失败: …"就是全部提示。
- `term.js loadTermList`：瞬时失败保留上一轮 `enabled/sources/list/pending`（"+"不消失）。
- 历史缺口按钮：点一次连续翻页直到填满（进度写在按钮上，再点一次中止并保留已取到的页），
  全部取完后渲染一次；`HISTORY_PAGE_CHAIN=false` 供逐页 E2E。
- `#backend-notice` 只在能力声明解析失败（`configuration_error`）时出现。

## 第四十四批（WP-G）：题卡与审批

后端补上 Python 的实时题卡（[delivery.md](delivery.md)"Live question cards and approvals"）：`sessiondock claude-hook`
子命令 + `--write-bridge-settings` 生成的 `--settings` 文件把 Claude `AskUserQuestion` 写到
`<SESSIONDOCK_STATE_DIR>/claude-prompts/<sid>.json`；`/api/messages` 主视图顶层带 `prompt`（无题卡为 `null`，子代理视图无此键），
`/api/watch` 的消息包带 `prompt`、只有题卡变化时发 `{"prompt_only":true,"prompt":…}`；Codex 审批由受管实例屏幕解析
（`bridge::codex::approval_prompt`，id 与 Python 逐字节一致）。前端不改渲染路径（`renderConversationTail`/`questionNode`），
但有一处**有意的基线偏离**：`legacy-web/cli.js` `ClaudeCli.questionAnswerKeys` 从 `Up×(n+3), Down×i, Enter` 改为直接按选项
数字 `[String(i+1)]`。实测 Claude Code 2.1.270 的 AskUserQuestion 菜单在选项后追加 "Type something." / "Chat about this"
两行且上下循环，旧序列一批送入后停在 "Type something."（隔离实例屏幕捕获），而单个数字键会立刻选中并提交对应选项，与光标
位置无关。Python 的 `agenthub/static/cli.js`（e5b023a）仍是旧序列，其 `tests/e2e.py` 断言 `["Up"]*5+["Down","Enter"]`；
这是 Python 侧对当前 CLI 版本的失效，不是本仓库要复制的行为。为此 `/api/term/send` 的 `keys` 接受单个 ASCII 字母/数字
（[terminal-input.md](terminal-input.md)）：Codex 审批的 `y`/`p` 与 `request_user_input` 的 `1`–`9` 原本就是这样发的，
此前会被 400 拒绝。多题表单（`questionFormAnswerKeyGroups`）未改也未在 2.1.270 上验证。

## 第四十四批（WP-F）：改名收尾、偏好一次性迁移、登录 shell 包装

**品牌收尾**（watcher F9/F10）：可安装身份全部改为 SessionDock——`manifest.webmanifest`
`name/short_name`、`index.html` 的 `apple-mobile-web-app-title` 与 `pwa-install.js` 的
`data-app-name`/`data-storage-key`（`sessiondock.pwa-install-dismissed`；脚本本身与 Python 相同，
从不读这两个属性）、`files.html`/`file.html` 的 `<title>` 与 `files.js`/`file.js` 运行时设置的标题，
以及 `service-worker.js` 的 `CACHE_NAME = "sessiondock-shell-v1"` 与离线文案。缓存名改掉的原因：
同源双部署（`/agenthub/` 与 `/sessiondock/`）下 Cache Storage 按 origin 共享，两边都写
`agenthub-shell-v1` 时任一边升到 v2 的 `activate` 会删掉另一边正在用的缓存；现在各自的
`activate` 只清自己前缀下的旧版本。`files.js` 的页面键从 `agenthub-files-*` 改为
`<namespace>files-<view|clipboard|history:…>`；[glossary.md](glossary.md) "Names" 相应把这两组
键从保留名单移到本节。`tests/brand_names_check.py` 静态锁定这些 shell 文件里除契约标识白名单
外没有 `agenthub` 拼写。

**偏好一次性迁移**（visual F4、watcher F8）：zj 平时经 nginx 同源访问两套页面，Python 存的
`agenthub.<k>`（hub 页 `agenthub.hub.<path>.<k>`）与 Rust 的 `sessiondock.<k>` 并存于同一个
localStorage，此前 Rust 一概不读，用户体感是"换了服务后字体/主题/侧栏宽/筛选/未读全部归零"。
现在 `capabilities.js` 暴露 `AgentHubCapabilities.stored(key, prefix = namespace)`：读
`<prefix><key>`，缺失且 Rust 命名空间非空时回退读 Python 键，读到即写成新键（一次性），
Python 键本身永不再写；`app.js store.get`（含 `unread`/`sel`/`queuedMessages` 等运行态键）、
`nodes.js` `nodesOff`、`typography.js` `font` 都走它，`index.html` 的主题引导在 `capabilities.js`
之前执行，因而内联了同样的回退。`files.js` 的三个键回退顺序是
`<namespace>files-<k>` → `<namespace>agenthub-files-<k>`（改名前 Rust 自己写下的）→ `agenthub-files-<k>`（Python 的裸键）。
Python 页面（无能力 meta、命名空间为空）不回退、不复制；hub 页回退到同 `location.pathname`
的 Python hub 前缀。写路径不变：只写新键，所以之后两边各自独立，不会互相覆盖。
契约：`tests/legacy_contract.mjs`（vm 里的 `stored` 语义、files 键拼写、shell 身份）、
`tests/prefs_migration_browser.py`（真实页面：预置 watcher w5 的九个键 + 运行态键 → 首屏全部生效、
只复制一次、后续改动只落新键、已存在的 Rust 键优先、files.html 两种旧拼写、全新浏览器保持默认）、
`tests/hub_browser.py` 的 `sessiondock.hub./.` 断言不变。

**登录 shell 包装**（主会话发现）：Python 经 `~/.local/bin/with-zshrc` 启动宿主，CLI 因此拿到交互
zsh 的完整环境（conda p311、npm-global、`~/.grok/bin`、各 API key、`.zshenv` 的代理）；Rust launcher
`env_clear` + 固定 `PATH`，CLI 里的 `python3` 落到 `/usr/bin/python3`。launcher 本身不需要改动：
把 profile 的 `executable` 指向包装脚本、`args[0]` 放 CLI 的 PATH 名即可，见
[lifecycle-launcher.md](lifecycle-launcher.md#login-shell-wrapper-batch-44-wp-f)。顺带的效果：
`exec grok` 让 grok 以 argv0 `grok` 启动（此前直接执行 `~/.grok/downloads/grok-<ver>-linux-x86_64`
时 procscan 的 `is_cli` 不认，Rust 起的 grok 在 `/api/live` 的 `tmux_uids`/`started_at` 里缺席，
WP-E 记录的后续项），Claude 的 argv0 也从版本目录名变回 `claude`。
