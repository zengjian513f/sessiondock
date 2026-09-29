# 会话跨机器迁移（设计稿）

状态：**设计稿，未实现**（2026-09-30，zj 提出）。Python 没有这项功能，属于用户明确要求的
新功能，不适用"只做到 Python 对齐"的限制；但除本文列出的拒绝条件外，不要另加限制。
实现落地后，把本文改写成现行合同，并删掉 [TODO.md](../TODO.md) 里对应的条目。

把一台节点上的会话连同它依赖的会话（父会话、fork 祖先、子代理、续接链）挪到另一台节点，
让用户在目标机器上继续 resume。只允许在两台机器能看到**同一绝对路径、内容一致**的工作目录时
迁移：目录可以是两台机器共享的 NFS 挂载，也可以是各自本地、内容相同的目录。

## 目标与非目标

目标：

- 在 Hub 页面选中一个会话，选目标机器，看到一份将要迁移的清单，确认后完成迁移。
- 迁移后目标机器的 SessionDock 列出这些会话，树形结构（fork、子代理、`spawned_by`）
  与源机器一致，可以直接在目标机器 resume。
- 任何一步失败，源机器上的会话保持原样，可以直接重试。

非目标（第一版不做）：

- OpenCode 会话：数据是 `opencode.db` 里的行，不是文件（[opencode.md](opencode.md)）。
- Linux 与 Windows/macOS 之间迁移：home 路径和 cwd 编码都不同，transcript 里的绝对路径会失效。
- 正在运行的会话"热迁移"：只迁移已停止的会话。
- 迁移终端录像：agent 行本身不带录像（[terminal-records.md](terminal-records.md)）。

## 迁移单元：依赖闭包

用户选中一个会话 S。迁移单元由三部分组成：

1. **下游（全部移走）**：S 本身，以及 history 依赖 S 的所有会话，递归展开：
   - Claude：`agent_items` 子代理、`<sid>/subagents/`；把 S 当作祖先的 fork。
   - Codex：`parent_thread_id` 指向 S 的子线程；`forked_from_id` 或 `history_base.thread_id`
     指向 S 的 fork。
   - Claude 续接链：`continued_in` 串起来的各代会话，无论新旧，同一逻辑会话的各代都算进来
     （[history-pages.md](history-pages.md)）。
   - `spawned_by` 指向下游会话、且来源是 Claude/Codex/Grok 的子会话。
2. **上游（祖先）**：S 的 fork 祖先、Claude ancestor 链、Codex `history_base` 父线程、
   `spawned_by` 父会话，一直追到根。这些是**硬依赖**：缺了它们，读模型会报
   `unsupported_history`（missing ancestor），Codex 的固定前缀也无法重新解析
   （[glossary.md](glossary.md)）。
   - 祖先在源机器上**已无**迁移单元之外的依赖者 → 移走。
   - 祖先仍被源机器上其他会话依赖（兄弟 fork、其他子代理）→ **复制**：源机器保留，
     目标机器多一份。这与 trash 的 fork 父会话保护规则一致（[trash.md](trash.md)）。
3. **不迁移、只提示**：OpenCode 子会话等不支持的来源。清单里列出这些会话，并用一行说明
   "留在源机器，挂靠会断开"。

闭包必须基于**一份**已发布的 inventory 快照计算，与 `trash::plan::protection_set` 的做法
相同，避免计算过程中别的操作改变依赖关系。一个长寿的父会话可能有几十个子会话，清单页必须
完整列出将移走、将复制、留在源机器的会话，以及总字节数，由用户确认后再执行。

## 每个会话的原生文件集

trash 只移动 transcript、子代理文件和它们的 sidecar。迁移要求的文件更全，否则目标机器上的
rewind 和大输出引用会失效。

| 来源 | 必须 | 应当 | 不迁移（机器运行时状态） |
|---|---|---|---|
| Claude | `projects/<编码cwd>/<sid>.jsonl`；整个 `projects/<编码cwd>/<sid>/`（`subagents/`、`tool-results/`、`agent-*.meta.json`） | `file-history/<sid>/`（rewind 恢复代码时用） | `sessions/<pid>.json`、`session-env/`、`shell-snapshots/` |
| Codex | 每个线程的 rollout 文件，保持 `sessions/YYYY/MM/DD/` 相对路径不变 | `session_index.jsonl` 中这些线程的名字行，追加到目标机器 | 各类 `*.sqlite` 日志和队列库；`state_5.sqlite` 的处理待实测，见下 |
| Grok | 整个会话目录 `sessions/<编码cwd>/<sid>/` | — | — |

Claude transcript 里写着绝对路径，例如 tool-results 的
`saved to: /home/<user>/.claude/projects/<编码cwd>/<sid>/tool-results/...`。因此要求源机器和
目标机器的 **CLI 根目录绝对路径相同**，文件按相对 CLI 根的同一路径落盘，内容不改写。

### 第一步先做 Codex 实测

`codex resume <id>` 是只靠 rollout 文件，还是必须有 `state_5.sqlite`（`threads` 等表）里的行，
目前**没有验证**，整个设计里这一环最不确定。实现前先做一次实测，结论写回本文：

1. 准备两个临时 `CODEX_HOME`：A 和 B。在 A 里用 `gpt-5.6-luna` low 建一个带一次 fork 的会话。
2. 只把 rollout 文件按相对路径复制到 B，在 B 里执行 `codex resume <id>` 和 fork 子线程的 resume。
3. 如果失败，找出缺的是哪张表的哪一行，再决定用 Codex 自带的 backfill 还是由目标节点补写。
   不允许为此改动用户日常的 Codex 默认配置（`~/.claude/cli-model-isolation.md`）。

Claude（`claude-haiku-4-5-20251001`）和 Grok（`grok-4.6`）用同样的方式各验证一次：复制后
resume，并确认 file-history rewind 可用。

## 前置条件与拒绝条件

计划阶段逐项检查。条件不满足时，这台目标机器在选择框里置灰，并给一行原因。

| 条件 | 不满足时 |
|---|---|
| 源节点和目标节点都在线，且都声明了能力 `session_move` | 409 `move_node_unavailable` |
| 两端操作系统同族（第一版只支持 Linux↔Linux） | 409 `move_platform` |
| 该来源的 CLI 根目录在两端的绝对路径相同 | 409 `move_root_mismatch` |
| 两端的 CLI 根**不是同一份存储**（例如同一个 NFS 导出的 `~/.claude`，此时本来就不需要迁移） | 409 `move_same_root` |
| 迁移单元内每个会话的 `cwd` 在目标机器上存在且是目录 | 409 `move_cwd_missing` |
| 迁移单元内没有正在运行的会话。与 trash 用同一套判断：受管 host 的最新状态，加原生进程扫描 | 409 `move_session_running` |
| 目标机器上已有同 sid 的文件，且内容与源文件不同、也不是源文件的前缀 | 409 `move_conflict` |

以下情况只警告、不拒绝。它们只在清单页显示，一致时不显示任何提示：

- cwd 是 git 仓库，两端 `HEAD` 不同或有未提交改动："目标机器上的 `<cwd>` 与源机器不一致，
  rewind 恢复的文件可能对不上"。不做逐文件哈希全量比对。
- 目标机器的 CLI 版本低于源机器："目标机器的 Claude 较旧，可能无法读取新的记录"。

目标机器上的同 sid 冲突按以下规则处理：内容完全相同 → 跳过；目标文件是源文件的字节前缀
（挪过去又挪回来）→ 覆盖；其他情况 → `move_conflict`。

新增的错误码必须登记到 [error-codes.md](error-codes.md)（由 `tests/error_codes.py` 生成）。

## 流程

Hub 负责编排。节点之间不直接通信：mesh 本身就是经中继的星形结构
（[network-topology.md](network-topology.md)），所以经 Hub 转发并不会多绕路，还能直接复用
已有的 Hub→节点鉴权通道（第二监听 `8743`）。

1. **plan**（只读）：Hub 向源节点请求迁移单元和文件清单（路径、大小、mtime，以及每个文件的
   sha256 数据摘要），向目标节点请求前置条件检查结果（路径是否存在、冲突、git 状态、CLI 版本）。
   返回清单和 `plan_id`。
2. **confirm**：用户在清单页确认。之后执行的是这份 plan，不重新计算闭包；若文件的 stamp 已经
   变化，就以 409 `move_plan_stale` 拒绝，页面重新 plan。
3. **lock**：源节点对要移走的 uid 加迁移锁。锁住期间拒绝 resume 和启动，前端显示"迁移中"。
   这把锁要能在进程重启后保留，并有过期时间：崩溃后不能永远锁住。
4. **transfer**：Hub 从源节点流式拉取 tar 格式的数据（第一个成员是 `manifest.json`，其余是
   相对 CLI 根的文件），同时流式推给目标节点。目标节点先写入私有 staging 目录，逐个文件校验
   大小和 sha256。
5. **publish**：目标节点按冲突规则把文件原子 rename 到 CLI 根下的原路径（跨文件系统时，先完整
   复制，再删除 staging），然后追加 Codex 名字行，并写入 SessionDock 元数据。
6. **verify**：目标节点刷新 inventory。迁移单元里的每个 uid 都必须出现，并能读出 history
   首页，不允许返回 `unsupported_history`。
7. **retire source**：源节点把"移走"那部分会话交给 trash（复用 `trash` 模块，manifest 标注
   `moved_to: <目标节点 id>`）。"复制"的祖先保留不动。然后解锁。
8. 页面跳转到目标机器上的 S。

失败处理：

- 第 4–6 步失败：目标节点删除 staging 和已经 publish 的文件（只删本次写入的），源节点解锁，
  源数据从未改动。
- 第 7 步失败：两边都有完整副本，源节点仍保持锁定。页面提示"已复制到目标机器，源机器清理
  失败"，并提供重试清理的按钮。**不自动删除任何一边**。
- Hub 把每次迁移的步骤记进 journal。Hub 重启后能看出迁移停在哪一步，并据此继续或回滚。

sha256 摘要只是数据证据：记进 manifest 和 journal，用来判断传输是否完整。不对任何源码做
哈希准入。

## SessionDock 自身状态

| 状态 | 处理 |
|---|---|
| `session-metadata.json` 的行（星标、fork 可见性、`nest_parent`、`independent`、`spawned_by`） | 随会话写入目标节点，从源节点删除（复制的祖先两边都保留） |
| 会话草稿（`state/conversations`） | 随会话迁移；源节点丢弃 |
| 已完成的启动回执 | 与 trash 相同，在源节点丢弃 |
| Hub 命名空间里的 uid | 节点前缀会变。旧的深链 `?sid=...&node=<旧节点>` 找不到会话时，回退到按 `sid` 查找（[external-links.md](external-links.md)） |

## 页面

- 能力标志：`session_move`（[capabilities.md](capabilities.md)）。只在 Hub 页面、源节点和
  至少一台其他节点都声明该能力时显示。
- 侧栏菜单和会话标题菜单加"移到其他机器…"，打开统一对话框（`popup.js`）：
  - 目标机器列表，不可用的置灰，并给一行原因。
  - 选中后显示清单，分为"移走 N 个 / 复制 M 个 / 留在源机器 K 个"，另显示总大小，警告放在最后。
  - 按钮文案："移动"。执行过程中显示当前步骤和进度。
- 文案用普通话，不暴露 plan_id、staging 这类内部概念。一致时不出任何提示。
- 多选第一版不支持：一次只迁移一个 S 及其闭包。

## 接口草案（实现时可调整，调整后更新本文）

Hub：

- `POST /api/move/plan {uid, target}` → `{plan_id, move[], copy[], stay[], bytes, warnings[]}`
- `POST /api/move {plan_id}` → `{move_id}`；`GET /api/move/{move_id}` → 当前步骤、进度、错误
- `POST /api/move/{move_id}/retry-cleanup`

节点（只对 Hub 鉴权通道开放）：

- `POST /api/move/node/plan`：返回闭包和文件清单（源节点）/前置条件检查结果（目标节点）
- `POST /api/move/node/lock`、`/unlock`
- `GET /api/move/node/export?plan_id=`：tar 流
- `POST /api/move/node/import`：tar 流 → staging → publish，返回校验结果
- `POST /api/move/node/retire`：交给 trash

## 验证

- 新增 `tests/session_move_browser.py`：起两个隔离的节点服务，加一个 Hub（fixture 参照
  `hub_bulk_browser.py`、`hub_nest_browser.py`、`trash_browser.py`）。两个节点使用
  **不同的临时 CLI 根，但绝对路径形状相同**（例如用独立的 mount namespace 或 chroot 式前缀，
  实现时选定），数据全部合成。在页面上点菜单 → 选目标 → 看清单 → 移动 → 在目标节点打开
  会话。覆盖以下场景：
  - Claude：带 subagents、tool-results、file-history，含 fork 链和 `continued_in` 续接链。
  - Codex：fork 带 `history_base`，另有 `parent_thread_id` 子线程。
  - Grok：整目录迁移。
  - 跨 CLI 的 `spawned_by` 子会话；OpenCode 子会话进入"留在源机器"一栏。
  - 祖先仍有兄弟会话时走复制分支，源机器兄弟的 history 仍然可读。
  - 拒绝场景：会话在运行、冲突、cwd 缺失、同一份根。
  - 故障注入：传输中断、publish 中途失败、retire 失败，以及 Hub 重启后的 journal 恢复。
- 真实 CLI（`--include-real`）：按上文"第一步先做 Codex 实测"的方法，在临时 home 下复制后
  resume，并断言实际使用的模型。
- 文档：`python3 tests/check_docs_links.py`；新错误码重新生成 `error-codes.md`。

## 风险清单

1. **同一 sid 在两台机器上分叉**：主要靠迁移锁、只迁移已停止的会话、源端进 trash 来防。
   "复制"的祖先本来就两边都有，两边都 resume 它就会分叉，清单页的复制栏要写明"两台机器都保留"。
2. **文件集漏项**：CLI 升级后可能新增按 sid 存放的目录。实现时对照每个 CLI 的真实根目录再核一遍，
   并在 plan 里列出 `<sid>` 相关但未识别的路径作为警告，不要静默忽略。
3. **目录内容不一致**：只做 git HEAD 和脏状态的警告，无法保证文件逐个一致；rewind 的风险由用户承担。
4. **闭包过大**：必须在清单页完整展示，不能自动截断。
5. **Codex 索引库**：见上文实测。结论出来之前，Codex 迁移不能上线。
