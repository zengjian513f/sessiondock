# 读模型设计：惰性索引 + 按需视图

这是会话读模型（列表、详情、分页、SSE、搜索、运行时身份的数据来源）的
**唯一有效设计**。它取代了 2026-09-12 之前的"冻结库存"（全量启动解析），
那套设计只保留在 [superseded/frozen-inventory.md](superseded/frozen-inventory.md)
供查阅，不再是任何代码或文档的依据。

## 原则

1. **没有启动解析。** 服务启动后第一次列表只做目录遍历 + `stat` + 每个文件的
   有界头/尾读取；不解析任何文件的全文。
2. **每个文件相互独立。** 一个文件在读取期间被追加、重写或删除，只影响它自己
   这一行/这一份视图；永远不存在"整份索引作废"。
3. **只在打开时才解析。** 会话视图（消息投影、LF 检查点、媒体片段）只为被打开
   的会话建立，按需增量续读，进有界 LRU；不为未打开的会话保留任何消息对象。
4. **常驻内存与数据总量无关。** 常驻的只有每个文件一条行摘要（几百字节）
   和 LRU 里有限个视图。
5. **没有会话数或总字节上限。** 唯一的规模限制是文件系统本身；单文件
   4 GiB 的全量解析预算只在打开该会话时生效（413 只针对它）。
6. **搜索不重复解析。** 每个主会话的可搜索正文按文件版本持久化在搜索文本
   缓存里（WP-B，见下文"搜索"）；一次搜索只读缓存、只解析版本变了的会话，
   投影结果不留驻，命中语义与逐文件流式扫描完全一致。
7. **多线程用在读摘要上。** 头/尾读取在有界阻塞线程池上并行；并行不是用来给
   全量解析提速的。

## 组件

| 组件 | 职责 | 输入 | 输出 |
| --- | --- | --- | --- |
| `sessions/index` | 目录遍历、`stat`、并行头/尾摘要、按 stamp 缓存、行推导、归属图、`sig`/`built_at` | 三个读根 | `/api/sessions` 行、候选文件表（uid → 路径/来源/stamp/原生 id/归属）、运行时原生目录 |
| `sessions/index/summary` | 三家来源的有界摘要：头 96 KiB（≤ 40 条记录）+ 尾 512 KiB，推导与 Python `list_sessions` 逐字段一致 | 单个文件 | `RowSummary` |
| `sessions/views` | 单会话视图：经既有 `RecordCache`/`RawIndex`/provider 投影流式解析**这一个**文件，增量续读，重写重建；LRU（条数 + 字节） | 候选文件 + 可选时间线 pin | `ViewSnapshot`（消息、分页、媒体、检查点、原生输入证据） |
| `search` + `search/cache` + `search/service` | 搜索文本缓存（按会话、按文件版本持久化的语义正文）、解析预算、后台预热；未缓存的会话借用已缓存视图或流式投影后丢弃；有界准入与 `partial` | 查询 + 候选表 + 缓存目录 | NDJSON 命中流 |
| `observe`（SSE） | 每会话 `stat` 轮询 + 视图增量扩展；列表 SSE 用索引 `sig` | 索引 + 视图 | 事件流 |
| 运行时 / lifecycle / trash / delivery | 从候选表取原生 id、路径、stamp、归属；发送确认边界与原生尾部来自打开的视图 | 索引、视图 | — |

## 列表：索引与摘要

- **候选发现**：Claude `<root>/<project>/<sid>.jsonl` 主会话与 `agent-*.jsonl`
  sidecar；Codex `<root>/YYYY/MM/DD/rollout-*.jsonl`；Grok `<root>/<dir>/summary.json`
  （+ `chat_history.jsonl`）。只走两级目录，不跟随符号链接，路径必须在根内。
  唯一例外（第四十四批 WP-C）：Claude 续写会话把原会话的 sidecar 以符号链接放进自己的
  `subagents/`（Python 的 `glob` 会列出它），`subagents/agent-*.jsonl` 若是链接且
  canonicalize 后是**任一配置读根内**的普通文件则跟随：链接路径是身份（uid、按目录归属
  给续写会话），canonical 目标是数据文件；指向根外、悬空、指向目录的链接仍跳过，主会话
  文件一律不跟随。
- **stamp** = `dev/ino/size/mtime_ns`。摘要缓存以 stamp 为键：stamp 未变则
  热刷新只有 `stat`；变了只重读这一个文件。
- **摘要读取**：头 96 KiB 内最多 40 条完整记录 + 尾 512 KiB 内的完整记录
  （残行丢弃），与 Python `_head_lines`/`_tail_lines` 相同；推导 `title`
  （custom-title > 最新 ai-title > 首条用户输入生成）、`cwd`（头部优先，尾部
  计数兜底）、`branch`、`created`、`updated`（`mtime`）、`size`、`model`、
  Codex `session_meta`/`history_base`、Claude `sessionId`/fork 来源、Grok
  `summary.json` 字段。Grok 的 `size` 与 Python `_dir_size` 相同：会话目录内全部普通
  文件字节之和（递归、不进入链接目录、深度 ≤ 8 / 条目 ≤ 100 000 的防病态上限），与
  摘要一起按 summary/chat 的 stamp 缓存——Python 也只在这两个文件变化时重算。头/尾里
  发现的硬错误（`content` 标量、缺 id 等）使该行 `supported:false` 并给出
  `migration_warnings == [原因]`；坏行、重复 `session_meta` 与未知记录类型只计入
  **详情 `meta.migration_warnings`** 的非致命备注（`跳过无效的JSONL 记录 ×N`、
  `跳过重复的Codex session_meta ×N`）：公开列表行（含 `agent_items`）只在
  `supported:false` 时带 `migration_warnings`（Python 行没有这个字段，前端不消费，
  真实根上它占了列表载荷的一半以上）。
- **并发变化**：读头/尾前后各 `stat` 一次；不一致则重读（最多 3 次），仍不一致
  就按已读字节发布并带上读取时的 stamp。删除的文件在下一次刷新消失。
- **归属图**（`index/graph`）：Claude sidecar 归属主会话（目录 + 文件名 +
  头部 `sessionId`）；Codex 子代理 rollout 归属（头部元数据）；fork 父子
  （`history_base`/`forked_from`）。规则与 [history-pages.md](history-pages.md)
  中记录的一致，输入改为摘要。
- **子代理运行态与续写**（`index/agent_stops`，第三十六批）：`agent_items[].active`
  与 Python 一致——Codex 看子代理 rollout 尾部最后一条回合边界 `event_msg`
  （`task_started`/`turn_started` 开、`task_complete`/`turn_complete`/`turn_aborted` 关）；
  Claude 看 sidecar 尾部最后一条 user/assistant 记录是否为 assistant `end_turn`
  （只有它算收尾），未收尾时再对照主会话里该子代理最近一次停止通知
  （`<task-notification>` 的 `<task-id>` 或前台 Agent 的 `toolUseResult`，
  `async_launched` 不算；同一段通知文本只认首次出现的时刻）：无通知或 sidecar
  最后记录晚于通知即 `active:true`。停止通知可能离尾部很远，所以主会话文件按
  已消费的 LF 偏移**增量**扫描（只读新增的完整行、只解码能点名子代理的行、
  半行留待下次），扫描状态按 owner 路径缓存并以 stamp 判定是否需要续读；
  **只对存在未收尾 sidecar 的 owner 扫描**，与摘要同一线程池并行，冷列表仍
  ≤ 1 s、热列表仍只 `stat`；文件变短或 inode 变化则从头重扫。Claude 主会话尾部的
  `continued-in` 记录（`continuedInSessionId`）在同源主会话里按 sid 解析成
  `continued_in` uid（Python `finalize_sessions`：按路径序最后一个同 sid 行胜出，
  指向自身丢弃），解析不到则不出字段。
- **刷新节奏**：500 ms TTL 内复用上一份行；`force=1` 立即重扫；扫描在有界
  阻塞线程池并行（默认 16 路）。
- **签名**：`sig` = 行序列化的散列，`built_at` = 发布时刻；两者只由摘要与持久元数据
  （星标、fork 可见、pin）决定。门面给每个支持的行加索引可知的物理 `cursor {end, head}`，
  语义 `anchor` 只在服务端缓存着该文件当前版本的视图时借用，且不进签名：打开会话
  永不改变 `sig`（细则见 [history-pages.md](history-pages.md)）。

## 详情：按需视图

- 打开会话 = 从候选表取路径与 stamp，经 `records::RecordCache` 流式解析这一个
  文件（LF 检查点、前缀 digest、provider 投影、媒体片段），产出不可变
  `ViewSnapshot`。追加时从上次已提交偏移续读；stamp/前缀 digest 不一致
  （重写、截断）则重建；读取期间文件变化只对这一会话返回既有的重试码。
- LRU：默认保留 64 个视图、序列化消息合计 2 GiB、索引容量 1 GiB
  （物理工作预算，不是 RSS 上限）；最近使用淘汰，依赖 stamp 变化即失效。
- 每视图契约不变：游标 schema `rs-m2-1`、前缀散列、语义锚点、时间线 pin、
  `valid_checkpoint`、`native_checkpoint`/`native_tail`、`claude_native_inputs`、
  分页/媒体授权、`message_total`/`partial`/`activity`。见
  [history-pages.md](history-pages.md)、[native-input.md](native-input.md)、
  [media.md](media.md)。

### 视图模块的接口（`sessions/views`）

- `Views::open(&ViewRequest, &dyn Dependencies) -> Arc<ViewSnapshot>`：`ViewRequest`
  由索引填充——`uid`（主会话）、`agent`（`""` 或精确 agent id）、`owner`
  候选文件、`selected`（agent 自己的文件，主视图为 `None`）、`pin`（Claude
  主会话的时间线 pin）、`row`（已发布并经 names/metadata 装饰的主会话行；
  视图 `meta` 就是这一行，agent 视图按 `row.agent_items` 派生，与 Python
  `session_view` 一致）。候选只带索引发现的路径（来源/根/数据文件/sidecar），
  视图自己 `stat` 并按与上次解析的差异续读/重建；行变化只重组 `meta`，不碰
  文件。视图读到的文件版本比索引发布的新时，门面强制重扫一次再复用视图，使
  `meta` 与视图描述同一份字节。
- `Dependencies::thread(source, thread_id) -> Result<Candidate>`：Codex
  `history_base` 父线程由索引按原生线程 id 解析（501 未索引/子代理文件、
  409 歧义）；视图从不拼接路径。可选 `parsed(candidate, pin)` 让持有同 stamp
  解析的调用方免于重复流式读取。
- `Views::cached_current` 供搜索借用仍然有效的缓存视图；`open_transient`
  为搜索未命中做一次性投影，不进 LRU、不留 AST；`evict(uid)`、`clear`、
  `stats`（视图数 / 文件数 / 序列化字节）。
- 每文件解析（`parse_candidate`）与视图组合分离：leaf/owner 解析进文件 LRU，
  继承前缀按父文件 stamp 缓存，父文件 tail 追加不重读前缀，前缀改写改变
  身份并重置游标。
- 打开视图时列表的新鲜度（第四十四批 WP-A）：`open` 复用 3 s 内（`OPEN_TTL`）
  的索引快照——视图自己 `stat` 它显示的文件，新文件 / 归属变化在几秒内仍会出现；
  视图读到比索引更新的字节时仍立即重扫一次，`meta` 与字节始终一致（节流过一次，
  撤回）。`/api/sessions` 与列表调用仍按索引自己的 500 ms 窗口；
  `/api/live` 与 spawner tick 用同样的 3 s 窗口（`list_recent`）。目录遍历后如果
  每个文件的 stamp 都与上次相同（且名称索引未变），直接复用上一份快照，不重建
  行 / 图 / 签名。`/api/sessions?sig=` 命中时不克隆、不装饰、不序列化文档。

## debug_run 视图（Python `agenthub/debug_runs.py`）

- 注册表 `<SESSIONDOCK_STATE_DIR>/debug-runs.json`，与 Python 同格式
  （`{"version":1,"runs":{<run_id>:{"root":…,"created":…,"sessions":[{source,cwd,sid,uid,name}]}}}`），
  由测试工具（monkey）写入、本服务只读，`stat` 变化即重载；缺失/损坏 = 空注册表。
  `tests/meta_import.py` 把 Python 的 `~/.local/share/agenthub/debug-runs.json` 一并搬运。
- 匹配（`sessions/debug_runs.rs`，Python `_match`）：行的 `uid`/`sid`/`name` 命中某 run
  的 sessions，或 `cwd`（`normpath`）等于/位于某 run 的 `root` 之下；多个 run 命中时取
  注册表顺序最靠前者。
- 视图（`SessionStore::list_view`，Python `filter_rows`）：默认视图剔除全部登记会话；
  `?debug_run=<id>` 只显示该 run（未登记或不合法的 id → 空列表，不是错误）。视图对
  可见行重新推导 `fork_parent`，并以可见行 + run id 重签 `sig`（`_view_signature`），
  按（已发布列表、注册表、run id）缓存。`/api/live`、`/api/term/list`（sessions 与
  pending）、`/api/search`（候选与 `total_pool`）用同一注册表筛；`/api/messages` 等
  详情路由忽略该参数（Python 同）。`security.rs` 不再对 `debug_run` 返回 501。

## 搜索

真实读根（803 会话、3.4 GB）上逐文件流式投影一次要 40 s 以上（Python 用
`~/.cache/agenthub/search-text` 的 gzip 缓存只要 1–4 s），所以搜索文本按会话
持久化（`search/cache.rs`、`search/service.rs`，2026-09-13）。命中语义、结果
顺序、片段、`scanned`/`truncated` 与逐文件顺序扫描完全一致，只是正文的来源变了。

- **可搜索正文** = `search::body`：主视图里 `user/assistant/user·subagent/
  assistant·subagent/thinking/question/answer` 角色的语义文本按 `\n` 拼接，
  与 Python `_search_text` 相同；工具参数/输出、媒体、游标、私有片段不进正文。
- **缓存条目**：`<SESSIONDOCK_SEARCH_CACHE_DIR>/<source>-<hex>`——目录必须是
  显式配置、已存在、0700 的独立目录（`SESSIONDOCK_STATE_DIR` 由元数据存储独占，
  容不下别的条目；服务不创建、不改权限，宽于 0700 即启动失败）；文件 0600、
  同目录临时文件 + rename；首行 JSON 头（schema、构建指纹、uid、版本键、kind、
  字节数），其后是未压缩正文（真实根 816 会话共 18 MB，读页缓存比解压快，见
  [performance.md](performance.md)）。确定性的打开失败（501/413，如父线程不在
  索引中的 fork）也按版本缓存，不再每次搜索重新流式解析。未配置目录时缓存只在
  内存（≤ 64 MiB）且不预热。
- **版本键**（`SessionStore::search_version`，只 `stat` + 索引，不读正文）：数据
  文件 `size/mtime_ns/dev:ino:ctime`、Claude 显示 pin（`tip`/`stale_end`）、
  Codex 声明的固定前缀链（每个父文件的路径、`cut` 与版本）或使链不可读的图错误；
  另加构建指纹（schema/crate 版本/二进制大小与 mtime），换二进制即整体重建，
  绝不信任旧投影。元数据 sidecar、名字索引、行装饰不影响正文，不进版本键。
  解析前后各取一次版本，期间变化的结果只用不存（Python 同款）。
- **正文来源顺序**（`SearchService::source`）：① 缓存命中（版本相等）→ 流式读；
  ② 视图 LRU 里仍是当前版本的视图（用户正打开着、SSE 增量续读的那些）→ 借用并
  写入缓存；③ 解析：同一 uid 只允许一个生产者（其余等待后重读缓存），先按数据
  文件大小取解析槽（`SESSIONDOCK_SEARCH_WORKERS` 个槽，每 8 MiB 一槽，大文件
  占多个槽，最大文件独占），然后一次性流式投影，投影随即丢弃、解码记录在投影
  完成时立即释放，并在每次大文件解析后 / 每 32 次解析后 `malloc_trim` 把 glibc
  各 arena 的空闲堆还给内核。**追加过的会话整体重解析**，不做"只解码新增字节"：
  增量解码要把该会话的解码记录常驻（约文件的数倍，每个活跃会话几十到几百 MB），
  违反本文的常驻内存原则；Python 也是整体重读；实测活跃的 20 MB 会话重解析
  ≈ 0.3 s，并行后热搜索仍 < 1 s。搜索不占普通读池的任何名额。
- **匹配**：候选来自索引（按 `updated` 倒序，`?debug_run=` 视图筛过），`workers` 个线程并行取正文并匹配，但命中/进度
  严格按候选顺序发出，`limit` 停止点与顺序扫描相同。缓存正文按行对齐的 1 MiB
  块流式匹配（块只在 `\n` 处切，不含换行的模式命中不跨块，首个命中的上下文
  跨块拼接；非最后块末尾的空匹配留给下一块计数），整个正文不进内存；只有可能
  跨行匹配的正则（含转义、字符类、内联标志或锚点）整体读入，受 64 MiB 在途预算。
- **预热**：有持久化目录时启动 2 s 后一趟后台预热（`workers/2` 个线程，
  解析槽按"前台无人等待才取"的低优先级），之后每 `SESSIONDOCK_SEARCH_WARMUP`
  秒（默认 300，0 关闭）复查一遍版本、只补解析变了的会话；预热不占读池、
  不阻塞索引（与列表共用索引 TTL）。
- **容量**：`SESSIONDOCK_SEARCH_CACHE_BYTES`（默认 1 GiB）按最近使用淘汰。
- 准入：2 路并发搜索，第三路最多等 10 s 后 503 `search_busy`；每查询 8 MiB
  结果预算，超出返回 `partial:true`；正则方言限制不变（见 [architecture.md](architecture.md)）。

## 目标与验收

真实读根（772 会话、3.4 GB，本机）：

| 指标 | 目标 |
| --- | ---: |
| `/api/sessions?force=1` 冷 | ≤ 1 s |
| `/api/sessions` 热（stat-only） | ≤ 100 ms |
| 打开最新会话（≤ 100 MB 文件） | ≤ 1 s |
| 列表后 RSS | ≤ 300 MB |
| 打开 20 个会话后 RSS | ≤ 512 MB |
| 依次打开最大的 5 个会话（含 385 MB、两个 304 MB 截图会话）后 RSS，空闲 20 s 不回升 | ≤ ~800 MB |
| 一次全文搜索（无命中、扫全部 800 会话）后 RSS 净增 | ≈ 0 |
| 活跃 CLI 持续追加时连续 30 次列表 | 0 失败 |

验收套件：`tests/inventory_scale_suite.py`（合成 ≥ 1500 会话 / ≥ 1 GB）、
`tests/inventory_live_append_suite.py`（并发追加）、`tests/list_rows_parity.py`
（行字段与 Python `list_sessions` 逐字段对照）、`tests/real_roots_bench.py`
（操作者手动、只读真实根）；既有 73 套（含六套差分与真实 CLI）保持通过。

## 物理工作预算（防病态文件，不是功能上限）

只保留防止单个病态文件打爆进程的几条，数值按 2026-09-12 真实分布（Claude 291
文件 / 352 MB，最大 51.7 MB / 23,763 行 / 最长行 1.3 MB；Codex 470 文件 /
2.6 GB，最大 228 MB / 36,124 行 / 最长行 2.9 MB）留 10–20 倍余量；超限只会
发生在真正损坏的文件上，处理方式是对**该会话**明确 413，列表永不受影响。

| 预算 | 值 | 依据 |
| --- | ---: | --- |
| 单条原生记录 | 64 MiB | 真实最长行 2.9 MB |
| 单文件全量解析 | 4 GiB | 真实最大文件 228 MB |
| 每文件 LF 检查点 | 2,000,000 | 真实最多 36,124 行 |
| 每视图解析记录 / 事件 | 1,000,000 / 2,000,000 | 同上 |
| 视图序列化消息 | 1 GiB | 228 MB 文件投影后约 50–100 MB |
| 进程内索引容量 | 1 GiB | 检查点与 digest 的记账上限（每 LF 28 字节），不是预分配 |
| 非 Grok / Grok 摘要文件 | 256 MiB / 16 MiB | 余量 |
| 会话数 / 读根总字节 | 无 | 列表不解析文件 |

这些值是常量，集中在 `sessions` 模块顶部并引用本表。

## 常驻内存预算（第四十四批 WP-A）

上表是防病态文件的工作上限；真正决定常驻内存的是两个 LRU 缓存，单用户机器上
默认值如下，启动时由环境变量覆盖（`--check-config` 回显）：

| 缓存 | 默认 | 环境变量 | 记账口径 |
| --- | ---: | --- | --- |
| 已解析文件 / 视图 LRU 条数 | 16 | `SESSIONDOCK_CACHE_ENTRIES`（1–256） | 条数；AST 缓存取其一半 |
| 视图 LRU 字节 | 128 MiB | `SESSIONDOCK_VIEW_CACHE_MB`（16–8192） | 视图的序列化消息字节 + 内嵌图片的 base64 驻留字节 |
| 解码 AST 缓存 | 64 MiB | `SESSIONDOCK_AST_CACHE_MB`（0–8192；0 = 不保留，追加全量重解码） | `serde_json::Value` 树的估重 |

实测（真实根，2026-09-13，[performance.md](performance.md#常驻内存第四十四批-wp-a)）：
一个视图的常驻 ≈ 记账字节的 1.2–1.6 倍（文本会话）到 ≈ 文件大小的 1.6 倍
（截图密集的 Codex 会话：≤ 2 MiB 的内嵌图片以 base64 驻留在视图里，这是
[media.md](media.md) 的媒体口径，不在本包范围）。AST 缓存只对小于其预算的文件
生效：它让活跃会话的每次追加免于重解码整文件，对 385 MB 的文件本来也不会保留。

除了缓存上限，进程在每次新解析、视图淘汰、打开视图的历史响应（非 `append=1`
或 > 4 MiB）和每次搜索结束后调用 `malloc_trim(0)`（遍历全部 arena 归还空闲页）：
在 256 核机器上 tokio 曾开 256 个 worker（`SESSIONDOCK_ASYNC_WORKERS` 现默认
`clamp(核数/8, 4, 16)`），每个线程一个 glibc arena，释放的几百 MB 永远留在 RSS 里。
`mallopt(M_ARENA_MAX, 2)` 实测被否决：并行索引读取和读 worker 争抢两个 arena，
十次 `/proc` 扫描的 futex 调用从 2.6k 涨到 564k，CPU 反而更高。搜索的瞬时投影
不进 LRU、不记 AST，扫完即释放并 trim，RSS 不净增。

`INDEX_BYTES`（1 GiB）与 `VIEW_BYTES`（1 GiB）仍是记账上限：前者每 LF 只占 28
字节，后者只在单个视图序列化超过 1 GiB 时才 413；两者都不是预分配。

## 明确不做

- 不做落盘的**列表**索引缓存：没有启动解析，也就没有需要缓存的东西。搜索
  文本缓存是按会话、按文件版本的正文快照，不是列表状态，丢了只会多解析一次。
- 不做跨文件的"一致性快照"：没有任何消费者需要它。
- 不为搜索预建倒排/全文索引：正文缓存 + 正则流式匹配已经比 Python 快，
  倒排索引才是"超出 Python"。
