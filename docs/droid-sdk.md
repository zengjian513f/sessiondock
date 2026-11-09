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
| 发送 | `droid.add_user_message {messageId,text,queuePlacement:"end_of_turn"}` |
| 中断 | `droid.interrupt_session`；结束以 `agent_turn_completed` 为证据 |
| 模型／强度 | `droid.update_session_settings {modelId,reasoningEffort}`，立即生效 |
| 模型目录 | `droid.list_models`，不创建会话或模型回合；强度按模型返回 |
| 审批 | `droid.request_permission` 回送 `selectedOption`；展示原生允许本次／取消选项 |
| 问答 | `droid.ask_user` 回送带原生 index 的 answers；拒绝回送 cancelled |

SDK 不接管独立 TUI；Linux 使用现有进程/SID/原生路径占用检测。其他平台尚不开放
既存会话恢复。页面关闭不停止任务；节点重启复接原 sdkhost，不重发初始化和输入。
投递请求 ID、消息 ID、输出游标、审批请求及回执保存在私有桥接检查点。
发送须收到原生响应才确认准入；失联无法确认时保留 unknown，不自动重发。
`create_message.requestId` 或相同 messageId 的回显才作为原生消费证据。

未开放 rename、compact、fork、steer、原生权限模式设置、子代理导航和共享 daemon；
它们不是 Droid 协议本身不支持，而是本次适配未覆盖。界面按能力隐藏相关操作，
不把斜杠控制命令发给模型。附件沿用共用文件发布合同。

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

消息里的文本 tag 不全局剥离；未知包裹保留正文。后台任务、hook、mission、MCP
状态等通知尚未投影，不宣称完整识别；原始流保留在私有 sdkhost 日志。
读取不写原生文件。删除将 JSONL 和同名 `.settings.json` 一起放入回收站；
不支持移动或克隆，不重写原生身份或元数据。

## 验证边界

[droid_sdk_browser.py](../tests/droid_sdk_browser.py) 使用
[合成协议夹具](../tests/droid_sdk_fixtures.py)，覆盖节点／Hub、历史与实时衔接、发送、
设置、审批和问答点击、失败、中断、刷新、新建弹窗、CLI 终端、回收站恢复及节点重启复接。
真实隔离探测确认 1.6.0 的模型查询、创建、恢复、用户回显、落盘与认证失败事件。
临时 HOME 无登录凭据，真实模型回合返回 `No access token available`；合成成功回复
不作为真实模型生成、工具执行或审批已验收的证明。后续项见 [TODO](../TODO.md)。

官方参考：[Droid Exec](https://docs.factory.com/droid-exec/overview)、
[TypeScript SDK](https://docs.factory.com/sdk/typescript)。
