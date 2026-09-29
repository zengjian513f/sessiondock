# 接入新的 AI CLI

本文把 OpenCode 接入（2026-09-28～29，`b14e070`、`6021595`、`9026e9b`、`f04f26b`、`99002fa`）以及同期几项按 CLI 分支的功能（`20b4098` 模型与强度、`08087c2` 未安装检测、`ea087cd` 问题报告、`b3d9c68`/`47dea60`/`9cb09ca` CLI 状态对象、`a68f673` 回合三态、`b9662b8` 子会话挂靠）整理成接入第五种 CLI 的清单。现行合同以各专题文档为准：[OpenCode](opencode.md)、[启动器](lifecycle-launcher.md)、[生命周期 HTTP](lifecycle-http.md)、[对话输入就绪](composer-input.md)、[CLI 状态对象](cli-state.md)、[读模型](read-model.md)、[liveness](liveness.md)。

## 先调研，再定路线

动手前先在隔离环境（临时 `HOME`/`XDG_*`，独立服务端口）里把下面的问题答清楚，并记录版本号。答案决定每一层走哪条路。

| 问题 | 决定什么 | 已有 CLI 的答案 |
| --- | --- | --- |
| 最新版怎么装 | 安装脚本与版本 | OpenCode 的 `opencode.ai/install` 只给 1.x 稳定版，2.x 要用 `/v2/install`；装错版本整套适配都会跑偏 |
| 会话存在哪里、什么格式 | 读模型路线 | Claude/Codex 是 JSONL 文件，Grok 是目录加 `summary.json`，OpenCode 2 是一个 SQLite 库 |
| 会话 ID 何时确定 | 启动类型（`Launch`） | Claude 启动参数指定 UUID（`new_assigned`）；OpenCode 先 `api session.create` 再 `--session`（`new_assigned`）；Codex 启动后才知道（`new_pending`） |
| 怎么恢复 | `resume_args` | Codex `resume {sid}`，OpenCode `--session {sid}` |
| 模型、强度怎么传；模型列表在哪 | 模型选择 | 见下文“模型与强度” |
| TUI 画面：编辑区、忙碌、提问/权限弹层、中断键 | 输入就绪识别 | 见下文“发送” |
| 删除语义 | 删除/回收站 | 文件型走回收站；OpenCode 只能调 `api session.remove`，不可恢复 |
| 子代理/子会话怎么表示 | 列表挂靠 | Claude 子目录，Codex `parent_thread_id`，OpenCode `parent_id` 和进程关系 |
| 进程怎么认 | 运行态、挂靠 | argv0、环境变量（`*_SESSION_ID`）、打开的会话文件；OpenCode 只能靠 argv0 加 `cwd` |

路线选择：

- **会话是文件**：直接写 summary 和 provider，索引按文件增量读取。
- **会话在数据库或服务里**：仿照 `sessions/opencode.rs`，只读打开并镜像成文件（`summary.json` 加 `messages.jsonl`），下游的列表、历史分页、搜索、媒体全部复用文件管线，不另开一套读路径。
- **ID 能在启动前确定**：一律用 `new_assigned`，页面在启动时就能跳到原生行。只有做不到时才用 `new_pending`。

## 分期

- **一期**：能启动、能恢复、控制台可用、输入框能发。侧栏图标、颜色、选择器都要到位；页面可以先以终端为主（`sessionTerminalFirst`）。
- **二期**：历史、列表、搜索、媒体，启动即知身份，删除。
- **后续**：按 CLI 补模型目录、问题报告、CLI 状态（忙碌、编辑区文字）、回合三态、子会话挂靠。

每一期单独提交、部署、做浏览器验证。不要攒成一个大提交。

## 接入清单

下面每条都是 OpenCode 实际改过的地方。新 CLI 要逐条判断：跟着改，还是明确不需要。在代码里搜 `"grok"` 和 `Source::Grok`，可以找到所有按 CLI 分支的位置；只列了 Grok 没列 OpenCode 的地方，要么是 OpenCode 不需要，要么是可以对照的写法。

### ptyhost 与 client

- `crates/ptyhost-client/src/association.rs`：`Source` 增加一项。
- `crates/ptyhost/src/guard.rs`、`native_binding.rs`：接受新名字。
- `crates/ptyhost/src/session.rs`：`cmd_label`。
- 这一层改动要用 `deploy.py --with-ptyhost` 才会发到线上。

### 生命周期与启动器

- `lifecycle/model.rs`：`Source` 增加一项；`Launch::validate` 里声明新 CLI 允许的启动类型。
  - **改允许集合时必须兼容已持久化的记录**：OpenCode 从 `new_pending` 改成 `new_assigned` 后，节点上一条旧记录让整本账本读不进来，服务启动失败（`Store(InvalidSpec)`，`f04f26b`）。
- `lifecycle/store/mod.rs`：`new_assigned` 时按 CLI 的 ID 格式生成 `session_id`（OpenCode 用 `sessions::opencode::new_session_id()`）。
- `api/lifecycle.rs` 的 `launch_for`、`lifecycle/service.rs` 的 `profile_new`：选择启动类型。
- `lifecycle/launcher.rs`：
  - 默认参数：`new_assigned` 时带上 ID（OpenCode `--session {session_id}`）；
  - 启动前的钩子：OpenCode 在 spawn 之前调 `opencode api session.create`，有 20 秒上限，已存在视为成功；
  - `choice_args`：模型和强度参数；
  - 未安装检测（`08087c2`）。
- `lifecycle/models.rs`：
  - `catalog()`：模型目录从哪里读，要按子进程的环境读；
  - `supports_effort()`：这个 CLI 有没有强度参数。
- `lifecycle/autobind.rs`：进程证据绑定。
- **节点配置**：每台机器 `etc/launcher.json` 为每种 CLI 配**唯一**一个 profile（`executable` 写 CLI 的绝对路径，`args`、`resume_args`；服务本身经 `with-zshrc` 启动，CLI 继承它的环境，见 [shell-env.md](shell-env.md)）；env 按需增加新变量。改之前先备份。

### 读模型

- `sessions/mod.rs`：
  - `SessionRoots` 增加字段，`data_stamp`、`index_cursor`、`restamp` 都要纳入；
  - 所有 `SessionRoots` 字面量都要补字段：`src` 的测试里有，`crates/sessiondock/tests/*.rs` 有十几处；
  - 编译检查必须用 `cargo check --tests`，否则漏掉的测试字面量要等到别人跑测试才暴露。
- `config.rs`、`lib.rs`：环境变量，以及镜像线程这类启动项。
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

### 发送与输入就绪

- `conversation/input.rs`：
  - `<cli>_editor(capture)`：识别编辑区。以边框结构、光标所在行和页脚标签为依据，不要依赖模型名子串。
  - 弹层识别（提问、权限确认）：归为 `blocked`（`cli_question`），放在编辑区识别之前。OpenCode 是按屏幕底部 `┃` 行上的按键提示识别的（`99002fa`）。
  - `classify` 里加一个分支。
- 识别不到时，状态是 `unknown`，发送按钮禁用，草稿保留。这是安全的默认行为：OpenCode 的提问表单在专门识别之前，就是这样被正确拦下的。
- `conversation/cli_state.rs`（[CLI 状态对象](cli-state.md)）：
  - `instance.busy`（忙碌指示）和 `editor.text`（编辑区正文提取）目前只有 Claude、Codex 实现，新 CLI 可以先给 `null`；
  - 回显对账靠原生 user 记录的正文摘要。记录没有时间戳时只按摘要匹配（Grok）。

### 运行态与挂靠

- `runtime/procscan.rs`：`KEYWORDS`、`CLI_NAMES`、环境变量表。
- `runtime/spawn.rs`：子会话挂到父会话下。OpenCode 不在命令行写会话 ID，也没有按会话的文件，所以用 argv0 加 `cwd` 加启动时间，和 `summary.json` 里没有 `parent_id` 的顶层会话配对（`b9662b8`，[liveness](liveness.md)）。

### 删除

- 文件型走现有回收站：`trash/plan.rs`、`trash/manifest.rs` 的 source 分支。
- 只能通过 CLI 删除的（OpenCode）：
  - 在 `api/trash.rs` 里把这类 UID 单独分流：调用 CLI 删除，并删掉镜像目录；
  - 确认框写明“不进回收站、无法恢复”；
  - 正在运行的会话拒绝删除。

### 问题报告

- `bug_report/mod.rs`：允许的处理 CLI。
- `index.html`：报告对话框的选项（`ea087cd`、`ffc02a3`）。

### 前端

- `legacy-web/cli.js`：`<Cli>Cli` 类。
  - 需要特殊按键行为时覆盖 `questionAnswerKeys`、`canAnswerQuestionForm`、`questionFormAnswerKeyGroups`、`questionCancelKeys`、`repeatedEscape`；
  - 目前只有 Claude、Codex 覆盖了这些，基类的默认值是安全的。
- `index.html`：`i-<cli>` 图标 symbol 和选择器 radio。
- `style.css`：
  - `--<cli>` 颜色，浅色和深色主题都要定义；
  - 新建会话选择器已经是五列，再加一列要重新验证手机宽度（390 px 一行排得下）。
- `term.js`：输入识别还不可靠时用终端优先布局（`sessionTerminalFirst`）。

### 文档与测试

- 文档：
  - 新建 `docs/<cli>.md` 写合同；
  - 同步 `lifecycle-launcher.md`、`lifecycle-http.md`（模型）、`composer-input.md`（识别规则）、`environment.md`（由 `tests/env_reference.py --write` 生成）、`validation.md`；
  - 最后重新生成 `docs/README.md`、`module-map.md`、`error-codes.md`。
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

## 用真实 CLI 验证

假 CLI 只能证明自己抄的画面，真实版本必须实际跑几轮。做法（`oc_live` 脚本的思路）：

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
- 工作区里有别的会话未提交的改动时，要按块暂存自己的部分（基于 HEAD 生成补丁再 `git apply --cached`），并用 `git checkout-index` 导出暂存区单独编译确认。`--allow-dirty` 部署会把别人未完成的后端一起发出去，要等对方提交后再部署。
- 数据库型 CLI 的助手记录在生成中会被原地更新。镜像只导出已结束的记录（有完成时间、有 `finish`，或后面已经有别的记录），避免历史里出现半截回复。OpenCode 按“一步”一行，粒度和 Claude/Codex 的“一条记录”基本相同，不需要逐字流式显示。
- 画面识别规则写进文档时注明实测版本（例如“2.0.18 实测”），CLI 改了布局才能追溯。
