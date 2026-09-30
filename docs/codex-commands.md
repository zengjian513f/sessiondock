# Codex 命令与原生回显

父合同：[会话草稿与 SEND](conversation.md)、[CLI 状态对象](cli-state.md)。BUG-20260930-150246-0774e3 的首个错误发生在成功投递之后：服务端把 `/model` 当作需要同文原生回显的普通输入入账，而 Codex 只打开本地菜单。相同假设也不适用于其它内置命令。

## 源码核对

核对 Codex 源码提交 `67727e7cf114cf3e1b71db368d74b24e32f6cb12`：

- `codex-rs/tui/src/slash_command.rs` 定义内置名称、别名及 `supports_inline_args`。
- `codex-rs/tui/src/bottom_pane/chat_composer/slash_input.rs` 和 `bottom_pane/prompt_args.rs` 定义首行裸命令、带参数命令、路径及前导空格转义的解析。
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs` 把内置命令分派到本地操作、会话操作或其它输入；本地输入回忆历史不等于 rollout 的 user/command 回显。
- `codex-rs/tui/src/chatwidget/service_tiers.rs` 从模型目录提供动态服务档位命令。

当前核对的 66 个内置名称和别名及参数标记存于 [命令清单](../tests/fixtures/codex_send_commands.json)。实现另外覆盖 `/fast` 和 `/goooal` 这类重复 `o` 的 goal 别名。清单包含依赖功能开关或仅供调试的名称；是否可用仍由实际 CLI 决定，SessionDock 不增加命令拒绝策略。

| 类别 | 例子 | 原生记录与 SEND 对账 |
| --- | --- | --- |
| 本地菜单、设置或查询 | `/model`、`/permissions`、`/status`、`/rename 名称`、`/mcp`、`/theme` | 不写原命令同文 user 记录；不建立等待回显气泡 |
| 会话切换、退出或独立操作 | `/new`、`/resume`、`/fork`、`/quit`、`/compact`、`/goal 目标` | 分派到对应操作；不等待原命令回显，也不以 `sent` 承诺操作结束 |
| 转成其它模型输入或任务 | `/init`、`/plan 正文`、`/review 正文`、`/side 正文` | 初始化模板、去前缀正文、review 操作或子会话输入不等于原命令；照常显示原生任务历史，不建立原命令等待项 |
| 普通正文 | ` /status`、`/model is mentioned here`、`/tmp/file` | 等待同文原生 user 回显；前导空白保存在排队账本，摘要去首尾空白 |

## 解析与边界

命令必须从实际发送正文第一个字节的 `/` 开始，名称紧跟其后且不含 `/`。前导空白不裁掉。首行只有已知命令及尾随空白时是裸命令；对不支持参数的命令，后续行不会把它变成普通输入，例如 `/model\n其它正文` 仍由 Codex 分派为模型菜单。支持参数的命令可从后续行读取正文。引用或附件说明拼接后的完整正文也使用这些规则，投递保持原样。

仅对清单中支持参数的命令识别带参数形式；`/model 正文` 等不支持参数的同一行输入仍保留普通消息的等待记录。未知名称、未来新增命令、自定义命令及 `/fast` 之外的动态服务档位没有在此次核对中建立语义保证，沿用普通输入对账；CLI 拒绝或不回显时仍可能留下等待项，可到终端核对后关闭。升级 Codex 时应重新核对名称、解析和分派。

旧账本修复必须用成功提交回执证明排队正文就是原输入，才撤掉误记的内置命令。无法区分被旧版本裁掉前导空格的记录、包含引用/附件的记录及无回执记录保留；不会因为当前文字看起来像命令就删掉真实消息。

## 浏览器验证

[send_native_codex_browser.py](../tests/send_native_codex_browser.py) 在临时目录和隔离假 CLI 下通过真实页面输入、点击 SEND，覆盖全部清单裸命令及参数形式、别名和多行解析；同时验证模型菜单、刷新、提交 ID 重放、按回执修复旧记录，以及转义文本、非参数命令正文、路径、同文重复发送的真实队列及原生回显退队。假 CLI 仅模拟命令分派和回显区别，保持进程存活以遍历全部名称；这不是每个命令实际业务效果的真实 CLI 集成测试。
