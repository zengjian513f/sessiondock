# SessionDock

独立演进的 Rust 会话服务，包含本地节点、多机 Hub、受管终端、可靠发送、文件与
媒体能力。当前生产前端位于 `legacy-web/`；`web/` 中保留的 Vue 3 / TypeScript
骨架是否继续迁移，作为独立产品决策记录在 `TODO.md`。

未完成工作见 [TODO.md](TODO.md)；全部当前合同文档索引见
[docs/README.md](docs/README.md)，路由清单见
[docs/route-ledger.md](docs/route-ledger.md)，术语见
[docs/glossary.md](docs/glossary.md)，模块地图见
[docs/module-map.md](docs/module-map.md)，能力开关见
[docs/capabilities.md](docs/capabilities.md)。Python → Rust 的批次过程仅保留在
`MIGRATION_HISTORY.md`，不作为当前设计或状态来源。

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
tests/               legacy 契约、隔离浏览器和可选 Python fixture 差分
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
| `SESSIONDOCK_PTYHOST_DIR` | 无 | 既有隔离host目录；精确UID/实例关联的会话控制台及部分观察，不开放CLI创建/启动接管 |
| `SESSIONDOCK_FILE_ROOTS` | 无 | 1–16个显式绝对开发文件目录；POSIX以冒号、Windows以分号分隔，只读 |

只读取明确配置的数据根，拒绝会话路径中的符号链接；这不是对敌对本地
文件系统的完整沙箱。建议先用脱敏副本，复杂历史会明确返回 501。
当前仅支持根路径本地开发，静态文件变更后需要重启以更新资源快照/build。
静态前端与所有私有数据目录不得重叠，元数据目录也不能与host/native输入重叠。

Linux 隔离账本可显式初始化一次（不启动 Web 或模型 CLI，不导入旧队列）：

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

```sh
cargo test --workspace --locked
cargo build --workspace --release --locked
node --test tests/legacy_contract.mjs tests/history_pages_contract.mjs tests/media_lazy_contract.mjs

# 免费浏览器验收：需要 Python Playwright + Chromium，测试自行创建/清理临时样例。
cargo build -p sessiondock --locked
python3 tests/legacy_browser.py

# 第二批历史回归：自行创建人工历史、启动并停止隔离 Rust 服务。
python3 tests/history_parity.py --python-source ../agenthub
python3 tests/history_browser.py
python3 tests/history_pages_browser.py
python3 tests/media_browser.py
python3 tests/media_lazy_browser.py
python3 tests/media_formats_browser.py
python3 tests/media_files_browser.py
python3 tests/media_parity.py --python-source ../agenthub

# 第三批：真实搜索UI与工具渲染/可选Python差分。
python3 tests/search_browser.py
python3 tests/tool_parity.py --python-source ../agenthub --browser
python3 tests/metadata_browser.py
python3 tests/files_browser.py
python3 tests/terminal_browser.py
python3 tests/host_identity.py
python3 tests/managed_terminal_browser.py
python3 tests/terminal_exit_browser.py
python3 tests/lifecycle_browser.py
python3 tests/lifecycle_browser.py --native-binding
python3 tests/names_parity.py --python-source ../agenthub --browser
python3 tests/grok_parity.py --python-source ../agenthub --browser
```

浏览器路径可用 `PLAYWRIGHT_CHROMIUM_EXECUTABLE` 指定。测试不调用模型、
或原项目服务；上述读取/偏好工具只启动临时 Rust 子进程，验证后停止。

可选的 Python 差分检查（先按上面的仓库 fixture 目录启动 Rust）：

```sh
python3 tests/provider_parity.py --python-source ../agenthub --base-url http://127.0.0.1:8741
```

此工具只读取三个人工 fixture 并调用原 Python adapter；是开发验证，**不是
Rust 运行依赖，也不是所有历史格式已兼容的证明**。

## 当前范围

- 已有：legacy 静态服务/能力声明，受限 Claude/Codex/Grok 列表、消息、输入历史、
  字节游标、SSE、半行/重写/reset、版本缓存、有界 worker/订阅和错误重试。
- 第十四批：有限事件分页与legacy逐页补齐历史，独立于实时SSE游标；支持多媒体
  预算和显式错误恢复，详见[历史页合同](docs/history-pages.md)。
- 第十五批：图片来源登记与GET时解码分离，显式失败/重试；未提高大原生记录
  上限，详见[媒体合同](docs/media.md)。
- 第十六至十九批：可信原生输入/物理索引、流式记录缓存、结构化原生大图（单图
  32MiB、当前分支冷/热授权）和嵌套字符串Codex工具信封的有界回放（共享512MiB
  工作预算），详见[输入合同](docs/native-input.md)。第二十批：单消息>16图按
  `media_more`/`media-page` 逐批续取，单消息上限256张，见[媒体合同](docs/media.md)。
- 第二批：Claude 活动祖先链、last-prompt、compact 和未回答中断输入；Codex
  固定前缀 history_base、多级分叉和两家子代理归属/详情。语义游标检测换枝和
  父前缀改写，普通追加不重放继承历史。缺父文件/错误切点/循环依赖明确失败。
- 读模型：惰性索引 + 按需视图（[docs/read-model.md](docs/read-model.md)）——列表只做
  stat 与头 96 KiB / 尾 512 KiB 摘要，没有启动解析、没有会话数/总字节上限；单文件
  256 MiB 全量解析预算只在打开该会话时生效；逻辑视图至多 100000 个事件；视图缓存
  条数与字节有界（物理工作预算，不是 RSS 上限）。普通文本、部分工具/问答和状态已支持；第十一批已接通
  有界内嵌图片，第十二批扩展GIF/WebP/APNG/静态AVIF/BMP及授权磁盘图片；不代表所有媒体或未知原生结构已支持。Claude 仅在 CLI 内存中发生、
  尚无原生日志信号的回滚不能观测；持久 pin 仍待迁移。Codex 名称索引进展见第六批。
- 原生内嵌媒体通过进程内随机token显示，支持三家图片、工具结果及明确MCP包装；
  单图≤1.5MiB，缓存32MiB/256项，受现有2MiB原生记录限制。搜索不解码图片，
  外链不自动读取。磁盘图片须显式文件根及当前完整会话分支授权，单张读取≤32MiB
  （AVIF仍≤1.5MiB）；GET重新核验文件版本，旧token不会悄悄返回替换后的内容。
  磁盘图片失败保留文字和单图解释，详见[媒体合同](docs/media.md)。
- 第十三批：追加复用经完整字节核验的JSON AST，仍全量计算时间线。额外32MiB
  逻辑weight/16项缓存换取追加成本下降，首次读取可能变慢，见[测量与边界](docs/append-cache.md)。
  媒体合成差分修正Claude块拆分/计数和Codex/Grok占位文本；真正历史分页、按需
  解码及32MiB大内嵌来源仍待[后续实现](docs/media-pagination-design.md)。
- 第三批：每逻辑视图共享观察，慢读者按独立游标追赶；fork列表cursor与详情
  对齐。全文搜索沿用legacy选项/NDJSON进度，支持取消和明确的部分失败。
  工具摘要、文件diff卡片/分栏、问答及执行信封已做三家合成Python差分。
- 搜索采用Rust regex；lookaround/backreference返回400，Unicode大小写与字符类
  不承诺与Python完全一致。结果有8MiB预算；工具diff超预算会解释原因并保留参数。
- 第四批：可选独立元数据存储已接星标/显示偏好，使用单writer锁、原子
  提交与版本快照。claim/WS已接显式host目录，完整UID/实例控制台见第六批。
  Codex发送纯状态机20项测试通过，不代表可以可靠发送或确认CLI输入。
- 第五批进行中：受控host精确full SID/UID关联与运行/退出/未知三态；全局live
  仍未知，不猜测外部CLI状态。Claude独立发送纯状态机21项测试通过，持久store
  与只读API见第六/七批，尚未开放发送。文件只读链路及legacy浏览器已接通；写作业/缩略图
  各自关闭，受限下载走有界按需流，不将二进制编码进JSON。
- 未实现：完整历史语义、外部进程探测、完整终端生命周期、
  可靠发送执行/确认链路、文件写操作/回收站、诊断存储、Hub、生产认证和部署。
- 第六批：guard-capable host 的完整 UID/实例绑定已接通 legacy 控制台，支持
  手动键盘输入和页面抢占；旧 host、重复关联、退出或错配不会进入可控列表。
  显式 Codex 名称索引与 Grok summary-only 元数据已完成隔离差分及浏览器验证。
  发送持久层已实现，但真实 CLI 发送适配器和确认链路仍未开放。
- 第七批：host输出ACK/回放/实时/退出有序，慢客户端隔离，PTY输出未读完会明确
  报错；legacy保留退出尾部与原因，不自动重连已退出实例。只读outbox接有界异步
  账本服务与NativeScope，独立的初始化命令不会启动Web或模型CLI。
- 第八批进行中：独立launch实例guard/client和持久幂等创建回执已实现，供未知
  原生SID的pending流程使用；第九批已接显式launcher、create/status/cancel与
  legacy pending控制台，默认仍关闭。仅配置独立回执目录、私有白名单文件及
  host目录后启用。第十批补一次性操作者确认的原生主会话绑定，原pending连接
  不自动升级，跨Web重启仍检查持久取消状态；不是CLI关联证明或可靠发送。
  见 [创建HTTP合同](docs/lifecycle-http.md) 与 [绑定合同](docs/lifecycle-binding.md)。
- 未实现操作返回明确 501，不返回空 outbox 假装确认，也不会提交用户输入。
- 开发时不读取或接管原项目生产数据；ptyhost 手工调用必须显式指定隔离 `--dir`。
- 此仓库尚未配置远程，也未选定对外发布许可证。

第二阶段 Vue 骨架仍可在 `web/` 执行 `npm ci && npm test && npm run build`。
当前没有前端框架重构。Linux 测试通过不代表 Windows/macOS 实机验证通过。

架构见 [docs/architecture.md](docs/architecture.md)，导入来源见
[docs/migration.md](docs/migration.md)。
合成读取基准与尚存的重解析成本见 [docs/performance.md](docs/performance.md)。
