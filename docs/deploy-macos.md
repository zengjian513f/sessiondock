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
~/.cargo/bin/cargo build --release --workspace --locked
shasum -a 256 target/release/sessiondock target/release/ptyhost
```

M4 上全量 release 约 40 s。跑测试时 **必须** 把 `TMPDIR` 指到一个真实、短的目录，否则
`/var/folders/...` 的符号链接会触发 launcher 的 `UnsafePath`，长路径会撞 socket 上限：

```sh
mkdir -p /private/tmp/sdtest && TMPDIR=/private/tmp/sdtest ~/.cargo/bin/cargo test --workspace --no-fail-fast
```

## 3. 运行目录与 launchd

`<PREFIX>/{bin,web,etc,state,delivery,lifecycle,host,audit,trash,search-cache,log}`，`etc/env`
同 Linux（`SESSIONDOCK_*`，只配置本机存在的会话根；`SESSIONDOCK_NODE_BIND=<NODE_WG_IP>:8743`）。
launchd 没有 `EnvironmentFile`，用一个 wrapper 装载：

```sh
#!/bin/sh
# <PREFIX>/bin/run-sessiondock
set -e
PREFIX=<PREFIX>
set -a; . "$PREFIX/etc/env"; set +a
cd "$PREFIX"
exec "$PREFIX/bin/sessiondock"
```

用户级 LaunchAgent `~/Library/LaunchAgents/<label>.plist`：`ProgramArguments` = wrapper，
`RunAtLoad`/`KeepAlive` = true，`ThrottleInterval` 2，`Umask` 63（0077），`EnvironmentVariables`
只给 `HOME`、`PATH`、`LANG`，stdout/stderr 落到 `<PREFIX>/log/`。

```sh
sessiondock --check-config                           # 先 source etc/env
sessiondock --initialize-delivery <PREFIX>/delivery
sessiondock --initialize-lifecycle <PREFIX>/lifecycle
sessiondock --write-bridge-settings <PREFIX>/etc/claude-bridge-settings.json
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/<label>.plist
launchctl print gui/$(id -u)/<label> | grep -E 'state|pid'
curl -s http://127.0.0.1:8741/api/meta
```

CLI 启动 profile 的 `executable` 用 `with-zshrc`（`#!/bin/zsh`，`source ~/.zprofile ~/.zshrc` 后
`exec "$@"`）：launchd 起的进程没有 brew / `~/.local/bin` 的 PATH。

## 4. 滚动更新

```sh
cp target/release/sessiondock <PREFIX>/bin/sessiondock.new && mv <PREFIX>/bin/sessiondock.new <PREFIX>/bin/sessiondock
rsync -a --delete legacy-web/ <PREFIX>/web/
launchctl kickstart -k gui/$(id -u)/<label>      # 只重启 sessiondock；ptyhost 会话不受影响
```

`ptyhost` 二进制变更时同样 `.new` + `mv`；已在跑的宿主继续用旧文件，直到该会话结束。

## 5. 验收

- `launchctl print` 状态 `running`，`/api/meta` 的 `hostname`、`node_id` 正确，`lsof -nP -iTCP:8743 -sTCP:LISTEN`
  只在私网地址上监听。
- Hub 侧 `sessiondock-hub register`（注册表在 Hub 启动时读取，注册后重启 Hub），`/api/nodes` 里
  `online: true`，`/api/sessions?nodes=<id>` 能列出本机会话。
- `POST /api/term/create {source: claude, cwd}` → `new-status` `running: true` → `POST /api/term/kill`。
- 机器不能休眠（`pmset -g` 里 `sleep 0`），WireGuard 需随开机自启，否则节点随之离线。
