# 单机切流清单：Python 节点 → Rust

文档，不是生产授权。Rust 服务已在本机以私有前缀并行部署（用户级单元 `sessiondock.service`、前缀 `/srv/sessiondock`、反代位置 `/sessiondock/`；[AGENTS.md](../AGENTS.md#scope-and-boundaries)），本清单描述的是把流量从 Python 切到它、以及回退的步骤。完成度目标：与现有 Python 后端持平或略高，超出仅在安全/隔离必需时（[计划 §1](../BACKEND_MIRGRATION_PLAN.md#1-目标与边界)）。未勾选项不表示已兼容。禁止把生产 host、队列、注册表或 CLI 家目录配进 Rust。`deploy/` 仅列名、未读内容：`agenthub.service`、`agenthub-hub.service`、`agenthub-tmux.service`、`nginx-agenthub.conf`、`nginx-agenthub-hub.conf`、`agenthub-legacy-paths-cleanup`。

来源：计划 [§1](../BACKEND_MIRGRATION_PLAN.md#1-目标与边界)、[§4](../BACKEND_MIRGRATION_PLAN.md#4-路由迁移账本)、[§6 M8](../BACKEND_MIRGRATION_PLAN.md#m8hub-与生产迁移)；[environment.md](environment.md)；[runbook-dev.md](runbook-dev.md)；[security-model.md](security-model.md)；[capabilities.md](capabilities.md)；[validation.md](validation.md)；只读 `python3 tests/route_ledger.py`（2026-09-12）。`tests/python_route_gap.py` **不存在**，无计数。Python 入口只读：`../agenthub/run.sh`、`../agenthub/agenthub/server.py` argparse。

`route_ledger.py` 摘要（工具始终 exit 0）：implemented **46**；501 **0**（路由级 stub 已清空；未配置时 handler 仍答 501：`/api/session/send`、`draft-status`、`outbox/retry|discard` 无账本，`/api/bug-report` 自第四十一批起实现、未配置答 501 `bug_report_disabled`，`/api/session/stop` 自第二十九批起对受管实例实现、未受管会话答 501 `session_stop_unmanaged`）；in plan but absent **0**；used by legacy but absent **3**（`/api/nodes/{param}`、`/api/nodes/{param}/display`、`/api/session`）。

## 1. 前置条件

构建（[runbook](runbook-dev.md#1-build)）：`cargo build --release -p sessiondock` 与 `cargo build -p ptyhost`。ptyhost 必须显式隔离 `--dir`，禁止默认目录（[session-host.md](session-host.md)、[AGENTS.md](../AGENTS.md#scope-and-boundaries)）。

目录（[runbook §4](runbook-dev.md#4-optional-isolated-features)、[security-model](security-model.md#explicit-configuration)）：delivery / lifecycle / audit / ptyhost 须已存在、绝对路径、Unix `0700`、无符号链接祖先；launcher JSON 绝对、`0600`、单硬链、≤64 KiB；各根互不重叠（含 `legacy-web`）。state 目录已存在且不含 Python `session-meta.json`（[metadata.md](metadata.md#directory-and-schema)）。回收站另需 `SESSIONDOCK_TRASH_DIR`（[trash.md](trash.md)；`environment.md` 现表未列）。缺省可选变量则能力关闭；空目录值启动失败。

`docs/environment.md` 现列 13 个 `SESSIONDOCK_*`：`BIND`、`WEB_DIR`、`CLAUDE_ROOT`、`CODEX_ROOT`、`GROK_ROOT`、`PTYHOST_DIR`、`STATE_DIR`、`DELIVERY_DIR`、`LIFECYCLE_DIR`、`LAUNCHER_CONFIG`、`AUDIT_DIR`、`CODEX_INDEX`、`FILE_ROOTS`。

Python `run.sh`（`--host 0.0.0.0 --port "${PORT:-8710}" --allow "${ALLOW:-}" --terminal`）与 `server.py` argparse / 启动期环境：

| Python | Rust |
| --- | --- |
| `--host` / `--port` / `PORT` | `SESSIONDOCK_BIND`（默认 `127.0.0.1:8741`）。`--host 0.0.0.0` **无对应**（非 loopback 拒绝） |
| `--allow` / `ALLOW` | **无对应**（无 IP 白名单、无 HTTP 认证） |
| `--terminal` | `SESSIONDOCK_PTYHOST_DIR`（显式目录，非布尔） |
| `--terminal-backend` / `AGENTHUB_TERM_BACKEND` | **无对应**（无 tmux；ptyhost 由目录启用） |
| `--node-token-file` / `--node-id-file` | `SESSIONDOCK_NODE_TOKEN_FILE` / `SESSIONDOCK_NODE_ID_FILE`（与 `SESSIONDOCK_NODE_BIND`、`SESSIONDOCK_NODE_PEERS` 四者齐备才开节点监听，第三十八批 H1） |
| `AGENTHUB_HOST_DIR` | `SESSIONDOCK_PTYHOST_DIR`（禁止默认 host 目录） |
| `AGENTHUB_HOST_BIN` | `SESSIONDOCK_LAUNCHER_CONFIG` 的 `host_binary` |
| `AGENTHUB_HOST_ENV_WRAPPER` / `AGENTHUB_HOST_SCOPE` | **无对应**（login-shell / `env -u` 不复现） |
| 隐式 `~/.claude/projects` | `SESSIONDOCK_CLAUDE_ROOT` |
| `~/.codex/sessions` | `SESSIONDOCK_CODEX_ROOT` |
| `~/.codex/session_index.jsonl` | `SESSIONDOCK_CODEX_INDEX` |
| `~/.grok/sessions` | `SESSIONDOCK_GROK_ROOT` |
| 包内 static | `SESSIONDOCK_WEB_DIR`（默认 `legacy-web`） |
| `session-meta.json` | `SESSIONDOCK_STATE_DIR`（新目录，不迁移） |
| `send-queue.json` / `claude-send-queue.json` | `SESSIONDOCK_DELIVERY_DIR`（只读账本；发送仍 501） |
| `create-requests` | `SESSIONDOCK_LIFECYCLE_DIR` |
| `audit.sqlite3` | `SESSIONDOCK_AUDIT_DIR`（JSONL，非 SQLite） |
| `trash` | `SESSIONDOCK_TRASH_DIR` |
| file-manager | `SESSIONDOCK_FILE_ROOTS`（只读） |
| `bug-reports` | `SESSIONDOCK_BUG_REPORT_DIR`（0700 私有目录，不迁移旧包）+ `SESSIONDOCK_BUG_REPORT_REPO`（= 本仓库，须在写根内）+ launcher `bug_report_profiles`（最便宜模型），[bug-report.md](bug-report.md) |
| Hub `hub-nodes.json` | `SESSIONDOCK_HUB_NODES`（独立二进制 `sessiondock-hub`，0600；另有 `SESSIONDOCK_HUB_CACHE_DIR` / `SESSIONDOCK_HUB_NETWORKS` / `SESSIONDOCK_HUB_BIND`；注册是 `sessiondock-hub register/remove/list` 子命令，非 HTTP 路由）。[deploy-hub.md](deploy-hub.md) |

不能当作已满足的切流前提：

- **TODO** [M1](../BACKEND_MIRGRATION_PLAN.md#m1只读会话纵向链路第一二批持续推进) 真实数据影子比对前不宣称读模型完整兼容。
- **TODO** [M5](../BACKEND_MIRGRATION_PLAN.md#m5可靠发送与交互) 可靠发送未接执行适配；`outbox` 保持 false。
- **TODO** [M8](../BACKEND_MIRGRATION_PLAN.md#m8hub-与生产迁移) 生产影子比对、隔离写入演练、用户授权切流均未勾选。
- **TODO** [M7](../BACKEND_MIRGRATION_PLAN.md#m7诊断与运维) 配置校验/优雅退出/CI 三平台未勾选（runbook 仅记录 SIGINT/SIGTERM）。
- **TODO** [M3](../BACKEND_MIRGRATION_PLAN.md#m3持久元数据与进程关联)/[M4](../BACKEND_MIRGRATION_PLAN.md#m4ptyhost-与终端) Windows/macOS 未实机验证。

## 2. 仍未迁移或有意不同

- **501 路由**（账本 stub）：`send`、`draft-status`、`outbox/retry|discard`。未配置时 handler 亦 501：audit、bug-report、outbox 无账本、文件作业/缩略图、`takeover force:true`、rename、Grok native-scope（[security-model](security-model.md#501-ledger-unimplemented-writes)）；`debug_run` 已是 Python 同款视图筛选（[read-model](read-model.md#debug_run-视图python-agenthubdebug_runspy)）。**TODO** [M4 外部 stop/takeover/rename](../BACKEND_MIRGRATION_PLAN.md#m4ptyhost-与终端)；**TODO** [M5 发送](../BACKEND_MIRGRATION_PLAN.md#m5可靠发送与交互)；**TODO** [M6 文件作业](../BACKEND_MIRGRATION_PLAN.md#m6文件附件回收站)。
- **正则方言**（有意不同）：Rust regex；lookaround/backreference 在流开始前 400。产品决定 P3，不再复制 Python 回溯语义（计划 §7 差异表）。
- **Hub**：H1–H4 已入库（[M8](../BACKEND_MIRGRATION_PLAN.md#m8hub-与生产迁移) 数据层与 HTTP 面完成；生产切流仍 TODO，见下）。节点侧 `hub:false`；未配置身份时 `meta.protocol:0`、`node_id:null`，配置后 `protocol:1` + 文件里的 `node_id`；loopback 监听对 Hub 头仍 403 `hub_unsupported`，Hub 流量走第二监听（H1）。多机聚合/代理是**独立二进制 `sessiondock-hub`**（[deploy-hub.md](deploy-hub.md)、[hub.md](hub.md)）：hub 模式页面、`/api/meta{mode:hub,protocol:1}`、`/api/nodes{nodes,machines}`、`/api/nodes/{id}/order|display`（legacy 账本此前记 missing，`route_ledger.py` 现按 `api/hub.rs::HUB_ROUTES` 标 `hub`）、五条聚合读 + NDJSON 搜索、分拆写、`resolve`→`proxy`（JSON/SSE/WS/附件）。注册是 `sessiondock-hub register` 子命令，网页无注册路由。
- **外部实例 stop/takeover**：**TODO** [M4](../BACKEND_MIRGRATION_PLAN.md#m4ptyhost-与终端)。`session/stop` 只停受管实例（Ctrl-D×2 → 宿主受保护停止，[lifecycle-http](lifecycle-http.md#stopping-a-managed-instance-batch-29)），未受管会话 501 `session_stop_unmanaged`；`takeover force` 501。仅受管创建/索引目录 resume。
- **bug report**（第四十一批，[bug-report.md](bug-report.md)）：`POST /api/bug-report` 捕获诊断包并经 lifecycle 启动处理会话，worker 只用 launcher `bug_report_profiles` 里固定最便宜模型的 profile（不符 501 `bug_report_model_policy`）；提示词经粘贴/Enter 两步持久化注入，`submitted` 只来自原生 `user` 记录；`uid=bug-report` 的附件上传落到仓库 `agenthub_attachments/`（≤ 32 MiB）。有意不同：审计事件只有结构化元数据（无 `content`）、报告 ID 用 UTC、`terminal.txt` 仅受管实例、提示词不含 push/部署步骤、Codex/Grok 找不到 rollout 时如实记 `submitted_unconfirmed`。切流前须配置 `SESSIONDOCK_BUG_REPORT_DIR`（新建 0700 目录）与 `SESSIONDOCK_BUG_REPORT_REPO`（本仓库，且在 `SESSIONDOCK_FILE_WRITE_ROOTS` 内），并在 launcher JSON 里加 `bug_report_profiles`。
- **Windows/macOS**：**TODO** [M3](../BACKEND_MIRGRATION_PLAN.md#m3持久元数据与进程关联)/[M4](../BACKEND_MIRGRATION_PLAN.md#m4ptyhost-与终端)/[M7](../BACKEND_MIRGRATION_PLAN.md#m7诊断与运维)。Linux 构建不是跨平台验收。
- 其它有意不同：`live:false`（空列表≠停止）；`outbox` 发送关；`mutations:false`；`media_remote:false`；`storage_namespace`=`sessiondock.`；游标 schema `rs-m2-1`（旧 Python 游标会 reset）；不发现 CLI 主目录。

## 3. 影子比对步骤

**TODO** [M8](../BACKEND_MIRGRATION_PLAN.md#m8hub-与生产迁移)/[M1](../BACKEND_MIRGRATION_PLAN.md#m1只读会话纵向链路第一二批持续推进)：生产只读影子比对未做。仓库已验证的是**合成**差分，显式 `--python-source ../agenthub`，临时目录，loopback，不启动 Python 服务、不扫 CLI 家目录。

1. 只读复制到隔离目录（操作者授权路径）；**不要**把生产 CLI 家目录或 `~/.local/share/agenthub` 配进 `SESSIONDOCK_*`。
2. 导出互不重叠的 `SESSIONDOCK_*_ROOT` / `CODEX_INDEX` / `WEB_DIR=legacy-web` / `BIND=127.0.0.1:…`。
3. 一键工具：[`shadow_compare.py`](../tests/shadow_compare.py)（grok-4.6 headless 产出，人工审阅）——操作者显式给 `--claude-root/--codex-root/--grok-root`（任意子集）+ `--python-source ../agenthub` + `--binary target/release/sessiondock` + `--i-understand-this-reads-real-histories`（缺 flag 只打印计划、exit 2）。它只注入这些读根起 loopback Rust 进程（无 state/host/audit/file 目录），在进程内加载 Python 适配器，比对 `/api/sessions` 的 sid 集合（Rust `supported:false` 记 DELTA）与每来源最近更新的 `--sample N`（或 `--uid`）会话的 `/api/messages`（count、role+text、counted、activity；已文档化差异走 DELTA），结束时核对根目录 mtime/size 未变；`--json OUT`、`--timeout`、`--max-bytes`、50 条 DIFF 即停（`--all` 取消）。有 DIFF 时 exit 1。仓库验证只跑合成语料（`fixture_gen.py`），本仓库测试不运行它（`# run_validation: skip`）。
4. 细粒度套件（[validation.md](validation.md#suites)）：[`history_parity.py`](../tests/history_parity.py)、[`advanced_parity.py`](../tests/advanced_parity.py)、[`media_parity.py`](../tests/media_parity.py)、[`names_parity.py`](../tests/names_parity.py)、[`grok_parity.py`](../tests/grok_parity.py)、[`tool_parity.py`](../tests/tool_parity.py)（均 `--python-source ../agenthub`）；形状用 [`api_smoke.py`](../tests/api_smoke.py)、[`route_ledger.py`](../tests/route_ledger.py)。差异必须是文档化 DELTA，禁止 UNVERIFIED。
5. 比对字段（计划 §3.2 + parity）：列表 `sessions,sig,built_at` 与 `uid/source/sid/title/cwd/created/updated/size/model/branch`、`agent_items/forked_from_id/root_sid`；详情 `meta`、`version{size,mtime,head}`、`reset/start/end/anchor/messages/message_total/partial/activity`；history_parity 另核 `fork_depth`、agent 拓扑、EOF `end`；media 核消息序/文本/角色/计数/图片序/解码字节 SHA-256/MIME，**不**比随机 token（[media-parity](media-parity.md#what-equality-means)）。

## 4. 切流步骤

整段 **TODO** [M8](../BACKEND_MIRGRATION_PLAN.md#m8hub-与生产迁移)（用户授权前禁止切生产流量，计划 §1）。拟定顺序：

1. 影子比对通过且 DELTA 已记录（§3）。
2. **quiesce**：停 Python 新发送/创建；Rust 发送本就 501，禁止双写两套队列。
3. **宿主目录共用，不迁移实例**：Python 与 Rust 用**同一份** ptyhost（`../agenthub/host-rs` 与 `crates/ptyhost` 同源）和**同一个**宿主目录——Python 侧 `AGENTHUB_HOST_DIR` 指到 `SESSIONDOCK_PTYHOST_DIR`（部署上是 `/srv/sessiondock/host`）。Python 起的实例带 `--meta {source, instance_id, launch_id[, sid, uid]}`（新建会话由其 `new-status` 用 `launch_bind_v1` 补绑），所以两边都按身份看到、接管同一批实例；没有身份的旧实例（Python 旧 ptyhost 起的）Rust 只列不控，切换前让它们全部退出，之后按需从任一边重新起。Python 的 tmux 后端会话（`tmux -L agenthub`）Rust 不托管。外部 CLI **TODO** [M4](../BACKEND_MIRGRATION_PLAN.md#m4ptyhost-与终端)，不要接管。
4. Python Web 不必停：它是并行保留的后备后端（zj 2026-09-13："这个项目变动多，我需要一个安全的后端"）。要停也**不要**杀 ptyhost 子进程（Web 重启不得结束 CLI）。
5. 用显式隔离目录启动 Rust（[runbook](runbook-dev.md#3-minimal-environment)）。`SESSIONDOCK_BIND` 仅 loopback。不要把 Python 数据目录指给 state/delivery/ptyhost/audit/trash。 启动前先 `sessiondock --check-config`（与启动完全相同的校验，只打印生效路径、不开账本不绑端口不起 CLI）；Python 自有元数据（`~/.local/share/agenthub/session-meta.json`：收藏、fork 父可见、Escape 停止点、时间线 pin）与 Rust 的 `session-metadata.json` 不同构，需要先用 `tests/meta_import.py` 离线转换进一个空的 state 目录，否则切流后这些偏好丢失（它同时把 `~/.local/share/agenthub/debug-runs.json` 搬成 `<state>/debug-runs.json`，monkey 测试会话才继续隐藏）。`hostname` 默认已是系统主机名（`SESSIONDOCK_HOSTNAME` 可覆盖），与 Python 页面标题一致。
6. 不改 `deploy/*.service`、不切 nginx、不接旧 Hub。

## 5. 回退步骤

**TODO** [M8](../BACKEND_MIRGRATION_PLAN.md#m8hub-与生产迁移) 回退勾选仍空；下列仅复述已文档行为：

1. SIGINT/SIGTERM 停 Rust：取消准入、尽力排空 lifecycle/delivery/audit 锁，**不**杀已建立的 ptyhost 子进程（[runbook §9](runbook-dev.md#9-shut-down)）。
2. 再启 Python `run.sh`。浏览器 lease 作废不影响 host。
3. **队列**：勿改 Python `send-queue.json` / `claude-send-queue.json`；Rust delivery 目录独立且发送未启用。
4. **身份**：勿改 Python 的 `node-id` / Hub 凭据（Rust 的节点身份文件是独立配置的 `SESSIONDOCK_NODE_ID_FILE`）。UID 公式仍为 `source:sha1(path)[:16]`，根路径不同则 UID 不同。
5. **认证**：回到 Python `--allow`；Rust 无认证、无 TLS。
6. `localStorage` 前缀 `sessiondock.` 与 Python `agenthub.` 不碰撞（[capabilities.md](capabilities.md)）。
7. Web 回滚不碰 CLI 主目录、ptyhost `--dir` 记录、原生 JSONL。

## 6. 每一步的验收命令

只读、loopback、合成或操作者显式副本。不要对生产路径跑写入套件。完整 `run_validation.py` 会占 `target/` 锁。

| 步 | 命令 |
| --- | --- |
| 1 账本 | `python3 tests/route_ledger.py`（对 40/6/0/3） |
| 1 链接 | `python3 tests/check_docs_links.py docs/replacement-checklist.md` |
| 1 构建 | `cargo build --release -p sessiondock`；`cargo build -p ptyhost`（持有 `target/` 锁时） |
| 1 ptyhost | `target/debug/ptyhost --dir <隔离目录> list` |
| 1 启动失败 | 非 loopback `BIND`、空目录变量、重叠路径 → 进程拒绝启动 |
| 3 合成差分 | `python3 tests/history_parity.py --python-source ../agenthub --binary target/release/sessiondock`；同参 `advanced_parity.py`、`media_parity.py` |
| 3 可选浏览器 | `names_parity.py` / `grok_parity.py` / `tool_parity.py` 加 `--browser` |
| 3 形状 | `python3 tests/api_smoke.py` |
| 4/5 能力 | loopback 页 `meta[name="agenthub-capabilities"]`：`hub`/`outbox`/`live` 为 false；`#backend-notice` 可见 |
| 5 回退 | Rust 停后 `ptyhost --dir <同一隔离目录> list` 仍列出原实例；再启 Python 不读 Rust state 目录 |

**TODO** [M8](../BACKEND_MIRGRATION_PLAN.md#m8hub-与生产迁移) 生产切流/回退的实机验收尚未发生，上表不是生产通过证明。
