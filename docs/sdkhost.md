# 独立 SDK 进程宿主

`crates/sdkhost` 是独立可执行文件，`crates/sdkhost-client` 是通道客户端。
宿主不依赖 sessiondock crate，不识别 SDK 来源、JSON-RPC、NDJSON、模型、
会话、工具或审批。它只持有子进程及三条标准流，传输不透明字节。

## 生命周期与边界

```text
browser → SessionDock 协议适配层 → 私有本地 IPC → sdkhost → stdin/stdout/stderr → CLI
```

Agy、Claude、Grok 的协议解析仍在 SessionDock；Codex 与 OpenCode 沿用各自
已经独立的原生服务，不增加另一层协议代理。Linux、macOS、Windows 上配置 state dir 后，stdio
SDK 使用独立宿主。未配置持久目录的开发模式保留进程内行为，不宣称支持重启恢复。
Windows 使用本地 named pipe，Unix 使用 Unix socket；不打开网络监听。

网页连接断开、SSE 断开、节点服务 shutdown 只释放客户端连接，不关闭原生 stdin，
也不发送停止信号。显式停止、原生中断及适配层既有空闲回收策略仍可结束自己持有的
任务；宿主只执行传入的生命周期操作，不推断任务是否完成。
Unix 宿主启动原生进程到独立会话，只有持有 Child 的任务能够发信号及回收，避免
进程退出后对复用 PID 发信号。DELTA（BUG-20261101-065421-ddbc1f）：Linux 上
原生进程另起会话，信号同时发给该进程组及会话内全部进程；Kiro 把 node 工作
进程移到别的进程组，原先只杀进程组会留下占满 CPU 的孤儿，并被识别为外部会话。Linux、macOS 在主进程退出后以 waitid WNOWAIT 保留未回收的 leader，直到双流 EOF；
此期间仍可停止会话内换过进程组的后代。主进程退出且 stdout/stderr EOF 后宿主结束，
原始输出、退出码/信号和输入回执仍可读取。

DELTA：节点后台将退出、双流 EOF 且闲置超过 24 小时的较大原始流压缩为 gzip。
压缩不删除退出状态和输入回执，也不改变未压缩字节 offset；客户端首次读取时无损
恢复普通文件，后续仍按 offset 分块。压缩与恢复共用文件锁，已打开的读句柄不受归档
影响。该冷任务不参与节点启动或普通会话列表。回收站及用户保存的故障证据不自动过期。

## 通道合同

私有目录为 `<state>/sdkhost/<opaque key>/`。客户端先检查目录所有者和权限，
Unix socket 两端检查同 UID。Linux 使用目录 FD 路径连接 socket，支持长 state
目录和无损身份编码，不依赖哈希路径。macOS 在同步 socket 操作期间使用线程本地目录，随后立即恢复，
不改变进程工作目录，也不受完整 state 路径长度影响。启动参数和环境通过启动管道传入宿主，
不写配置副本或打印到日志。宿主启动锁和实例标记阻止同一 key 启动第二个进程；
只读 attach 不会创建实例。宿主协议与具体 SDK 协议是两个独立边界。

Windows 私有目录创建时设置当前用户专属、可继承 DACL；附着时检查所有者、ACL 和
reparse point，不静默修改已有目录权限。首次创建可递归补齐缺失父目录，每层新目录
均采用私有 ACL；已有外层系统或用户目录不要求私有。Win32 文件操作使用扩展路径
命名空间，支持超过 MAX_PATH 的 state 路径，不增加应用层长度限制。管道采用随机实例名、当前用户 DACL、
拒绝远程客户端及首实例保护；客户端核对管道服务进程与私有 host.pid。启动锁、
永久实例标记及回执格式与 Unix 相同。启动参数的 Vec<u8> 在 Windows 使用无损
UTF-16LE，在 Unix 保持原始字节；调用方应使用 LaunchSpec::from_command，不能
把 UTF-8 字节自行当作 Windows 原生参数。Windows 环境覆盖与移除采用原生
大小写不敏感的键比较，Unix 仍区分大小写。宿主与客户端必须使用相同平台编码。

DELTA（Windows）：宿主启动时脱离调用方 Job，只继承显式列出的启动管道及日志
句柄，避免保持节点或 SSH 的句柄。原生进程先挂起创建、加入独立 Job Object，再恢复执行，后代
自动加入同一 Job。主进程先退出后仍保留 Job 控制；signal(9) 和 signal(15) 均停止
整棵进程树，其它 Unix 信号明确返回 Unsupported。Job 在宿主异常退出时清理后代；
节点服务或客户端退出不关闭宿主 Job。Windows 退出状态记录原生退出码，signal 为 null。

- stdout/stderr 分开追加到私有文件；read 带字节 offset，返回 bytes、next_offset、eof。
  读取分块不限制原生消息长度，完整行的组装由对应协议适配层负责。
- `stdout.time`／`stderr.time` 随输出保存宿主读到字节时的 UTC 毫秒时间；每条索引为
  两个 little-endian u64（字节区间的结束 offset、接收时间），不解析或修改原始流。
  适配层用完整消息最后一个字节定位接收时间，节点停服期间的输出恢复后仍用原时间。
  索引随原流保留，压缩原流不删除索引；旧宿主没有索引时，旧缓冲输出不补造时间，
  新输出可采用适配层接收时间。原生时间字段优先，接收时间不等同于模型生成时间。
- write 带稳定传输 ID；先持久化 unknown，再写管道，成功后记录 written。
  同一个 ID 再次请求只返回原回执，不重复写入。written 不代表 CLI 已消费。
- close_stdin 是显式操作，不随客户端 Drop 自动发生。
- status/receipt/read 可在宿主正常退出后离线读取。宿主异常消失不启动替代进程，
  也不重放不确定写入。
- 宿主不解释输出、不自动回答审批、不生成会话 ID、不决定模型或队列顺序。

## 适配层恢复

私有 `<state>/sdk-adapters/` 由 SessionDock 拥有，记录原生运行时绑定及协议状态。
成功打开的可信目标保存在 `native-sdk-bindings.json`，服务启动时仅复接已有宿主，
即使主回复已完成也能恢复后台运行状态；显式停止移除绑定，不会自动重新启动。
Agy 连续回合复用宿主及原生进程，绑定记录当前输入的输出起点；恢复从原始输出
重建该轮投影，并使用该输入独立的传输 ID 核对写入回执。Claude/Grok 保留协议状态、输出游标、待回答请求和稳定编号的发送记录；
重连附着原进程，不重新初始化、加载或发送已投递的 prompt。

输入账本中的在途工作重启后先进入内部恢复状态，网页仍显示 accepted。适配层
确认原宿主及请求关联后继续观察；没有原绑定、宿主故障或无法确认写入则记 unknown，
等待用户核对，不启动新 CLI 来假装恢复。这个机制保留网页服务重启期间的任务，
不是 CLI 崩溃后的无损续跑，也不声称跨机器故障的 exactly-once。

适配层发送队列必须先持久化，再交给宿主写入。落盘失败时撤回本次内存追加，
返回失败并保留待回答审批；恢复后以原生请求身份重试，不将失败请求当作已发送。
[发送队列故障回归](../tests/sdk_outbox_browser.py)通过 Chromium 审批按钮验证
Claude、CodeBuddy、Droid 的写入失败、无原生副作用、重试一次及重载回执。

## 发布与验证

正常节点或前端发布不包含 sdkhost，也不重启已有宿主。首次安装或修改宿主代码时
显式使用 `deploy --with-sdkhost --targets ...`；二进制按现有部署工具原子替换并备份，
已有实例继续旧程序，新实例使用新程序。不要通过重启宿主来更新 SDK 功能。
首次迁移无法将旧进程内管道转移给新宿主；部署检查仍保护旧的在途输入。

[原生队列浏览器](../tests/native_sdk_queue_browser.py)使用私有目录和合成 CLI，
验证节点停止后同 PID 存活、停服期间输出重放、输入只写一次、审批恢复及显式停止。
测试中途重启节点保留宿主，最终只清理测试自身私有目录中的实例。

[宿主平台浏览器](../tests/sdkhost_platform_browser.py)使用专属 loopback 页面和
`platform_fixture` 合成进程，每次操作均启动新的客户端进程，覆盖三流、稳定 ID
去重、节点客户端重启后的附着、offset/时间索引、离线回执、close_stdin、长目录、
主进程先退出后的后代停止与正常停止。可通过 SSH 运行目标平台临时二进制，Chromium
留在测试机器；不连接生产服务，也不运行真实 CLI。构建入口：
`cargo build -p sdkhost -p sdkhost-client --bin sdkhost --example platform_fixture`。
