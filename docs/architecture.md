# 架构边界

## 当前可运行链路

第一阶段默认链路：`legacy-web/` → Axum API → 有界 blocking 工作池 →
`sessions` 原生记录解析 / 版本缓存；详情增量通过 SSE 返回。
静态资源在启动时读取为内存快照，HTML 注入模式、build 与能力；
请求不访问静态目录中的动态路径。无需 Node.js 服务或前端构建。

`ptyhost-client` 是独立的异步本地协议库；仅配置私有 host 目录时，HTTP才接入
claim/WS传输。受控host匹配full SID/UID，缺少/冲突证据保留unknown；
关联快照本身不授权控制，claim重新观察唯一关联并固定instance，随后只使用
guarded attach。legacy已接手动控制台；受控创建需要额外显式配置，可靠发送仍关闭。
Vue `web/` 仅保留第二阶段骨架。

显式 delivery 目录通过 `prepare_app` 异步打开已有私有账本，恢复 epoch 后才
发布路由；不隐式初始化。单 coordinator 串行拥有状态机和持久存储，blocking
读取/有界序列化与 HTTP Body 共同持有准入，关停等待 OS 锁释放。只读 outbox
使用已验证快照的 NativeScope，且与可靠发送能力分离。host 输出则由每客户端
独立有界队列隔离慢读者，退出完整性与进程身份分别验证。

独立的 `LaunchTarget`/host launch guard 与同步 `lifecycle` 回执库：
前者在没有SID/UID时验证真实启动实例，后者在持久化Prepared/Starting后才返回
一次性授权，并将崩溃后的Starting恢复为Uncertain。已接显式版本化
launcher、排队 coordinator 和 pending HTTP/WS；独立 child reaper 保留
进程句柄，Web退出不杀host。取消先保存意图、退休输入租约，再一次guarded kill，
未确认退出仍是Uncertain；4002退休通知不冒充进程退出。
native BoundTarget不接受launch身份替代，详见
[生命周期接线合同](lifecycle-integration.md)。

host内有独立的write-once native binding，不修改immutable meta/record。
服务schema3保存操作者确认的关联意图，guarded Info才能确认完整原生身份；
恢复与离线先降Uncertain。NativeScope证明记录身份，不证明进程归属，因此禁止
自动猜测；新绑定使用同snapshot的真实ID目录，旧metadata目录与其分离。
pending租约不升级，衍生native租约仍受launch退休及持久取消约束。

读工作池 `SESSIONDOCK_READ_WORKERS`（默认 `clamp(核数/2, 8, 32)`）个 blocking
worker；请求排队直到取得许可或自身取消。探测、
历史页/媒体/文件写/生命周期响应池按比例派生（表见
[performance.md](performance.md#并发预算)）；搜索不占读池。
同一文件版本复用解析结果。`/api/meta` 声明 `stage:"replacement"`、
`read_only:false`——这是 Python 服务的替代品，前端没有常驻横幅。
`observe::WatchHub` 为每个 `(uid,agent)` 共享一次500ms版本读取，最多2个后台
工作准入；Tokio watch只保留最新不可变快照，每个浏览器按自己的checkpoint
生成增量，慢读者不堆积旧版本。首次并发订阅合并、最后订阅回收、错误重试
使用代际标识防止旧任务清理新任务。共享槽的语义见
[Tokio watch](https://docs.rs/tokio/latest/tokio/sync/watch/index.html)。
完整checkpoint与原始前缀digest按视图缓存；追加从上次已提交偏移续读，较早
checkpoint仍核对投影。

## 历史读模型

设计以 [read-model.md](read-model.md) 为准（惰性索引 + 按需视图）：

- `sessions/index`：列表只做目录遍历、`stat` 与每文件有界头/尾摘要（头 96 KiB
  ≤ 40 条、尾 512 KiB，与 Python `list_sessions` 同一推导），按 stamp 缓存，
  并行读取；没有启动解析，没有会话数/总字节上限，单个文件的变化只影响它自己。
- `sessions/views`：只在打开会话时经 `records`/`native_input`/`providers` 流式
  解析这一个文件，增量续读，进有界 LRU；游标、锚点、pin、原生输入证据等每视图
  契约不变（[history-pages.md](history-pages.md)、[native-input.md](native-input.md)）。
- `providers/` 只投影传入的原生记录，不打开文件；`index/graph.rs` 的归属图从
  摘要推导父/子关系，请求的 `agent` 和记录中的父 ID 从不拼接成任意磁盘路径。
- `sessions/mod.rs` 的 `SessionStore` 只是门面：列表 = 索引行 + 元数据装饰 +
  重签名（打开会话不改变 `sig`）；打开 = 从索引取候选文件，经 `views` 打开；
  运行时目录、回收站文件集、续接身份都取自索引，不解析文件。
- 原生数据永不被修改。

## 搜索与工具展示

- 搜索对完整语义正文匹配（按 `updated` 倒序），不对原始 JSONL 匹配；正文来自
  按文件版本持久化的搜索文本缓存（[read-model.md](read-model.md#搜索)：
  显式 `SESSIONDOCK_SEARCH_CACHE_DIR`，LRU 上限），只有版本变了的会话才重新投影，
  投影不留驻；不可读视图产生显式部分失败（并按版本缓存），不能把跳过的会话算作
  完整搜索成功。
- 搜索请求等待自己的准入许可和解析 worker
  （`SESSIONDOCK_SEARCH_WORKERS`，不占普通读池）；
  取消响应会取消工作并释放阻塞发送，blocking permit保持到实际工作结束。JSON大对象
  序列化也在worker。
- Rust regex只接受有界非回溯方言，lookaround/backreference明确400；全词边界
  单独适配，Unicode大小写和字符类差异在legacy选项说明和模块契约中公开。
- 工具changes是原生参数的纯投影，不读取被编辑文件。完整Write/片段Edit和
  patch保持原有before/after可信程度；生成差异超预算时保留原始参数并解释原因。

## 后续实现原则

文件读取由当前会话引用或已签发路径引用确定目标；兼容配置中的 file roots
不充当授权边界。路径解析保留目录能力句柄，读响应保留已检查文件；按 HTTP 消费者
需求每次只在blocking任务读取64KiB。未轮询Body不开始读，取消后尚未结束的
读取继续持有permit，避免慢下载挤占普通历史worker。文件作业与缩略图另行实现。

可选的 `MetadataStore`：每次操作重读当前文件，并以原子替换持久化。
列表、详情、搜索和 SSE 从同一元数据版本装饰读模型；偏好变化不修改消息 anchor。
legacy增量补接元数据变化，只更新缓存/标题栏，不重绘已有消息正文；子代理名称
更新已经过无列表刷新、无reload的实际SSE/DOM回归。

1. HTTP / SSE / WebSocket 是传输层，不承担 CLI 历史解释或发送确认逻辑。
2. 接口 DTO 和业务状态分开；缺失字段与显式 null 必须有明确含义。
3. 消息同步、队列确认、重复/乱序处理置于独立状态机，并用事件序列测试。
4. 第二阶段 Vue 负责视图，Pinia 按会话、机器、偏好等拆分；第一阶段只对
   legacy 做必要兼容小改，不复制旧全局大对象到 Vue。
5. xterm 和 WebSocket 的生命周期由终端管理模块负责。终端字节不经过
   全局深层响应式状态；切换视图不等于销毁连接。
6. ptyhost 继续每会话一个独立进程。Web 服务重启不得终止 CLI 会话。
7. Rust 浏览器接口可与新前端一起演化；旧 Python Hub 兼容性需要单独的
   适配层和契约测试，当前健康检查的版本字段不代表旧节点协议兼容。

当前实现合同以本文件及相应模块文档为准；未完成工作只记录在根目录
[TODO.md](../TODO.md)，路由清单见 [route-ledger.md](route-ledger.md)。
