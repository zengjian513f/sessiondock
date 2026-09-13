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
