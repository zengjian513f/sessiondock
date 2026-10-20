# 文档索引

开发从 [AGENTS.md](../AGENTS.md)、[开发手册](runbook-dev.md) 与受影响合同开始。
当前行为写在专题合同，未完成工作只在 [TODO](../TODO.md)；交付结果、事故时间线和
完整提交目录由 Git 历史保存，不作为当前规范。历史资料仅在相关任务时查阅。

本索引只生成分类和标题，避免复制易过期的摘要、行数或功能状态。
重新生成：`python3 tests/docs_index.py --write`。

## 开发与交付

| 文档 | 内容 |
| --- | --- |
| [adding-a-cli.md](adding-a-cli.md) | 接入新的 AI CLI |
| [delegation.md](delegation.md) | 委派本机 grok CLI（grok-4.6）做简单任务 |
| [deploy-hub.md](deploy-hub.md) | 部署 `sessiondock-hub`（多机 Hub，仅样例） |
| [deploy-macos.md](deploy-macos.md) | macOS 节点原生构建与 launchd 部署 |
| [deploy-windows.md](deploy-windows.md) | Windows 节点原生构建与滚动部署 |
| [deployment.md](deployment.md) | 舰队部署工具 `deploy/deploy.py` |
| [network-topology.md](network-topology.md) | 运维：节点身份与网络边界 |
| [onboarding.md](onboarding.md) | SessionDock onboarding |
| [public-release.md](public-release.md) | Public source snapshots |
| [release.md](release.md) | Release package and CI |
| [replacement-checklist.md](replacement-checklist.md) | 运行与回退清单 |
| [runbook-dev.md](runbook-dev.md) | Local developer runbook |
| [validation.md](validation.md) | Validation suites |

## SDK、对话与界面

| 文档 | 内容 |
| --- | --- |
| [agy-sdk.md](agy-sdk.md) | Agy 原生 SDK 模式 |
| [claude-sdk.md](claude-sdk.md) | Claude 原生 SDK 桥接 |
| [cli-state.md](cli-state.md) | 会话 CLI 状态对象 |
| [codex-commands.md](codex-commands.md) | Codex 命令与原生回显 |
| [codex-sdk.md](codex-sdk.md) | Codex SDK mode |
| [composer-input.md](composer-input.md) | 对话输入就绪 |
| [conversation.md](conversation.md) | 会话草稿与 SEND |
| [frontend-migration-surfaces.md](frontend-migration-surfaces.md) | 现有前端用户可见功能清单 |
| [grok-sdk.md](grok-sdk.md) | Grok 原生 SDK 桥接 |
| [native-sdk.md](native-sdk.md) | 多来源 SDK 模式 |
| [opencode-sdk.md](opencode-sdk.md) | OpenCode 原生 SDK 桥接 |
| [question-card-regression-audit.md](question-card-regression-audit.md) | 活动题卡核验范围 |
| [quotas.md](quotas.md) | Agent 额度 |
| [sdk-feature-audit.md](sdk-feature-audit.md) | 五种 SDK 能力对照 |
| [sdk-ui.md](sdk-ui.md) | SDK 与对话状态栏 |

## 会话、历史与文件

| 文档 | 内容 |
| --- | --- |
| [agy.md](agy.md) | Agy（Antigravity CLI） |
| [append-cache.md](append-cache.md) | Reusing native JSON records on append |
| [codex-names.md](codex-names.md) | Codex name metadata |
| [delivery-scope.md](delivery-scope.md) | Trusted native scope selection |
| [external-links.md](external-links.md) | External links |
| [files.md](files.md) | Files and FileDock |
| [grok-metadata.md](grok-metadata.md) | Grok native metadata compatibility |
| [history-pages.md](history-pages.md) | Session list, views and finite history pages |
| [media-pagination-design.md](media-pagination-design.md) | Media continuation and history pagination |
| [media.md](media.md) | Native media |
| [metadata.md](metadata.md) | Development metadata store |
| [native-input.md](native-input.md) | Native input and structural scanning |
| [native-tags.md](native-tags.md) | Native history tag envelopes |
| [nesting-regression-audit.md](nesting-regression-audit.md) | 附属关系核验范围 |
| [opencode.md](opencode.md) | OpenCode sessions |
| [read-model.md](read-model.md) | 读模型设计：惰性索引 + 按需视图 |
| [session-clone.md](session-clone.md) | 会话整组克隆 |
| [session-move.md](session-move.md) | 会话整组移动与克隆 |
| [trash.md](trash.md) | Session recycle bin |

## 终端与生命周期

| 文档 | 内容 |
| --- | --- |
| [host-launch-identity.md](host-launch-identity.md) | Host launch identity envelope |
| [host-native-binding.md](host-native-binding.md) | One-time host native binding |
| [host-output.md](host-output.md) | Host attachment output |
| [lifecycle-binding.md](lifecycle-binding.md) | Explicit operator native binding |
| [lifecycle-http.md](lifecycle-http.md) | Explicit local creation and pending consoles |
| [lifecycle-integration.md](lifecycle-integration.md) | Controlled creation: integration contract |
| [lifecycle-launcher.md](lifecycle-launcher.md) | Lifecycle launcher |
| [lifecycle-service.md](lifecycle-service.md) | Bounded lifecycle coordinator |
| [lifecycle-store.md](lifecycle-store.md) | Private lifecycle creation receipts |
| [session-host.md](session-host.md) | ptyhost 开发说明 |
| [terminal-final-screen.md](terminal-final-screen.md) | Terminal final screen |
| [terminal-grid.md](terminal-grid.md) | Server-side terminal grid |
| [terminal-identity.md](terminal-identity.md) | Instance-bound terminal protocol |
| [terminal-input.md](terminal-input.md) | Raw terminal input over HTTP |
| [terminal-ownership.md](terminal-ownership.md) | Browser terminal ownership |

## 架构、接口与运行服务

| 文档 | 内容 |
| --- | --- |
| [architecture.md](architecture.md) | 架构边界 |
| [bug-report.md](bug-report.md) | Bug reports and their workers (M7) |
| [capabilities.md](capabilities.md) | Capability flags |
| [diagnostics.md](diagnostics.md) | Browser diagnostics |
| [environment.md](environment.md) | Environment variables |
| [error-codes.md](error-codes.md) | HTTP error codes |
| [glossary.md](glossary.md) | Glossary |
| [hub.md](hub.md) | Hub：注册表、健康监控、节点客户端、命名空间、聚合、代理与 `sessiondock-hub` |
| [liveness.md](liveness.md) | External CLI liveness: the `/proc` scan |
| [logging.md](logging.md) | Server log |
| [module-map.md](module-map.md) | Module map |
| [process-links.md](process-links.md) | Shared process attribution and session resource accounting |
| [processes.md](processes.md) | Controlled process observations |
| [resource-agent.md](resource-agent.md) | Optional Linux resource collector |
| [route-ledger.md](route-ledger.md) | HTTP route ledger |
| [sdkhost.md](sdkhost.md) | 独立 SDK 进程宿主 |
| [security-model.md](security-model.md) | SessionDock trust boundaries |
| [session-transfer-tool.md](session-transfer-tool.md) | 整组操作：离线计划与暂存工具 |
| [session-transport.md](session-transport.md) | 会话迁移传输层 |
| [shell-env.md](shell-env.md) | Login-shell environment |

## 历史资料（按需查阅）

| 文档 | 内容 |
| --- | --- |
| [history-incidents.md](history-incidents.md) | 历史事故与验收记录 |
| [perf-methodology.md](perf-methodology.md) | Performance measurement methodology |
| [performance.md](performance.md) | 历史性能记录：真实读根与合成规模 |
| [process-launch-audit.md](process-launch-audit.md) | 外部程序启动与资源释放审计（2026-10-07） |
