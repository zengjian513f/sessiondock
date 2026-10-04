# 接入新的 AI CLI

本文把 OpenCode 接入及后续修复整理成新增 AI CLI 的清单；2026-10-04 对照主线至 `7dabf54` 核对，包含菜单交互、客户端更新、镜像增量读取、会话挂靠和 Vue 迁移入口。每项都要明确实现或记录不适用原因。现行合同以各专题文档为准：[OpenCode](opencode.md)、[启动器](lifecycle-launcher.md)、[生命周期 HTTP](lifecycle-http.md)、[对话输入就绪](composer-input.md)、[CLI 状态对象](cli-state.md)、[读模型](read-model.md)、[liveness](liveness.md)、[移动](session-move.md)、[克隆](session-clone.md)。

## 先调研，再定路线

动手前先在隔离环境（临时 `HOME`/`XDG_*`，独立服务端口）里把下面的问题答清楚，并记录版本号。答案决定每一层走哪条路。

| 问题 | 决定什么 | 已有 CLI 的答案 |
| --- | --- | --- |
| 目标版本怎么装 | 安装脚本与版本 | OpenCode 接入时普通安装入口给 1.x，2.x 使用 `/v2/install`；这是当时的调查记录，新增 CLI 时重新核对目标版本和渠道 |
| 怎么读取版本、查询最新版本、无交互更新 | 机器设置中的客户端矩阵 | 现有四种 CLI 用 `--version` 和 `update`；最新版本查询按各自渠道实现，新 CLI 不能直接假定同样支持 |
| 会话存在哪里、什么格式 | 读模型路线 | Claude/Codex 是 JSONL 文件，Grok 是目录加 `summary.json`，OpenCode 2 是一个 SQLite 库 |
| 会话 ID 何时确定 | 启动类型（`Launch`） | Claude 启动参数指定 UUID（`new_assigned`）；OpenCode 先 `api session.create` 再 `--session`（`new_assigned`）；Codex 启动后才知道（`new_pending`） |
| 怎么恢复 | `resume_args` | Codex `resume {sid}`，OpenCode `--session {sid}` |
| 模型、强度怎么传；模型列表在哪 | 模型选择 | 见下文“生命周期与启动器” |
| TUI 画面：编辑区、忙碌、菜单、提问/权限弹层、中断键 | 输入就绪识别与网页题卡 | 见下文“发送与输入就绪”和“原生菜单与网页回答” |
| 删除语义 | 删除/回收站 | 文件型走回收站；OpenCode 只能调 `api session.remove`，不可恢复 |
| 原生数据能否移动或克隆，身份与依赖如何重写 | 整组操作能力 | OpenCode 的镜像不是可恢复的原生文件，当前不支持移动或克隆 |
| 子代理/子会话怎么表示 | 列表挂靠 | Claude 子目录，Codex `parent_thread_id`，OpenCode `parent_id` 和进程关系 |
| 会话进程与常驻服务怎么区分 | 运行态、挂靠 | argv0、环境变量（`*_SESSION_ID`）、打开的会话文件；OpenCode 外部 `run` 按进程、cwd 和出生时间配对，`serve` 不算会话命令活动 |

路线选择：

- **会话是文件**：直接写 summary 和 provider，索引按文件增量读取。
- **会话在数据库或服务里**：仿照 `sessions/opencode.rs`，只读打开并镜像成文件（`summary.json` 加 `messages.jsonl`），下游的列表、历史分页、搜索、媒体全部复用文件管线，不另开一套读路径。
- **ID 能在启动前确定**：一律用 `new_assigned`，页面在启动时就能跳到原生行。只有做不到时才用 `new_pending`。

## 分期

- **一期**：能启动、能恢复、控制台可用、输入框能发。侧栏图标、颜色、选择器都要到位；页面可以先以终端为主（`sessionTerminalFirst`）。
- **二期**：历史、列表、搜索、媒体，启动即知身份，删除。
- **后续**：按 CLI 补模型目录、问题报告、菜单题卡、客户端版本与更新、CLI 状态（忙碌、编辑区文字）、回合三态、子会话挂靠；明确移动和克隆是否支持。

每一期完成浏览器验证后单独提交、推送，并立即部署、重启及健康检查。至少一台
SessionDock 节点和 Hub 成功即满足部署要求；其余节点记录待补发，不阻塞接入工程
或 goal，也不反复轮询。不要攒成一个大提交。

## 接入清单

下面覆盖 OpenCode 实际改动及后续共用能力。Rust 路径除另有注明外相对 `crates/sessiondock/src/`。使用 `rg` 同时查 `opencode`、`Opencode`、`grok`、`Source::Grok` 和来源枚举/集合，检查服务端、两套前端入口、测试和文档；只搜一种来源不能保证覆盖所有分支。清单不是新增输入限制的依据，保持各现行合同的能力与拒绝边界。

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
  - 所有 `SessionRoots` 字面量都要补字段：`src` 的测试里有，`crates/sessiondock/tests/*.rs` 有十几处；
  - 编译检查必须用 `cargo check --tests`，否则漏掉的测试字面量要等到别人跑测试才暴露。
- `config.rs`、`lib.rs`、`state.rs`：环境变量、镜像线程及供删除等操作使用的投影根。数据库与镜像根成对配置，原生数据库只读、私有投影目录可写。
- `sessions/index/mod.rs`：`discover` 和 `Walk`。
- `sessions/index/summary/<cli>.rs` 和 `summary/mod.rs`：
  - 列表行：标题（没有标题时取首条提示）、cwd、时间、模型、原生 ID；
  - `skipped_warnings` 里列出已知的记账类记录类型。漏一个，这个会话行就会带上“未知记录”警告，例如 OpenCode 的 `agent-switched`、`location-switched`（`99002fa`）。
- `sessions/providers/<cli>.rs` 和 `providers.rs`：历史解析。约定如下：
  - 思考记为 `thinking`；
  - 一轮最后一段正文标 `phase: final`，其余标 `progress`；
  - 工具调用和结果要配对，报错带 `isError`；
  - 服务端报错和“没有产生回复的失败回合”各显示一条 `[<CLI> …]` 提示；
  - 用户中断：把这一轮最后一条非终稿的助手消息标 `interrupted` 和 `interrupt_reason`（同 Codex `turn_aborted`），页面会显示“已中断”。
- 按 source 分支的白名单，一处都不能漏：
  - `sessions/scope.rs`、`views/mod.rs`：原生 ID；
  - `index/titles.rs`；
  - `records/native_images.rs`：图片字段路径；
  - `media/native_media.rs`；
  - `search/cache.rs`；
  - `sessions/history.rs`；
  - `index/graph.rs` 的 `native_scope`。
- 回合三态（`turn`，`a68f673`）：Claude、Codex 由 summary 按尾部记录判断。新 CLI 做不到时不输出这个字段，不要猜。

### 数据库镜像的增量与恢复

- 只打开显式配置的数据源，不遍历其他 HOME 或临时目录寻找数据库。`5b43e4b` 曾从启动回执发现隔离 OpenCode HOME，已由 `bdd9fa8` 撤回，不能把该提交当作现行接入模式。
- 复用持久只读连接；OpenCode 用 `PRAGMA data_version` 检测提交，包括 WAL。无变化时不查询项目、会话和消息历史。不要仅凭 session 时间戳或消息数量判断历史未变，也不要用定时全历史扫描补漏（`cebf194`）。
- 在读事务前采样版本，完整同步成功后才确认；并发提交留给下一轮。数据库被替换、连接恢复或服务重启时重新核对，数据库内容相同则不重写镜像、不改变文件时间戳。
- 处理旧消息原地更新、删除/回退、流式尾部结束和后续记录使尾部结束的情况；新记录追加，已导出记录变化则原子重写，让索引按新文件读取。定期恢复检查只查看已知镜像路径，缺失时才重新同步。
- 以上是 OpenCode 的具体实现。新存储采用相应变化信号，保持列表、搜索、历史与媒体一致，并验证重启、替换和删除，见 [镜像合同](opencode.md#mirror)。

### 发送与输入就绪

- `conversation/input.rs`：
  - `<cli>_editor(capture)`：识别编辑区。以边框结构、光标所在行和页脚标签为依据，不要依赖模型名子串。
  - 弹层识别（提问、权限确认）：归为 `blocked`（`cli_question`），放在编辑区识别之前。最初的 OpenCode 底部按键提示识别（`99002fa`）已补充为共用菜单投影；CHECK 与 SEND 使用同一结果，不能网页显示菜单而底层仍判为空编辑区。
  - `classify` 里加一个分支。
- 识别不到时，状态是 `unknown`，发送按钮禁用，草稿保留。这是安全的默认行为：OpenCode 的提问表单在专门识别之前，就是这样被正确拦下的。
- `conversation/cli_state.rs`（[CLI 状态对象](cli-state.md)）：
  - `instance.busy`（忙碌指示）和 `editor.text`（编辑区正文提取）目前只有 Claude、Codex 实现，新 CLI 可以先给 `null`；
  - 回显对账靠原生 user 记录的正文摘要。记录没有时间戳时只按摘要匹配（Grok）。

### 原生菜单与网页回答

- 新增 `bridge/menus/<cli>.rs` 并接入 `bridge/menus/mod.rs`，从同一次当前 PTY 捕获投影 `screen_menu`；不依赖原生历史或 hook 已出现。沿用 [composer 合同](composer-input.md#ui-与否决边界) 的题卡结构，保留原生选项、审批范围与警告。
- 为目标版本建立 `tests/fixtures/cli_menu_inventory_<cli>.json` 审计清单和 `cli_menus_<cli>.json` 画面夹具，记录已支持操作与原生终端回退项。逐项核对单选、多选、文本、翻页、返回、提交与取消语义，不能猜快捷键；敏感字段保持原生终端路径。
- OpenCode 权限菜单的 `●` 表示配置值，不等于键盘焦点。需要 styled 捕获/SGR 样式时保留它们，避免把“允许一次”按成“始终允许”（`1675003`）。
- 点击前再 CHECK 同一语义问题 ID，按新画面焦点计算按键；菜单消失、目标变化或切换会话时不写入。固定终端实例身份，部分写入不自动重试。菜单输入与消息草稿分开，轮询保留正在输入的答案，多选切换与提交分开执行。

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
- **已存在的 Vue 入口**：生产 legacy 与 Vue 预览分别按 [部署](deployment.md) 的 frontend 选择，不把这次接入当作另起一套 Vue 界面的授权。对照 [前端界面清单](frontend-migration-surfaces.md)，同步现有来源分支：
  - `web/src/domain/runtime/cli.js`（及类型声明）：CLI 类、注册表、来源识别；
  - `web/src/components/session-ui/NewSessionDialog.vue`、`BugReportDialog.vue` 与 `web/src/services/session-ui/launch.js`：选择器、报告来源和能力集合；
  - `web/migration/index.html`：图标 symbol；`web/src/services/terminal/controller.js`：终端优先和生命周期展示；
  - `web/src/components/session-ui/DeletedReceipt.vue`、`web/src/services/runtime/bulk.js`：特殊删除提示和批量操作。遵循现有 Vue 状态所有权，不重新在视图回调中实现会话同步。
- 新图标、按钮、颜色与交互优先复用所在界面已有样式；两种入口按实际发布范围验证，包含 390 px 窄屏及深浅主题。

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
  - 选择器；新建后落到原生行；发送与回显；多行发送；
  - 弹层时拒发；
  - 停止、恢复、删除；图标；问题报告。
- 写死 source 集合的旧测试要同步：`hub_browser`、`hub_pending_state_browser`、`lifecycle_browser`、`lifecycle_cli_browser`、`lifecycle_http_suite`、`meta_capabilities_suite`。

基础流程之外，按改动覆盖下列浏览器路径；OpenCode 专用套件可作为新 CLI 的实现范例，不能只跑旧来源来代替新来源验收。套件命令和前提见 [验证清单](validation.md)。

| 范围 | 参考套件与要求 |
| --- | --- |
| 菜单与原生按键 | [cli_menus_browser.py](../tests/cli_menus_browser.py)：增加来源、审计/画面夹具，实际点击、输入、提交，并验证过期菜单和实例变化不写入 |
| 客户端版本与更新 | [client_update_browser.py](../tests/client_update_browser.py)：扩展新来源的版本、渠道与更新替身，点击更新，覆盖失败、未安装、离线和 Hub 矩阵 |
| 模型、强度与能力 | [new_session_model_browser.py](../tests/new_session_model_browser.py)：目录、搜索、记忆、实际 argv/预创建数据、缺 CLI、Hub 与窄屏 |
| 预创建和旧目录快照 | [opencode_browser.py](../tests/opencode_browser.py)：已有原生记录时仍显示“停止”，首条记录到达且标题不变也更新 |
| 镜像增量与恢复 | [opencode_mirror_browser.py](../tests/opencode_mirror_browser.py)：无变化不查询历史、WAL、旧记录变化、流式结束、回退、替换、重启与删除 |
| 外部启动与持久挂靠 | [opencode_spawn_browser.py](../tests/opencode_spawn_browser.py)：Claude/Codex 发起、歧义目录、原生子代理、旧会话及用户解除后的扫描/重启 |
| 命令活动 | [process_activity_browser.py](../tests/process_activity_browser.py)：常驻服务不显示假忙碌，真正命令仍计入并在退出后撤销 |
| Hub 批量删除 | [hub_bulk_browser.py](../tests/hub_bulk_browser.py)：慢 CLI 删除超过 5 秒，逐项结果和页面一致 |
| 整组能力 | [session_mixed_clone_browser.py](../tests/session_mixed_clone_browser.py)、[session_transfer_environment_browser.py](../tests/session_transfer_environment_browser.py)：支持路径、混合组阻碍项及目标 CLI 能力 |

实现改动在完成编辑后跑覆盖新路径的 headless Chromium，用临时数据和假 CLI；不自行运行单元测试。编译检查不等于执行单元测试。仅修改文档时运行 `python3 tests/check_docs_links.py` 与 `python3 tests/check_agents_md.py`，无需浏览器占位验证。记录本次实际结果，不把历史提交中的结果当作新改动的验证。

## 用真实 CLI 验证

假 CLI 只能证明自己抄的画面，真实版本必须实际跑几轮。真实 CLI 测试仅以 `--include-real` 或直接调用显式执行，按 `AGENTS.md` 的模型隔离规则；新 CLI 先确定并记录允许的测试模型，不擅自回退到更贵模型。历史 `oc_live` 实验只是方法参考，不是仓库可复用测试入口：

- **隔离**：临时 HOME 和各个 `XDG_*`，从日常配置**复制**一份（不要链接），有后台服务的 CLI 用独立端口（OpenCode `service set port` 或 `--standalone`）。凭据从环境变量传给子进程，不打印。
- **钉住模型**：用最便宜的档位，只作用于被测会话（OpenCode 用 `api session.switchModel`；其他 CLI 用启动参数），不改日常默认值（`AGENTS.md` 的 Real-CLI tests 规则）。以原生记录里实际的模型为准做断言，不看界面。
- **逐帧记录**：每一步同时记下终端画面、服务端 `conversation/check` 的判定和页面输入框的状态。场景至少包括：
  - 空闲、生成中、工具调用；
  - 权限确认：必要时在临时配置里把权限设为 `ask`；
  - 提问表单、命令面板、模型选择；
  - Esc 中断、工具报错、子代理。
- **按键**：走页面真实的按键路径 `sendToSession(null, [key], uid)`。对话模式下终端面板是折叠的，往隐藏的 xterm 按键不会送到 PTY。
- **对照历史**：把渲染出的历史和数据库或原生记录逐条比对，确认每种记录都被识别了。

## 踩过的坑

- 仓库放在网络共享盘、在另一台机器上编译时，本机看到的 `target/debug` 可能还是重编前的旧文件，测试会悄悄跑旧二进制。重跑前先刷新目录，再对比两边 `stat` 的 inode。
- 对 crate 根（`lib.rs`）跑 rustfmt 会顺带格式化别的模块，提交前检查有没有波及无关文件。
- 工作区有并发改动时，只暂存本任务文件或修改块，不覆盖他人内容。交付使用当前工作区和官方 `python3 deploy/deploy.py deploy --all`；`--allow-dirty` 会包含已跟踪的并发修改，不能把它当作只部署本任务的开关。遵循 [部署合同](deployment.md) 与当前用户指令；快照变化或测试门失败时不手工绕过，报告尚未完成的步骤和目标。
- 数据库型 CLI 的助手记录在生成中会被原地更新。镜像只导出已结束的记录（有完成时间、有 `finish`，或后面已经有别的记录），避免历史里出现半截回复。OpenCode 按“一步”一行，粒度和 Claude/Codex 的“一条记录”基本相同，不需要逐字流式显示。
- 画面识别规则写进文档时注明实测版本（例如“2.0.18 实测”），CLI 改了布局才能追溯。
