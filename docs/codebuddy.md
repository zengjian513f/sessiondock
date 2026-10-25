# CodeBuddy 会话

CodeBuddy Code（腾讯，npm 包 `@tencent-ai/codebuddy-code`）与 Kiro 一样只提供两种模式：
[SDK 模式](codebuddy-sdk.md)和 CLI 终端。不解析 PTY 屏幕做对话视图
（`sessionHasChatView` 为 false），输入就绪识别、菜单题卡和问题报告处理会话均未接入，
这些入口保持原生终端，不宣称支持。

证据版本：CodeBuddy 2.175.0（真实 CLI，中国站账号，测试模型 `glm-5.3-flash`）。
下列结论来自该版本的实机探测和包内源码阅读；升级后须重新核对。

## 原生存储

- 根：`SESSIONDOCK_CODEBUDDY_ROOT`，缺省 `$HOME/.codebuddy/projects`（规则同 Kiro：
  其他已配置根位于该 HOME 下，或都未配置）。空值关闭。
- 路径：`<root>/<项目目录>/<sid>.jsonl`，一个会话一个文件，无子代理或旁路文件。
  项目目录名＝工作目录绝对路径中的每个 `/` 换成 `-`，再去掉开头的 `-`
  （`/tmp/cb-probe/.hid/a.b` 为 `tmp-cb-probe-.hid-a.b`）；空格、点、下划线和非 ASCII
  原样保留。没有观察到长度截断。
- 会话 ID 是 UUID，可在启动前用 `--session-id <uuid>` 指定（`new_assigned`），
  用 `--resume <sid>` 在原工作目录恢复。UID 为 `codebuddy:` 加路径哈希。
- 登录凭据在 `~/.codebuddy/local_storage`，设置在 `~/.codebuddy/settings.json`。
  SessionDock 不读取、复制或展示这些文件。

## 原生记录类型

| `type` | 字段 | 投影 |
| --- | --- | --- |
| `message` + `role:user` | `content:[{type:input_text,text}]`，`providerData` | 用户消息；`isMeta`/`skipRun`/`isCompact*` 为内部记录，不显示；以 `<turn_aborted>` 开头的记录显示为"已中断"状态 |
| `message` + `role:assistant` | `content:[{type:output_text,text}]`，`status`，`providerData.model` | 助手正文；一轮中最后一段无后续工具调用的正文标 `phase:final`，其余为 `progress` |
| `reasoning` | `rawContent:[{type:reasoning_text,text}]` | 思考 |
| `function_call` | `callId`、`name`、`arguments`（JSON 字符串） | 工具调用；`AskUserQuestion` 投影为问题卡 |
| `function_call_result` | `callId`、`status`、`output:{type:text,text}` | 工具结果，按 `callId` 配对；`status` 非 `completed` 视为错误 |
| `file-history-snapshot` | `messageId`、`snapshot` | 记账，不显示 |
| `custom-title` / `ai-title` / `topic` | `customTitle` / `aiTitle` / `topic` | 标题：`custom-title` 优先于 `ai-title`，再取首条真实用户消息 |

每条投影事件携带 `native_id`＝记录 `id`，SDK 视图用它在历史和实时投影之间对齐。
未列出的记录类型进入"跳过未知 CodeBuddy 记录类型"警告，内容不丢弃也不猜测。
本版本没有观察到独立的中断、压缩或错误记录类型，未核实的类别保持未知。

## 读模型、删除与能力边界

- 列表、历史、搜索和标题走通用文件管线（`sessions/index/summary/codebuddy.rs`、
  `sessions/providers/codebuddy.rs`）。摘要不推断回合状态（`turn` 不输出）。
- 删除走文件回收站：只移动该会话的 jsonl，可恢复。
- 不支持移动、克隆（混合组会列出阻碍项）和问题报告处理会话。
- 进程识别：npm 包是 Node 脚本，`node …/codebuddy --session-id|--resume <sid>` 按脚本名
  归为 `codebuddy`，会话 ID 取命令行。

## 启动与客户端

- CLI 终端：新建 `--session-id <uuid>`，恢复 `--resume <sid>`，`--model`/`--effort` 来自选择器。
  终端模式不带 `--permission-mode`，沿用用户配置。
- 模型目录：`codebuddy --help` 中 `--model` 行的 "Currently supported: (…)" 列表；
  强度 `minimal/low/medium/high/xhigh/max` 来自同一份帮助。SDK 模式使用 `initialize`
  返回的账号模型列表。
- 版本：`codebuddy --version`；最新版本查询 npm `@tencent-ai/codebuddy-code`；更新用
  `codebuddy update`。
- 节点 `etc/launcher.json` 需配置唯一的 `codebuddy` profile，`executable` 写
  `codebuddy` 的绝对路径（npm 全局安装目录不在非登录 shell 的 PATH 中）。
- 中国站账号需 `enterpriseEndpoint`（登录写入 `~/.codebuddy/settings.json`）；国际站
  `www.codebuddy.ai` 在部分网络不可达，须按节点网络核对。

## 验收

- [codebuddy_sdk_browser.py](../tests/codebuddy_sdk_browser.py)：读模型列表与历史、SDK 全流程。
- 假 CLI 证明适配链路，真实 CLI 证据见上文版本号；两者分别记录。
