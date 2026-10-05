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

- [ ] 部署补发（不阻塞 [Agy](docs/agy.md) 接入和 Vue 前端切换）：Linux、macOS、Windows
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

## Frontend direction

- [ ] 为全部展开的大侧栏增加可见区渲染，并保持分组多选、深链定位和文本选择语义。
  当前已复用未变化行、按需创建折叠组；全部展开时 DOM 数量仍随会话数增长。
- [ ] 将 Grid 历史折行改为逻辑行惰性计算或可取消分批计算，同时保持选择坐标和完整历史。
  当前拖动已合并、临时 cell 不常驻，但最后一次变窄仍同步遍历历史。

### Vue 前端改为 Vue 写法（仅 `web/`，生产 `legacy-web/` 不变）

验收：services/pages 直接调用 DOM API 只留在网格画布渲染器与 diagnostics 快照；
偏好读写全部经由偏好 store；能由状态推导的界面数据用 `computed`；`any` < 100。
不变：DOM id/class、存储键、请求格式、文案、`window.SessionDockRuntime` 上被测试
使用的路径；发送确认、同步与终端字节缓冲留在 service。每阶段完成即验证并部署预览站。

- [ ] Hub 注册表的 `renderer` 字段与 `set_renderer` 只剩 legacy 使用；legacy 退役时一并删除。
- [ ] 对话区：消息容器 `#msgs` 仍由服务创建并作为各视图的 Teleport 目标，历史分页、
  差量追加和待落盘提示按 `#msgs` 查询；改为组件渲染容器后再收掉这些查询。
- [ ] 终端：每个视图的 socket/心跳/重连/同步帧收进 `TerminalSession`；布局策略、
  CLI 菜单改为组件；新建会话启动流程移出终端；列表与 pane 绑定独立。
- [ ] 辅助页：grid 拆为画布渲染器类加组件；records 列表与播放器改为组件。
- [ ] 收尾：依赖接口改为实现类型推导，更新 `architecture.md` 与
  `frontend-migration-surfaces.md`。
