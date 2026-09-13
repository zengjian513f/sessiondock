# 后端迁移计划：Python AgentHub → Rust

> 文件名按本次任务约定保留 `MIRGRATION` 拼写。
> 基线：原仓库提交 `ee2e373c134f4f16a3452ba35fa5346ea441fb9f`。
> 第一阶段继续使用 legacy HTML/CSS/JavaScript；Vue/TypeScript 重构是第二阶段。
> 本文同时记录实施状态；未勾选项不表示已经兼容。Rust 服务已作为独立用户级服务部署在本机私有前缀下与 Python 并行运行（2026-09-12）；切换生产流量仍是 M8 的单独授权步骤。

## 1. 目标与边界

- 在独立 `sessiondock` 仓库实现后端，不让服务依赖 Python 进程。
- 先保持浏览器 HTTP JSON、SSE、NDJSON、WebSocket 的既有行为；不把 CLI 原始格式或 Rust 内部类型直接暴露成新协议。
- `legacy-web/` 是第一阶段可修改、可服务的前端；`reference/legacy-web/` 继续冻结。必要的兼容/能力提示小改记入迁移记录，不重构组件、不引入框架。
- 已有 `web/` Vue 骨架保留但暂不作为默认前端、不继续扩展。
- ptyhost 是独立会话进程，Web 重启不能结束 CLI；不合并进 Web 进程。
- 真实 CLI 读根只经显式配置接入且只读；不自动发现主目录，不连接旧 Hub，不共用 Python 服务的 host/队列/元数据目录。测试只用人工合成的记录和隔离目录。
- 现状：Rust 服务已部署为本机用户级 systemd 服务（私有前缀、loopback、经既有反向代理，与 Python 服务并行；部署路径不入库）。停 Python、切反向代理等生产切流仍需单独验收与用户授权（M8，[docs/replacement-checklist.md](docs/replacement-checklist.md)）；不 push、不建远端。
- 不能把未实现能力伪装成成功。只读阶段写操作返回明确的 `501` JSON；未知路由 `404`，输入错误 `400`，权限失败 `403`。
- 完成度目标：整体与现有 Python 后端持平或略高即可，不要 go too far。当前目标是尽快替代已有 Python 后端；超出 Python 现有行为的扩展只在安全/隔离必需时做，不做额外的功能铺开。
- 性能红线（用户 2026-09-12）：任何用户可感知的操作超过 10 秒就是错误的设计，推翻重来而不是修补；本次重写的初衷就是性能。读模型设计以 [docs/read-model.md](docs/read-model.md) 为准。
- 文档纪律（用户 2026-09-12）：主要文档只写当前正确的设计；被推翻的设计单独放到 `docs/superseded/`，主文档只留一条指向它的链接，不与正确设计混写。
- 委派并行度：grok-4.6 headless 委派（见 [docs/delegation.md](docs/delegation.md)）按 8–12 路并行推进，每路只做一个自包含、可机械验证、不碰共享文件的任务；再高的并行度会和 `target/` 锁、浏览器套件互相抢占，不再提速。

## 2. 全后端代码盘点

原 Python 后端共 31 个 `.py` 文件（含包入口与 host 子包），约 14,045 行。
以下是职责分解，不按原文件一对一翻译。详细路由账本见后文。

| 原模块 | 职责 / 依赖 | Rust 落点和迁移风险 |
| --- | --- | --- |
| `server.py`（2555 行） | HTTP、静态资源、授权、SSE/WS、附件、发送协调、启动配置 | 拆成 config / api / static assets / application services；不能原样搬一个大 handler |
| `adapters.py`（1995） | Claude/Codex/Grok 元数据、消息、工具、状态、分叉/子代理 | `sessions/providers`；原生记录的语义最复杂，先有 fixture 差分测试 |
| `index.py`（1108） | inventory、局部重建、快照、窗口缓存、游标、搜索 | `sessions` + 后续索引/搜索模块；物理字节与逻辑历史不能混淆 |
| `live.py`（452） | 进程与会话关联、进程启动时间、活跃状态 | 独立进程探测适配层；Linux `/proc` 不能当三平台实现 |
| `session_meta.py`（355） | 星标、分叉显示、活动覆盖、回滚时间线 | 独立元数据 store，与原生记录分开；先只读再写入 |
| `pending.py`（80）、`create_requests.py`（54） | 新建未落盘会话、请求去重与 canonical 关联 | 会话生命周期服务；防重复创建，rename 后保留临时 UID 映射 |
| `send_queue.py`（408） | Codex 发送账本和原生确认 | 单独 Codex 状态机；进入不确定注入边界后禁止盲重试 |
| `claude_queue.py`（516） | Claude 队列、注入、确认、完成/中断 | 单独 Claude 状态机；不能强行合并为同一 retry queue |
| `send_protocol.py`（159）、`send_audit.py`（70） | provider driver、账本快照、审计关联 | 共享接口，不共享不兼容的业务状态 |
| `claude_bridge.py`（316）、`codex_bridge.py`（288） | hook/原生终端题卡与审批、键盘回答 | provider 专属 bridge；未知屏幕不推断已完成 |
| `term.py`（512） | 后端选择、命名、CLI 启动、输入、路径补全 | terminal application service；外部命令参数数组、明确权限 |
| `term_host.py`（297）、`host/client.py`（247）、`host/protocol.py`（66） | ptyhost 控制和 attach 客户端 | 独立 `ptyhost-client` crate；显式目录、token、超时、帧长度上限 |
| `host/procs.py`（193）、`term_tmux.py`（478） | 跨平台进程辅助、tmux 兼容 | ptyhost 优先；tmux 作为可选兼容层，不能阻塞三平台主线 |
| `term_ownership.py`（91）、`wsock.py`（95） | 单控制者 lease、WS framing | ownership registry + Axum WS；lease 与浏览器连接分离 |
| `hub.py`（1014）、`federation.py`（113） | 节点注册、凭据、缓存、聚合、代理、UID 命名空间 | 最后单独迁移；local 前端兼容不等于 node protocol 兼容 |
| `files.py`（248）、`file_manager.py`（655） | 会话引用、目录、预览、范围下载、文件作业和回收 | file service；路径授权、符号链接、原子不覆盖与跨平台语义先明确 |
| `media.py`（215） | 图片注册、内容存储/token、媒体读取 | media store；不能信任客户端提交的本地绝对路径 |
| `trash.py`（336） | 会话软删除、恢复、清理、依赖文件 | 删除保护、恢复冲突和部分成功契约；不修改 CLI 官方历史索引 |
| `audit.py`（452） | 有界异步审计、内容/blob、保留策略 | 结构化元数据优先，内容默认关闭；队列拒收前不能先做大对象复制 |
| `bug_report.py`（451）、`debug_runs.py`（222） | 报告及 CLI worker、测试会话标签/筛选 | 后置能力；该功能本身会启动一个调查 CLI，是否实现由用户决定（见 下一批的具体入口） |
| `__init__.py`、`host/__init__.py` | 包入口 | Cargo workspace / module 边界 |

## 3. 必须保持的契约

### 3.1 浏览器与静态资源

- HTML 的 `__AGENTHUB_MODE__`、`__AGENTHUB_HOSTNAME__`、`__AGENTHUB_ASSET_VERSION__` 必须替换；模式保持 `local`。
- 资源 build 标识与实际服务的文件同源；HTML 不缓存，资源可重验证。旧页发送版本闸门后续迁移时不可漏掉。
- `/api/meta`、`/api/sessions`、`/api/live`、`/api/term/list` 是旧前端启动链路；详情后会建立 `/api/watch`，审计会 POST `/api/audit/browser`。local 模式不会自动请求 `/api/nodes`。
- 控制台按钮始终可见且可点击；不可用时灰色、悬停/聚焦解释、点击给具体原因。不能用隐藏或 disabled 规避缺功能。
- 初期加明确只读/开发阶段提示。不要显示空 live/outbox 就声称实现进程探测或发送账本。

### 3.2 会话、消息与游标

- 列表 `{sessions,sig,built_at}`；相同快照 `{unchanged:true,sig}`。数据与签名必须来自同一次已发布快照。
- 保留 `uid/source/sid/title/cwd/created/updated/size/model/branch`，拓扑 `agent_items/forked_from_id/root_sid` 等有能力才给出可靠值。
- UID 的本地基线是 `source:sha1(path)[:16]`；Hub 另做 node 命名空间，不能随意混用 UUID。
- 详情包含 `meta/version{size,mtime,head}/reset/start/end/anchor/messages/message_total/partial/activity_changed/activity`。
- `start/end` 是 UTF-8 文件字节偏移，不是消息下标。未结束 JSONL 行不能被消费；文件截断、替换、同尺寸改写、语义分叉失配必须 reset。
- Claude 的有效叶子、父子链、compaction/rewind，Codex 的 rollback/fork/子代理与消息去重需要语义解析；不能只取文件最后 N 行当历史。
- `status` 记录形成 activity，不直接塞进正文；`counted:false` 不计消息数。`prompt` 缺失意为不更新，`null` 意为清除。
- 首屏窗口是完整语义历史上的窗口，不是未经解析的文件尾；第十四批Rust能力分支在100head/500tail范围内按媒体/JSON预算缩小，并用独立cursor逐页补齐gap。
- SSE 增量的 `start` 必须匹配接收者缓存 `end`；共享读取可以，不能向不同游标无脑广播同一 diff。

### 3.3 发送、终端与写操作

- HTTP 200/终端 write 成功不是 CLI 原生接收确认；注入之前持久化不确定边界，崩溃恢复不得重复提交。
- Outbox 有 epoch/revision，不能接受上个服务进程的滞留快照；浏览器断开仍要观察原生确认。
- request_id 去重、Enter 前后的崩溃点、draft 冲突、停止与 rollback 都需要事件序列测试。
- 本地 ptyhost：token 鉴权的 JSON 行控制协议，attach 后 `kind:u8 + length:u32 BE + payload`。
- 浏览器终端：标准 WS（二进制输出/输入、JSON resize/revoked、close code 4001），不暴露 host token 或本地 framing。
- 多页 claim/force/bind/release 必须原子，旧连接清理不能释放新 lease。
- 只查不到 PID 不代表进程已死。现有 host Rust client 的 Unix `/proc` 判活在 macOS 有缺口，新客户端禁止据此删除记录。

## 4. 路由迁移账本

状态以第 6 节和实际测试为准，不能依据路由存在就算功能完成。

| 路由组 | 接口 | 阶段 |
| --- | --- | --- |
| 基础 | GET `/api/health`, `/api/meta`, `/api/nodes`，静态页面 | M0 |
| 只读 | GET `/api/sessions`, `/api/messages/{uid}`, `/api/messages/{uid}/page`, `/api/session/input-history` | M1 / M2 |
| 同步/检索 | GET `/api/watch`, `/api/search`（含 `progress=1` NDJSON） | M2 |
| 状态 | GET `/api/live`, `/api/term/list`, `/api/session/outbox` | M1 仅声明不可用；M3–M5 实现 |
| 偏好/时间线 | POST `/api/session/star`, `/api/sessions/fork-visibility`, `/api/session/rewind` | M3 / M5 |
| 创建/控制 | POST `/api/term/create`, `/api/term/takeover`, `/api/term/kill`, `/api/term/backend`, `/api/session/stop`；GET `/api/term/new-status`, `/api/term/complete-dir` | M4 |
| 终端传输 | POST `/api/term/claim`, `/api/term/send`, `/api/term/scroll`；WS `/api/term/attach` | M4 |
| 发送 | POST `/api/session/send`, `/api/session/draft-status`, `/api/session/outbox/retry`, `/api/session/outbox/discard` | M5 |
| 文件/媒体 | POST `/api/session/resolve-files`, `/api/session/attachment`, `/api/session/files/action`, `/api/session/files/upload`；GET `/api/session/file`, `/api/session/files`, `/api/media/{token}` | M6 |
| 回收站 | DELETE `/api/session/{uid}`；POST `/api/sessions/delete`, `/api/trash/restore`, `/api/trash/purge`；GET `/api/trash` | M6 |
| 诊断 | POST `/api/audit/browser`, `/api/bug-report`；`debug_run` 筛选 | M0 能力禁用声明；M7 完整实现 |
| Hub | node registry、展示属性、节点转发、SSE/NDJSON/WS 代理、鉴权头/UID 转换 | M8 |

## 5. 性能评审：代码证据与待测项

这里是静态分析，不是生产基准结论；不能承诺换语言后的倍数。

1. `ThreadingHTTPServer` 每条长连接占用线程；`server._watch` 每 50 ms 每观察者 stat，且最多每 150 ms 探测终端。Tokio 能降低连接成本，但必须共享每个逻辑会话的读取/探测，限制订阅者、慢客户端和队列。
2. `index` 已做 0.5 s inventory TTL、按 owner 重建、cursor 缓存、大会话窗口缓存；这些优化不能迁移时倒退为每次列表全读 JSONL。
3. 完整语义历史构造、JSON 序列化/压缩、大对象复制可能比路由更耗 CPU/内存。阻塞文件读取和解析放 bounded worker，不阻塞 Tokio reactor；先做已解析版本缓存，再考虑磁盘索引/SQLite。
4. 搜索遍历语义消息，Python 回溯正则存在失控风险。Rust 搜索不能无声改变 lookaround/backreference/全词语义；在兼容性尚未决定时明确拒绝不支持选项。
5. 审计部分路径复制/序列化大正文、SSE 包与终端字节，即使后台写盘也可能阻塞生产者。先采样/大小预算/容量检查，测试饱和时请求延迟与丢弃计数。
6. Hub 聚合有节点超时、离线缓存与有界 NDJSON 通道；迁移为异步仍需每节点并发限制、取消传播和错误隔离。
7. `/proc`、`psutil`、tmux、`renameat2`、只认 `/` 的路径判断是跨平台瓶颈，不是 Python→Rust 自动解决的问题。

额外审计发现（源代码证据，未做生产压测）：

- ptyhost `session.rs` 的输出线程顺序执行各 client 的阻塞 `write_all`，慢消费者可能反压 PTY；Web 换成 Rust 不能消除它。attach 在 ack/replay 前加入 live clients，存在发送顺序竞态风险，需专门复现后修改 host。
- Python 两种发送账本频繁整文件读写；Codex 全局发送锁覆盖屏幕 capture、草稿确认和注入，会跨会话串行。迁移应按会话合并观察/per-session 排序，不降低 durable 边界。
- Codex 确认即删除 receipt，晚到重复 request ID 的防重弱于 Claude tombstone；是否增强为持久去重需列作明确行为改进。
- Hub 缓存条目上限不是字节预算；聚合 JSON、深复制、签名和重新压缩会放大峰值内存。未认证旧 Hub 不应通过 `/api/meta` 的 protocol 握手。
- 文件管理器是受信操作者可导航的文件系统，不是原有 cwd 沙箱。迁移时需独立决定权限模型；不要把开发会话输入根限制误当现有文件管理 API 的全部兼容语义。
- `bug_report` 会读取诊断内容并启动调查 CLI，且旧提示词包含原仓库发布流程，不能直接照搬到新仓库。
- 旧 `docs/multi-node.md` 部分缓存/超时描述落后于代码；实现基线以代码和测试为准。

异步任务仍需显式并发控制：Tokio 的 blocking 工作已经开始后不会因 HTTP
取消而自动停止，因此当前 permit 持有到 worker 真正结束，而不是请求取消即释放。
相关接口语义见 [Tokio spawn_blocking](https://docs.rs/tokio/1.53.1/tokio/task/fn.spawn_blocking.html)。

待建立基准：空闲连接数/内存、1/10/50 个同会话 watch、冷/热列表、1/10/100 MiB 合成记录首屏与 append、1000 会话扫描、取消搜索、超慢客户端、审计满队列。记录 p50/p95/p99、CPU/RSS、读取字节数与重复解析次数；优先对比相同正确性 fixture。

## 6. 实施步骤与完成标准

### M0：运行边界与 legacy 接线（第一批）

- [x] 全模块审计和迁移计划初稿。
- [x] 工作前端 `legacy-web/`，默认 Rust 静态服务、模板替换、build id；Vue 冻结到第二阶段。
- [x] 显式输入目录、强制 loopback、URI/声明 body 大小和读取并发限制、JSON 错误与 capabilities；不是生产认证/完整 DoS 防护。
- [x] 未实现路由明确失败；小改 legacy 禁止持续发送不支持的诊断、解释只读状态。
- [x] HTTP 与真实 Chromium 启动/错误/窄屏检查。

### M1：只读会话纵向链路（第一、二批持续推进）

- [x] 明确配置的 Claude/Codex/Grok 数据源；无配置默认空，不探测用户主目录。
- [x] **受限子集**的原生格式解析、列表、详情、输入历史，明确已支持/未支持语义。
- [x] UTF-8 字节游标、半行/坏行、截断/替换/改写、稳定 snapshot/sig。
- [x] 人工 fixture 驱动的 parser/API/browser 检查；不以模拟 JSON API 当真实解析实现。
- [x] 第二批：Claude 原生活动祖先链/last-prompt/两种 compact/未回答中断输入；本地命令/通知等只读事件。
- [x] 第二批：Codex 固定父前缀、多级 fork；Claude/Codex 子代理归属和原有 agent 详情接口。
- [x] 第二批：语义 anchor、父依赖改写/丢失/重复 SID、跨视图游标和多 SSE reset 回归。
- [x] 高级合成差分（第二十一批 `tests/advanced_parity.py`）：Claude 三级 compact/rewind/sidechain/事件、Codex 三级 fork/rollback/子代理、三家工具/问答/错误/超大输出、Grok 信封与 summary-only；全部差异已明确归类。
- [x] 第三十三批：未知的记录/attachment/事件/内容块类型与 Python 同样跳过，计入非致命 `migration_warnings`（每种一条带计数，≤32 种+溢出行），`supported` 保持 true；坏 JSON/缺失祖先/标量 content/坏图片/预算等硬失败不变。当前 Claude Code 2.1.269 与 Codex rollout 的新类型已进合成语料。
- [x] 第三十四批：读模型重做为惰性索引 + 按需视图；真实读根（792 行、3.4 GB）冷列表 0.308 s、热 6 ms、打开 ≤ 0.3 s、RSS 103/319 MB，全部目标达成；没有会话数/总字节/目录条目上限。
- [x] 第三十五批：真实读根 803 行全部支持，sid 集合与 Python 一致；消息影子比对仅余 Codex 多块信封一类差异（见第三十五批与下一批入口）。

### M2：增量与搜索

- [x] 每逻辑视图共享版本读取，SSE 有界订阅、断开回收、心跳、重连游标。
- [x] 全文与 NDJSON 进度/分批结果、取消、错误；正则语义选型有明确兼容表。
- [x] 完整前缀核验的有界JSON AST追加复用；全量时间线重算、冷解析对照和新旧release追加测量。
- [x] 独立事件页cursor、有限gap读取和legacy逐页插入；保留live字节cursor，完整分支授权与展示页分离。不是native span或无限大历史支持。
- [x] 第三十四批：大历史按需视图与有界 LRU（64 项 / 2 GiB），列表与打开分锁（列表从不等待正在解析的会话），SSE 每会话 stat 轮询 + 增量续读；228 MB + 90 MB 继承前缀的真实 Codex 链打开 3.1 s、热 24 ms。
- [ ] 乱序/重复/reset 并发测试与慢读者隔离的负载证据。观测（2026-09-12，`tests/concurrency_probe.py` 8 线程 3 秒，10 会话×300 条）：sessions/messages/search 各约 2200 请求，其中约 74%/26%/77% 命中 `reader_busy`/`search_busy` 503 准入拒绝，无 5xx；有界准入按设计工作，但并发浏览器体验取决于 legacy 的重试策略，M2 需重新评估 worker 数与排队。

### M3：持久元数据与进程关联

- [x] 独立开发 state dir、原子写入与 schema 版本；星标/显示偏好。
- [x] 时间线 pin 持久化并接入读模型（第二十七批）：`POST /api/session/rewind` 只改显示（parser 选项等价），原生 lineage 信号出现即带原因退役；不写原生文件、不向 CLI 发信号（真正的原生回滚仍未实现）。
- [ ] 活动覆盖（activity stop）接受原生操作确认后接入读模型；domain 存储已具备，不提前启用。
- [x] Linux 进程身份（pid + `/proc` start ticks，仅限已验证 host 记录命名的子/宿主进程）与 UID ↔ host 实例精确关联（第二十二批）；非 Linux 明确 `platform_unsupported`。
- [ ] Windows/macOS 进程身份实机实现与验证。
- [x] 显式受控host的full SID/UID精确只读关联、instance缺失/重复/失联原因；不猜测外部CLI。
- [x] 未知/退出/运行三态明确（第二十二批）：`running` 需 host Info + 身份核验，`exited` 需 host 退出/生命周期回执/本进程内已验证身份消失，其余为带原因的 `unknown`；空列表不表示全部停止，`live` 能力仍为 false。

### M4：ptyhost 与终端

- [x] 独立异步 host client；fake peers验证framing/token/超时/EOF/大帧/Unix/TCP，现已接HTTP和Linux隔离host。
- [x] ownership 原子状态机与 20 项纯状态机测试。
- [x] WS 桥接、backpressure；浏览器断开只关 attach（Linux隔离host验证）。
- [x] 显式 allowlist 创建、持久幂等回执、pending 控制台与精确取消；Linux 免费 shell 端到端验证。
- [x] 显式操作者确认的一次性原生主会话绑定、衍生native租约退休；不是自动CLI归属证明。
- [x] 真实 CLI 创建/恢复参数契约（第二十四批）：launcher schema 2 的 per-source profile（可执行文件、固定前缀、`new_args`/`resume_args` 整参数占位符、env 白名单/黑名单、profile cwd 根）、Claude 新建 `--session-id` 身份信封、Codex/Grok 新建保持 pending、`resume_uid`/takeover 经冻结库存解析 SID、`complete-dir`、`backend`；只以假 CLI 脚本验证，未运行真实模型 CLI。
- [x] 受管实例停止（第二十九批）：`POST /api/session/stop` 只对冻结库存 + 新鲜 guarded 运行时观察解析出的受管实例生效，Ctrl-D×2（各 1.2 s，同 Python `graceful_stop`）→ 既有受保护停止（受管回执走 `term/kill` 持久取消路径，宿主自行 HUP），报告 `graceful|stopped|already_exited|uncertain`；未受管会话 501 `session_stop_unmanaged`，从不静默成功，Web 进程不向 PID 发信号。
- [ ] 外部（非受管）实例接管/原生停止/重命名：外部实例 `session/stop`、`takeover force`、rename 仍 501，依赖外部进程探测决策。
- [x] Web 重启后 PTY 仍存活，隔离免费shell的PID/输出复验。
- [ ] Windows/macOS 实机验证后再标跨平台完成。

### M5：可靠发送与交互

- [x] Codex独立发送纯状态机/持久确认门控/20项测试；尚无可用CLI执行与可靠关联适配器。
- [x] Claude独立纯状态机21项测试：本地FIFO与native queue分离、持久确认门控、迟到强ack/恢复和turn隔离。
- [x] Claude 接持久 store/执行适配（第三十一批）：执行器 + 宿主终端驱动 + 原生确认适配 + 四条 HTTP 路由 + legacy composer，假 CLI 端到端验收；Codex 执行器接线待第三十二批。
- [x] Codex 接执行器（第三十二批）：同一执行器按 `Provider` 分派，复用第三十一批驱动/路由与第三十批适配器；Codex composer 识别（含粒子字形）、两步持久化、TUI 自持排队不等空闲、边界后带 `turn_id` 的 user 记录以执行器自身 Enter 的 `OperationTurn` 确认；假 CLI 端到端 + 浏览器 + 真实 CLI 套件（`gpt-5.6-luna` low，实跑通过）。
- [x] Codex 原生确认适配器（库，第三十批）：固定边界后的记录分类为 `PossibleTextMatch`/`Absent`/`Uncertain` 证据，状态机拒绝弱证据；未接执行器，不确认任何回执。
- [x] 独立typed持久store、恢复门控Engine、有界异步只读service及精确native scope的outbox GET；不包含CLI执行或强确认适配。
- [x] 请求去重、崩溃恢复、原生确认、固定确认游标复核与限频（第三十一批，Claude）：request_id 重放=状态查询、粘贴/Enter 两步持久化、崩溃→Uncertain 不重注、8 s 复核固定边界并限频、一小时跟踪窗。
- [ ] 输入草稿、审批/问答、Esc/rewind、native 回滚和回执退役。
- [ ] 真实 CLI 集成验证（以真实 claude/codex/grok 作为被测 CLI 的自动化测试）：模型规则见 AGENTS.md。已有：`tests/send_claude_real.py`、`tests/send_codex_real.py` 在常规扫描中通过。

### M6：文件、附件、回收站

- [x] 显式授权根+完整语义分支的只读库、隔离测试、legacy文件浏览/下载实际浏览器接线。
- [x] Session/agent scope 内的可信路径解析测试（第二十八批写侧覆盖：根内 symlink 指向外部 403、`..`/绝对名 400、resolve 与 write 之间组件换成 symlink 409 `file_changed`、Unix 拒绝反斜杠/盘符、作业中根被替换 409）；Windows 实机仍未验证。
- [x] 授权文件读取的Range 206/416、PDF/媒体 CSP、大小限制、断开取消、不把二进制转 JSON；原生媒体另见下项。
- [x] PNG/JPEG原生内嵌图片/工具媒体→token HTTP→legacy；第十一批已隔离验证预算、窗口/SSE、分支与缓存回收。
- [x] GIF/WebP/APNG/静态AVIF/BMP及授权磁盘图片；第十二批完成格式预算、完整分支授权、版本化token和实际legacy浏览器验证。路径不能从JSONL/cwd直接取得读取权。
- [x] 图片descriptor登记/GET按需物化、独立encoded-source/blob预算、文件首次GET前版本绑定及冷/热重授权；legacy加载错误和显式重试，不提升大原生记录上限。
- [x] 常用媒体的真实Python adapter/media合成差分；第十三批修正消息拆分/计数/占位，明确安全与新增能力差异。
- [x] 结构化原生大图（第十八批）与嵌套字符串Codex工具信封回放（第十九批）：32MiB单图、当前分支冷/热授权、共享回放预算；外链保持独立策略，不自动请求。
- [x] 单消息超过16图的media continuation（第二十批）：单消息上限256张、内联16张、`media_more`/`media-page` grant 逐批续取。
- [x] 跨多字符串拼接的巨型工具 JSON（第三十六批 G：多块信封按序拼接，巨型块仍走私有区段）。
- [ ] 256 张以上单消息和未支持编码子类型；设计不等于实现。
- [x] 文件作业（第二十八批）：显式 `SESSIONDOCK_FILE_WRITE_ROOTS`（须位于读根内且与私有路径不相交）、job 绑定 scope、分块 offset 幂等重放、`linkat`/`O_EXCL` 原子不覆盖、删除进根内回收目录、过期/并发/字节限额；copy/compress/extract/bundle/restore/purge/retry 与缩略图仍 501。
- [x] 回收站（第二十六批）：显式 `SESSIONDOCK_TRASH_DIR`、冻结库存派生文件集与 fork-parent 保护集合、新鲜三态判活（running 拒绝、unknown 需 force）、rename 前 stamp 复验与回滚、批量部分成功 200、恢复不覆盖、purge 不碰原生目录。

### M7：诊断与运维

- [x] 浏览器审计接入（第二十三批）：显式 `SESSIONDOCK_AUDIT_DIR`、准入先于拷贝、字节+条数双限队列、脱敏、轮转/保留上限、`/api/health` 计数、`Gate` 故障注入测试。
- [ ] 服务端请求/响应追踪、结构化日志与 bug report 存储策略。
- [ ] debug run 隔离、bug report worker 的安全边界和显式启动权限。
- [ ] 配置校验、优雅退出、结构化日志、可复现发布包及 CI 三平台矩阵。

### M8：Hub 与生产迁移

- [ ] 单独决定旧 Python Hub 兼容适配层；按旧协议测试，不靠 `/api/meta` 自称兼容。
- [ ] 节点身份/凭据、真实 peer 授权、注册表管理仅服务端、超时/离线缓存/代理取消。
- [ ] HTTP、SSE、WS、媒体/文件引用命名空间完整合约测试。
- [x] 生产数据只读影子比对（第三十五批，`tests/shadow_compare.py` 三来源 sid 一致、消息差异已归类）与隔离切流/回退演练（`tests/cutover_drill.py`）已做；用户授权切流仍未执行，队列和会话先 quiesce/明确所有者，不双写。
  - 工具已备：`tests/shadow_compare.py`（grok-4.6 headless 产出、人工审阅，见 `docs/replacement-checklist.md` §3）；只在操作者显式给出真实历史目录并加 `--i-understand-this-reads-real-histories` 时运行，仓库测试不运行。真实数据比对本身仍未做。
- [x] 回退流程见 `docs/replacement-checklist.md` §5，`tests/cutover_drill.py` 证明 Web 回滚不结束 ptyhost 实例、不碰 Python 数据目录（第三十五批）。

### 第二阶段：前端框架迁移

仅在后端契约和业务回归稳定后继续 `web/` Vue 3/TypeScript。按会话、终端、文件等边界替换；第一阶段不借迁移后端之名重写 legacy UI。

## 7. 执行与验收记录

- 开始状态：仅健康检查与 Vue 骨架，ptyhost 源码导入但未接线；原 Python 仓库干净。
- 子任务按独立目录划分，主 agent 负责配置/API/静态服务与集成，不允许并行改同一 manifest/入口。

### 第一批已交付

- `config.rs`：只读输入根、loopback 强制校验，不自动发现真实数据。
- `assets.rs`：有界静态资源快照、模板注入、能力也计入 build、缓存重验证。
- `security.rs`：拒绝非 loopback Host、跨站 API、旧 Hub 协议/认证头；所有 API 非空 `debug_run` 明确 501，不能伪造隔离。
- `api/`：启动接口、列表/详情/输入历史、基础 SSE；32 订阅/4 blocking worker；命名错误事件/取消/心跳，服务退出取消 SSE。
- `sessions/`：普通三家原生记录、稳定版本缓存、UTF-8 游标/完整前缀 anchor、原生状态分离、语义首100/末500窗口。
- `ptyhost-client`：显式目录、token 不进入公开 DTO、typed 请求/回应、取消安全读取、写超时或取消后禁止复用帧流；不查 PID、不删记录、不启动任何进程。
- `legacy-web/`：能力/存储隔离小改，未实现能力不自动请求，控制台保持可解释；读取失败显示旧快照/具体错误并暂停，显式成功重试后恢复。

### 第二批已实现的链路

- `sessions/providers/claude.rs`：先求活动祖先链，再投影正文/状态；last-prompt 切叶、compact 跨树接回、已完成废弃分支隐藏，狭义未回答 sibling 输入保留中断标记。主视图 sidechain 不决定主叶子。
- `providers.rs`：Codex 正确区分主线程 session_id 与子代理 id；保留父关系字段，compact 事件、重复遥测/内部上下文过滤、中断标记等。所有 provider 均不直接打开文件。
- `sessions/history.rs`：只在显式 inventory 内解析关系；Codex 父前缀按完整 JSONL 边界重新解析，多级继承不带入 cut 后的消息/中断语义。父链、代理归属分别处理，不把 fork 当子代理。
- Claude 子代理 sidecar 读取受相同根目录/文件预算约束；两家 agent_items、嵌套代理和 `?agent=` 详情沿用 legacy 接口。请求参数不拼成文件路径。
- `SessionStore`：逻辑视图缓存、依赖版本整组核对、SSE-only 拓扑刷新；列表元数据变化会使缓存菜单失效，避免新建/删除代理仍显示旧菜单。
- `rs-m1-2` 游标：核对原始已读前缀 + 当前已显示事件 + 固定继承历史身份；普通追加增量返回，换枝/旧消息中断状态改变/父前缀改写 reset。对外 end 始终属于所选叶文件。
- legacy 源码本批无需更改；仅把第一批错误测试中的“未实现分枝”改为真正未知的原生控制记录，继续验证错误暂停和修复重试。

### 当前明确的差异与限额

| 项目 | 当前行为 / 后续要求 |
| --- | --- |
| 会话格式 | Claude 活动树/compact、Codex 固定前缀 fork 和两家子代理已覆盖合成用例；媒体/缺失 Claude 活动祖先/无固定前缀的 Codex fork 等仍明确失败，不宣称全量兼容。第三十三批起未知的记录/attachment/事件/内容块**类型**不再判不支持，而是与 Python 同样跳过并计入非致命 `migration_warnings` |
| 真正执行回滚 | 第二十七批：显示 pin 已持久化并接入读模型（`POST /api/session/rewind`，原生信号出现即退役并报原因）；仍不能观测 CLI 内存中尚未落日志的 rewind，也不向 CLI 发起原生回滚 |
| 关系异常 | 缺父/循环/错切点等详情 501，歧义 ID 409，未知或不属于该主会话的 agent 404，超预算 413；孤儿/循环代理保留为 unsupported 顶层项（Python 原实现会隐藏） |
| 列表与详情 | 列表只做头/尾摘要 + 轻量拓扑/切点检查，不解析正文；继承前缀 grammar 在详情严格验证。列表行 `cursor` 只带物理 `{end, head}`，语义 `anchor` 在该会话被打开且视图仍是当前文件版本时才出现；谱系错误与内容块警告在打开时报告 |
| 原生名称 | Codex `session_index.jsonl` rename 已由第六批名称索引覆盖；Grok summary-only 会话已列出（正文空、`end 0`，与 Python 一致） |
| 展示 | 工具摘要、Write/Edit/MultiEdit/apply_patch 文件 changes、问答和 Codex 多段执行结果已做合成差分；差异生成超预算明确提示且保留原始参数。媒体/未知内容块仍不完整 |
| 数据量 | 第三十四批起没有会话数/总字节/目录条目上限，列表不解析任何文件；只剩防病态文件的按会话预算（打开时 413/501）：单条记录 64 MiB、单文件 4 GiB、每文件 2,000,000 LF、每视图 1,000,000 记录 / 2,000,000 事件 / 1 GiB 序列化消息、父链 32 层 / 4 GiB 前缀、摘要 256 / 16 MiB；视图缓存 64 项 / 2 GiB 序列化消息、AST 缓存 1 GiB（逻辑权重，不是 RSS 上限；真实根打开 20 个后 RSS 319 MB） |
| 刷新成本 | 第三十四批：列表刷新 = 目录遍历 + `stat` + 只对 stamp 变化的文件重读头/尾（真实根 792 行冷 0.3 s、热 6 ms）；打开的视图按追加续读、按 stamp 失效，完整 checkpoint 与原始前缀 digest 缓存；早期 checkpoint 仍需核对当前叶投影 |
| SSE | 每逻辑 `(uid,agent)` 一个500ms publisher；最多32条浏览器订阅/32个视图、2个后台准入、共享4个blocking worker。每客户端独立游标，Tokio watch只保留最新不可变快照，不积压旧版本；最后订阅断开回收 |
| 搜索 | 按需流式投影候选文件的完整语义视图（缓存视图直接复用，未命中投影后即丢弃），不搜索原始JSONL/工具参数。2任务准入、8包NDJSON队列、8MiB结果预算、最多200条结果/每会话200命中；不可读视图返回errors/partial/incomplete。Rust regex方言的lookaround/backreference明确400。**产品决定（2026-09-12）：正则搜索为 P3，保持现状，不再为复制 Python 回溯正则语义投入** |
| 游标兼容 | 字段仍是 legacy byte cursor，当前schema为 `rs-m2-1`；anchor绑定叶语义投影和固定父前缀身份。有意让旧Rust/Python游标reset，浏览器命名空间继续隔离 |
| 路径安全 | roots 规范化固定、每次检查祖先/symlink/根内路径、读句柄版本前后核对；已阻止长期父目录替换，不宣称目录句柄链级无竞态沙箱 |
| 运行状态 | `live.known:false`；不根据无法探测的状态推断会话已停止。终端CLI入口enabled:false；显式host目录可开独立claim/WS传输，但尚未UID关联 |
| 写入/队列/Hub | 原生记录只读；显式state目录可保存偏好。队列/原生控制/Hub未实现返回501，不出具空outbox假确认；`meta.protocol:0`阻止旧Hub握手 |
| 平台 | 当前仅 Linux 运行验证；Unix/TCP fake-peer 测试不能代替 Windows/macOS 实机 |

### 第一批验证证据（历史记录）

- 本轮最终结果：`cargo test --workspace --locked` **84 项通过**（含1个 doctest；原 ptyhost 性能 benchmark 1项保持 ignored），新增 server/client 的 Clippy `-D warnings` 通过；workspace release 构建通过。
- legacy Node 契约测试 **24 项通过**；保留的 Vue 健康契约 **6 项通过**且类型检查/构建通过。真实 Chromium 场景和3个人工 provider 差分均通过。
- 原 Python 仓库仍干净；原 ptyhost 源码与冻结前端 reference 未改。未连接生产、未运行付费 CLI、未配置远程/提交/push/部署，临时测试服务已停止。
- `sessions/tests.rs`：合成数据、UTF-8/毫秒、半行/0 checkpoint、截断/同尺寸中段改写、路径替换、读句柄核验、坏记录修复、重复遥测/turn/工具错误输出。
- `tests/http.rs`：实际 Axum router、静态模板/build、未知404/未实现501、权限、查询、原生读取、SSE watermark/订阅回收/退出。
- `ptyhost-client/tests`：fake Unix/TCP peers，粘包/分片/EOF/exit、错误不回显 token、取消和 idle/partial-frame deadline；另有 rustdoc 编译示例。
- `tests/legacy_contract.mjs`：无 capabilities 时保留 Python 行为、关闭后台未支持工作、pending 不被清除、旧流隔离、失败/手动重试和快照保护。
- `tests/legacy_browser.py`：复制 fixture 到临时目录启动 Rust，Chromium 验证三家列表/详情、中文 SSE 半行追加仅一次、未知原生记录错误、修复后手动重试、截断 reset、控制台灰色且可点击、窄屏/暗色、无不支持后台请求、SSE 仍打开时退出。
- `tests/provider_parity.py`：显式旧源码与本地服务，仅3个固定人工 fixture；正文角色/文字/turn/phase/call/error、最新 activity、计数/EOF 子集对齐。Claude 6条正文/5计数，Codex4/4，Grok5/5；工具摘要差异另列。**不是全量 provider parity**。

### 第二批验收记录

- 新 `tests/history_parity.py` 自建 13 份原生文件，仅导入显式旧源码 adapter；10 个可读主/子视图做 Python 差分，其余孤儿/循环作为明确安全差异。旧实现不是新后端运行依赖。
- 新 `tests/history_browser.py` 使用同一人工语料与独立 Rust 子进程，在真实 Chromium 操作 sidebar/分叉父链/子代理菜单，核对 DOM 与实际 SSE 包。
- 覆盖 last-prompt 仅追加但撤回旧显示、两种 compact、未回答 Esc 输入、两层继承/子代理、叶字节 EOF、父 cut 后追加不影响孩子、同尺寸父前缀改写 reset、跨视图游标与元数据菜单失效。
- 最终 `cargo test --workspace --locked` **124 项通过**（比第一批增加40项；server单元65、HTTP9、原ptyhost32、client17+doctest1），原ptyhost性能benchmark仍1项ignored。
- `cargo fmt -p sessiondock -p ptyhost-client --check`、两crate所有targets的Clippy `-D warnings`、workspace release和server debug构建全部通过。未格式化或修改导入的ptyhost源码。
- legacy Node **24项通过**；10视图Python高级差分、两套真实Chromium回归通过，基础三provider差分也复验通过（工具摘要差异仍另列）。Vue本批未变，未重复执行第二阶段构建。
- 本批仍仅Linux本地验证；只用临时人工语料。未读取真实CLI会话、未启动付费CLI、未提交/push/部署；原Python仓库干净，冻结前端与ptyhost源码不变。测试子进程由各自工具退出并清理。

### 第三批已实现的链路

- `ViewSnapshot` 固定完整 cursor/revision，`SessionSnapshot` 固定搜索 inventory；纯批次生成不重新读文件。父前缀 digest 缓存，fork 列表 checkpoint 与详情对齐，父 cut 后追加不改继承身份。
- `observe.rs` 合并相同逻辑视图订阅；首次读取合并、最新值发布、慢读者按各自游标追赶、最后订阅回收、失败重试的代际清理与服务取消均有测试。
- `search.rs` / `api/search.rs` 提供 JSON 和 NDJSON 搜索、原有三个选项/来源筛选、进度和心跳。未poll响应也占准入，丢弃响应会取消worker并解除阻塞发送；原生解析取消仍等待当前有界工作结束。
- 正则采用 Rust regex，有界编译、Unicode匹配；全词边界按字母/数字/下划线实现，含相邻标点与零宽回归。不支持的语法在流开始前400；Unicode大小写与 `\\w` 不承诺精确模拟Python。
- `providers/tools.rs` 补工具摘要、问答、执行信封、文件变更与有界 SequenceMatcher。差分预算为512KiB/20000行/4M匹配步骤；超限保留参数并输出 `changes_unavailable_reason`。
- legacy 只加 Rust 正则选项提示与差分超限可见说明，沿用原搜索框、结果列表、文件变更卡片和分栏视图。没有引入框架。
- `terminal/ownership.rs` 提供随机256bit lease、原子claim/force/bind/release、精确15秒未绑定过期、单调时钟及旧连接代际保护。此项只是纯状态机；WS桥接另行验收。

### 第三批阶段验证

- 当前 workspace **176 项通过**：server单元107、HTTP9、search10、ptyhost32、client17+doctest1；ptyhost性能benchmark仍1项ignored。下一批接线后的总数另记，不以历史结果替代最新验证。
- legacy Node **24项通过**；基础和高级两套真实 Chromium 回归、10视图历史Python差分再次通过。搜索独立Chromium已验证命中/跳转、选项、部分错误、不支持正则400后修复重试。
- 工具差分已验证 Claude89/Codex93/Grok89条语义消息、每家75项文件changes，包含64组确定性随机diff与重复行autojunk；三家桌面/移动、分栏和独立超限提示/展开原始参数逐字一致全部通过。
- 本地原Python仓库已由并行工作推进到 `aa941ad`（触摸拖动侧栏）；该提交不属于迁移修改。冻结reference仍保留原 `ee2e373` 基线，未自动同步新前端改动；ptyhost源码仍一致。
- Clippy发现的工具模块两处风格告警已修复；随后两crate所有targets Clippy及workspace release构建通过。SSE补测/第四批接线后的完整结果另记如下。

### 第四批已实现及验收的子链路

- `metadata/`：显式既有独立目录、schema_version/revision、4MiB/10000行预算、0600文件/私有目录、OS单writer锁。唯一temp→file sync→原子替换→Unix目录sync后才发布Arc；写后不确定状态统一503并冻结，重启核验。
- `api/metadata.rs`：沿用star/fork-visibility接口；原生UID/父关系校验、有效子集原子提交、逐项无效结果、版本闸门和请求大小上限。list/messages/search/SSE用一致偏好快照，偏好变化不重置历史anchor。
- 真正的原生活动覆盖/rewind只存在typed domain存储方法，尚未接入HTTP和读模型；不会把保存pending误作原生回滚确认。
- `terminal/service.rs` / `api/terminal.rs`：显式host目录、Info验证后claim，WS二进制/resize/revoked/4001，RAII处理升级失败和取消。按name异步IO锁串行写与force发布，已发送到host的字节不可撤回。
- 终端16连接/8操作、64KiB输入、8MiB host帧、16×32KiB输出队列、写超时/尾包排空。断开/退出只关闭attach，不杀host。CLI UID关联、创建、可靠发送能力仍未声明可用。
- `delivery/codex.rs`：20项纯测试覆盖Persist门控、完整payload去重、固定确认cursor、草稿冲突、注入不确定恢复、tombstone和精确turn完成。旧TUI没有可靠operation→turn关联适配器；仅同文匹配不会被说成确认，M5仍未完成。
- 默认不写数据，不配置目录则对应操作501；配置时meta/HTML同源声明 `metadata` 与 `terminal_transport`，能力参与build。禁止静态资源目录和私有目录重叠。
- `tests/metadata_browser.py`：两标签页star/unstar SSE、父会话显隐、窄屏可点击控制台、重启持久恢复、原生字节不变全通过。
- `tests/terminal_browser.py`：真实临时免费shell与Chromium/xterm、resize、4001接管、Web重启后同一child PID继续响应全通过；不连接生产或付费CLI。
- 新metadata测试发现writer Drop仅close会短暂受其他线程fork继承的同一文件描述影响；改为显式unlock且不删除锁文件，新增duplicate-handle确定性回归。metadata专属现15项通过。
- 本阶段完整workspace **231项通过**：server单元146、HTTP10、metadata4、search10、terminal11、ptyhost32、client17+doctest1；host benchmark仍1项ignored。两crate所有targets Clippy `-D warnings`通过。后续新模块另行复验，不用这个数字代表最终迁移完成。
- 约1/10MiB、2000消息的release合成读取基准已建立，窗口/增量正确性通过；结果和限制见 [docs/performance.md](docs/performance.md)。追加全量重解析仍是明显优化目标，不承诺Rust倍数或生产尾延迟。

### 第五批实现与阶段验证（持续推进）

- `runtime/` + `ptyhost-client::probe`：只读全ID关联、重复host双方unknown、Info确认运行/退出，记录前后身份一致性检查；忽略私有token/endpoint/arbitrary meta。既有Python新建host未传meta，不用八字符名称/cwd/PID猜测会话。
- `/api/live`额外返回`managed`部分观察，原有`enabled:false/known:false`和空全局UID集合不变；2请求准入、256host/8并发、单2秒/总5秒deadline，退出时取消探测。client新增6个fake-peer测试、runtime6个纯/异步测试与4个HTTP回归通过。
- `delivery/claude.rs`21项通过；当前CLI hook没有可靠request/injection-tag→native prompt关联。queue enqueue/dequeue不能当作接受；弱同文证据不会解除不确定边界。持久store另行实现，未开放真实发送。
- `files/`检查显式根与所选分支引用，目录能力句柄/no-follow/nonblocking、硬链接/特殊文件拒绝、身份改写中止。首轮14项库回归和Windows MSVC交叉编译通过；不是Windows/macOS实机验证。
- 文件HTTP3条读路由已接线，2准入、按消费者拉取才读取64KiB chunk，取消中的blocking读取仍保留容量。二进制不走JSON；Range/HEAD/If-Range、HTTP隔离测试继续补测。
- `tests/files_browser.py`实际点会话链接，验证Markdown安全/源码、下载内容、目录嵌套、只读控件和窄屏。先复现旧前端仍轮询jobs/请求thumbnail、错误原因被通用提示覆盖，再修正并重跑通过；未修改冻结reference。
- 此阶段workspace快照**284项通过**（server189、HTTP10、metadata4、runtime4、search10、terminal11、ptyhost32、client23+doctest1），host benchmark仍1项ignored；两crate所有targets Clippy通过。后续新测试/持久store仍须重新全量验收。
- 历史、偏好、搜索三套Chromium在首次SSE元数据对齐变更后再次全部通过；legacy Node24项通过。
- 文件库最终15项及HTTP9项通过，含无配置/越界、分支/agent隔离、HTTP大小上限、Range/HEAD/If-Range、2条未消费body占满准入、逐块改写失败、取消；无效PDF的错误页保留显式下载恢复动作，也已先复现再通过浏览器验证。

### 第六批：实例绑定与发送持久层（持续推进）

- fake peer先证明原name-only attach在成功probe后可能接到同名新实例；只向旧send加expected字段仍会被旧host忽略并执行。采用新外层`guarded_v1`操作：旧host拒绝未知op，不降级；新host在同一Session的immutable meta核验后才执行。
- 新bound client只从guard-capable、唯一native身份、明确instance构造目标；每次控制/attach校验record并发送guarded envelope，成功ACK必须携带版本和精确instance。客户端35项通过，不能将ACK缺失当成没有执行。
- host新增`guard.rs`、Info能力字段及dispatch/attach前检查，原op保持兼容。4项纯guard测试以及`tests/host_identity.py`隔离免费shell已通过：错误实例不能resize或写quit，合法ACK/回放/输入正常，同名重建后旧请求拒绝。原Web/xterm/resize/4001/Web重启测试再次通过。
- host仅`main.rs`、`session.rs`和新guard模块相对冻结导入基线有意变更；其他host文件保持原样。未改原Python仓库或原host，未部署。
- browser lease不可变UID/instance绑定和HTTP接线已验收，并接通legacy会话控制台；创建/可靠发送仍关闭，M5仍未完成。
- `delivery/store/`23项通过，两家delivery合计64项；单typed envelope、显式initialize/open、严格JSON/重复key、epoch恢复门控、单writer与持久边界。Windows目前明确DurabilityUnavailable；不是可用发送，也不是完整跨平台持久层。
- 此前完整workspace快照346项通过（server227、files9、HTTP10、metadata4、runtime4、search10、terminal11、ptyhost36、client34+doctest1）；2项ignored分别为host性能benchmark与需显式运行的免费shell runtime smoke。随后Clippy的while-let风格告警已修正并复验通过；后续新增功能的全量结果另记。
- bound终端32项单测、raw HTTP/WS11项和bound HTTP/WS5项通过。列表只发布精确唯一/运行中/guard-capable关联；错误SID/UID、重复、退出、离线和旧host均不开放控制。浏览器捕获同一UID/instance用于claim、force及WS，持久布局绑定同一身份，不按name猜测或自动追随新实例。
- `tests/managed_terminal_browser.py`已通过真实legacy按钮与xterm键盘、两个独立页面force/revoke、移动导航、Web重启同一免费shell、native文件字节不变。另先复现开启terminal误带出发送composer，再由独立outbox能力关闭，保留草稿且不发发送请求。Node合同目前29项通过，含持久布局实例身份和拒绝旧视图重连，保留Python无能力声明的原行为。
- Codex names显式索引7项单测及合成Python/浏览器差分通过；配置新增4项边界测试，合计7项config测试通过。拒绝与static/native/host/state/file roots重叠，原路径保留供no-follow检查，不自动发现CLI home。
- Grok 11项相关单测、sessions 94项及合成Python/Chromium差分通过：summary-only/0B chat、metadata fallback、相同cursor的exists变化SSE、正文追加与损坏恢复。消息ts和目录size的明确差异记录于专属文档；不声称原生媒体已支持。
- `delivery/engine`15项、全部delivery79项通过：独占typed Machines/store、两家恢复屏障、先持久化再释放动作、唯一pending重试、弃置/旧engine遗留batch失效、保守legacy outbox投影。仅同步库，尚无HTTP/异步executor/真实CLI原生确认适配器，不把该成果算成M5完成。
- 本批统一workspace快照382项通过（server258、files9、HTTP10、metadata4、runtime4、search10、terminal11、terminal_bound5、ptyhost36、client34+doctest1），2项ignored；server/client所有targets Clippy无警告，Node29项和基础legacy Chromium再次通过。后续host输出修改须重新验收，不能沿用此快照。

### 第七批：输出隔离与账本服务接线

- 已新增显式 `--initialize-delivery ABSOLUTE_DIRECTORY` 本地管理入口：先验证独立目录，blocking初始化后退出，不启动Web或模型CLI。CLI集成2项通过，覆盖help/非法参数、已占用配置端口仍可初始化、重复调用保留原字节、拒绝相对/不存在目录、OS锁释放后可重开。
- `delivery_dir`配置隔离已实现并通过12项config测试（本批新增5项）：默认None、保留原路径、Unix0700/祖先no-link、全部边界双向排斥，包括alias及未创建的配置子目录；from_env通过隔离子进程测试，不改全局环境。Web使用async prepare_app，只打开已初始化账本并完成两家epoch恢复；同步工厂不静默忽略配置。初始化不是导入、生产迁移或完整M5验收。
- DeliveryService 12项单测与outbox HTTP8项通过：单coordinator、有界blocking读/序列化，8个queued/active/未释放response总准入，Body32KiB输出仍持permit，取消不丢worker，关停等待OS锁释放。outbox_read独立能力，不启用send/retry/discard或假queued。
- NativeScope 8项新单测、sessions合计102项通过：原生ID证据与展示sid独立，Claude子代理使用owner UID/真实parent sessionId/完整agentID；缺失、冲突和错owner显式失败，同一已验证snapshot/解析缓存，不改普通历史兼容。
- host输出已完成本批隔离验收：47项单测与4项真实免费shell集成通过，复现后修复ACK前live、慢client阻塞publisher、旧200ms退出丢延迟尾部；独立有界队列/写线程、共享退出deadline，真实PTY EOF或显式incomplete marker，不杀未知后代。
- typed exit与WS保留尾部后再关闭，裸socket EOF不冒充进程退出；terminal HTTP合计13项通过。`terminal_exit_browser.py`先复现旧前端自动reclaim覆盖具体退出错误，再修复为保留当前xterm、停止已退出实例重连、灰色可点击按钮解释；桌面/移动端、正常EOF及不完整输出均通过，native字节不变。
- 本批统一workspace快照435项通过、2项ignored；server/client所有targets Clippy无警告。Node30项、managed console、基础legacy及raw transport Chromium通过；raw旧测试更新为区分已配置terminal能力与无native关联的空sessions，仍明确关闭create/outbox。Windows交叉编译通过仅是编译证据，不是实机验收。

### 第八批：未绑定原生ID的创建实例（进行中）

- 已复核旧Python创建流程：Claude/Grok预分配SID仅证明旧代码调用方式，Codex未知SID的cwd/PID候选没有跨平台启动回执；不得照搬多候选选最新或将pending冒充native身份。
- 下一步拆分独立launch guard/typed LaunchTarget、持久创建回执，再接显式launcher与pending HTTP/WS。launch身份不是SID/UID，不开启可靠发送；一次性原生绑定协议与真实CLI参数契约另行验证。
- 已补写 [生命周期接线合同](docs/lifecycle-integration.md)，明确持久化先于spawn、取消/重启不重复启动、独立pending租约与后续一次性原生绑定边界；此设计不等同于已开放HTTP。
- `terminal_exit_browser.py`扩展真实同名重建，先复现live:false导致不刷新终端列表，再补Rust独立有界轮询；正常/不完整退出、hover/focus/click、手动新实例接管通过。Node31项、managed/basic/raw Chromium和高级合成历史Python差分再次通过，未启动模型CLI或改原生字节。
- host `launch_guard_v1`独立协议已实现：immutable source/launch/instance全部匹配，ROOT capability launch_guard:1、独立ACK；54个host单测、3个真实launch shell集成及4个输出集成通过。pending没有SID/UID也可typed控制，但不能通过native guard；旧操作保持兼容，不实现自动关联。新fixture显式cwd/env、限时等待，仅清理自己持有的测试host。
- `LaunchTarget`客户端新增11项测试通过，info/record一致性覆盖launch身份，strict ACK后才暴露attach流；metadata解析不把launch误作native关联，不泄露任意meta/token。此库不是浏览器pending租约，HTTP仍未接线。
- `lifecycle`持久创建回执14项专项测试通过：独立私有目录、128条/1MiB硬上限、不淘汰幂等记录，new Prepared/Starting均先持久化才给不可Clone的本handle authority；相同spec重放不授新权、异spec冲突。Starting重开durable→Uncertain，旧handle/cross-directory token失效，写失败冻结，cwd变化/serde绕过不能启动；只读历史重放不要求旧cwd仍存在。详见 [回执合同](docs/lifecycle-store.md)，尚不启动任何进程。
- 本批统一workspace快照470项通过（server297、HTTP系列65、host54+7、client46+doc1），3项默认ignored；server/client全targets Clippy无警告、Windows workspace交叉编译通过。新增typed真实host互通测试通过显式binary单独运行：guarded Info/attach、仅合成磁盘记录被伪造时host仍拒绝kill/resize、原始字节与完整退出、同名替换旧target失效。新fixture全部private cwd/最小环境，不启动模型CLI。
- 原Python仓库本轮并行推进到`9b1c2fd`（Codex composer particle glyphs，涉及codex_bridge/server/app及测试）；原worktree保持clean，本任务未修改/提交该仓库。冻结reference仍是`ee2e373`，这笔并行修复未自动导入，后续可靠发送/屏幕观察适配须重新核对，不能把本批验收当该修复已迁移。

### 第九批：显式 launcher、生命周期服务与 pending 浏览器链路

- 显式私有 launcher JSON：版本化 adapter/executable/固定 args/env/cwd roots，64KiB/0600/no-follow 校验，禁止浏览器命令拼装；env_clear、一次 spawn、authority 不随错误丢失。默认无配置不启动任何 CLI；非 Unix 实际启动仍明确不支持。
- `--initialize-lifecycle` 只初始化已有空私有目录并退出；Web 要求 ledger/config/host 三者明确配置，启动恢复失败不服务，静态资源失败与关停释放 writer。配置边界覆盖 static/native/state/delivery/files/index，工作目录不扩大为私有数据权限。
- 独立 8 请求 coordinator 与 128 槽 Child reaper；persist-before-spawn、重复请求只返回同一回执、结果丢失不丢进程句柄。状态查询要求 fresh launch-guarded Info，列表共享观察 deadline。schema1 严格持久迁移至 schema2，取消意图跨重启保留，不自动再次 spawn/kill。
- 新 create/status/cancel HTTP + pending list，8 个独立响应准入持有至 Body 消费或丢弃；有界 blocking JSON 编码上限 2MiB，输出 32KiB 分块。拒绝客户端 argv/env/executable/native UID 混入创建身份；成功回执不等于 Running。
- pending/raw/native 三类租约互斥且不降级，claim/force/WS 固定完整 record/launch/instance；取消先退休输入再 guarded stop，WS4002 不冒充退出，现有 xterm 尾部及持久回执保留。legacy 仅沿用现有弹窗/侧栏/移动停止菜单，不猜测原生关联、不自动 discard，不启用可靠发送。
- host stop 补同一 child Mutex 跨 try_wait→signal，已回收/查询失败不使用旧 PID；新增3项 host 单测。client 新只读 `status_launch` 能证明已退出，但不暴露可写 LaunchTarget。
- 浏览器五套已复验通过：lifecycle 创建/幂等/输入/Web重启/移动取消、managed console、raw transport、完整/不完整退出及手动同名替换、基础 legacy 历史/SSE；Node32项通过，native fixture 字节不变。
- 全量回归发现新 HTTP 测试并行争抢进程级启动预算导致 Busy；已用文件内测试锁隔离独立启动，不调大生产预算，连续5轮9/9通过。额外审查发现瞬时退休失败后的取消重放须重试本地退休而不得再次 kill，已补回归验证首次 Busy、释放在途 claim、再次取消撤权且 kill 数仍为0。
- 最终 workspace **522项通过、5项默认ignored**（server单元335、HTTP系列75、host57+7、client47+doc1）；server/client全targets Clippy无告警，Windows workspace交叉编译通过，仅代表编译而非实机验收。3项需显式 binary 的免费 shell 测试分别单独通过：launcher、lifecycle service、typed client；另外2项默认ignored为已有runtime smoke与host benchmark，本批未单独重跑。
- lifecycle service免费shell验证 dropped response 后启动仍被持有、重复不spawn、shutdown/reopen后同host继续运行和精确取消；同进程只按完整身份订阅仍由reaper持有的Child，不无限保留退出记录。真正跨Web进程重启后host不可达仍只能Uncertain。原Python仓库保持clean/`9b1c2fd`，未修改冻结reference、未提交/push/部署、未运行模型CLI。

### 第十批：一次性操作者绑定与派生权限

- Host新增独立outer `launch_bind_v1`，不加入raw/其他guard的通用操作；完整launch/source/instance及native SID/UID严格核验。child存活检查与一次性原子发布，同值重放、异值永久冲突。Info root公开受限binding状态，immutable meta/磁盘record保持不变，现有pending attachment不被升级或中断。
- Client新增独立`NativeBindingState`及`bind_launch`，capability/身份/ACK严格校验，`status_launch`取最新guarded Info而不是拼接旧probe绑定。BoundTarget保留origin_launch_id，native元组不能代替launch退休约束；缺失/坏ACK不重试，不从磁盘meta恢复绑定。
- `SessionStore/SessionSnapshot::native_catalog`从同一已验证snapshot生成真实NativeScope目录；新binding不能使用display SID。保留冲突文件声明以防过滤后假唯一，子代理不能作主绑定。审查发现Grok旧metadata及Codex display alias兼容回归，已通过独立同snapshot legacy目录修复；新binding仍无legacy fallback，Grok新绑定明确不支持。
- schema3持久化固定BindingSpec/Intent/Confirmed/Uncertain；严格迁移旧schema1/2，重启先降Uncertain且只观察不重新bind。明确人工确认生成不可serde的VerifiedNativeBinding，先持久Intent才调用host，ACK后仍需fresh Info；同值显式重试、异值冲突保留原意图。离线/超时不能沿用cached Confirmed。
- POST `/api/term/bind`要求完整回执/instance/UID与`operator_confirmed:true`，SID只由server verified NativeScope提供。8响应准入/8KiB body沿用生命周期闸门，未知字段、子代理、Grok新绑定、缺失/冲突身份均拒绝。关联是操作者声明，不是原生CLI写入证明或可靠投递确认。
- pending/native互不自动升级，即使force也拒跨类型租约；显式释放本页连接后才能申请native。`authorize_native`在claim和attach前核验本ledger唯一receipt、fresh确认与取消状态；registry退休同时撤销精确launch衍生native，不能通过Web重启空registry绕过。
- 实际legacy弹窗/移动窄屏、原pending socket不变、显式释放→native键盘输入、独立浏览器存储取消native租约、Web再次重启后拒绝仍活着的host全部通过。测试shell特意忽略HUP，证明拒绝来自持久取消而非断网/死亡；native字节和host meta不变。第二tab共享存储自动恢复曾提前接管，已用DOM/4001证据定位并改为独立测试存储，没有改正常接管逻辑。
- 补测先复现跨类型打开虽拒绝、持久布局却被改成native的前端问题，已将身份检查前移到布局/DOM写入之前，保持旧pending socket和保存身份。
- 最终workspace **567项通过、5项默认ignored**（server366、HTTP系列77、host61+12、client50+doc1）；Node35项，新增native binding与原有5套Chromium、10视图高级Python合成差分全部通过。server/client全targets Clippy通过；Windows全workspace/alltargets交叉编译通过，不代表Windows/macOS实机。没有运行付费CLI、提交/push或部署。

### 第十一批：原生内嵌 PNG/JPEG 与工具媒体

- 新媒体源采用typed私有字段，不将原始base64放进消息JSON或搜索；普通文本消费者不触发解码。HTTP/SSE先选完整语义分支和分页窗口，再一次注册整个选中批次。
- 媒体读取仅接受不可猜测的32hex本地token；不打开正文路径，不抓取外链。独立8响应准入和已有4个blocking reader配合，Body及消费者保留的字节frame继续持有响应额度和缓存blob。
- legacy沿用已有图片gallery，新增`media_remote:false`能力时拒绝自动加载外链图片；未声明能力的Python页面保留原行为。Node合同36项通过。
- 新增HTTP5项与真实Chromium通过：三家PNG/JPEG实际解码、Claude/Codex子代理和分叉前缀不串图、刷新/SSE/移动端、外链零自动请求及native字节不变；另覆盖缓存淘汰404后重新投影、8个未消费Body及已yield仍持有的frame保持准入额度。
- 集成审查收窄媒体识别：普通用户JSON教程/工具参数不是原生图片，不因出现`image_url`字段就改写或拒绝；仅结构化内容块及已知工具output信封按媒体解析。另限制信封候选扫描，避免普通长文本按每行后缀反复解析。
- 明确MCP根content包装纳入工具媒体，保留isError；业务JSON的content数组不自动升级为MCP。13项新增provider回归覆盖这些边界；最终浏览器另以真实Grok MCP包装验证同一gallery链路，不仅测试数组格式。
- 媒体模块12项、窗口/纯文本消费者/追加/批次准入4项通过。单图base64-decoded压缩图像字节≤1.5MiB、缓存≤32MiB/256项、事件≤16图；现有2MiB JSONL记录限制仍含信封。PNG CRC/容器和JPEG marker/尺寸校验不是完整像素解码，预算不是进程RSS或浏览器内存承诺。project([])不争用解码锁；held Arc按最终释放计费。详见[媒体合同](docs/media.md)。
- 最终workspace **601项通过、5项默认ignored**（server395、HTTP系列82、host61+12、client50+doc1）；Node36项、server/client全targets Clippy、fmt检查通过；Windows workspace/alltargets交叉编译通过，不代表Windows/macOS实机。最终构建后的媒体/高级历史/工具diff/基础legacy/搜索/文件六套Chromium和高级合成Python差分通过。未运行付费CLI、提交/push或部署，原Python worktree仍clean/`9b1c2fd`。

### 第十二批：图片格式扩展与授权磁盘媒体

- 保留legacy框架与已有gallery，扩展GIF/WebP/APNG动画、静态AVIF和BMP；容器/声明尺寸校验不声称完整像素解码。动画最多128帧/64Mi累计canvas像素；AVIF使用纯Rust parser，显式拒绝未支持序列/grid/变换并在调用前限制box/item/extent放大，防解析异常污染媒体缓存锁。
- 原生结构化路径只保留在typed私有媒体源；Markdown/聊天裸路径发现不授予文件权限。读取必须显式配置独立文件根，使用完整当前分支/子代理索引（不是分页窗口），不从cwd或native根推断授权。file:///仅解码一次，普通百分号保持字面，禁止网络authority/HOME扩展；Unicode、盘符与第17个显示候选之后的basename歧义均覆盖。
- PreparedImage保留CheckedImage句柄，统一缓存预留之后读取，不重开display path。磁盘压缩图片≤32MiB，AVIF仍≤1.5MiB；内嵌仍≤1.5MiB/现有2MiB记录，所有resident blob共用32MiB/256项预算。PNG/APNG临时校验副本≤32MiB，另有有界AVIF scratch；这些不是总RSS承诺。
- 磁盘token每次投影重新产生，缓存项持有owner UID/agent/ref/FileVersion。GET/HEAD重新解析当前native分支、核引用/根/文件与祖先身份版本，文件替换旧token409，删除引用撤销旧token访问；不能通过embedded get绕过FileTicket。已交付bytes不可追溯撤销，尚未承诺图片文件监听，显式重载取得新token。
- 可选磁盘图片缺失、越权、格式坏或预算占用时显示单图错误，保留文本和其它图片；结构化私有路径不反射。混合路径/内嵌的媒体顺序保留，读前后替换不会发布新路径的bytes，held响应跨embedded/file共享原32MiB预算。发现最多16图/消息、256正文候选/窗口，原生事件/批次硬限制保持。
- 实际Chromium新增两套：六个真实格式fixture（三家工具结果、实际GIF/WebP帧变化、移动错误页）；磁盘三家path/fileURI/Markdown/raw/未知扩展真图、未配置可见解释、越权/分支/子代理、替换409+重载。Linux inotify验证窗口省略图片零IN_OPEN，完整视图作有实际OPEN的正对照；不是只看API推测未读。
- 浏览器先复现移动错误页缺少返回列表按钮，最小补loading/error占位header的mobile-back并复验；原灰色控制台保持可见、可点击具体解释。新单图错误转义合同避免错误消息注入HTML，不引入框架重构。
- 全workspace **647项通过、5项默认ignored**（server432、HTTP系列91、host61+12、client50+doc1）；Node37项、高级10视图Python合成差分、server/client全targets Clippy与fmt通过。最终构建后媒体/新格式/磁盘媒体/历史/工具diff/基础legacy/搜索/文件八套Chromium全部通过。Windows `x86_64-pc-windows-msvc` workspace/alltargets交叉编译通过；最初误选未安装的gnu target报缺core，改用现有msvc验证，没有把交叉编译称作Windows/macOS实机验收。
- 原Python仓库仍clean/`9b1c2fd`；未运行付费CLI、提交/push/部署，未接生产或真实CLI记录。上述两项旧runtime/benchmark及三项需显式binary的默认ignored测试本批未另行运行，保持之前批次证据，不冒称本批重验。

### 第十三批：追加AST复用与媒体语义差分

- State新增独占owned-record缓存，decode前取出Vec，投影后移回；不深clone旧JSON、不随View Arc无限pin。必须匹配旧Candidate与稳定file identity，并对新读文件的完整旧committed前缀做字节相等核验；不靠size/mtime/4KiB head猜append。原完整mtime/ctime/restamp和文件读取校验全部保留。
- 仅反序列化新增完整行，partial尾部从旧committed重新解析；坏行/2MiB单行/累计50000条仍failclosed。改写、已提交截断、替换、淘汰均回退全量；摘要改变可复用JSON但重新投影metadata。缓存32MiB逻辑weight/16项，按容器/节点/字符串计费，超过缓存预算不改变HTTP语义；额外weight遍历越预算后停止。不是RSS上限，也不是增加原raw/view/media预算。
- Provider仍全量重算；新增集成计数证明append只decode一条但Claude last-prompt换枝与Codex abort仍能修改旧消息并reset。13项缓存单测对照cold records/error/committed/weight，覆盖全前缀改写、partial、inode、LRU与累计预算；不会把纯Event拼接冒充增量解析。
- 新 `append_benchmark.py` 对1k/5k/10k×三家×3样本分别验证旧/新release，共每binary27组。相邻串行10k追加p50下降Claude18.5%/Codex21.4%/Grok7.1%，首次窗口却增加4.5%/3.3%/19.1%；1k Grok追加略慢，rewrite基本持平。小样本smoke不声称统计显著或整体性能完成，测量binary摘要与全部取舍见[追加缓存说明](docs/append-cache.md)。
- 媒体严格差分先发现Claude混合native块被合并、图片计数少算及Codex/Grok多余占位。实际DOM复现1条对8图及tool文本误折叠后，恢复Claude逐block事件，其他provider无文本才单个占位、mixed/tool保留纯文本；不改调用/阶段/隐私与新增MCP支持。全字节/MIME对照与明确delta见[媒体差分](docs/media-parity.md)。
- 大图设计评审确认32MiB base64约42.7MiB，现有record/file/view边界和legacy全量中间历史请求不能靠涨一个常量解决。下一步明确为真正事件分页、独立页游标、按需media descriptor/blob与private native span扫描器；[设计及验收清单](docs/media-pagination-design.md)尚未实现，保持完整剩余目标。
- 最终workspace **663项通过、5项默认ignored**（server448、HTTP系列91、host61+12、client50+doc1）；Node37项、server/client全targets Clippy与fmt通过，Windows MSVC全workspace/alltargets交叉编译通过（不是Windows/macOS实机）。最终构建后八套既有Chromium＋新增媒体差分DOM套件共九套全部通过；高级10视图Python差分也通过。媒体49场景明确为28语义一致+21已断言差异，不冒称49全部相同。
- 原Python保持clean/`9b1c2fd`，未运行模型CLI、提交/push或部署；默认ignored的独立shell/runtime/benchmark未在本批额外运行。本批测量的pre13/newrelease摘要另存说明，未把性能观测改名为最终binary测量。下一批继续实现，不将本批通过当作整个后端迁移完成。

### 第十四批：有限事件分页

- 新 `sessions/pages.rs` 使用非status事件位置，而非字节end或message_total；同一原生记录的多事件、继承end零、counted:false均不漏。随机token固定canonical UID/精确agent/完整旧checkpoint及初始gap范围，不保留View或原生正文。普通append仍能补旧gap，重写/rewind/父前缀变化409。
- 初始窗口优先最新最多500条，再补最早最多100条；每页最多200事件。初始窗口/页共同遵守128个typed原生图片引用、24MiB估算embedded bytes、最终8MiB JSON预算；大消息阻断时已有页进度仍能返回，单条无法分割明确413。257图可跨页读，不提升单图/单条原生记录限制。
- `GET /api/messages/{uid}/page`只返回messages与page范围，不返回或推进实时cursor。1024条grant/十分钟TTL、不消费的重试、淘汰404/仍存的过期410；8个独立读取/响应准入覆盖取消中的worker、未消费Body和retained byte frame。SSE初始与后续reset同样有限window，显式非window HTTP仍保留原有完整view行为。
- legacy以Rust `history_pages`能力选择新gap路径，未声明能力的Python保留旧行为。旧版实际Chromium已复现600条窗口点击后一次全取1400条；新路径保持SSE，逐页拼接并恢复gap位置，旧视图/reset响应丢弃，HTTP失败保留快照并显式重载。并行审计发现并修复reload覆盖新append、yield期间activity-only旧状态回画和畸形消息破坏页面；reload额外固定activity/meta/prompt观察，page仍安全合并新尾。
- 最终workspace **687项通过、5项默认ignored**（server462、HTTP系列101、host61+12、client50+doc1），含新增14项分页单测和10项HTTP测试；Node **44项通过**。fmt、server/client全targets Clippy、Windows MSVC全workspace/alltargets交叉编译通过，不代表Windows/macOS实机。新增HTTP SSE用真实prefix重写验证初始/后续reset均有限窗口，连续append仍独立推进。
- 新Chromium分页套件已通过：真实响应延迟期间SSE追加、render分批yield期间追加/纯活动状态变化、旧视图/reset丢弃、畸形null消息保护、404/409/410显式恢复、重载与追加竞争、精确agent、移动gap位置/最终1403条顺序和控制台。另直接驱动真实covered-activity处理器验证原地中断标记与工具轮次重建；不是把该合成时序冒称原生CLI运行。最终资产统一复验包含该新套件与既有九套Chromium，十套全部通过；高级10视图Python差分通过，媒体仍为28一致/21已断言差异。
- 合同与剩余边界见[有限历史页](docs/history-pages.md)。未实现lazy decode、per-message media continuation或大原生图片span；不会把有限页等同于完整旧32MiB嵌入图片兼容。
- 原Python仓库仍clean/`9b1c2fd`。未运行模型CLI、读取真实CLI记录、提交/push/部署；默认ignored的独立shell/runtime/benchmark本批未另行运行。本地迁移继续下一批，不将分页验收当整个后端完成。

### 第十五批：按需媒体与独立来源预算

- 新 `media/descriptors.rs` 将登记和物化分开，生产不调用旧eager投影；旧路径仅保留为底层格式/预算单测助手。描述符表1024项，embedded强引用按String capacity计入独立32MiB预算，不pin View；被淘汰但仍被ticket持有的来源继续计费。普通append重新投影/释放旧视图不使未GET的旧来源意外丢失。
- 历史/page/SSE仅返回`src/alt/lazy:true`，不伪造尺寸或已验证MIME；读到坏容器仍保留文字，到GET再明确422/413。结构化source/alias/编码长度边界保持早期拒绝。每次GET单图分配前预留32MiB共享blob/256项预算，held Body/frame沿用Arc生命周期；descriptor/blob锁不跨读图或解码，同token并发物化有明确Busy。
- 文件登记只授权/open/stat并捕获版本，不读图或留长期句柄；GET包括warm hit均重新解析当前分支、引用、根和版本。首GET前替换旧token409、原生撤销引用后的warm拒绝有HTTP证据。Linux IN_ACCESS/IN_OPEN检测证明选中history登记有open但零read、首GET才有正读取；不是仅按函数名推断lazy。
- API最多8个queued/active/held响应，实际media worker最多2个；其余在8额度内最多等待2秒，不占共享Reader。确定性barrier测试验证取消排队/已开工请求的容量分别何时归还，两个media worker阻塞期间普通历史仍能读。图片容量不足不绕过预算或释放仍在途的bytes。
- Rust `media_lazy`分支先复现GET503只有破图，再加可见错误、同token手动重试及404/409显式有限窗口重载；Python不变。错误诊断每token去重、128缓存、4并发/5秒/4KiB响应上限，成功image立即cancel body，结果不污染旧视图。无partial完整会话也能显式恢复；诊断/重试不推进livecursor，不自动全量或无限重试。
- 最终统一workspace **705项通过、5项默认ignored**（server476、HTTP系列105、host61+12、client50+doc1），Node **50项通过**；fmt、server/client全targets Clippy及Windows MSVC全workspace/alltargets交叉编译通过，非Windows/macOS实机。新增12项descriptor单测、2项HTTP lazy及2项文件冷/热授权回归，保持18项媒体HTTP整体通过。高级10视图Python差分通过；最终Rust reset修复与HOOK校准后的全部11套Chromium再次通过，媒体49案例仍为28一致/21明确差异。
- 独立审查实际复现reset分批render期间SSE导致DOM倒序，Rust reset现统一复用entry协调合并新尾/活动状态，Python默认路径不变。另发现上一批宽泛stack timer hook可能停在嵌套helper、未真正暂停render；已校准直接Promise/render帧，并断言fragment尚未发布/代际未变化，重验真实yield期间append与activity，不沿用旧hook作为此边界证明。
- 最终两套分页/lazy Chromium额外覆盖：图片显式reload暂停时native append及failed→idle纯状态变化，历史gap显式reload暂停时native append，page暂停时append/activity及原地covered中断。缓存和DOM的顺序、恰一次新尾、livecursor、未发布fragment证据同时核验。所有fixture仅临时人工数据，工具记录未执行。
- 合同已更新[媒体](docs/media.md)、[差分](docs/media-parity.md)、[分页/大图剩余设计](docs/media-pagination-design.md)。未实现native span/32MiB内嵌/单消息多图延续，仍继续全部迁移目标；没有付费CLI、生产、提交/push/部署。
- 原Python仓库最终仍clean/`9b1c2fd`；5项默认ignored的独立shell/runtime/benchmark未在本批额外运行，不沿用旧实机证据冒充本批重验。没有报告未经测量的性能倍数或RSS改善。

### 第十六批：可信输入、物理索引与结构扫描基础

- `sessions/native_input.rs` 新增 retained checked handle：逐目录nofollow、普通文件/nlink核验、每次最多64KiB、完整消费和末次stamp核验。`RawIndex` 在同一读流上建立每LF完整SHA1检查点（含空行）、前4096字节和原始全摘要；零点/半行/物理size/父cut与`rs-m2-1`游标保持原义。
- 独立进程级32MiB索引RAII按结构及实际Vec/String capacity计费，覆盖旧Arc快照、扫描中对象和扩容新旧allocation重叠；满额413不覆盖旧发布索引。最多100000 LF是明确新增预算，不等同旧50000非空记录限制。17项native input测试覆盖真实临时文件替换/截断/链接、生成流、检查点和并发/失败/旧Arc计费。
- 新私有结构scanner验证JSON语法、UTF8/代理对、转义key、重复键、深度/节点/key/number/resident预算。普通记录移动为Value；超阈值仅生成未授权TextSpan，带物理范围/解转义长度/全文SHA1。仅转为span时才初始化摘要缓冲；约42.7MiB生成式源以小块扫描、逻辑resident峰值低于80KiB，**不是**32MiB图片HTTP已支持或RSS测量。
- 生产record cache及父prefix重解析已接strict小记录adapter。仍保留原16MiB文件/2MiB行和`Parsed.bytes`供完整prefix验证/父prefix切片，不能宣称已移除整文件驻留或放宽大图。新的结构预算会提前拒绝物理很小但含几十万节点的记录；旧超重缓存fixture已显式改为拒绝/不缓存回归。重复key拒绝及修复后恢复新增生产测试。
- 实际tool parity及Chromium复现初版BTreeMap重排对象参数，改变摘要选取。Value相等本身忽略对象顺序，旧10项合同不能作为保序证据；改用锁中已有IndexMap版本，新增有序序列化及5000逆序key测试，外部合同扩到11项（旧实现5项明确失败→修复全通过）。真实DOM断言固定`z=first a=False r=4 b=last`，不改legacy JS。
- 最终Rust workspace **748项通过、5项默认ignored**（server519、HTTP系列105、host61+12、client50+doc1），Node仍**50项通过**。格式、server/client全targets Clippy及Windows MSVC全workspace/alltargets交叉编译通过；不代表Windows/macOS实机。最终保序debug上的全部11套Chromium、10视图高级Python差分通过；媒体仍是28一致/21明确差异。动画格式套件首次并行截图只采到单帧，原样独立复跑通过；保留波动记录，未改断言，也未把CPU原因当已证实。
- [输入契约及剩余边界](docs/native-input.md)明确hardlink/结构预算差异、Linux运行证据与非Unixstamp限制。索引/扫描器只是大span管线基础，provider归类、嵌套JSON串读回、native当前分支授权及冷/热GET仍未完成。原Python仍clean/`9b1c2fd`；没有模型CLI、提交/push、部署，默认ignored付费/独立运行边界未扩大。
- 同脚本前后release各30个合成样本（3来源×1000/10000条×5次）游标/同长重写检查全通过。1万条首次窗口p50 Claude176→224ms、Codex113→142ms、Grok86→121ms，追加约增加9–11%；不是性能提升。完整二进制摘要/p95及小样本局限见[测量](docs/native-input.md#batch-16-release-comparison)。接下来的span接入必须同时profile并减少重复表示/扫描，不能隐藏这一冷读成本；本批未测RSS。

### 第十七批：流式记录缓存、父前缀与直接构造

- 删除生产`Parsed.bytes`，普通记录以最多2MiB当前行缓冲在同一raw-index读取流中解码。超大半行不保留全文、不提前报已提交错误；LF后明确拒绝。原文件/行/图片限制不放宽，仍未支持32MiB内嵌图。
- AST缓存复用改用完整旧committed前缀SHA256（不是头部抽样），另保留所有SHA1公共游标不变。与previous候选身份/边界匹配且强摘要一致才接受旧AST；不一致先丢弃推测AST/index再打开同stamp冷读一次，第二次失败不恢复陈旧缓存。完整provider投影仍执行，不拼旧Event。
- `CheckedNative::open_prefix`读取严格限制在[0,cut)，但前后校验整个expected文件。父固定prefix也走同一decoder/index，检查物理LF cut及原prefix摘要。原纯图结构单测现在保留真实临时文件，不引入测试专用bytes后门。新增7项native input回归，专属共24项；新增4项流式record/缓存失败回归。
- scanner新增直接`scan_value`，共享同一泛型语法/保序/重复键/所有逻辑预算，避免Node→Value对象重建。两条路径序列化与ScanStats对照、深度/错误/边界测试通过；显式free ignored CPU基准5次交错，native组约51→36ms、小对象68→50ms、大纯文本基本持平，不把该局部收益冒称HTTP收益。
- 最终workspace **761项通过、6项默认ignored**（server532、HTTP系列105、host61+12、client50+doc1），Node **50项通过**。新增真实HTTP `native_streaming.py`、高级10视图差分及全部11套Chromium通过；formats本批首跑动画多帧通过。媒体仍28一致/21明确差异。fmt、server/client全targets Clippy、Windows MSVC全workspace/alltargets交叉编译通过，不代表Windows/macOS实机。新增ignored仅纯CPU基准已显式运行；其余5项旧ignored未额外运行。
- 前后release各30样本真实HTTP+Linux测试进程RSS测量：1万条冷读p50 Claude216→195ms/Codex152→130ms/Grok108→105ms，追加改善约0–6%；峰值RSS下降约4–6MiB，但最终阶段当前RSS未一致下降。完整摘要/p95/观测局限见[输入测量](docs/native-input.md#batch-17-direct-construction-and-streaming-comparison)。未完全恢复batch15性能，不宣称大图能力或全负载内存上限。
- 原Python仍clean/`9b1c2fd`，无付费CLI、提交/push或部署。下一步必须把私有span接入真实provider图片上下文、嵌套JSON解转义及冷/热GET当前branch授权；当前只移除整原文驻留，不能以这些基础验收缩减原迁移目标。

### 第十八批：结构化原生大图与可信范围 GET

- 拉取式record扫描将所有物理字节交给RawIndexBuilder，保留首个I/O/预算错误；partial不提前提交，首次Interrupted重试。≤64KiB仍直接Value，长记录先私有Node，完整结构校验后才提取span。普通大正文/参数/教程仍拒绝，去掉图载荷后的普通JSON独立限制2MiB，不复制正文计数。
- 私有sidecar按最终Value地址关联三家provider结构化消息和工具/MCP结果；不伪造JSON标记、不用空base64替代，也不把clone或重新解析的字符串JSON视为原始授权。字段别名校验完整payload摘要和规范MIME。sidecar按path/String/Vec capacity计费，每record最多256项/8MiB，记录缓存额外计算外层容量。
- NativeSpan保留可信root/path/file_identity/真实record与string范围及全部decoded/payload摘要；不保留大encoded正文。完整当前branch/精确agent授权后，用生成该view的candidate stamp打开range，冷暖均读源校验，JSON EOF及retained checked handle通过后才发布HTTP。parent继承仍保留真实来源与leaf-only end，scope token独立。
- 结构化native span单图32MiB，+1 byte明确413；AVIF独立1.5MiB不放宽。physical文件/清单扫描各256MiB，summary原预算不变；span的view/page仅计metadata，GET仍共用独立32MiB blob及inflight/HTTP RAII预算，encoded_len没有伪造为零。image-semantic-v2统一inline/span/dataURL，明确跨版本旧图片cursor一次reset。
- 新真实HTTP/Chromium检查三来源大图、转义、等价/冲突alias、正文伪装拒绝、文字搜索、冷暖读、append/rewind、fixed-parent-cut、同长恢复mtime重写及子代理精确scope。最终workspace **814项通过、6项默认ignored**（server585、HTTP系列105、host61+12、client50+doc1），Node50项通过；最终release全部11套既有Chromium与新增native-spans浏览器检查通过，高级10视图差分/新native-streaming/authority通过。媒体仍28一致+21明确差异；fmt、Clippy、MSVC全workspace/alltargets交叉编译通过，非跨平台实机。默认ignored本批未额外执行。
- 真实32MiB测量发现PNG/APNG validator原有整图临时副本，现删除并保持双CRC/原检查顺序；新增5项旧filtered oracle/截断/损坏/语义比较回归。明确安全收紧：旧strip路径可漏过IEND后的fdAT，新实现拒绝，不伪称完全等价。前后release与实际大图RSS/HWM测量见[输入说明](docs/native-input.md#batch-18-measurement-method-and-intermediate-finding)。
- 最终普通前后各30合成样本通过；1万条冷读p50约增加1–6%，追加约-1–6%，峰值RSS基本不变，不宣称整体提速。3/32MiB各3个fresh-process大图样本通过，32MiB首窗口/冷GET/暖GET p50约242/515/175ms；去PNG副本后该场景峰值约74→48MiB，首窗口RSS约16MiB。完整binary摘要、p95及小样本/合成padding限制均保留。
- 本批仍未实现巨型stringified工具JSON和单消息media continuation；不修改原Python，不调用模型CLI，不提交/push/部署，不声称跨平台实机或全迁移完成。

### 第十九批：嵌套字符串工具输出与共享回放预算

- 新 `native_replay`：`DecodePlan`（一段真实外层物理范围 + 至多8段内层`StringRange`，内层偏移只针对上一层解码文本）、`WorkBudget`（跨层/候选/重开共享、失败标志粘滞、clone不补充）、`ReplayReader`（逐层`JsonStringReader`+`Window`，finish时排空每个父层未读尾部并校验各层长度/SHA-1与外层精确EOF）。计划只描述范围，不携带文件权限。
- 新 `records::tool_envelopes`：只从Codex `function_call_output`/`custom_tool_call_output`/`local_shell_call_output`的`output`、MCP `{content,isError}`单文本块和单元素文本数组取候选；按旧parser顺序（最后一个`Output:\n`、首个非空白、≤32个对象行，Unicode空白裁剪）流式定位，每个候选用结构scanner（重复键/128深度/2MiB inline/8MiB resident）重扫，仅接受含`output`+`wall_time_seconds`+`exit_code|session_id|chunk_id`的完整对象；语法miss继续下一候选，来源/预算/结构错误致命；每次打开都读完并finish整个checked来源。普通正文、工具参数、未知巨型对象、重复键、第九层和跨多文本块组合明确501。
- `native_records::replay_source`按当前记录candidate stamp打开外层范围，分类回调不获得任何路径打开能力；预算用尽的409/413判定发生在失败读取或finish之后（本次接手修正了finish前预判导致的误判，并补单测）。被接受的信封替换为私有结构树后继续沿`output`找结构化图片，图片`NativeSpan`携带plan并按堆字节计入sidecar；MCP `isError`保留为独立sidecar，数组包裹的MCP不继承该标志；去图后普通JSON仍受2MiB限制。
- GET：`native_media_reader`按plan重建层叠读取器，最内层流式base64解码后必须完成全部父层finish与retained checked handle才发布；GET自有512MiB预算。图片字节相同不授权：外层非图片字段同长同mtime改写→冷/热token 409；普通append保留旧图；固定父cut与Codex子代理保持真实外层范围与leaf-only end。
- 验收：最终workspace **849项通过、6项默认ignored**（server lib 620、HTTP系列105、host61+12、client50+doc1），Node **50项通过**；`native_envelopes.py --browser`（1/2/4/8层、`Output:`前缀、转义内层base64、Unicode裁剪、混合结构/字符串信封、MCP isError、32MiB与+1字节、共享预算413、append稳定、同长恢复mtime外层尾改写撤销、6类拒绝、Chromium桌面/移动控制台入口）、`native_envelopes_authority.py`（固定cut/子代理scope、cut外坏记录、冷/热撤销隔离）通过；native_spans/authority、native_streaming、history/tool/media/names/grok差分及全部13套既有Chromium回归通过（媒体仍28一致/21明确差异）。fmt、server/client全targets Clippy、Windows MSVC `cargo check --all-targets`通过，非跨平台实机。
- 测量（[记录](docs/native-input.md#batch-19-nested-envelope-measurement)）：普通1万条冷读p50相对第十八批-3%…+4%、追加-1%…+6%、峰值RSS不变，嵌套路径只在巨型工具字符串时进入。单层32MiB字符串化图片首窗口/冷GET/暖GET p50约680/637/228ms、冷GET后RSS约50MiB，相对第十八批结构化span约2.8×/1.2×/1.3×（候选发现+结构重扫各读一遍外层）；3MiB八层首窗口约683ms、内存仍约18MiB：回放工作随层数增长而驻留字节不增长是有意取舍。
- 本批未实现单消息>16图continuation和多字符串拼接巨型工具JSON；原Python仍clean、无付费CLI、无生产部署、无Windows/macOS实机。

### 第二十批：单消息媒体 continuation

- 实测基线：一条 Codex 用户消息含 17 张 `input_image` 即整个会话 501（`unsupported_history`）。`MAX_MEDIA` 16→256（`image_content.rs`/`providers.rs`/`tools.rs` 三处，超出仍明确失败）；native span 路径 256 张接受、256 span+1 inline 拒绝有单测。
- `media_projection`：`DISPLAY_LIMIT=16`，`project` 只投影每条消息前 16 张 typed 图（discover 文本图仍追加其后）；新增 `project_range` 走同一 `PreparedImage`/`register_prepared` 路径（native span/embedded/file 描述符、error 描述符、file ref 需 files scope），不做文本 discover。`Selected{index,event}` 携带非 status 事件绝对下标（含 delta/append 路径），`project_selected` 在带 MediaStore 时附加 `media_more{remaining,total,cursor}`；无 PageStore 的调用方 `cursor:null`；搜索/输入历史不带 media 字段。
- `pages.rs`：`PageStore` 改为 `enum Grant{Page,Media}` 共享 1024/10 分钟/最旧淘汰/32 hex token，`lookup`/`lookup_media` 各自 400/403/404/410，跨类型 token 404。`MediaGrant` 绑定 uid/agent/checkpoint/非 status 下标/消息 JSON SHA-1 身份/offset/total，并记录已签发的 `next` 使同页重读返回同一 token（不刷新 TTL）。`Budget::take` 只计 `min(len,16)` 张及其字节，删除 `media.len()>128` 的 too-large 原因。`ViewSnapshot::media_page`：checkpoint 失效/事件不在/身份或总数变化 409、范围非法 409、无法推进 413、响应 >8 MiB 413。
- API：`GET /api/messages/{uid}/media-page?cursor&agent`（`deny_unknown_fields`），与历史页共用 `history_page_http` 8 permit、8 MiB 检查与同样 4 个响应头；能力 `media_continuation:true`。
- legacy：`mediaContinuationEnabled`/`mediaMoreInfo`/`mediaMoreHtml`/`mediaGallery(items,more)`/`currentMediaPage`/`validateMediaPage`/`fetchMediaPage`（1 MiB 有界读）/`mediaPageFailure`/`loadMediaContinuation`，`document` 级委托点击并限定 `#msgs` 内；按钮 `.media-more[data-media-cursor]`，cursor 为 null 时 disabled 说明；失败 `.media-page-error`（含 HTTP 状态与服务端 error）+ 重试；视图切换/reset/cursor 不匹配丢弃结果；4 处 `mediaGallery` 调用传 `media_more`。Python 页面无 `media_more`，标记不变。
- 验收：workspace **860 项通过、6 项默认 ignored**（server lib 627，新增 `tests/media_continuation.rs` 4 项集成、pages 6 项 grant 单测、provider 17..=256/257 单测）；Node **57 项**（新套件 7 项）；`media_continuation_browser.py`（HTTP：16 图+media_more、Codex 16/32/40 与 Claude tool_result 16/20 分页、交替字节解码、403/400/404、append 后旧 grant 仍可读、同长恢复 mtime 改写 409、搜索不受影响；Chromium 桌面/移动：按序解码、live cursor 不变、分页后 SSE 追加、控制台可见、扣住 watch 后真实 409 的可见告警/重试且快照完整）连跑 3 次稳定；media_lazy/history_pages/media/native_spans/native_envelopes/history/search/legacy/tool_parity/media_parity 回归通过（媒体差分仍 28 一致/21 明确差异）。fmt、server/client Clippy `-D warnings`、Windows MSVC `cargo check --all-targets` 通过，非跨平台实机。
- Claude 用户内容块被 adapter 拆成逐图消息，永远不触发 continuation，Claude 单消息多图形态以 `tool_result` 验证。未做性能对照（不改普通历史路径）。未实现跨多字符串拼接巨型工具 JSON；原 Python 未改，无付费 CLI、生产部署或跨平台实机。

### 第二十一批：高级 Python 差分、三处读模型对齐与验证运行器

- 新 `tests/advanced_parity.py`（约 900 行，合成 fixture，≈1 秒）：Claude compact_boundary→二次压缩→`last-prompt` 回退/废弃分支隐藏、未应答 Esc sibling、local_command/bash 流/task-notification/queue/custom-title/中断标记、sidechain 子代理嵌套工具与问答、祖先缺失；Codex 三级 `history_base` 固定前缀 fork 各级追加与尾巴隔离、`turn_aborted` 与前缀剥离、重复遥测/compacted/重建上下文过滤、挂在中间 fork 的子代理及其子代理、孤儿 fork、行中 cut；三家问答/取消、isError/退出码、空输出、unicode 路径 Write/Edit/MultiEdit/apply_patch、1.5 MiB 工具输出、>512 KiB Write 参数、`local_shell_call`/`custom_tool_call`；Grok summary+chat（tool_calls 两种形状、孤儿 tool_call_id、`<user_query>`/`<image_files>` 信封、协议注入、system 行）与 summary-only。列表元数据（sid/title/cwd/branch/created/updated/agent_items、root_sid/fork_depth）逐项对照。
- 差分暴露并修正三处 Rust 读模型偏差（`providers.rs`）：Grok user 信封改为与 `_GROK_USER_QUERY` 等价的整体、大小写不敏感、可重复 `<image_files>` 前缀匹配，正文只去恰好一个首尾换行（原实现把整段协议 XML 当用户原话显示、且 trim 掉用户缩进）；Claude/Grok `tool_result` 的空文本块在 join 前过滤（原多出空行）；Codex `web_search_call`/`tool_search_call` 无参数渲染 `{}`、JSON 字符串参数按 2 空格重排（与 `_pretty_json(arguments or {})` 一致）。
- 结果 **19 PASS / 13 DELTA / 0 UNVERIFIED / 0 FAIL**。全部 DELTA 均为计划已声明的安全差异：祖先缺失/孤儿 fork/行中 cut 明确 501 而非静默截断、超预算 Write 的 `changes_unavailable_reason`、空 tool_result 不伪造 `[图片]`、Grok 记录 `ts` 保留、Codex/Grok 文本嗅探退出码为额外 `exit_code` 字段（`error` 标志两边一致）。修正了差异表中"Grok summary-only 暂不列表"的过期描述。
- 新 `tests/run_validation.py`（grok-4.6 headless 产出，人工审阅）：声明式串行验证运行器，覆盖 cargo test/fmt/clippy/MSVC check/release 构建、运行时发现的 Node 合同与 Python 套件（parity 自动带 `--python-source`，`--browser`/`--binary` 按 argparse 探测，lifecycle `--native-binding` 变体），`--only/--skip/--tags/--list/--log-dir/--timeout-scale/--keep-going`，每套独立超时与日志，失败打印尾行，退出码。`--list` 解析出 34 套。
- 验收（隔离 worktree、独立 target）：`cargo test -p sessiondock` 736 项全绿（lib 627）、tool/history/grok/media/names 差分与 history/search Chromium 通过；主工作区其余模块当时有其他批次的未完成改动，故本批以 HEAD+本批改动单独验证。未改原 Python，无付费 CLI。
- 后续补充（grok-4.6 headless 8 路并行产出，人工审阅）：`tests/bench_summary.py`（基准 JSONL → 对照表，复算与第十九批手工表一致）、`tests/plan_status.py`（§6 勾选统计：38/60）、`docs/validation.md`（35 套件表）、`tests/check_docs_links.py`（89 链接 0 坏链）、`tests/legacy_asset_diff.py`、`tests/route_ledger.py`（实现 29/501 17）、`tests/rss_watch.py`、`run_validation.py --json/--rerun-failed/--dry-run`。见 [docs/delegation.md](docs/delegation.md)。

### 第二十二批：进程身份与运行/退出/未知三态

- 新 `runtime/process.rs`：`ProcessIdentity{pid,start_time}`，只读已验证 host 记录命名的子进程与 host 自身的 `/proc/<pid>/stat` 第 22 字段（启动 ticks），`/proc/stat btime` + `AT_CLKTCK` 换算 `started_at`；先校验 `/proc/<pid>` 属主等于本进程属主，首次观察要求启动时间 ≤ 记录 `created`+5s（否则 `started_after_record`），之后每次严格相等（`mismatch`）；ticks 比较不换算秒。不枚举 `/proc`、不发信号、不读他人进程；非 Linux 返回 `unsupported_platform`，`/proc` 缺失永不等于退出。
- `runtime/mod.rs`：身份记忆（实例键 = name+host_pid+pid+created+instance_id，≤1024 条，gone/最旧淘汰）、每次观察复验、按 UID 折叠三态 `sessions`、`ExitReceipt`（lifecycle `Exited`+`Confirmed` 绑定，同实例才算）、单飞共享缓存（TTL 2s，`?force=1` 绕过 TTL 仍串行）；`/api/term/list` 与 claim 仍用新鲜观察。优先级：当前实例证据 > 记忆 > 回执，不同实例的证据不覆盖当前实例。`exited/identity_gone`（本进程内验证过的身份在 host 不再应答/记录消失后从 `/proc` 消失）是对任务要求的补充：仅 Linux、仅限本进程生命周期内验证过的实例、Web 重启后回到 unknown；没有它真实 ptyhost 退出约 100ms 即清理记录，退出态几乎不可观测。
- `/api/live`：未配置 host 目录时完全不变；配置后 `enabled/known:true`、`partial:true` + 解释、`uids` 只列 `running`、`started_at` 取验证过的子进程启动时间，`managed` 增加 `process_identity`、`observed_at`、每 host 的 `process{status: verified|unverifiable|reaped|unchecked, child, host}`、`sessions[uid]{state,evidence|reason,host,instance_id,pid,started_at}`、`unlisted`、`cache{hit,age_ms,ttl_ms}`；全部加法兼容，legacy 只读 `uids/tmux_uids/started_at`。`capabilities.live` 保持 false（legacy 的 `live:true` 语义把列表当全集，会把未列出会话标成已停止）。`ptyhost-client` 新增只读 `HostClient::declared(name)`（不连接、不判活，仅为不可达 host 的 unknown 行命名声明的 UID）。
- 验收（隔离 worktree、独立 target，HEAD+本批）：workspace **874 项通过、7 项默认 ignored**（runtime 单测 +7：真实 `sleep` 子进程身份、PID 复用/记录早于进程、优先级折叠、单飞缓存；`ptyhost-client` +1；`tests/runtime_live.rs` 新增 2 项 + 1 项 ignored）；fmt、server/client Clippy `-D warnings`、MSVC `cargo check --all-targets`（非 Linux 路径可编译）、release 构建通过；opt-in 真实免费 shell：`runtime` 与 `runtime_live` 的 ignored 各 1 项通过（running(pid==记录 pid) → quit → exited/identity_gone）；新 `tests/live_browser.py`（Chromium：受控实例控制台可用 → quit 后灰色退出说明；两个无实例会话保持"运行状态未知"、活跃筛选拒绝隐藏；桌面 + 390px；legacy 未自行轮询 `/api/live`；原生 fixture 未改）及 managed_terminal/terminal_exit/lifecycle/terminal/legacy 五套 Chromium 回归通过。
- 合同见 [processes.md](docs/processes.md#process-identity-and-the-three-run-states-batch-22)。仍开放：Windows/macOS 实机（运行时恒为 `platform_unsupported`）；外部/未受控 CLI 不探测；不修改原 Python、无付费 CLI、无生产部署。

### 第二十三批：有界浏览器审计接收

- 新 `audit/`（`intake`/`limiter`/`writer`）与 `api/audit.rs`：`POST /api/audit/browser` 只在显式 `SESSIONDOCK_AUDIT_DIR`（已存在、绝对、无 `..`、祖先无符号链接、Unix 0700，且与 web/native/ptyhost/state/delivery/lifecycle/launcher/Codex index/file roots 双向不重叠）下启用；未配置保持 501 与 `capabilities.audit:false`，空串启动失败。
- 准入顺序 shutdown → 每客户端令牌桶（10/s、突发 40、≤64 客户端）→ 4 个解析槽 → 按 `Content-Length` 预留队列字节（封顶 1,024,000）→ 读 body → 解析 → 收缩预留 → `try_send`；队列（256 批且 4 MiB）满时**不读 body** 直接 `202 dropped:true`（旧页对非 2xx 会无限重发）。body 4 MiB 与 Python 相同（`dom.snapshot` 的 `content` 由 serde 跳过、从不分配）；每请求 ≤100 事件；每事件 `data` ≤8 KiB（超限替换为 truncated 标记）、字符串 ≤1024、深度 ≤8、数组 ≤256、键 ≤128。
- 只存结构化元数据：`content`/未知字段/请求头从不落盘；`authorization/cookie/api_key/token/password…` 键值 `<redacted>`，绝对路径形值 `<path>`，`uid` 只到 `source:<hash>`；服务端加 `seq/received_at/client/source`。独立 OS 线程写 `browser-YYYY-MM-DD.jsonl`，8 MiB 轮转、64 MiB 总保留（只删匹配模式的已关闭分段）、文件 0600；每批一次 `write_all`，空闲 1s/轮转/优雅退出时 `fdatasync`；退出 drain ≤2s，超出计 dropped。`/api/health` 增加 `audit{enabled, accepted/rejected/rate_limited/dropped/written/queued 计数, retained_bytes, write_errors}`。
- 验收：audit 单测 13 项（校验/轮转/保留/丢弃计数/`Gate` 故障注入）、`tests/audit_http.rs` 7 项（未配置 501、202+落盘、413/400/429、队列饱和快速 202 且计数、health）、新 `tests/audit_browser.py`（真实 legacy 页面经 fetch 与 pagehide beacon 投递 12 类事件、13 条有界记录、正文/`content`/语料路径均未落盘、能力关闭时零请求且目录不变、原生 fixture 不变）；随第二十四批在隔离 worktree 全量复验。合同见 [diagnostics.md](docs/diagnostics.md)。未做：Python 式全路由追踪与 SQLite/blob 存储、`/api/bug-report`、`debug_run`。

### 第二十四批：真实 CLI 创建/续接参数契约

- `lifecycle/model.rs`：`Launch{fixed|new_pending|new_assigned|resume{sid,uid}}`、`LaunchSpec.launch`、`Record.session_id`、`declared_sid/uid()`；规则写死：Claude 新建必须 `new_assigned`，Codex/Grok 新建必须 `new_pending`，已声明身份的回执不允许再带操作者 `binding`。`lifecycle/store`：ledger schema 3→4 严格迁移（旧记录补 `spec.launch={"kind":"fixed"}`、`session_id:null`；旧文件若已含新字段 fail closed）；`new_assigned` 在 Prepared 持久化前一次性铸造 UUID v4，replay/重启复用同一 `--session-id`。
- `lifecycle/launcher.rs`：schema 2 `profiles`（与 adapters 共用文件/权限规则，两表合计 ≥1 且各 ≤16，ID 跨表唯一版本化）；进配置的：可执行文件（存在/regular/可执行位/无 symlink，launch 前复验 stamp）、`args`、`new_args`/`resume_args`、`env`/`env_remove`、profile `cwd_roots`（须落在全局根内，请求 cwd 无 symlink 祖先 + canonicalize 后落入）。写死的：占位符只能是整参数 `{session_id}`/`{sid}`（嵌入/未知/重复拒绝），Claude `new_args` 恰一个 `{session_id}`、Codex/Grok 不含，`resume_args` 为空或恰一个 `{sid}`，SID 统一小写 UUID；launcher 仍 `env_clear`+默认 `TERM`，profile env 键白名单（`HOME/PATH/TERM/LANG/LC_*/XDG_*/ANTHROPIC_*/CLAUDE_*/CODEX_*/OPENAI_*/GROK_*/XAI_*/AGENTHUB_TEST_*`，不含 `NODE_OPTIONS`），`CLAUDE_CODE_SESSION_ID/CODEX_COMPANION_SESSION_ID/GROK_SESSION_ID/TMUX/AGENTHUB_SESSION` 永远拒绝。Python 的 `/bin/sh -c 'env -u …'`+登录 shell 包装不进 Rust：argv 数组直接交给 ptyhost `--`，`env -u` 语义由 `env_clear`+黑名单+ptyhost `STRIP_ENV` 三层保证，PATH/HOME 显式配置。
- 路由：`POST /api/term/create`（新增 `resume_uid`；回执 `launch_kind/declared_sid/declared_uid`）、`POST /api/term/takeover`（`uid,request_id`，`action: started|reused`；`force:true` 501）、`GET /api/term/complete-dir`（严格在全局 cwd 根内、不跟随不列出 symlink、≤50/默认 24）、`POST /api/term/backend`（只认 `ptyhost`/`host`，tmux 明确 400）；`/api/term/list` 增 `resume_sources`、`backends`；`bind` 对已声明身份 409 `launch_identity_declared`；能力 `terminal_takeover/terminal_complete_dir/terminal_backend`。续接经 `native_catalog().verified_scope()` 解析完整 SID（拒绝客户端 SID/路径），`--meta` 带 `sid+uid`，实例直接以该 uid 的 session 行出现并走 `guarded_v1` 原生 claim；同一原生身份只允许一个受管实例（Running 复用，Uncertain 未取消 409 `launch_conflict`）。Grok 续接可配置但目录尚无 Grok verified scope → 409。
- legacy（能力门控最小改动）：`complete-dir` 仅在 `terminal_complete_dir` 下启用；rust 模式 takeover 带 `request_id`；已声明身份的 pending 文案与禁用 bind 弹窗；pending 页在目录关联后按 host name + instance nonce 跟随到真实会话（非 cwd/时间/文件名）；未关联会话仅在 `terminal_takeover` 且该来源有唯一可续接 profile 时允许接管；侧栏对已声明 SID 的 pending 行去重；`legacy_contract.mjs` 更新钉住的 rust guard 文案。
- 验收：launcher 单测 +3、store 单测 +2、`tests/lifecycle_cli_http.rs`（真实隔离 ptyhost + 假 CLI，经 launch-guarded capture 断言精确命令行/环境/cwd、meta、复用、bind 409、各类拒绝、complete-dir、backend、精确 kill 到 exited；ptyhost 未构建时自跳过）、新 `tests/lifecycle_cli_browser.py`（Chromium 桌面+390px：Claude profile 新建后控制台显示假 CLI 回显 argv，`/api/term/list` 出现 uid 行并跟随；续接现有合成 Codex 会话）；lifecycle/native-binding/terminal_exit/managed_terminal/legacy 回归与 Node 37 项通过。合并树全量：见第二十三批同一轮验收。仍 501：`session/stop`（Python 依赖外部进程树探测与 Ctrl-D/TERM/KILL 升级，Rust 无外部探测，受管实例已有精确 `term/kill`）、`takeover force`、rename。未运行真实 claude/codex/grok。合同见 [lifecycle-launcher.md](docs/lifecycle-launcher.md#cli-profiles-batch-24-schema-2) 与 [lifecycle-http.md](docs/lifecycle-http.md#launch-identity-kinds-batch-24)。

### 第二十五批：原始终端输入 `term/send`/`term/scroll`

- `terminal/input.rs`：命名键 → host 键名/字节（enter/escape/tab/backspace/delete/insert/space/home/end/pageup/pagedown/方向键/F1–F12/`ctrl-x`，只认精确名与小写别名，其他 400 且绝不按字面输入）、字节计数、按实例滑动窗口限频；`ownership.rs` 新增 `authorize_input`（校验当前租约不消费）与 `NoLease`(403)/`Revoked`(409)；`service.rs` `send_input` 走操作 permit + 同名 gate + 租约校验 + 限频后经 `ControlOp::Send/Keys` 发送（`guarded_v1`/launch/raw 三类目标，raw 重探：launch 身份 409、退出 410）；host 无 ack → 504 `terminal_input_ambiguous`，绝不自动重试。
- `POST /api/term/send`（`deny_unknown_fields`，`data` 与 `keys` 二选一，`data` 复刻 legacy 的 `_build` 陈旧闸 409 `stale_build`；成功 `{ok,bytes,acknowledged:true,processed:"unknown"}`）；限额 16 KiB 解码字节、≤256 键、每实例每秒 16 次、128 KiB body、2 s host 超时。`POST /api/term/scroll` → `{pos:0,scrollback:"browser"}`：Python 的 ptyhost 后端 `scroll()` 本就返回 0、`leave_copy_mode()` 为空操作，host 协议无滚动视图状态，故不做 host I/O。能力 `terminal_input`（终端传输配置时为 true）；`outbox` 仍 false，可靠发送不在本批。
- legacy（能力门控）：`term.js` 在 claim 时保存 `inputLease`，两条原有 HTTP 路径（滚动请求挂起时的 `onData` → `{data}`，移动端键位栏 `sendToSession(null, keys)` → `{keys}`）带上租约与身份；Rust 模式回车提交文本提示"尚未提供可靠发送"；Python 页面 body 不变。
- 验收（隔离 worktree，HEAD+本批）：见本批提交说明；`tests/terminal_input.rs` 5 项（真实隔离 ptyhost 免费 shell：文本+回车回显、无租约/撤销后/退出后拒绝、大小与限频）、input 单测 5 项、`tests/terminal_input_browser.py`（桌面 HTTP `data`、390px 键位栏 `Tab`/`Up`、精确租约 body、退出后不可 claim/input、composer 隐藏、fixture 不变）与 managed_terminal/terminal_exit/terminal/legacy 回归、Node 37 项。合同见 [terminal-input.md](docs/terminal-input.md)。未做：可靠发送、Escape 的活动态副作用、tmux copy-mode。

### 第二十六批：会话回收站

- 新 `trash/`（`manifest.rs`、`plan.rs`）与 `api/trash.rs`：`SESSIONDOCK_TRASH_DIR` 与 audit 同样的私有目录校验并与所有其他路径双向不重叠；未配置 5 条路由保持 501、能力 `trash:false`。
- 文件集只来自冻结库存行（Claude 主 JSONL + `agent_items` sidecar + `.meta.json`；Codex rollout + owned 子代理 rollout；Grok `summary.json` + `chat_history.jsonl`），计划时抓 size/mtime/dev:ino，rename 前复验（不符 409 `changed_since_inventory` 并回滚），不跟随符号链接，只用同文件系统 rename（EXDEV → 409 `cross_filesystem`）。保护集合每请求从冻结快照算一次（`history_base.thread_id`/`forked_from_id`），批量先子后父仍拒绝。判活对同一冻结 catalog 做**新鲜**观察（不走 `/api/live` 缓存）：running 永远拒绝（force 无效），exited 放行，unknown（含无 host 目录的 `no_runtime`）需 `force:true`，文案写明 unknown 不是退出证据。
- 路由：`DELETE /api/session/{uid}[?force=1]`、`POST /api/sessions/delete`（≤200，始终 200 部分成功）、`GET /api/trash?limit≤200&cursor`（损坏清单列为 `corrupt` 只可清除）、`POST /api/trash/restore`（任一原路径存在 409 `restore_conflict`，逐文件 rename 失败回滚）、`POST /api/trash/purge`（`id|ids|all|days`，每次 ≤200 并报 `remaining`，从不触碰原生目录）。列表立即排除已删会话。legacy：`trashCapable()`、`run_state_unknown` → 确认 → force 重试、回收站视图 `?limit=200` 与截断提示；能力缺失行为不变。
- 与 Python 差异：不整体移动 Grok 目录、purge 不删"孤儿 agents 目录"、不推断无清单旧条目、不用名称前缀判活；`/api/session/{uid}` 其他方法 405。
- 验收：trash 单测 8 项、`tests/trash_http.rs` 6 项（未配置 501、三家 delete/list/restore/purge、fork parent 拒绝、unknown 需 force、恢复冲突 409、stamp 变化 409、批量部分成功、列表排除）、`tests/trash_browser.py`（桌面+390px：删除 → force 确认 → 回收站 → 恢复、可见拒绝原因）、legacy 回归、Node 37。合同见 [trash.md](docs/trash.md)。

### 第二十七批：持久化时间线 pin

- `metadata/model.rs` `TimelinePin` 增加可选 `target`/`pinned_at`（旧文档兼容），`with_timeline_pin`（同 tip/target/stale_end 重复为 no-op）、`without_timeline_pin`；`metadata/mod.rs` `set/clear_timeline_pin` 走既有原子 update。`providers/claude.rs` 暴露 `Lineage.parents`，新增纯函数 `apply_pin`（pin → `declared_tip`/`abandoned_after` + 退役原因）与 `resolve_pin_target`；`sessions/mod.rs` 的 `Parsed.pin`、`parse_candidate` 接收 pin 只改 parser 选项，pin 变化时热复用 AST 重投影，`semantic_anchor` 附加 pin 戳（pin/unpin/退役均 reset），`claude_rewind_target` 经 `CheckedNative::open_prefix` 字节核验冻结前缀后再读记录；子代理视图剔除 `timeline_pin`。
- `POST /api/session/rewind {uid,target|null,request_id?}`：tip = target 的父节点（与 Claude 原生"恢复到该输入之前"一致）；200 `{ok,uid,pinned,target,tip,stale_end,timeline_pin,native_rewind:false,metadata_revision}`；501 `metadata_disabled`、400 非法/非 Claude 主会话、404 target 不是该会话节点、409 不在当前活动时间线/首条之前无内容/`_build` 过期、413 >8 KiB。退役原因 `native_confirmed`/`native_continued`/`native_advanced`/`native_diverged`/`tip_missing`（规则同 Python `_claude_effective_tip`）。能力 `timeline_pin`（配置 state dir 时 true）。
- legacy（能力门控 3 处 hunk）："回到此处"按钮、`#timeline-pin-notice`（"已固定显示到所选输入之前，CLI 未回滚…"/退役原因 + 取消固定）、成功后 `syncSession` 取 reset；Python 页面无变化。
- 验收（隔离 worktree）：`cargo test -p sessiondock` 783 项通过（rewind_http 4、单测 4）、Clippy/MSVC/release/fmt 通过；`tests/rewind_browser.py`（桌面 pin→裁剪+说明、SSE 退役 `native_advanced`、reload 保持、390px pin/unpin、Web 重启持久、原生文件只追加）、metadata/history 浏览器回归、history/advanced 差分（0 FAIL）、Node 38。合同见 [metadata.md](docs/metadata.md#timeline-pins-batch-27)。范围外：真正的 CLI 回滚（双 Esc 原生流程）、`rewind_pending` 域钩子接线。

### 第二十八批：显式写根下的文件作业

- 新 `files/write.rs`（`WriteService`：路径授权、原子无覆盖原语、mkdir/new-file/rename/move/delete、根内回收目录、分块上传/发布）、`files/jobs.rs`（作业登记：scope 绑定、过期、并发上限、legacy `job` JSON）、`files/write_tests.rs`（8 项含 TOCTOU 注入点）、`tests/files_write.rs`（4 项 HTTP）、`tests/files_write_browser.py`。`boundary.rs` 提升若干 `pub(super)` 并新增 `ResolvedTarget::verify_identity()`（只比 inode 身份、先身份后类型）。
- 配置 `SESSIONDOCK_FILE_WRITE_ROOTS`：1..16 个既有目录，必须等于或位于某个读根内（读根绝不隐式变写根，写路径先经读侧 session 引用解析），与 web/native/state/host/delivery/lifecycle/launcher/audit 不相交、写根间不嵌套；未设置三条路由 501 `files_jobs_disabled`。能力 `files_jobs:true` + `files_write{actions,conflicts,delete:"trash",chunk_bytes,job_bytes,max_jobs,expiry_seconds,max_items}`。
- 路由：`POST /api/session/files/action`（upload 登记/mkdir/new-file/rename/move/delete/cancel，批次部分失败 200 + state failed；copy/compress/extract/bundle/restore/purge/retry 501，`conflict:replace` 400）、`POST /api/session/files/upload?uid&agent&ref&job&offset`（offset 必须等于已收字节，重发上一已接受分块幂等 200，其余 409；超声明 413；末块同步发布，sha256 不符 409；撞名 409；他 scope 404；过期 410）、`GET …&mode=jobs`、`POST /api/session/attachment`（配置 state dir 时写入 metadata `recorded:true`，否则 false；与 Python 原始字节流合约不同，legacy 调用在 outbox 门后不会触发）。
- 原子性：普通文件 `linkat` 入目标（存在即 EEXIST 绝不替换）→ 复核 inode → 删源名，硬链接不可用时 `O_EXCL` 复制 + 源 stamp 复核；目录 `mkdirat`/`O_EXCL`，目录 rename/move 先查目标再 `renameat`，非空目录/文件永不被替换（残余竞争仅限窗口内新出现的空目录，已在模块文档说明）；删除 `renameat` 进 `<root>/.agenthub-trash/<32hex>/` + manifest，跨设备 409，从不 unlink；暂存 `<root>/.agenthub-upload/`（0700）`<job>.part`（0600 `create_new`）；全部经保留目录句柄前后 `verify_identity()`。未用 `renameat2(RENAME_NOREPLACE)`（需裸 FFI 且仅 Linux）。
- 限额：8 并发（429）、256 MiB/作业（413）、4 MiB/分块、10 分钟过期（410）、单批 ≤256、名称 ≤255 字节、已完成保留 ≤64；可经 `Config.file_write_limits` 调整。legacy `files.js` 读 `files_write`：不支持的动作不出现、去掉"覆盖"、按 `chunk_bytes` 分块、删除说明为移入回收目录；无声明（Python）行为不变。
- 验收（私有 worktree，HEAD+本批）：`cargo test -p sessiondock` 787 通过（lib 664 含 write 8）、Clippy/MSVC/release 通过；`tests/files_write_browser.py`（能力门、真 UI 两次分块 XHR、任务面板、覆盖报错、重命名、390px 删除落入 `.agenthub-trash`、只读同级仍只读、字节不变）、files_browser、Node 37；安全用例：根内 symlink 指向外部 403、`..`/绝对名/相对路径 400、组件在 resolve 与 write 之间换成 symlink 409 `file_changed` 且外部无落盘、Unix 拒绝反斜杠/盘符 400、作业中根被替换 409 后续传成功、覆盖 409、分块重放 200/乱序 409、过期 410。合并树全量验收见本批提交说明。合同见 [files.md](docs/files.md#write-operations-under-explicit-write-roots-batch-28)。

### 第二十九批：受管实例停止 `session/stop`

- `lifecycle/service.rs`：`stop_session(uid, StopCandidate, request_id?)` 走同一协调器；`StopCandidate::Instance` 只接受来自一次新鲜运行时观察的 `guarded_v1` `BoundTarget`（声明身份或操作者绑定），按 name+instance(+launch) 找回执（无回执的 `--meta` 宿主也可停）。阶段：精确状态已退出 → `already_exited`；经 guarded `keys ["C-d"]` 发 EOF、轮询精确实例 ≤1.2 s，最多两次（Python `graceful_stop(timeout=2.4)` 只发两次 C-d，无 Ctrl-C）→ `graceful`；否则受管回执走既有 `cancel`（持久取消 → 撤销 launch 派生租约 → 宿主一次 HUP → ≤3 s 精确退出证据）、无回执宿主走 bound guard 的 `kill{force:false}` → `stopped`；限时内未见退出 → `uncertain`（取消标记保留、不重试、不复刻 Python 对 PID 的 TERM/KILL）。`request_id` 在内存记住最近 256 条已执行结果（Python 也不持久化），重放 `replayed:true`，同 ID 换 UID 409 `stop_request_conflict`；类型化拒绝不记忆。观察到退出后立即 `refresh` 持久化回执 Exited（新鲜 deadline，避免超时降级为 Uncertain）。
- `api/lifecycle.rs` `stop`：`{uid, request_id?}`（8 KiB、`deny_unknown_fields`、legacy 诊断字段）；冻结库存缺失 404 `session_missing`；`runtime::observe` 新鲜观察（不走 `/api/live` 缓存）找唯一声明该 UID 的 bound target；无实例且运行时/回执均无退出证据 → 501 `session_stop_unmanaged`（说明不探测外部 CLI）；宿主不可达/重复/身份不可核验 → 409 `run_state_unknown`（未发送任何指令）；成功 `200 {ok, uid, stage, stopped, name, instance_id, record_id, graceful_attempts, replayed, tmux:false, external_detection, explanation}`。不要求操作者确认标志（Python 的确认只在浏览器 `confirm`）；不要求也不撤销浏览器租约（同 Python，WS 随宿主退出标记结束，回执路径按 `term/kill` 撤销 launch 派生租约）。能力 `session_stop`（terminal + lifecycle 同时配置）；`api/mod.rs` 501 名单移除 `/session/stop`。
- legacy（能力门控）：`sessionStoppable(uid)` = `S.live` 含该 UID **或** `term/list` 列出该 UID 的受管实例（Rust 下 `live:false` 不轮询 `S.live`）；请求带 `request_id`；结果/拒绝写入 `#session-stop-notice`（role=status，无实例的"外部实例无法停止"解释、未知状态），确认停止后从 `S.live` 删除；侧栏长按菜单同源判定；Python 页面 body 不变。
- 验收（私有 worktree `wt-stop`、独立 target；主树因他人未提交的 `sessions/native_tail.rs` 暂不编译）：`cargo test -p sessiondock --locked` 822 通过 / 0 失败 / 5 ignored（新 `tests/session_stop.rs` 2 项：能力关闭 501；受管 Codex resume → `graceful` 且 `/api/live` 转 exited、回执 exited、同 `request_id` 重放、换 UID 409、再停 `already_exited`、未受管 501、400/404/未知字段；忽略 EOF 的假 Claude CLI → 两轮 C-d 后经受保护停止 `stopped`，耗时在 2.4 s 与 9 s 之间；`lifecycle_cli_http` 改为真实停止 resume 实例）；fmt、Clippy `-D warnings`、MSVC `cargo check --all-targets`、`cargo build --release --workspace` 通过；新 `tests/session_stop_browser.py`（桌面：接管 → 头部"停止会话" → confirm → `graceful` 提示、控制台灰色退出说明、动作回到"删除会话"、宿主记录清理、`/api/live` exited；无实例会话（`S.live` 陈旧项）→ 501 内联解释且无 alert；390px：新 resume → 侧栏长按菜单停止）；meta_capabilities/api_smoke/lifecycle_http/live_http/term_send HTTP 套件、lifecycle_browser（含 `--native-binding`）、terminal_exit/live/lifecycle_cli/managed_terminal/legacy/trash 浏览器回归、Node 38 项、`legacy_gating_check`（`/api/session/stop` 不再是 ungated 501）、`check_docs_links` 通过。合同见 [lifecycle-http.md](docs/lifecycle-http.md#stopping-a-managed-instance-batch-29)。仍 501：外部/未受管实例的 stop、`takeover force`、rename（都需要外部进程探测）；未运行真实 claude/codex/grok。

### 第三十批：Codex 原生确认适配器（库）

- 新 `delivery/codex_adapter.rs`（+ `codex_adapter/tests.rs` 12 项）：`observe(&ViewSnapshot, &Boundary, &Delivered) -> Observation { sequence, watch, outcome }`，把冻结视图里**固定提交边界之后**的 Codex 记录分类为 `delivery/codex.rs` 的证据类型。边界 `Boundary { confirmation: NativeCursor, sequence, delivered_ms }` 在 Enter 前从冻结视图取（`Boundary::capture`），永不移动；每次观察都重新用 `valid_checkpoint` 校验边界，重写/截断到边界之下 → `Uncertain(CheckpointMismatch)`。结果：`Possible(PossibleMatch)`（边界后恰好一条真实 `user` 记录、两端 trim 后等文、时间戳不早于 `delivered_ms` 或不可解析）→ `AckEvidence{ correlation: PossibleTextMatch, real_user_input: true, validated_confirmation = 边界, record: NativeAcceptance{ record_id:"codex-line:<start>-<end>", start, end, turn_id } }`，**状态机必然以 `UnprovenAcknowledgment` 拒绝**（单测真实驱动 Machine 到 `Uncertain` 后喂入验证）；`Absent{skipped_earlier}`（`watch` 给出可 `AdvanceWatch` 的当前游标）；`Uncertain(Ambiguous{candidates} | ForeignScope | MediaUnsupported | UnmatchableText | Unreadable{status})`。绝不产生 `NativeRequestId`/`OperationTurn`。常量取自 `send_queue.py`/`server.py`：`CONFIRM_TIMEOUT_MS=8000`、`REPLAY_INTERVAL_MS=8000`、`TRACK_WINDOW_MS=3_600_000`、`POLL_INTERVAL_MS=500`；`ReplayClock::plan/peek` → `Watch|Replay|Expired`。
- 相对 Python `observe` 只更严：匹配最多是 `PossibleTextMatch`、不删回执不确认（Python 直接 `rows.pop`）；边界后 ≥2 条同文 user 记录 → `Ambiguous`（Python 跳过更早的那条后仍会确认）；只接受投影出的 `user`（Python 还接受 `command`）；developer/协议注入/`goal.internal_context`/telemetry/status/inferred rename 都不算；有附件一律 `MediaUnsupported`；与 Python 一致的部分：仅 trim 两端，缺失/无效时间戳通过。
- `sessions/native_tail.rs`（只读 facade，无 I/O）：`ViewSnapshot::native_checkpoint()` 与 `native_tail(position, head, anchor)` 在 `SessionStore::snapshot` 已经过 `CheckedNative`+`RawIndex`+严格投影解析出的冻结视图上，用与 `/api/messages` 相同的 `valid_checkpoint` 校验边界，再返回边界之后每个事件的精确行范围 `[start,end)`（`RawIndex::record_start`，继承的 fork 前缀物理 end 为 0 永不进 tail，未完成的尾行不是记录）。`delivery` 侧因此不需要命名 `sessions` 的私有类型。
- 未接线：无 HTTP 路由、无调度器、无 PTY/屏幕解析、无 CLI 启动；不使任何回执 `acknowledged`。执行器接线与 Claude 版并行工作另记批次。
- 验收（隔离 worktree，HEAD+本批）：`cargo test --workspace --locked` 通过（新 `--lib delivery::codex_adapter` 12 项、`--test codex_ack` 1 项：私有假 Codex CLI shell 脚本（stdin 驱动，无 PTY）向真实 rollout 追加记录，经真实 `SessionStore` 读路径观察：边界后单条匹配 → `Possible` 且带物理行范围、重复输入 → `Ambiguous`、截断/重写 → `CheckpointMismatch`；`sessions::` 279 项无回归）、fmt、Clippy `-D warnings`、MSVC `cargo check --all-targets`、release 构建通过；`check_docs_links`、`check_agents_md`（`docs/README.md` 由 `tests/docs_index.py --write` 再生成）通过。合同见 [delivery-codex-ack.md](docs/delivery-codex-ack.md)。未运行真实 claude/codex/grok。

### 第三十一批：Claude 可靠发送执行器与 HTTP（假 CLI 验收 + 真实 CLI 自动跳过）

- `delivery/driver.rs`：`TerminalDriver` trait + `HostTerminalDriver`（在浏览器同一 ownership registry 上）——捕获屏幕+光标、按 Python bridge 方式识别 Claude composer（横线/`❯`/dim 建议/光标位置）、粘贴、发键、以服务端自持租约（页面 `agenthub-delivery-executor`）claim/release；请求带页面自身控制台租约（含 `launch_id`）时**借用**而非 claim，故持台页面从自己页面发送不与自身冲突、无租约的第二页面/服务端 claim 得到 `409 terminal_ownership`。截图健康位 `lag/dropped`：主机未暴露即 `None`＝未知（非空闲），暴露且 `lag>0` 则拒绝当作已知 composer、并在 `dropped` 变化时中止 prepare/Enter。11 项单测。
- `delivery/claude_adapter.rs`：把 `SessionStore` checked reader 读到的、固定确认边界之后的真实 `user` 记录转成 Claude Machine 的 `UserEvidence`（关联 `VerifiedEnter`：执行器亲自持租、验编辑器、按 Enter，故边界后首条同文人类输入即本次投递）；仅两端 trim，内部空白显著；边界失效（重写/截断/身份变）→ 不猜。6 项单测。不确定永远不确定；无屏幕文本确认、无 request-id（真实 TUI 不回显）。
- `delivery/executor.rs`：`DeliveryExecutor` 驱动引擎 dispatch batch——先持久化再注入、正文与 Enter 两次独立持久化、两者间崩溃恢复为 `Uncertain` 且绝不重注、有界 worker 准入、按会话（UID）串行、8s 复核固定确认边界并限频、一小时跟踪窗、关停取消。`ManagedResolver` 只从运行时目录**已验证原生关联**的唯一 guard-capable 受管实例解析目标（launch 出身经 lifecycle 绑定授权），绝不按名/cwd/时间/PID。重试仅限域允许的"未写入的本地等待项"；discard 退役回执但保留去重墓碑。13 项单测。`sessions::claude_native_inputs`（同 `/api/messages` 的 `valid_checkpoint` 边界校验，返回边界后人类 `user` 输入的精确行范围与 UUID）。
- HTTP `POST /api/session/send`、`/draft-status`、`/outbox/retry`、`/outbox/discard`：Python 兼容 body/响应/状态码（stale-build 闸、request_id 去重与重放=状态查询、草稿冲突需 consent token、retry 仅未写入项、discard 幂等 404）；附件字段接受但非空 `media` 明确 `400 delivery_media_unsupported`（尚无文件解析）。能力 `outbox` 仅在**发送账本目录 + 终端传输**同时配置时为 true（否则四条路由 `501 delivery_send_disabled`）；legacy composer 在其下启用，附带页面控制台租约。`GET /api/session/outbox` 不变。
- 假 CLI：`tests/fake_claude_cli.py`（`# run_validation: skip`），经 schema-2 launcher `--session-id {session_id}` 启动，渲染 Claude 式 composer 并为每行提交追加合成 `user` JSONL（`--delay`/`--busy-footer`/`--swallow N`）。
- 验收（共享树，release 为本批构建）：`cargo test -p sessiondock --locked` 787 项通过（新 driver 11、claude_adapter 6、executor 13 单测 + 集成 `delivery_send.rs` 3 项：先持久化后注入→原生记录→confirmed、request_id 重放、media 400、错名/未知会话、草稿 consent、慢 TUI 迟到确认、吞行保持 uncertain+拒绝重试+Web 重启不重注+discard 退役+再发成功、无账本/传输 501），Clippy `-D warnings` 无警告、MSVC `cargo check` 通过、release `cargo build --workspace --locked` 通过；`tests/send_browser.py`（桌面+390px：真实 legacy composer 发送、SSE 入历史、outbox 状态迁移、第二页面持台时的 ownership 错误）、`tests/outbox_suite.py`、`tests/delivery_http.rs`(8)、`tests/lifecycle_cli_browser.py`、`tests/terminal_input_browser.py`、`tests/managed_terminal_browser.py`、`tests/legacy_browser.py`、Node 合同 38 项、`api_smoke`/`python_route_gap`（四条发送路由 implemented）通过。
- 真实 CLI：`tests/send_claude_real.py`（自动以最便宜配置运行：`--model claude-haiku-4-5-20251001 --effort low`、JSONL 精确模型断言、隔离 `CLAUDE_CONFIG_DIR` 只读复用登录并预先接受目录信任、透传代理变量、`--permission-mode plan --tools ""`；先用一次 `-p --session-id` 建立会话再经 profile `--resume {sid}` 接管，然后 `session/send` 一条提示）。2026-09-12 实跑：登录探针通过（子代理原先报告的 `403 Request not allowed` 是清空环境时丢了 `HTTPS_PROXY`），但 `/api/sessions` 把真实 Claude Code 2.1.269 的会话列为 `supported:false`（`尚未支持的 Claude attachment 类型：environment`），套件按契约**打印原因跳过**。
- 这是读模型的真实缺口：Rust 对未知记录/附件/事件类型整会话判不支持，而 Python 忽略未知项；实测当前版本 Claude 会话含 `mode`/`permission-mode`/`atis-latch`/`bridge-session`/`file-history-delta`/`agent-name`/`cost-state` 记录与 `hook_success`/`environment`/`model`/`language`/`diagnostics` 等 attachment，Codex rollout 含 `token_usage_record`/`inter_agent_communication_metadata` 记录与 `event_msg item_completed`，均触发不支持。修复列为第三十三批。合同见 [delivery-executor.md](docs/delivery-executor.md)。共享文件仅最小 hunk：`api/mod.rs` 挂路由并从 501 名单移除四条、`state.rs` 加 `executor` 字段、`lib.rs` 装配并从异步上下文起 tracker、`service.rs` 加 `with_engine` 执行器钩子并将并发 open 上限 2→8（消除并行测试 open 争用；生产仅启动数次）、`engine.rs` 加只读回执访问、`sessions/{mod,native_input}.rs` 加 `claude_native_inputs`/`record_start`、`ownership.rs` 加 `into_server_token`、`terminal/service.rs` 加 `capture_screen`/`InputPayload::Paste`、`tests/{http.rs,api_smoke.py,history_parity.py}` 更新为新契约。未运行真实 claude/codex/grok（真实 CLI 按上跳过）。

### 第三十二批：Codex 可靠发送接线（同一执行器，假 CLI 验收 + 真实 CLI 自动跳过）

- `delivery/executor.rs` 泛化为按 `Provider{Claude,Codex}` 分派（来源取自冻结库存的 `native_scope`，Grok/子代理仍 501 `delivery_source_unsupported`/`delivery_agent_unsupported`）：准入、按 UID 串行、租约借用/自持、`overwrite_draft`、`prepare`（粘贴后独立观察到完整正文）、`enter`（帧复核）、tracker tick 全部共用；Codex 走 `codex::Command`：`Submit` 持久化→`InspectComposer`→截屏+冻结视图 `native_checkpoint()`（粘贴前取固定边界）→`DraftObserved`（未知 composer→`FailedBeforeWrite` 可显式重试、未批准草稿→`DraftConflict` 409、否则 `PrepareInFlight`）→`InjectPrepare`→`Prepared`（`EnterInFlight`+`enter_operation`）→`InjectEnter`→`EnterFinished`→`Uncertain`。Codex TUI 自持排队：不等空闲，后续发送各取自己的边界立即粘贴；粘贴与 Enter 两次独立持久化，之间崩溃恢复 `Uncertain` 不重注；Codex 折叠成占位符的长粘贴保持 `Uncertain` 且不按 Enter。`ManagedResolver` 不变（`terminal_unlinked` 文案改为按来源）。
- `delivery/driver.rs` 新增 `ComposerKind` + `inspect_for`/`inspect_codex`：移植 `codex_bridge.composer_state`——ANSI 剥离后把 U+2800–U+28FF 粒子字形空白化（Python `9b1c2fd`）、`Ready`+`Context N% used`/`model · cwd`/暗色 `esc again to edit previous message` 三种 footer 定位 composer 块、无 footer 时按光标锚定（含底行上方 1–2 空行的停靠布局）、块首 `›`/`»`、标记后非空白非粒子非暗字符→editing 否则 empty、菜单/审批提示→unknown、`lag>0` 永不 idle；`text` 取标记后的亮色非粒子文本按行拼接（空白不敏感比较）。6 项单测。
- `delivery/codex.rs` 加 `Receipt.dismissed`（serde default，仅允许已尝试的 `Uncertain` 行）与 `Command::Dismiss`：写前行同 `Discard`（墓碑保留）、`Uncertain` 行仅隐藏且不改状态/修订（一次性回调令牌不失效）、注入边界中 `WrongState`、已隐藏重放；`validate_snapshot` 复核。`Discard` 语义与既有测试不变。`engine.rs` 加 `codex_receipt`/`codex_receipts`（创建序）。
- 原生确认：tracker 对每条未隐藏的 `Uncertain` Codex 回执用 `codex_adapter::ReplayClock::plan`（`created_ms` 为 `delivered_ms`，同 Python `_tracked_since` 回退）决定 `Watch`（提交末尾未越过 watch 游标则跳过读取）/`Replay`（≥8 s 过期、每 8 s 一次、始终从固定边界读）/`Expired`（一小时停止轮询，状态不变不可重试）；在读工作线程上对新鲜 `SessionStore::snapshot` 调 `codex_adapter::observe`。`Possible` 且记录自带 `turn_id`（`payload.turn_id` 或 `internal_chat_message_metadata_passthrough.turn_id`，真实 rollout 2026-09-12 核实）→ `NativeAck{correlation: OperationTurn{enter_operation: 回执自身持久化的 Enter, turn_id}}` → `Acknowledged`，随后同轮 `task_complete` → `Completed`；无 `turn_id` → 保持 `Uncertain`（绝不借用后来的 `task_started`）；`Absent` → `AdvanceWatch`；`CheckpointMismatch` → 一次 `NativeReset` 注记；`Ambiguous`/其他 → 保持 `Uncertain`。理由同 Claude `VerifiedEnter`：执行器自持独占租约、验空编辑器（或只清已批准草稿）、粘贴前取物理边界、独立观察到完整正文、复核帧后自己按 Enter，边界后唯一同文人类输入即本次投递；适配器本身仍只产 `PossibleTextMatch`。
- HTTP：四条路由接受 Codex uid，body 同第三十一批；Codex 投影 `failed`+`attempts 1`+`error "发送结果待核对；禁止自动重试"`（legacy `codexNeedsInspection` → "终端写入待核对"、检查终端/移除，页面无需改动；Python 页面不变）；重放确认/隐藏行 `state:"confirmed"`；retry 仅 `FailedBeforeWrite`（或带 token 的 `DraftConflict`）重新检查（先探草稿），已尝试行 409 "消息已经写入终端或仍在确认，禁止重复发送"，未知 404 "待发送消息不存在"；discard 写前行退役、`Uncertain` 行隐藏（Python `9b1c2fd` 允许移除 confirming 回执，绝不重发）、边界中 409 "消息不存在或已经开始发送"、缺失/已移除 → 200 `{ok,uid,outbox}` 幂等（Python Codex 语义，区别于 Claude 的 404）。
- 假 CLI：`tests/fake_codex_cli.py`（`# run_validation: skip`）经 schema-2 profile `resume_args ["resume","{sid}"]` 续接语料 rollout，渲染 Codex 0.154 布局（RGB 粒子行、`›` + 暗色占位符 + 粒子、`model · cwd` footer、延迟时 `Working … esc to interrupt`），每行提交追加真实 rollout 形状（`turn_context{model,effort,turn_id}`、`task_started`、user `response_item`（passthrough `turn_id`）、`user_message`、可选 assistant 回复、`task_complete`）；`--delay`/`--swallow N`/`--no-turn-id`/`--duplicate`/`--reply`/`--busy-footer`，接受并忽略 `-c`/`--model` 等真实参数。
- 验收（共享树，含另一子代理进行中的第三十三批读模型改动）：`cargo test -p sessiondock --lib delivery --locked` 154 项通过（新 driver Codex 6、codex `Dismiss` 1、`executor::codex_tests` 9：先持久化→粘贴→Enter→`OperationTurn`→`Completed` 且边界为粘贴前文件末尾、request_id 重放/冲突、草稿 consent、ownership/错名/未知会话、吞行 uncertain+拒绝重试+重启不重注+dismiss 隐藏+墓碑重放+同文新请求各自确认、无 turn_id 与重复记录保持 uncertain、Enter 结果不明与未知 composer（写前失败、手动重试）、`lag` 永不 idle、跟踪窗、立即粘贴的后续发送；Claude executor 13 项与 driver 11 项不变）；新集成 `tests/delivery_send_codex.rs` 3 项通过（真实路由+隔离 ptyhost+launcher+账本：resume 续接、驱动把粒子/占位符 composer 读成 empty、发送→`failed`/`attempts 1`→确认且账本关联为 `OperationTurn` 且 `turn_id` 等于 rollout passthrough、重放、media 400、错名/未知会话、页面租约下的草稿 consent、慢 TUI（Working footer）迟到确认、吞行 uncertain+拒绝重试+Web 重启不重注+discard 隐藏且二次 200+再发确认、`--no-turn-id`/`--duplicate` 无关联、无账本/传输四路由 501）；`tests/send_codex_browser.py` 通过（桌面：控制台按钮 resume → 真实 composer 发送 → "终端写入待核对" 行被 SSE 原生记录替换、账本清空、第二页面两处 `terminal_ownership`；390px：服务端自持租约发送）。
- 真实 CLI：`tests/send_codex_real.py`（自动以最便宜配置运行：`gpt-5.6-luna` + `-c model_reasoning_effort="low"`、隔离 `CODEX_HOME` 只读复用 `auth.json` 并自写 `config.toml`（模型/低推理/只读沙箱/预信任临时目录）、透传代理变量、`codex exec` 登录探针建立会话再经 profile `resume {sid}` 接管、一条提示、`turn_context.payload.model` 精确断言、跑完 kill）。2026-09-12 实跑两次：第一次因账号临时用量限制按契约跳过并打印 CLI 原文；第二次（共享树含第三十三批读模型）PASS——探针 `codex exec` 回复 OK（2,602 tokens）、真实 Codex 0.154 TUI 经 `resume <sid>` 接管、`draft-status` 读真实粒子/占位符 composer 为 `empty`、`send` → `failed`/`attempts 1` → 从真实 rollout 的 user 记录确认（`OperationTurn`）、`turn_context.payload.model == gpt-5.6-luna`、assistant 回复进 `/api/messages`、会话列为 `supported:true` 并带 `migration_warnings`（`world_state`/`item_completed`/`token_usage_record` 跳过）、实例 kill、临时目录清空。读模型 `supported:false` 仍作为跳过原因。
- 落地修正：`delivery/service.rs` 的账本打开许可（`OPEN_WORKERS`=8）从 `try_acquire`→`Busy` 改为等待许可或关停（256 线程并行测试会同时打开超过 8 个账本，`last_handle_drop_releases_lock_after_worker_without_channel_cycle` 因此在全量扫描中偶发 `Busy`；生产启动只打开一两次，等待没有语义损失）。落地全量 `run_validation` 见提交说明。
- 仍不确定/未做：Codex 折叠占位符的长粘贴不按 Enter；同一边界后两条同文记录两条回执都保持 uncertain（Python 会确认后一条）；未知 composer 是写前失败需手动重试（Python 盲粘）；Codex 跟踪窗到期无注记；附件 400、stop/interrupt、rename/compact 确认、Grok 发送未做。合同见 [delivery-codex-executor.md](docs/delivery-codex-executor.md)。

### 第三十三批：读模型对当前 CLI 版本的兼容（未知类型跳过，Python 同）

- 起因：第三十一批真实 CLI 实跑发现 Rust 投影对任何未知记录类型整会话 `supported:false`（空消息），Python adapter 的 `if/elif` 链只是落空。当前 CLI 写入白名单外的类型——Claude Code 2.1.269 主会话：`mode`/`permission-mode`/`atis-latch`/`bridge-session`/`file-history-delta`/`agent-name`/`cost-state` 记录与 `hook_success`/`environment`/`model`/`language`/`deferred_tools_delta`/`agent_listing_delta`/`mcp_instructions_delta`/`skill_listing`/`auto_mode`/`instructions`/`session_context`/`date`/`remote_session_change`/`prompt_snapshot`/`deferred_tools_record`/`edited_text_file`/`diagnostics`/`hook_blocking_error`/`file`/`task_status` 20 种 attachment（`-p` 一次性会话：`queue-operation`→`user`→uuid/parentUuid 链接的 attachment 串→`atis-latch`→assistant，assistant 的 parentUuid 是最后一条 attachment）；Codex rollout：`token_usage_record`/`inter_agent_communication_metadata` 记录、`event_msg item_completed`、`response_item agent_message`。只看了形状与计数，未复制真实数据。
- 策略（`sessions/providers.rs` `Skipped` + `claude.rs`/`codex`/`grok`/`content`/`text_parts`）：未知的记录类型、Claude attachment 类型、Codex `event_msg`/`response_item` 类型、Grok 记录类型、可读记录内未知的非图片内容块类型（含 content 数组里的非字符串/对象元素）一律跳过，与 Python 相同；`supported` 保持 true，事件列表完整，列表/详情/分页/SSE/搜索/冻结 inventory/lifecycle 续接/发送确认都按可读会话处理。跳过种类按首次出现计数写入非致命 `migration_warnings`：`跳过未知的<provider> <类别>：<kind> ×<count>`（如 `跳过未知的Claude 记录类型：atis-latch ×264`），最多 32 种，其余合并成 `另有 N 条其他未知类型已跳过（超过 32 种，未逐一列出）`，kind 超 64 字符截断。硬失败不变且 `migration_warnings == [原因]`：坏 JSON/重复键、Claude 声明叶子缺失/祖先缺失/环、`content` 标量、文本块 `text` 非字符串、Codex `history_base` 非对象/重复 `session_meta`、无法解码的图片块/外链、100000 条与其他预算；Codex 工具输出信封（第十九批 `tools::output_text`）仍拒绝未知块。`queued_command` 人类提示照常渲染；`atis-latch` 等不生成消息。attachment 是图节点，祖先链经其回到 user，`turn_id`/已回答/last-prompt 逻辑不变（单测按此形状验证）。`sessions/mod.rs` 只在 `supported` 时保留 provider 警告；`history.rs` `mark_unsupported` 追加硬原因。
- 夹具：`tests/history_parity.py` 新增 `CLAUDE_ATTACHMENT_KINDS`（20 种）与 `claude_control_rows`/`claude_attachment_chain`/`claude_turn_tail_rows`/`codex_telemetry_rows`，语料加 `claude-cli-current`（一次性 + resume 第二轮）和 `codex-cli-current`；`advanced_parity.py` 的 `claude-events`（控制记录、user→reply 间 20+2 条 attachment、尾部记录）、`codex-l2`（遥测/item/agent_message）、`grok-chat`（`usage`×2/`checkpoint`）混入同类型并断言精确警告；`fixture_gen.py` 默认按观察位置写入（`--plain` 关闭，`messages()` 不变，`history_pages_walk` 不受影响）；`sessions_list_suite.py` 断言 `supported:true`+精确警告+详情可读。原以"未知类型"触发不支持的测试改用 Python 同样读不了的形状（`content: 42`、重复 `session_meta`）：`legacy_browser`/`search_suite`/`search_browser`/`tests/search.rs`/`tests/files.rs`/`sessions/history.rs`/`sessions/tests.rs`/`history_parity`；`send_claude_real.py` 把"读模型不支持"从 skip 改为断言失败并打印警告。
- 验收（隔离 worktree，HEAD+本批；共享树含另一子代理进行中的 delivery 文件）：`cargo test -p sessiondock --locked` 874 项通过（lib 730 项，`sessions::` 286 项；新增单测 7 项：providers `claude_current_cli_record_and_attachment_kinds_are_skipped_with_counted_warnings`/`claude_queued_prompt_still_renders_and_sidechain_attachments_stay_silent`/`codex_current_cli_record_event_and_item_kinds_are_skipped_with_warnings`/`grok_unknown_record_kinds_are_skipped_with_warnings`/`unknown_content_blocks_are_skipped_in_all_sources_but_invalid_media_still_fails`/`skipped_kind_warnings_are_bounded_to_32_kinds_plus_one_overflow_line`、media_tests `unknown_nonimage_blocks_are_skipped_without_leaking_their_payload`；改写 `unsupported_media_scalar_content_and_cycles_still_fail_closed`/`invalid_external_images_still_fail_closed`），fmt、Clippy `-D warnings`、MSVC `cargo check --all-targets`、release 构建通过。
- 验收（Python/Node，均 `--python-source ../agenthub`）：`history_parity`（Python 差分 0 DIFF，含两条新会话）、`advanced_parity` 22 PASS/14 DELTA/0 UNVERIFIED/0 FAIL（DELTA 与第二十一批相同，无新增）、`media_parity` 49 用例 28 精确+21 声明 DELTA、`names_parity`、`grok_parity`、`tool_parity` 通过；`sessions_list_suite` 8 项、`search_suite` 9 项、`api_smoke` 17 项、`history_pages_walk`、`history_browser`、`legacy_browser`、`lifecycle_cli_browser`、`search_browser`、Node 合同 68 项、`check_docs_links`、`check_agents_md`、`plan_lint` 通过。
- 真实 CLI（2026-09-12，`tests/send_claude_real.py`，`claude-haiku-4-5-20251001 --effort low`，两次实跑均 PASS）：一次性 `-p --session-id` 会话被列为 `supported:true`，`migration_warnings` 10 项（`environment`/`model`/`deferred_tools_delta`/`agent_listing_delta`/`skill_listing`/`session_context`/`date`/`remote_session_change` ×1、`prompt_snapshot` ×2、`atis-latch` ×3），profile `--resume {sid}` 接管并与 UID 关联，`session/send` 回执 persisted→injected→从真实 JSONL `user` 记录确认（第二次为 `ambiguous`→确认），assistant 回复经 `/api/messages` 可见，JSONL assistant 记录模型精确为 `claude-haiku-4-5-20251001`，实例已杀、临时目录已删。未运行真实 codex/grok。

### 第三十四批：读模型重做为惰性索引 + 按需视图（真实数据规模）

- 起因：2026-09-12 首次接真实读根（772 会话、3.4 GB）：列表 413（1000 会话上限）；放宽后冷启动单线程全量解析 23 s 且因活跃 CLI 追加被"整份作废"拒绝发布；消息对象常驻 O(总字节)。对照 Python 头/尾摘要冷 1.2 s、读全文 0.38 s、读头尾 0.04 s。用户裁定：推翻重来。设计见 [docs/read-model.md](docs/read-model.md)，旧设计归档于 [docs/superseded/frozen-inventory.md](docs/superseded/frozen-inventory.md)；契约与验收见 [docs/history-pages.md](docs/history-pages.md)、[docs/performance.md](docs/performance.md)、[docs/migration.md](docs/migration.md)。
- **WP-A 索引与摘要**（`sessions/index/`，Opus）：目录遍历 + `stat` + 每文件 96 KiB 头（Claude 40 / Codex 120 条）与 512 KiB 尾的有界摘要，按 `dev/ino/size/mtime_ns` 缓存，16 路并行，读取期间 stamp 变化重读 ≤ 3 次，500 ms TTL / `force=1` 重扫，`graph.rs` 从摘要推导子代理/fork 归属与运行时原生目录，`names.rs` 承接 Codex 名称索引；行与 Python `list_sessions` 逐字段一致（`tests/list_rows_parity.py` 18 会话 0 DIFF）。基准：2000 会话 / 1.05 GB 冷 327 ms、热（stat-only）33 ms。单测 39 项（index 13 + summary 19 + names 7，含 Python oracle 1 项 ignored）。
- **WP-B 视图与搜索**（`sessions/views/`，Opus）：`Parsed`/`parse_candidate`/`ViewSnapshot` 迁出 `mod.rs`，`ViewRequest` + `Dependencies` 回调按候选文件按需建视图，追加从已提交偏移续读（完整旧前缀 digest 校验 + AST 复用）、重写/截断/pin 变化重建、64 项 / 2 GiB LRU，`open_transient` 供搜索一次性投影不留驻，`search::execute` 流式扫描；预算按 read-model 表放宽（记录 64 MiB、文件 4 GiB、LF 2M、记录/事件 1M/2M、视图 1 GiB、索引 1 GiB、摘要 256/16 MiB）。基准：258 MB / 40k 行 Codex 文件全量解析 1.98 s、热打开 0.06 s。单测 11 项（views）。
- **WP-C 集成**（`sessions/mod.rs` 门面，Opus）：`list(force)` = `Index::refresh` 行 + 索引物理 `cursor {end, head}` + metadata 装饰（星标/fork 可见/`timeline_pin`）+ `signed_document` 重签名（行不变则保留 `sig`/`built_at`），列表时只借用缓存视图的 `anchor` 与 pin 退役态且不改 `sig`；`snapshot(uid, agent)` = 从索引取 owner/agent 候选（`IndexSnapshot::agent`）、元数据 pin、已发布行，经 `Views::open` 打开（`Dependencies` 由 `IndexSnapshot::thread` 按原生线程 id 解析，501/409 同图规则），视图比索引新（追加/重写/Grok 聊天新建）则强制重扫一次再复用视图，404/409/501/503 重扫重试一次；`search_pool`/`search_view` = 行 + `cached_current`/`open_transient`；`search_snapshot`/`native_catalog` = 索引行与 `IndexSnapshot::catalog()`（lifecycle 续接/接管、回收站、`/api/live` 不再解析文件）。删除：全量解析、整份 503、`ENTRY_LIMIT`/会话数/总字节上限、旧 `sessions/names.rs`、`history::Graph` 生产用途（仅 views 测试参照）、`#![allow(dead_code)]` 标记。契约变化 6 项（列表 cursor/anchor、`timeline_pin` 行、谱系错误移到打开、预算只在打开、Grok 非普通聊天文件为不支持、视图新于索引时的强制重扫）已写入 history-pages/migration 并更新 `sessions_list_suite`/`budget_boundaries_suite`/`advanced_parity`/`rewind_http`；legacy `syncSidebarUpdates` 只在两边都有 anchor 时比较。
- **WP-D 验收套件**（grok-4.6 headless 产出，人工审阅）：`inventory_scale_suite`（1500 会话 / 1 GiB）、`inventory_live_append_suite`（并发追加 30 次列表 0 失败）、`list_rows_parity`（0 DIFF）、`real_roots_bench`（操作者手动只读，`# run_validation: skip`）、`fixture_gen --head-tail-edges` 语料，4 套均进 `docs/validation.md`。
- 验收（Rust）：`cargo test --workspace --locked` 1061 项通过 / 0 失败 / 9 ignored（lib 789 项，`sessions::` 331 项），fmt、Clippy `-D warnings`、MSVC `cargo check --all-targets`、release 构建通过。
- 验收（真实读根，2026-09-12 本机，`tests/real_roots_bench.py --open 20`，只读，根 size/mtime 校验未变）：792 行（729 支持 / 63 不支持），冷列表 0.308 s（目标 ≤ 1 s）、热 p50 6 ms / p95 9 ms（≤ 100 ms）、最慢打开 0.298 s（14.3 MB Claude，≤ 1 s）、列表后 RSS 102.9 MB（≤ 300 MB）、打开 20 个后 319.2 MB（≤ 512 MB）、增量读 4 ms、打开后 `force=1` 76 ms 且 `sig` 不变，6 项目标全 PASS。最大文件拷贝到临时根（Claude 51.7 MB；Codex 228 MB 叶 + 51.8 MB + 38.9 MB 父链）：4 行 13 ms 列出，`window=1` 打开 3.07 s / 1.01 s / 0.81 s / 0.40 s（热 12–24 ms），228 MB 链 16,081 条消息；RSS 打开 318 MB 链后 625 MB、四个全开 1052 MB（≈2.5× 已打开字节，受 LRU/AST 缓存约束，不是真实根目标）；无父链的 fork 叶按设计 `supported:false` + 打开 501。
- 验收（全量扫描 `tests/run_validation.py --keep-going`，2026-09-12 本机）：77 套中 75 项首轮 PASS、2 项 FAIL 均为套件自身而非服务端——`inventory_scale_suite` 全量模式的填充器在第二次填充时 64 KiB 尾窗读不到完整 user/assistant 对（改为 4 MiB 尾窗），且原语料把 1 GiB 堆进最新 3 个文件使“打开 20 个”必然打开三个 340 MB 文件（改为两个 50 MiB 巨文件 + 其余摊到 128 个文件，巨文件单独 `window=1` 打开并报告 RSS）；`native_spans` 的 `arguments` 期望仍是旧的 501，而 WP-B 已把非媒体位置的巨型字符串按纯文本物化（改为断言 200、无 media、`role:tool`）。修正后两套单跑与 `--rerun-failed` 复跑均 PASS，合计 77 项 PASS。其中新套件：`inventory_scale_suite` 全量 1500 会话 / 1 GiB：冷 91 ms、热 p50 6 ms、列表后 RSS 62 MB、打开 20 个后 62 MB、两个 50 MiB 巨文件 `window=1` ≤ 0.475 s 且 RSS 311 MB、`sig` 稳定；`inventory_live_append_suite` 30 轮 88 次追加 0 列表失败；`list_rows_parity` 18 会话 0 DIFF；`advanced_parity` 22 PASS / 14 DELTA / 0 FAIL（`claude-missing-parent` 改为详情 501）；`send_claude_real`（haiku-4-5、low）与 `send_codex_real`（gpt-5.6-luna、low）均 PASS。
### 第三十五批：真实读根里 Python 能读而 Rust 拒绝的形状（63 行 → 0）

- 起因：第三十四批部署后真实读根 795 行中 63 行 `supported:false`。逐一对照 Python `adapters.py`：52 个 Codex 文件含重复 `session_meta`（2026-07/08 的旧式 fork 与子代理 rollout 把祖先的 meta 整条拷入，最多 28 条，`history_base` 为 null）；2 个 fork 的 `forked_from_id ≠ history_base.thread_id`（在父分叉点之前回退，`history_base` 指向物理持有前缀的文件）；6 个 Codex 子代理与 5 个 Claude sidecar 的父/主会话文件已不存在（Python 根本不列出）；1 个 Claude 文件第 205 行是 4 KB NUL 撕裂行（崩溃时零填充）。真正致命的原因都在 `migration_warnings` 最后一行，前面的"跳过未知类型"只是噪声（`tests/unsupported_rows_report.py` 按末行分组）。
- 规则（Python 为准，`sessions/index/summary/codex.rs`、`providers.rs`、`scope.rs`、`history.rs`、`index/graph.rs`、`index/names.rs`、`records.rs`、`providers/claude.rs`、`views/mod.rs`）：Codex 首条 `session_meta` 是唯一身份（`declared_ids` 只含它），后续条计入 `跳过重复的Codex session_meta ×N`；`history_base` null + `forked_from_id` = 自足的旧式 fork，不继承；`history_base.thread_id` 是物理前缀父（可与 `forked_from_id` 不同，一致性错误删除）；列表装饰（`root_sid`/`fork_depth`/`created`/`title`/`size`）沿 `forked_from_id` 逻辑链按 Python `finalize_sessions`（含其 `Σ min(end_byte_offset or 0, parent.size)` 的 size 公式），名称继承同链；父/主会话不在索引的孤儿子代理不再是顶层行（按 uid 打开仍 501；歧义/环/深度仍是可见的不支持行）；不是 JSON 的完整行跳过并计 `跳过无效的JSONL 记录 ×N`（字节留在物理索引，游标/LF 检查点不变；重复键行同样跳过，Python last-key-wins 为文档化 DELTA）；Claude 祖先链走到缺失/已访问 uuid 或声明叶子缺失即停，可达部分为时间线，三条非致命警告只进详情 `meta.migration_warnings`（行只带头尾可数的计数，详情把同类计数原位替换为精确值）。硬失败保留：`content` 标量、文本块非字符串、`history_base` 非对象、cut 不在行边界/越界、64 MiB 行与各预算。
- 执行：4 个 Opus 包（A Codex 身份/旧式 fork，B 图谱系/孤儿，C 撕裂行/谱系，D Python oracle 套件）各在隔离 worktree 提交后合并；10 个 grok-4.6 headless 单文件（7 套进扫描：`codex_legacy_fork_suite`、`codex_fork_rewind_suite`、`orphan_agents_suite`、`claude_torn_lines_suite`、`fork_rows_parity`、`fork_messages_parity`、`claude_lineage_parity`；3 个操作者工具：`unsupported_rows_report`、`state_dir_switch_check`、`cutover_drill`），全部 rc=0、人工审阅。`shadow_compare.py` 把两类已文档化媒体差异（Codex/Grok 工具正文里不插 `[图片]` 行；只含图片的信封输出 Rust 显示占位并附媒体）归为 DELTA；`meta_import.py` 校验容忍 serde_json 默认浮点解析的 1 ULP。
- 验收（Rust）：`cargo test -p sessiondock --locked --no-fail-fast` 952 通过 / 0 失败 / 7 ignored，fmt、Clippy `-D warnings` 通过；`delivery::service` 测试的 `Busy` 偶发改为等待准入。
- 验收（扫描，2026-09-12 本机）：84 套 76 首轮 PASS；8 项 FAIL 均已处置——`cargo_test` 是上述 `Busy` 偶发；`native_streaming`/`native_spans_authority`/`native_envelopes_authority`/`budget_boundaries_suite`/`claude_torn_lines_suite` 断言的是旧规则（坏行 501、谱系警告在行上、64 MiB 文案），改为新规则后单跑通过；`history_parity`/`advanced_parity` 对 Python 工作树失败是因为 Python 当天 `d16c5e1` 改了 Esc 中断分支语义——对 `79a21ab`（本批开工时的 Python HEAD）两套 0 FAIL（37 PASS / 14 DELTA），新语义列入第三十六批 D 包。真实 CLI 套件（haiku-4-5 low、gpt-5.6-luna low）PASS。
- 验收（真实读根，只读，release 构建）：`unsupported_rows_report` 803 行 / 803 支持 / **0 不支持**（claude 294、codex 394、grok 115；孤儿 11 行与 Python 一样不再列出）；`shadow_compare --python-source 79a21ab --sample 30 --all`：三来源 sid 集合与 Python 完全一致（`PASS inventory` ×3），消息 23 PASS / 70 DELTA（文档化）/ 5 DIFF——4 处是 Codex `custom_tool_call_output` 多块 `input_text` 信封（Python 取第一块的 `output`、Rust 取最后一块，两边都不完整；按序拼接列入下一批），1 处 `native roots were written` 是运行中的 CLI 在追加；`real_roots_bench --open 20` 六项目标全 PASS（冷列表 0.303 s、热 p50 7 ms / p95 8 ms、最慢打开 0.146 s、列表后 RSS 104 MB、打开 20 个后 161 MB）。
- 元数据迁移与部署：`tests/meta_import.py` 把 Python `session-meta.json` 转进新建的空 0700 目录 `state2`（131 行导入、0 跳过；2 条 pending rewind 按设计丢弃，2 条 `spawned_by` 字段本批无对应——第三十六批 AB 包接手），`--verify` 对真实根 122 行 ok（其余 9 行的会话已不在列表）；本批 release 部署到私有前缀、`SESSIONDOCK_STATE_DIR` 切到 `state2` 后重启，健康 200、冷列表 0.30 s、6 个星标行可见。`tests/cutover_drill.py`（隔离目录）11 步全 PASS：Rust 收 SIGTERM 与重启期间 ptyhost 实例存活、Python 数据目录字节与 mtime 未变、state 目录无 `session-meta.json`。
- 文档：AGENTS.md（范围改为现状描述 + 第三十五批规则 + 真实根验收门）、`docs/migration.md`、`read-model.md`、`error-codes.md`、`performance.md`、`validation.md`（10 个新条目）、`security-model.md`、`replacement-checklist.md`。

### 第三十六批：后端追平 Python `e5b023a`（进程判活、`spawned_by`、子代理运行态、`continued_in`、Esc 中断分支、多块信封）

- 起因：Python 仓库 2026-09-12 一天推进 34 个提交；用户裁定 parity 目标改为 `e5b023a`（含前端），并要求每批对真实读根只读实跑验收。冻结导出的 `e5b023a` 是全部 parity 套件的 oracle（`--python-source`）。
- **AB 进程判活与发起者**（`runtime/procscan.rs`、`runtime/spawn.rs`、`api/runtime.rs`、`metadata/*`、`docs/liveness.md`）：Linux 只读 `/proc` 扫描按 Python `live.py` 全部规则移植（cmdline sid 优先于继承 env、跨家继承不算、无活 CLI 祖先的孤儿辅助进程不算、`*.jsonl` fd 含 `events.jsonl`、bare `claude` 按 cwd+启动时间、Codex fork 链折叠、tmux/host 祖先 → `tmux_uids`、3 s TTL single-flight、`force=1`），显式 `SESSIONDOCK_PROC_SCAN=1` 开启，`SESSIONDOCK_PROC_ROOT` 供合成树测试，`SESSIONDOCK_GROK_ACTIVE` 显式；关闭时响应逐字节不变。`capabilities.live` 仅 Linux 且开启时为真。`spawned_by {source,sid}` 写一次进元数据（10 s 任务 + 每次 `/api/live`），行原样带出，`meta_import.py` 搬运。唯一刻意差异：fd 目标落在配置读根内也算（真机读根就是 Python 的三个 home）。launcher `DENIED_ENV` 补 `CODEX_THREAD_ID`/`CODEX_SESSION_ID`/`CLAUDE_PID`。
- **C 子代理运行态与 `continued_in`**（`index/agent_stops.rs`、`summary/*`、`index/graph.rs`）：Claude 只有 `end_turn` 收尾，父文件停止通知按已消费偏移增量扫描（sha1 去重、只认首见、`async_launched` 不算），只对有未收尾 sidecar 的 owner 扫且按 stamp 缓存；Codex 按最后一条回合边界 `event_msg`；`continued_in` 按 Python `finalize_sessions`（同源主会话、路径序最后者、自指丢弃）。真实根冷列表 0.34 s（停止扫描 12 个 owner ≈96 MB 只增 ~35 ms）。`continued-in` 是已知记录类型，不再计入未知警告。
- **D Esc 中断分支**（`providers/claude.rs`、`providers.rs`）：Python `d16c5e1` 的 `interrupt_nodes`/`offshoot`/`deferred_abort` 规则 1–6 逐条对齐；被回退的中断输入带 `interrupted:true`/`interrupt_reason`、自开 `turn_id`、后代可见、`aborted` 由原生中断记录给出。
- **G 多块工具信封**（`providers/tools.rs`、`records/native_records.rs`）：真实根普查 470 个 rollout 的块形态（`HE`×12809、`HEE`×1973、`HEEE`×492、`HSE`×878…）后，`custom_tool_call_output` 多段信封的 `output` 按序拼接、`exit_code` 取末个、`duration_s` 求和、图片仍登记媒体、巨型块仍走私有区段；Python 按拼接文本选一块（有时首块有时末块），记为文档化 DELTA。
- E：后端下拉标签 `默认宿主`。
- 验收（Rust）：各包在隔离 worktree 内 fmt/Clippy `-D warnings`/`cargo test` 0 失败后合并；合并后 `sessions::` 370 项通过。MSVC 交叉检查：unix 专用测试模块加 `cfg(all(test, unix))`。
- 验收（Python oracle `e5b023a`）：`history_parity`/`advanced_parity` 0 DIFF/0 FAIL（43 PASS / 27 DELTA，DELTA 全为文档化）；`list_rows_parity` 36 PASS（含 `agent_items[].active/created/updated`、`continued_in`）；新套件 `spawned_by_suite`、`agent_active_suite`、`continued_in_suite`、`claude_interrupt_parity`、`audit_events_suite`（grok-4.6 headless 产出，人工审阅：状态行归 `activity`、假 UUID、连接被关等于 413 三处修正）全部通过。
- 验收（真实读根，只读）：`/api/live` 对照 8710 上的 Python 服务 **41 = 41** 个活会话、`started_at` 41 条逐值相等、`spawned_by` 3 条一致（`tmux_uids` Python 41 / Rust 0：Python 的 CLI 跑在它自己的 ptyhost 下，Rust 只认自己的 host 目录——两套服务 host 目录分离的必然）；扫描 56–69 ms、`/api/live?force=1` 98–130 ms；子代理 `active` 对照 Python `list_sessions` 37 个 owner / 293 个 item **291 一致**（2 处：本会话正在写的 sidecar 7 s 竞态；一个指向他人会话文件的符号链接，索引按设计不跟随）；`shadow_compare --sample 30`：Claude 30 会话 **0 DIFF**（此前 2 会话差 5–7 条正是中断分支），Codex 30 会话 **0 DIFF**（4 处多块信封 → DELTA，Rust 正文均以 Python 正文开头且更长）。
- 操作者工具（grok-4.6 headless，人工审阅）：`live_shadow_compare`、`agent_active_real_check`、`spawn_real`（真实 CLI：haiku 派生 grok，自删）。

### 第三十七批：前端重同步到 Python `e5b023a`

- `legacy-web/` 逐文件三方合并（ours = Rust 门控，base = 冻结基线，theirs = Python static）：index.html/style.css/cli.js 自动合并，app.js 6 处 + term.js 1 处 + nodes.js 1 处冲突按预案解决（`flushBrowserAudit` 门控进 `auditPayload` 批量与 beacon、`liveStatusTitle` 合并未知态、`renderTimelinePinNotice` 与 `layoutSessionHead` 并存、`#dlive` 随 `sessionIconMarkup` 迁移、`mediaGallery` 进 `appendToolResult`、新建按钮 `allows('terminal_create')` + `layoutHeader()`、`STORAGE_PREFIX` 与 `Nodes.machines` 并存）；58 处能力门控与 Rust 专有增补逐 hunk 复核；唯一 Rust 专有样式修正 `body.mobile-detail #backend-notice{display:none}`（通知条会盖住 390px 的标题下拉）。`reference/legacy-web` 换成 `e5b023a` 快照（新冻结基线）。
- 后端字段缺席时按 Python 同样退化（无嵌套、无运行点、不跟续写）；套件用 Playwright `route` 在 HTTP 边界注入字段断言完整行为，字段落地后改为断言真实值。
- 验收：Node 合同 71 项；`legacy_browser`（桌面 + 375/390px）与 5 套移植的浏览器套件（`nest_tree`/`agent_menu`/`header_fold`/`side_drag`/`tool_group_fold`）及全部既有浏览器/静态套件 41 套 PASS；真实读根只读 Playwright：804 行、页面加载 0.81 s、开分层 0.35 s → 1036 行含 287 子代理行、最新 5 条会话打开 0.24–0.55 s、390px 无横向溢出、0 控制台错误 / 0 失败请求。
- 全量扫描（合并 AB/C/D/F 后，oracle `e5b023a`）：95 套 93 通过；2 项处置——MSVC 交叉检查（unix 测试模块加 cfg 门）、`agent_menu_browser`（改为断言真实 `active` 并在关闭 context 前 `unroute_all`）。合并 G/H1/H3 后再扫：97 套 95 通过；2 项处置——`cargo_test` 两个并行负载下的偶发（delivery 带 hook 的打开改走等待准入的助手；hub client 对刚释放端口接受 refused/closed 两种结果），`spawn_real` 改为操作者按批手跑（haiku 是否真的执行 grok 命令取决于模型，不作扫描门）。
- 部署与公开访问：本批 release + 新前端部署，`SESSIONDOCK_PROC_SCAN=1`。用户从 LAN 打开时得到 403 `local_only`——nginx 转发 `Host $http_host`，Rust 的 Host 门只认 loopback，**自第三十四批首次部署起公开地址就从未通过过**（此前只用 loopback curl 验证）。修正：`SESSIONDOCK_PUBLIC_HOSTS`（反代转发的精确公开 authority 白名单，只放行 Host/Origin 校验，认证仍是反代登录门），`security_suite` 覆盖接受/拒绝/端口/同源 POST；部署后 `Host: 203.0.113.177` 200、其它主机名 403。教训：部署验收必须经浏览器走公开地址。
- ptyhost 互通实测：Rust Web 对 **Python 仓库自带 `bin/ptyhost`** 起的实例能列出、浏览器 attach、resize、takeover，Web 重启后 PTY 仍在（`terminal_browser` 全 PASS）；`cutover_drill` 用同一二进制 11 步 PASS。切流时接管 Python 会话 = 把 `SESSIONDOCK_PTYHOST_DIR` 指向 Python 的 host 目录（记录/协议兼容；Rust 构建的 ptyhost 多出守卫/绑定应答，Python 旧二进制对这两类请求答错误，attach/list/kill 不受影响）。

### 第三十八批：Hub 节点身份/鉴权（H1）与注册表/监控/客户端（H2）

- **H1**（`config.rs`、`security.rs`、`api/node_auth.rs`、`api/mod.rs`、`lib.rs`、`main.rs`）：第二监听 `SESSIONDOCK_NODE_BIND`（具体接口，拒绝 `0.0.0.0`/`::`）只在 `SESSIONDOCK_NODE_TOKEN_FILE`、`SESSIONDOCK_NODE_ID_FILE`、`SESSIONDOCK_NODE_PEERS` 齐备时启用（缺一启动失败）；中间件要求对端 IP ∈ PEERS、`X-AgentHub-Node-Token` 常量时间相等、`X-AgentHub-Protocol: 1`，三者缺一 403（`node_peer_denied` / `node_auth_required`）；只服务 `/api`；loopback 监听对任何 hub 头仍 403 `hub_unsupported`。`/api/meta` 配置身份后 `protocol:1` + `node_id`（节点 id 文件首次启动 `O_EXCL` 0600 铸造），`/api/nodes` 返回本机节点行；`capabilities.hub` 仍 false。真实根只读：经节点监听带 token 列出 804 行，与 loopback 列表逐行相等且 `sig` 相同。
- **H2**（`hub/identity.rs`、`hub/registry.rs`、`hub/client.rs`、`tests/hub_fake_node.py`、`docs/hub.md`）：注册表 `hub-nodes.json`（0600 tmp+replace；`register` 校验 URL 形状/CIDR/节点 `/api/meta`，node_id 主键；display/enabled/order；strikes → `offline_since`；离线快照落盘并重启加载；条件 `sig` 请求；`recheck`/`nudge`；缓存 128 上界；搜索 NDJSON 流 60 s 空闲），手写 HTTP/1.1 客户端（无重定向、字面 IP、5/10/45 s、64 MiB）；刻意差异：只收 `http://`，缺省网络只 loopback（WireGuard 网段由 `SESSIONDOCK_HUB_NETWORKS` 显式配置）。集成测试对 Python 假节点 10 例。
- 验收：Rust 1037 通过 / 0 失败（合并 H1 后）；`node_auth_suite` 20 项、`check_config_suite` 51 例、`security_suite`、`meta_capabilities_suite` 通过。

### 第三十九批：Hub 命名空间与聚合（H3）

- `hub/namespace.rs`（`qualify`/`split`/`public_payload`/`decorate_rows`，对 Python `federation.public_payload` 51 例夹具 0 DIFF，`tests/hub_namespace_parity.py` 进程内 oracle）、`hub/aggregate.rs`（5 条聚合读路由、`?nodes=` 筛选 400、`sig`/`unchanged`、离线节点 errors + 过期行、term/list capabilities 合并、trash 合计、search NDJSON progress/matches/heartbeat/result、分拆写 delete/fork-visibility/purge_all）；集成测试对两台假节点 13 例。刻意差异：无法加前缀的 uid 原样保留（Python 整节点 `invalid_response`）、`sig` 不与 Python 逐字节相同。
- 验收：`cargo test -p sessiondock --locked` lib 878 项通过 / 0 失败，集成 `hub_namespace` 2 项、`hub_aggregate` 13 项、`hub_registry` 10 项通过；`hub_namespace_parity.py` 51 例 0 DIFF。H4 与 B1 见第四十、四十一批。

### 第四十批：Hub 代理、Hub HTTP 面与 `sessiondock-hub` 二进制（H4）

- `hub/proxy.rs`：`resolve()` 唯一节点（歧义 400 "操作必须明确指定同一台机器"）；JSON 经命名空间改写、SSE 按 `data:` 行改写（45 s 空闲断开）、WS 101 后原样双向拷贝、其它流式透传保留框架头、附件分块上传 ≤ 512 MiB、`PageNodes` LRU 512、`file_navigation` 303。`api/hub.rs`：loopback Host 门 + 拒绝 hub 头、`/api/meta {mode:hub,protocol:1,build,hostname}`、`/api/nodes {mode,nodes,machines}`、display/order + 审计、5 条聚合读 + NDJSON search、分拆写、`browser_audit` 分发。`hub_config.rs`：`SESSIONDOCK_HUB_BIND` 默认 `127.0.0.1:8742`（仅 loopback）、`SESSIONDOCK_HUB_NODES`（0600，必填）、`SESSIONDOCK_HUB_CACHE_DIR`、`SESSIONDOCK_HUB_NETWORKS` 默认 loopback。`bin/hub`：`register/remove/list/--check-config` 子命令。`assets.rs Mode::Hub`：`__AGENTHUB_MODE__=hub`、hostname、命名空间 `sessiondock.hub.<path>.`。依赖：hyper/hyper-util 升为直接依赖（原已是传递依赖，锁文件 +2 行，无新 crate）。
- 验收：Rust 918 通过 / 1 失败（client 端口复用偶发，主线已修）；`tests/hub_http.rs` 8 例、`hub_http_suite`、`hub_browser`（3 台假节点）通过。真实根只读：真实 Rust 节点（节点监听 loopback）+ 假节点经 hub 二进制注册，Playwright 列出 810 行（真实节点 809）、命名空间 `<source>:<nid>~…`、经代理打开最新会话、流式搜索、机器设置；根 mtime/size 未变；冷列表 2.8 s（经代理）、打开 1.1 s、搜索 0.3 s。部署样例 `docs/deploy-hub.md`（占位地址）。已知未做：`api/hub.rs::hub_gate` 只放行 loopback Host，未读 `public_hosts`（hub 若挂在 nginx 后会像节点当初那样 403）。

### 第四十一批：bug-report（B1）

- 配置：`SESSIONDOCK_BUG_REPORT_DIR`（0700、与其它私有目录不相交）与 `SESSIONDOCK_BUG_REPORT_REPO`（须在写根内）必须同设；launcher `bug_report_profiles{claude|codex|grok}` 指向同来源的 profile（id 须带 `-vN`），启动前按最便宜模型策略断言 argv（不符 501 `bug_report_model_policy`）；任一依赖缺失 501 `bug_report_disabled`、`capabilities.bug_report:false`。
- create：目录/文件 0700/0600；`events.jsonl` = 900 s 窗口内与 uid/page_id/trace_id/report_id 相关的审计行（`audit/query.rs`；`AuditService::record` 写服务端结构化事件）；`environment.json` 含 git 三命令 + Rust build；附件 ≤ 12 且须在 `<repo>/agenthub_attachments/`；提示词为本仓库版（不 push、不部署）；manifest 与 Python 同形。`worker::launch`：经 lifecycle 按 profile 创建；等 composer 空且稳定 0.6 s（Claude/Codex 用 driver composer 模型，Grok 用屏幕稳定探测）；paste → 验证 → Enter 每步持久化到 `manifest.injection`；Enter 重发 ≤ 4；`submitted` 只来自原生 user 记录，否则 `submitted_unconfirmed`/`failed`；审计 created/worker_started/probe/enter_retry/submitted/unconfirmed/failed/launch_failed；`/api/term/list` pending 行带 kind/title/report_id。`POST /api/session/attachment?uid=bug-report` 经写服务落到 `<repo>/agenthub_attachments/<id>/<name>`（`O_EXCL`、同内容复用、`stem__N`）。
- 验收：Rust 1064 通过 / 0 失败 / 7 跳过；`bug_report_http` 3、`bug_report_http_suite` 10 场景、`check_config_suite` 61（+10）、`meta_capabilities`、`route_ledger` implemented 46 / 501 0；真实 haiku worker 一次：`BUG-20260912-153139-adbbbd` submitted、模型 id 断言、实例 kill、目录/会话文件删除；只读复核 809 行无残留。DELTA：审计无 content；report id 用 UTC；worker 固定最便宜模型；`terminal.txt` 仅受管实例；附件 ≤ 32 MiB 且 `media:null`；cols/rows 仅记录。

### 第四十二批：追平 Python `16cc89c`（K 后端 + F2 前端）

- K：Grok 中途 `user_query` 信封（前后缀、`image_files`）剥离与 Python 正则一致；进程树遍历（`in_tmux`/hosted）遇中间 CLI 主进程即截断；`/api/live tmux_uids` 第四来源 `inherits_pane`（续写会话继承原会话 pane，≤ 8 跳，孙辈不继承）。验收：1088 通过 / 0 失败 / 7 跳过；`grok_parity`（含 in-flight）、`advanced_parity` 43/27/0、`live_http_suite` 15、`spawned_by_suite` 8；真实根 Grok 30 会话 0 DIFF + 5 条含信封会话 0 DIFF；`/api/live` 对照 Python 43 = 43、`started_at` 43/43；`tmux_uids` Python 42 / Rust 0（host 目录分离，含 1 条续写继承）。
- F2：`legacy-web` 与 Python `16cc89c` static 三方合并零冲突；分层图标、状态着色/角标、隐藏被续写父行（列表侧）、去分支项/子代理计数；`nest_tree`/`agent_menu` 去掉 HTTP 注入改用真实字段（`spawned_by` 种进 state 目录、`active` 来自未收尾 sidecar、`continued_in` 来自尾记录）。验收：Node 53、浏览器 31 套全 PASS；真实根：809 行、分层 2.3 s（1029 行）、续写父行隐藏、最新 5 会话 0.30–0.71 s、0 错误。
- 部署：K/F2/B1/H4 合并后的 release 部署（改名前最后一次以旧名部署）。

### 第四十三批：改名 sessiondock（零 agenthub-rs 痕迹）

- 用户裁定（2026-09-12 23:16）：项目名 **sessiondock**，不保留任何 agenthub-rs 痕迹（无别名、无 301）。crate/二进制 `agenthub-server` → `sessiondock`、`agenthub-hub` → `sessiondock-hub`（ptyhost 名字不变）；env 前缀 `AGENTHUB_RS_*` → `SESSIONDOCK_*`；localStorage 命名空间 `agenthub.rs.` → `sessiondock.`（hub `sessiondock.hub.<path>.`）；systemd 用户单元 `sessiondock.service`；`/srv/sessiondock/{bin,web,etc,state,delivery,lifecycle,host,audit,trash,bug-reports}`（账本重新初始化，state/audit 从旧目录搬运）；nginx `location ^~ /sessiondock/`（`/agenthub-rs/` 删除）；仓库目录 `/home/zj/Projects/sessiondock`。保留的协议标识（`X-AgentHub-*` 头、`__AGENTHUB_MODE__`、`agenthub-capabilities` meta、`AgentHubCapabilities`、`agenthubCli` 等）列在 `docs/glossary.md`"Names"，因为它们是与 Python 前端/hub 共享的线上契约。顺带：`SESSIONDOCK_PUBLIC_HOSTS` 解析移入 `config.rs`（`AppState.public_hosts`，非法/空值启动失败）。
- 验收：全量 sweep 101 套 100 PASS + `hub_browser` 随树标记同步后 PASS；部署后 `/api/meta` 200、公网 `https://203.0.113.177/sessiondock/` 经 nginx 鉴权可达；旧路径 404。

### 第四十四批：monkey 全功能对比与修复包（A–G）

- 起因：用户（2026-09-13 07:31）"有很多小问题，字体问题，功能问题。建议你自己发几个 monkey 去尝试各个功能，浏览页面。可对比 agenthub。"四只 Playwright monkey（visual / reader / watcher / operator）对真实数据只读对照 Python 8710，共 50 条发现（会话产出 `scratchpad/monkey/*.md`，成对截图 300+）。字体渲染两边逐像素一致，"字体问题"实为偏好命名空间不迁移与行内代码字体栈无 CJK 回退（Windows 落到宋体，Python 同款规则）。
- 主会话直修：`7bf362b` bug-report worker profile 不计入交互来源（阻断：新建/接管/报告问题全灰、创建 400）；`84d4649` hub 读 `SESSIONDOCK_PUBLIC_HOSTS`；`2137221` 搜索缓存指纹改 size/mtime（对整个二进制做 SHA-256 让 bind 晚 7.6 s）；`5f909e1` 行内代码/代码块/侧栏 cwd/搜索选项字体栈前置 `"AgentHub CJK Sans"`；`b9d3a08` 机器键盘排序不被周期刷新与过期保存盖掉（Python 同款 bug）。
- WP-C（`33be8ec`，读模型对齐）：debug-run 注册表（`<STATE_DIR>/debug-runs.json`，Python 同格式按 stat 重载，`?debug_run=` 视图，默认列表 398=398）、symlink 子代理（目标在读根内才跟随）、Grok 目录大小（125/125 相等）、`exit: N` 不作退出码、Codex `/rename` 合成 command 事件、hostname 取系统主机名 + `SESSIONDOCK_HOSTNAME`、列表行去 `migration_warnings`（962 KB → 389 KB）、`term/list.home`。
- WP-B（`f31aca6`，搜索文本缓存）：`search/{cache,service}.rs`，`SESSIONDOCK_SEARCH_CACHE_DIR/BYTES/WORKERS/WARMUP`；热搜索 0.07–0.13 s（Python 1–2.2 s，改前 36–44 s），结果逐条一致；搜索期间 RSS 峰值增量 ≤ 113 MB 且回落；启动 30 s 预热 403 条 / 18 MB；`malloc_trim`。
- WP-D（`c3a95f9`，媒体与文件）：读侧不拒绝多硬链接；发现规则对齐 Python `_RAW_PATH`，Python 静默的失败不投影占位；引用预算 250k/512 MiB、media 模式超限降级；media 索引按视图 revision 缓存（文件图 GET 0.85 s → 4 ms）；files.js 上传目标回退、越界导航回滚、根之上面包屑不可点；文件删除回收目录对齐 Python 私有目录（`<STATE_DIR>/file-trash`）。真实根 816 会话扫描 1279 可加载 / 1 占位。
- WP-E（`c1730fa`，生命周期/终端）：`lifecycle/autobind.rs` 进程证据自动绑定（ledger schema 5，`binding.method/evidence/bound_at`，审计 `lifecycle.autobind`）；`POST /api/term/discard` + 600 s 归档；Grok 整目录进出回收站；bug-report worker 注入走服务端输入；退出/撤销关面板；`T.ended` 门放行接管。真实 CLI：codex 5.0 s 自动绑定、grok 0 s、claude 1.2 s；bug-report haiku 7.7 s submitted。
- WP-G（`276054e`，题卡与审批）：`sessiondock claude-hook` 子命令 + `--write-bridge-settings`（不依赖 Python），`/api/messages` 顶层 `prompt` 与 `/api/watch` 的 `prompt`/`prompt_only` 包，Codex 审批屏幕解析（`bridge/codex.rs`，id 与 Python 一致）；`/api/term/send` 接受单字符键；`cli.js` AskUserQuestion 改按数字选项（Claude Code 2.1.270 菜单多两行，旧序列在 Python 侧同样失效）。实跑：haiku 题卡 2.5 s 出现、网页作答 0.5 s 消失；luna 审批 6.7 s。
- WP-A（`523ba80`+`db806c2`，并发与稳态）：读池 `SESSIONDOCK_READ_WORKERS`（默认 clamp(核数/2,8,32)）+ `SESSIONDOCK_ADMISSION_WAIT_MS` 有界等待（探测/分页/媒体/文件写响应池按比例派生），搜索不占读池；历史页 `SESSIONDOCK_HISTORY_PAGE_EVENTS`（默认 2000）+ 前端一键连续翻页（51 MB 会话 3.7 s vs Python 8.2 s）；前端瞬时失败（网络/abort/408/429/5xx 非 501）退避重试不停更，SSE 1.5→15 s 指数退避，`term.js` 瞬时失败不清 `T.enabled`；`capabilities.stage:"replacement"`、`read_only:false`，`/api/meta` 去 `migration`，无横幅；视图 LRU 16 项/128 MiB、AST 64 MiB、tokio 线程 ≤ 16（`SESSIONDOCK_ASYNC_WORKERS`）、`malloc_trim`。
- WP-F（`1ea0fe1`，改名收尾/偏好迁移/启动环境）：manifest/SW/标题/文件页/`files.js` 键改 SessionDock（`brand_names_check` 19 → 0）；`AgentHubCapabilities.stored()` 一次性从 `agenthub.*`（hub `agenthub.hub.<path>.*`）迁移偏好，只写新键；六个 launcher profile 改经 `~/.local/bin/with-zshrc`（launcher 零改动；haiku 实跑 `which python3` = p311；Rust 启动的 grok 以 `grok` 名进入 `/api/live`）。
- grok-4.6 headless 套件（人工审阅复跑）：term_sources_suite、search_bench_real、debug_runs_suite、symlink_agents_suite、grok_size_suite、codex_rename_suite、meta_hostname_check、reader_pool_suite、media_hardlink_suite、pending_discard_suite（`2df6b16`、`c68289e`、`a98cb1d`）。
- 验收：合并后 `cargo test -p sessiondock --locked` lib 982 通过 / 0 失败，集成全过，fmt/clippy 干净，Node 契约 49 + 14，`run_validation.py` 111 套 106 过（5 个失败逐一修复：ptyhost 测试二进制内嵌改名前路径需重建、`_parity` 命名约定、退出关面板断言、页面自中止请求、响应池按读者派生），`hub_browser` 修后 5/5。
- 基准（真实数据，生产实例；基线 `scratchpad/bench/BASELINE-20260913.md`）：搜索 36–44 s → 0.07–0.13 s（Python 1–2.2 s）；8 并发列表/详情 4×503 → 8/8 200（109 ms / 357 ms，Python 779 ms / 8.05 s）；RSS 重负载 2.45 GB → 开 385 MB 会话后 562 MB（Python 1.4 GB）；页面→列表 0.48 s → 0.38/0.23 s（Python 2.4/1.2 s）；打开会话端到端中位 462/528、p90 602/608、49 MB 792/631 ms（Python 458/496、1286/821、932/703）；`/api/live` 22 ms vs 376 ms；空闲页面 CPU 1.7% vs 29%。用户裁定的四条达标线（搜索 ≤ 3 s、并发 0×503、RSS ≤ Python、打开会话端到端 ≤ Python）全部达标。
- 部署（build `028d63fb66d1`）：`/srv/sessiondock/search-cache`（0700）+ `SESSIONDOCK_SEARCH_CACHE_DIR`；`state/debug-runs.json` 从 Python 搬入；`etc/claude-bridge-settings.json` 由 `--write-bridge-settings` 生成；launcher.json = with-zshrc 包装 + Claude `--settings` + Codex `--enable default_mode_request_user_input -c suppress_unstable_features_warning=true`。
- Hub 上线（2026-09-13 11:43，用户"部署到 <hub-domain> 接上所有机器"）：Lyra 节点开第二监听（WireGuard 地址 :8743，沿用 Python 的 node-id/token，UFW 只放中央 WG 对端）；中央 VPS 以用户服务跑 `sessiondock-hub`（loopback :8742，`SESSIONDOCK_HUB_NETWORKS` 含 <wg-subnet>，`SESSIONDOCK_PUBLIC_HOSTS=<hub-domain>,www.<hub-domain>`），注册表从 Python 的 `hub-nodes.json` 复制并把 Lyra 指向 Rust 节点，其余 Cygnus/Orion/Cetus 仍是 Python 节点（协议兼容）；nginx `/sessiondock/` 位置复用 `/agenthub/` 的登录鉴权。经 hub 聚合 449 行（Lyra 406 / Cetus 24 / Cygnus 19）0.26 s，经代理打开 Lyra 3852 条消息 0.63 s，搜索 1.2 s；Orion 离线是其 WireGuard 对端 3 天无握手（Python hub 同样离线）；Pavo 从未是节点（无 CLI、无 WG）。hub 页品牌改为 SessionDock（`ff29e00`）。旧 `/srv/agenthub-rs` 内容已删（空目录归 root，待 `sudo rmdir`）。
- 全节点铺开（2026-09-13 11:50–12:05，用户"全布置，你都有 ssh 权限。除了 windows 那边让我在桌面自己起"）：Cygnus（WG <cygnus-wg>）与 Pavo（新加 WG 对端 <pavo-wg>，装 wireguard-tools、`wg-quick@wg0`、中央持久化 peer）各部署 `/srv/sessiondock`（bin/web/etc/state/delivery/lifecycle/host/audit/trash/search-cache，用户单元 `sessiondock.service`，linger），launcher 六 profile 走 `with-zshrc`（with-zshrc 一并复制），`--write-bridge-settings`，Cygnus 沿用 Python node-id/token 并从 `session-meta.json` 导入 4 条元数据、UFW 只放中央、LAN 入口 `https://203.0.113.147/sessiondock/` 加入 nginx；Pavo 新铸 node-id、新 token、`sessiondock-hub register --name Pavo --color lime`。Hub 现在：Lyra 406 / Cetus 24 / Cygnus 19 / Pavo 2 = 451 行，Orion（WG 3 天无握手）仍是离线的 Python 节点条目，Cetus（Windows）保留 Python 节点由用户自行处理。部署脚本 `scratchpad/nodes/deploy_node.sh`（会话产出，未入库）。
- WP-W（`a6e52e2`，Windows 节点运行时；用户"windows 机器部署好 rust 版，我不想同时维护几套"）：`Launcher::command()` 抽公共组装，`spawn_detached()` 在 Windows 用 `DETACHED_PROCESS|CREATE_NEW_PROCESS_GROUP|CREATE_BREAKAWAY_FROM_JOB`、被拒退回不带 breakaway（Python `term_host._spawn` 同款；计划任务/终端窗口的 Job 不许 breakaway）；env allowlist 加 18 个 Windows 系统变量（`ProgramFiles(x86)` 精确放行）；`runtime/process.rs` Windows 进程身份 = pid + `GetProcessTimes` 创建时间、`GetExitCodeProcess` 判已退出、token SID 比对属主（`windows-sys 0.61` cfg(windows) 依赖）；lifecycle/delivery 账本非 Unix 不再 `DurabilityUnavailable`；`canonicalize` 的 `\\?\` 前缀折回盘符；ConPTY 下 ptyhost drain 600 ms 视为正常退出。Linux lib 983/0、MSVC check 0 警告；Cetus 实机（隔离实例）Claude/Codex/Grok 各一条：create 0.1 s → verified/running → attach 有画面 → composer 发送 → stop → 回收站，每源 20–23 s。已知限制：无 procscan/autobind（`live=false`，Codex/Grok 手动绑定），Windows 无 mode 位检查，"别处手工开同一会话再接管会有两个实例"。
- Cetus 部署（2026-09-13 12:50）：`C:\Users\<user>\sessiondock\{bin,web,etc,state,delivery,lifecycle,host,audit,trash,search-cache}`，launcher 三 profile 写 winget 真实目标路径（Links 是符号链接，提权令牌穿不过），沿用 Python node-id/token，6 条元数据导入，`--write-bridge-settings`，防火墙 8743 只放 <hub-wg>；**手动启动、无计划任务**（`start_sessiondock.py` 脱离 Job 起，`start-sessiondock.cmd`/`stop-sessiondock.cmd`），hub 注册表切到 `<cetus-wg>:8743`。Hub：Lyra 407 / Cetus 30 / Cygnus 19 / Pavo 2 = 458 行，Orion 待其 WG。
- 后续：Claude 中断分支消息计数（`claude:017851b4c1a4b53c` 114 条 interrupted user vs Python 15）；空闲 CPU 再降（活动文件增量投影、根变化增量图）；`lifecycle_http` 响应池仍即时 429；Python 侧同款问题（行内代码 CJK 字体、机器排序竞争、AskUserQuestion 键序）由用户决定是否回移。

### 下一批的具体入口

Python 仓库 2026-09-12 一天推进了 34 个提交（分层侧栏 `spawned_by`、子代理运行态、外部 CLI 判活、Esc 中断分支可见、顶栏折叠等）；用户裁定（2026-09-12 21:03）：parity 目标改为 Python **`e5b023a`**（含前端），多 host（Hub）与 bug-report 不再搁置，全部派发；每批验收都要对真实读根只读实跑（可用最便宜模型临时新建会话并自删）。差距清单与切包见会话产出 `GAP.md`（要点已并入下表）。

| 批 | 包 | 内容 | 状态 |
| --- | --- | --- | --- |
| 三十六 | AB `procscan`+`spawned_by` | Linux 只读 `/proc` 扫描（显式 `SESSIONDOCK_PROC_SCAN=1`，`SESSIONDOCK_PROC_ROOT` 供合成测试），`/api/live` 合并受管观察与扫描（`capabilities.live` 随之为真），`spawned_by` 写一次进元数据 + 10 s 任务，`meta_import` 搬运 | 已合入 |
| 三十六 | C `agent_items[].active` + `continued_in` | 子代理运行态（Claude `end_turn` 收尾 + 父文件停止通知增量扫描；Codex 末条 event_msg），Claude 尾部 `continued-in` → 同列表 uid；冷列表仍 ≤ 1 s | 已合入 |
| 三十六 | D Claude 中断分支 | Python `d16c5e1`：interrupt_nodes / offshoot / deferred_abort，`turn_id` 与 `interrupted` 语义 | 已合入 |
| 三十六 | E 小改 | 后端下拉标签"默认宿主"、DENIED_ENV 三个键 | 已合入 |
| 三十六 | grok ×8 | `live_shadow_compare`、`spawned_by_suite`、`spawn_real`、`agent_active_suite`、`agent_active_real_check`、`continued_in_suite`、`claude_interrupt_parity`、`audit_events_suite` | 已合入 |
| 三十七 | F 前端重同步 | `legacy-web/` 与 Python `e5b023a` static 三方合并（app.js 6 处冲突已定解法），58 处能力门控复核，`reference/legacy-web` 换新基线，5 套浏览器套件移植 | 已合入 |
| 三十八 | H2 注册表/监控/节点客户端 | `hub/registry.rs`、`hub/client.rs`、`hub/identity.rs`（类型），假节点 `tests/hub_fake_node.py` | 已合入 |
| 三十八 | H1 节点身份与鉴权 | 第二监听 `SESSIONDOCK_NODE_BIND` 仅在 token/id/peers 三者齐备时启用；常量时间 token、协议头、CIDR；loopback 监听仍 403 hub 头 | 已合入 |
| 三十九 | H3 命名空间与聚合 | `federation.public_payload` 逐字段、5 条聚合路由、search NDJSON、批量写分拆 | 已合入 |
| 四十 | H4 代理与 Hub 二进制 | `resolve()` 唯一节点、JSON/SSE/WS/二进制/附件转发、离线 503、`_build` 409、display/order 路由、`sessiondock-hub` 二进制 + 部署样例 | 已合入（第四十批） |
| 四十一 | B1 bug-report | `SESSIONDOCK_BUG_REPORT_DIR/REPO` + launcher `bug_report_profiles`（固定最便宜模型并启动前断言），复用 lifecycle 创建 + 发送执行器两步注入与 JSONL 确认，审计 15 分钟事件窗口；未配置 501 | 已合入（第四十一批） |
| 三十六 | G Codex 多块工具信封 | `custom_tool_call_output` 的多段信封按序拼接全部 `output`（Python 按拼接文本选一块，两边都不完整；真实根 4 处 DIFF → DELTA），见 `docs/native-input.md` | 已合入 |

有意不做：外部实例 stop/takeover/rename、Windows/macOS 实机。（历史批次记录里“未实现多字符串拼接巨型工具 JSON”是当时状态，第三十六批 G 已实现。）
