# 对话输入就绪

父合同：[会话草稿与 SEND](conversation.md)。本文只定义 AI 对话输入是否就绪：以当前 PTY 画面为唯一判定源。`POST /api/session/conversation/check` 与 `POST /api/session/conversation/send` 使用同一分类器。终端传输所有权、会话身份、草稿持久化与提交去重仍由父合同及其他合同处理，本文不改写、不合并。

画面截图启发式不是原生 CLI 的 ready/ack 协议。屏幕捕获与终端写入不是原子操作。SEND 成功字段 `sent` 只表示终端写入完成，不证明模型已接收。

## 状态

分类结果为 `ready`、`starting`、`blocked`、`unknown` 之一。CHECK 响应的 `input` 给出该状态及 `code`、`message`。

| 状态 | 含义 | 输入 |
| --- | --- | --- |
| `ready` | 已识别的空编辑区，包括 CLI 忙碌时 | 允许 |
| `starting` | 空启动、画面滞后、正在粘贴等瞬时过程 | 不允许；实际 SEND 可短暂等待并再检 |
| `blocked` | 已知菜单（优先于任何残留编辑区），或 Claude/Codex 编辑区里已有文字 | 不允许 |
| `unknown` | 登录页或未识别画面 | 不允许；保留草稿，提示改用 PTY |

已识别的空编辑区在 CLI 忙碌时仍为 `ready`。已知菜单优先于残留编辑框。不能以「没有选择题」推断可输入。

Claude/Codex 编辑区里已有非 dim 文字（用户在 PTY 里打的字，或 CLI 退回的上一条）时为 `blocked`：粘贴会接在这段文字后面，与它一起作为一条消息提交（BUG-20260928-235840-93d732）。与 Python 的 `draft_conflict` 一样拒发，但不代为清空，由用户在 PTY 发送或清空。dim 的占位提示（`Try "…"`、`Press up to edit queued messages`）不算文字。Claude Code 2.1.284 实测：回车后、模型尚无任何输出时按 Esc，Claude 中断并把正文放回编辑区，原生 user 记录保留；已排进 CLI 队列或已开始调工具后按 Esc 不会退回。编辑区文字与本会话最近一条被回显退掉的发送（或仍在排队的发送）忽略空白后相同时，码细化为 `cli_input_returned`，对话里这条用户气泡标注"已被 Esc 退回终端输入框，CLI 未处理"。SEND 粘贴后的再检不受此规则限制，那时编辑区里就是本条消息。

## CHECK

- `ready`：HTTP 200，`ok` 为 true；按父合同带回 `draft_revision`，供空闲页面跟随。
- 其余状态：HTTP 409，`ok` 为 false，并带父合同已有的顶层 `code`/`error` 以及 `draft_revision`。
- 顶层码与 `input.code` 一致：`cli_starting`（空屏）、`cli_catching_up`（捕获滞后）、`cli_pasting`（粘贴中）、`cli_question`（已知菜单）、`cli_input_pending`（编辑区已有文字）、`cli_input_returned`（编辑区里是被 Esc 退回的上一条，仅 CHECK 与 CLI 状态对象给出，SEND 否决仍为 `cli_input_pending`）、`cli_not_ready`（未知画面）。身份或所有权等错误仍返回原有错误响应，不伪造画面分类。

SEND 使用同一分类器与同一否决。`starting` 在 CHECK 上仍是非 ready；真正执行 SEND 时每个检查点最多等待 3 秒，每 100ms 再检。Claude/Codex 粘贴后等待编辑区文字变化，且新文字包含消息末尾或 CLI 的多行粘贴折叠占位符；该画面连续 200ms 未再变化、没有粘贴提示或画面滞后时立即发送 Enter。与 Python 一致，这一等待只是尽力而为：3 秒内仍未确认（CLI 重绘慢）也照常发送 Enter，只有期间出现的选择菜单会否决 Enter。粘贴已写入却拒发 Enter 会把消息留在 CLI 编辑区，且同一提交 ID 的重试都被拒绝（BUG-20260927-112827-5aa96e）。Grok 和 OpenCode 尚无可用的编辑区正文提取，仍保留 600ms 最短间隔和画面就绪再检。OpenCode 的原生用户消息来自服务端的镜像（[OpenCode](opencode.md)），与其他 CLI 一样按 `echo_hash` 等待对话回显。

## UI 与否决边界

CHECK 从同一次当前 PTY 捕获返回可回答的 `prompt`，不依赖原生 JSONL 或 hooks 已经出现。Claude/Codex 原有目录信任使用 `kind: folder_trust`；四个客户端的其他菜单使用 `kind: screen_menu`。composer 复用历史问题的题卡样式，保留原生目录、命令、审批范围、警告与选项说明。普通选择、多选、当前页文本输入、下一页、返回、检查答案和提交分别提供原生操作，不自动作答或记住授权默认值。多选按钮同步原生勾选状态，另点 Submit/Next 才提交；不是把网页上选择的数字一次性灌进 CLI。

`screen_menu` 的 `questions` 只描述当前可见页；选项携带 `keys`，多选另带 `toggle`/`selected`，不可选项没有可执行按键。`actions` 为原生页内操作，`cancel_keys` 只在语义已核对时给出；原生“保存并关闭”显示其真实作用。可编辑的 `text` 带 `before_keys`、`after_keys`、`mode` 与原生当前值；只有源代码证明能替换原有文字，或原生输入确实为空时才提供。密钥、凭据、无法区分秘密字段的 MCP 文本和任意快捷键捕获保留原生终端路径。

点击前重新 CHECK 同一语义 ID，再按新画面的焦点计算按键；`revision` 区分勾选、输入或页内状态变化。显示后菜单已消失、切换会话或目标选项已变化时不写入。每次操作在再检前固定终端实例身份，通过 `/api/term/send` 的所有权与实例校验写入；文本操作的多次写入都使用同一身份。部分写入不自动重试。题卡输入独立于消息草稿，轮询不会清空正在填写的答案；问题离开画面后题卡撤下，输入状态仍由 CHECK 决定。

导航或纯筛选写入后再观察一次即可继续操作，方向键在列表边界没有改变画面时也不锁住整张题卡；不自动重发按键。提交、授权和多选切换仍等待原生状态变化。

源码清单与浏览器画面分别记录已支持和原生回退项；它们是对应版本的审计记录，不宣称未来版本的所有菜单都能被画面启发式识别。所有 CLI 必须使用同一识别结果否决 SEND，不能只加网页按钮而把底层菜单当成空编辑区。

| 客户端 | 源码/二进制审计清单 | 画面夹具 |
| --- | --- | --- |
| Claude | [cli_menu_inventory_claude.json](../tests/fixtures/cli_menu_inventory_claude.json) | [cli_menus_claude.json](../tests/fixtures/cli_menus_claude.json) |
| Codex | [cli_menu_inventory_codex.json](../tests/fixtures/cli_menu_inventory_codex.json) | [cli_menus_codex.json](../tests/fixtures/cli_menus_codex.json) |
| Grok | [cli_menu_inventory_grok.json](../tests/fixtures/cli_menu_inventory_grok.json) | [cli_menus_grok.json](../tests/fixtures/cli_menus_grok.json) |
| OpenCode | [cli_menu_inventory_opencode.json](../tests/fixtures/cli_menu_inventory_opencode.json) | [cli_menus_opencode.json](../tests/fixtures/cli_menus_opencode.json) |

逐项点击、输入与原生按键验证见 [cli_menus_browser.py](../tests/cli_menus_browser.py)；原有目录信任回归见 [startup_question_browser.py](../tests/startup_question_browser.py) 与 [startup_claude_browser.py](../tests/startup_claude_browser.py)。

发送按钮的可用性和原因文案以服务端 `input` 为准，浏览器不另做一套画面分类。原生历史或 hook 中的过期问题仍是展示与回答数据，不是独立的 SEND 否决。回答走终端键盘路径。Shell/SSH 与直接 PTY 键盘/回答控件保持原始输入语义，不经本分类器否决。

对话页在打开、切换回来及重新聚焦时检查输入状态，可见期间每 1.5 秒再检。非 ready 的原因持续显示在输入框旁，草稿仍可编辑；恢复 ready 后自动撤掉提示并启用发送，不自动提交。回车遵守同一就绪状态，受阻时保留行内提示，不另弹发送失败框。CHECK 超过 5 秒显示检查超时并继续轮询；草稿同步不阻塞状态检查。

用户主动切换到终端时，当前已存在的题卡记为已展示；后台终端列表刷新不能再因同一题卡关闭终端。新问题 ID 仍可自动展示对话，用户随后切回终端的选择同样保留。这只控制题卡展示，不代表回答或确认了 CLI 问题。

窄屏软键盘只压缩网页可视区域（`interactive-widget=resizes-content` 与 `--visual-viewport-height`），不改变 PTY 行列。把键盘高度 SIGWINCH 进 CLI 会挤掉编辑区，CHECK/SEND 变成 `cli_not_ready`；收起键盘后画面恢复只是又一次重排。行列始终按键盘收起时的布局测量，因此键盘开着时的界面缩放仍会让 PTY 行列跟着变。

纯终端布局或父容器隐藏输入框时停止 CHECK，切回可见输入框立即再检；草稿保存队列仍照常完成。

## 再检与写入

在附件发布到会话 cwd 之前、粘贴之前、以及发出 Enter 之前，必须再检。粘贴出错后，不得对结果不明的写入自动重试（父合同 `send_result_unknown`）。

## 识别摘要

Claude/Codex 按父合同复用各自 composer 识别。Grok 匹配框式编辑区结构、框内光标、以及非空页脚标签；不依赖特定模型名子串。OpenCode（1.18.32 与 2.0.18 编辑区布局相同）匹配只有左边框 `┃` 的连续行块、其下 `╹▀` 底边、块内最后一行的「agent · 模型」标签，且光标在标签行之上的块内；命令面板和对话框会把光标移出，补全弹层左右都有边框，均不算编辑区。四个客户端的可识别菜单优先归为 `blocked`（`cli_question`）。OpenCode 2.0.18 的权限、表单与模态选择同时核对 SGR 样式：`●` 是配置值，不能当作键盘焦点；从同一次 styled 捕获推算当前焦点，避免误把“允许一次”按成“始终允许”。未识别或证据不完整的界面保留 `unknown`/原生 PTY 路径。

Codex 多行编辑区可以持续隐藏页脚，光标停在下一空行；不能据此判断仍在粘贴，也不等待状态栏恢复。SEND 仍须确认消息末尾或折叠占位符已经出现，并连续稳定 200ms；编辑区中的空段落不会截断识别。当前状态栏的 `tab to queue message … 100% context left` 与旧版 `Context … used / Ready` 都作为编辑区的边界，而非消息正文。

Codex 的「模型 · Context … used · Main […]」状态栏在手机窄终端中可能折行，仍按完整状态栏定位编辑区；不能把折行后的末行当成未知正文而丢失输入就绪和后台活动状态。普通输出跟在状态栏之后时，不据此恢复历史编辑区。

## 测试矩阵

Agy 1.2.16 已实现正向编辑区识别：上下相同的完整 `─` 横线、`> ` 首行、
缩进续行与区内光标，底部最多一条非空页脚；不按模型名识别。非空编辑区
返回 `cli_input_pending` 保留终端正文，SEND 复用粘贴后的正文稳定再检与原生 user
镜像回显。菜单投影支持 model 单选/取消、workspace trust 信任/退出、permissions
scope 三种范围选择，沿用 `screen_menu` 与当前焦点的 Up/Down/Enter；model/scope
可 Escape，trust 不提供猜测的取消键。已知菜单先拒发；未知菜单（如 permissions
深层编辑与未投影的 `/resume`）回退 native terminal，编辑区未识别时仍为
`unknown` 并拦截 SEND。这不表示已支持全部审批/表单或整个权限编辑器。
[Agy](agy.md) 区分真实 CLI 证据与合成字段验证；
[agy_real_browser.py](../tests/agy_real_browser.py) 为显式 operator-only 合成 gateway
路径，已验证停止恢复与继续发送，不使用账号凭据或付费模型。

下列文件覆盖本分类器与再检路径；此处不声称测试已通过或已部署。

| 范围 | 文件 |
| --- | --- |
| 画面夹具 | [composer_input_frames.json](../tests/fixtures/composer_input_frames.json) |
| 合同脚本 | [composer_input_contract.mjs](../tests/composer_input_contract.mjs) |
| 登录/未知拒绝、恢复、粘贴后再检、不明写入不重试、软键盘不改 PTY 行列 | [send_readiness_browser.py](../tests/send_readiness_browser.py) |
| Codex 两张图片与文字、`.txt` 与文字连续发送 | [send_codex_attachments_browser.py](../tests/send_codex_attachments_browser.py) |
| 忙碌发送、选择题拒绝、草稿与 SEND、编辑区已有文字与 Esc 退回时拒发 | [send_browser.py](../tests/send_browser.py) |
