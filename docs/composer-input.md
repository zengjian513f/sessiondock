# 对话输入就绪

父合同：[会话草稿与 SEND](conversation.md)。本文只定义 AI 对话输入是否就绪：以当前 PTY 画面为唯一判定源。`POST /api/session/conversation/check` 与 `POST /api/session/conversation/send` 使用同一分类器。终端传输所有权、会话身份、草稿持久化与提交去重仍由父合同及其他合同处理，本文不改写、不合并。

画面截图启发式不是原生 CLI 的 ready/ack 协议。屏幕捕获与终端写入不是原子操作。SEND 成功字段 `sent` 只表示终端写入完成，不证明模型已接收。

## 状态

分类结果为 `ready`、`starting`、`blocked`、`unknown` 之一。CHECK 响应的 `input` 给出该状态及 `code`、`message`。

| 状态 | 含义 | 输入 |
| --- | --- | --- |
| `ready` | 已识别编辑区，包括忙碌或框内已有文字 | 允许 |
| `starting` | 空启动、画面滞后、正在粘贴等瞬时过程 | 不允许；实际 SEND 可短暂等待并再检 |
| `blocked` | 已知菜单；优先于任何残留编辑区 | 不允许 |
| `unknown` | 登录页或未识别画面 | 不允许；保留草稿，提示改用 PTY |

已识别的编辑区在 CLI 忙碌或已有文字时仍为 `ready`。已知菜单优先于残留编辑框。不能以「没有选择题」推断可输入。

## CHECK

- `ready`：HTTP 200，`ok` 为 true；按父合同带回 `draft_revision`，供空闲页面跟随。
- 其余状态：HTTP 409，`ok` 为 false，并带父合同已有的顶层 `code`/`error` 以及 `draft_revision`。
- 顶层码与 `input.code` 一致：`cli_starting`（空屏）、`cli_catching_up`（捕获滞后）、`cli_pasting`（粘贴中）、`cli_question`（已知菜单）、`cli_not_ready`（未知画面）。身份或所有权等错误仍返回原有错误响应，不伪造画面分类。

SEND 使用同一分类器与同一否决。`starting` 在 CHECK 上仍是非 ready；真正执行 SEND 时每个检查点最多等待 3 秒，每 100ms 再检。Claude/Codex 粘贴后必须看到编辑区文字变化，且新文字包含消息末尾或 CLI 的多行粘贴折叠占位符；该画面还必须连续 200ms 未再变化，没有粘贴提示或画面滞后，才发送 Enter；超时保留输入。Grok 尚无可用的编辑区正文提取，仍保留 600ms 最短间隔和画面就绪再检。

## UI 与否决边界

发送按钮的可用性和原因文案以服务端 `input` 为准，浏览器不另做一套画面分类。原生历史或 hook 中的过期问题仍是展示与回答数据，不是独立的 SEND 否决。回答走终端键盘路径。Shell/SSH 与直接 PTY 键盘/回答控件保持原始输入语义，不经本分类器否决。

窄屏软键盘只压缩网页可视区域（`interactive-widget=resizes-content` 与 `--visual-viewport-height`），不改变 PTY 行列。把键盘高度 SIGWINCH 进 CLI 会挤掉编辑区，CHECK/SEND 变成 `cli_not_ready`；收起键盘后画面恢复只是又一次重排。

## 再检与写入

在附件发布到会话 cwd 之前、粘贴之前、以及发出 Enter 之前，必须再检。粘贴出错后，不得对结果不明的写入自动重试（父合同 `send_result_unknown`）。

## 识别摘要

Claude/Codex 按父合同复用各自 composer 识别。Grok 匹配框式编辑区结构、框内光标、以及非空页脚标签；不依赖特定模型名子串。CLI 布局变化导致无法识别时，状态为 `unknown`，须用 PTY。

Codex 多行粘贴时可能先画出无页脚的编辑区，光标停在下一空行；此时仍是 `starting`，SEND 等待状态栏恢复。当前状态栏的 `tab to queue message … 100% context left` 与旧版 `Context … used / Ready` 都作为编辑区的边界，而非消息正文。

## 测试矩阵

下列文件覆盖本分类器与再检路径；此处不声称测试已通过或已部署。

| 范围 | 文件 |
| --- | --- |
| 画面夹具 | [composer_input_frames.json](../tests/fixtures/composer_input_frames.json) |
| 合同脚本 | [composer_input_contract.mjs](../tests/composer_input_contract.mjs) |
| 登录/未知拒绝、恢复、粘贴后再检、不明写入不重试、软键盘不改 PTY 行列 | [send_readiness_browser.py](../tests/send_readiness_browser.py) |
| Codex 两张图片与文字、`.txt` 与文字连续发送 | [send_codex_attachments_browser.py](../tests/send_codex_attachments_browser.py) |
| 忙碌发送、选择题拒绝、草稿与 SEND | [send_browser.py](../tests/send_browser.py) |
