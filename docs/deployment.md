# 舰队部署工具 `deploy/deploy.py`

`deploy/deploy.py` 从同一提交快照构建、验证并发布到选定目标，失败自动回滚。
真实清单只在未跟踪的 `deploy/targets.local.json`；不要把真实地址写进被跟踪文件。

## 从任意构建机执行

当前工作区所在机器就是构建机；它与任何目标节点都没有默认对应关系。
先运行 `hostname`，检查本机工具链和 `deploy/targets.local.json`。
可迁移的清单应为每个固定目标填写明确的 SSH 别名（包括当前机器自身），
`build.cargo` 使用 `~/.cargo/bin/cargo` 或当前机器实际可用的路径。
使用当前机器上已安装测试依赖的 Python 运行部署命令；测试门及其子套件
沿用该解释器，不切换回系统 `python3`。执行前用同一个解释器检查
`python3 -c 'import playwright.sync_api'`，并按 [验证前提](validation.md#prerequisites)
准备 Chromium。不要把某台机器的虚拟环境路径写进共享清单。
`ssh: null` 只表示执行命令的本机，绝不表示“名称指定的机器”；复制工作区时
必须重新检查，通常应改成该目标的 SSH 别名。SSH 端口来自别名配置或
`ssh_port`，不能根据构建机或目标名称推断。

Linux 节点及 Hub 可在 `extra.expected_hostname` 填写目标 `hostname` 的完整输出。
部署 probe 会核对实际主机名，不一致则跳过该目标并使整轮失败，上传和重启均不会执行。
先按改动范围选择产物与目标；需要发布时用 `python3 deploy/fleet_status.py` 检查连通性、节点标识和现有版本，
再运行 `python3 deploy/deploy.py deploy` 并指定对应参数。Linux 产物在当前构建机编译一次；
构建机的架构和 glibc 基线须兼容 Linux 目标，macOS/Windows 仍在目标机原生构建。
不同构建机的独立 checkout 不共享部署锁，执行前须确认没有另一轮部署。

## 命令

发布范围遵循 [AGENTS.md](../AGENTS.md#scope-and-boundaries)：

- **产物范围与机器覆盖分别确定**：只更新受影响产物，但必须覆盖部署清单中该组件的所有
  已启用实例。共享前端或会话行为（列表、历史、搜索、SDK 交互等）覆盖全部 SessionDock
  节点及 Hub；不能按报错机器、开发机器、便利程度或最低成功数量抽选两三台。
  发布前列出完整目标集合，并说明未选目标属于无关组件、已禁用或用户明确限定范围。
- 纯前端修正只运行受影响的 Chromium 套件和看图验收，使用
  `deploy --web-only --targets <全部相关节点及Hub名称> --test-suites <相关套件>`；从本地清单选出所有提供受影响前端的目标，
  排除 `resource-agent`。不重建或替换后端、ptyhost，不重启资源代理。
  Web 服务的重启、健康检查和回滚仍由部署工具完成。
- 纯规则/文档只做文档检查、提交和推送，不构建、部署或重启生产服务。
  纯测试或部署脚本修改同样不发布未变化的运行产物，只验证相关范围。
- 后端、资源代理把受影响产物发布到范围内的所有实例；组件专属修改不重启无关组件。
  仅清单全部目标都受影响时用 `--all`，否则用 `--targets` 完整列出受影响目标。
- 共享文件可用 `--test-suites <名称,名称>` 明确本次行为所需的套件。已经独立完成
  同一产物的相关验证时，可用 `--test none` 避免重复，记录实际结果。
- 修改服务 unit、mount 等运行配置仍按相关组件验收和发布，不能当作纯脚本文档修改。

```sh
python3 deploy/deploy.py build    [--allow-dirty PATH ...] [--with-ptyhost] [--web-only]
                                  [--test none|affected|full] [--test-base REF] [--test-suites NAMES]
python3 deploy/deploy.py push     [--targets a,b | --all] --stage DIR [--web-only | --bin-only]
                                  [--with-ptyhost] [--dry-run] [--parallel N] [--keep-backups N]
                                  [--health-timeout S] [--targets-file PATH] [-v]
python3 deploy/deploy.py deploy   # build → test → push，标志相同；--test 在这里默认 affected
python3 deploy/deploy.py rollback --targets X [--backup DIR]
```

- `build`：源码是 HEAD，外加 `--allow-dirty` 点名的未提交文件或目录（点名的路径必须确有改动，
  否则拒绝）。共享 checkout 里其他未提交改动（别的会话尚未完成的工作）既不阻止构建也不进入
  stage，构建只列出它们。源码树检出到私有构建 worktree `target/deploy/tree`，在那里用独立的
  `--target-dir target/deploy/cargo-target` 编译，web 快照也取自同一棵树；固定路径让增量缓存在
  多次部署间有效。
  `deploy` 只构建选定 Linux 目标声明的二进制；原生构建目标从同一 source.tar 构建。
  ptyhost 仅在相关代码变化时用 `--with-ptyhost` 加入。单独 `build` 使用清单中的
  `build.packages`；二进制名只选择该二进制。`--web-only` 不跑 Cargo。默认 `--targets-file` 是
  `deploy/targets.local.json`（缺省时退回 `targets.example.json`），其中 `build` 段给出 cargo
  路径与包名；`sessiondock-hub` 是 `sessiondock` 包里的第二个 bin，工具用 `cargo metadata` 解析。
  前端 `web/` 按快照原样进入 stage，不执行任何 Node 构建。
  `--test`（单独 build 默认 `none`）在构建完成后按[测试门](#测试门build--test--push)跑测试。
- `push`：必须显式传入 `--stage DIR`，不自动取其他任务的最新 stage。默认并行 4、
  保留 5 份备份（`0` = 不清理）、健康
  超时 45 s。`--dry-run` 只做 probe 并打印每台的计划，什么都不上传。离线或禁用的目标记为
  `SKIPPED`（未更新），不使命令失败；真实部署失败仍退出 1。全离线也会逐项报告跳过，不声称已部署。
  运行产物改动仍须按完整范围立即部署、重启和健康检查；初步舰队检查显示不可达的目标
  也纳入一次正式发布尝试。已核对同一运行产物且健康的目标无需重复重启。
  每个目标都要报告为成功且健康、已是当前产物、离线、失败、已禁用或按范围排除；
  范围内未尝试的目标不能遗漏或算作完成。
  全部范围内目标都有结果后，离线或失败项可明确记为待补发，不阻塞工程或 goal，也不反复轮询。
  此时共享前端/服务发布仍须至少一台 SessionDock 节点和 Hub 成功；组件专属发布须有该组件
  的健康实例。这个最低条件只用于全部目标尝试后的完成判断，绝不能用于缩减发布目标。
  `push` 在构建机上**从不跑测试**（它只是把已构建的 stage 发出去），但会先打印这个 stage 是按哪种模式
  验证过的（`stage tests: mode=… result=… base=… suites=…`）。
- `deploy`：build → test → push。`--test` 默认 `affected`；验证失败则不上传。
  `deploy --dry-run` 只输出本地目标、产物和测试计划，不构建、不跑测试、不访问远端。
  `push --stage DIR --dry-run` 只探测目标并打印已有产物的发布计划。
  web-only 自动排除无前端的资源代理；缺少目标所需二进制在上传前报错。
- `rollback`：不给 `--backup` 时取目标机上**mtime 最新**的 `backup-deploy-*`（手工备份的时间戳是
  本地时间、本工具是 UTC，按名字排不可靠）。`--backup` 只接受匹配
  `backup-deploy-<hex>-<YYYYmmdd>-<HHMMSS>` 的目录。
- 目标若在本地清单里声明 `extra.frontend` 为 `legacy` 以外的值，发布阶段跳过它（`--bin-only` 除外）。
- `status` 不在本工具里：只读的舰队状态见 `deploy/fleet_status.py`。

## Kiro 目录升级前保留身份

从旧版原生文件目录切换到 ACP 自有目录前，先在仍运行旧版的每个节点执行
`python3 deploy/kiro_identities.py --targets <全部节点名称>`（可先 `--dry-run`）。
工具核对目标 hostname，读取服务自己的目录 API，只将 UID、SID、cwd 的明确关联
原子保存到私有 `state/kiro-legacy-identities.json`，不读取原生正文、不重启服务。
Hub 和资源代理没有本地 Kiro 目录，排除。捕获失败的节点先修复捕获再升级；
离线节点补发时同样先执行捕获。新目录只在初始配置作用域导入一次，不按路径、
标题或时间猜测身份；后续 scope 切换不能重复导入其他账号的数据。

## 并发部署锁

`build`、`deploy`、`push`、`rollback` 启动后先获取 Git common directory 下的
`sessiondock-deploy.lock`，整个命令结束才释放；`deploy` 的 build → test → push 共用一次
持锁。同一仓库的多个会话及所有 worktree 互斥，不同目标也排队。默认最多等待 30 秒，
`--lock-timeout SECONDS` 可调整，`0` 立即失败。等待时显示持锁者，不终止其他部署。
等待者拿锁后才读取工作区、目标清单和默认 stage，避免使用等待前的旧状态。

锁由操作系统管理，正常退出、异常和进程终止都会释放；文件保留，里面的旧 PID 只供
诊断。**不要删除锁文件来解锁**，删除会让不同进程锁住不同 inode，破坏互斥。
锁覆盖同一 Git 仓库及 worktree；独立 clone 或不同构建机之间仍须协调部署。
只读的 `fleet_status.py` 和 `deploy --dry-run` 不持锁。部署自检使用临时仓库或临时锁，避免与运行自检的外层部署互锁。

## 三种产物（`target/deploy/<UTC 时间戳>-<short>/`，同一秒内再建则加 `-2`、`-3` 后缀）

| 产物 | 内容 | 用途 |
| --- | --- | --- |
| `bin/<name>` + `artifacts.json` 里的 `version` | `cargo build --release --locked` 的 glibc 二进制，从 `target/release/` 拷进 stage | `linux-node`、`hub` 直接上传；stage 一旦生成就不再受后续构建影响 |
| `web/` | 源码树（HEAD 加点名文件）里 `web` 的快照，原样发布 | 所有 kind 的 `web/` |
| `source.tar` | 源码树（HEAD 加 `--allow-dirty` 点名文件）的 `git archive` | `build_on_target` 的 kind（macOS、Windows）在节点上原生构建，与本机构建使用相同源码 |

`artifacts.json` 还记录 commit、`dirty`、`built_at`、web 来源和 cargo 命令，以及测试门的结果
（`test_mode`、`test_base`、`test_full`、`test_suites`、`test_result`、`test_log`）；`logs/` 放 cargo 日志、
测试门的 `tests.log` / `tests.json` / `validation/<suite>.log` 和每台目标的 `<name>.log`；每次 push/rollback
写一份 `report-<stamp>.json`。

`--allow-dirty` 的源码树通过临时 Git index（从 HEAD 读入，再加入点名路径，含新增文件）生成，
不改变共享工作区的暂存区，只包含点名的文件。`dirty_files` 记录带入的文件，
`ignored_dirty_files` 记录被排除的其他未提交改动，`source_tree` 与 `source_source` 记录源码树及来源。

## 测试门（build → test → push）

`deploy/testplan.py` 实现三种模式：

| 模式 | 行为 |
| --- | --- |
| `affected` | 默认。按受影响功能、fixture 引用链和 Rust 包选检查；先打印计划 |
| `none` | 跳过部署测试门，记录 `skipped`；只在已完成相关独立验证时使用 |
| `full` | 显式完整验收；付费真实 CLI 仍不自动运行 |

`--test-suites a,b` 明确指定本次浏览器套件，适用于共享样式等跨功能文件；保留相关
文档/脚本检查和包级 Rust 检查。未知路径不自动升级全量，而是提示指定套件或补映射。
没有检查需要执行时记录 `not-needed`，不能当作功能验证通过。

测试脚本、fixture、Cargo 源码与发布产物使用同一 source tree，不读取他人的工作区
未提交代码。Cargo 验证仅对相关包运行 fmt/clippy；普通验收不重复 release 构建，
不自动加入 Windows 车道。浏览器显式使用 stage 的 web；前端发布可通过
`--test-binary PATH` 复用已有兼容测试二进制（默认 `target/release/sessiondock`）。
测试所需的其他二进制复用开发目录的构建结果，不使用生产目录或会话。

套件日志在 `<stage>/logs/`，总超时由 `--test-timeout` 控制。macOS/Windows 只有发布
二进制且测试模式不为 `none` 时才执行相关包的原生编译检查。检查失败先于换入。

### base commit（`affected` 的起点）

1. 显式 `--test-base REF`；多提交任务可指定本任务起点。
2. 所选目标有效部署标记中距发布提交最远的版本。
3. 无部署标记时使用发布提交的父提交；初始提交使用空树。

终点固定为本次发布快照；不会因为刚推送使 `origin/main` 等于 HEAD 而得到空差异。
目标版本较旧时，审阅计划并按实际兼容范围选择套件。

### 映射表

| 改动 | 默认检查 |
| --- | --- |
| SDK、额度等独立前端 | 对应功能的浏览器套件 |
| 共享 style/app/index | 可识别的单一 SDK/额度改动走对应套件；其余走页面、顶栏、菜单基础回归，可用 `--test-suites` 指定实际功能 |
| fixture/helper | 直接和传递导入该文件的已登记套件 |
| 后端模块/API | 对应功能套件，加相关 Rust 包的 fmt/clippy |
| ptyhost / collector | 各自的集成套件与相关包检查 |
| 文档 | 文档链接及规则入口检查 |
| 部署/CI 脚本 | 文档检查和 `python3 deploy/check.py`，实际逻辑改动另做隔离 CLI 验收 |
| unit/mount | 服务模板结构检查和资源集成浏览器路径 |

资源代理的服务文件从 stage 的 source.tar 读取，与二进制同源。并行目标按完成顺序
报告结果；只选本次相关目标，不为无关节点扩大发布或延长等待。

## 每台目标的步骤与不变量

顺序固定为 `probe → plan → stage → backup → swap → restart → verify → write_marker → prune_backups`
（`deploy/sdtargets/base.py` 是合同，每个 kind 一个模块实现这些步骤）。`backup` 成功之后任何一步失败：
`rollback(backup_dir) → restart → 再 probe`，结果记 `ROLLED_BACK`（仍算失败）；SSH 连接超时、拒绝、
断开或网络不可达记 `SKIPPED`，不计测试或部署失败；身份不匹配、认证失败、目录缺失、探测程序异常
仍记 `FAILED`。开始部署后的上传、构建、重启或健康检查失败仍按失败和回滚处理。回滚命令也会跳过离线节点。
kind 的处理模块缺失或坏掉记 `UNSUPPORTED`，不会让整轮崩溃。

Linux 节点在 stage 前和 backup 前只读检查 `state/native-sdk-inputs.json`。
存在旧进程内 `sending` / `accepted` 输入时，本节点发布失败并保留当前服务运行。
已确认由 [sdkhost](sdkhost.md) 托管的输入不阻止节点发布，任务在独立宿主继续运行。
该检查也适用于 web-only，因为它仍会重启节点服务；Hub 不持有这些原生进程，不受此检查影响。
检查发生在 backup 前，拒绝不会触发自动回滚和重启，不提供强制跳过开关。
这只是旧运行时的迁移保护；不能把旧进程内管道在线转移到宿主。
`--with-sdkhost` 显式构建并发布宿主二进制，普通发布不替换它，也不重启已有实例。
首次部署须带该参数；后续仅 SDK 业务适配层变更不需要该参数。
Linux/macOS 节点发布 `sessiondock` 二进制前，若本轮不包含 `sdkhost`，
工具检查目标 `bin/sdkhost` 可执行；缺失时在上传、备份和重启前报错，提示
使用 `--with-sdkhost`。该参数在 Linux 和 macOS 都包含宿主，Hub/Windows
不包含；web-only 不替换或补装宿主。
账本中的 unknown 仍需要核对，不能自动重发。

测试使用临时清单、模拟 SSH 或回环服务，不连接真实舰队节点；测试结果不依赖节点在线状态。

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
  日期版本标记等于 stage 的 `version`，发布 web 时 `/api/meta.build` 等于该版本号。
- dry run 只做 `probe()` 并打印计划，什么都不上传。

## `verify` 证明了什么

`build` 直接读取 `web/release-version.txt`，不计算内容哈希。舰队每次构建生成 UTC 日期版本
`YYYYMMDDTHHMMSSffffffZ`（含微秒），写入 stage 的 `artifacts.json` 和 web 版本文件。
二进制旁的 `<name>.version`（Windows 为 `<name>.exe.version`）记录同一版本。
服务重启后，`verify` 核对所发布二进制的版本标记及 `/api/meta.build`，并检查单元 active、
probe 时的 ptyhost PID 仍在、host 记录数不少于之前。没有版本文件的开发目录使用最新资产
修改时间作为 `dev-<纳秒时间戳>`，不再扫描内容计算摘要。

`--bin-only` 保留 web 版本，`--web-only` 保留二进制版本。首次从旧部署工具升级须做完整发布，
让服务支持日期版本；之后可继续单独更新前端。备份包含版本文件和 `deployed-commit`，回滚一并
恢复；旧备份没有版本文件时移除本轮标记，健康检查照常执行。版本号用于识别发布批次，
不做文件防篡改校验。旧 stage 应重新构建再发布。

[`tests/release_version_browser.py`](../tests/release_version_browser.py) 在私有目录运行真实 Linux
暂存、替换和回滚，通过 Chromium 打开会话并加载版本化资源，覆盖相同 mtime 的资源更新、
浏览器缓存、版本标记恢复、重复发布及开发目录修改时间。

## Linux 节点与 Hub 的实现（`linux.py`、`hub.py`）

- probe：一次 ssh 往返跑一段 POSIX 脚本（远程统一用 `sh -c` 包住，登录 shell 是 zsh 也不会因为通配
  失配而中断），输出 `key=value`：`is-active`、`/api/meta`、每个二进制的 `.version`、`pgrep -x ptyhost`、
  `ls host | wc -l`、`etc/deployed-commit`。
- stage：二进制 → `bin/<name>.new`（附带日期版本标记）；web → `<prefix>/web.staging/`。Linux 节点与 Hub
  不在节点上跑测试：它们拿到的二进制已经在构建机上按 stage 的 `test_mode` 验证过。
- swap：`mv -f bin/<name>.new bin/<name>`；在目标机上 `rsync -a --delete web.staging/ web/` 后删掉 staging。
- restart：`systemctl --user restart <unit>`；节点单元是 `KillMode=process`，分离的 ptyhost 不受影响。
- 备份始终同时拷 `bin/` 里声明的二进制和整个 `web/`；rollback 用同一条 `.new` + `mv` 路径恢复二进制、
  `rsync -a --delete` 恢复 web，再重启。ptyhost 仅在指定 `--with-ptyhost` 时随行，同样走 `.new` + rename，
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

## Optional Linux resource collector

A `resource-agent` target installs the independent system service documented in
[process-links](process-links.md#independent-linux-service). Add `resource-agent`
to `build.packages`, and add one opt-in target per monitored Linux host with
`kind: resource-agent`, `prefix: /opt/resource-agent`, `binaries: [resource-agent]`,
the same explicit SSH routing/hostname checks as its node, and
`extra.node_id_file` pointing to that node's existing identity file. The deployment
SSH user is the monitored UID; sudo is required for installing the system unit.
The target verifies the local socket, release version and active BPF event capture;
a failed installation restores the previous collector, or disables/removes a
new installation. It never restarts SSH or workload/session hosts. Non-Linux
nodes without a collector retain the compatibility API.
