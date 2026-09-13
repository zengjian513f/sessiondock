# HTTP error codes

This file is produced by `tests/error_codes.py`. Handlers return JSON `{"error": "<message>", "code": "<code>"}`. Status **501** means the route or capability is declared not implemented in this migration stage. Session errors use `unsupported_history` at 501 and `session_error` otherwise. Regenerate:

```sh
python3 tests/error_codes.py --write
```

Scanned `crates/sessiondock/src`: **209** (status, code) pairs.

## 400 Bad Request

### `backend_unknown`

- 未知终端后端 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L668 → `POST /api/term/backend`

### `backend_unsupported`

- Rust 后端不支持 tmux；新建会话只能由 ptyhost 托管 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L663 → `POST /api/term/backend`

### `bad_body`

- bad body
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report` L110, L114 → `POST /api/bug-report`
- [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `bad_body` L238

### `delivery_media_unsupported`

- 此后端尚不能解析上传附件；请去掉附件后再发送 — [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `send` L303 → `POST /api/session/send`

### `file_absolute_path_required`

- 目录导航需要绝对路径 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `absolute_navigation` L501

### `file_action_invalid`

- 未知文件操作 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L450

### `file_attachment_id`

- 附件目录编号无效 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1550

### `file_conflict_invalid`

- 无效的重名处理方式 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `parse` L98

### `file_conflict_replace_unsupported`

- 写入服务从不覆盖既有项目；请选择停止、跳过或保留两份 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `parse` L93

### `file_cwd_unavailable`

- 相对引用需要有效的所选会话工作目录；不会使用服务进程目录 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `absolute_path` L516

### `file_directory_anchor_required`

- 文件操作入口必须是会话提及的目录
- 文件浏览入口必须是会话提及的目录
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `anchor` L444
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `target` L331
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L427

### `file_directory_required`

- 此操作需要目录
- 请选择目录浏览
- 目标必须是目录
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `directory` L316
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `list` L557
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `directory` L373

### `file_field_required`

- 缺少必需字段 {field} — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `required` L1029

### `file_foreign_path`

- 当前平台不接受 Windows 分隔符或驱动器路径 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `validate_path_text` L419

### `file_image_required`

- 图片引用必须指向普通文件，不能指向目录 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `new` L88

### `file_invalid_pdf`

- 文件没有有效的 PDF 标识，请下载后检查 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `validate_pdf` L190

### `file_job_invalid`

- 无效的任务编号 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `find` L266

### `file_list_options`

- 无效的目录分页或排序参数 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `list` L549

### `file_media_reference_invalid`

- 媒体文件引用需要本地路径或无 authority 的 file:/// URL；不展开 HOME、网络地址或控制字符 — [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `normalize_media_ref` L42

### `file_move_into_self`

- 不能把目录放进自身或子目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L631

### `file_name_invalid`

- 名称无效：须为单个路径组件，不能包含 /、\、控制字符或为 . 与 .. — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `validate_name` L1040

### `file_parent_not_directory`

- 路径的父组件不是目录
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L350
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `entry` L401

### `file_path_encoding`

- 开发文件目录必须能表示为 UTF-8 路径
- 路径无法表示为 UTF-8
- 目录含非 UTF-8 名称，不能安全导航
- 路径必须是 UTF-8
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `list` L575
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L140
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `wire_path` L527
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1575
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `delete` L686
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L620

### `file_path_invalid`

- 开发文件目录需要标准绝对路径
- 路径组件无效
- 路径为空、过长或含不支持的字符；不展开 HOME 或 URL
- Windows 仅接受显式盘符绝对路径，不接受 UNC/设备/隐式当前盘路径
- 不接受 Windows 数据流或歧义路径组件
- 不接受 Windows 保留设备名称
- 写入路径不接受 . 或 .. 组件
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L161
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L334
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `validate_path_text` L409, L428, L441, L457
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1571
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `entry` L395
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `normalized` L325

### `file_paths_required`

- 请先选择项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `items` L456

### `file_preview_unsupported`

- 此格式请使用文本预览或下载 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `read` L408

### `file_reference_invalid`

- 文件引用为空、过长或包含控制字符
- 文件引用不能为空
- [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `clean_ref` L119, L129

### `file_reference_limit`

- 一次最多解析 256 个文件引用 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_many` L356

### `file_required`

- 此操作需要普通文件
- 目录只提供有界列表，不能作为文件预览或下载
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `file` L278
- [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `read` L352
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1306
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1241

### `file_reserved_name`

- 以 .sessiondock- 开头的名称保留给上传暂存目录
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `normalized` L346
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `validate_name` L1065

### `file_root_not_directory`

- 开发文件授权入口必须是既有目录 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L178

### `file_roots_overlap`

- 开发文件目录不能重叠或重复 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `open` L146

### `file_roots_required`

- 须显式配置 1 至 16 个既有独立开发文件目录 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `open` L134

### `file_same_path`

- 源路径和目标相同
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L638
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `rename` L576

### `file_scope_invalid`

- 文件访问需要已解析的有效会话视图 — [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `validate_scope` L145

### `file_sha256_invalid`

- sha256 须为 64 位十六进制 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `parse_sha256` L1082

### `file_single_item_required`

- 请选择一个项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `rename` L565

### `file_trash_overlap`

- 回收目录所在的状态目录不能与写入目录重叠 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `open` L227

### `file_upload_content_type`

- 上传分块必须使用 application/octet-stream — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L532 → `POST /api/session/files/upload`

### `file_upload_empty`

- 附件为空或缺少 Content-Length — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1530

### `file_upload_modified_invalid`

- 无效的修改时间 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L782

### `file_upload_offset_invalid`

- offset 必须是非负整数 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L520 → `POST /api/session/files/upload`

### `file_upload_size_invalid`

- 无效的上传大小 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L768

### `file_write_limits_invalid`

- 写入预算必须为正数 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `open` L200

### `file_write_roots_overlap`

- 写入目录不能重叠或重复 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `open` L212

### `file_write_roots_required`

- 须显式配置 1 至 16 个既有独立写入目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `open` L193

### `invalid_attach`

- 终端连接参数无效
- 终端连接标识无效
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L179, L232 → `GET /api/term/attach`

### `invalid_audit_request`

- (dynamic) — [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L98 → `POST /api/audit/browser`

### `invalid_bug_report`

- (dynamic) — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `invalid` L82

### `invalid_claim`

- 终端预约请求格式无效
- 终端预约诊断字段过长
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim` L83, L93 → `POST /api/term/claim`

### `invalid_cursor`

- 回收站分页游标无效 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `list` L322 → `GET /api/trash`

### `invalid_file_request`

- 需要有效的会话、分支和文件参数 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `invalid` L47

### `invalid_launch_request`

- 创建请求格式或身份字段无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `invalid` L99

### `invalid_limit`

- limit 必须在 1 到 {LIST_LIMIT} 之间 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `list` L313 → `GET /api/trash`

### `invalid_metadata_batch`

- 需要 1 至 1000 个有效会话 uid — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `visibility` L193 → `POST /api/sessions/fork-visibility`

### `invalid_metadata_request`

- 偏好请求诊断字段过长
- 需要有效的偏好 JSON 请求和布尔状态
- request_id 过长
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `invalid` L109
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L248 → `POST /api/session/rewind`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `validate` L45

### `invalid_metadata_uid`

- 需要有效的会话 uid
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L232 → `POST /api/session/rewind`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `star` L153 → `POST /api/session/star`

### `invalid_outbox_query`

- 需要有效的会话 UID 和完整子代理 ID
- 发送账本查询标识无效
- [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `outbox` L96, L103 → `GET /api/session/outbox`

### `invalid_path`

- 启动目录路径过长
- 启动目录路径无效
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `complete_dir` L607, L618 → `GET /api/term/complete-dir`

### `invalid_purge`

- 需要 id/ids，或 all:true / days:N（二者不能同时给出）
- 单次最多清除 {BATCH_LIMIT} 个条目；days 不能超过 36500
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L399, L406 → `POST /api/trash/purge`

### `invalid_query`

- 无效的消息游标参数
- force 必须为 0 或 1
- 查询参数无效
- force 必须为 0/1
- [`api/read.rs`](../crates/sessiondock/src/api/read.rs) `list` L67 → `GET /api/sessions`
- [`api/read.rs`](../crates/sessiondock/src/api/read.rs) `query_error` L42
- [`api/read.rs`](../crates/sessiondock/src/api/read.rs) `validate_query` L52
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_session` L178, L180
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `list` L306 → `GET /api/trash`

### `invalid_rewind_target`

- target 必须是 1 至 256 字节的 Claude 记录节点 ID，或 null 表示取消固定 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L241 → `POST /api/session/rewind`

### `invalid_scroll`

- 终端滚动请求格式无效
- 终端滚动参数无效：lines 须在 1..100
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `scroll` L478, L489 → `POST /api/term/scroll`

### `invalid_search_query`

- (dynamic) — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L124 → `GET /api/search`

### `invalid_send`

- 会话 UID、终端名或消息 ID 无效 — [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `bounded` L281

### `invalid_stop_request`

- 停止请求格式或会话 UID 无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `invalid_stop` L778

### `invalid_terminal_input`

- (dynamic) — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `invalid_input` L340

### `invalid_trash_request`

- 请求体无效: {} — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `invalid` L64

### `invalid_uid`

- 会话 uid 无效
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_batch` L254 → `POST /api/sessions/delete`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_session` L187

### `launch_adapter`

- 来源没有唯一的可续接 CLI 配置；请明确选择已授权的版本化 adapter_id
- 来源没有唯一的已配置适配器；请明确选择已授权的版本化 adapter_id
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `select_entry` L385, L390

### `no_sessions`

- 没有选中任何会话 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_batch` L265 → `POST /api/sessions/delete`

### `session_error`

- Codex 名称索引需要显式、标准的绝对 UTF-8 文件路径
- Codex 名称索引路径不能包含相对跳转
- 子代理参数过长
- 历史页游标格式无效
- 图片分页游标格式无效
- 可靠发送只支持 Claude 主会话
- 消息视图参数过长
- append 和 window 只接受 0 或 1
- 这不是 Claude 主会话
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `refresh_within` L462
- [`sessions/index/names.rs`](../crates/sessiondock/src/sessions/index/names.rs) `new` L85, L96
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `prepare_within` L893
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L175
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_native_inputs` L367
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L494
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_message_query` L449, L454
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_request` L1356

### `too_many_sessions`

- 单次最多删除 {BATCH_LIMIT} 个会话 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_batch` L272 → `POST /api/sessions/delete`

### `websocket_required`

- 需要有效的 WebSocket 升级请求 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L249 → `GET /api/term/attach`

## 403 Forbidden

### `cross_origin`

- 拒绝跨源 API 请求 — [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L91

### `cross_site`

- 拒绝跨站 API 请求 — [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L80

### `file_forbidden`

- 文件缺少读取权限，或目录缺少访问权限
- 无法访问此文件或目录，或路径越出授权目录
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `ordinary` L89
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `io` L81

### `file_hardlink_forbidden`

- 写入服务不移动、改名或删除具有多个硬链接的文件 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `unshared` L103

### `file_move_cross_root`

- 只能在同一个写入目录内移动 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L624

### `file_outside_anchor_root`

- 目录导航不能越出入口所属的开发文件目录
- 写入路径不能越出入口所属的开发文件目录
- 该上传任务的目标不在当前入口所属目录内
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `target` L339
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `normalized` L333
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L887

### `file_outside_roots`

- 路径越出授权目录
- 路径越出文件系统根目录
- 路径不在显式授权的开发文件目录内
- 仓库目录不在写入授权目录内
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `normalized` L475
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L329
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `candidate` L167
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1561, L1568

### `file_outside_write_roots`

- 路径不在显式配置的写入目录内；只读目录不会隐式变为可写 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `root_for` L360

### `file_private_dir_invalid`

- {name} 必须是目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1134

### `file_private_dir_permissions`

- {name} 目录必须仅所有者可访问 (0700) — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1147

### `file_root_immutable`

- 不能重命名、移动或删除写入目录本身 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `entry` L386

### `file_root_too_broad`

- 文件系统根目录不能作为开发文件授权目录 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L148

### `file_special_forbidden`

- 只允许普通文件与目录，不读取设备、管道或套接字 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `ordinary` L79

### `file_symlink_forbidden`

- 开发文件服务不跟随符号链接或重解析点 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `ordinary` L72

### `hub_unsupported`

- 尚不支持旧 Hub 节点协议 — [`security.rs`](../crates/sessiondock/src/security.rs) `local_only` L62

### `local_only`

- 仅允许本地 loopback Host — [`security.rs`](../crates/sessiondock/src/security.rs) `local_only` L50

### `media_native_scope`

- 原生图片需要当前来源授权读取器
- [`media/descriptors.rs`](../crates/sessiondock/src/media/descriptors.rs) `materialize` L213
- [`media/native_media.rs`](../crates/sessiondock/src/media/native_media.rs) `materialize_native` L267

### `media_scope`

- 图片不属于当前媒体服务
- 图片需要当前所选会话授权
- 图片不属于当前所选会话
- [`media/descriptors.rs`](../crates/sessiondock/src/media/descriptors.rs) `materialize` L220, L224
- [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `authorize` L26, L47
- [`media/native_media.rs`](../crates/sessiondock/src/media/native_media.rs) `materialize_native` L264

### `node_auth_required`

- node authentication required — [`api/node_auth.rs`](../crates/sessiondock/src/api/node_auth.rs) `auth_required` L73

### `node_peer_denied`

- forbidden — [`api/node_auth.rs`](../crates/sessiondock/src/api/node_auth.rs) `peer_denied` L69

### `session_error`

- 会话输入必须是普通文件，不支持符号链接
- 会话输入路径越过已配置的数据源边界
- 会话路径或父目录已被替换为符号链接；拒绝跟随
- 会话输入路径不能经过符号链接
- 原生输入必须经过普通目录且为无链接普通文件
- 原生输入需要已验证的绝对规范路径
- 历史页不属于所选会话或子代理
- 图片分页不属于所选会话或子代理
- 文本读回范围不属于当前原生记录
- 工具解码范围不属于当前原生记录
- 工具解码范围无效
- 图片缺少当前原生来源授权
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `stamp` L1194
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `trusted_path` L1245, L1251, L1262
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `open_range` L217
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `ordinary` L128
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L185
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L87
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `prepare` L171, L175
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_source` L443

### `terminal_disabled`

- (dynamic) — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `terminal_off` L46

## 404 Not Found

### `entry_not_found`

- 回收站条目不存在 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L429 → `POST /api/trash/purge`

### `file_job_unknown`

- 任务不存在或不属于此会话 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `find` L272, L286

### `file_not_found`

- 文件不存在，或会话未记录其完整路径
- 文件或目录不存在
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `io` L80
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L305

### `file_not_referenced`

- 该路径未出现在所选会话分支中
- [`files/media.rs`](../crates/sessiondock/src/files/media.rs) `image` L116
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L235

### `launch_missing`

- 没有这个创建回执 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `failure` L95

### `media_not_found`

- 图片不存在或已过期，请重新加载会话 — [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `get` L76 → `GET /api/media/{token}`

### `not_found`

- node listener serves /api only
- API route not found
- [`api/mod.rs`](../crates/sessiondock/src/api/mod.rs) `node_not_found` L216
- [`api/mod.rs`](../crates/sessiondock/src/api/mod.rs) `not_found` L224 → `ANY (fallback)`

### `session_error`

- 会话不存在
- 子代理必须通过所属主会话访问
- 子代理不存在或不属于此主会话
- 历史页不存在或已淘汰，请重新载入会话
- 图片分页不存在或已淘汰，请重新载入会话
- 目标不是这个 Claude 会话的记录节点
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `select` L274, L283, L310
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `select_main` L491
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `physical_chain` L267
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `prepare` L972, L977, L983
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `search_version` L790, L795
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L183
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `lookup` L201
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `lookup_media` L212
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L502
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_request` L1359, L1362
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `view_meta` L1403

### `session_missing`

- 会话不存在
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L837 → `POST /api/session/stop`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L285 → `POST /api/session/rewind`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `star` L165 → `POST /api/session/star`

### `terminal_missing`

- 指定目录中没有这个终端 host — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `scroll` L496 → `POST /api/term/scroll`

## 409 Conflict

### `file_ambiguous`

- 会话中有多个同名文件，请点击完整路径 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L254, L283

### `file_attachment_dir`

- {attachment_dir} 不是安全目录
- 附件编号对应的不是安全目录
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1591, L1601

### `file_changed`

- 文件或父目录在读取期间已变化，请刷新后重试
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L187
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L359, L385
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `verify` L122, L126, L262, L267, L270
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `verify_identity` L297, L305, L307
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `changed` L72
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1339, L1346
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1143
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1254, L1257
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate_entry` L1400, L1402
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `trash` L746, L748

### `file_cwd_missing`

- 会话当前目录不存在 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1578, L1581

### `file_exists`

- 目标已存在；写入服务不会覆盖，请改名、跳过或保留两份
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `exists_error` L1107
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L796

### `file_job_not_completed`

- 只能登记已完成的上传任务 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `completed_upload` L298

### `file_job_skipped`

- 该上传因重名被跳过，没有可登记的文件 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `completed_upload` L305

### `file_keep_exhausted`

- 无法生成唯一名称
- 同名附件过多，无法分配文件名
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1720
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `keep_exhausted` L1114

### `file_move_cross_device`

- 目录不能跨文件系统移动；不会复制后删除目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate_entry` L1418

### `file_trash_cross_device`

- 项目与回收目录不在同一文件系统，不能移入回收目录；不会直接删除 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `trash` L735

### `file_upload_checksum`

- 上传数据的 SHA-256 与声明不符，已丢弃暂存数据 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `finalize` L968

### `file_upload_finished`

- 上传已结束或已取消 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L882

### `file_upload_offset`

- 重发的分块与已接收数据不一致，请刷新任务后从预期位置继续
- 上传位置不一致，请刷新任务后从预期位置继续
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L926, L934

### `launch_cwd_unknown`

- 该会话没有记录可用的工作目录；请通过创建接口明确指定白名单内的目录续接 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `takeover` L564 → `POST /api/term/takeover`

### `launch_identity`

- 创建回执与进程实例不匹配 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `check_instance` L127

### `launch_identity_declared`

- 该实例启动时已在命令行声明完整原生会话 ID；由运行时目录关联，不接受另行的操作者绑定 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `bind` L205 → `POST /api/term/bind`

### `launch_not_finished`

- 该创建实例尚未退出或取消，不能丢弃；请先停止它 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `discard` L751 → `POST /api/term/discard`

### `launch_source`

- 续接会话的数据源与请求来源不一致 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `create` L500 → `POST /api/term/create`

### `media_changed`

- 图片文件已变化，请重新加载会话 — [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `authorize` L30, L51

### `media_native_changed`

- 原生图片来源已变化，请重新加载会话 — [`media/native_media.rs`](../crates/sessiondock/src/media/native_media.rs) `changed` L171

### `run_state_unknown`

- 该会话的受管实例运行状态未知（{reason}），未发送任何停止指令；未知不等于已退出，请稍后重试或检查宿主 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L883 → `POST /api/session/stop`

### `session_error`

- Claude 子代理声明的会话 ID 与主会话不一致
- 父线程 ID 在已配置索引中存在歧义
- Codex 子代理 ID 在索引中存在歧义
- 主会话中子代理 ID 存在歧义
- 原生记录包含冲突的会话 ID
- 图片分页范围已变化，请重新载入会话
- 原生图片内容或范围已变化，请重新加载会话
- 图片原生来源已消失
- 图片原生来源已替换，请重新加载会话
- 原生图片区段无效
- 历史页对应的时间线已变化，请重新载入会话
- 历史页范围已变化，请重新载入会话
- 图片分页对应的时间线已变化，请重新载入会话
- 图片分页对应的消息已不在时间线中，请重新载入会话
- 图片分页对应的消息已变化，请重新载入会话
- 原生工具输出来源或解码范围已变化，请重试
- 原生文本来源已消失
- 原生文本区段无效
- 原生工具来源已消失
- 图片已不属于当前会话分支，请重新加载会话
- 目标不在当前 Claude 时间线上，无法固定显示
- 首条消息之前没有可固定显示的时间线
- 子代理与主会话的数据源不一致
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `native_scope` L126
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `select` L280, L303, L311
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `sid` L224
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `build` L589
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `select_main` L488
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `sid` L326
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `thread` L251
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `native_identity` L387
- [`sessions/media_projection.rs`](../crates/sessiondock/src/sessions/media_projection.rs) `project_range` L40
- [`sessions/native_media.rs`](../crates/sessiondock/src/sessions/native_media.rs) `authorized_reader` L73, L75, L100
- [`sessions/native_media.rs`](../crates/sessiondock/src/sessions/native_media.rs) `finish` L37
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `history_page` L431
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `media_page` L474, L477, L483, L489
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `validate_grant_scope` L406
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `classify` L25
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L84, L102
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `prepare` L178
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `native_identity` L80
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L505, L508
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_scope` L1587
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_source` L434
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_request` L1367

### `stale_build`

- 页面版本已过期，请刷新后再保存偏好 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `validate` L52

### `terminal_binding_unavailable`

- 无法确认会话与终端实例的唯一关联；请刷新，不会降级按名称连接。 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `binding_unavailable` L299

## 410 Gone

### `file_job_expired`

- 任务已因 {} 秒无活动而过期，暂存数据已丢弃，请重新开始 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `find` L277

### `session_error`

- 历史页已过期，请重新载入会话
- 图片分页已过期，请重新载入会话
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L189

## 413 Payload Too Large

### `body_too_large`

- 浏览器诊断请求体过大
- 缺陷报告请求体过大
- 附件记录请求体过大
- {what}请求体过大
- 文件引用请求体过大
- 文件操作请求体最多 512 KiB
- 创建请求体过大
- 停止请求体过大
- 偏好请求体超过大小限制
- 终端预约请求体过大
- 请求体过大
- [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L90 → `POST /api/audit/browser`
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `attachment` L404 → `POST /api/session/attachment`
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report` L104 → `POST /api/bug-report`
- [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `bad_body` L232
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `action` L461 → `POST /api/session/files/action`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `resolve` L138 → `POST /api/session/resolve-files`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `parse_body` L108
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L802 → `POST /api/session/stop`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `invalid` L103
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim` L77 → `POST /api/term/claim`
- [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L107

### `delivery_text_too_large`

- 消息正文超过 256 KiB — [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `send` L310 → `POST /api/session/send`

### `file_directory_budget`

- 目录超过 10000 项预算；未返回假完整列表，请选择较小目录 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `list` L567

### `file_image_budget`

- 单张磁盘图片超过 32 MiB 读取上限 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `new` L96

### `file_items_limit`

- 一次最多操作 {MAX_WRITE_ITEMS} 个项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `items` L459

### `file_job_too_large`

- 单个上传最多 {} 字节 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L770

### `file_path_depth`

- 路径超过 64 层解析预算 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `normalized` L488

### `file_probe_budget`

- 文件解析超过 20000 次路径检查预算，不能保证完整消歧 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `probe` L184

### `file_raw_budget`

- 原始打开超过 32 MiB，请使用预览或下载
- 原始打开超过 32 MiB，请使用文本预览或下载
- [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `read` L397, L415

### `file_reference_budget`

- 所选历史中的文件引用过多，无法安全完成解析
- 所选历史的文件引用解析超过 512 MiB 预算
- [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `charge` L318
- [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `insert` L251

### `file_reference_depth`

- 工具参数嵌套过深，不能完整解析文件引用 — [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `scan_value` L328

### `file_stream_budget`

- 单文件超过 16 GiB 开发传输上限 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `read` L381

### `file_upload_chunk_too_large`

- 单个分块最多 {limit} 字节
- 单个分块最多 {} 字节
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L545 → `POST /api/session/files/upload`
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L862

### `file_upload_overflow`

- 分块超过声明的上传大小 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L901

### `file_upload_too_large`

- 单个附件不能超过 {} MB — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1537

### `lifecycle_response_limit`

- 创建状态响应超过大小限制 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `response` L271

### `media_budget`

- 本次图片超过描述符数量预算；文字仍可查看 — [`sessions/media_projection.rs`](../crates/sessiondock/src/sessions/media_projection.rs) `project_ranges` L139

### `session_error`

- 子代理归属超过 32 层限制
- 分叉历史超过 32 层限制
- 逻辑历史超过 2000000 条消息预算
- 逻辑历史超过 1 GiB 消息预算
- 继承历史预算溢出
- 继承历史超过 4 GiB 原始前缀预算
- Codex 名称索引超过 4 MiB 字节预算
- Codex 名称索引超过 50000 行预算
- Codex 名称索引 id 或名称字段超过预算
- Codex 名称索引超过 10000 个 ID 预算
- 单次消息投影超过 256 张图片上限
- Grok summary.json 超过 16 MiB 元数据预算
- 原生输入超过此操作的读取或索引预算
- 原生嵌套图片超过共享读取预算
- 单条消息超过历史页预算，尚不能拆分此消息
- 历史页响应超过 8 MiB 预算
- 此历史页无法在读取预算内推进
- 此图片分页无法在读取预算内推进
- 图片分页响应超过 8 MiB 预算
- 嵌套原生工具输出超过共享读取预算
- 原生单文件超过 4 GiB 扫描预算
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `chain` L353
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `dependencies` L335
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `inherit` L662, L674, L676
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `ownership` L258
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `push` L631, L644
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `chain` L399
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `ownership` L360
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `physical_chain` L272
- [`sessions/index/names.rs`](../crates/sessiondock/src/sessions/index/names.rs) `load` L154
- [`sessions/index/names.rs`](../crates/sessiondock/src/sessions/index/names.rs) `ordinary` L67
- [`sessions/index/names.rs`](../crates/sessiondock/src/sessions/index/names.rs) `parse` L174, L192, L207
- [`sessions/media_projection.rs`](../crates/sessiondock/src/sessions/media_projection.rs) `project_ranges` L55
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `restamp` L1297
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `limit_error` L108
- [`sessions/native_media.rs`](../crates/sessiondock/src/sessions/native_media.rs) `failure` L28
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `history_page` L439
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `media_page` L494, L528
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `too_large` L290
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `validate_response` L312
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `classify` L23
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `build` L1531, L1534
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `encoded_bytes` L108
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `inherit` L1623, L1634, L1636
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `parse_candidate` L569

### `terminal_input_too_large`

- 终端输入请求体过大
- 单次终端输入不能超过 16 KiB
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `send` L350, L386, L398 → `POST /api/term/send`

### `too_many_events`

- 单次诊断批次事件过多 — [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L103 → `POST /api/audit/browser`

## 414 Uri Too Long

### `uri_too_long`

- 请求 URI 过长 — [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L97

## 429 Too Many Requests

### `file_jobs_limit`

- 最多同时进行 {} 个上传任务，请稍后重试 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `insert_uploading` L243

### `lifecycle_response_busy`

- 创建状态响应达到并发限制，请释放旧响应后重试 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `admit` L239

### `rate_limited`

- 浏览器诊断上报过于频繁 — [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L62 → `POST /api/audit/browser`

## 500 Internal Server Error

### `bug_report_failed`

- 报告捕获任务异常退出 — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report` L215 → `POST /api/bug-report`

### `file_headers_invalid`

- 文件响应头无效 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `response_body` L371

### `file_worker_failed`

- 附件写入任务异常退出
- 文件读取任务异常退出
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `attachment` L486 → `POST /api/session/attachment`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `work` L106

### `live_failed`

- 进程表配对失败 — [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `live` L178 → `GET /api/live`

### `metadata_worker_failed`

- 偏好工作异常退出，请重新读取状态确认结果 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `write` L137

### `reader_failed`

- 只读任务失败
- [`state.rs`](../crates/sessiondock/src/state.rs) `run` L178
- [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L153

### `runtime_encoding`

- 受控进程观察无法编码 — [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `live` L94 → `GET /api/live`

### `search_failed`

- 搜索任务失败 — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L174 → `GET /api/search`

### `session_error`

- 历史消息序列化失败
- 会话索引锁不可用
- 会话视图锁不可用
- 原生事件的物理范围与已提交的行边界不一致
- 消息序列化失败
- 会话行不是对象
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `push` L636
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `refresh_within` L467
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `list_state` L507
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `views` L517
- [`sessions/native_tail.rs`](../crates/sessiondock/src/sessions/native_tail.rs) `native_tail` L108
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `encoded_bytes` L100
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `view_meta` L1388

### `trash_encoding`

- 回收站结果无法编码 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `encoding` L227

### `trash_failed`

- 回收站任务失败
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `list` L334 → `GET /api/trash`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L418 → `POST /api/trash/purge`

## 501 Not Implemented

### `bug_report_disabled`

- 缺陷报告未启用：需要 SESSIONDOCK_BUG_REPORT_DIR/REPO、审计目录、终端传输、受控创建和 launcher 的 bug_report_profiles — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `disabled` L38

### `bug_report_model_policy`

- {} 处理会话的 launcher 配置没有固定为最便宜模型（{}），不会启动 — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report` L141 → `POST /api/bug-report`

### `delivery_disabled`

- 发送账本读取未启用：需要显式初始化并配置独立开发目录 — [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `outbox` L89 → `GET /api/session/outbox`

### `delivery_send_disabled`

- 可靠发送未启用：需要已初始化的发送账本目录和显式终端传输目录 — [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `executor` L172

### `delivery_source_unsupported`

- 此来源尚未实现发送账本 — [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `outbox` L133 → `GET /api/session/outbox`

### `file_action_not_implemented`

- 写入服务尚未实现此文件操作：{}；不会伪造任务或成功结果 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L442

### `file_mode_not_implemented`

- 只读文件服务尚未实现此模式；不会伪造空任务或成功结果
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `get` L256, L272
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `validate` L201
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `unsupported` L65

### `file_trash_unconfigured`

- 未配置 SESSIONDOCK_STATE_DIR，没有回收目录；写入服务不会直接删除 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `trash` L704

### `files_disabled`

- 文件读取未启用：必须显式配置独立的开发文件目录 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `configured` L55

### `files_jobs_disabled`

- 文件写入未启用：报告附件需要显式的写入目录
- 文件写入未启用：必须显式配置 SESSIONDOCK_FILE_WRITE_ROOTS（只读目录不会隐式变为可写）
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `attachment` L418 → `POST /api/session/attachment`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `write_configured` L388

### `media_files_disabled`

- 未配置图片读取目录；不会自动访问磁盘 — [`sessions/media_projection.rs`](../crates/sessiondock/src/sessions/media_projection.rs) `project_ranges` L79

### `metadata_disabled`

- 偏好保存未启用：必须显式配置独立的开发状态目录 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `configured` L93

### `not_implemented`

- Rust 后端尚未迁移此能力：{capability}。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：受控会话创建与 pending 生命周期。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：终端后端选择。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：受管实例停止。当前是只读开发阶段。
- [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L41 → `POST /api/audit/browser`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L650 → `POST /api/term/backend`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `enabled` L28
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L797, L845 → `POST /api/session/stop`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `configured` L60
- [`error.rs`](../crates/sessiondock/src/error.rs) `unavailable` L31

### `session_stop_unmanaged`

- (dynamic) — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L872 → `POST /api/session/stop`

### `takeover_force_unsupported`

- 不支持结束未受管的外部 CLI 实例：没有跨平台的进程归属证据，不会按目录或时间猜测 PID — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `takeover` L553 → `POST /api/term/takeover`

### `terminal_disabled`

- 终端传输未启用：必须显式配置隔离的 ptyhost 目录 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `enabled` L36

### `unsupported_history`

- 此数据源尚不支持原生操作范围
- 历史依赖或子代理缺少父线程 ID
- 父线程不在已配置索引中，请确认显式数据源包含其原生文件
- 分叉历史不能把子代理文件当作主线程父历史
- 子代理归属关系存在循环
- 分叉历史依赖存在循环
- history_base 必须是对象
- history_base 缺少父线程 ID
- history_base 前缀偏移必须是非负整数
- 父历史固定前缀超出完整原生数据范围
- 父历史固定前缀不在完整 JSONL 行边界
- 父历史缺少原生输入
- 父历史前缀不受支持：{error}
- 父历史固定前缀不受支持：{error}
- 父历史固定前缀不包含相同线程身份
- Claude 子代理的主会话不在已配置索引中
- Claude 子代理的主会话路径存在歧义
- 此数据源没有分叉父历史
- 原生记录缺少明确会话 ID，不能用文件名或显示名称推断
- 原生会话 ID 无效，不能确定操作范围
- M1 尚不支持非 UTF-8 会话路径；不能用替换字符生成冲突 UID
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `chain` L357
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `check_cut` L546, L549
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `dependencies` L327
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `history_link` L525, L534, L539
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `history_parent` L234
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `inherit` L666
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `native_scope` L116, L120
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `ownership` L255, L267
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `parse_prefix` L697, L723, L735, L738
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `resolve` L751
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `sid` L220, L225
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `unsupported` L57
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `chain` L403
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `check_cut` L379, L380
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `history_link` L556, L565, L569
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `history_parent` L336
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `native_scope` L499, L502
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `new` L261, L264
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `ownership` L357
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `sid` L322, L327
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `unsupported` L74
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `physical_chain` L276
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `thread` L231, L234, L244, L252
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `blank` L164
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `native_identity` L381, L394
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `trusted_path` L1239
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `grok_native_identity` L31
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `native_identity` L46, L74, L87
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `build` L1490
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L497, L510
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `committed_records` L480
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `inherit` L1628
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_scope` L1577, L1581
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `parse_prefix` L1679

## 503 Service Unavailable

### `audit_busy`

- 诊断解析槽位繁忙，请稍后重试 — [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L72 → `POST /api/audit/browser`

### `cancelled`

- 观察已取消 — [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L143

### `file_attachment_id`

- 无法分配附件目录编号 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1632

### `file_io`

- 文件访问失败；未忽略错误或返回空内容 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `io` L86

### `file_job_poisoned`

- 文件任务状态不可用，请刷新任务列表 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `lock` L162

### `file_jobs_poisoned`

- 文件任务登记不可用，请重启核验 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `inner` L190

### `file_publish_unlink`

- 已发布 {candidate}，但无法移除源名称：{error}
- 已复制到 {candidate}，但无法移除源名称：{error}
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1357
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1260

### `file_random_unavailable`

- 系统随机源不可用，不能分配任务编号 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `random_id` L152

### `file_trash_id`

- 无法分配回收目录编号 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `fresh_private_child` L1171

### `files_busy`

- 文件写入繁忙，请稍后重试
- 文件工作池繁忙，请稍后重试
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `attachment` L452 → `POST /api/session/attachment`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `admission` L76

### `lifecycle_response`

- 创建状态序列化失败 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `response` L263

### `media_busy`

- 图片处理繁忙，请稍后重试 — [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `read_media` L52

### `media_unavailable`

- 图片服务暂不可用 — [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `file` L98

### `metadata_clock_invalid`

- 系统时钟无效，不能记录固定时间 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L262 → `POST /api/session/rewind`

### `reader_busy`

- 读取服务已关闭 — [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L144

### `runtime_busy`

- 受控进程观察繁忙，请稍后重试 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `frozen_liveness` L99

### `runtime_unavailable`

- 受控 host 目录不可用或超出观察预算
- 受控 host 目录不可用或超出观察预算，无法确认会话是否仍在运行
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `unavailable` L331
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `frozen_liveness` L110

### `search_busy`

- 同时搜索过多，请稍后重试 — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L140 → `GET /api/search`

### `search_cancelled`

- 服务正在退出，搜索已取消 — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L172 → `GET /api/search`

### `session_error`

- 父历史固定前缀在读取期间变化，请重试
- {source} 数据源目录暂时不可枚举
- Codex 名称索引：{message}
- 会话行尚未发布，请重试
- 已配置的会话文件暂时不可读取
- 已索引的会话路径暂时不可解析
- 无法确认会话路径的父目录
- Grok 聊天文件无法检查，不能视为尚未创建
- 原生输入在打开或读取期间变化，请重试
- 原生输入读取失败，不能发布不完整快照
- 历史分页暂不可用
- 原生文本缓冲区分配失败
- 原生文本在读取期间变化，请重试
- 原生记录读取失败，未发布不完整快照
- 会话在校验期间变化，请重试
- 会话文件读取失败
- 会话或子代理元数据尚不是完整有效的 JSON
- 会话在读取期间变化，请重试
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `parse_prefix` L716
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `check_cut` L381
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `discover` L706, L708
- [`sessions/index/names.rs`](../crates/sessiondock/src/sessions/index/names.rs) `error` L31
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `prepare` L989
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `restamp` L1280
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `stamp` L1192
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `trusted_path` L1249, L1260
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `changed` L102
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `read_error` L105
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L180
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `issue` L119, L133, L143
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `link_media` L157
- [`sessions/records/native_records.rs`](../crates/sessiondock/src/sessions/records/native_records.rs) `io_error` L75
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L105, L110, L112, L116
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `committed_records` L476
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `parse_candidate` L591, L596, L599
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `read_bounded_limit` L537

### `shutdown`

- 服务正在关闭
- [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L54 → `POST /api/audit/browser`
- [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `shutting_down` L270
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `admission` L64
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `write_admission` L398
- [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `get` L83 → `GET /api/media/{token}`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `write` L121
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `observe` L345
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `shared` L281
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L258 → `GET /api/term/attach`
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim` L104 → `POST /api/term/claim`
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `send` L432 → `POST /api/term/send`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `frozen_liveness` L107

### `view_limit`

- 同时观察的逻辑视图过多 — [`observe.rs`](../crates/sessiondock/src/observe.rs) `subscribe` L76

### `watch_closed`

- 会话观察已关闭，请重试 — [`observe.rs`](../crates/sessiondock/src/observe.rs) `closed` L215

### `watch_limit`

- 同时观察的会话过多，请稍后重试 — [`api/read.rs`](../crates/sessiondock/src/api/read.rs) `watch` L393 → `GET /api/watch`

### `服务正在关闭`

- (dynamic) — [`state.rs`](../crates/sessiondock/src/state.rs) `admit` L109
