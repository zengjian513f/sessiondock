# Kiro 原生 SDK 桥接

目标链路为公共 SDK HTTP/SSE、草稿和队列 → Kiro ACP 适配器 → sdkhost →
`kiro-cli acp --agent-engine=v3`。当前实现尚未通过
[实施计划](../implementation_plan_codex_for_kiro.md)的原生身份与恢复验收，不能按
初稿硬编码的能力表宣称所有 V3 操作可用。

## 版本与能力证据

2026-10-21 实际安装为 `2.24.1`；此前文档写的 `1.29.0` 不能作为现场版本依据。
隔离空认证探测在 `initialize` 返回前结束，因此尚无该安装版的真实能力广告。
[官方迁移说明](https://kiro.dev/docs/cli/v3/acp-migration/)与目标安装版可能有差异，
所有可选操作须以运行时 `initialize`、配置选项和扩展目录为准。

| 接口 | 接入依据及验证边界 |
| --- | --- |
| `initialize` | 读取真实能力与扩展目录，不能只按版本号启用功能 |
| `session/new` | 使用返回的 SID；先持久化创建意图，再保存稳定 UID 与宿主绑定；真实双模式闭环未验收 |
| `session/list` | 按广告能力分页发现；标准字段含 SID/cwd，不提供原生文件路径；目标版返回尚未取得 |
| `session/load` | 显式恢复并接收回放；在调用前注册更新处理；不当作普通只读历史接口 |
| `session/prompt` | 稳定投递 ID、原生最终结果与实例退出分开；重启不重发已写入或结果未知的输入 |
| `session/cancel` | 请求取消不等于回合结束，须等待对应 prompt 结果或确定退出才能释放队列 |
| 权限与问题请求 | 按实际请求结构答复；回答持久化及宿主写入确认后再退卡，刷新不能丢失待回答项 |
| 模型、effort、mode | 仅使用原生返回的配置选项与确认值，不硬编码 V2/V3 参数回退 |
| 改名、分支、压缩、历史导出 | 仅按广告的标准或扩展接口接线；未知方法和参数不能用私有文件改写模拟 |
| 删除 | 标准 ACP 有可选删除能力，但目标版是否支持、能否恢复、如何处理活动会话仍未验证；不能模拟回收站语义 |

标准协议参考：[会话建立](https://agentclientprotocol.com/protocol/v1/session-setup)、
[会话目录](https://agentclientprotocol.com/protocol/v1/session-list)、
[可选删除](https://agentclientprotocol.com/protocol/v1/session-delete)。
这些是接口依据，不是 Kiro `2.24.1` 的实测结果。

## 恢复与验收

已有宿主恢复须重建持久 RPC 关联、输出游标、消息、待回答请求和最后回合结果，
不能仅重新连接 stdout 尾部。宿主不存在或写入不确定时据实报告，不启动替代进程
假装无损恢复。新建宿主与最终 UID 必须持久绑定，停止不能依赖私有历史文件存在。

当前合成浏览器套件覆盖基本 SDK 页面、审批、问答及部分动作，不证明上述重启、
创建和真实原生接口都已完成。[TODO](../TODO.md#kiro-integration)记录具体缺口；
历史和双模式边界见 [Kiro sessions](kiro.md)。
