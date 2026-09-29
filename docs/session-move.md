# 会话跨机器迁移（设计稿）

状态：**实现中：已完成首轮 Codex 隔离实验，迁移接口和页面尚未实现**（2026-09-30）。Python 没有这项功能，属于用户明确要求的
新功能，不适用"只做到 Python 对齐"的限制；但除本文列出的拒绝条件外，不要另加限制。
实现落地后，把本文改写成现行合同，并删掉 [TODO.md](../TODO.md) 里对应的条目。

把一台节点上的会话连同所需的会话关系和物理历史文件复制到另一台节点，验证后切换执行归属，
让用户在目标机器上继续 resume；只有源端不再被引用的文件才可以清理。
第一版要求两端工作目录的绝对路径相同，并按下文规定的范围验证内容一致。目录可以是共享
NFS，也可以是各自本地的目录；工作目录共享与会话存储共享必须分别判断。

## 目标与非目标

目标：

- 在 Hub 页面选中一个会话，选目标机器，看到一份将要迁移的清单，确认后完成迁移。
- 迁移后目标机器的 SessionDock 列出这些会话，树形结构（fork、子代理、`spawned_by`）
  与源机器一致，可以直接在目标机器 resume。
- 切换执行归属前失败，源机器数据保持原样；切换后清理失败，目标完整副本保留，并可重试源端清理。

非目标（第一版不做）：

- OpenCode 会话：数据是 `opencode.db` 里的行，不是文件（[opencode.md](opencode.md)）。
- Linux 与 Windows/macOS 之间迁移：第一版只验证 Linux↔Linux；跨平台路径及工具适配不在范围内。
- 正在运行的会话"热迁移"：只迁移已停止的会话。
- 进程、PTY、内存中的工具状态、待处理请求和正在执行的命令不迁移。复制全部 subagents
  只保证记录和关系完整，历史中的 shell session ID、工具句柄不代表目标端存在可继续使用的对象。
- 迁移终端录像：agent 行本身不带录像（[terminal-records.md](terminal-records.md)）。

## 迁移单元：依赖闭包

用户选中一个会话 S。计划分别维护**会话关系图**和**物理历史依赖图**，不能只按侧栏树或 sid
收集文件。会话层清单由三部分组成：

1. **下游（全部移走）**：S 本身，以及 history 依赖 S 的所有会话，递归展开：
   - Claude：`agent_items` 子代理、`<sid>/subagents/`；把 S 当作祖先的 fork。
   - Codex：`parent_thread_id` 指向 S 的子线程、`forked_from_id` 指向 S 的 fork；另外沿
     物理历史的反向引用查找真正依赖 S 所持文件的会话，不能只比较 `history_base.thread_id`。
   - Claude 续接链：`continued_in` 串起来的各代会话，无论新旧，同一逻辑会话的各代都算进来
     （[history-pages.md](history-pages.md)）。
   - `spawned_by` 指向下游会话、且来源是 Claude/Codex/Grok 的子会话。
2. **上游（祖先）**：S 及下游会话的 fork 祖先、Claude ancestor 链、`spawned_by` 父会话，
   一直追到根；另补齐所有物理历史依赖。逻辑关系用于保留树形结构，物理依赖用于完整读取历史，
   两者不能混为一谈：旧式 Codex fork 可能自包含，缺少 `history_base` 所需文件则会导致
   `unsupported_history`（见 [glossary.md](glossary.md)）。
   - 祖先在源机器上**已无**迁移单元之外的依赖者 → 移走。
   - 祖先仍被源机器上其他会话依赖（兄弟 fork、其他子代理）→ **复制**：源机器保留，
     目标机器多一份。这与 trash 的 fork 父会话保护规则一致（[trash.md](trash.md)）。
3. **不迁移、只提示**：OpenCode 子会话等不支持的来源。清单里列出这些会话，并用一行说明
   "留在源机器，挂靠会断开"。

闭包必须基于**一份**已发布的 inventory 快照计算，与 `trash::plan::protection_set` 的做法
相同，避免计算过程中别的操作改变依赖关系。一个长寿的父会话可能有几十个子会话，清单页必须
完整列出将移走、将复制、留在源机器的会话，以及总字节数，由用户确认后再执行。
物理文件身份、当前 rollout 选择、归档索引与引用边界须作为同一计划的补充快照固定下来，
不能把只显示当前会话的 inventory 行当成完整文件清单；确认及执行时验证这些快照的 stamp。

### Codex 物理历史依赖和多代 rollout

`forked_from_id`、`parent_thread_id` 描述逻辑关系；非空 `history_base` 描述必须读取的另一份
rollout 及其固定前缀边界。依赖可以递归延伸，实际历史文件所属的会话不一定是界面上的父会话。
现有 [history.rs](../crates/sessiondock/src/sessions/history.rs) 的 `history_link` 和
[index/graph.rs](../crates/sessiondock/src/sessions/index/graph.rs) 的 `physical_parent` 已区分
这两类关系。迁移复用相应语义，但不能据此假定现有 inventory 已覆盖每个 CLI 版本的全部原生文件。

原生 `thread/revert` 等操作可能保留 thread ID、创建新 rollout 并切换当前指针。因此：

- 一个 sid 可以对应多份文件。搜索范围包含 `sessions/` 和 `archived_sessions/` 等该版本的
  历史目录；不能取搜索到的第一个文件，也不能只复制 inventory 当前显示的路径。
- manifest 分别记录 thread ID、原生 rollout 身份（该版本存在 rollout ID 时必须记录）、
  相对路径、当前选中版本、归档状态和每条 `history_base` 的字节/ordinal 边界。
  根据两端版本解析具体文件身份；不能用 sid 或“最新文件名”代替。
- 收集迁移线程的各代 rollout，并递归补齐其中的物理依赖；依赖文件可能没有单独的可见会话行。
  文件按身份去重，字节数只计一次，清单显示这些附带历史文件。
- 带偏移的原始文件必须逐字节保留，包括 JSON 排列和换行；禁止重新序列化、重排 JSON、
  改换行或批量替换路径。目标端必须仍能解析到同一文件的同一固定前缀。

### 带过去与从源端移除分别决定

计划同时记录会话的目标归属、需要复制的文件集合，以及每个源文件的全部已知引用。
例如 P 分出 A、B，迁移 A 需要 P 的历史，而 B 留在源端，则 P 只复制，源文件保留。
这个判断递归应用于逻辑祖先和物理历史依赖，不能只复用 trash 当前按 fork 父会话计算的保护集。

“移走”的会话也可能有仍被其他会话引用的旧 rollout；它的当前执行归属可以切换，旧文件却仍须
保留。清理前重新核对剩余引用和文件身份：出现新的外部引用时保留该文件，不能删除它，也不能
借此扩大用户已确认的迁移闭包。源端 trash 必须接收确认无剩余引用的文件集合，不能对会话目录
无差别整目录移走。依赖解析尚不完整时不得宣称源端可安全清理。

## 每个会话的原生文件集

trash 只移动 transcript、子代理文件和它们的 sidecar。迁移要求的文件更全，否则目标机器上的
rewind 和大输出引用会失效。

| 来源 | 必须 | 应当 | 不迁移（机器运行时状态） |
|---|---|---|---|
| Claude | `projects/<编码cwd>/<sid>.jsonl`；整个 `projects/<编码cwd>/<sid>/`（`subagents/`、`tool-results/`、`agent-*.meta.json`） | `file-history/<sid>/`（rewind 恢复代码时用） | `sessions/<pid>.json`、`session-env/`、`shell-snapshots/` |
| Codex | 迁移线程各代 rollout 及递归物理依赖，包含归档历史，保持相对路径及字节不变；当前 rollout 的选择信息 | 按两端版本导入原生名称、归档、置顶、项目关联等元数据；`session_index.jsonl` 只是可能的来源之一 | 日志、队列和进程运行状态；状态库的会话元数据不能直接当作可丢弃缓存，见下 |
| Grok | 整个会话目录 `sessions/<编码cwd>/<sid>/` | — | — |

Claude transcript 里写着绝对路径，例如 tool-results 的
`saved to: /home/<user>/.claude/projects/<编码cwd>/<sid>/tool-results/...`。因此要求源机器和
目标机器的 **CLI 根目录绝对路径相同**，这是第一版为保留原始路径而采用的范围限制，
不是所有 CLI 格式都要求 home 相同。文件按相对 CLI 根的同一路径落盘，内容不改写。
Codex 支持配置 cwd，但迁移仍须检查本次历史涉及的工作目录、文件、附件及工具可用性，
不能把更换 cwd 当作已有引用已被修复。

动态工具定义也属于恢复审计范围：即使 Codex 保存并恢复了定义，目标端仍可能缺少执行器、
MCP 服务或权限。计划列出依赖及目标端检查结果；缺失时明确提示，不能据定义存在就承诺工具可用。
不复制认证凭据或机器级工具运行状态。

### 第一步先做 Codex 实测

2026-09-30 已做一次有限实测：Codex CLI 0.157.0，在临时 `CODEX_HOME` A 中创建普通会话并 fork，
只将 `sessions/` 内 rollout 按原相对路径复制到初始没有 SQLite 的 B，父线程和子线程的
`codex exec resume <id>` 均成功。四次调用的 rollout `turn_context` 均记录
`gpt-5.6-luna`、`low`；B 在运行后自行创建了状态库。此结果仅证明这个版本和样本按 ID 恢复成功，
没有验证原生列表、名称/归档/置顶/项目关联、分页 fork、多代 rollout 和工具恢复，不构成上线依据。

随后新增 [session_move_codex_real.py](../tests/session_move_codex_real.py)，使用 Codex 0.159.0
的 app-server 在临时 home 中创建分页父线程、两个 fork、同 ID revert、从回退版本再次 fork、
真实子代理和归档兄弟线程。全部真实 turn 的模型/强度都从 rollout 断言为 Luna / low；
日常配置哈希前后不变。首轮结果：

| 检查 | 仅复制原始历史文件 | 同版本、同 schema 的按线程导入实验 |
|---|---|---|
| 原生活动/归档列表和当前 rollout 路径 | 能发现，当前路径正确 | 保持正确 |
| 回退线程及其后续 fork 的分页历史 | 丢失回退前保留的一轮，即使按 ID resume 成功 | 导入所选线程的历史投影后恢复 |
| 名称、置顶、项目关联 | 不能完整保留 | 导入所选线程元数据及关联项目后保留 |
| 目标端已有无关会话 | 保留 | 保留 |
| 在目标端继续新一轮 | 单独成功不能证明历史完整 | 导入并核对后继续成功，源历史字节不变 |

该版本除 `state_5.sqlite` 的会话元数据外，还涉及 `thread_history_1.sqlite` 中的
`thread_turns`、`thread_items`、`thread_history_projection_state`。实验只在临时 home 中、
原生扫描建立目标 thread 行之后，核对表结构并导入指定线程和必需的项目关联，不复制整库。
已验证每个数据库内事务的回滚；**这不是生产导入适配器，也没有证明跨数据库崩溃恢复或跨版本兼容**。
动态工具定义随原文件保留，目标执行能力仍未验证。

还确认了 `history_base.thread_id` 在该版本实际指向不可变的 rollout ID，可能不同于 thread ID。
SessionDock 的读取实现据此补齐了独立物理身份解析；合成浏览器回归覆盖三代文件、归档依赖、
回退后 fork、搜索，以及缺失/重复依赖的报错和恢复。迁移计划仍须独立核对原生数据库的当前选择，
不能把读模型按头部时间排列各代文件的规则当成原生选择元数据。

原生实现存在数据库未命中后扫描文件、修复路径的流程；不能断言缺少 thread 行就无法 resume，
也不能由普通 resume 成功推导整个 SQLite 都可丢弃。两端版本的隔离实测必须分别覆盖：

1. **按 ID 恢复**：恢复的是预期的当前 rollout，完整读取递归父历史。
2. **原生列表发现**：CLI 的会话列表能够找到目标线程，归档状态和当前版本正确。
3. **元数据保真**：名称、`is_pinned`、`project_id` 等该版本存在的字段，以及动态工具定义，
   分别有导入结果和验证证据；不能将原生元数据全部概括为 `session_index.jsonl`。

生产导入实现前还需扩充实测，结论写回本文：

1. 准备两个临时 `CODEX_HOME`：A 和 B。用 `gpt-5.6-luna` low 建立分页 fork、多代 rollout、
   子代理以及仍有兄弟会话引用共同祖先的样本，包含活动和归档目录。
2. 将完整物理依赖集合原样复制到 B，分别验证上述三项，并验证源端兄弟会话仍可读取和恢复。
3. 若文件扫描/backfill 无法保全元数据，按实际缺项选择该版本支持的导入方式，并验证事务回滚。
   不覆盖目标整库，不预先写死 `state_5.sqlite` 表结构或 INSERT；源、目标版本不同也必须测试。
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
| 两端的 CLI 根**不是同一份存储**（例如同一个 NFS 导出） | 409 `move_same_root`；此时应切换执行位置，第一版不走文件迁移 |
| 迁移单元内每个会话的 `cwd` 在目标机器上存在且是目录 | 409 `move_cwd_missing` |
| cwd 内容通过下述一致性核对 | 409 `move_cwd_mismatch`；不可读取时保留实际文件系统错误 |
| 迁移会话及所需历史文件的相关写入已停止，包括仅复制的祖先。与 trash 使用同一套受管 host 最新状态和原生进程扫描，并校验文件 stamp | 409 `move_session_running`；计划后文件变化为 `move_plan_stale` |
| 目标文件身份、内容和当前版本选择满足下述冲突规则 | 409 `move_conflict` |

### 工作目录一致性的验证范围

第一版保留“内容一致”的硬条件：核对 cwd 下的路径集合、文件类型、普通文件字节摘要、执行权限
和符号链接目标，包含未跟踪及忽略文件；`.git` 管理数据不作为工作树内容比较，git 状态另行展示。
cwd 内指向外部的符号链接所需内容，以及历史引用的外部附件、文件，必须列入依赖清单，明确哪些
随迁移复制、哪些在目标端原位验证。不能把未检查的外部路径描述为已一致。记录核对范围与结果，
确认时重查 stamp；扫描中发生写入需重新计划。摘要用于数据一致性证据，不用于源码准入。

相同 Git HEAD 加相同 dirty 状态不能证明上述内容相同：两个工作树可以修改同一文件却得到不同
字节，未跟踪、忽略文件和外部链接也可能不同。若以后改为只提示风险，应同时取消硬条件及对应
拒绝码，不得保留“已验证内容一致”的承诺。

以下情况只警告、不拒绝。它们只在清单页显示，一致时不显示任何提示：

- cwd 是 git 仓库，两端 `HEAD` 不同或有未提交改动：提示版本/改动状态与 rewind 风险。
  此提示独立于内容核对，不把 Git 状态当作内容一致的证明。
- 目标机器的 CLI 版本低于源机器："目标机器的 Claude 较旧，可能无法读取新的记录"。

目标冲突先核对文件身份和当前选中的版本，再比较字节：同一物理文件且内容完全相同 → 跳过；
同一 rollout、目标确为源的字节前缀，且扩展不会改变现有引用语义或选择错误的当前版本 → 可覆盖；
否则 → `move_conflict`。不同 rollout 不能仅因 sid 相同就互相覆盖。目标已有同 thread 的另一条
演化分支或不能确认当前版本时，需要保留现状并报告冲突。覆盖已有文件前保留原字节及选择元数据，
用于失败回滚；“只删本次写入”不足以恢复被覆盖的文件。

同一存储须根据挂载/存储身份和实际文件身份核实，不能只比较路径字符串或跨机器的 inode 数值。
两端共享会话存储时，复制后源端 trash 会移动目标依赖的同一份文件；第一版明确拒绝该流程，
将来实现执行位置切换时也不得执行文件复制和源端 trash。

新增的错误码必须登记到 [error-codes.md](error-codes.md)（由 `tests/error_codes.py` 生成）。

## 流程

Hub 负责编排。节点之间不直接通信：mesh 本身就是经中继的星形结构
（[network-topology.md](network-topology.md)），所以经 Hub 转发并不会多绕路，还能直接复用
已有的 Hub→节点鉴权通道（第二监听 `8743`）。

1. **plan**（只读）：Hub 向源节点请求迁移单元和文件清单（路径、大小、mtime，以及每个文件的
   sha256 数据摘要、rollout 身份、物理依赖边界和源端剩余引用），向目标节点请求前置条件检查
   （目录内容、引用可用性、冲突、当前版本、工具环境、git 状态、CLI 版本）。
   返回清单和 `plan_id`。
2. **confirm**：用户在清单页确认。之后执行的是这份 plan，不重新计算闭包；若文件的 stamp 已经
   变化，就以 409 `move_plan_stale` 拒绝，页面重新 plan。
3. **lock**：锁定迁移会话和本次依赖文件的相关写入，在两端阻止受管 resume/启动与发布冲突。
   仅复制的祖先也须在取快照和复制期间停止写入；锁不能阻止用户独立启动的原生 CLI，因此还须
   重查进程和文件 stamp。前端显示"迁移中"。
   这把锁要能在进程重启后保留，并有过期时间：崩溃后不能永远锁住。
4. **transfer**：Hub 从源节点流式拉取 tar 格式的数据（第一个成员是 `manifest.json`，其余是
   相对 CLI 根的文件），同时流式推给目标节点。目标节点先写入私有 staging 目录，逐个文件校验
   大小和 sha256。
5. **publish**：目标节点按冲突规则把文件原子 rename 到 CLI 根下的原路径（跨文件系统时，先完整
   复制，再删除 staging），然后按已验证的版本适配方式导入原生索引/元数据和当前 rollout 选择，
   并写入 SessionDock 元数据。每次覆盖和元数据修改都有本次事务的恢复记录。
6. **verify**：目标节点刷新 inventory。迁移单元里的每个 uid 都必须出现，并能读出 history
   首页及跨物理依赖边界的分页，不允许返回 `unsupported_history`；同时核对原生列表、当前
   rollout 选择和元数据。恢复能力使用两端版本已经隔离验证的解析/恢复检查，不自动发送新模型
   请求或运行历史中的命令。仅普通单文件 resume 成功不足以通过。
7. **switch ownership**：目标验证通过后，持久化切换执行归属；源端保持迁移会话不可启动，
   目标端成为继续会话的入口。会话关系迁移与依赖文件留存分别记录。
8. **retire source**：重新核对源端剩余引用，只把可清理的文件交给 trash，manifest 标注
   `moved_to: <目标节点 id>`。复制的祖先及仍有引用的旧 rollout 保留；成功后释放临时锁。
9. 页面跳转到目标机器上的 S。

失败处理：

- 第 4–6 步失败：删除本次新建文件，恢复本次覆盖的原文件、版本选择及元数据，删除 staging；
  源节点解锁，源数据从未改动，不影响目标原有会话。
- 第 7 步结果不确定：通过持久化 journal 与两端状态对账，先确定执行归属，不能直接清理任一端。
- 第 8 步失败：目标仍有完整副本，源端原文件或本次 trash 项保留，迁移会话仍不可启动。
  页面提示"已复制到目标机器，源机器清理失败"，并提供重试清理的按钮。**不自动删除任何一边**。
- Hub 把每次迁移的步骤记进 journal。Hub 重启后能看出迁移停在哪一步，并据此继续或回滚。
  临时锁过期不得撤销已持久化的执行归属切换，否则清理失败后源端会再次允许启动同一会话。

sha256 摘要只是数据证据：记进 manifest 和 journal，用来判断传输是否完整。不对任何源码做
哈希准入。

## SessionDock 自身状态

| 状态 | 处理 |
|---|---|
| `session-metadata.json` 的行（星标、fork 可见性、`nest_parent`、`independent`、`spawned_by`） | 随会话写入目标节点，从源节点删除（复制的祖先两边都保留） |
| 会话草稿（`state/conversations`） | 转移会话在切换成功后从源节点丢弃；仅复制祖先的草稿在源端保留 |
| 已完成的启动回执 | 与 trash 相同，在源节点丢弃 |
| Hub 命名空间里的 uid | 节点前缀会变。旧的深链 `?sid=...&node=<旧节点>` 找不到会话时，回退到按 `sid` 查找（[external-links.md](external-links.md)） |

## 页面

- 能力标志：`session_move`（[capabilities.md](capabilities.md)）。只在 Hub 页面、源节点和
  至少一台其他节点都声明该能力时显示。
- 侧栏菜单和会话标题菜单加"移到其他机器…"，打开统一对话框（`popup.js`）：
  - 目标机器列表，不可用的置灰，并给一行原因。
  - 选中后显示清单，分为"移走 N 个 / 复制 M 个 / 留在源机器 K 个"；复制栏写明“两台机器都保留”。
    另列附带历史文件、源端因剩余引用而保留的文件和去重后的总大小，警告放在最后。
  - 按钮文案："移动"。执行过程中显示当前步骤和进度。
- 文案用普通话，不暴露 plan_id、staging 这类内部概念。一致时不出任何提示。
- 多选第一版不支持：一次只迁移一个 S 及其闭包。

## 接口草案（实现时可调整，调整后更新本文）

Hub：

- `POST /api/move/plan {uid, target}` → `{plan_id, move[], copy[], stay[], files[], retained_files[], bytes, warnings[]}`
  文件清单包含物理身份、引用和清理资格；内部边界信息保存在 manifest 中。
- `POST /api/move {plan_id}` → `{move_id}`；`GET /api/move/{move_id}` → 当前步骤、进度、错误
- `POST /api/move/{move_id}/retry-cleanup`

节点（只对 Hub 鉴权通道开放）：

- `POST /api/move/node/plan`：返回闭包和文件清单（源节点）/前置条件检查结果（目标节点）
- `POST /api/move/node/lock`、`/unlock`
- `GET /api/move/node/export?plan_id=`：tar 流
- `POST /api/move/node/import`：tar 流 → staging → publish，返回校验结果
- `POST /api/move/node/retire`：交给 trash

## 验证

最优先原型必须覆盖分页 fork、多代 rollout、子代理，以及源端仍有兄弟会话依赖共同祖先。
普通单文件迁移只作为基础样本，不能据此宣布方案可靠。

- 新增 `tests/session_move_browser.py`：起两个隔离的节点服务，加一个 Hub（fixture 参照
  `hub_bulk_browser.py`、`hub_nest_browser.py`、`trash_browser.py`）。两个节点使用
  **不同的临时 CLI 根，但绝对路径形状相同**（例如用独立的 mount namespace 或 chroot 式前缀，
  实现时选定），数据全部合成。在页面上点菜单 → 选目标 → 看清单 → 移动 → 在目标节点打开
  会话。覆盖以下场景：
  - Claude：带 subagents、tool-results、file-history，含 fork 链和 `continued_in` 续接链。
  - Codex：递归 `history_base` 的分页 fork、逻辑父会话与物理父历史不同、同 thread 多代 rollout、
    归档目录依赖、当前版本选择，另有 `parent_thread_id` 子线程；确认所有偏移仍落在原字节边界。
  - Grok：整目录迁移。
  - 跨 CLI 的 `spawned_by` 子会话；OpenCode 子会话进入"留在源机器"一栏。
  - 祖先仍有兄弟会话时走复制分支；源端兄弟引用的旧 rollout 保留，兄弟的 history 仍然可读。
  - 拒绝场景：会话/依赖文件仍在写入、rollout 分支冲突、cwd 缺失或内容不同、同一份会话存储。
    包含 Git HEAD/dirty 状态相同但文件字节不同，以及不同路径实际共享同一存储的样本。
  - 原生列表发现、名称/归档/置顶/项目关联等元数据；动态工具定义存在但执行器缺失时的提示，
    确保不重用源端 shell session ID 或工具句柄。
  - 故障注入：传输中断、publish 中途失败（恢复被覆盖文件和元数据）、归属切换结果不确定、
    retire 部分失败、新增源端引用，以及 Hub 重启和临时锁过期后的 journal 恢复。
- 真实 CLI（`--include-real`）：按上文"第一步先做 Codex 实测"的方法，在临时 home 下复制后
  resume，并断言实际使用的模型。
- 文档：`python3 tests/check_docs_links.py`；新错误码重新生成 `error-codes.md`。

## 风险清单

1. **同一 sid 在两台机器上分叉**：主要靠迁移锁、只迁移已停止的会话、源端进 trash 来防。
   "复制"的祖先本来就两边都有，两边都 resume 它就会分叉，清单页的复制栏要写明"两台机器都保留"。
2. **文件集漏项**：CLI 升级后可能新增按 sid 存放的目录。实现时对照每个 CLI 的真实根目录再核一遍，
   并在 plan 里列出 `<sid>` 相关但未识别的路径作为警告，不要静默忽略。
3. **目录验证范围与成本**：内容核对可能很大，必须显示范围和进度；Git 状态只提供版本背景。
   外部引用、符号链接和工具执行能力不能由 cwd 摘要替代。
4. **闭包过大**：必须在清单页完整展示，不能自动截断。
5. **Codex 原生发现与恢复**：普通 fork 的按 ID resume 已验证；分页 fork、多代 rollout、原生列表
   和元数据等完整验证通过前，Codex 迁移不能上线。
