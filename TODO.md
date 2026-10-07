# SessionDock TODO

这里只记录当前尚未完成或尚未决定的工作。当前行为以代码、测试和 `docs/` 下的
合同为准。

## Runtime and platform coverage

- [ ] 扩展公共进程归属层的平台覆盖与完整计量：Linux `resource-agent` 已提供
  生命周期事件、CPU/PSS/GPU/proc storage 指标及可选 60 秒 VFS/TCP/NFS 诊断。
  macOS/Windows 采集器、短命任务无遗漏的累计账本仍待实现；连接复用、跳板、NAT
  的完整关联需要额外证据，不能将部分观测视为完整计费。
  现行接口与后续计量口径见 [process-links.md](docs/process-links.md)。

- [ ] 为 Windows/macOS 的外部（非 ptyhost 管理）CLI 补齐进程发现与强身份验证。
  Windows 的受管 ptyhost 路径已经过实机验证，不应与此外部进程缺口混为一谈。
- [ ] 在 macOS 实机验证 sessiondock、ptyhost、文件原子替换、进程身份和终端生命周期；
  Linux 测试或 MSVC 交叉编译不能代替该验证。

## Native interaction semantics

- [ ] 部署补发（不阻塞 [Agy](docs/agy.md) 接入）：Linux、macOS、Windows
  各一个节点网络不可达，恢复联机后用官方部署工具补发并验证。

## Operations and release engineering


## Read-path optimization follow-up

- [ ] 为大 JSONL 的列表/搜索提供按需字段投影，减少短摘要读取时构建完整 AST 的
  开销。先保持 [read-model](docs/read-model.md) 的坏行、分页与源字节语义，
  再用 Chromium 大会话场景验证；不以新增输入限制代替优化。
- [ ] 缩小视图缓存全局锁的持有范围，避免不同会话的解析互相阻塞；同时维持
  缓存发布、重置和活跃 checkpoint 的一致性。
- [ ] 评估追加读取后 provider 投影的增量化，区分可追加状态与必须重建的重写/
  截断路径，保持历史、列表与搜索结果一致。

## Validation stability

- [ ] 跟进完整浏览器验收的偶发停止/丢响应恢复失败，利用新增失败响应和
  删除进度诊断定位原因；保留成功数量、恢复结果与原生字节断言。
  评估发布门收集全部失败后统一修复，减少长菜单套件被重复执行的成本。
