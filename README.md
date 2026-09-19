# SessionDock

独立演进的 Rust 会话服务，包含本地节点、多机 Hub、受管终端、可靠发送、文件与
媒体能力。当前生产前端位于 `legacy-web/`；`web/` 中保留的 Vue 3 / TypeScript
骨架是否继续迁移，作为独立产品决策记录在 `TODO.md`。

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
  ptyhost-client/    独立异步 host 客户端，显式开发目录才接入传输
  ptyhost/           独立 Rust PTY host，保留旧协议并增加可选实例校验
legacy-web/          第一阶段工作前端，少量能力/错误处理兼容改动
web/                 第二阶段 Vue / TypeScript 骨架，当前非默认
reference/
  legacy-web/        原前端的冻结快照，仅作迁移参考
tests/               legacy 契约、临时浏览器环境和可选 Python fixture 差分
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
`.claude`、`.codex`、`.grok` 或原项目。当前只允许 loopback 监听，拒绝非本地
Host、跨站 API 和旧 Hub 的认证/协议头，不能替代正式认证。

要查看随仓库提供的**人工合成原生记录**（不会启动 CLI）：

```sh
SESSIONDOCK_CLAUDE_ROOT=crates/sessiondock/tests/fixtures/claude \
SESSIONDOCK_CODEX_ROOT=crates/sessiondock/tests/fixtures/codex \
SESSIONDOCK_GROK_ROOT=crates/sessiondock/tests/fixtures/grok \
cargo run -p sessiondock --locked
```

以上是 POSIX shell 写法；PowerShell 可逐个设置对应 `$env:SESSIONDOCK_*` 后
运行 cargo。这些是测试样例，不是导入的用户运行数据。

后端支持以下环境变量（不会自动读取 `.env`）：

| 变量 | 默认值 | 含义 |
| --- | --- | --- |
| `SESSIONDOCK_BIND` | `127.0.0.1:8741` | 只允许 loopback，不允许公网/局域网绑定 |
| `SESSIONDOCK_WEB_DIR` | `legacy-web` | 工作前端目录，相对服务启动目录 |
| `SESSIONDOCK_CLAUDE_ROOT` | 无 | `项目/*.jsonl`，及 `项目/会话/subagents/agent-*.{jsonl,meta.json}` |
| `SESSIONDOCK_CODEX_ROOT` | 无 | 显式的 sessions 形状目录，递归 JSONL |
| `SESSIONDOCK_CODEX_INDEX` | 无 | 显式外置 `session_index.jsonl` 副本，不能置于 native/host/state/static/file roots 中；只读名称索引 |
| `SESSIONDOCK_GROK_ROOT` | 无 | `项目/会话/{summary.json,chat_history.jsonl}` |
| `SESSIONDOCK_STATE_DIR` | 无 | 既有独立开发元数据目录；仅保存星标/显示偏好，不迁移旧数据 |
| `SESSIONDOCK_DELIVERY_DIR` | 无 | 显式独立私有账本目录；仅打开已初始化账本，提供只读outbox，不启用发送 |
| `SESSIONDOCK_PTYHOST_DIR` | 无 | 私有 host 目录；启用精确 UID/实例控制台和观察 |
| `SESSIONDOCK_FILE_ROOTS` | 无 | 1–16个显式绝对开发文件目录；POSIX以冒号、Windows以分号分隔，只读 |

只读取明确配置的数据根，拒绝会话路径中的符号链接；这不是对敌对本地
文件系统的完整沙箱。建议先用脱敏副本，复杂历史会明确返回 501。
当前仅支持根路径本地开发，静态文件变更后需要重启以更新资源快照/build。
静态前端与所有私有数据目录不得重叠，元数据目录也不能与host/native输入重叠。

Linux 私有账本可显式初始化一次（不启动 Web 或模型 CLI，不导入旧队列）：

```sh
mkdir -p -m 700 .runtime/delivery
cargo run --locked -p sessiondock -- --initialize-delivery "$PWD/.runtime/delivery"
```

目标必须既存、空、私有且与其他配置根分离；重复初始化报错，不覆盖账本。
正常 Web 启动不会因账本缺失而自动初始化。初始化后可以显式配置
`SESSIONDOCK_DELIVERY_DIR="$PWD/.runtime/delivery"` 启动服务，读取既有账本。
打开时先持久化两家新的恢复epoch；后续GET不改账本。`outbox_read`与发送总开关
分离，legacy发送/重试/丢弃入口仍关闭；Windows 持久化尚未实现。
如需保存偏好，请先创建新的私有目录（例如忽略的 `.runtime/metadata`，Unix
权限0700），再显式设置 `SESSIONDOCK_STATE_DIR`。未配置时仍返回501；坏schema、
不安全权限、已有writer不会被自动修复或覆盖。详见 [元数据边界](docs/metadata.md)。
文件目录还须独立于静态资源、native、metadata和host目录；只允许读取所选会话
实际提及的文件，或以提及目录为入口在所属授权根内导航。不会扩大到整个文件系统，
不会展开HOME。详见 [文件读取边界](docs/files.md)。

## 构建与检查

绝不自行跑单元测试；改动用覆盖该功能的 headless 浏览器测试验证（没有就补）。
全量清扫是 `python3 tests/run_validation.py`。

```sh
cargo build --workspace --release --locked
node --test tests/legacy_contract.mjs tests/history_pages_contract.mjs tests/media_lazy_contract.mjs

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

- 提供 Claude、Codex、Grok 会话读取、搜索、分页、媒体和 SSE。
- 可选服务提供受管终端、生命周期、可靠发送、文件、回收站和诊断能力。
- 本地节点与 `sessiondock-hub` 支持多机 Hub。
- 未启用的能力明确返回 501；不会伪造成功。
- 原生历史只读。写入数据仅进入 SessionDock 的私有目录。

第二阶段 Vue 骨架仍可在 `web/` 执行 `npm ci && npm test && npm run build`。
当前不做前端框架重构。平台限制见对应合同。

架构见 [docs/architecture.md](docs/architecture.md)，冻结前端参考见
[reference/README.md](reference/README.md)。
合成读取基准与尚存的重解析成本见 [docs/performance.md](docs/performance.md)。
