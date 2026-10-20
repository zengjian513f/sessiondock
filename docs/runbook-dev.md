# Local developer runbook

Run SessionDock against synthetic or explicitly authorized data. Ordinary reads
preserve native histories. Explicit clone, move and trash operations change only
the confirmed group/files under their contracts; test those in a synthetic tree.

## 修改前、验收与交付

先读取全局及机器规则，再读受影响合同、相邻实现和最近相关提交。用户新要求
覆盖旧方案；不能只搜到同名事故便恢复旧设计。共享工作区先确认并发改动归属。
工作流与交付范围以 [AGENTS.md](../AGENTS.md) 为准，本表用于检查本次工作是否闭环。

| 修改涉及 | 必须核对的当前要求 | 验收证据与合同 |
| --- | --- | --- |
| 界面、标签、布局 | 先参考已有设计；列出适用来源、入口和显示条件，统一到所有同类实例；实际看最终整页、密度、可读性和美观 | 按全局 `references/web-ui.md` 实际操作并亲自看图；[共用状态栏](sdk-ui.md)、[前端合同](frontend-migration-surfaces.md) |
| 菜单、焦点、移动端 | 刷新不打断正在使用的菜单/编辑/选区；有效动作可达，空菜单与重复反馈不出现；保留既有不可用原因 | 浏览器打开、选择、关闭、输入、窄屏操作；[前端合同](frontend-migration-surfaces.md) |
| 缓存、异步读取 | 已有内容先显示再刷新；失败保留旧值及真实时间；旧请求不串会话，缓存不授予写权限 | 阻塞请求期间检查首屏，再释放/失败/切换；[额度](quotas.md)、[SDK 界面](sdk-ui.md) |
| 身份、生命周期 | 节点、来源、UID、agent、原生 SID 与实例各司其职；无证据不猜配；切视图不接管进程或重发输入 | 打开、重开、跨节点及独立分支路径；[终端身份](terminal-identity.md)、[SDK](native-sdk.md) |
| 草稿、发送、队列 | 保留后续编辑；分清保存、接收、入队、原生消费与完成；取消、steer、停止不能混同；不确定结果不自动重放 | 实际发送、双页面、丢响应、刷新及恢复；[草稿](conversation.md)、[CLI 状态](cli-state.md)、[SDK 队列](native-sdk.md#持久输入与失败恢复) |
| 复制、移动、删除、分页 | 最小身份字节修改；保全物理依赖及用户关系决定；迁移可恢复交接；补页不移动实时检查点 | 页面操作及对应原生结果；[克隆](session-clone.md)、[移动](session-move.md)、[回收站](trash.md)、[分页](history-pages.md) |
| 测试与结论 | 最终版本走受影响 Chromium 用户路径；不放宽断言掩盖错误；区分合成、真实 CLI、视觉、未覆盖与环境阻断 | [验证清单](validation.md)；静态检查或截图生成不能替代交互与视觉验收 |
| 文档与交付 | 当前合同只写现行事实；未完成只进 TODO；按产物选择检查和发布范围，隔离并发改动，保留回滚 | 文档检查；按 AGENTS 提交推送；需要发布时先看 dry-run，再核对重启和健康结果，离线目标如实记录 |

验收记录只需写清最终版本、实际操作与数据形状、运行结果、亲自观察到的视觉
问题/修正及未覆盖范围。不能用旧 PASS、套件登记、委派结论或某一次上线证明后续版本。
具体用例留在浏览器夹具，规则不复制事故原话、逐提交目录、测试计数或临时绕过。
文档出现冲突时核实最近要求和最终实现，修正原权威段落及引用，不在末尾再加一段相反解释。
现行文档只写当前规则、合同和操作要求。对过去规则的评价、纠错经过、方案演变和
历史验收结果只放在明确标注的专门历史文档；当前合同中的兼容行为直接写明适用条件。

## Never do this

Do not expose the unauthenticated ordinary listener directly, commit credentials
or runtime data, or point destructive test cases at production paths. Use an
explicit ptyhost directory and coordinate use of the shared Cargo target directory.

## 1. Build

```sh
cargo build --release -p sessiondock
cargo build -p ptyhost
```

The `sessiondock` package also builds `sessiondock-hub`; use `--bin sessiondock-hub`
when selecting that binary alone for Hub federation.

## 2. Synthetic corpus

Point the read roots at the checked-in synthetic fixtures (see the README):
`crates/sessiondock/tests/fixtures/{claude,codex,grok}`. Copy them into a
private directory first when exercising writes.

## 3. Minimal environment

```sh
export SESSIONDOCK_BIND=127.0.0.1:8741
export SESSIONDOCK_WEB_DIR=web
# export the desired native read roots
./target/release/sessiondock
```

`sessiondock --check-config` parses and reports the effective configuration
without binding a listener or starting a CLI. The hostname defaults to the system
hostname and can be overridden with `SESSIONDOCK_HOSTNAME`.

## 4. Optional private runtime services

Use a private development tree for synthetic ptyhost, lifecycle, audit,
trash and bug-report data. The runtime follows each module's documented format;
it does not impose a blanket disjoint-root or 0700 policy on otherwise valid
layouts.

**Metadata — `SESSIONDOCK_STATE_DIR`.** Metadata reloads on every operation.
Missing, damaged or unsupported data behaves as empty; external edits are
allowed. See [metadata.md](metadata.md).

**Search text — `SESSIONDOCK_SEARCH_CACHE_DIR`.** Optional persistent cache;
bodies are built on search requests, with no startup or periodic scan. Cache
size and workers affect performance and eviction, not which valid
search input is accepted. See [read-model.md](read-model.md#搜索).

**Ptyhost and launcher.** Configure `SESSIONDOCK_PTYHOST_DIR`, lifecycle storage
and a launcher file when testing create/resume/takeover/stop. Launcher profiles
choose executable and fixed arguments; HTTP supplies session identity and cwd.
External sessions use the same observed-instance confirmation path.

**Files and attachments.** File reading is available without a separate root
grant. Writes are enabled with terminal operation. Test uploads and mutations
inside the synthetic tree; see [files.md](files.md).

**Hub.** Configure the node listener as a complete peer/protocol/token set. The
Hub registers literal IP HTTP or HTTPS URLs; HTTPS uses the system trust store.

**Audit and bug reports.** Configure their stores only when exercising persisted
diagnostics. Their absence does not disable unrelated file or attachment APIs.

## 5. Open the page

Open the loopback URL printed by SessionDock. `web/` is served directly.

## 6. Banner and capabilities

The injected capabilities describe current runtime support. Conversation SEND
needs metadata, terminal transport, lifecycle and file writes. File reads are available; file writes track
terminal operation. A false capability should reflect a truly unavailable
service or unsupported operation.

## 7. Watch a session

```sh
curl -N 'http://127.0.0.1:8741/api/watch?uid=codex:…'
```

## 8. Sample memory

```sh
rg 'VmRSS|VmHWM' "/proc/$SERVER_PID/status"
```

## 9. Shut down

SIGINT or SIGTERM stops admission and drains service work. It does not kill
independent ptyhost children. Inspect a test host with:

```sh
target/debug/ptyhost --dir <explicit-test-dir> list
```
