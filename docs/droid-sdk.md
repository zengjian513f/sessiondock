# Droid SDK 与原生会话

DELTA：Factory Droid 接入原生 JSON-RPC，复用 SessionDock 的 SDK 界面、持久输入
队列、历史分页和 sdkhost。它也提供原生 CLI 终端；不解析 TUI 屏幕作为对话视图。
协议依据：Droid 1.6.0 隔离探测、官方 `@factory/droid-sdk` 0.10.0 协议定义。

## 配置与身份

- 原生根为 `SESSIONDOCK_DROID_ROOT`，默认 `$HOME/.factory/sessions`；空值关闭。
  默认根沿用其他来源的 HOME 作用域规则，不创建缺失目录。
- launcher 配置唯一 `source:"droid"` profile，使用本机 Droid 可执行文件及其环境。
  SDK 校验 `FACTORY_HOME_OVERRIDE`（否则 HOME）下的 `.factory/sessions` 与读根一致。
- 原生文件为 `<root>/<cwd 中斜杠替换成连字符>/<sid>.jsonl`；兼容根下旧式文件。
  `session_start.id` 是身份，UID 沿用文件管线；不按标题猜 SID。
- SDK 创建生成 UUID，并通过 `droid.initialize_session` 传 `sessionId/cwd`，
  原生 CLI 负责建文件。恢复发送 `droid.load_session {sessionId}`。
- CLI 新建身份待原生文件出现，恢复用 `--resume <sid>`。CLI 模型目录和启动覆盖
  尚未接入，沿用原生设置；SDK 模型目录来自 `droid.list_models` 或初始化回执。
- 版本使用 `droid --version`，更新沿用 `droid update`；最新版本远程查询暂不提供。

## SDK 操作

子进程 argv：`droid exec --input-format stream-jsonrpc --output-format stream-jsonrpc`。
JSON-RPC 模式的模型、强度必须通过请求传递，CLI 同名参数在此模式不生效。

| 操作 | 原生协议 |
| --- | --- |
| 新建／恢复 | `droid.initialize_session` / `droid.load_session` |
| 发送／运行中引导 | `droid.add_user_message {messageId,text,queuePlacement:"end_of_turn"}`；普通发送先经桥接 FIFO，引导直接提交原生 |
| 中断 | `droid.interrupt_session`；结束以 `agent_turn_completed` 为证据 |
| 模型／强度／模式／自主权限 | `droid.update_session_settings`；模式 auto/spec，自主权限 off/low/medium/high，显示原生通知确认值 |
| 重命名 | `droid.rename_session {title}`，原生更新 session_start，不自行重写 JSONL |
| 分支 | 全量用 `droid.fork_session {title?}`；消息前分支用 `droid.execute_rewind`，文件恢复／删除数组均为空；使用 newSessionId 在新会话继续 |
| 压缩 | `droid.compact_session {customInstructions?}`；按返回 SID 刷新原会话或切换新会话 |
| 模型目录 | `droid.list_models`，不创建会话或模型回合；强度按模型返回 |
| 审批 | `droid.request_permission` 回送 `selectedOption`；展示原生允许本次／取消选项 |
| 问答 | `droid.ask_user` 回送带原生 index 的 answers；拒绝回送 cancelled |

SDK 不接管独立 TUI；Linux 使用现有进程/SID/原生路径占用检测。其他平台尚不开放
既存会话恢复。页面关闭不停止任务；节点重启复接原 sdkhost，不重发初始化和输入。
投递请求 ID、消息 ID、输出游标、审批请求及回执保存在私有桥接检查点。
发送须收到原生响应才确认准入；失联无法确认时保留 unknown，不自动重发。
`create_message.requestId` 或相同 messageId 的回显才作为原生消费证据。

未开放文件回退、原地对话回退、子代理独立 SDK 控制及共享 daemon。
界面按能力隐藏相关操作，不把斜杠控制命令发给模型。附件沿用共用文件发布合同。

DELTA：重命名、分支和压缩使用持久操作请求 ID；发送前保存意图，重复请求不重放
原生操作，不确定结果保留错误供核对。设置以 settings_updated 为准，不将请求值
当作生效值。自主权限使用 Droid 原生等级，不冒充 Codex 沙箱策略。
普通压缩在 1.6.0 实测返回原 SID 并写 compaction_state；即使字段名为 newSessionId，
也不能推断一定新建。压缩后刷新历史与 ctx；若返回新 SID，先结束旧身份的传输，
再转到原生新身份，避免后续输入串会话。分支保留源会话，原生文件决定子会话设置；
原生可能迁移旧模型，界面显示实际模型，不承诺与源会话完全相同。操作取消不发送 RPC。
消息前分支的 before 必须是原生消息 ID，截去选中消息及后文；原生创建新身份，
源历史保留，结束原 SDK 传输后连接新身份，不修改工作目录文件。仍不提供文件恢复界面，
也不把原生 execute_rewind 宣称为原地回退。

DELTA：运行中 Ctrl+Enter、Ctrl 点击、移动长按及菜单引导复用持久输入提交；空闲时
手势沿普通发送。已排队输入可用“立即引导”保留原投递身份，普通 FIFO 仍等待当前任务
结束。引导不重置当前回合状态；原生拒绝不能结束正在运行的任务。原生响应只确认接收，
create_message 的 requestId 或 messageId 匹配才确认已进入原生记录，不承诺模型已经执行。
请求结果不确定时保留账本且不自动重发。引导附件沿普通输入发布为文件引用；不复制
其他来源仅文字的限制。

## 记录与通知投影

| 原生记录／通知 | 投影 |
| --- | --- |
| `session_start` | SID、标题、cwd；不作为消息 |
| `message` / `create_message` | role、content、时间、modelId、原生 id；历史和实时按 id 对齐 |
| `visibility:llm_only` | 非空上下文显示为 system，不冒充用户提交；空上下文不显示 |
| `text` / `thinking` | 正文／思考；`redacted_thinking` 不展示不透明数据 |
| `tool_use` / `tool_call` | name、input、工具原生 id |
| `tool_result` | 按 toolUseId 配对，isError 保留；没有已知调用时独立保留原始结果 |
| `assistant_text_delta` / `thinking_text_delta` | 按 messageId/blockIndex 增量更新 |
| `assistant_message_retracted` | 移除该原生消息的实时投影 |
| `settings_updated` | 回读模型和强度；不修改日常配置文件 |
| `session_token_usage_changed` / `agent_turn_completed` | Token 用量与结束原因；cancelled 为中断，认证等错误为失败 |
| `error` | 可见原生错误；不把失败当成功 |
| `permission_resolved` | 撤去已解决审批 |
| `session_compacted` | 状态栏信息，不制造助手回复 |
| 未识别历史记录 | 可见 system 原文及 native_type，不推测语义 |

DELTA：session_start.callingSessionId 明确关联同来源父会话，复用现有子代理历史
与导航；普通 fork 的 parent 单独作为分支关系，不能误判为子代理。关系本身不表示
任务正在运行；孤儿和歧义父身份沿用通用索引行为。子代理独立 SDK 发送、中断仍关闭，
child_session_available 实时通知尚未接入关系投影。

消息里的文本 tag 不全局剥离；未知包裹保留正文。后台任务、hook、mission、MCP
状态等通知尚未投影，不宣称完整识别；原始流保留在私有 sdkhost 日志。
读取不写原生文件。删除将 JSONL 和同名 `.settings.json` 一起放入回收站；
不支持移动或克隆，不重写原生身份或元数据。

## 验证边界

[droid_sdk_browser.py](../tests/droid_sdk_browser.py) 使用
[合成协议夹具](../tests/droid_sdk_fixtures.py)，覆盖节点／Hub、历史与实时衔接、发送、
设置、审批和问答点击、失败、中断、刷新、新建弹窗、CLI 终端、回收站恢复及节点重启复接。
真实 1.6.0 探测确认模型查询、创建、恢复、用户回显、落盘与认证失败事件。
用户授权的生产专用会话以 `claude-haiku-4-5-20251001` / low 验证真实回复、
ctx、Token/Credits 和账户额度。隔离 HOME 已验证 Execute 的简单命令及运行中引导；
审批和问答仍只有合成验收，后续项见 [TODO](../TODO.md)。

官方参考：[Droid Exec](https://docs.factory.com/droid-exec/overview)、
[TypeScript SDK](https://docs.factory.com/sdk/typescript)。

## 外部 CLI 与接管

DELTA：Linux 的 Droid 外部实例优先使用原生日志
`.factory/logs/droid-log-single.log` 的 `process.pid`、`tags.sessionId` 和 TUI 标记
关联当前会话。日志时间必须不早于该存活进程的启动时间；同进程使用最新的有效身份，
忽略 PID 复用前的旧记录。日志路径取该进程的 `FACTORY_HOME_OVERRIDE` 或 HOME，
不读取认证文件。无原生身份时，裸 `droid` 沿用 Claude 的同 cwd、创建时间相对进程
启动 −5 至 +30 秒的识别规则；不能仅凭同目录认领所有历史会话。

外部实例显示绿点；CLI 接管先确认，取消不结束进程，确认后复用既有 PID/start-time
核对与 TERM/KILL 流程，结束外部实例再由 ptyhost `--resume`，受管实例显示蓝点。
SDK 同样拒绝并行打开已识别的原生会话；同目录身份未解析的裸 Droid 沿用 Claude
的占用检查。浏览器覆盖见 [外部实例测试](../tests/droid_external_browser.py)。

## 上下文与用量

DELTA：SDK 用量读取在已经连接的原生传输上调用 `droid.get_context_stats`，
显示 used/limit 和 accuracy；估算值明确标为“原生估计”，不是累计 Token。
只读查询不启动或接管外部 TUI；失败保留先前数据并显示错误。
会话累计用量来自恢复回执 tokenUsage、session_token_usage_changed 和结束事件的
cumulativeTokenUsage。结束事件仅有单回合 tokenUsage 时不覆盖累计值。
分别显示输入、输出、思考、缓存读写和 Factory Credits；累计 Token 摘要为输入、输出、缓存读写之和，
思考为输出子项不重复累加，缓存计数在提示中也单列，不把它当上下文长度或账户剩余额度。未提供字段不补零。
账户的 Standard/Core 六个窗口及重置倒计时见 [额度合同](quotas.md#droid-接入)。
[Droid 用量回归](../tests/droid_usage_browser.py) 覆盖加密认证、上下文、累计计数、
Credits、刷新失败保留、原生命令不推理及宽窄屏。

## 高级操作验收

[Droid 操作浏览器回归](../tests/droid_actions_browser.py) 覆盖节点与 Hub 的模式／权限、
原生改名、分支继续、同 SID 压缩和新 SID 回执、取消、失败及幂等。
[子代理历史回归](../tests/droid_children_browser.py) 覆盖父子导航、独立分支、孤儿与
歧义父身份，不启动子代理写者。原生 1.6.0 独立 HOME 探测确认改名落盘、模式／权限
通知、fork 新 SID、普通 compact 同 SID，以及 callingMetadata 的持久父身份。
[Droid 引导回归](../tests/droid_steer_browser.py) 验证节点／Hub 的发送手势、队列转引导、
附件、草稿、幂等及失联不重发。隔离 Haiku 4.5 / low 原生探测确认运行中引导进入同一
回合并改变最终回复，以及 execute_rewind 的消息前新分支和零文件变更。

生产专用探测会话经 Chromium 验证原生重命名、模式／自主权限回读及分支后的 SDK
连接；分支实际模型由 Haiku 4.5 迁移为 Haiku 5.5，未向迁移后的模型发送推理请求。
验收后关闭这两个专用 SDK 运行实例，保留原生历史；压缩真实验证仍为隔离 HOME 探测。

生产专用会话另以 Haiku 4.5 / low 验证：在 Execute 执行期间从网页 Ctrl+Enter
投递引导，原生最终回复按引导改变；消息前分支返回新 SID，恢复原输入到编辑器且
原生报告恢复／删除文件数均为零。分支后未发送新推理，验收结束关闭专用运行实例。
