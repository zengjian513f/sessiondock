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
- [ ] 为外部会话实现原生 rename；现有历史中的 `/rename` 展示不等于进程控制操作。

## Native interaction semantics

- [ ] 部署补发（不阻塞 [Agy](docs/agy.md) 接入）：Linux、macOS、Windows
  各一个节点网络不可达，恢复联机后用官方部署工具补发并验证。

- [ ] 决定并实现真正的 native rewind/rollback。现有 timeline pin 只改变 SessionDock
  的展示视图，不改 CLI 原生历史，也不向 CLI 发送回滚动作。
- [ ] 若仍需要 activity stop 覆盖，先定义原生确认和退役语义，再接入读模型；不得仅凭
  HTTP 或终端写入成功声明完成。

## Operations and release engineering

- [ ] 评估是否仍需独立合成语料统计工具，以及由源码生成的预算参考表；仅维护
  仍有效的性能/协议数字，不恢复已删除的输入拒绝规则。

- [ ] 增加服务端请求/响应 tracing 与结构化日志，同时保持凭据、正文和原生记录默认不落盘。
- [ ] 产出可复现发布包和 Linux/Windows/macOS CI 矩阵；Windows OpenSSH 原生构建遵循
  `docs/deploy-windows.md`。
- [ ] 将真实 Claude/Codex/Grok CLI 套件纳入明确的发布验收步骤；继续使用临时配置和
  `AGENTS.md` 规定的低成本测试模型，不进入普通 `cargo test`。
