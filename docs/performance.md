# 读模型性能：真实读根与合成规模

读模型设计以 [read-model.md](read-model.md) 为准（惰性索引 + 按需视图）。
本文记录 2026-09-12 在开发机（Linux、Rust 1.98.1、release 构建、页缓存热）上的
实测：真实读根的操作者基准、合成规模套件、以及最大真实文件的拷贝验收。这不是
生产压测，也没有与 Python 做相同条件的对比；没有 Windows/macOS 或跨机器结果。

## 真实读根（操作者手动、只读）

```sh
python3 tests/real_roots_bench.py --claude-root ~/.claude/projects --codex-root ~/.codex/sessions \
  --grok-root ~/.grok/sessions --binary target/release/sessiondock --open 20 \
  --i-understand-this-reads-real-histories
```

独立 loopback 服务只配置三个读根（无 state/host/delivery 目录），结束后校验根下
每个文件的 size/mtime 未变。2026-09-12 本机：792 行（729 支持、63 不支持），
Claude 508 文件 / 795 MB，Codex 470 文件 / 2.5 GB，Grok 187 MB。

| 指标 | 目标 | 实测 | 结果 |
| --- | ---: | ---: | --- |
| `/api/sessions?force=1` 冷（首次目录遍历 + 全部头/尾摘要） | ≤ 1 s | 0.308 s | PASS |
| `/api/sessions` 热 p50 / p95（10 次，TTL 内） | ≤ 100 ms | 6 ms / 9 ms | PASS |
| 打开最新 20 个会话（≤ 100 MB 文件，`/api/messages/{uid}` 全文） | ≤ 1 s | 最慢 0.298 s（14.3 MB Claude，2002 条） | PASS |
| 列表后 RSS | ≤ 300 MB | 102.9 MB | PASS |
| 打开 20 个后 RSS | ≤ 512 MB | 319.2 MB | PASS |
| 追加增量读（最新会话，游标续读） | — | 4 ms | — |
| 打开后 `force=1` 重扫（stat-only） | — | 76 ms，`sig` 不变 | PASS |
| 根未被修改 | 必须 | unchanged | PASS |

最常见的行警告（均为非致命跳过计数）：`world_state`（Codex，304 行）、`atis-latch`
/`permission-mode`/`mode`（Claude，各 ≈186 行）、attachment `plan_mode`（149 行）。
63 个不支持的行是头/尾摘要能看见的硬错误（坏行、缺失父线程等），打开时给出同一原因。

## 全文搜索：搜索文本缓存（2026-09-13）

设计见 [read-model.md](read-model.md#搜索)。独立实例（只配三个真实读根 + scratch
下的 `SESSIONDOCK_SEARCH_CACHE_DIR`，816 行、3.6 GB），对照 Python 8710（只 GET），机器
负载 load average 120–160（其它 WP 并行验收），release 构建。`rss` 为
`/proc/<pid>/status` 的 VmRSS，每 100 ms 采样，"后"为搜索结束 1 s 后。

| 查询 | 改前（逐文件流式，单线程） | 冷缓存（8 槽，预热关） | 热缓存 | Python |
| --- | ---: | ---: | ---: | ---: |
| `ddp_guard` | 42.2 s | 25.3 s | 0.31 s | 2.63 s |
| `guard`（全词） | ≈ 43 s | — | 0.30 s | 1.03 s |
| `agenthub.*rust`（正则） | ≈ 41 s | — | 0.13 s | 0.79 s |
| `zzzz_no_such_term_qq`（无结果） | ≈ 44 s | — | 0.09 s | 0.97 s |
| `会话列表` | ≈ 40 s | — | 0.26 s | 1.38 s |
| `agenthub\s+rust`（可跨行正则，整体读入） | — | — | 0.10 s | 0.63 s |

结果集合、顺序、命中数与 Python 逐条一致（Rust 多出的 debug-run 行按 Python 的
uid 集合过滤）；唯一差异是 `claude:017851b4c1a4b53c` 的片段
上下文——Rust 投影把 114 条 `interrupted` 用户消息当正文（Python 15 条），改前
的二进制片段与改后逐字相同，属读模型投影差异，不是搜索的。

RSS 三点（前 / 峰 / 后，MB）：冷缓存搜索 105 / 823 / 73（峰 = 最大单次投影，
228 MB Codex 链；`malloc_trim` 在每次大文件解析后归还 arena）；热缓存 `ddp_guard`
74 / 187 / 76、`guard` 76 / 135 / 79、`agenthub.*rust` 79 / 89 / 100、无结果词
100 / 104 / 104——热搜索的峰值增量 ≤ 113 MB（活跃会话重解析），结束后回到搜索前
水平 ±30 MB。改前一次搜索让生产实例 RSS 从 1.23 GB 涨到 1.76 GB 且不回落。

预热：空缓存启动，8 线程后台预热 816 个候选 25.8 s（799 个解析、17 个借用
LRU 视图），预热期间 RSS 峰 732 MB、结束后 98 MB；启动后 3 s 发出的搜索与预热
共用解析槽，24.4 s 返回；预热完成后搜索 0.32 s。重启后缓存仍在，首次搜索即热。

缓存目录：816 个条目、18.0 MB（未压缩；Python 的 gzip `search-text` 823 个
11 MB）。读页缓存 18 MB 比解压快，且流式分块匹配不需要整体解压，故不压缩。

解析槽粒度是峰值内存与冷搜索耗时的取舍（`ParseSlots`，`SLOT_BYTES`）：
64 MiB/槽时冷搜索 10.0 s 但 RSS 峰 3.2 GB（8 个 arena 各保留其最大投影）；
16 MiB/槽 18.9 s / 3.1 GB（无 trim）；8 MiB/槽 + 逐大文件 `malloc_trim` 25.3 s /
0.82 GB。默认取后者：冷搜索只在空缓存或换二进制后的头半分钟出现，由预热承担。
真实读根 5597 个文件的 size/mtime 在全部验收前后逐一相同。

## 最大真实文件的拷贝验收（临时根，只读拷贝，用后删除）

把最大的 Claude 文件（51.7 MB，23,763 行）和最大的 Codex rollout（228 MB）连同
其 `history_base` 父链（51.8 MB + 38.9 MB 两级父前缀）拷入临时根，同一基准脚本
`--open 4`，另用 `?window=1`（legacy 的首屏请求）单独计时：

| 文件 | 列表 | 打开（全文） | 打开（`window=1` 冷 / 热） | 消息数 |
| --- | ---: | ---: | ---: | ---: |
| Codex 228 MB 叶 + 90 MB 继承前缀（逻辑 318.7 MB） | 4 行共 13 ms | 3.216 s | 3.074 s / 24 ms | 16,081 |
| Codex 90.4 MB（中间父） | 同上 | 0.903 s | 0.809 s / 20 ms | 4,061 |
| Claude 51.7 MB | 同上 | 0.998 s | 1.012 s / 12 ms | 7,515 |
| Codex 38.9 MB（根） | 同上 | 0.456 s | 0.402 s / 17 ms | 2,672 |

RSS：列表后 27.7 MB；打开 318 MB 链后 625 MB；四个都打开后 1052 MB（≈ 2.5× 已打开
字节，受 64 项 / 2 GiB 视图 LRU 与 1 GiB AST 缓存约束，列表不受影响）。≤ 512 MB 的
RSS 目标是对真实根"最新 20 个"的承诺，不是对任意 400 MB 已打开文件集的承诺；若需
更低常驻，`budgets::AST_CACHE_BYTES` / `VIEW_CACHE_BYTES` 是唯一要调的旋钮。
没有父链时，228 MB 的 fork 叶（`history_base` 指向未索引的线程）按设计列为
`supported:false`（"父线程不在已配置索引中"），打开 501（2.3 s 用于流式解析后拒绝）。

## 合成规模（`tests/inventory_scale_suite.py`，进入 `run_validation`）

fixture_gen 语料 1500 会话 / 1 GiB（三家来源，最后一对 user/assistant 填充到目标字节）：

| 指标 | 目标 | 实测 |
| --- | ---: | ---: |
| 冷列表（1500 行、1 GiB） | ≤ 2 s | 0.091 s |
| 热列表 p50 | ≤ 100 ms | 6 ms |
| 列表后 RSS | ≤ 300 MB | 62 MB |
| 打开 20 个（未填充的）会话后 RSS | ≤ 512 MB | 62 MB |
| 两个 50 MiB 巨文件 `window=1` 打开 | ≤ 2 s | 0.475 s（最慢）；之后 RSS 311 MB（仅报告） |
| 打开后 `force=1` 重扫 | ≤ 2 s | 0.042 s，`sig` 不变 |
| `sig` 跨打开与 `force=1` 稳定 | 必须 | 稳定 |

语料：fixture_gen 1500 会话（每会话 8 条），两个文件填充到 50 MiB，其余字节摊到
128 个文件（每个约 7 MB），使“打开 20 个”对应真实分布（真实根最新 20 个共约
45 MB），巨文件单独打开并报告常驻。

索引单元基准（`cargo test -p sessiondock --lib sessions::index::tests::benchmark -- --ignored --nocapture`，
2000 会话 / 1.05 GB，16 路）：冷 327 ms，热（stat-only）33 ms。视图基准：
258 MB / 40k 行 Codex 文件全量解析 1.98 s，热打开 0.06 s。

## 并发预算

所有池使用 tokio `Semaphore` 排队。等待中的请求取消后离开队列且不持有许可；
已开始的阻塞工作持有许可到结束，不因 HTTP 取消而提前释放。池关闭会唤醒等待者。

| 池 | 旧值 | 新默认（W = `SESSIONDOCK_READ_WORKERS` = `clamp(核数/2, 8, 32)`） | 503 代码 | 准入方式 |
| --- | ---: | ---: | --- | --- |
| 只读工作池（列表 / 详情 / 分页 / 文件 / 媒体 / 偏好写入 / 观察） | 4，`try_acquire` | W（本机 256 核 → 32） | `reader_busy`（仅关闭） | 等待许可 |
| 受控进程观察 `runtime_probes` | 2 | `max(W/2, 2)` | `runtime_busy`（仅关闭） | 等待许可 |
| 历史页 / 图片分页响应 `history_page_http` | 8 | `max(2W, 8)` | `history_page_busy`（仅关闭） | 等待许可 |
| 媒体响应 `media_http` | 8 | 同上 | `media_busy`（仅关闭） | 等待许可 |
| 文件写入响应 `file_write_http` | 8 | 同上 | `files_busy`（仅关闭） | 等待许可 |
| 生命周期响应 `lifecycle_http` | 8 | 同上 | `lifecycle_response_busy`（仅关闭） | 等待许可 |
| 搜索 `searches` | 2（另占 1 个读 worker） | `SESSIONDOCK_SEARCH_WORKERS`，**不再占读池** | `search_busy`（仅关闭） | 等待许可 |
| 媒体 / 文件作业 `media_jobs` / `file_jobs` | 2 / 2 | 不变 | | 2 s 等待 |

独立实例对真实根（803 会话）：16 并发 `/api/messages`（8 个 `window=1` + 8 个
`append=1`）全 200；一次 40 s 全文搜索期间 12 个 `/api/sessions`、`/api/live`、
`/api/term/list`、`/api/messages` 探针全 200；两个标签页（列表 + 活动会话详情）
观察 180 s 0 个非 2xx；断网 6 s 恢复后 SSE 在 ≤ 2 s 内重连，无横幅、不暂停
（`scratchpad/monkey/wpa/accept_concurrency.py`）。

## 常驻内存

同一台机器、同一批真实根（只读），`scratchpad/monkey/wpa/mem_bench.py` 按主会话
给出的顺序：列表 → p90（7.9 MB）→ 最大 Claude（49 MB）→ 最大 Codex（385 MB）→
热读 → 全文 → 一次无命中搜索 → 空闲 20 s → 依次打开最大的 5 个 → 空闲 20 s。
VmRSS，MB：

| 步骤 | 修改前（HEAD 7bf362b 同源构建） | 修改后（默认值） |
| --- | ---: | ---: |
| 冷启动（线程数） | 18（259） | 47（35 → 19） |
| 列表 | 111 | 108 |
| p90 7.9 MB | 148 | 92 |
| 最大 Claude 49 MB | 452 | 147 |
| 最大 Codex 385 MB（`window=1`） | 1138 | 325 |
| 385 MB 热读 | 1140 | 318 |
| 385 MB 全文（54 MB 响应） | 1146 | 317 |
| 一次无命中搜索之后 | 1708 | 324 |
| 空闲 20 s | 1541 | 365 |
| 依次打开最大 5 个（155 / 166 / 304 / 304 / 385 MB）之后 | 2686 | 464 |
| 再空闲 20 s | 2686 | 497 |

三个因素各占多少（同一脚本单独切换）：

- **线程数 / trim**：tokio 默认按核数开 256 个 worker，每个线程一个 glibc arena，
  释放的内存留在各自 arena 里；`SESSIONDOCK_ASYNC_WORKERS` 默认 `clamp(核数/8, 4, 16)`，
  并在新解析、淘汰、打开视图的响应（非 `append=1` 或 > 4 MiB）与每次搜索结束后
  `malloc_trim(0)`。仅这一项就让"385 MB 全文之后"从 1146 回到 ~300（解析临时对象和
  54 MB 响应副本真正归还），搜索后不再净增。`mallopt(M_ARENA_MAX, 2)` 试过并否决：
  十次 `/proc` 扫描的 futex 调用 2.6k → 564k（两个 arena 被并行索引读取和读 worker
  争抢），空闲 CPU 反而上升。
- **AST 缓存**：原 1 GiB 让 385 MB 文件解码后的整棵 `Value` 树驻留（+700 MB）；默认
  改为 64 MiB（`SESSIONDOCK_AST_CACHE_MB`），只有小于预算的活跃文件才为追加保留 AST。
- **视图 LRU**：原 64 项 / 2 GiB（按序列化字节记账）；默认 16 项 / 128 MiB
  （`SESSIONDOCK_CACHE_ENTRIES` / `SESSIONDOCK_VIEW_CACHE_MB`）。截图密集的 304 MB Codex
  会话一个视图 ≈ 230 MB 常驻（≤ 2 MiB 的内嵌图片 base64 驻留在视图里，媒体口径另计；
  解析临时对象归还之前曾把它显示成 +486 MB）。"最大 5 个"依次打开时 LRU
  按记账字节淘汰，终值 464 MB、空闲后 497 MB。

`tikv-jemallocator` 未试：需要新增外部依赖（联网取包）；`malloc_trim` 已让释放
真正回落，且只新增已在 lock 里的 `libc` 直接依赖。

## 空闲页面 CPU

一个无头页面停在活动会话 `claude:179009468904dece`（22 个子代理，文件持续追加）
40 s，独立实例（`scratchpad/monkey/wpa/idle_cpu_iso.py`）：

| | 修改前 | 修改后 |
| --- | ---: | ---: |
| 进程 CPU | 8.11 CPU-s / 41 s = 19.8 % 一核 | 8.67 CPU-s / 41 s = 21.2 %（见下文归因：不在本包范围的两项占八成） |
| 线程数 | 262 | 22 |
| 非 2xx | 4 × 503 | 0 |

每请求服务端 CPU（`cpu_probe.py`，ms）：

| 请求 | 修改前 | 修改后 | 说明 |
| --- | ---: | ---: | --- |
| `/api/sessions?sig=<当前>`（每标签 8 s 一次） | ≈19（整份文档克隆+装饰+序列化后才比对 sig） | 0 | sig 先比对，命中直接返回 `{"unchanged":true}` |
| `/api/sessions?force=1`（目录遍历，内容未变） | 68 | 27 | 遍历后 stamp 全同 → 直接复用快照，不重建行 / 图 / 签名 |
| `/api/live`（缓存内） | 9 | 16 | `/proc` 冷扫描本身 71 ms 墙钟（3630 进程、243 命中），3 s 缓存（Python 178 ms） |
| `/api/messages … append=1`（无新内容） | 2 | 2–3 | 不变 |
| 一条 SSE 观察空转 20 s（安静会话） | — | 1440 ms（7.2 %；其中"什么都不做"1.9 %） | 发布者每 500 ms 的探测不再每次遍历目录（视图打开容忍 3 s 旧列表，文件自己 `stat`）；剩余是根变化引起的重扫 |
| 一条 SSE 观察空转 20 s（活动会话） | 2680 ms（13.4 %） | 1850 ms（9.2 %） | 同上 + 文件每次增长的重投影 |

归因（临时计时，不计进程扫描、同一页面 41 s、根目录持续被写入）：4.47 CPU-s 中
**2.3 s 是 19 次"活动文件变了 → 重投影"**（11 MB 的 Claude 主会话每次 100–300 ms：
`parse_candidate` 每次都把全部记录重新投影成事件，AST 复用与否都一样——
`SESSIONDOCK_AST_CACHE_MB=1024` 实测无差别），0.17 s 是 19 次 `malloc_trim`，其余是遍历 +
重建行（根一变就要重建：遍历 ~20 ms + `graph::build` ~27 ms + 签名 ~7 ms；视图读到比
索引新的字节时仍立即重扫一次——把它节流到每秒一次试过，但读模型的测试与文档承诺
`meta` 与字节一致，故撤回；其余打开按 `OPEN_TTL` 3 s 复用列表）。计入进程扫描再加 ~3.5 %
（每 3 s 一次冷扫描）。要到 ≤ 5 % 一核，剩下两件都在本包范围外：
`sessions/views` 的追加改成增量投影，`graph::build` 改增量
（`sessions/index/graph.rs`）。

## 旧的合成读取基准（供对照）

`python3 tests/read_benchmark.py --binary target/release/sessiondock --samples 10`
创建临时合成记录（每份 2000 条正文）与 loopback 服务；2026-09-12 一次采样（ms）：

| 场景 | 1,039,032 字节 | 10,475,032 字节 |
| --- | ---: | ---: |
| 冷列表（单次） | 22.71 | 75.29 |
| 热列表 p50 / p95（10次） | 0.41 / 1.43 | 0.39 / 0.74 |
| 首次窗口（单次） | 7.14 | 54.66 |
| 热窗口 p50 / p95（10次） | 3.59 / 4.04 | 17.15 / 29.94 |
| 空增量 p50 / p95（10次） | 0.41 / 0.45 | 0.70 / 0.81 |
| 追加后重解析并返回增量（单次） | 24.95 | 101.69 |

共享观察的确定性证据来自 `observe.rs` 单测：20 订阅共用一次初始版本读取；慢读者
跳过中间快照后仍能按自己的游标得到完整新增消息；最后订阅取消后回收。这证明读取
合并机制，不替代 50/1000 长连接的 CPU/RSS 负载测试。

尚待补充：多连接真实 SSE 基准、长期缓存抖动、慢消费者、审计饱和、Hub 节点超时，
以及相同正确性语料下的 Python 对照。
