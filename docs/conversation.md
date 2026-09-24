# 会话草稿与 SEND

问题报告和普通会话共用服务端 conversation 服务。每个逻辑会话只有一个编辑草稿，保存文本、引用、附件元数据和提交 ID；修订号通过 CAS 防止页面互相覆盖。草稿位于私有 state/conversations 目录，原生 UID、经验证的启动记录和恢复实例关联到同一身份，不按目录或时间猜测、不拼接不同会话。

新建会话默认进入对话页，ptyhost 在后端启动，用户可切换终端。SSH/shell 没有对话归档：PTY 在输入框上方，输入框只有文字和发送（没有附件按钮和 Esc），控制台是纯终端；从输入框发出的命令同样清空服务端草稿，退出的控制台不会以"草稿保留"行回到侧栏。粘贴/选择附件先保存元数据，随即在后台把字节流式上传到私有暂存区（每个草稿最多两路并发，卡片显示排队/进度，允许取消，失败的卡片保留 File 并提供重试）；512 MiB 限制在选择和上传时都检查。发布到会话 cwd 的 sessiondock_attachments 仍等到发送：SEND 先等待进行中的上传，只对失败的附件重新上传，再通过检查过的文件写入边界发布。报告的目标 cwd 是仓库。已暂存的引用可跨刷新和跨设备发送；只有上传未完成或失败的字节还只在浏览器里，离开页面才会提示。从服务端读到草稿的页面只有附件元数据，没有 File：图片卡片按需从暂存区读回字节显示预览，读不到（已发布、已回收）或超过 32 MiB 时显示类型图标，不留破图。移除附件先保存去掉引用的草稿，再尽力丢弃暂存字节；服务端拒绝丢弃仍被草稿或未完成发送引用、或已发布的上传。

CLI 忙碌时直接 SEND，由 CLI 管理后续消息。选择题、trust、更新菜单或审批阻止发送，前端禁用发送，服务端独立检查并报错，保留输入；回答走终端键盘路径。画面检测同时识别编号菜单（`❯ 1. Yes`）和 Claude Code 2.1 起的无编号菜单（`❯ No, exit` 加同列缩进的其余选项），二者都以 `Enter to confirm/select/continue` 页脚结尾；工作区 trust 对话框默认选中 “No, exit”，没有 CLI 参数可以跳过它，放行 SEND 的 Enter 会直接让 CLI 退出。检测读取的是 host 的 styled 屏幕捕获：vt100 把空白格写成光标前移 `ESC[nC` 而不是空格，去除转义时必须把它们还原成空格，否则菜单行只剩 `❯No,exit`，列对齐判断失效并放行 SEND。SEND 成功清除提交时的草稿修订，不等待 JSONL 确认，不产生暂存消息 stack。发送期间的新编辑保留。稳定提交 ID 防止响应丢失后重复粘贴；已开始写入但结果不明的请求拒绝自动重发。

打开或切换会话只读取草稿，不触发保存。多设备同步的模型是"正在编辑的设备赢，空闲设备跟随"：输入框在首次读取返回前禁用；从未保存过的页面在首次读取时采纳服务端修订号（若仍有读取前进入的本地内容，排在服务端内容之后并入并保存），不会因修订号停在 0 而被反复拒绝；保存遇到 `draft_revision` 时重读一次服务端草稿、换成其修订号后重发（最多两次），只有仍失败才显示错误。空闲页面（没有未保存编辑、没有排队保存）通过 `check` 轮询带回的 `draft_revision`、切回会话、页面重新可见和窗口聚焦跟随更新，采纳时保留本页仍持有的附件 File、预览和进行中的上传。不做字段级合并。页面按稳定提交 ID 查询服务端发送结果，包括后台发送的报告首条任务；查询期间的新编辑及未上传 File 保留。服务端在完成发送、保存和启动恢复时，用提交 ID 与回执摘要核验待发送副本，仅清除已发送的文字、附件和引用，阻止旧页面将其写回；后续新增内容保留，不建立已发送正文归档。

报告使用临时草稿身份，提交后绑定到普通处理会话的同一个草稿。诊断包按报告请求 ID 冻结一次，第一条任务经共用 SEND 交给普通配置的 CLI；诊断文字仍是数据。启动/发送失败保留草稿，退出且未绑定的 CLI 可重新启动，报告的实例关联和提交状态同步更新。附件移除或取消一键完成；上传成功发布后删除私有字节副本，无引用的暂存与中断残件超过 24 小时回收，保留历史文件与不确定写入所引用的附件。

旧客户端记录按可验证 UID 导入；旧 File 原件先复制到私有服务端，再删除已验证的浏览器副本。成功消息归档不恢复到草稿，多个旧副本不合并。无法确认身份的数据及未迁移 File 保留原件。旧服务端账本保留，停止后台派发，新 outbox 投影返回空列表及只读 legacy_delivery。

SEND 和 `check` 使用同一 PTY 编辑区分类器，返回 `ready / starting / blocked / unknown` 及原因，前端直接用于按钮和提示。原生历史/hook 问题保留问答展示，不再独立否决 SEND；身份、草稿和去重检查仍独立。识别、短暂等待和兼容边界见[对话输入就绪](composer-input.md)。

会话页的 CHECK、SEND 和回答不依赖浏览器 PTY 控制权，也不抢占已打开的终端。服务端仍核验完整会话与实例身份，并在每次 host 操作时串行写入。只有用户主动进入 PTY 模式且终端由其他页面持有时，才询问是否接管。

## HTTP

| 接口 | 行为 |
| --- | --- |
| GET/POST /api/session/conversation | 读取草稿 / `{uid, revision, value}` CAS 保存 |
| GET 同接口带 request_id / report_request_id / legacy=true | 查询 SEND / 报告结果 / 只读迁移证据 |
| POST /api/session/conversation/attachment | `{uid, id, name}` 查询参数，原始流式文件体；成功返回 upload_id |
| GET /api/session/conversation/attachment | `{uid, id}` 读回暂存字节；图片类型原样下发，其余按不透明字节，一律 `nosniff` 加沙箱 CSP；已发布或已回收的上传返回 404 |
| POST /api/session/conversation/attachment/discard | `{uid, id}` 丢弃未发布且无引用的暂存上传；返回 `removed`，被引用或已发布返回 409 |
| POST /api/session/conversation/check | 检查 PTY 输入状态；画面检查结果带 `input` 和 `draft_revision`，供前端展示及空闲草稿同步 |
| POST /api/session/conversation/send | `{uid, request_id, text, attachments, quotes, draft_revision, lease}` |
| POST /api/session/conversation/restart | 已退出、未绑定的实例重新启动，保留逻辑草稿 |
| POST /api/session/conversation/import | 只读迁移旧版输入证据 |
| GET /api/session/conversation/drafts | 发现保留输入的会话，包括退出实例；`term/discard` 删除的回执随之清除草稿，不再列出。原生 UID 别名只有在目录里仍有该会话时才挡住草稿清理；已进回收站的会话不算共享。缺少 `session.started` 的旧草稿按账本记录的创建时间回填，侧栏行不随渲染时钟移动 |

状态：sent 表示 SEND 已成功；cli_question 拒绝选择题期间发送；draft_revision 表示编辑冲突；send_result_unknown 不授权重发。需要 state、terminal、runtime、lifecycle、files_write 服务，conversation_send 能力明确启用。测试使用私有目录和假 CLI。

草稿首次读取失败和保存失败会在网络可用时自动重试；读取与保存都有请求期限，避免永久等待。读取恢复后先合并服务端草稿与本页早期输入，再保存，保留附件与引用。失败提示只显示当前错误，不递归叠加；后台恢复仅重试草稿读写，不自动 SEND。
