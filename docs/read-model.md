# 读模型设计：惰性索引 + 按需视图

这是会话读模型（列表、详情、分页、SSE、搜索、运行时身份的数据来源）的
**唯一有效设计**。它取代了 2026-09-12 之前的"冻结库存"（全量启动解析）；
那套设计已废弃，不再是任何代码或文档的依据。

## 原则

1. **没有启动解析。** 服务启动后第一次列表只做目录遍历 + `stat` + 每个文件的
   有界头/尾读取；不解析任何文件的全文。
2. **每个文件相互独立。** 一个文件在读取期间被追加、重写或删除，只影响它自己
   这一行/这一份视图；永远不存在"整份索引作废"。
3. **只在打开时才解析。** 会话视图（消息投影、LF 检查点、媒体片段）只为被打开
   的会话建立，按需增量续读，进有界 LRU；不为未打开的会话保留任何消息对象。
4. **常驻内存与数据总量无关。** 常驻的只有每个文件一条行摘要（几百字节）
   和 LRU 里有限个视图。
5. **没有人为的历史容量上限。** 不以单文件、单记录、记录数、事件数或索引
   大小拒绝读取；实际分配和 I/O 失败仍正常报告。
6. **搜索不重复解析。** 每个主会话的可搜索正文按文件版本持久化在搜索文本
   缓存里（见下文"搜索"）；一次搜索只读缓存、只解析版本变了的会话，
   投影结果不留驻，命中语义与逐文件流式扫描完全一致。
7. **多线程用在读摘要上。** 头/尾读取在有界阻塞线程池上并行；并行不是用来给
   全量解析提速的。

## 组件

| 组件 | 职责 | 输入 | 输出 |
| --- | --- | --- | --- |
| `sessions/index` | 目录遍历、`stat`、并行头/尾摘要、按 stamp 缓存、行推导、归属图、`sig`/`built_at` | 三个读根 | `/api/sessions` 行、候选文件表（uid → 路径/来源/stamp/原生 id/归属）、运行时原生目录 |
| `sessions/index/summary` | 三家来源的有界摘要：头 96 KiB（≤ 40 条记录）+ 尾 512 KiB | 单个文件 | `RowSummary` |
| `sessions/views` | 单会话视图：经既有 `RecordCache`/`RawIndex`/provider 投影流式解析**这一个**文件，增量续读，重写重建；LRU（条数 + 字节） | 候选文件 + 可选时间线 pin | `ViewSnapshot`（消息、分页、媒体、检查点、原生输入证据） |
| `search` + `search/cache` + `search/service` | 搜索文本缓存（按会话、按文件版本持久化的语义正文）、解析预算、按需构建；未缓存的会话借用已缓存视图或流式投影后丢弃；有界准入与 `partial` | 查询 + 候选表 + 缓存目录 | NDJSON 命中流 |
| `observe`（SSE） | 每会话 `stat` 轮询 + 视图增量扩展；列表 SSE 用索引 `sig` | 索引 + 视图 | 事件流 |
| 运行时 / lifecycle / trash / delivery | 从候选表取原生 id、路径、stamp、归属；发送确认边界与原生尾部来自打开的视图 | 索引、视图 | — |

## Agy 原生数据投影

当前 Agy 1.2.16 实现用成对的 `SESSIONDOCK_AGY_HOME`（显式原生目录）与
`SESSIONDOCK_AGY_ROOT`（私有可写镜像）接入：只读目录 DB 加完整
`brain/<sid>/.system_generated/logs/transcript_full.jsonl`，生成
`<AGY_ROOT>/cli/<sid>/summary.json`、`messages.jsonl`，复用下游索引、历史分页、
增量、搜索和媒体。持续连接的 `data_version` 负责目录提交，已知 transcript stamp
负责正文变化；追加保留前缀，旧记录改写/回退原子重写，相同字节不改 mtime。
完整目录 DB 暂时缺失保留镜像；目录行尚在但 transcript 缺失则保留上次消息镜像，
摘要标记 `transcript_missing`，历史详情显示暂不可读提示（从未导出时消息文件才
为空）；恢复后重新核对并清除提示。成功读取
目录发现行被删时只清理带本实现标记的私有镜像，不修改 native 文件。

真实 user、planner final、thinking、错误与 fd identity 已核对；工具/媒体字段来自
CLI 内置格式文档，尚无成功真实工具/媒体样本。工具调用保留原始 JSON，其他步骤
独立投影结果，不按位置猜配对；图片使用既有媒体授权，其合成 schema browser
路径已有通过记录。Agy 不输出 `turn`，未证实 busy；parent/status 的读取也不代表
原生子代理/回合状态已支持。具体合同及验证边界见 [Agy](agy.md)。

Agy 的原生 `SYSTEM_MESSAGE` 独立显示为带类型标注的系统气泡，保留通知来源、
优先级和时间，不作为工具结果或回复计数。用户外层包裹的附加元数据与设置变更
分别投影为 metadata 与系统气泡；用户正文里的同名 tag 保持原样。每条投影携带
`native_type`，未知类型/不完整包裹保留可见内容，规则详见 [Agy 历史投影](agy.md#历史投影与证据)。

## 列表：索引与摘要

- **候选发现**：Claude `<root>/<project>/<sid>.jsonl` 主会话与 `agent-*.jsonl`
  sidecar；Codex 根下递归的 `rollout-*.jsonl`；Grok `<root>/<dir>/summary.json`
  （+ `chat_history.jsonl`）。目录递归和普通文件链接跟随 `glob`/`rglob`
  语义，不另设目录层数、路径组件、canonical root 或硬链接数量门槛。
  已配置的原生读根不存在时，该来源暂为空，不阻断其他来源或服务启动，也不创建目录；
  后续扫描会自动发现重新出现的目录。权限及其他 I/O 错误仍按原有错误语义报告。
- **stamp** = `dev/ino/size/mtime_ns`。摘要缓存以 stamp 为键：stamp 未变则
  热刷新只有 `stat`；变了只重读这一个文件。
- **摘要读取**：头 96 KiB 内最多 40 条完整记录 + 尾 512 KiB 内的完整记录
  （残行丢弃）；推导 `title`
  （custom-title > 最新 ai-title > 首条用户输入生成；Claude 都没有时为
  `新建 Claude 会话`，Python 回退为文件名前 8 位，DELTA）、`cwd`（头部优先，尾部
  计数兜底）、`branch`、`created`、`updated`（`mtime`）、`size`、`model`、
  Codex `session_meta`/`history_base`、Claude `sessionId`/fork 来源、Grok
  `summary.json` 字段。Grok 的 `size`：会话目录内全部普通
  文件字节之和（递归、不进入链接目录、没有额外深度或条目数门槛），与
  摘要一起按 summary/chat 的 stamp 缓存。头/尾里
  发现的硬错误（`content` 标量、缺 id 等）使该行 `supported:false` 并给出
  `migration_warnings == [原因]`；坏行、重复 `session_meta` 与未知记录类型只计入
  **详情 `meta.migration_warnings`** 的非致命备注（`跳过无效的JSONL 记录 ×N`、
  `跳过重复的Codex session_meta ×N`）：公开列表行（含 `agent_items`）只在
  `supported:false` 时带 `migration_warnings`（前端不消费，
  真实根上它占了列表载荷的一半以上）。
- **当前模型**（BUG-20261003-110654-be9964）：Claude 取最近一条有效 assistant
  的 `message.model`，Codex 取最近一条有效 `turn_context.payload.model`。
  主会话跳过 Claude sidechain；子代理只读自己的文件；空值和 `<synthetic>`
  合成回复标记不覆盖已知模型。冷读以块反向查找，追加只检查上次 LF 后的完整行，
  缓存标量模型与 Codex 回合状态；记录不受头/尾摘要窗口限制，截断、同尺寸重写及
  inode 替换重建。列表、详情、搜索元数据使用同一索引字段，模型变更会刷新已打开
  的标题栏。显示的是最近原生记录确认的模型：CLI 尚未写入模型记录的选择无法由
  转录确认。相对冻结 Python 的 Claude 空值 / Codex 首个模型，这是用户要求的
  **DELTA**。回归见 `tests/session_model_browser.py`。
- **并发变化**：读头/尾前后各 `stat` 一次；不一致则重读（最多 3 次），仍不一致
  就按已读字节发布并带上读取时的 stamp。删除的文件在下一次刷新消失。
- **归属图**（`index/graph`）：Claude sidecar 归属主会话（目录 + 文件名 +
  头部 `sessionId`）；Codex 子代理 rollout 归属（头部元数据）；fork 父子
  （`history_base`/`forked_from`）。规则与 [history-pages.md](history-pages.md)
  中记录的一致，输入改为摘要。
- **子代理运行态与续写**（`index/agent_stops`）：`agent_items[].active`
  ——Codex 看子代理 rollout 最后一条回合边界 `event_msg`
  （`task_started`/`turn_started` 开、`task_complete`/`turn_complete`/`turn_aborted` 关）；
  回合边界不受头/尾窗口限制：Codex 冷读按块反向查找最近边界，只解码回合与
  最新问题调用的标量字段；后续仅检查上次 LF 后新增的完整记录，缓存标量状态。
  扫描期间同一文件继续追加时保留已扫描前缀的状态和原 stamp，下次读取新增后缀；
  不把普通追加当成不可读，也不让会话行暂时丢失。重写、截断与 inode 替换仍使缓存失效。
  半行不生效，截短、同尺寸重写或 inode 变化重建；主会话 `turn` 使用同一状态。
  Codex 恢复后的子代理可能保留未关闭的旧回合，但父线程后续已报告它不在执行
  （BUG-20261005-124541-c50fee）。增量扫描直接父线程各 rollout 代里的
  `collaboration.list_agents` 调用与同 `call_id` 结果，按完整 `agent_name` 匹配：
  `pending_init`、`idle`、`shutdown` 或 `completed`/`errored`/`interrupted` 状态
  撤销不晚于该结果的旧回合活动。只有子代理新的回合开始边界才能重新激活；
  settings/token 等记录不算唤醒，未知状态、无匹配调用和引用正文不算停止证据。
  该状态校正用于列表和子代理活动标记，不改原生历史；相对仅看子代理边界的
  基线为本缺陷修复的 **DELTA**。回归见 `tests/turn_state_browser.py`。
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
  `continued_in` uid（按路径序最后一个同 sid 行胜出，
  指向自身丢弃），解析不到则不出字段。Codex 同线程轮转的旧代 rollout 行同样
  带 `continued_in` = 最新一代 uid（最新一代链校验通过时），列表只显示一行。
- **主会话回合状态**（`turn`）：Claude/Codex 主会话行按尾部从新到旧第一条能定论的记录
  给出最近一个回合的状态，词汇与对话 `activity` 相同。Codex 看最后一条回合边界
  `event_msg`：`task_started`/`turn_started` 为 `working`（其后最新一条 response item
  是提问工具调用则为 `waiting`），`task_complete`/`turn_complete` 为 `idle`（带 `error`
  为 `failed`），`turn_aborted` 为 `aborted`。Claude 跳过 sidechain、`isMeta`、注入文本与
  `!` shell 记录：`system` 的 `turn_duration` 或压缩边界为 `idle`，Esc 中断标记为
  `aborted`，含提问工具调用的 assistant 为 `waiting`，API 错误为 `failed`，`end_turn`
  为 `idle`，其余输入、工具调用与工具结果为 `working`。主回合已收尾但仍有
  `agent_items[].active` 的子代理（后台子代理还在跑、主会话在等它们）时，非 `waiting` 的
  `turn` 记为 `working`。后台任务同理：尾部主转录里 Monitor 结果带 `taskId`、后台 Bash
  结果带 `backgroundTaskId` 即开始，带 `<status>` 或 Monitor 到期事件的
  `<task-notification>`（user 记录或 `queue-operation` 正文）、`TaskStop` 结果的 `task_id`
  即结束；Monitor 命令监视的是已结束任务的 `tasks/<id>.output` 时（看门狗的 `tail -F` 会一直活到到期，却不会再有进展）不计入；仍在跑的个数写进 `background`（没有则不出字段），非 `waiting` 的 `turn` 记为
  `working`。开始于尾部窗口之前的任务看不到。尾部没有能定论的记录、Grok 与 OpenCode 不出字段。`turn` 只说转录写到哪里，进程是否还在由 `/api/live` 决定；
  前端只在会话运行中时区分轮转中/等待/空闲，正在看的会话再用对话 `activity` 与
  [CLI 状态对象](cli-state.md)的 `instance.busy` 覆盖（等待回答优先）。
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
- LRU：默认 16 条、128 MiB 视图记账预算，最近使用淘汰，依赖 stamp 变化即
  失效。缓存预算决定保留量，不是历史可读容量或 RSS 保证。
- **继承前缀共享**（`views::PrefixCache`，2026-09-15）：Codex 分叉子会话继承父文件
  `[0, cut)` 的投影按（父 uid、cut、父 stamp）缓存（≤ 8 条、同一 128 MiB 视图字节
  预算），已打开的视图、SSE 续读和搜索的瞬时投影共用同一份；十个分叉共享一个 1.6 GB
  父文件时只流式解析一次。父文件 stamp 一变即重读，不按内容猜。

### 大文件的读取代价（2026-09-15）

打开会话必须把这个文件（及其继承前缀）的每个字节读一遍：LF 边界、记录计数、
`message_total`/`partial.omitted`、语义锚点都依赖全部记录。但"读一遍"不等于
"全部解码驻留"，GB 级会话的代价被压到接近 I/O：

- **指纹代替 SHA**：物理索引每行的前缀校验点、追加复用时对旧前缀的整段校验、
  私有 span 的读回校验，全部改用 `crate::fingerprint`（128 位非加密混合，
  单核数 GB/s）。它们校验的是本机文件是否在我们眼皮底下变了，不是对抗性完整性；
  能改写原生根的人本来就拥有数据。SHA-1 仍用于 `head`（首 4 KiB）、`sig`、
  uid 等短输入。
- **64 KiB 以上的字符串一律是 span**（`budgets::INLINE_STRING_BYTES`，原 2 MiB）：
  图片以 `NativeSpan` 留在文件里、GET 时按范围读回校验，不再以 base64 驻留在
  AST 和视图中（截图密集会话的常驻从"≈ 文件大小的 1.6 倍"降到与图片无关）；普通
  巨型文本仍从 stamped 来源读回进记录。AST 也因此小到能进 64 MiB 的追加复用缓存：
  活跃的 GB 级会话每次追加只解码新增行。
- **小记录先走 serde_json**：≤ 64 KiB 的完整行不可能含 span，`serde_json` 与结构
  扫描器对它接受的输入产生相同的 `Value`（保序、重复键后者胜、同一数字解析器）；
  它拒绝的（超过其递归深度、真正的语法错误）再交给没有深度门槛的扫描器裁决。
- **长字符串用 SIMD 跳过**：扫描器在字符串内用 `memchr2` 找下一个引号/反斜杠，
  逐行读取器只在本次能拷贝的窗口内找 LF（原先每 8 KiB 就重扫整段 64 KiB 缓冲，
  长行是二次方开销）。

实测见 [performance.md](performance.md#大文件-2026-09-15)。仍未做的：不读中间
字节的"真正局部解析"需要把物理索引与浅投影落盘（按文件版本），留作下一步。
- 每视图契约不变：游标 schema `rs-m2-1`、前缀散列、语义锚点、时间线 pin、
  `valid_checkpoint`、
  分页/媒体授权、`message_total`/`partial`/`activity`。见
  [history-pages.md](history-pages.md)、[native-input.md](native-input.md)、
  [media.md](media.md)。

### 视图字节缓存（2026-09-15）

热读（同一文件版本的第二次 `/api/messages`）以前仍要把每条消息 `Value` 克隆一遍再
序列化：隔离实例上 52 MB 的 Claude 会话每次全文 105–119 ms，404 MB 的 Codex 会话
437–444 ms，`window=1` 首屏 18–33 ms（[performance.md](performance.md#热读视图字节缓存2026-09-15)）。
现在每个已解析文件（`Parsed.encoded`，
`views/encoded.rs`）在投影之后**只序列化一次**，把结果留在视图里，
HTTP/SSE 的响应（`views/body.rs`）按字节拼接：

- **单位是一个事件列表**：叶文件 `parsed.events` 一份，视图的继承前缀
  `inherited` 一份（与链一起复用）。每条非 status 消息的 `serde_json::to_vec` 原样
  输出首尾相接、逗号分隔，并记每条的起点，于是任意连续的一段消息位置就是一次
  `memcpy`；status 事件另存（不进 `messages`，但进语义 digest）。
- **同一次序列化还产出**语义 digest（`projection_digest(events, committed)`，
  与旧算法逐字节相同的 SHA-1 输入）和 LRU 记账（Σ 消息字节），代替以前解析时的两遍
  丢弃式序列化；过期检查点的锚点（`start < committed`）也从缓存字节算 digest，不再
  为一次增量请求重新序列化整个前缀。
- **响应 = 小字段 + 拼接**：`meta`/`version`/`reset`/`start`/`end`/`anchor` 由 serde_json
  照旧生成、去掉收尾的 `}`，接 `"messages":[` + 缓存片段 + `]`，再接
  `message_total`/`partial`/`activity_changed`/`activity`，最后由 handler 追加
  `prompt`（主视图）并收尾。全文、`window=1`（头 100 + 尾 500 是两段
  `memcpy`）、`start=/head=/anchor=` 增量（`end > start` 的后缀）、`append=1`、
  历史页（`/page?cursor=`，按非 status 位置切片）都走这条路；分页预算
  （`pages::Budget`）用缓存里记下的消息长度与文本引用数，不再为了称重再序列化 600 条。
- **逐字节相同**：可缓存的消息就是它自己的 `to_vec` 输出，周围字段由同一个
  `json!` 值序列化，字节渲染器的输出与原 `Value` 渲染器逐字节相等（原
  `Value` 渲染器只为测试保留，已随单元测试于 2026-10-06 删除）。
- **不是纯函数的消息每次照旧投影**：带类型化图片的事件（描述符注册是每次请求的
  副作用，超过 16 张还要签发 `media_more` grant）和正文里发现了图片引用的事件
  （文件 token 每次随机）在编码时标为 `special`，请求时克隆 + 投影 + 序列化，与
  以前完全一样；其它消息直接拷贝。Codex 重命名事件（`rename_at`）按 `ts` 插在序列
  里，只切断一次拷贝。
- **追加只编码新尾巴**：追加后的重投影（仍是全量投影）对照上一次解析逐条比较
  （`same_value`：键序敏感、`-0.0`/`0.0` 区分——`Value::eq` 都不区分，而字节要求
  区分），`end` 与消息树都相同且无类型化媒体的消息直接拷贝旧字节，只有新尾巴和被
  后续记录改写的旧消息（Codex `turn_aborted`、Claude 分支）重新序列化；重写/截断
  让比较失败即全部重编码。测试证明扩展后的字节与从未见过旧文件的进程投影相同。
- **单飞**：字节在 `Views::open` 持有视图锁时随解析建立，8 个并发冷读只编码一次，
  其余等锁后拼接同一份；热读不建任何东西。
- **瞬时投影不留字节**：搜索的 `open_transient` 以同一遍流式序列化算 digest 与记账，
  `retain=false`，峰值内存不变。
- **预算**：见下文"常驻内存预算"——字节与 `Value` 树记在同一笔账上（`SESSIONDOCK_VIEW_CACHE_MB`），
  不另设旋钮；代价是常驻倍率从记账的 1.2–1.6× 估为约 2.2–2.6×（同一预算最坏多
  128 MiB；实测见 performance.md，打开 404 MB 之后的 RSS 反而从 472 降到 415 MB）。

### 视图模块的接口（`sessions/views`）

- `Views::open(&ViewRequest, &dyn Dependencies) -> Arc<ViewSnapshot>`：`ViewRequest`
  由索引填充——`uid`（主会话）、`agent`（`""` 或精确 agent id）、`owner`
  候选文件、`selected`（agent 自己的文件，主视图为 `None`）、`pin`（Claude
  主会话的时间线 pin）、`row`（已发布并经 names/metadata 装饰的主会话行；
  视图 `meta` 就是这一行，agent 视图按 `row.agent_items` 派生）。
  候选只带索引发现的路径（来源/根/数据文件/sidecar），
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
- 打开视图时列表的新鲜度：`open` 复用 3 s 内（`OPEN_TTL`）
  的索引快照——视图自己 `stat` 它显示的文件，新文件 / 归属变化在几秒内仍会出现；
  视图读到比索引更新的字节时仍立即重扫一次，`meta` 与字节始终一致（节流过一次，
  撤回）。`/api/sessions` 与列表调用仍按索引自己的 500 ms 窗口；
  `/api/live` 与 spawner tick 用同样的 3 s 窗口（`list_recent`）。目录遍历后如果
  每个文件的 stamp 都与上次相同（且名称索引未变），直接复用上一份快照，不重建
  行 / 图 / 签名。`/api/sessions?sig=` 命中时不克隆、不装饰、不序列化文档。
- **列表响应字节缓存**（`SessionStore::list_view_bytes`，2026-09-15）：`sig` 短路之后、
  `sig` 变了或没带 `sig` 的热请求也不再"克隆文档 → 借视图装饰 → 去警告 → 序列化"
  （真实根 480 行 / 438 KB 一次约 10 ms），而是留一份最终响应
  字节（`Bytes`），键是 **已发布文档的 `Arc` 身份**
  （`publish` 在 `sig` 不变时复用同一份 `Arc<Value>`，所以键变化 ⇔ 当前 `sig`
  会变）
  加 **视图缓存修订号** `Views::revision`（缓存的 `(uid, agent)` 视图被插入、
  换成新快照或淘汰时递增——列表从视图缓存借来的只有 `cursor.anchor` 与
  `timeline_pin`，修订号不变即装饰不变；同一文件未变的重复打开返回同一快照，
  不递增）。命中返回同一块共享缓冲，8 个并发全列表请求排队在缓存锁后依次命中
  而不是各自重建。`force=1` 仍然重扫并重新渲染（与前身一致），结果替换缓存项；
  渲染时视图锁被正在进行的打开占住则照旧无装饰返回且不缓存。缓存与非缓存
  路径字节相同（`sessions::tests::list_bytes_*` 断言）。

`GET /api/sessions?children=hidden` 用于「按需展开」：初次只传主会话与 `child_count`，
省去子会话行和 `agent_items`。点击箭头时追加 `expanded` JSON 数组，每项是
`{source, sid, node_id}`（本地 `node_id` 可为空）；只返回这些父会话的直接附属会话和
子代理，更深层保留计数。收起从本页展开集合移除对应分支及后代；刷新页面不保留展开态。
扫描和内部索引保持完整，详情与全文搜索接口不受影响。精简形态的字节缓存和签名均含
展开选择，轮询支持 `unchanged`，不同分支不会误用缓存。节点用 `child_counts` 聚合
跨节点附属关系的父身份与数量，Hub 把它们加到对应主会话的箭头计数；原生 SID 相同的
不同机器仍分别匹配。前端同时按当前展开集合过滤搜索结果。

## 测试数据隔离

测试使用临时独立数据根，不通过生产显示过滤隐藏会话。旧
`debug-runs.json` 不再读取、导入或修改；`debug_run` URL 参数作为未知参数忽略。
列表、搜索、live、term 与 Hub 始终使用正常会话集合，共享普通缓存与 SSE 观察器。
独立测试节点通过 `SESSIONDOCK_CLAUDE_ROOT`、`SESSIONDOCK_CODEX_ROOT`、
`SESSIONDOCK_GROK_ROOT` 指定各来源的临时目录；OpenCode 使用临时数据库和镜像根。

## 搜索

字面搜索默认是多关键词 AND（2026-09-15）：`q` 按空白拆词，双引号保留连续短语，
例如 `部署 失败` 要求整个会话正文同时包含两个词，顺序不限、可以在不同消息中；
`"部署 失败" 重启` 要求连续的“部署 失败”以及“重启”。`mode=any` 选择 OR，
省略 `mode` 或其他值使用 AND。`AND`、`OR` 本身是普通关键词。空词与完全相同的
重复词忽略，不做中文自动分词；未闭合引号把剩余输入作为短语，引号内 `\"`、`\\`
分别表示字面引号、反斜杠。默认不区分大小写，`case=1`、`word=1` 分别作用于每个词。
`regex=1` 仍把原始 `q` 作为一个正则，忽略 `mode`，不拆词。

前端搜索框内的 AND / OR 按钮切换“全部词 / 任一词”，偏好保存在 `sessiondock.opts`。
匹配方式与 `Aa`（大小写）、`ab|`（全词）、`.*`（正则）按钮保持单排布局；
AND 与 OR 始终使用相同的蓝色样式，通过文字表示当前匹配方式；
正则启用时匹配方式按钮禁用，但保留颜色和位置。
即时筛选无需回车，将标题、目录、节点名、Agent 来源标识与显示名、模型名、
代理类型、原生会话 ID（UUID）和 UID 作为同一候选文本；复用上述匹配选项，
只匹配已加载的列表字段，不发起全文搜索。子代理使用自己的标题、模型、类型与 ID，
共用所属会话的 Agent 来源和机器名；目录缺失时沿用所属会话目录，
不加入所属主会话的 ID 或模型。缺失字段按空值处理。完整 ID 或任意连续片段都可筛选。
Enter 匹配正文，正文未命中或不可读时也按同一查询匹配该行的 ID；正文与 ID 分别
应用 AND / OR、大小写、全词及正则选项，不跨两者拼凑关键词。ID 命中显示其摘要，
不把元数据写入正文缓存，也不改变详情正文。预览、正文高亮和
导航分别匹配每个词，使跨消息满足 AND 的所有词都可见。命中数是各个不同关键词的
出现次数之和（重叠关键词分别计数），上限仍为 200。摘要在各词首次命中的上下文中
优先选覆盖词最多的片段，再补其他词的片段，以 ` … ` 分隔；不保留整个会话正文。

子代理也按自己的视图独立匹配，关键词不跨主会话与子代理或不同子代理拼凑。
搜索结果的 `agent_items` 只含命中的子代理，每项带自己的 `hits`、`hits_capped`、
`snippet`；仅子代理命中时，响应中的所属主会话只承载身份与导航元数据，其 `hits` 为 0，
左栏平铺模式不显示未命中的主会话行；分层模式保留命中行所需的父会话作为结构入口，
但不显示其摘要或命中次数，也不带出未命中的兄弟会话。
子代理显示自己的命中次数与高亮摘要，点击直接打开该子代理。
搜索结果计数只统计实际命中的主会话与子代理行，不计结构入口；即时标题筛选遵循同样规则。
两行摘要缩短命中前的上下文，避免关键词及其高亮被裁切；悬停仍可查看完整摘要。
独立进程的附属子会话仍按自身正文参与搜索，不因父会话命中而带出未命中的子会话；
平铺模式中，命中的独立子会话按根项显示；分层模式沿可见附属关系保留必要的父级结构。
即时标题筛选同样只展开匹配的子代理。
搜索与即时筛选结果默认展开，不沿用主列表的日期、目录分组或子会话折叠状态；
结果中可手动折叠分组及子会话，列表刷新保留本次折叠。修改查询、再次 Enter 搜索
或退出搜索会清空搜索折叠；主列表保存的折叠状态保持独立。
列表刷新只更新搜索结果的元数据，不恢复未命中的子代理。`scanned`、`total_pool` 和
结果上限仍以主会话候选计数；子代理解析失败带 `agent` 标识，计入显式部分失败。
子代理复用按原生文件 UID 与版本保存的正文缓存；冷搜索投影只读取自己的正文及
声明的继承前缀，不为生成原生操作范围反复解析所属主会话。搜索投影不用于原生操作授权。

真实读根（803 会话、3.4 GB）上逐文件流式投影一次要 40 s 以上，所以搜索文本按会话
持久化（`search/cache.rs`、`search/service.rs`，2026-09-13）。命中语义、结果
顺序、片段、`scanned`/`truncated` 与逐文件顺序扫描完全一致，只是正文的来源变了。

- **可搜索正文** = `search::body`：主视图里 `user/assistant/user·subagent/
  assistant·subagent/thinking/question/answer` 角色的语义文本按 `\n` 拼接，
  工具参数/输出、媒体、游标、私有片段不进正文。
- **缓存条目**：`<SESSIONDOCK_SEARCH_CACHE_DIR>/<source>-<hex>`——配置目录后以
  同目录临时文件 + rename 更新；首行 JSON 头（schema、构建指纹、uid、版本键、kind、
  字节数），其后是未压缩正文（真实根 816 会话共 18 MB，读页缓存比解压快，见
  [performance.md](performance.md)）。确定性的打开失败也按版本缓存，不再每次
  搜索重新流式解析。未配置目录时缓存只在
  内存（≤ 64 MiB）。
- **版本键**（`SessionStore::search_version`，只 `stat` + 索引，不读正文）：数据
  文件 `size/mtime_ns/dev:ino:ctime`、Claude 显示 pin（`tip`/`stale_end`）、
  Codex 声明的固定前缀链（每个父文件的路径、`cut` 与版本）或使链不可读的图错误；
  另加构建指纹（schema/crate 版本/二进制大小与 mtime），换二进制即整体重建，
  绝不信任旧投影。元数据 sidecar、名字索引、行装饰不影响正文，不进版本键。
  解析前后各取一次版本，期间变化的结果只用不存。每次搜索仍为每个候选 `stat`
  一次（899 个候选 ≈ 17 ms CPU，8 个 worker 摊到 2–3 ms 墙钟）：这是"追加后的
  下一次搜索就能搜到"（`tests/search_cache_suite.py`）的代价——索引快照 ≤ 500 ms
  一刷，改用它的戳会让刚追加的会话在下一次刷新前搜到上一版正文，实测被该套件
  拒绝。
- **折叠副本与预筛**（`search/fold.rs`、`search/prefilter.rs`，2026-09-15）：
  缓存为每个提供过或生成过的正文在内存里常驻一份大小写折叠副本
  （`SESSIONDOCK_SEARCH_FOLD_BYTES`，默认 128 MiB，按最近使用淘汰；真实根 899
  会话 19.3 MB）。折叠 = 把每个字符映射为其 Unicode simple case folding 等价类
  （`regex-syntax` 的 `ClassUnicode::case_fold_simple`，正是匹配器 `(?i)` 用的表）
  中码点最小的成员：匹配器认为 `c` 与 `d` 大小写相等 ⟹ 折叠相同，所以任何能
  命中的正文，其折叠文本必含折叠后的 needle（`fold::tests` 枚举全部码点验证
  每个等价类与 `regex`/`fancy-regex` 一致，且 U+1FFFF 以上无映射）。折叠不保长
  （`K` U+212A → `K`），因此预筛只回答"是否包含"，从不给位置；位置一律来自原文。
  每个查询先在折叠副本上跑预筛：单词字面/全词查询要求折叠 needle 出现；多词
  AND 要求每个折叠词出现，OR 要求至少一个出现，超出预筛子句预算时只减少预筛、
  不改变精确匹配或拒绝输入；正则查询从
  `fancy-regex` 自己的语法树取**必需字面量**（串联、分组、正向 lookaround、下限
  ≥ 1 的重复里的字面量必须出现；交替取各分支各一个字面量组成"至少一个"子句；
  字符类、`.`、反向引用、负向 lookaround、`*`/`?` 不要求任何东西；至多 8 个子句、
  每子句 32 个字面量，超出即不预筛）。预筛不通过的候选不打开缓存文件；通过的才
  按下文流式匹配。无命中的查询因此除版本 `stat` 外只读内存。启动后第一次为某
  版本提供正文时整体读一次以生成折叠副本。
- **正文来源顺序**（`SearchService::source_at`）：① 缓存命中（版本相等）→ 流式读；
  ② 视图 LRU 里仍是当前版本的视图（用户正打开着、SSE 增量续读的那些）→ 借用并
  写入缓存；③ 解析：同一 uid 只允许一个生产者（其余等待后重读缓存），先按数据
  文件大小取解析槽（`SESSIONDOCK_SEARCH_WORKERS` 个槽，每 8 MiB 一槽，大文件
  占多个槽，最大文件独占），然后一次性流式投影，投影随即丢弃、解码记录在投影
  完成时立即释放，并在每次大文件解析后 / 每 32 次解析后 `malloc_trim` 把 glibc
  各 arena 的空闲堆还给内核。**追加过的会话整体重解析**，不做"只解码新增字节"：
  增量解码要把该会话的解码记录常驻（约文件的数倍，每个活跃会话几十到几百 MB），
  违反本文的常驻内存原则；实测活跃的 20 MB 会话重解析
  ≈ 0.3 s，并行后热搜索仍 < 1 s。搜索不占普通读池的任何名额。
- **匹配**：候选来自索引（按 `updated` 倒序；同一
  已发布列表的候选行只构造一次、各次搜索共享），`workers` 个
  线程并行取正文并匹配，但命中/进度
  严格按候选顺序发出，`limit` 停止点与顺序扫描相同。缓存正文按行对齐的 1 MiB
  块流式匹配（块只在 `\n` 处切，不含换行的模式命中不跨块，首个命中的上下文
  跨块拼接；非最后块末尾的空匹配留给下一块计数），原文整体不进内存；只有可能
  跨行匹配的正则整体读入；在途预算只调度内存使用，不拒绝有效查询。字面查询
  直接用 `regex` crate 的字面量搜索（`fancy-regex` 对纯字面量本就委托给它）；
  **全词**字面查询取字面量的每个出现，再检查前后邻字符。邻字符在
  `[\p{L}\p{N}_]`（`regex-syntax` 同一张表）里则挡住这次出现；汉字、平假名、
  片假名、谚文、注音与其它字母直接相邻时不算挡住，所以 `tag` 命中
  `无法识别的tag`，`猫` 仍不命中 `猫猫`，数字和 `_` 仍粘在词上。该规则与
  `whole_word` 的环视逐位置等价，但不再让回溯引擎在出现之间逐字符扫描（lookbehind 使引擎失去
  字面量预扫，原来 19 MB 要 3 s CPU）；多词各用一个字面匹配器，在扫描器里跨块
  保留每个词的命中状态，AND 必须找到所有词才返回结果（即使某个词已达计数上限），
  不合成为回溯正则；不含换行的词和短语仍分块读取。`regex=1` 的查询仍由 `fancy-regex`
  提供 lookaround 与 backreference。`search::tests::matchers_equal_the_reference_pattern…`
  用改前的单一 `fancy-regex` 模式作参照，对每种查询形状、整体与分块逐一断言
  命中数、上限与片段相同。未知 `source` 是
  空筛选，`flags` 只有数值等于 `1` 时启用。
- **按需构建**：仅搜索请求为候选检查版本并构建或复用正文缓存；启动和空闲期间
  不遍历会话预热，也不定时刷新历史。旧 `SESSIONDOCK_SEARCH_WARMUP` 环境变量
  不再读取（与其它未识别变量一样忽略），任何值都不能开启后台扫描。已有持久条目
  重启后仍复用；追加或改写由下一次搜索的版本检查发现。
- **容量**：`SESSIONDOCK_SEARCH_CACHE_BYTES`（默认 1 GiB）按最近使用淘汰磁盘
  条目；`SESSIONDOCK_SEARCH_FOLD_BYTES`（默认 128 MiB）单独按最近使用淘汰常驻
  折叠副本（`CacheStats.folded_*` 记账 = 折叠字节 + 每条 256 B），被淘汰的
  正文退回"读文件、不预筛"。
- 搜索并发在服务内部排队；没有 10 秒 Busy、查询长度、编译大小或结果总字节数的
  Rust 专属拒绝。`limit` 控制命中条数。搜索结束只在本次解析过正文时
  `malloc_trim`（热搜索不再每次花 6 ms 遍历 arena）。

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
| 一次全文搜索（无命中、扫全部 800 会话）后 RSS 净增 | ≈ 0（折叠副本常驻一次，≤ `SESSIONDOCK_SEARCH_FOLD_BYTES`） |
| 活跃 CLI 持续追加时连续 30 次列表 | 0 失败 |

验收套件：`tests/inventory_scale_suite.py`（合成 ≥ 1500 会话 / ≥ 1 GB）、
`tests/inventory_live_append_suite.py`（并发追加）、`tests/list_rows_parity.py`
（行字段与 Python `list_sessions` 逐字段对照）、`tests/real_roots_bench.py`
（操作者手动、只读真实根）；既有 73 套（含六套差分与真实 CLI）保持通过。

## 历史容量与读取边界

不再设置 64 MiB
记录、4 GiB 文件、100 万记录、200 万事件/检查点、1 GiB 消息/索引以及摘要
文件的固定拒读门槛。源文件的已验证长度限定实际读取范围；JSON 语法、完整行、
源身份、跨度长度和 digest 校验仍生效。AST 节点/键/驻留记账上界从实际输入长度
推导，缓冲区随已读数据增长，不按允许上限预分配。

历史分页的 8 MiB JSON / 128 张图片 / 24 MiB 图片估算是分组目标。超过目标的
单条消息独占一页，下一页仍能继续；每页事件数限制和游标一致性校验保留。

结构扫描不另设重复键、嵌套深度、节点/键/数字长度门槛；重复键
由最后一个值胜出。工具 envelope 不另设层数、候选数或累计 replay work 门槛，
媒体也不增加单图容量拒绝。

## 常驻内存预算

两个 LRU 缓存决定解析结果的保留量。单用户机器上的默认值如下，启动时由
环境变量覆盖（`--check-config` 回显）：

| 缓存 | 默认 | 环境变量 | 记账口径 |
| --- | ---: | --- | --- |
| 已解析文件 / 视图 LRU 条数 | 16 | `SESSIONDOCK_CACHE_ENTRIES`（0 = 不保留） | 条数；AST 缓存取其一半 |
| 视图 LRU 字节 | 128 MiB | `SESSIONDOCK_VIEW_CACHE_MB`（0 = 不保留） | 视图的序列化消息字节 + 内嵌图片的 base64 驻留字节；序列化字节本身自 2026-09-15 起真的驻留（视图字节缓存），同一笔账同时约束 `Value` 树和字节 |
| 解码 AST 缓存 | 64 MiB | `SESSIONDOCK_AST_CACHE_MB`（0 = 不保留，追加全量重解码） | `serde_json::Value` 树的估重 |
| 搜索折叠副本 | 128 MiB | `SESSIONDOCK_SEARCH_FOLD_BYTES` | 折叠正文字节 + 每条 256 B（[搜索](#搜索)） |

实测（真实根，2026-09-13，[performance.md](performance.md#常驻内存)）：
一个视图的常驻 ≈ 记账字节的 1.2–1.6 倍（文本会话）。2026-09-13 时截图密集的 Codex
会话还 ≈ 文件大小的 1.6 倍（≤ 2 MiB 的内嵌图片以 base64 驻留在视图里）；2026-09-15
起 64 KiB 以上的字符串一律是 span，图片不再驻留（见上文"大文件的读取代价"）。
视图字节缓存（2026-09-15）之后序列化字节也驻留：同一个视图估为记账的 ≈ 2.2–2.6 倍，
预算不变，所以同一预算下最坏多 128 MiB 常驻、保留的视图数不变；不为字节另设预算，
是因为 Σ 字节 ≤ Σ 记账本来就被同一上限约束，而把字节按 2× 记账会让 404 MB + 52 MB +
p90 三个会话无法同时留在 128 MiB 里，热切换时重新解析 5 s 远比多 100 MB 常驻贵。
AST 缓存只对小于其预算的文件生效：它让活跃会话的每次追加免于重解码整文件；
图片改为 span 后 GB 级会话的 AST 也落在预算内。

除了缓存上限，进程在每次新解析、视图淘汰和每次搜索结束后调用 `malloc_trim(0)`
（遍历全部 arena 归还空闲页；打开视图的历史响应曾经也 trim，因为它克隆了全部消息，
字节缓存之后热读不再产生投影临时对象，这一处 trim 撤掉了）：
在 256 核机器上 tokio 曾开 256 个 worker（`SESSIONDOCK_ASYNC_WORKERS` 现默认
`clamp(核数/8, 4, 16)`），每个线程一个 glibc arena，释放的几百 MB 永远留在 RSS 里。
`mallopt(M_ARENA_MAX, 2)` 实测被否决：并行索引读取和读 worker 争抢两个 arena，
十次 `/proc` 扫描的 futex 调用从 2.6k 涨到 564k，CPU 反而更高。搜索的瞬时投影
不进 LRU、不记 AST，扫完即释放并 trim，RSS 不净增；只有折叠副本按上表预算常驻。

索引容量与视图序列化字节继续记账，但不再以固定 1 GiB 门槛拒绝读取。

## 明确不做

- 不做落盘的**列表**索引缓存：没有启动解析，也就没有需要缓存的东西。搜索
  文本缓存是按会话、按文件版本的正文快照，不是列表状态，丢了只会多解析一次。
- 不做跨文件的"一致性快照"：没有任何消费者需要它。
- 不为搜索预建倒排/全文索引：正文缓存 + 折叠副本预筛 + 正则流式匹配已经够快，
  倒排索引才是超出范围。
