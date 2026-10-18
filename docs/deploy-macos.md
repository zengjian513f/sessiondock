# macOS 节点原生构建与 launchd 部署

这是 macOS（Apple Silicon，Command Line Tools 已装）上的节点构建与更新流程。不要记录
生产地址或凭据；`<PREFIX>` 是节点私有目录（如 `~/sessiondock`），`<NODE_WG_IP>` 是 Hub↔节点
私网地址。Linux 的目录布局、环境变量和 Hub 注册都不变（[deploy-hub.md](deploy-hub.md)）；
本文只列 macOS 与 Linux 不同的地方。

## 1. 平台差异（代码层，已内建）

| 能力 | Linux | macOS |
| --- | --- | --- |
| 托管进程身份（`process_identity`） | `linux_proc`：`/proc/<pid>/stat` | `macos_proc_pidinfo`：`proc_pidinfo(PROC_PIDTBSDINFO)`，start 为 epoch 微秒（`boot_time 0`、`1_000_000` ticks/s）；僵尸 = `not_visible`；内核 `EPERM` 在 owner 检查时 = `not_owned` |
| ptyhost 记录的 `boot_id` | `/proc/sys/kernel/random/boot_id` | `sysctl kern.bootsessionuuid`；跨重启的旧记录同样按 boot id / `kern.boottime` 判死 |
| `ptyhost list/kill/...` 的宿主探活 | `/proc/<pid>/stat` | `kill(pid, 0)` + libproc 僵尸判断（**不会**因缺 `/proc` 把活会话文件误删） |
| 外部 CLI 扫描 `/api/live` | `/proc` 扫描 | `scan.status = unsupported_platform`，`capabilities.live=false`；托管实例观测照常（同 Windows，[liveness.md](liveness.md)） |
| `hostname` | `/proc/sys/kernel/hostname` | `gethostname(2)`（launchd 不设 `HOSTNAME`） |
| PTY 尾输出 | 子进程退出后其它持有者仍可写 | 会话首领退出即 `revoke(2)`，master 立刻 EOF；`host_output.rs` 两条 drain 合同不适用 |
| 会话根 / cwd 的无符号链接祖先规则 | `/tmp` 等真实目录 | `/tmp`、`/var` 都是到 `/private/*` 的符号链接：会话根、host、附件根都放在 `/Users/<user>/…` 下 |
| Unix socket 路径上限 | 108 | **104**：`host_dir` 要短（`/Users/<user>/sessiondock/host` + `sessiondock-<32hex>.sock` = 76） |

## 2. 构建

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path
# 源码用 git archive 导出到独立目录（不在运行目录里跑 Cargo）
cd <SD_SOURCE>
~/.cargo/bin/cargo check -p sessiondock --all-targets --locked
~/.cargo/bin/cargo build --release --workspace --locked
shasum -a 256 target/release/sessiondock target/release/ptyhost
```

行为验证使用 headless Chromium；编译和部署检查按 [验证合同](validation.md) 与
[部署测试门](deployment.md#测试门build--test--push) 执行，不运行已移除的 HTTP-only 套件。

## 3. 运行目录与 launchd

`<PREFIX>/{bin,web,etc,state,lifecycle,host,audit,trash,search-cache,log}`，`etc/env`
同 Linux（`SESSIONDOCK_*`，只配置本机存在的会话根；`SESSIONDOCK_NODE_BIND=<NODE_WG_IP>:8743`）。
launchd 没有 `EnvironmentFile`，用一个 wrapper 装载：

```sh
#!/bin/sh
# <PREFIX>/bin/run-sessiondock
set -e
PREFIX=<PREFIX>
set -a; . "$PREFIX/etc/env"; set +a
cd "$PREFIX"
exec "$HOME/.local/bin/with-zshrc" "$PREFIX/bin/sessiondock"
```

用户级 LaunchAgent `~/Library/LaunchAgents/<label>.plist`：`ProgramArguments` = wrapper，
`RunAtLoad`/`KeepAlive` = true，`ThrottleInterval` 2，`Umask` 63（0077），`EnvironmentVariables`
只给 `HOME`、`PATH`、`LANG`，stdout/stderr 落到 `<PREFIX>/log/`。

```sh
sessiondock --check-config                           # 先 source etc/env
sessiondock --initialize-lifecycle <PREFIX>/lifecycle
sessiondock --write-bridge-settings <PREFIX>/etc/claude-bridge-settings.json
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/<label>.plist
launchctl print gui/$(id -u)/<label> | grep -E 'state|pid'
curl -s http://127.0.0.1:8741/api/meta
```

launchd 的初始环境不含交互 shell 的完整 PATH；上面的 `with-zshrc` 在服务启动时
加载一次登录环境并 `exec` 服务。CLI profile 的 `executable` 直接指定 CLI 绝对路径，
继承服务环境。包装器与环境漂移检测配置见 [shell-env.md](shell-env.md)。

## 4. 滚动更新

```sh
cp target/release/sessiondock <PREFIX>/bin/sessiondock.new && mv <PREFIX>/bin/sessiondock.new <PREFIX>/bin/sessiondock
rsync -a --delete web/ <PREFIX>/web/
launchctl kickstart -k gui/$(id -u)/<label>      # 只重启 sessiondock；ptyhost 会话不受影响
```

`ptyhost` 二进制变更时同样 `.new` + `mv`；已在跑的宿主继续用旧文件，直到该会话结束。

## 5. 验收

- `launchctl print` 状态 `running`，`/api/meta` 的 `hostname`、`node_id` 正确，`lsof -nP -iTCP:8743 -sTCP:LISTEN`
  只在私网地址上监听。
- Hub 侧 `sessiondock-hub register`（注册表在 Hub 启动时读取，注册后重启 Hub），`/api/nodes` 里
  `online: true`，`/api/sessions?nodes=<id>` 能列出本机会话。
- `POST /api/term/create {source: claude, cwd}` → `term/list` 的 `pending` 行 `running: true` → `POST /api/term/kill`。
- 机器不能休眠（`pmset -g` 里 `sleep 0`），WireGuard 需随开机自启，否则节点随之离线。

## 6. 用 deploy.py 部署

`deploy/sdtargets/macos.py`（kind `macos-node`）把第 2、4、5 节固化为
[deploy/deploy.py](deployment.md) 的一个目标。`deploy/targets.local.json`（不跟踪）里的条目形状见
`deploy/targets.example.json`：`ssh`/`ssh_port`、`prefix=<PREFIX>`、`service.launchd_label` 与
`service.gui_uid`、`build_on_target: true`、`extra.source_dir=<SD_SOURCE>`（必须在 `<PREFIX>` 之外）、
`extra.cargo`。

```sh
python3 deploy/deploy.py build                    # 产出 source.tar（git archive HEAD）+ web 快照
python3 deploy/deploy.py push --stage <stage> --targets <name> --dry-run   # 只 probe 并打印计划
python3 deploy/deploy.py push --stage <stage> --targets <name>    # 完整一轮；--web-only 跳过构建；ptyhost 仅在相关改动时加 --with-ptyhost
python3 deploy/deploy.py rollback --targets <name> --backup <PREFIX>/backup-deploy-<short>-<UTC stamp>
```

每一步在节点上做的事（远程命令全部无通配，登录 shell 是 zsh 也不会中断）：

- probe：launchd 的 state/pid、`/api/meta.build`、二进制 `.version`、ptyhost PID、host 记录数和 `etc/deployed-commit`。
- stage：`source.tar` 上传到 `<SD_SOURCE>/.deploy/`，清空 `<SD_SOURCE>` 里 `target/` 以外的一切再
  `tar -x`；stage 的测试模式（`deploy.py --test`，见 [deployment.md](deployment.md#测试门build--test--push)）
  不是 `none` 时先按第 2 节跑 `mkdir -p /private/tmp/sdtest && TMPDIR=/private/tmp/sdtest <cargo> check
  -p sessiondock（含 ptyhost 改动时再加 -p ptyhost）--all-targets --locked`（超时 1800 s；失败即该目标 `FAILED`，在构建和换入之前中止，`.deploy-test.log`
  留在 `<SD_SOURCE>` 供查看）；`cargo build --release --locked -p sessiondock`（超时 900 s，M4 上热构建约 30–40 s）；
  产物拷成 `<PREFIX>/bin/<name>.new` 并记录与 stage 相同的日期版本号；
  web 快照 rsync 到 `<PREFIX>/web.staging/`，携带 `release-version.txt` 日期版本号。
- backup：`cp -Rp bin web <PREFIX>/backup-deploy-<short>-<UTC stamp>/`。
- swap：`mv -f bin/<name>.new bin/<name>`；`rsync -a --delete web.staging/ web/`。
- restart：`launchctl kickstart -k gui/<uid>/<label>`——只重启 sessiondock。
- verify：健康端点应答、launchd 正在运行且 PID 已变、二进制版本与 stage 相同，发布 web 时
  `build` 等于 stage 日期版本，ptyhost PID 和 host 记录保留。
- rollback：从备份用同一条 `.new` + `mv` 路径恢复二进制、`rsync --delete` 恢复 `web/`，再 kickstart。

历史验收：2026-09-15 在 macOS 节点上实测 push → rollback → push，各轮约 40 秒，
重启前后 ptyhost PID 不变。当时使用内容摘要；当前日期版本流程见上文。
