# Kiro 原生 SDK 桥接

公共 SDK HTTP/SSE、草稿和输入队列 → Kiro ACP 适配器 → sdkhost →
`kiro-cli acp --agent-engine=v3 --auth-method=cli`。不静默回退到 V2 或直接子进程；
无持久 state 的开发通道不宣称能跨服务重启恢复。

## 协议与恢复

`initialize` 的标准能力及 `_meta.kiro.extensionMethods` 门控高级动作。
配置菜单使用原生目录和确认的值；未广告的设置、改名、steer、额度接口不伪装支持。
模型菜单使用官方 CLI 目录或已观测缓存，不通过 `session/new` 探测模型。
新建菜单继续强制刷新账号目录；CLI 查询在阻塞线程池执行，不占用 SDK 事件循环。
已有会话优先读取原生配置，目录失败保留之前的缓存。

私有 `<state>/sdk-adapters/kiro` 保存 RPC/写入 ID、完整待发送 frame、宿主写入证据、
原生响应、回合归属、stdout/stderr 游标及 live 投影。每次宿主读取中的完整 ACP 帧
按顺序合并处理，先持久化状态与最后完整帧的游标，再通知网页和 RPC 等待者；未完整
的尾帧保留到下一次读取。检查点缓冲序列化后 flush/fsync，不逐字段写盘。
重附着追赶原宿主，不重新 initialize/load，不重发 written 或结果未知
的 prompt。恢复后的公共队列观察原结果；宿主缺失明确待核对，不启动替代实例。

- 创建／分支先保存稳定操作意图与宿主键，再发 mutation。HTTP 回应丢失重取原 RPC
  结果，不重新造 SID。显式 open 的初始化中断窗口也复用原 setup 请求。
- prompt 的最终响应才结束当前回合；无关联 `turn_end` 通知不释放 FIFO。
- cancel 表示请求取消，保持 busy 至对应 prompt 最终结果或确认退出。取消写失败
  保留错误与归属，旧回合迟到响应不覆盖新回合。
- 权限／问题请求持久化后显示；回答先保存原 response frame，确认宿主写入后退卡。
  双页面重复回答使用原写入 ID。权限只呈现并接受原生提供的选项，空选项不合成允许。
- stdout 最后一个无 LF 响应和 stderr 尾部读完后再确认退出。最后回合结果与实例
  退出原因分开保存；进程退出不覆盖已经收到的成功或失败结果。
- 用户停止确认原宿主退出后才解绑。服务停机只断开托管通道，原生进程由 sdkhost
  保留；CLI／SDK 模式互斥与原创建宿主移交见 [Kiro sessions](kiro.md)。

普通查看只读缓存。官方 history 的默认响应未证实是完整历史，不能通过普通查看
调用激活式 load；用户显式启动 SDK 后同步回放。缓存投影与 live 检查点分开，历史
分页不移动 live 游标。

## 验收入口与证据边界

- [SDK 浏览器](../tests/kiro_sdk_browser.py)：官方目录及缓存、显式启动、流式消息、
  审批／问答、配置、分支、取消、原生删除与确认文案。
- [CLI 浏览器](../tests/kiro_cli_browser.py)：原生终端恢复／新建、双模式互斥、停止切换，
  以及旧创建请求重试不影响后来 SDK 实例。
- [Hub 浏览器](../tests/kiro_hub_browser.py)：经实际隔离节点与 Hub 验证顶部 SDK／CLI
  入口、模式切换和节点能力门控；不能以节点直连或其他来源的 Hub 验收替代。
- [恢复浏览器](../tests/kiro_recovery_browser.py)：服务重启附着、审批和输入去重、
  延迟取消与 FIFO、旧 RPC 迟到、空审批选项、退出尾部与终态。

以上套件使用合成 CLI，不能证明真实模型服务行为。
[真实浏览器验收](../tests/kiro_sdk_real_browser.py)显式接收隔离登录环境、CLI 路径和
用户授权的 `--model auto`，默认验证扫描不运行它。2.24.1 已通过标准 Google
设备登录与真实网页 SDK 回复 → 停止 → CLI 同 SID 回放 → 停止 → SDK 回放；
原生模型配置已确认。此后验收脚本改用自然中文问题，恢复断言核对实际收到的助手正文；
修改后的脚本已通过合成浏览器闭环，未为措辞调整重复发起真实请求。

原生高级能力与尚未验证项见 [Kiro sessions](kiro.md)，剩余工作只记在
[TODO](../TODO.md#kiro-integration)。
