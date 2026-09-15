# 舰队部署工具 `deploy/deploy.py`

`deploy/deploy.py` 把"在构建机上 release 构建一次，再推到每台机器"的手工流程固定下来：每台目标机
按同一套步骤走，失败自动回滚，最后给出对齐表格和 JSON 报告。它取代的手工做法（备份到
`backup-deploy-*`、`bin/sessiondock.new` + `mv`、`rsync -a --delete legacy-web/ web/`、重启、curl
`/api/meta`）形状不变，所以旧备份和新备份长得一样。不要把真实地址写进任何被跟踪的文件；
真实清单只在未跟踪的 `deploy/targets.local.json`。

## 命令

```sh
python3 deploy/deploy.py build    [--allow-dirty] [--web-from-head] [--with-ptyhost] [--web-only]
                                  [--test none|affected|full] [--test-base REF] [--test-timeout S]
python3 deploy/deploy.py push     [--targets a,b | --all] [--stage DIR] [--web-only | --bin-only]
                                  [--with-ptyhost] [--dry-run] [--parallel N] [--keep-backups N]
                                  [--health-timeout S] [--targets-file PATH] [-v]
python3 deploy/deploy.py deploy   # build → test → push，标志相同；--test 在这里默认 affected
python3 deploy/deploy.py rollback --targets X [--backup DIR]
```

- `build`：`git status --porcelain -- crates legacy-web Cargo.toml Cargo.lock` 非空即拒绝并列出文件；
  `--allow-dirty` 才继续（此时 web 快照来自工作树，`--web-from-head` 可改回 HEAD，用于共享
  checkout 里别人的未提交前端改动）。`--web-only` 完全不跑 cargo。默认 `--targets-file` 是
  `deploy/targets.local.json`（缺省时退回 `targets.example.json`），其中 `build` 段给出 cargo
  路径与包名；`sessiondock-hub` 是 `sessiondock` 包里的第二个 bin，工具用 `cargo metadata` 解析。
  `--test`（默认 `none`，build 常用来做 dry run）在构建完成后按[测试门](#测试门build--test--push)跑测试。
- `push`：默认取 `target/deploy/` 下最新的 stage，默认并行 4、保留 5 份备份（`0` = 不清理）、健康
  超时 45 s。`--dry-run` 只做 probe 并打印每台的计划，什么都不上传。任一目标不是 OK 就退出 1。
  `push` 在构建机上**从不跑测试**（它只是把已构建的 stage 发出去），但会先打印这个 stage 是按哪种模式
  验证过的（`stage tests: mode=… result=… base=… suites=…`）。
- `deploy`：build → test → push。`--test` 默认 `affected`；任一套件失败就退出 1，**什么都不上传**。
- `rollback`：不给 `--backup` 时取目标机上**mtime 最新**的 `backup-deploy-*`（手工备份的时间戳是
  本地时间、本工具是 UTC，按名字排不可靠）。`--backup` 只接受匹配
  `backup-deploy-<hex>-<YYYYmmdd>-<HHMMSS>` 的目录。
- `status` 不在本工具里：只读的舰队状态见 `deploy/fleet_status.py`。

## 三种产物（`target/deploy/<UTC 时间戳>-<short>/`，同一秒内再建则加 `-2`、`-3` 后缀）

| 产物 | 内容 | 用途 |
| --- | --- | --- |
| `bin/<name>` + `artifacts.json` 里的 `sha256` | `cargo build --release --locked` 的 glibc 二进制，从 `target/release/` 拷进 stage | `linux-node`、`hub` 直接上传；stage 一旦生成就不再受后续构建影响 |
| `web/` | `git archive HEAD legacy-web` 解出的快照（或 `--allow-dirty` 的工作树，排除 `node_modules`、`.DS_Store`、`*.swp`） | 所有 kind 的 `web/` |
| `source.tar` | `git archive --format=tar HEAD` | `build_on_target` 的 kind（macOS、Windows）在节点上原生构建 |

`artifacts.json` 还记录 commit、`dirty`、`built_at`、web 来源和 cargo 命令，以及测试门的结果
（`test_mode`、`test_base`、`test_full`、`test_suites`、`test_result`、`test_log`）；`logs/` 放 cargo 日志、
测试门的 `tests.log` / `tests.json` / `validation/<suite>.log` 和每台目标的 `<name>.log`；每次 push/rollback
写一份 `report-<stamp>.json`。

## 测试门（build → test → push）

`deploy/testplan.py`（仅标准库）实现三种模式，`--test` 选择：

| 模式 | 含义 | 跑什么 |
| --- | --- | --- |
| `none` | **1 不测试直接上线** | 打印 `tests skipped by --test none` 后继续；`build` 的默认值 |
| `affected` | **2 只测本次改动影响到的组件** | 把 `git diff --name-only <base>..HEAD`（`--allow-dirty` 时并上未提交文件）按下表映射到套件，`python3 tests/run_validation.py --only <names> --binary <stage 的 bin/sessiondock，web-only 时退回 target/release/sessiondock>`；`deploy` 的默认值 |
| `full` | **3 全量测试** | `python3 tests/run_validation.py --binary …`，即默认全量扫描（`*_real` 付费套件与 run_validation 一样默认排除） |

顺序固定为 build → test → push：测试失败先于任何上传，退出 1，并在 stderr 列出失败套件名与各自的日志
路径（`<stage>/logs/validation/<suite>.log`）。测试输出实时流到控制台（run_validation 自己的进度行）并
落盘到 `<stage>/logs/tests.log`；整轮超时 `--test-timeout`（默认 2400 s）。跑之前一定先打印计划：
base commit 及其来源、改动文件（数量 + 前 20 个及命中的规则）、选中的套件（或 `full sweep` 与触发它的
路径）、额外脚本。

**测试按平台跑，一个平台一次，绝不按节点跑。** Linux：构建机上 push 之前跑一次，覆盖所有 `linux-node`
和 Hub（它们拿的是同一个二进制）。macOS / Windows 在节点上原生构建，所以同一个模式（记录在 stage 的
`test_mode` 里，随 `push` 传给处理器的 `DeployOptions.test_mode`）在它们的 `stage()` 里驱动一步原生测试：
解出源码之后、构建之前，macOS 跑 `TMPDIR=/private/tmp/sdtest <cargo> test --workspace --locked`
（[deploy-macos.md](deploy-macos.md) §2），Windows 在 `build.cmd` 里跑
`<toolchain_bin>\cargo.exe test -p sessiondock --locked`（`RUSTC`/`RUSTDOC` 指向同一工具链，
[deploy-windows.md](deploy-windows.md) §2）；失败即该目标 `FAILED`，在换入任何东西之前中止，错误信息带
节点上的 `.deploy-test.log` 路径和本地的 `<stage>/logs/<name>.log`；`none` 则跳过。Python/浏览器套件只在
Linux 构建机上跑。

### base commit（`affected` 从哪里开始算"改动"）

1. `--test-base REF` 显式指定；
2. 否则取所选目标 `etc/deployed-commit` 标记里的 commit（`deploy` 在测试前对每台目标做一次只读 probe）。
   多台不一致时取**最旧**的那个（`git rev-list --count <sha>..HEAD` 最大，即 diff 最大）；
3. 没有任何目标给出标记（或 `build` 单独运行——它没有目标可 probe）时退回 `origin/main`，没有 `origin/main`
   再退回 `HEAD~1`。

### 映射表（`deploy/testplan.py` 的 `RULES` / `MODULE_SUITES`）

规则只写"路径前缀 → 套件名模式"；套件名**从不硬编码**，每个模式在运行时对 `run_validation.py --list`
的输出做 fnmatch，取所有改动文件结果的并集。带 `/` 的条目是不在扫描里的脚本，直接 `python3` 跑（若它的
stem 恰好是套件名则按套件跑；某条改动触发全量时这些脚本仍照跑）。按顺序首个命中的规则生效；没有规则
命中 → 全量。

| 改动路径 | 选中 |
| --- | --- |
| `.gitignore`、`.gitattributes`、`.editorconfig` | 不测（不影响产物） |
| `docs/**`、任何 `*.md` | 只跑文档检查：`tests/check_docs_links.py`、`tests/check_agents_md.py` |
| `crates/ptyhost/**`、`crates/ptyhost-client/**` | `cargo_*`（run_validation 没有按 crate 的 Rust 车道，`cargo_test` 就是 workspace）+ 依赖 ptyhost 的 Python 套件：`terminal*`、`term_*`、`lifecycle*`、`cutover*`、`host*`、`native_*`、`managed_*`、`send_*`、`live_*`、`session_stop_*`、`pending_*`、`restart_state_*`、`bug_report_*`、`grok_raw_send_*` |
| `crates/sessiondock/src/<module>/**`、`src/<module>.rs` | `cargo_*` + 模块别名：`sessions` → `history_*`、`sessions_*`、`messages_*`、`native_*`、`*_parity`、`codex_*`、`claude_*`、`grok_*`、`agent_*`、`orphan_*`、`continued_*`、`fork_*`、`list_rows_*`、`input_history_*`、`inventory_*`、`debug_runs_*`、`symlink_*`、`unicode_*`、`names_*`、`budget_*`、`reader_pool_*`、`sse_*`、`rewind_*`；`terminal` → `terminal_*`、`term_*`、`managed_*`、`session_stop_*`、`grok_raw_send_*`；`lifecycle` → `lifecycle_*`、`send_*`、`outbox*`、`pending_*`、`restart_state_*`、`live_*`、`session_stop_*`；`hub`/`hub_config`/`bin` → `hub_*`（`hub` 另加 `node_auth_*`）；`search` → `search_*`；`media` → `media_*`、`native_*`；`files` → `file*`；`delivery` → `delivery_*`、`send_*`、`outbox*`；`bug_report` → `bug_report_*`；`audit` → `audit_*`；`metadata` → `metadata_*`、`prefs_*`；`trash` → `trash_*`；`runtime` → `live_*`、`spawned_by_*`、`managed_*`、`restart_state_*`、`lifecycle_*`；`bridge` → `claude_prompt_*`、`prompt_*`、`live_*`；`native_replay` → `native_*`；`assets` → `static_assets_*`、`meta_*`、`prefs_*`。表里没有的模块用 `<module>*`，一个都匹配不上就全量 |
| `crates/sessiondock/src/api/**`、`main.rs`、`lib.rs`、`config.rs`、`security.rs`、`state.rs`、`error.rs` | 横切面 → 全量 |
| `crates/sessiondock/tests/fixtures/**` | 全量（Python 套件也用这些 fixture） |
| `crates/sessiondock/tests/**`（其它） | `cargo_*` |
| `legacy-web/**` | `node_contracts` + 所有 `*_browser*` + `brand_names_check` |
| `deploy/**` | `deploy_*`（`deploy_native_handlers`、`deploy_testplan`）+ 脚本 `tests/deploy_dry_run.py` |
| `tests/<stem>.py` | 若 `<stem>` 是套件 → 该套件及 `<stem>_*`（如 `lifecycle_browser` 带上 `lifecycle_browser_native_binding`）；否则取同前缀的套件（`hub_fake_node.py` → `hub_*`）；仍没有（`fake_claude_cli.py`、`python_oracle.py`）→ 全量；`*.mjs` → `node_contracts`；`tests/fixtures/**` → 全量；`check_docs_links.py`、`check_agents_md.py`、`deploy_dry_run.py` 改自己就跑自己 |
| `Cargo.toml`、`Cargo.lock`、`.github/**`、其它任何未命中路径（`web/**`、`reference/**` …） | 全量 |

离线回归：`tests/deploy_testplan.py`（映射、base 规则、三种模式的 CLI 行为，runner 被替身替换）；
`tests/deploy_native_handlers.py` 钉住 macOS / Windows 处理器里原生测试一步的命令序列。

## 每台目标的步骤与不变量

顺序固定为 `probe → plan → stage → backup → swap → restart → verify → write_marker → prune_backups`
（`deploy/sdtargets/base.py` 是合同，每个 kind 一个模块实现这些步骤）。`backup` 成功之后任何一步失败：
`rollback(backup_dir) → restart → 再 probe`，结果记 `ROLLED_BACK`（仍算失败）；不可达记 `SKIPPED`；
kind 的处理模块缺失或坏掉记 `UNSUPPORTED`，不会让整轮崩溃。

每个处理器都必须遵守（摘自 `base.py`）：

- 绝不原地覆盖正在运行的二进制：上传为 `<name>.new` 再 rename 过去（Linux "Text file busy"、
  Windows 文件锁）。
- 绝不动 ptyhost 会话宿主：只重启 Web 服务。`probe()` 记下 ptyhost pid（或 host 记录），`verify()`
  证明同一批仍然活着。
- 绝不动前缀下的私有数据与配置（`etc/`、`state/`、`delivery/`、`lifecycle/`、`host/`、`audit/`、
  `trash/`、`search-cache/`、`hub/`）：只有 `bin/` 与 `web/` 会变（外加 `etc/deployed-commit` 标记和
  `backup-deploy-*`）。
- 备份放在 `<prefix>/backup-deploy-<short commit>-<UTC 时间戳>/{bin,web}`。
- `verify()` 必须看到：服务 active、目标机自己的 loopback 上 `/api/meta` 有应答、磁盘上二进制的
  SHA-256 等于 stage 里的产物（原生构建则等于刚构建的文件）、web 变了时 `build` 字段也变了。
- dry run 只做 `probe()` 并打印计划，什么都不上传。

## `verify` 证明了什么

`/api/meta` 的 `build` 是服务启动时对 web 资源快照 + capabilities 算的 SHA-1，**不是**二进制的哈希。
所以：web 内容变了（工具在目标机上比较 `web/` 的内容摘要）就要求 `build` 变化；只换 Rust 二进制时
`build` 不必变，靠 `sha256sum` 核对；web 变了就必须重启。`verify` 另外要求：单元 active、每个上传的
二进制哈希等于 stage、probe 时的每个 ptyhost pid 仍在、host 记录数不少于之前。`--bin-only` 跳过 web
（也不要求 `build` 变化），`--web-only` 跳过全部二进制步骤。

## Linux 节点与 Hub 的实现（`linux.py`、`hub.py`）

- probe：一次 ssh 往返跑一段 POSIX 脚本（远程统一用 `sh -c` 包住，登录 shell 是 zsh 也不会因为通配
  失配而中断），输出 `key=value`：`is-active`、`/api/meta`、每个二进制的 `sha256sum`、`pgrep -x ptyhost`、
  `ls host | wc -l`、`etc/deployed-commit`、web 内容摘要。
- stage：二进制 → `bin/<name>.new`（上传后再校验哈希）；web → `<prefix>/web.staging/`。Linux 节点与 Hub
  不在节点上跑测试：它们拿到的二进制已经在构建机上按 stage 的 `test_mode` 验证过。
- swap：`mv -f bin/<name>.new bin/<name>`；在目标机上 `rsync -a --delete web.staging/ web/` 后删掉 staging。
- restart：`systemctl --user restart <unit>`；节点单元是 `KillMode=process`，分离的 ptyhost 不受影响。
- 备份始终同时拷 `bin/` 里声明的二进制和整个 `web/`；rollback 用同一条 `.new` + `mv` 路径恢复二进制、
  `rsync -a --delete` 恢复 web，再重启。ptyhost 只在 `--with-ptyhost` 时随行，同样走 `.new` + rename，
  正在跑的宿主继续用旧 inode 直到退出。
- prune 只删匹配 `^backup-deploy-[0-9a-f]+-[0-9]{8}-[0-9]{6}$` 的目录，按 mtime 保留最新 N 份。
- Hub 是同一处理器的子类：`sessiondock-hub.service`、二进制 `sessiondock-hub`、没有 `host/` 也不发
  ptyhost。

## 新增目标

复制 `deploy/targets.example.json` 为 `deploy/targets.local.json` 并填入真实主机；每项：`name`、
`kind`（`linux-node` / `hub` / `macos-node` / `windows-node`）、`ssh`（`user@<NODE_WG_IP>`，`null`
表示本机）、可选 `ssh_port`（ssh-config 别名不要写端口，rsync 才不会强加 `-e "ssh -p"`）、`prefix`、
`health_url`（在目标机 loopback 上取）、`service`、`binaries`、`build_on_target`、`extra`。机器本身的
准备（目录、`etc/env`、单元文件、Hub 注册）见 [deploy-hub.md](deploy-hub.md)，macOS 原生构建与 launchd
见 [deploy-macos.md](deploy-macos.md)，Windows 原生构建与会话 1 监督器见 [deploy-windows.md](deploy-windows.md)。

真实清单不入库：Hub 自身无鉴权，Hub↔节点走 WireGuard 私网，地址、端口与防火墙规则按
[AGENTS.md](../AGENTS.md#scope-and-boundaries) 的规定不进仓库；`.gitignore` 已排除
`deploy/targets.local.json`。
