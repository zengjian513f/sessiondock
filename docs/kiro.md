# Kiro sessions

Kiro 使用同一原生 SID 提供 CLI 终端与 ACP SDK 两种模式。接入边界见
[实施计划](../implementation_plan_codex_for_kiro.md)与[SDK 桥接](kiro-sdk.md)。
不解析 TUI，也不读取或改写 Kiro 私有会话文件。

## 原生验证及限制

2026-10-21 在独立 HOME/XDG、配置目录和工作目录中验证 `kiro-cli 2.24.1`：

- 用户通过设备授权登录，认证只留在隔离环境。
- `initialize` 广告 `loadSession`、标准 `session/list`、`session/delete`、
  `session/fork`（支持 `messageId`），以及 `_kiro/session/history`、
  `_kiro/session/export`、`_kiro/session/context`、`_kiro/session/compact`。
  没有广告改名、steer 或账户额度接口。能力仍以每次运行的广告为准。
- ACP `session/list {}` 能跨本地工作目录列出会话；显式 `cwd` 会筛选。
  分页使用 `nextCursor`；尚无真实多页或远程来源的验收证据。
  同一安装版 CLI `--list-sessions --all-cwds --format json` 返回空列表，
  不能替代 ACP 目录或据此删除缓存。
- SDK 创建的 SID 可用于 CLI `--agent-engine=v3 --resume-id`；CLI 界面显示原
  用户消息。停止 CLI 后，独立 ACP `session/load` 接受同 SID 并回放原用户消息。
  `load` 响应可以没有 `sessionId`，桥接保留可信请求中的 SID。
- 显式选择目录中最低费率的 `qwen3-coder-next`，原生配置确认该模型。
  其 `hasEffort=false`，无 effort 选项，强度标为不适用，不伪造确认值。
- 首个模型请求被服务以 `TemporarilySuspendedError` 拒绝，提示测试账号临时暂停。
  没有成功的模型回答，**完整 SDK → CLI → SDK 对话连续性仍未通过真实验收**；
  未重试付费请求或回退到其他模型。[Kiro 支持](https://support.aws.amazon.com/#/contacts/kiro)
  处理账号限制后，须继续真实验收。

`history {sessionId}` 在独立 ACP 中能返回结果，但会发初始化通知；实测其默认
`updates:[]` 与随后 `load` 回放的用户消息不一致。因此不能将它当作完整只读历史，
也不能由调用成功推导零激活副作用。非空历史分页、删除／分支的全部原生交互、工具
审批和问答的真实模型回合仍需验收；合成测试不替代这些证据。

## 目录、身份与缓存

发现只使用广告的 ACP `session/list`，完整分页后保留可信 SID/cwd/标题/时间。
目录失败保留已知数据；普通列表、查看和搜索不调用 `session/load`。

`SESSIONDOCK_KIRO_ROOT` 只标识账号配置作用域，不再作为扫描或写入目录。
CLI profile 的 HOME/XDG/Kiro 环境决定原生认证作用域。私有投影默认放在
`<state>/kiro-catalog/<scope>`；`SESSIONDOCK_KIRO_PROJECTION_ROOT` 可设置其基目录。
切换 HOME／配置作用域时目录、历史和创建回执隔离；切回原作用域保留原身份。
旧身份清单只在初始作用域导入一次，删除 tombstone 不因重启重新导入。
没有私有 state 或投影目录时不启用这个缓存适配器。

UID 由配置作用域与原生 SID 绑定，独立于原生文件的存在和布局。创建意图、宿主
实例和原生结果先持久化，再返回 HTTP 回执；响应丢失重试取原结果。
同一创建请求不能换目录或选项。CLI 新建通过 ACP 预创建，确认释放**原创建宿主**后
按同 SID 启动 CLI；重试旧请求不会结束后来启动的新 SDK 实例。

升级前使用 [身份迁移准备工具](../deploy/kiro_identities.py)从原 SessionDock 目录 API
保存 UID/SID 映射；工具不读原生会话正文。启动导入这些记录和已有 SDK bindings／
receipts，保留旧 UID。不能逆推路径哈希，不能按目录、标题或最近时间猜身份。
离线节点必须先完成该准备步骤再升级。

历史采用“缓存可查看，用户显式启动 SDK 后同步”的降级：`load` 回放与 live 更新
通过同一 ACP 投影器进入 SessionDock 自有消息文件，再复用公共历史分页、搜索和
媒体管线。未同步会话显示提示；搜索只覆盖已缓存正文，缓存不声称与原生始终同步。
原生文件和终端文字均不作为消息来源。缺少稳定消息 ID 时不按相同文本猜重。

## 生命周期与管理

CLI／SDK 的启动检查、受管实例绑定和停止使用同一会话锁；另一模式仍活动时提示
先停止。外部 CLI 只使用明确 `--resume-id` 的进程证据，不读取或清理私有锁文件，
不声称识别没有明确 SID 的所有外部实例。网页断线不结束原生进程。

改名未广告则明确不可用。分支和删除只调用实时广告的官方方法；分支请求通过
独立持久宿主保存 RPC 结果，重试不重复创建。Kiro 删除是原生永久删除，不模拟可恢复
回收站，不移动或改写原生文件；确认成功后才清私有投影和创建回执。
通用复制／移动不模拟原生恢复能力，混合组保留具体阻碍项。

原生参考：[V3 ACP 迁移](https://kiro.dev/docs/cli/v3/acp-migration/)、
[CLI 命令](https://kiro.dev/docs/reference/cli-commands/)。
