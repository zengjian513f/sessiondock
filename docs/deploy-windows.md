# Windows 节点原生构建与滚动部署

这是 Windows OpenSSH 的构建和更新流程。不要记录生产地址或凭据。
`SD_SOURCE`、`SD_RUNTIME`、`SD_BACKUP` 必须是已核对的绝对路径。

## 1. OpenSSH 下直接使用真实 Rust 工具链

Windows SSH 可能拒绝 rustup shim，并返回 OS error 448。因此直接调用稳定
MSVC 工具链中的 `cargo.exe`，并把 `RUSTC`、`RUSTDOC` 都指向该工具链；
`cargo test` 的 doctest 会单独启动 `rustdoc.exe`。

```bat
set "SD_TOOLCHAIN=%USERPROFILE%\.rustup\toolchains\stable-x86_64-pc-windows-msvc"
set "RUSTC=%SD_TOOLCHAIN%\bin\rustc.exe"
set "RUSTDOC=%SD_TOOLCHAIN%\bin\rustdoc.exe"
"%SD_TOOLCHAIN%\bin\cargo.exe" --version
"%RUSTC%" --version
"%RUSTDOC%" --version
```

文件缺失或版本检查失败时停止。`rustup which` 仅用于诊断。

## 2. 在独立构建目录中构建当前工作区

复制当前工作区的全部内容，包括未提交改动。独立目录只防止构建污染运行目录；
不得借此筛选源码。不要在生产运行目录中执行 Cargo。

```bat
set "SD_SOURCE=<independent-build-directory>"
cd /d "%SD_SOURCE%"
set "SD_TOOLCHAIN=%USERPROFILE%\.rustup\toolchains\stable-x86_64-pc-windows-msvc"
set "RUSTC=%SD_TOOLCHAIN%\bin\rustc.exe"
set "RUSTDOC=%SD_TOOLCHAIN%\bin\rustdoc.exe"

"%SD_TOOLCHAIN%\bin\cargo.exe" test -p sessiondock --locked
"%SD_TOOLCHAIN%\bin\cargo.exe" build -p sessiondock --release --locked
certutil -hashfile target\release\sessiondock.exe SHA256
```

测试、release 构建和 SHA-256 记录都成功后才能部署。错误 448 属于工具链问题；
修正后重建，不得复用旧 EXE。

## 3. 先暂存，再停止 Web 服务

把新 EXE 暂存为 `sessiondock.pending.exe`。核对哈希，再备份线上 EXE。
完成前不要停止服务。

```bat
set "SD_RUNTIME=<private-runtime-directory>"
set "SD_BACKUP=<new-timestamped-backup-directory>"
certutil -hashfile "%SD_RUNTIME%\bin\sessiondock.pending.exe" SHA256
mkdir "%SD_BACKUP%"
copy /y "%SD_RUNTIME%\bin\sessiondock.exe" "%SD_BACKUP%\sessiondock.exe"
```

使用现有监督器停止 SessionDock Web。不要停止 `ptyhost.exe`，也不要删除私有数据。

停止后原子替换 EXE并启动。任一步失败都从 `SD_BACKUP` 回滚。

## 4. 部署后验收

每次更新检查：

1. Web 服务由既有监督器管理并处于健康状态；配置检查和 `/api/health` 成功。
2. 线上 `sessiondock.exe` 的 SHA-256 等于第 2 节记录。
3. host 记录数和既有 `ptyhost.exe` 的 PID、创建时间、Session ID 不变。
4. 用只读或合成探针验证修复；不要创建付费会话或真实附件。
5. 记录回滚目录、哈希、健康结果和平台限制；新工作写入 `TODO.md`。

按当前工作区部署有改动的静态文件和 EXE。复制前确认构建输入未变化。

## 5. 用 deploy.py 部署

`deploy/sdtargets/windows.py`（kind `windows-node`）把第 1–4 节和 `restart-session1.cmd` 的
操作手法固化为 [deploy/deploy.py](deployment.md) 的一个目标。`deploy/targets.local.json`（不跟踪）
里的条目形状见 `deploy/targets.example.json`：`ssh`/`ssh_port`、`prefix=<SD_RUNTIME>`、
`service.restart_cmd`（节点上的 `restart-session1.cmd`）、`build_on_target: true`、
`extra.source_dir=<SD_SOURCE>`（必须在 `<SD_RUNTIME>` 之外）、`extra.toolchain_bin`
（`%SD_TOOLCHAIN%\bin`）。可选 `service.python` / `service.shortcut`；缺省时 probe 从
`restart_cmd` 的 `set PY=` / `set LNK=` 行读取。

```sh
python3 deploy/deploy.py build
python3 deploy/deploy.py push --targets <name> --dry-run   # 只 probe，并把要上传的三份 .cmd 全文打印出来
python3 deploy/deploy.py push --targets <name>
python3 deploy/deploy.py rollback --targets <name> --backup <SD_RUNTIME>\backup-deploy-<short>-<UTC stamp>
```

与 Linux/macOS 的差别：

- 经 SSH 到达的命令由 cmd.exe 执行，且单行 `a & echo %ERRORLEVEL%` 里的 ERRORLEVEL 在解析时就已展开，
  拿不到真实退出码；所以多步操作都以 ASCII `.cmd` 文件（模板在 `deploy/windows/`，批处理文件里
  出现非 ASCII 字节会让 `cd /d` 失效）上传到 `<SD_SOURCE>\.deploy\` 后执行。没有 rsync，上传用 scp。
- stage：提交的 zip（由 `source.tar` 在构建机上转换）上传后 `Expand-Archive -Force` 到 `<SD_SOURCE>`
  （保留 `target\`），`build.cmd` 用工具链目录里**真实的** `cargo.exe` 并把 `RUSTC`/`RUSTDOC` 指向同一
  工具链（`.cargo\bin` 下的 rustup shim 是 reparse point，提权的 SSH 进程执行会报 448）；stage 的测试
  模式（`deploy.py --test`，见 [deployment.md](deployment.md#测试门build--test--push)）不是 `none` 时
  `build.cmd` 渲染 `TEST=1`，在解压之后、构建之前按第 2 节跑 `cargo.exe test -p sessiondock --locked`
  （`TEST_FAILED` → 退出码 18，该目标 `FAILED`，什么都没暂存；日志 `<SD_SOURCE>\.deploy-test.log`），
  处理器还要求输出里有 `TEST_OK`；除退出码外
  还比较 `target\release\sessiondock.exe` 构建前后的 mtime，编译过但文件没更新即失败；产物
  `copy /y` 成 `bin\sessiondock.new.exe`，`certutil -hashfile … SHA256` 作为期望值；web 快照 zip
  解到 `web.staging\`。
- backup：`backup.cmd` 拷 `bin\*.exe`、`robocopy /MIR` 整个 `web\`（robocopy 退出码 < 8 为成功）。
- swap + restart 是**同一份** `swap-restart.cmd`，逐行镜像 `restart-session1.cmd`：
  `process_identity.py --include-supervisor` 停旧进程 → `move /y bin\sessiondock.new.exe
  bin\sessiondock.exe`（运行中的 exe 被锁，必须停后再换；旧文件先改名停放，ptyhost.exe 宿主不受影响）
  → `robocopy /MIR web.staging web` → 清 STOP/lock → `/IT` 一次性计划任务
  `explorer.exe SessionDock.lnk` 在桌面会话 1 里拉起 → 打印 pid/session/父进程与 `term/list` 状态码。
  因此重启发生在 swap 里，`restart()` 之后是空操作；rollback 把备份拷回 `.new.exe` / `web.staging` 后
  再跑同一份脚本。绝不从 SSH 直接 `start_sessiondock.py start`（会造出 session 0 的 supervisor）。
- verify：`/api/meta` 在健康超时内应答且 `sessiondock.exe` 的 pid 已变、`Win32_Process` 里它的
  SessionId 为 1、certutil 哈希等于 stage 时的值、web 内容变了则 `build` 必须变、probe 时的每个
  `ptyhost.exe` pid 仍在、host 记录数不少于之前；`etc\deployed-commit` 由 PowerShell `Set-Content` 写入。

**状态（2026-09-15）**：这条自动化路径是按上述手工配方和转录记录写成的，只有
`tests/deploy_native_handlers.py` 里用假 Shell 钉住的命令序列做过离线验证；Windows 节点当时离线，
**尚未在真实机器上跑过一次**。首次实跑前先 `--dry-run` 逐行核对打印出的三份 `.cmd`。
