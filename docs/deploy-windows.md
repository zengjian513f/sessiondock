# Windows 节点原生构建与滚动部署

本文固化通过 Windows OpenSSH 会话构建和更新 SessionDock 节点的唯一推荐流程。
生产地址、凭据和实际目录不进入仓库；下文的 `SD_SOURCE`、`SD_RUNTIME` 和
`SD_BACKUP` 必须由操作者在目标机上设为经过核对的私有绝对路径。

## 1. OpenSSH 下直接使用真实 Rust 工具链

Windows 的 `%USERPROFILE%\.cargo\bin\cargo.exe` 通常是 rustup shim。远程 SSH
登录的挂载点信任策略可能允许 shim 本身启动，却拒绝它随后解析或启动另一个 shim，
并返回 OS error 448（路径包含不受信任的装入点）。因此生产构建不得依赖 `PATH`，
也不得使用 `cargo ...` 或 `rustup run ... cargo ...`。

在远程 `cmd.exe` 会话中固定使用稳定 MSVC 工具链目录内的真实可执行文件，并显式
指定真实 `rustc.exe`：

```bat
set "SD_TOOLCHAIN=%USERPROFILE%\.rustup\toolchains\stable-x86_64-pc-windows-msvc"
set "RUSTC=%SD_TOOLCHAIN%\bin\rustc.exe"
"%SD_TOOLCHAIN%\bin\cargo.exe" --version
"%RUSTC%" --version
```

任何一个文件不存在或版本检查失败时立即停止，不尝试其他随机入口，也不触碰运行目录。
可以用 `rustup which cargo` 和 `rustup which rustc` 做只读诊断；其输出应落在同一个
`stable-x86_64-pc-windows-msvc` 工具链目录，但构建命令仍直接调用上面的真实路径。

## 2. 只在隔离源码快照中构建

先把待发布的精确提交复制到独立源码目录。未提交修复需要显式覆盖到该快照并记录；不要
在生产运行目录中执行 Cargo，也不要让构建产物、测试状态或源码进入私有运行数据目录。

```bat
set "SD_SOURCE=<isolated-source-snapshot>"
cd /d "%SD_SOURCE%"
set "SD_TOOLCHAIN=%USERPROFILE%\.rustup\toolchains\stable-x86_64-pc-windows-msvc"
set "RUSTC=%SD_TOOLCHAIN%\bin\rustc.exe"

"%SD_TOOLCHAIN%\bin\cargo.exe" test -p sessiondock --locked
"%SD_TOOLCHAIN%\bin\cargo.exe" build -p sessiondock --release --locked
certutil -hashfile target\release\sessiondock.exe SHA256
```

小修可以先运行对应的定向测试以快速发现问题，但发布前仍按当前批次要求完成规定的测试。
只有测试、release 构建和 SHA-256 记录全部成功后，才进入部署阶段。错误 448 是构建环境
错误，不是代码失败；按第 1 节纠正工具链路径后重新从隔离快照验证，禁止拿旧 EXE 冒充
本次构建结果。

## 3. 先暂存，再停止 Web 服务

将新 EXE 复制为运行目录中的临时文件，例如 `sessiondock.pending.exe`。先核对暂存文件
哈希与第 2 节产物一致，再创建新的、不可复用的时间戳回滚目录，并复制当前线上 EXE。
到这一步为止不停止服务，也不替换线上文件。

```bat
set "SD_RUNTIME=<private-runtime-directory>"
set "SD_BACKUP=<new-timestamped-backup-directory>"
certutil -hashfile "%SD_RUNTIME%\bin\sessiondock.pending.exe" SHA256
mkdir "%SD_BACKUP%"
copy /y "%SD_RUNTIME%\bin\sessiondock.exe" "%SD_BACKUP%\sessiondock.exe"
```

停止和启动必须使用部署已有的监督器脚本或服务管理器。只停止 SessionDock Web 进程；
不得 `taskkill` 独立的 `ptyhost.exe`，不得删除 host、lifecycle、delivery、state、audit、
trash 或其他私有目录。

停止成功后，将 `.pending.exe` 原子改名为 `sessiondock.exe`，再立即启动监督器。若替换、
启动、配置或健康检查任一步失败，停止新 Web 进程，从 `SD_BACKUP` 恢复旧 EXE并重新启动。

## 4. 部署后验收

每次更新至少核对：

1. Web 服务由既有监督器管理并处于健康状态；配置检查和 `/api/health` 成功。
2. 线上 `sessiondock.exe` 的 SHA-256 等于第 2 节记录。
3. 更新前后的 host 记录数、独立 `ptyhost.exe` PID、创建时间和 Windows Session ID
   保持不变；Web 服务重启不应重启或收割受管 CLI。
4. 使用只读或合成探针验证本次修复路径；不要为探针启动付费 CLI、创建真实会话或写入
   真实附件。
5. 记录回滚目录、最终哈希、健康结果及任何平台限制到迁移账本。

前端静态文件与 EXE 必须各自按实际改动决定是否部署。发现另一批并发更新时，不覆盖
对方文件；先核对提交、线上哈希和文件归属，再合并为一份经过验证的发布快照。
