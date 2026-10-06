# SessionDock

独立演进的 Rust 会话服务，包含本地节点、多机 Hub、受管终端、可靠发送、文件与
媒体能力。前端位于 `legacy-web/`（`/sessiondock/`），是按提交原样发布的纯 JS 静态资源。

未完成工作见 [TODO.md](TODO.md)；全部当前合同文档索引见
[docs/README.md](docs/README.md)，路由清单见
[docs/route-ledger.md](docs/route-ledger.md)，术语见
[docs/glossary.md](docs/glossary.md)，模块地图见
[docs/module-map.md](docs/module-map.md)，能力开关见
[docs/capabilities.md](docs/capabilities.md)。

## 目录

```text
crates/
  sessiondock/    Axum / Tokio、配置/API、原生会话读模型、共享SSE和搜索
  process-links/    共享进程归属协议与关联库
  resource-agent/   独立 Linux 资源采集服务
  ptyhost-client/    独立异步 host 客户端，显式开发目录才接入传输
  ptyhost/           独立 Rust PTY host，保留旧协议并增加可选实例校验
  ptyhost-record/    终端录像格式、存储与读取
  ptyhost-screen/    服务端终端画面模型
legacy-web/          前端（纯 JS 静态资源，无构建步骤）
reference/
  legacy-web/        原前端的冻结快照，仅作迁移参考
tests/               实际浏览器操作、协议验证和可选 Python fixture 差分
docs/                当前架构、协议、运维和验证合同
```

## 开发

需要较新的 Rust 2024 edition 工具链（本轮使用 Rust 1.98.1，尚未验证最低支持版本）。
默认页面无需 Node.js、Vite 或前端构建。
以下命令从仓库根目录执行。

```sh
cargo run -p sessiondock --locked
```

打开 <http://127.0.0.1:8741>。未配置数据根时列表确实为空，不自动扫描
CLI home 或原项目。普通监听只允许 loopback，Host 接受本地地址及显式配置的
`SESSIONDOCK_PUBLIC_HOSTS`，拒绝跨站 API 和旧 Hub 的认证/协议头。
公开部署使用已鉴权反代，支持 `/sessiondock/` 等子路径；这些检查不能替代正式认证。

要查看随仓库提供的**人工合成原生记录**（不会启动 CLI）：

```sh
SESSIONDOCK_CLAUDE_ROOT=crates/sessiondock/tests/fixtures/claude \
SESSIONDOCK_CODEX_ROOT=crates/sessiondock/tests/fixtures/codex \
SESSIONDOCK_GROK_ROOT=crates/sessiondock/tests/fixtures/grok \
cargo run -p sessiondock --locked
```

以上是 POSIX shell 写法；PowerShell 可逐个设置对应 `$env:SESSIONDOCK_*` 后
运行 cargo。这些是测试样例，不是导入的用户运行数据。

常用环境变量如下（不会自动读取 `.env`），完整表见 [environment.md](docs/environment.md)：

| 变量 | 默认值 | 含义 |
| --- | --- | --- |
| `SESSIONDOCK_BIND` | `127.0.0.1:8741` | 只允许 loopback，不允许公网/局域网绑定 |
| `SESSIONDOCK_WEB_DIR` | `legacy-web` | 工作前端目录，相对服务启动目录 |
| `SESSIONDOCK_CLAUDE_ROOT` | 无 | `项目/*.jsonl`，及 `项目/会话/subagents/agent-*.{jsonl,meta.json}` |
| `SESSIONDOCK_CODEX_ROOT` | 无 | 显式的 sessions 形状目录，递归 JSONL |
| `SESSIONDOCK_CODEX_INDEX` | 所配置 Codex 根父目录的 `session_index.jsonl` | 只读名称索引；显式设置可覆盖默认位置 |
| `SESSIONDOCK_GROK_ROOT` | 无 | `项目/会话/{summary.json,chat_history.jsonl}` |
| `SESSIONDOCK_OPENCODE_DB` / `SESSIONDOCK_OPENCODE_ROOT` | 无 | 成对配置原生只读数据库与私有投影目录；见 [OpenCode](docs/opencode.md) |
| `SESSIONDOCK_STATE_DIR` | 无 | SessionDock 元数据与会话草稿等私有状态；目录按需创建 |
| `SESSIONDOCK_PTYHOST_DIR` | 无 | 私有 host 目录；启用精确 UID/实例控制台和观察 |
| `SESSIONDOCK_FILE_ROOTS` | 无 | 兼容旧配置的目录列表，不授予或限制文件读取；POSIX 以冒号、Windows 以分号分隔 |

只读取明确配置的数据根，拒绝会话路径中的符号链接；这不是对敌对本地
文件系统的完整沙箱。建议先用脱敏副本；无法解释的历史形状会明确报错。
静态文件变更后需要重启以更新资源快照/build。配置没有通用的目录互斥要求；
具体服务仍检查其实际协议、身份和 OS 访问条件。

保存偏好时显式设置 `SESSIONDOCK_STATE_DIR`（例如忽略的 `.runtime/metadata`）。
未配置时元数据写接口返回 501；目录按需创建，缺失、损坏或未知 schema 按空数据
读取，后续更新可以覆盖文件。每次操作重载，进程内 mutex 串行更新，无独占生命周期
文件锁。详见 [元数据边界](docs/metadata.md)。
文件浏览与独立预览由 FileDock 提供；SessionDock 负责解析会话引用和附件等接口。
文件访问遵循 OS 权限，不以旧 file roots 或 cwd 作为授权边界。详见
[文件入口](docs/files.md) 和 [安全边界](docs/security-model.md)。

## 构建与检查

用户不主动要求时，不跑任何单元测试（`cargo test`、`tests/*_contract.mjs`、Python unittest）；改动用覆盖该功能的 headless 浏览器测试验证（没有就补）。历史原因：2026-10-06 核查发现单元测试长期无人维护，失败几乎都是测试没跟上有意的改动，真正的问题都由浏览器测试发现。
全量清扫是 `python3 tests/run_validation.py`。

```sh
cargo build --workspace --release --locked

# 免费浏览器验收：需要 Python Playwright + Chromium，测试自行创建/清理临时样例。
cargo build -p sessiondock --locked
python3 tests/legacy_browser.py

# 历史回归：自行创建人工历史和临时 Rust 服务。
python3 tests/history_parity.py --python-source PATH
python3 tests/history_browser.py
python3 tests/history_pages_browser.py
python3 tests/media_browser.py
python3 tests/media_lazy_browser.py
python3 tests/media_formats_browser.py
python3 tests/media_files_browser.py
python3 tests/media_parity.py --python-source PATH

# 真实搜索UI与工具渲染/可选Python差分。
python3 tests/search_browser.py
python3 tests/tool_parity.py --python-source PATH --browser
python3 tests/metadata_browser.py
python3 tests/files_browser.py
python3 tests/terminal_browser.py
python3 tests/host_identity.py
python3 tests/managed_terminal_browser.py
python3 tests/terminal_exit_browser.py
python3 tests/lifecycle_browser.py
python3 tests/lifecycle_browser.py --native-binding
python3 tests/names_parity.py --python-source PATH --browser
python3 tests/grok_parity.py --python-source PATH --browser
```

浏览器路径可用 `PLAYWRIGHT_CHROMIUM_EXECUTABLE` 指定。测试不调用模型、
或原项目服务；上述读取/偏好工具只启动临时 Rust 子进程，验证后停止。

可选的 Python 差分检查（先按上面的仓库 fixture 目录启动 Rust）：

```sh
python3 tests/provider_parity.py --python-source PATH --base-url http://127.0.0.1:8741
```

此工具只读取三个人工 fixture 并调用原 Python adapter；是开发验证，**不是
Rust 运行依赖，也不是所有历史格式已兼容的证明**。

## 当前范围

- 提供 Claude、Codex、Grok、OpenCode 会话读取、搜索、分页、媒体和 SSE。
- 可选服务提供受管终端、生命周期、可靠发送、文件、回收站和诊断能力。
- 本地节点与 `sessiondock-hub` 支持多机 Hub。
- 未启用的能力明确返回 501；不会伪造成功。
- 普通读取不修改原生历史。用户明确确认的整组移动、复制和回收站操作可以按各自合同
  发布、移动或清理原生文件；克隆保留源组，并仅改写目标必需的身份、路径与偏移字节。
- 整组操作见 [移动](docs/session-move.md) 与 [克隆](docs/session-clone.md)；
  独立 Linux 资源采集及会话计量见 [process-links](docs/process-links.md)。

前端 `legacy-web/` 不经过构建，Rust 服务直接提供并注入能力声明；验证采用 Chromium 用户操作。
平台限制见对应合同。

架构见 [docs/architecture.md](docs/architecture.md)，冻结前端参考见
[reference/README.md](reference/README.md)。
合成读取基准与尚存的重解析成本见 [docs/performance.md](docs/performance.md)。
