# SessionDock TODO

这里只记录当前尚未完成或尚未决定的工作。当前行为以代码、测试和 `docs/` 下的
合同为准。

## Runtime and platform coverage

- [ ] 实现 SSH 跨机器自动父会话发现：普通直连可匹配两端连接四元组和本地
  SSH 进程祖先；连接复用、跳板、NAT 需要逐次启动的父会话标记或额外证据。
  保留原生创建时间校验，不能仅凭目标机器/目录/时间邻近猜测。现状与边界见
  [liveness.md](docs/liveness.md#ssh-boundary)；跨机器手工挂载已支持。

- [ ] 实现会话跨机器迁移：带上依赖闭包（祖先、fork、子代理、续接链），只迁移已停止的
  会话，两端的 CLI 根和 cwd 绝对路径必须一致。已完成 Codex 0.159.0 同版本隔离实验和
  不可变 rollout ID 读取支持；文件复制不足以保全分页历史和元数据。待完成跨版本/其他 CLI
  验证、生产导入与回滚、迁移编排和页面。设计及实测见 [session-move.md](docs/session-move.md)。

- [ ] 为 Windows/macOS 的外部（非 ptyhost 管理）CLI 补齐进程发现与强身份验证。
  Windows 的受管 ptyhost 路径已经过实机验证，不应与此外部进程缺口混为一谈。
- [ ] 在 macOS 实机验证 sessiondock、ptyhost、文件原子替换、进程身份和终端生命周期；
  Linux 测试或 MSVC 交叉编译不能代替该验证。
- [ ] 为外部会话实现原生 rename；现有历史中的 `/rename` 展示不等于进程控制操作。

## Native interaction semantics

- [ ] 决定并实现真正的 native rewind/rollback。现有 timeline pin 只改变 SessionDock
  的展示视图，不改 CLI 原生历史，也不向 CLI 发送回滚动作。
- [ ] 若仍需要 activity stop 覆盖，先定义原生确认和退役语义，再接入读模型；不得仅凭
  HTTP 或终端写入成功声明完成。

## Operations and release engineering

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

- [ ] 明确 `legacy-web/` 是否继续作为正式前端，或分阶段迁移到 `web/` 的 Vue 3 /
  TypeScript 架构。在作出产品决定前，不以“迁移收尾”为名扩展两套实现。
