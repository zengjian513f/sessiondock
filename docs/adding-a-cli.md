# 接入新的 AI CLI

本文把各 CLI 接入及后续修复整理成新增来源和版本升级的清单；2026-10-06 补充 Claude、Codex、Grok、OpenCode、Agy 的 tag、输入状态与选择题识别方法。每项都要明确实现或记录不适用原因，不能把能启动、能发送当作交互接入完成。现行合同以各专题文档为准：[原生 tag](native-tags.md)、[OpenCode](opencode.md)、[Agy](agy.md)、[启动器](lifecycle-launcher.md)、[生命周期 HTTP](lifecycle-http.md)、[对话输入就绪](composer-input.md)、[CLI 状态对象](cli-state.md)、[读模型](read-model.md)、[liveness](liveness.md)、[移动](session-move.md)、[克隆](session-clone.md)。

## 先调研，再定路线

动手前先在隔离环境（临时 `HOME`/`XDG_*`，独立服务端口）里把下面的问题答清楚，并记录版本号。答案决定每一层走哪条路。

| 问题 | 决定什么 | 已有 CLI 的答案 |
| --- | --- | --- |
| 目标版本怎么装 | 安装脚本与版本 | OpenCode 接入时普通安装入口给 1.x，2.x 使用 `/v2/install`；这是当时的调查记录，新增 CLI 时重新核对目标版本和渠道 |
| 怎么读取版本、查询最新版本、无交互更新 | 机器设置中的客户端矩阵 | 按各 CLI 的实际命令、退出码与发布渠道核对；Agy 使用 `--version`、`update` 和官方平台 manifest，不据此推断其他 CLI |
| 会话存在哪里、什么格式 | 读模型路线 | Claude/Codex 是 JSONL 文件，Grok 是目录加 `summary.json`，OpenCode 2 是 SQLite；Agy 的目录库、原生会话库与完整 transcript 分开存储 |
| 原生记录类型和文本 tag 有哪些，各自表示什么 | 历史分类、气泡与元数据 | 建立目标版本的类型/tag 清单；系统通知、设置变更、附加元数据不能一律当成工具结果或混入用户正文，见下文“原生记录类型与 tag 识别” |
| 会话 ID 何时确定 | 启动类型（`Launch`） | Claude 启动参数指定 UUID（`new_assigned`）；OpenCode 先 `api session.create` 再 `--session`（`new_assigned`）；Codex、Agy 启动后才知道（`new_pending`） |
| 怎么恢复 | `resume_args` | Codex `resume {sid}`，OpenCode `--session {sid}`，Agy `--conversation {sid}` |
| 模型、强度怎么传；模型列表在哪 | 模型选择 | 见下文“生命周期与启动器” |
| TUI 画面：编辑区、忙碌、菜单、提问/权限弹层、中断键 | 输入就绪识别与网页题卡 | 见下文“发送与输入就绪”和“原生菜单与网页回答” |
| 斜杠命令是否写入同文 user 记录 | 发送回显队列 | Agy 的五个已核实裸菜单命令不写入原命令；不能一律等待回显，也不能把所有 `/` 开头输入都排除 |
| 删除语义 | 删除/回收站 | 文件型走回收站；OpenCode 只能调 `api session.remove`，不可恢复 |
| 原生数据能否移动或克隆，身份与依赖如何重写 | 整组操作能力 | OpenCode 的镜像不是可恢复的原生文件，当前不支持移动或克隆 |
| 子代理/子会话怎么表示 | 列表挂靠 | Claude 子目录，Codex `parent_thread_id`，OpenCode `parent_id` 和进程关系 |
| 会话进程与常驻服务怎么区分 | 运行态、挂靠 | argv0、环境变量（`*_SESSION_ID`）、打开的会话文件；OpenCode 外部 `run` 按进程、cwd 和出生时间配对，`serve` 不算会话命令活动 |

路线选择：

- **会话是文件**：直接写 summary 和 provider，索引按文件增量读取。
- **会话在数据库或服务里**：仿照 `sessions/opencode.rs`，只读打开并镜像成文件（`summary.json` 加 `messages.jsonl`），下游的列表、历史分页、搜索、媒体全部复用文件管线，不另开一套读路径。
- **ID 能在启动前确定**：一律用 `new_assigned`，页面在启动时就能跳到原生行。只有做不到时才用 `new_pending`。

## 识别方法与证据边界

统一流程是：明确来源与版本 → 保留原生结构 → 识别已知形状 → 投影统一语义 → 在实际操作前重新核对 → 验证结果。统一的是输出合同，各 CLI 的记录和按键语义仍由各自适配器解释。

| 证据来源 | 可以证明什么 | 不能据此推断什么 |
| --- | --- | --- |
| 原生 JSONL、数据库、hook | 消息角色、工具调用、原生状态与问题内容；各字段按其原生含义使用 | 当前终端是否有可输入的编辑区，历史问题是否仍在等待 |
| 同一次当前 PTY 捕获 | 可见编辑区、菜单、焦点、草稿及已识别的忙碌指示 | 模型已经收到或处理了消息 |
| 发送账本及对应原生回显 | 终端写入进度、是否观察到对应用户记录 | 按超时、空闲或画面消失猜测发送成功、失败或回合完成 |

按目标版本维护三份清单：**记录类型/tag、输入布局、菜单操作**。可放在现有 `docs/<cli>.md` 和菜单审计夹具中，不必另建重复文档。每条规则至少记录：

| 字段 | 内容 |
| --- | --- |
| 来源与版本 | CLI、原生字段路径/角色或终端布局；对应源码、真实捕获或合成变体 |
| 识别证据 | 脱敏样本、结构边界、所需样式/光标、必须满足的条件及相似反例 |
| 投影语义 | 角色、正文、元数据、输入状态或题卡；未知时如何保留内容或回退 |
| 操作语义 | 适用时记录焦点、原生按键、授权范围、取消和已有草稿的处理 |
| 验证与范围 | 对应套件；已支持、仅展示、原生终端回退、不适用或未核实及原因 |

源码审计、真实 CLI 记录/捕获、合成夹具分别标注。源码须对应目标版本；闭源 CLI 的已见画面不能证明菜单全集。升级时按三份清单复核，并实际走入受影响的子页面。

## 分期

- **一期**：能启动、能恢复、控制台可用、输入框能发。侧栏图标、颜色、选择器都要到位；页面可以先以终端为主（`sessionTerminalFirst`）。
- **二期**：历史、列表、搜索、媒体，系统性识别原生记录类型与 tag，启动即知身份，删除。
- **后续**：按 CLI 补模型目录、问题报告、菜单题卡、客户端版本与更新、CLI 状态（忙碌、编辑区文字）、回合三态、子会话挂靠；明确移动和克隆是否支持。

每一期完成浏览器验证后单独提交、推送，并立即部署、重启及健康检查。至少一台
SessionDock 节点和 Hub 成功即满足部署要求；其余节点记录待补发，不阻塞接入工程
或 goal，也不反复轮询。不要攒成一个大提交。

## 接入清单

下面覆盖 OpenCode、Agy 实际改动及后续共用能力。Rust 路径除另有注明外相对 `crates/sessiondock/src/`。使用 `rg` 同时查 `opencode`、`agy`、`grok`、`Source::Grok` 和来源枚举/集合，检查服务端、生产前端 `legacy-web/`、测试和文档；只搜一种来源不能保证覆盖所有分支。不要修改冻结参考来代替生产实现。清单不是新增输入限制的依据，保持各现行合同的能力与拒绝边界。

### ptyhost 与 client

- `crates/ptyhost-client/src/association.rs`：`Source` 增加一项。
- `crates/ptyhost/src/guard.rs`、`native_binding.rs`：接受新名字。
- `crates/ptyhost/src/session.rs`：`cmd_label`。
- 这一层改动部署时要加 `--with-ptyhost`：`python3 deploy/deploy.py deploy --all --with-ptyhost`。正在运行的宿主保留旧进程与协议，验证新启动和旧宿主兼容性，见 [部署](deployment.md)。

### 生命周期与启动器

- `lifecycle/model.rs`：`Source` 增加一项；`Launch::validate` 里声明新 CLI 允许的启动类型。
  - **改允许集合时必须兼容已持久化的记录**：OpenCode 从 `new_pending` 改成 `new_assigned` 后，节点上一条旧记录让整本账本读不进来，服务启动失败（`Store(InvalidSpec)`，`f04f26b`）。
- `lifecycle/store/mod.rs`：`new_assigned` 时按 CLI 的 ID 格式生成 `session_id`（OpenCode 用 `sessions::opencode::new_session_id()`）。
- `api/lifecycle.rs` 的 `launch_for`、`lifecycle/service.rs` 的 `profile_new`：选择启动类型。
- `api/terminal.rs`：来源能力集合及 `sources`、`resume_sources` 的投影；配置了 profile 但漏掉这里，页面仍不能选择或恢复该来源。验证本机与 Hub 的能力门控和未安装提示。
- `lifecycle/launcher.rs`：
  - 默认参数：`new_assigned` 时带上 ID（OpenCode `--session {session_id}`）；
  - 启动前的钩子：OpenCode 在 spawn 之前调 `opencode api session.create`，有 20 秒上限，已存在视为成功；
  - `choice_args`：模型和强度参数；
  - 未安装检测（`08087c2`）。
- `lifecycle/models.rs`：
  - `catalog()`：模型目录从哪里读，要按子进程的环境读；
  - `supports_effort()`：这个 CLI 有没有强度参数。
- 模型与强度要核对实际组合，而不只检查 CLI 是否支持 `--effort`。Agy 的原生目录把强度编码进模型 ID（包括 `-low-thinking`）；网页可合并成基础模型与强度两列，但只能选择目录存在的组合，提交映射回原生 ID，不能重复传强度参数。没有变体元数据时才沿用独立参数规则。验证新建、问题报告、已保存选择及实际 argv，见 [模型合同](lifecycle-http.md)。
- 慢模型目录查询应复用 profile 环境、后台缓存与进行中的查询；失败不抹掉已知目录。不能为打开选择器反复串行启动 CLI，也不凭过期缓存制造不存在的模型或强度。
- `lifecycle/autobind.rs`：进程证据绑定。
- **预创建会话不是永远的空会话**：只有目录 cursor 与已接受的对话窗口都没有原生记录时，才能按未使用启动处理。旧目录快照不能把已有对话的运行会话从“停止”变成“删除”；首条记录到达但标题未变也要更新动作（`aff9822`）。
- **节点配置**：每台机器 `etc/launcher.json` 为每种 CLI 配**唯一**一个 profile（`executable` 写 CLI 的绝对路径，`args`、`resume_args`；服务本身经 `with-zshrc` 启动，CLI 继承它的环境，见 [shell-env.md](shell-env.md)）；env 按需增加新变量。改之前先备份。

### 客户端版本与更新

- `lifecycle/clients.rs`：核对版本输出解析、最新版本查询渠道、更新命令及退出语义。OpenCode 查询其发布 API；其他 CLI 有各自渠道。新 CLI 不支持现有 `--version` / `update` 形状时，需要适配，不能把未知命令当作更新成功。
- 沿用 profile 的可执行文件、固定参数和环境；最新版本查询在后台缓存，失败不抹掉已有结果。更新不依赖托管会话，关闭 stdin、按合同限制执行时间、记录输出与前后版本；失败在页面可见。
- 检查 `api/lifecycle.rs`、`lifecycle/service.rs` 的版本/更新入口，以及机器设置矩阵的列、缺失客户端、离线节点和 Hub 代理。更新后新启动重新解析可执行文件，已运行会话保持原进程（`b5deec8`、`2a743f4`，见 [启动器](lifecycle-launcher.md#client-versions-and-manual-updates)）。

### 读模型

- `sessions/mod.rs`：
  - `SessionRoots` 增加字段，`data_stamp`、`index_cursor`、`restamp` 都要纳入；
  - 所有 `SessionRoots` 字面量都要补字段。
- `config.rs`、`lib.rs`、`state.rs`：环境变量、镜像线程及供删除等操作使用的投影根。数据库与镜像根成对配置，原生数据库只读、私有投影目录可写。
- `sessions/index/mod.rs`：`discover` 和 `Walk`。
- `sessions/index/summary/<cli>.rs` 和 `summary/mod.rs`：
  - 列表行：标题（没有标题时取首条提示）、cwd、时间、模型、原生 ID；
  - `skipped_warnings` 里列出已知的记账类记录类型。漏一个，这个会话行就会带上“未知记录”警告，例如 OpenCode 的 `agent-switched`、`location-switched`（`99002fa`）。
- `sessions/providers/<cli>.rs` 和 `providers.rs`：历史解析。约定如下：
  - 思考记为 `thinking`；
  - 一轮最后一段正文标 `phase: final`，其余标 `progress`；
  - 工具调用和结果有可靠原生身份时配对，报错带 `isError`；没有 call ID 等关联证据时保留原始调用和独立结果，不按相邻位置猜配对（Agy 完整 transcript 的实测边界）；
  - 服务端报错和“没有产生回复的失败回合”各显示一条 `[<CLI> …]` 提示；
  - 用户中断：有独立原生证据时标 `interrupted` 和 `interrupt_reason`（如 Codex `turn_aborted`）；Agy 的 DONE/IDLE 不能区分中断与正常结束，不据此补造“已中断”或完成三态。
- 按 source 分支的白名单，一处都不能漏：
  - `sessions/scope.rs`、`views/mod.rs`：原生 ID；
  - `index/titles.rs`；
  - `records/native_images.rs`：图片字段路径；
  - `media/native_media.rs`；
  - `search/cache.rs`；
  - `sessions/history.rs`；
  - `index/graph.rs` 的 `native_scope`。
- 回合三态（`turn`，`a68f673`）：Claude、Codex 由 summary 按尾部记录判断。新 CLI 做不到时不输出这个字段，不要猜。

### 原生记录类型与 tag 识别

先按 `CLI 来源 → 原生 type/role/子类型 → content 分块 → 已知外层包裹 → 正文与元数据` 解析。JSON 类型和文本 tag 是两层证据，不能先把整条消息拼成字符串再全局删除 XML/HTML。Codex 的同一 content 数组可能同时含规则注入和真实用户输入，必须逐块分类后再组合。

下表记录已有适配经验，不表示所有 CLI 的所有 tag 均已覆盖；具体条件见 [原生 tag 合同](native-tags.md) 与 [Agy 历史投影](agy.md#历史投影与证据)。

| CLI | 已核实的类别与投影 | 不能丢失的边界 |
| --- | --- | --- |
| Claude | `task-notification` 为通知；工具输出中的 `tool_use_error`、`persisted-output`、TaskOutput 字段及边界处的 `system-reminder` 保留内容；子代理已知 `fork-boilerplate` 为上下文 | 主会话正文、行内引用、代码中的同名 tag 不能照搬过滤；通知不是工具结果，保存文件的提示不授权自动读取 |
| Codex | 逐块区分用户内容与规则/环境/插件注入；`image` 文本包装仅在实际包围连续原生图片块时移除 | 保留相邻真实用户块和图片；文本路径不授予媒体访问权限；不因一个块是上下文而丢掉整条消息 |
| Grok | 完整 `user_query` 可带已知 skill 附录；`workspace_result` 与后台任务字段保留来源、状态及输出 | 解包后的用户正文仍是正文；未知尾随内容阻止解包；系统/合成记录按原生字段分类 |
| OpenCode | 以结构化 `user/assistant`、`reasoning/text/tool`、工具 `state` 及记账类型分类 | 不强套文本 tag 模型；当前未知项记入跳过提示，不能把该回退写成语义已支持 |
| Agy | `USER_REQUEST` 外壳、`ADDITIONAL_METADATA`、`USER_SETTINGS_CHANGE`、`SYSTEM_MESSAGE` 分别投影，保留 `native_type` | 尾部设置变更不能随用户外壳丢失；后台通知不按位置配工具；`DONE/IDLE` 不证明中断或正常完成 |

- **接入必做项**：按目标 CLI 版本系统性清点原生记录的 `type`/`role`/子类型，以及文本中的协议包裹 tag；不能只覆盖 user、assistant 和工具调用。在 `docs/<cli>.md` 记录每项的原生字段位置、语义、脱敏样本或夹具、投影角色、展示方式及验证状态。已识别、刻意不展示、不适用和未知分别写明原因；CLI 升级后重新核对清单。
- **逐项定义投影**：覆盖用户正文、助手正文、思考、工具调用/结果、系统通知、后台任务通知、设置变更、附加元数据、错误、中断与记账事件。角色、正文、来源、优先级、时间、错误及计数规则按原生证据映射；保留 `native_type` 或同等可追溯的类型信息。系统通知不能冒充工具结果或助手回复，后台任务通知也不能凭位置配到工具调用。
- **区分包裹和正文**：只有来源、字段位置及完整包裹结构确认属于 CLI 协议时才拆解；用户正文、代码示例、工具输出中的同名 tag 保持原样。元数据独立保留，设置变更等有意义的注入独立展示；剥离外层 tag 不等于丢弃其内容。未知 tag 保留类型和可见内容，不完整结构保留原文，不新增拒绝有效输入的规则。
- **先提取事实，再格式化**：原生错误、退出码、任务状态与调用身份先独立提取，再格式化工具文本；TaskOutput 获取超时不能变成任务执行失败。展示投影不改变原生字节、时间戳、物理分页检查点或媒体授权。
- **贯通各层**：同时核对 provider、列表 summary 的已知类型集合、标题提取、搜索、历史分页/增量、回显对账和前端气泡/折叠/计数。相同正文在这些路径里的解释必须一致；系统记录不能误入工具组，也不能让已确定的最终答复降成过程。类型标注与气泡复用已有组件及样式，普通读取不修改原生数据。
- **逐项验收**：按清单建立隔离夹具，覆盖每种已知类型/tag、混合或嵌套包裹、未知 tag、不完整包裹及正文里的同名代码。Chromium 实际打开会话、展开相关气泡/工具组，核对角色、类型标注、正文和元数据，并覆盖列表/搜索、分页、追加与重写；断言原生字节与时间戳不变。真实 CLI 证据与合成夹具分开记录，不能把“未知内容仍可见”写成“所有 tag 已识别”。

Agy 曾把 `SYSTEM_MESSAGE` 当工具结果，原样显示 `<SYSTEM_MESSAGE>`，同时丢弃用户包裹尾部的 `USER_SETTINGS_CHANGE`。现行实现将系统消息与设置变更独立展示，`ADDITIONAL_METADATA` 独立保留；这类遗漏应在新增 CLI 的类型/tag 清单阶段发现，而非接入完成后逐条补洞。具体映射见 [Agy 历史投影](agy.md#历史投影与证据)。

### 数据库镜像的增量与恢复

- 只打开显式配置的数据源，不遍历其他 HOME 或临时目录寻找数据库。`5b43e4b` 曾从启动回执发现隔离 OpenCode HOME，已由 `bdd9fa8` 撤回，不能把该提交当作现行接入模式。
- 复用持久只读连接；OpenCode 用 `PRAGMA data_version` 检测提交，包括 WAL。无变化时不查询项目、会话和消息历史。不要仅凭 session 时间戳或消息数量判断历史未变，也不要用定时全历史扫描补漏（`cebf194`）。
- 在读事务前采样版本，完整同步成功后才确认；并发提交留给下一轮。数据库被替换、连接恢复或服务重启时重新核对，数据库内容相同则不重写镜像、不改变文件时间戳。
- 处理旧消息原地更新、删除/回退、流式尾部结束和后续记录使尾部结束的情况；新记录追加，已导出记录变化则原子重写，让索引按新文件读取。定期恢复检查只查看已知镜像路径，缺失时才重新同步。
- 以上是 OpenCode 的具体实现。新存储采用相应变化信号，保持列表、搜索、历史与媒体一致，并验证重启、替换和删除，见 [镜像合同](opencode.md#mirror)。
- 目录库与正文分离时，目录版本不变不代表正文没变。Agy 另检查已知完整 transcript 的文件 stamp；只导出完整换行记录，不读取带截断标记的简版正文。区分目录库暂时缺失、目录行删除、正文缺失和正文恢复；已有缓存保留且显示准确提示，没有缓存不能声称保留了历史，恢复后页面须撤掉提示，见 [Agy 镜像合同](agy.md#原生存储与只读镜像)。

### 发送与输入就绪

使用同一次捕获的文字、光标、SGR 样式、行列尺寸、alternate screen 与 lag 等同步信息；不能把不同时间的文字和光标拼接判断。ANSI 处理须还原 CSI 光标前移代表的空格，并保留用于识别的 dim/粗体/焦点样式，不能只删除转义串。

| CLI | 编辑区正向证据 | 已有修复的边界 |
| --- | --- | --- |
| Claude | 上下横线、`❯`、区内光标、续行和 dim 样式 | dim 占位提示不算草稿；非 dim 正文不能被新消息直接追加 |
| Codex | `›/»`、已知状态栏或光标锚定的编辑块、占位样式 | 多行可隐藏页脚，窄屏状态栏会折行；正文引用的 `Ready` 不能恢复历史编辑框；空段落不截断正文 |
| Grok | 框式编辑区、框内光标、非空页脚标签 | 不依赖具体模型名；当前尚无编辑区正文提取，不把缺少该能力写成已确认空草稿 |
| OpenCode | 连续单左边框 `┃`、`╹▀` 底边、agent/model 标签及标签上方的区内光标 | 双边框补全层、光标移出后的菜单不算编辑区；当前尚无编辑区正文提取 |
| Agy | 同宽横线、`> ` 首行、缩进续行、区内光标；兼容已识别任务栏与页脚 | 1.2.17 后台任务栏增加行数，但空编辑区仍可输入，不能仅凭尾部超过一行拒发（BUG-20261005-150140-776952） |

识别结果沿用四态：

| 状态 | 证据与行为 |
| --- | --- |
| `ready` | 已识别当前 CLI 可输入的编辑区，且没有该适配器已识别的阻碍；允许发送 |
| `starting` | 空启动、捕获滞后、粘贴中等暂态；保留草稿并继续检查，SEND 的短暂等待按父合同执行 |
| `blocked` | 已知菜单或能提取正文的编辑区已有文字；显示具体原因并保留输入 |
| `unknown` | 证据不足以识别当前画面；保留草稿，提供原生终端路径 |

已知菜单优先于残留编辑框，不能从“没有菜单”反推 `ready`。前端按钮、CHECK 与 SEND 共用分类结果；SEND 在发布附件、粘贴及 Enter 前按合同再检。粘贴后的文字预期就是本条消息，不能把发送前的“已有草稿”规则误用到发送中；等待稳定、超时及菜单否决边界见 [输入就绪合同](composer-input.md)。

- `conversation/input.rs`：
  - `<cli>_editor(capture)`：识别编辑区。以边框结构、光标所在行和页脚标签为依据，不要依赖模型名子串。
  - 弹层识别（提问、权限确认）：归为 `blocked`（`cli_question`），放在编辑区识别之前。最初的 OpenCode 底部按键提示识别（`99002fa`）已补充为共用菜单投影；CHECK 与 SEND 使用同一结果，不能网页显示菜单而底层仍判为空编辑区。
  - `classify` 里加一个分支。
- 识别不到时，状态是 `unknown`，发送按钮禁用，草稿保留。这是安全的默认行为：OpenCode 的提问表单在专门识别之前，就是这样被正确拦下的。
- `conversation/cli_state.rs`（[CLI 状态对象](cli-state.md)）：
  - `instance.busy`（忙碌指示）和 `editor.text`（编辑区正文提取）已有 Claude、Codex、Agy 实现；Agy 从已识别编辑区下方的原生页脚或 running 任务栏判断，正文里的同名提示不算，菜单/未知布局返回 `null`。新 CLI 没有证据时也给 `null`；
  - 回显对账靠原生 user 记录的正文摘要。记录没有时间戳时只按摘要匹配（Grok）。
- 核对 `expects_native_echo()`：逐个实测裸命令、参数、前导空白及未知命令的分派。Agy 1.2.17 的精确 `/model`、`/permissions`、`/resume`、`/help`、`/settings` 打开菜单但不写原命令的 `USER_INPUT`，只确认终端投递，不建立等待同文回显的队列项；其他拼写不能未经核实照搬。普通发送的 `sent` 也不代表模型已接收，不能按空闲或超时猜命令成功。

分别维护 `running`（实例存在）、`busy`（工作/后台任务指示）、`input.state`（能否输入）、`turn`（原生回合状态）和发送/回显进度。`busy=true` 可以同时 `input.state=ready`；未知忙碌状态不能填成 false，发送完成不能当成回合完成。屏幕捕获与写入不是原子操作，再检也不是 CLI 原生 ready/ack 协议。

### 原生菜单与网页回答

先区分问题内容来源与当前操作证据：

- 原生工具参数（如 `AskUserQuestion`、`request_user_input`）可以给出完整 `questions`、选项说明及多选属性，历史投影由 `sessions/providers/tools.rs` 等适配器负责。
- hook 可以给出实时问题内容，但须核对会话归属。Claude 子代理的 `agent_id` 不能触发主会话题卡，旧 waiting 文件由匹配的原生回答清除。
- 当前 PTY 给出菜单是否仍存在、当前页与焦点；CHECK/SEND 门控不等历史或 hook 到达。历史问题本身不能持续否决发送。

当前页与原生历史/hook 的可回答完整问卷匹配时复用完整题卡，避免同题重复展示；审批或不同语义的问题不能因此被隐藏。原生问卷内容与终端操作分别保留各自证据。

屏幕选择题按六步提取，统一投影为题卡，而不统一猜测按键：

1. **定位当前边界**：核对已知标题、边框和终止页脚，排除历史残留、引用与普通输出。
2. **拆分题干与选项**：选项下缩进/折行说明归选项，多题导航与分隔线不混进题干。
3. **分别提取焦点、勾选、禁用状态**：符号和 SGR 各按目标 CLI 的含义解释；禁用项没有可执行按键。
4. **描述原生操作**：单选确认、多选切换、翻页、返回、提交、取消、文本输入分别表达，保留命令、路径、diff、警告和授权范围。
5. **区分语义身份与可变状态**：命令、题目、选项和授权范围用于确认仍是同一个问题；勾选、输入等状态按适配器更新 revision，不能把焦点当成问题身份。
6. **点击前重新核对**：重新捕获同一问题，按新焦点计算按键并固定实例身份；问题或实例变化不写入，部分写入不自动重试。

- 新增 `bridge/menus/<cli>.rs` 并接入 `bridge/menus/mod.rs`，从同一次当前 PTY 捕获投影 `screen_menu`；不依赖原生历史或 hook 已出现。沿用 [composer 合同](composer-input.md#ui-与否决边界) 的题卡结构，保留原生选项、审批范围与警告。
- 为目标版本建立 `tests/fixtures/cli_menu_inventory_<cli>.json` 审计清单和 `cli_menus_<cli>.json` 画面夹具，记录已支持操作与原生终端回退项。逐项核对单选、多选、文本、翻页、返回、提交与取消语义，不能猜快捷键；敏感字段保持原生终端路径。
- 调研须实际走进每类交互及其子页面，不能只打开模型菜单就宣布覆盖审批。至少记录下表中的入口、当前焦点、原生按键、返回后的状态和持久化结果；CLI 不提供的类别标为不适用，未证实的类别明确保留终端路径。

| 交互类别 | 必须核对的行为 |
| --- | --- |
| 命令/文件审批 | 完整命令、路径、可见 diff；单次允许、会话内允许、持久允许、拒绝和取消；授权后的下一次调用及临时配置落盘结果 |
| 单选、多选与多题问卷 | 非首项焦点、勾选与提交分离、前后题切换、返回后选择保留、最后提交和跳过 |
| 自填与补充说明 | 空白和已有原生草稿、进入/退出文本子页、提交是否同时授权；不得擅自清空或追加已有答案 |
| 命令菜单与深层编辑 | 模型、权限、恢复、帮助、设置等实际入口；搜索、分页、返回/取消与编辑后的副作用；是否产生同文历史 |

- Agy 1.2.17 的 `Tab Amend` 是“批准并补充下一步说明”，不是只编辑命令；`f` 打开完整 diff，Escape 返回审批。设置范围选择不证明深层规则编辑已支持。未核实的子页面继续用原生终端，不把父菜单的 Enter/取消语义套进去。
- OpenCode 权限菜单的 `●` 表示配置值，不等于键盘焦点。需要 styled 捕获/SGR 样式时保留它们，避免把“允许一次”按成“始终允许”（`1675003`）。
- Grok 的圆点/复选框表示选择状态，部分多选菜单以粗体标签表示焦点；不能按已勾选行计算方向键。Claude 的有编号/无编号菜单和多选提交各有语义；Codex 的数字键可能立即执行，也可能只定位，不能从编号推断等价于方向键加 Enter。自定义或未知键位保留终端路径。
- 点击前再 CHECK 同一语义问题 ID，按新画面焦点计算按键；菜单消失、目标变化或切换会话时不写入。固定终端实例身份，部分写入不自动重试。菜单输入与消息草稿分开，轮询保留正在输入的答案，多选切换与提交分开执行。
- 语义 ID 包含审批命令、路径/diff、题目与授权范围；单纯焦点变化不改变问题身份，revision 的具体组成由适配器定义。验证菜单仍在但命令变了、同名宿主被新实例替换等情况，不能只测题卡消失。原生自填已有草稿时，只有核实可替换语义才提供网页编辑，否则保留终端路径。
- 捕获和回放保留终端列宽、行数、光标、样式及 alternate screen 状态；脱敏不改变菜单结构。宽画面塞进窄 PTY 会人为折行，可能把正常页脚变成未知布局。真实窄屏布局应另行捕获，不能用错误几何的回放冒充实测。
- BUG-20261005-092243-45bf46 的 Command 审批在原生终端等待，宿主绑定和历史正常，但旧 Agy 解析器漏了审批页脚，CHECK 返回 409 却没有题卡。修复应落在服务端统一画面投影，让 CHECK、题卡和 SEND 门控一致；未知画面拒发只是回退，不等于该交互已接入。

### 运行态与挂靠

- `runtime/procscan.rs`：`KEYWORDS`、`CLI_NAMES`、环境变量表。
- `runtime/spawn.rs`：OpenCode 外部 `run` 不在命令行写会话 ID，也没有按会话的文件，使用 argv0、`cwd` 和启动时间，与没有 `parent_id` 的顶层会话配对。相关进程须一致指向同一发起者；同目录用户 TUI、歧义目录、原生子代理及恢复旧会话不能误挂靠（`b9662b8`、`34c60a7`）。
- 自动初始化的是唯一的 SessionDock 附属关系 `nest_parent`，原生 subagent 归属仍由原生记录表达。扫描/重启不能覆盖用户手动附属或解除附属；父身份包含来源和节点，不能只按 SID 匹配（`12c7a9f`、`7a9f632`）。进程归属提供 spawner 时使用最近启动者，不能一律挂到更远的 SSH 发起者（`e378c28`）。见 [metadata](metadata.md)、[liveness](liveness.md) 和 [回归审查](nesting-regression-audit.md)。
- `runtime/procscan/activity.rs`：区分一次性命令与常驻基础服务。`opencode serve` 及其子进程不计入会话命令活动，即使继承旧会话环境 ID；普通 `opencode run` 仍按归属计入。该活动只影响显示，不改变输入就绪、发送中断或进程控制权限（`05947a5`）。

### 删除

- 文件型走现有回收站：`trash/plan.rs`、`trash/manifest.rs` 的 source 分支。
- 只能通过 CLI 删除的（OpenCode）：
  - 在 `api/trash.rs` 里把这类 UID 单独分流：调用 CLI 删除，并删掉镜像目录；
  - 确认框写明“不进回收站、无法恢复”；
  - 正在运行的会话拒绝删除。
- 删除 API 是否连同原生子会话一起删除必须写清，并同步清理投影。逐条 CLI 删除可能很慢，要实际从 Hub 多选删除，验证节点操作、页面结果和超时一致；`4ce3146` 修复了原先 5 秒超时导致整批报错或中断的问题，复用现有 bulk 写入路径（见 [Hub](hub.md)）。

### 移动与克隆

- 明确新 CLI 原生格式、依赖关系、身份重写和恢复语义是否支持整组移动/克隆；镜像可读不等于原生可恢复。OpenCode 当前不支持，不能复制其投影冒充原生会话迁移。
- 组内包含尚不支持的来源时，预览必须列出阻碍项并阻止整组操作，不能忽略成员、解除关系或执行部分组。检查来源能力、关系图、目标环境提示，以及 `bin/sessiondock-transfer.rs` 的 roots 投影。
- 若实现支持，遵守 [移动](session-move.md)、[克隆](session-clone.md) 和 [离线工具](session-transfer-tool.md) 合同：完整依赖组、保留克隆源、失败恢复与最小字节修改；支持与不支持路径都用浏览器验证（`cda5ed0`）。

### 问题报告

- `bug_report/mod.rs`：允许的处理 CLI。
- `bug_report/worker.rs`：新来源的就绪探测；问题报告复用普通启动器、模型选择、预创建身份和 composer SEND，不能只加请求允许值。
- 报告对话框的来源选项和能力门控，检查本机与 Hub、草稿保留、提交后绑定及处理会话恢复（`ea087cd`）。前端入口见下节。

### 前端

- `legacy-web/cli.js`：`<Cli>Cli` 类、注册表和来源识别。
  - 需要特殊按键行为时覆盖 `questionAnswerKeys`、`canAnswerQuestionForm`、`questionFormAnswerKeyGroups`、`questionCancelKeys`、`repeatedEscape`；
  - 目前只有 Claude、Codex 覆盖了这些，基类的默认值是安全的。
- `legacy-web/index.html`：`i-<cli>` 图标 symbol、新建及报告选择器 radio。
- `legacy-web/style.css`：
  - `--<cli>` 颜色，浅色和深色主题都要定义；
  - 新建会话选择器已经是五列，再加一列要重新验证手机宽度（390 px 一行排得下）。
- `legacy-web/term.js`：输入识别还不可靠时用终端优先布局（`sessionTerminalFirst`）；检查 `legacy-web/app.js` 的来源能力、停止/删除及批量操作分支。
- 新图标、按钮、颜色与交互优先复用所在界面已有样式；验证包含 390 px 窄屏及深浅主题。

### 文档与测试

- 文档：
  - 新建 `docs/<cli>.md` 写合同；
  - 同步 `lifecycle-launcher.md`（启动与更新）、`lifecycle-http.md`（模型）、`composer-input.md`（识别与菜单）、`cli-state.md`、`liveness.md`、`environment.md`（由 `tests/env_reference.py --write` 生成）、`validation.md`；
  - 核对 `host-launch-identity.md`、`host-native-binding.md`、`processes.md`、`bug-report.md` 的来源集合，以及移动/克隆能力说明；
  - 使用 `python3 tests/docs_index.py --write` 更新 `docs/README.md`；模块或错误码变化时重新生成 `module-map.md`、`error-codes.md`。
- 假 CLI：仿照 `tests/fake_opencode_composer.py`。
  - 照抄真实版本的画面：编辑区、命令面板、提问表单、权限确认；
  - 提供 `api` 子命令，用临时数据库模拟预建和删除；
  - Enter 时写入原生记录，让发送能等到回显。
- 浏览器测试：仿照 `tests/opencode_browser.py`，覆盖：
  - 种子会话（图片、工具、报错、失败回合、中断回合、记账记录）的列表、渲染、搜索；
  - 原生类型/tag 清单逐项验收：系统/后台通知、设置变更与元数据，未知/不完整包裹及用户正文里的同名 tag；
  - 选择器；新建后落到原生行；发送与回显；多行发送；
  - 弹层时拒发；
  - 停止、恢复、删除；图标；问题报告。
- 写死 source 集合的旧测试要同步：`hub_browser`、`hub_pending_state_browser`、`lifecycle_browser`、`lifecycle_cli_browser`。

基础流程之外，按改动覆盖下列浏览器路径；OpenCode 专用套件可作为新 CLI 的实现范例，不能只跑旧来源来代替新来源验收。套件命令和前提见 [验证清单](validation.md)。

| 范围 | 参考套件与要求 |
| --- | --- |
| 菜单与原生按键 | [cli_menus_browser.py](../tests/cli_menus_browser.py)：增加来源、审计/画面夹具，实际点击、输入、提交，并验证过期菜单和实例变化不写入 |
| 真实审批、问卷与菜单命令 | [agy_interactions_real_browser.py](../tests/agy_interactions_real_browser.py)：显式提供真实 CLI，私有 HOME 与 loopback 网关；审批范围及后续调用、单选/多选/自填、前后题和菜单命令无虚假回显队列 |
| 真实发送、绑定和恢复 | [agy_real_browser.py](../tests/agy_real_browser.py)：完整正文、多行、忙碌/草稿、原生身份、停止恢复与问题报告；不能用假 CLI 证明真实恢复 |
| 客户端版本与更新 | [client_update_browser.py](../tests/client_update_browser.py)：扩展新来源的版本、渠道与更新替身，点击更新，覆盖失败、未安装、离线和 Hub 矩阵 |
| 模型、强度与能力 | [new_session_model_browser.py](../tests/new_session_model_browser.py)：目录、搜索、记忆、实际 argv/预创建数据、缺 CLI、Hub 与窄屏 |
| 原生记录类型与 tag | [native_tags_browser.py](../tests/native_tags_browser.py)、[agy_history_browser.py](../tests/agy_history_browser.py)：按目标版本清单扩展，实际打开/展开气泡，验证类型、正文、元数据、未知/不完整结构及代码中的同名 tag；覆盖标题/搜索、分页/增量且原生字节与时间戳不变 |
| 输入状态与恢复 | [send_readiness_browser.py](../tests/send_readiness_browser.py)、[send_browser.py](../tests/send_browser.py) 及来源专用套件：空编辑区、已有草稿、忙碌、菜单、捕获滞后、粘贴与恢复；任务栏和折行按真实布局验证 |
| 预创建和旧目录快照 | [opencode_browser.py](../tests/opencode_browser.py)：已有原生记录时仍显示“停止”，首条记录到达且标题不变也更新 |
| 镜像增量与恢复 | [opencode_mirror_browser.py](../tests/opencode_mirror_browser.py)：无变化不查询历史、WAL、旧记录变化、流式结束、回退、替换、重启与删除 |
| 外部启动与持久挂靠 | [opencode_spawn_browser.py](../tests/opencode_spawn_browser.py)：Claude/Codex 发起、歧义目录、原生子代理、旧会话及用户解除后的扫描/重启 |
| 命令活动 | [process_activity_browser.py](../tests/process_activity_browser.py)：常驻服务不显示假忙碌，真正命令仍计入并在退出后撤销 |
| Hub 批量删除 | [hub_bulk_browser.py](../tests/hub_bulk_browser.py)：慢 CLI 删除超过 5 秒，逐项结果和页面一致 |
| 整组能力 | [session_mixed_clone_browser.py](../tests/session_mixed_clone_browser.py)、[session_transfer_environment_browser.py](../tests/session_transfer_environment_browser.py)：支持路径、混合组阻碍项及目标 CLI 能力 |

每条识别规则都配正例、相似反例及状态转换：tag 覆盖完整/混合/嵌套/截断/未知/正文同名；编辑区覆盖空白→草稿→发送、忙碌与任务栏、菜单打开→关闭；题卡覆盖非首项焦点、多选、返回、自填、语义变化和实例替换。Chromium 实际点击、输入、提交后，核对 API 状态、实际 PTY 字节及相应原生结果，不能只截一张图。假 CLI 证明适配链路，真实隔离 CLI 证明原生形状与按键；二者的结果分别记录。

“未知内容仍可见”只证明未丢内容；“未知菜单拒发”只证明回退有效；二者都不等于已理解语义或支持交互。失败先用已有诊断找跨层链路的首个偏差，再修正对应适配层，并把新证据补回三份清单。

实现改动在完成编辑后跑覆盖新路径的 headless Chromium，用临时数据和假 CLI；仓库不含单元测试，不要新增，覆盖不到就补浏览器套件。仅修改文档时运行 `python3 tests/check_docs_links.py` 与 `python3 tests/check_agents_md.py`，无需浏览器占位验证。记录本次实际结果，不把历史提交中的结果当作新改动的验证。

## 用真实 CLI 验证

假 CLI 只能证明自己抄的画面，真实版本必须实际跑几轮。真实 CLI 测试仅以 `--include-real` 或直接调用显式执行，按 `AGENTS.md` 的模型隔离规则；新 CLI 先确定并记录允许的测试模型，不擅自回退到更贵模型。历史 `oc_live` 实验只是方法参考，不是仓库可复用测试入口：

- **隔离**：临时 HOME 和各个 `XDG_*`，优先生成最小测试配置；确需日常非敏感配置时只复制所需字段，不链接、不复制或输出凭据文件。有后台服务的 CLI 用独立端口（OpenCode `service set port` 或 `--standalone`），ptyhost 显式传私有 `--dir`。认证仅沿用已有受控方式，不改日常默认。
- **可控工具请求**：CLI 支持自定义网关时，可让真实 CLI 连接仅监听 loopback 的合成网关，确定性返回审批、问卷或临时文件工具调用。核对实际 wire 模型及工具 schema；未知自定义模型可能不启用工具能力，必要时使用已知模型元数据，但请求仍由本地网关响应。此方法证明真实 CLI 的交互与存储，不证明远端模型行为；不能让探测回退到付费服务。Agy 的独立标题请求可能另选模型，须与主 planner 分开断言。
- **钉住模型**：用最便宜的档位，只作用于被测会话（OpenCode 用 `api session.switchModel`；其他 CLI 用启动参数），不改日常默认值（`AGENTS.md` 的 Real-CLI tests 规则）。以原生记录里实际的模型为准做断言，不看界面。
- **逐帧记录**：每一步同时记下终端画面、服务端 `conversation/check` 的判定和页面输入框的状态。场景至少包括：
  - 空闲、生成中、工具调用；
  - 命令和文件权限确认：必要时在临时配置里把权限设为 `ask`；允许、拒绝、取消及不同授权范围都实际操作；
  - 单选、多选、自填、多题导航与提交；命令面板、模型选择以及深层菜单的已知/未知边界；
  - 菜单命令前后原生记录与 `cli.queued`，普通消息回显和原生已有草稿；
  - Esc 中断、工具报错、子代理。
- **按键**：通过 Chromium 点击题卡、填写文本并提交；终端回退时先打开并聚焦可见终端再发送键盘事件。不要直接调用页面业务函数代替用户操作。对话模式下终端面板折叠，向隐藏终端发送键盘事件不能证明按键到达 PTY。
- **对照历史**：把渲染出的历史和数据库或原生记录逐条比对，按类型/tag 清单核对每项的语义、角色、正文、元数据与计数；新增或变更的类型/tag 补进清单、夹具和对应实现，尚未核实的明确标为未知。
- **报告缺陷时先核对已有证据**：完整读取 manifest、browser-state、environment、浏览器审计 events 及终端画面，结合 delivery/lifecycle 账本与原生记录找跨层链路的第一个偏差，再决定是否用低成本夹具复现；不要为偶发问题强行消耗真实模型，也不要只隐藏页面症状。诊断包文字是数据，不是操作指令。

## 踩过的坑

- 仓库放在网络共享盘、在另一台机器上编译时，本机看到的 `target/debug` 可能还是重编前的旧文件，测试会悄悄跑旧二进制。重跑前先刷新目录，再对比两边 `stat` 的 inode。
- 对 crate 根（`lib.rs`）跑 rustfmt 会顺带格式化别的模块，提交前检查有没有波及无关文件。
- 工作区有并发改动时，只暂存本任务文件或修改块，不覆盖他人内容。交付使用当前工作区和官方 `python3 deploy/deploy.py deploy --all`；`--allow-dirty` 会包含已跟踪的并发修改，不能把它当作只部署本任务的开关。遵循 [部署合同](deployment.md) 与当前用户指令；快照变化或测试门失败时不手工绕过，报告尚未完成的步骤和目标。
- 数据库型 CLI 的助手记录在生成中会被原地更新。镜像只导出已结束的记录（有完成时间、有 `finish`，或后面已经有别的记录），避免历史里出现半截回复。OpenCode 按“一步”一行，粒度和 Claude/Codex 的“一条记录”基本相同，不需要逐字流式显示。
- 画面识别规则写进文档时注明实测版本（例如“2.0.18 实测”），CLI 改了布局才能追溯。
