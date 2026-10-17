# SessionDock TODO

这里只记录当前尚未完成或尚未决定的工作。当前行为以代码、测试和 `docs/` 下的
合同为准。

## Runtime and platform coverage

- [ ] 根据 [外部程序启动审计](docs/process-launch-audit.md) 补足共享 tmux/systemd、
  容器/调度器/常驻 RPC 服务的每作业身份或因果证据；无证据时保持未归属。
  不能把启动请求、业务租约释放或自然语言“完成”当作 OS 资源归属/释放证明。
  本次未访问的节点及历史坏行仍有审计覆盖缺口。

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

- [ ] 修复 Claude SEND 的制表符回显对账：BUG-20261017-072946-4cbf80 的首条
  输入在 07:22:10 UTC 已写入原生记录，但 CLI 把每个 tab 转成四个空格，严格正文
  摘要未匹配，旧回执一直显示等待处理。回滚分支解析修复不结清此回执；后续需用
  假 CLI 覆盖明确的字符转换及回滚后旧回执恢复，不能泛化为忽略任意空白或自动重发。

- [ ] 定位 BUG-20261013-232935-441af3 的具体 PTY 上行中断环节：四条连接首屏正常，
  42 次浏览器输入没有节点写入回执，首个心跳均超时；第五条连接随后自行恢复。
  [诊断记录](docs/diagnostics.md#bug-20261013-232935-441af3-repeated-pty-upstream-stalls)
  已核对原生记录、账本及节点/Hub 日志。同一连接的浏览器发送、Hub 双向转发、
  节点接收/等待/宿主写入及取消最终状态已补充审计，Hub 记录自动复制进节点诊断包。
  再次发生时按这些边界定位；旧包仍不能指定故障代理/网络跳，隔离浏览器回归通过
  不代表历史生产问题已修复。

- [ ] 部署补发（不阻塞 [Agy](docs/agy.md) 接入）：Linux、macOS、Windows
  各一个节点网络不可达，恢复联机后用官方部署工具补发并验证。

- [ ] 补齐 [Codex SDK 模式](docs/codex-sdk.md) 的其余 app-server 能力：从 SDK 视图
  新建会话（`thread/start` 后在列表出现）、图片/文件附件（`localImage`）、
  `review/start`、`thread/compact/start`、`thread/fork`/`thread/revert`、目标
  （`thread/goal/*`）与 `thread/shellCommand`；评估让 SessionDock 启动的 Codex 终端
  使用 `--remote unix://` 共用 daemon，使终端与 SDK 视图实时共享同一运行实例。

## Operations and release engineering

- [ ] 在现有 16 个 CI 浏览器回归套件之上，评估按改动范围选套件及周期性完整合成
  数据验收；复用 `run_validation.py` 的 JSON 结果与失败重跑能力。

## Architecture follow-up

- [ ] 搜索控制器已独立管理请求、取消和计时器；继续按职责拆分侧栏、会话同步、
  草稿发送与终端连接模块，用现有浏览器路径保持 DOM、草稿和连接生命周期语义。
- [ ] 测量 Hub→节点的连接/握手开销后评估短请求连接复用；保留字面 IP、认证、
  证书验证、空闲超时及禁止重定向的合同，SSE/WS 独立管理。
- [ ] 扩展现有浏览器性能记录到大文件首屏、追加与冷搜索，并补充排队、解析、
  缓存命中和流完成的分阶段观测；不恢复已移除的非浏览器测试体系。

## Read-path optimization follow-up

- [ ] 评估搜索正文与独立模型/回合扫描的按需字段投影。列表头/尾的大记录投影
  已接入；后续仍须保持 [read-model](docs/read-model.md) 的坏行、分页与源字节语义，
  不以新增输入限制代替优化。
- [ ] 评估 Grok 等来源的追加投影，处理跨记录工具状态并证明等价。Agy 无媒体且
  summary 未变的已验证前缀已支持续算，边界见 [append-cache](docs/append-cache.md)；
  Claude/Codex 可能回改旧消息，继续完整投影，不直接拼接旧事件。

## Validation stability

- [ ] 排查 `hub_pending_state_browser` 启动页等待 `networkidle` 的超时：
  BUG-20261008-120649-69fc59 修复验收时，工作区页面及未修改的 HEAD 页面均在
  `node_source_picker` 首次导航的 30 秒等待中失败，尚未执行测试操作。
  当前缺陷的桌面/手机回归、待落盘链接及页面导航套件已通过。

- [ ] 继续观察 `turn_state_browser` 持续追加时的瞬态读失败。已稳定复现并修复
  mtime 更新导致标量扫描丢弃完整记录的问题；上轮没有保留底层错误，不能断定
  所有追加偶发失败均由此引起。现有严格断言保留，失败时收集结构化读错误。
- [ ] 跟进完整浏览器验收的偶发停止/丢响应恢复失败，利用新增失败响应和
  删除进度诊断定位原因；保留成功数量、恢复结果与原生字节断言。
  评估发布门收集全部失败后统一修复，减少长菜单套件被重复执行的成本。
