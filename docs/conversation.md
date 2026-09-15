# 会话草稿与 SEND

问题报告和普通会话共用服务端 conversation 服务。每个逻辑会话只有一个编辑草稿，保存文本、引用、附件元数据和提交 ID；修订号通过 CAS 防止页面互相覆盖。草稿位于私有 state/conversations 目录，原生 UID、经验证的启动记录和恢复实例关联到同一身份，不按目录或时间猜测、不拼接不同会话。

新建会话默认进入对话页，ptyhost 在后端启动，用户可切换终端。SSH/shell 没有对话归档：PTY 在输入框上方，输入框只有文字和发送（没有附件按钮和 Esc），控制台是纯终端。粘贴/选择附件不上传，仅保留浏览器 RAM 中的 File 和服务端元数据。点击发送才检查现有 512 MiB 限制，流式上传私有暂存区，显示进度并允许取消；成功上传后先保存引用，再通过检查过的文件写入边界发布到会话 cwd 的 sessiondock_attachments。报告的目标 cwd 是仓库。尚未上传的字节无法跨刷新恢复，离开页面会提示重新选择；不伪装成已保存。

CLI 忙碌时直接 SEND，由 CLI 管理后续消息。选择题、trust、更新菜单或审批阻止发送，前端禁用发送，服务端独立检查并报错，保留输入；回答走终端键盘路径。SEND 成功清除提交时的草稿修订，不等待 JSONL 确认，不产生暂存消息 stack。发送期间的新编辑保留。稳定提交 ID 防止响应丢失后重复粘贴；已开始写入但结果不明的请求拒绝自动重发。

打开或切换会话只读取草稿，不触发保存。页面按稳定提交 ID 查询服务端发送结果，包括后台发送的报告首条任务；查询期间的新编辑及未上传 File 保留。服务端在完成发送、保存和启动恢复时，用提交 ID 与回执摘要核验待发送副本，仅清除已发送的文字、附件和引用，阻止旧页面将其写回；后续新增内容保留，不建立已发送正文归档。

报告使用临时草稿身份，提交后绑定到普通处理会话的同一个草稿。诊断包按报告请求 ID 冻结一次，第一条任务经共用 SEND 交给普通配置的 CLI；诊断文字仍是数据。启动/发送失败保留草稿，退出且未绑定的 CLI 可重新启动，报告的实例关联和提交状态同步更新。附件移除或取消一键完成；上传成功发布后删除私有字节副本，无引用的暂存与中断残件超过 24 小时回收，保留历史文件与不确定写入所引用的附件。

旧客户端记录按可验证 UID 导入；旧 File 原件先复制到私有服务端，再删除已验证的浏览器副本。成功消息归档不恢复到草稿，多个旧副本不合并。无法确认身份的数据及未迁移 File 保留原件。旧服务端账本保留，停止后台派发，新 outbox 投影返回空列表及只读 legacy_delivery。

## HTTP

| 接口 | 行为 |
| --- | --- |
| GET/POST /api/session/conversation | 读取草稿 / `{uid, revision, value}` CAS 保存 |
| GET 同接口带 request_id / report_request_id / legacy=true | 查询 SEND / 报告结果 / 只读迁移证据 |
| POST /api/session/conversation/attachment | `{uid, id, name}` 查询参数，原始流式文件体；成功返回 upload_id |
| POST /api/session/conversation/check | 检查当前 CLI 选择状态和终端所有权 |
| POST /api/session/conversation/send | `{uid, request_id, text, attachments, quotes, draft_revision, lease}` |
| POST /api/session/conversation/restart | 已退出、未绑定的实例重新启动，保留逻辑草稿 |
| POST /api/session/conversation/import | 只读迁移旧版输入证据 |
| GET /api/session/conversation/drafts | 发现保留输入的会话，包括退出实例 |

状态：sent 表示 SEND 已成功；cli_question 拒绝选择题期间发送；draft_revision 表示编辑冲突；send_result_unknown 不授权重发。需要 state、terminal、runtime、lifecycle、files_write 服务，conversation_send 能力明确启用。测试使用私有目录和假 CLI。
