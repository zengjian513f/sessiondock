# Kiro sessions

Kiro 的目标合同以 [接入实施计划](../implementation_plan_codex_for_kiro.md) 为准。
当前接入仍是未完成的初稿，不能将合成浏览器测试通过视作真实原生验收完成。

## 已核对的原生入口

2026-10-21 在隔离 HOME、XDG、Kiro 配置目录和临时工作目录中核对：

- 安装版本为 `kiro-cli 2.24.1`。
- CLI 帮助包含 `--agent-engine=v3`、`--resume-id`、`--list-sessions`、
  `--all-cwds`、`--format json`、`--model`、`--effort` 和删除入口。
- ACP 帮助包含 V3 引擎和 `--auth-method=cli`。
- 空认证环境中的结构化列表，以及显式或省略认证参数的 ACP 启动，均在返回
  列表或 `initialize` 能力之前因未登录退出。没有调用模型，也没有操作生产会话。

这只验证了命令入口。目标安装版的能力广告、列表数据、只读历史接口、删除语义，
以及 SDK 创建后经 CLI 再回到 SDK 的 SID 和内容连续性，均未得到真实验证。
官方命令说明见 [CLI reference](https://kiro.dev/docs/reference/cli-commands/)。

## 双模式及只读边界

Kiro 提供 CLI 原生终端和 ACP SDK 两种模式，不增加 PTY 屏幕解析或普通 CLI 对话
投影。两个模式必须使用同一原生 SID，互斥及停止行为遵循实施计划和公共生命周期
合同，不能凭命令名称宣称已经验证。

[官方 V3 迁移说明](https://kiro.dev/docs/cli/v3/acp-migration/)要求用公开接口
发现和恢复会话，不依赖私有存储布局。SessionDock 应保存自己的目录和历史投影：
发现使用广告的 `session/list`；恢复使用 `session/load`；只读历史或导出仅调用
实际广告的接口。`session/load` 会激活会话并回放历史，普通查看不能调用它。

若目标版本没有已确认的只读历史接口，采用“缓存可查看，用户显式启动 SDK 后同步”
的降级；未缓存正文及搜索覆盖范围必须据实说明。配置作用域与原生 SID 定义身份，
不能以原生文件是否落盘或猜测的路径定义 UID。升级必须保留已有 SessionDock 身份
映射，不能把切换投影目录当作会话迁移。

当前代码中的私有 V2/V3 文件解析及原生文件改写不符合该合同，不能作为官方协议
证据或后续实现依据。具体未完成事项集中记录在 [TODO](../TODO.md#kiro-integration)。
ACP 接口的证据边界见 [SDK 桥接](kiro-sdk.md)。
