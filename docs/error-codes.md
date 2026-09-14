# HTTP error codes

This file is produced by `tests/error_codes.py`. Handlers return JSON `{"error": "<message>", "code": "<code>"}`. Status **501** means the route or capability is declared not implemented in this migration stage. Session errors use `unsupported_history` at 501 and `session_error` otherwise. Regenerate:

```sh
python3 tests/error_codes.py --write
```

Scanned `crates/sessiondock/src`: **175** (status, code) pairs.

## 400 Bad Request

### `backend_unknown`

- 未知终端后端 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L838 → `POST /api/term/backend`

### `backend_unsupported`

- Rust 后端不支持 tmux；新建会话只能由 ptyhost 托管 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L833 → `POST /api/term/backend`

### `bad_body`

- bad body
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report` L105, L109 → `POST /api/bug-report`
- [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `bad_body` L381
- [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `python_media_list` L434

### `create_cwd_failed`

- 创建启动目录失败：{error} — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `prepare_cwd` L405

### `file_absolute_path_required`

- 目录导航需要绝对路径
- 需要绝对路径
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `absolute_navigation` L407
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `mutation_path` L454

### `file_action_invalid`

- 未知文件操作 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L403

### `file_attachment_id`

- 附件目录编号无效 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1941

### `file_conflict_invalid`

- 无效的重名处理方式 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `parse` L78

### `file_cwd_unavailable`

- 相对引用需要有效的所选会话工作目录 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `absolute_path` L436

### `file_directory_anchor_required`

- 文件操作入口必须是会话提及的目录
- 文件浏览入口必须是会话提及的目录
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `anchor` L509
- [`files/grants.rs`](../crates/sessiondock/src/files/grants.rs) `directory_grant` L138
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `target` L261
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L380

### `file_directory_required`

- 此操作需要目录
- 请选择目录浏览
- 目标必须是目录
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `directory` L298
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `list` L519
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `directory` L309

### `file_field_required`

- 缺少必需字段 {field} — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `required` L1015

### `file_image_required`

- 图片引用必须指向普通文件，不能指向目录 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `new` L87

### `file_invalid_pdf`

- 文件没有有效的 PDF 标识，请下载后检查 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `validate_pdf` L189

### `file_job_invalid`

- 无效的任务编号 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `find` L190

### `file_list_options`

- 无效的目录分页或排序参数 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `list` L511

### `file_media_reference_invalid`

- 图片路径不能为空 — [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `normalize_media_ref` L33

### `file_move_into_self`

- 不能把目录放进自身或子目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L587

### `file_name_invalid`

- 名称无效：须为单个路径组件，不能包含 /、\、控制字符或为 . 与 .. — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `validate_name` L1026

### `file_parent_not_directory`

- 路径的父组件不是目录
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L332
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `entry` L331

### `file_path_encoding`

- 文件目录必须能表示为 UTF-8 路径
- 路径无法表示为 UTF-8
- 目录含非 UTF-8 名称，不能安全导航
- 目录路径不是 UTF-8
- 链接目标无法表示为 UTF-8
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `list` L530
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L133
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `wire_path` L483
- [`files/grants.rs`](../crates/sessiondock/src/files/grants.rs) `browser_anchor` L180
- [`files/info.rs`](../crates/sessiondock/src/files/info.rs) `browser_info` L34
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `delete` L670
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `move_recycle_entry` L1608
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L583

### `file_path_invalid`

- 需要绝对路径
- 文件目录需要标准绝对路径
- 路径与打开的文件系统卷不一致
- 路径组件无效
- 路径为空、过长或包含空字符
- 文件引用不能是 URL
- 无效的目录引用
- 源路径缺少名称
- 目标路径缺少名称
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `absolute_path` L420
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L143
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L311, L316
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `validate_path_text` L386
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `volume_root` L101
- [`files/grants.rs`](../crates/sessiondock/src/files/grants.rs) `directory_grant` L130
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_then_remove_entry_for_test` L1585, L1588
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `entry` L325

### `file_paths_required`

- 请先选择项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `items` L409

### `file_preview_unsupported`

- 此格式请使用文本预览或下载 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `read` L417

### `file_reference_invalid`

- 文件引用为空、过长或包含空字符
- 文件引用不能为空
- [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `clean_ref` L98, L108

### `file_reference_limit`

- 一次最多解析 256 个文件引用 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_many` L278

### `file_required`

- 此操作需要普通文件
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `file` L260
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1280
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1215

### `file_root_not_directory`

- 文件入口必须是既有目录 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L160

### `file_same_path`

- 源路径和目标相同
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L594
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `rename` L535

### `file_scope_invalid`

- 文件访问需要已解析的有效会话视图 — [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `validate_scope` L118

### `file_sha256_invalid`

- sha256 须为 64 位十六进制 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `parse_sha256` L1049

### `file_single_item_required`

- 请选择一个项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `rename` L523

### `file_upload_content_type`

- 上传分块必须使用 application/octet-stream — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L597 → `POST /api/session/files/upload`

### `file_upload_empty`

- 附件为空或缺少 Content-Length — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1921

### `file_upload_modified_invalid`

- 无效的修改时间 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L781

### `file_upload_offset_invalid`

- offset 必须是非负整数 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L585 → `POST /api/session/files/upload`

### `file_upload_size_invalid`

- 无效的上传大小 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L767

### `file_write_limits_invalid`

- 写入预算必须为正数 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `open` L171

### `invalid_attach`

- 终端连接参数无效 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L215 → `GET /api/term/attach`

### `invalid_audit_request`

- (dynamic) — [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L60 → `POST /api/audit/browser`

### `invalid_bug_report`

- (dynamic) — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `invalid` L77

### `invalid_claim`

- 终端预约请求格式无效 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim` L117 → `POST /api/term/claim`

### `invalid_file_request`

- 需要有效的会话、分支和文件参数 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `invalid` L47

### `invalid_launch_request`

- 创建请求格式或身份字段无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `invalid` L90

### `invalid_metadata_batch`

- 需要有效会话 uid — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `visibility` L179 → `POST /api/sessions/fork-visibility`

### `invalid_metadata_request`

- 需要有效的偏好 JSON 请求和布尔状态 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `invalid` L99

### `invalid_metadata_uid`

- 需要有效的会话 uid
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L219 → `POST /api/session/rewind`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `star` L144 → `POST /api/session/star`

### `invalid_outbox_query`

- 需要有效的会话 UID 和完整子代理 ID — [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `outbox` L64 → `GET /api/session/outbox`

### `invalid_path`

- 启动目录路径无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `complete_dir` L789 → `GET /api/term/complete-dir`

### `invalid_purge`

- 需要 id/ids，或 all:true / days:N（二者不能同时给出） — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L364 → `POST /api/trash/purge`

### `invalid_query`

- 查询参数无效
- force 必须为 0/1
- [`api/read.rs`](../crates/sessiondock/src/api/read.rs) `query_error` L41
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_session` L168, L170
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `list` L286 → `GET /api/trash`

### `invalid_rewind_target`

- target 必须是 Claude 记录节点 ID，或 null 表示取消固定 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L226 → `POST /api/session/rewind`

### `invalid_scroll`

- 终端滚动请求格式无效 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `scroll` L541 → `POST /api/term/scroll`

### `invalid_search_query`

- (dynamic) — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L120 → `GET /api/search`

### `invalid_stop_request`

- 停止请求格式或会话 UID 无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `invalid_stop` L942

### `invalid_terminal_input`

- (dynamic) — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `invalid_input` L362

### `invalid_trash_request`

- 请求体无效: {} — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `invalid` L40

### `invalid_uid`

- 会话 uid 无效
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_batch` L241 → `POST /api/sessions/delete`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_session` L177

### `launch_adapter`

- 来源没有唯一的可续接 CLI 配置
- 来源没有唯一的已配置 CLI
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `select_entry` L307, L312

### `no_sessions`

- 没有选中任何会话 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_batch` L252 → `POST /api/sessions/delete`

### `session_error`

- 历史页游标格式无效
- 图片分页游标格式无效
- 可靠发送只支持 Claude 主会话
- append 和 window 只接受 0 或 1
- 这不是 Claude 主会话
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `refresh_within` L446
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L176
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_native_inputs` L364
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L488
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_message_query` L448

### `websocket_required`

- 需要有效的 WebSocket 升级请求 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L269 → `GET /api/term/attach`

## 403 Forbidden

### `cross_origin`

- 拒绝跨源 API 请求 — [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L91

### `cross_site`

- 拒绝跨站 API 请求 — [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L80

### `file_forbidden`

- 操作系统不允许访问此文件或目录 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `io` L77

### `file_private_dir_invalid`

- {name} 必须是目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1101

### `file_private_dir_permissions`

- {name} 目录必须仅所有者可访问 (0700) — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1114

### `file_root_immutable`

- 不能修改根目录、用户主目录或文件管理器的数据目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `guard` L294

### `file_special_forbidden`

- 只允许普通文件与目录，不读取设备、管道或套接字
- 只允许普通文件、目录与符号链接
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `ordinary` L74
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `unshared` L88

### `hub_unsupported`

- 尚不支持旧 Hub 节点协议 — [`security.rs`](../crates/sessiondock/src/security.rs) `local_only` L62

### `local_only`

- 仅允许本地 loopback Host — [`security.rs`](../crates/sessiondock/src/security.rs) `local_only` L50

### `media_native_scope`

- 原生图片需要当前来源授权读取器
- [`media/descriptors.rs`](../crates/sessiondock/src/media/descriptors.rs) `materialize` L201
- [`media/native_media.rs`](../crates/sessiondock/src/media/native_media.rs) `materialize_native` L258

### `media_scope`

- 图片不属于当前媒体服务
- 图片需要当前所选会话授权
- 图片不属于当前所选会话
- [`media/descriptors.rs`](../crates/sessiondock/src/media/descriptors.rs) `materialize` L208, L212
- [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `authorize` L26, L47
- [`media/native_media.rs`](../crates/sessiondock/src/media/native_media.rs) `materialize_native` L255

### `node_auth_required`

- node authentication required — [`api/node_auth.rs`](../crates/sessiondock/src/api/node_auth.rs) `auth_required` L73

### `node_peer_denied`

- forbidden — [`api/node_auth.rs`](../crates/sessiondock/src/api/node_auth.rs) `peer_denied` L69

### `session_error`

- 会话输入必须是普通文件
- 会话输入路径越过已配置的数据源边界
- 历史页不属于所选会话或子代理
- 图片分页不属于所选会话或子代理
- 文本读回范围不属于当前原生记录
- 工具解码范围不属于当前原生记录
- 工具解码范围无效
- 图片缺少当前原生来源授权
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `file_stamp` L1190
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `trusted_path` L1245
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L186
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L71
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `prepare` L154, L158
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_source` L440

### `terminal_disabled`

- (dynamic) — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `terminal_off` L41

## 404 Not Found

### `entry_not_found`

- 回收站条目不存在 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L387 → `POST /api/trash/purge`

### `file_job_unknown`

- 任务不存在或不属于此会话 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `find` L195, L200

### `file_not_found`

- 会话没有绝对工作目录
- 文件不存在，或会话未记录其完整路径
- 文件或目录不存在
- [`files/media.rs`](../crates/sessiondock/src/files/media.rs) `image` L117
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `io` L75
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L236

### `file_not_referenced`

- 该路径未出现在所选会话分支中
- [`files/media.rs`](../crates/sessiondock/src/files/media.rs) `image` L105
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L196

### `launch_missing`

- 没有这个创建回执 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `failure` L86

### `media_not_found`

- 图片不存在或已过期，请重新加载会话 — [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `get` L72 → `GET /api/media/{token}`

### `not_found`

- node listener serves /api only
- API route not found
- [`api/mod.rs`](../crates/sessiondock/src/api/mod.rs) `node_not_found` L283
- [`api/mod.rs`](../crates/sessiondock/src/api/mod.rs) `not_found` L291 → `ANY (fallback)`

### `session_error`

- 会话不存在
- 子代理必须通过所属主会话访问
- 子代理不存在或不属于此主会话
- 历史页不存在或已淘汰，请重新载入会话
- 图片分页不存在或已淘汰，请重新载入会话
- 目标不是这个 Claude 会话的记录节点
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `pending_attachment_scope` L717
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `select` L262, L271, L298
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `select_main` L480
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `physical_chain` L258
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `prepare` L940, L945, L951
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `search_version` L761, L766
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L184
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `lookup` L202
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `lookup_media` L213
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L496
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_request` L1337, L1340
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `view_meta` L1381

### `session_missing`

- 会话不存在
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `external_processes` L488
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L988 → `POST /api/session/stop`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L263 → `POST /api/session/rewind`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `star` L156 → `POST /api/session/star`

### `terminal_missing`

- 指定目录中没有这个终端 host — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `scroll` L548 → `POST /api/term/scroll`

## 409 Conflict

### `file_ambiguous`

- 会话中有多个同名文件，请点击完整路径 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L212, L227

### `file_attachment_dir`

- {attachment_dir} 不是安全目录
- 附件编号对应的不是安全目录
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1958, L1975

### `file_changed`

- 文件或父目录在读取期间已变化，请刷新后重试
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L169
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L341, L367
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `ordinary` L71
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `verify` L115, L119, L244, L249, L252
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `verify_identity` L279, L287, L289
- [`files/info.rs`](../crates/sessiondock/src/files/info.rs) `browser_info` L52
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `changed` L67
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L2015
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_entry` L1475, L1492, L1510
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1313, L1320
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1110
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1228, L1231
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate_entry` L1757, L1759
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `remove_verified` L1384, L1393
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `trash_named` L745, L747
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `verify` L1358, L1364

### `file_exists`

- 目标已存在；写入服务不会覆盖，请改名、跳过或保留两份
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `exists_error` L1074
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L796

### `file_job_not_completed`

- 只能登记已完成的上传任务 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `completed_upload` L253

### `file_job_skipped`

- 该上传因重名被跳过，没有可登记的文件 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `completed_upload` L260

### `file_keep_exhausted`

- 无法生成唯一名称
- 同名附件过多，无法分配文件名
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L2108
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `keep_exhausted` L1081

### `file_move_cross_device`

- 需要跨文件系统复制后回收源项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate_entry` L1775

### `file_upload_checksum`

- 上传数据的 SHA-256 与声明不符，已丢弃暂存数据 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `finalize` L959

### `file_upload_finished`

- 上传已结束或已取消 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L880

### `file_upload_offset`

- 重发的分块与已接收数据不一致，请刷新任务后从预期位置继续
- 上传位置不一致，请刷新任务后从预期位置继续
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L917, L925

### `launch_cwd_unknown`

- 该会话没有记录可用的工作目录；请通过创建接口明确指定目录续接 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `takeover` L741 → `POST /api/term/takeover`

### `launch_identity`

- 创建回执与附件目标实例不匹配
- 创建回执与进程实例不匹配
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `pending_attachment_cwd` L666, L675
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `check_instance` L114

### `launch_identity_declared`

- 该实例启动时已在命令行声明完整原生会话 ID；由运行时目录关联，不接受另行的操作者绑定 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `bind` L187 → `POST /api/term/bind`

### `launch_not_finished`

- 该创建实例尚未退出或取消，不能丢弃；请先停止它 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `discard` L919 → `POST /api/term/discard`

### `launch_not_ready`

- 创建回执已被丢弃，不能上传附件 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `pending_attachment_cwd` L685

### `launch_source`

- 续接会话的数据源与请求来源不一致 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `create` L618 → `POST /api/term/create`

### `media_changed`

- 图片文件已变化，请重新加载会话 — [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `authorize` L30, L51

### `media_native_changed`

- 原生图片来源已变化，请重新加载会话 — [`media/native_media.rs`](../crates/sessiondock/src/media/native_media.rs) `changed` L162

### `run_state_unknown`

- 该会话的受管实例运行状态未知（{reason}），未发送任何停止指令；未知不等于已退出，请稍后重试或检查宿主 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1073 → `POST /api/session/stop`

### `session_error`

- Claude 子代理声明的会话 ID 与主会话不一致
- 父线程 ID 在已配置索引中存在歧义
- Codex 子代理 ID 在索引中存在歧义
- 主会话中子代理 ID 存在歧义
- 原生记录包含冲突的会话 ID
- 图片分页范围已变化，请重新载入会话
- 原生输入范围无效或已变化，请重试
- 原生图片内容或范围已变化，请重新加载会话
- 原生嵌套图片的外层来源未通过校验
- 图片原生来源已消失
- 图片原生来源已替换，请重新加载会话
- 原生嵌套图片范围无效
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
- 原生工具输出解码无法建立
- 图片已不属于当前会话分支，请重新加载会话
- 目标不在当前 Claude 时间线上，无法固定显示
- 首条消息之前没有可固定显示的时间线
- 子代理与主会话的数据源不一致
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `native_scope` L117
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `select` L268, L291, L299
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `sid` L215
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `build` L578
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `select_main` L477
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `sid` L324
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `thread` L242
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `native_identity` L381
- [`sessions/media_projection.rs`](../crates/sessiondock/src/sessions/media_projection.rs) `project_range` L40
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `invalid_range` L21
- [`sessions/native_media.rs`](../crates/sessiondock/src/sessions/native_media.rs) `authorized_reader` L62, L64, L75, L84
- [`sessions/native_media.rs`](../crates/sessiondock/src/sessions/native_media.rs) `finish` L29, L32
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `history_page` L415
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `media_page` L458, L461, L467, L473
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `validate_grant_scope` L390
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L68, L86
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `prepare` L161, L170
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `replay_changed` L21
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `native_identity` L78
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L499, L502
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_scope` L1559
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_source` L431
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_request` L1345

### `stale_build`

- 页面版本已过期，请刷新后再保存偏好 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `validate` L42

### `stop_superseded`

- 该回滚分支已不是当前运行分支，未停止共享的子会话 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L996 → `POST /api/session/stop`

### `takeover_superseded`

- 该回滚分支的运行实例已转移到更新的子会话，请先处理当前子会话 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `takeover` L702 → `POST /api/term/takeover`

### `terminal_binding_unavailable`

- 无法确认会话与终端实例的唯一关联；请刷新，不会降级按名称连接。 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `binding_unavailable` L321

## 410 Gone

### `session_error`

- 历史页已过期，请重新载入会话
- 图片分页已过期，请重新载入会话
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L190

## 413 Payload Too Large

### `body_too_large`

- 浏览器诊断请求体过大
- 缺陷报告请求体过大
- {what}请求体过大
- 文件引用请求体过大
- 文件操作请求体最多 512 KiB
- 创建请求体过大
- 停止请求体过大
- 偏好请求体超过大小限制
- 终端预约请求体过大
- 请求体过大
- [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L52 → `POST /api/audit/browser`
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report` L99 → `POST /api/bug-report`
- [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `bad_body` L375
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `action` L526 → `POST /api/session/files/action`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `resolve` L125 → `POST /api/session/resolve-files`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `parse_body` L99
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L961 → `POST /api/session/stop`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `invalid` L93
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim` L111 → `POST /api/term/claim`
- [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L107

### `delivery_text_too_large`

- 消息正文超过 1 MiB — [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `send` L454 → `POST /api/session/send`

### `file_image_budget`

- 单张磁盘图片超过 32 MiB 读取上限 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `new` L95

### `file_items_limit`

- 一次最多操作 {MAX_WRITE_ITEMS} 个项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `items` L412

### `file_job_too_large`

- 单个上传最多 {} 字节 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L769

### `file_raw_budget`

- 原始打开超过 32 MiB，请使用预览或下载
- 原始打开超过 32 MiB，请使用文本预览或下载
- [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `read` L406, L424

### `file_upload_chunk_too_large`

- 单个分块最多 {limit} 字节
- 单个分块最多 {} 字节
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L610 → `POST /api/session/files/upload`
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L861

### `file_upload_overflow`

- 分块超过声明的上传大小 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L892

### `file_upload_too_large`

- 单个附件不能超过 {} MB
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload_attachment` L743
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L1928

### `session_error`

- 继承历史预算溢出
- 此历史页无法在读取预算内推进
- 此图片分页无法在读取预算内推进
- 图片分页响应超过 8 MiB 预算
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `inherit` L646
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `history_page` L423
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `media_page` L478, L512
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `inherit` L1603

### `terminal_input_too_large`

- 终端输入请求体过大
- 单次终端输入不能超过 1 MiB
- 单次终端粘贴不能超过 1 MiB
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `send` L373, L402, L428, L440 → `POST /api/term/send`

### `too_many_events`

- 单次诊断批次事件过多 — [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L65 → `POST /api/audit/browser`

## 414 Uri Too Long

### `uri_too_long`

- 请求 URI 过长 — [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L97

## 500 Internal Server Error

### `bug_report_failed`

- 报告捕获任务异常退出 — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report` L201 → `POST /api/bug-report`

### `file_headers_invalid`

- 文件响应头无效 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `response_body` L441

### `file_worker_failed`

- 附件写入任务异常退出
- 文件读取任务异常退出
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `attachment` L440 → `POST /api/session/attachment`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `work` L94

### `live_failed`

- 进程表配对失败 — [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `live` L245 → `GET /api/live`

### `metadata_worker_failed`

- 偏好工作异常退出，请重新读取状态确认结果 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `write` L127

### `reader_failed`

- 只读任务失败
- [`state.rs`](../crates/sessiondock/src/state.rs) `run` L155
- [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L130

### `runtime_encoding`

- 受控进程观察无法编码 — [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `live` L154 → `GET /api/live`

### `search_failed`

- 搜索任务失败 — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L159 → `GET /api/search`

### `session_error`

- 历史消息序列化失败
- 会话索引锁不可用
- 会话视图锁不可用
- 原生事件的物理范围与已提交的行边界不一致
- 历史页响应序列化失败
- 消息序列化失败
- 会话行不是对象
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `push` L614
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `refresh_within` L451
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `list_state` L471
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `views` L481
- [`sessions/native_tail.rs`](../crates/sessiondock/src/sessions/native_tail.rs) `native_tail` L108
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `take` L251
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `validate_response` L296
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `encoded_bytes` L97
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `view_meta` L1366

### `trash_encoding`

- 回收站结果无法编码 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `encoding` L214

### `trash_failed`

- 回收站任务失败
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `list` L299 → `GET /api/trash`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L376 → `POST /api/trash/purge`

## 501 Not Implemented

### `bug_report_disabled`

- 缺陷报告未启用：需要 SESSIONDOCK_BUG_REPORT_DIR/REPO、审计目录、终端传输和受控创建 — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `disabled` L33

### `delivery_disabled`

- 发送账本读取未启用：需要显式初始化并配置独立开发目录 — [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `outbox` L57 → `GET /api/session/outbox`

### `delivery_send_disabled`

- 可靠发送未启用：需要已初始化的发送账本目录和显式终端传输目录 — [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `executor` L155

### `file_action_not_implemented`

- 写入服务尚未实现此文件操作：{}；不会伪造任务或成功结果 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L395

### `file_mode_not_implemented`

- 只读文件服务尚未实现此模式；不会伪造空任务或成功结果
- 文件服务尚未实现此模式
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `get` L313, L342
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `validate` L183, L188
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `unsupported` L64

### `file_trash_unconfigured`

- 未配置 SESSIONDOCK_STATE_DIR，没有回收目录；写入服务不会直接删除 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `trash_named` L703

### `files_disabled`

- 文件读取服务不可用 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `configured` L55

### `files_jobs_disabled`

- 文件写入服务未启用
- 文件写入未启用：必须显式配置 SESSIONDOCK_FILE_WRITE_ROOTS（只读目录不会隐式变为可写）
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `attachment` L384 → `POST /api/session/attachment`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `write_configured` L458

### `media_files_disabled`

- 未配置图片读取目录；不会自动访问磁盘 — [`sessions/media_projection.rs`](../crates/sessiondock/src/sessions/media_projection.rs) `project_ranges` L71

### `metadata_disabled`

- 偏好保存未配置状态目录 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `configured` L83

### `not_implemented`

- Rust 后端尚未迁移此能力：{capability}。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：受控会话创建与 pending 生命周期。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：终端后端选择。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：受管实例停止。当前是只读开发阶段。
- [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L32 → `POST /api/audit/browser`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L820 → `POST /api/term/backend`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `enabled` L29
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L956, L1004 → `POST /api/session/stop`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `configured` L36
- [`error.rs`](../crates/sessiondock/src/error.rs) `unavailable` L31

### `terminal_disabled`

- 终端传输未启用：必须显式配置隔离的 ptyhost 目录 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `enabled` L40

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
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `chain` L338
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `check_cut` L527, L530
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `dependencies` L315
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `history_link` L506, L515, L520
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `history_parent` L225
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `inherit` L638
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `native_scope` L107, L111
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `ownership` L246, L255
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `parse_prefix` L666, L686, L698, L701
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `resolve` L714
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `sid` L211, L216
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `unsupported` L48
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `chain` L392
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `check_cut` L371, L372
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `history_link` L545, L554, L558
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `history_parent` L334
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `native_scope` L488, L491
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `new` L259, L262
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `ownership` L355
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `sid` L320, L325
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `unsupported` L72
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `physical_chain` L264
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `thread` L222, L225, L235, L243
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `blank` L158
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `native_identity` L375, L388
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `grok_native_identity` L31
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `native_identity` L46, L72, L85
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `build` L1468
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L491, L504
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `committed_records` L474
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `inherit` L1597
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_scope` L1549, L1553
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `parse_prefix` L1645

## 503 Service Unavailable

### `cancelled`

- 观察已取消 — [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L120

### `cwd_check_failed`

- 启动目录检查失败 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `prepare_cwd` L422

### `file_attachment_id`

- 无法分配附件目录编号 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `bug_report_upload` L2006

### `file_grant_store`

- 无法保存目录浏览授权
- 无法创建目录授权暂存文件
- [`files/grants.rs`](../crates/sessiondock/src/files/grants.rs) `insert` L81, L84

### `file_io`

- 文件访问失败；未忽略错误或返回空内容 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `io` L79

### `file_job_poisoned`

- 文件任务状态不可用，请刷新任务列表 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `lock` L149

### `file_jobs_poisoned`

- 文件任务登记不可用，请重启核验 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `inner` L173

### `file_publish_unlink`

- 已发布 {candidate}，但无法移除源名称：{error}
- 已复制到 {candidate}，但无法移除源名称：{error}
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1331
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1234

### `file_random_unavailable`

- 系统随机源不可用，不能分配任务编号 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `random_id` L139

### `file_trash_id`

- 无法分配回收目录编号 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `fresh_private_child` L1138

### `lifecycle_response`

- 创建状态序列化失败 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `response` L225

### `media_busy`

- 图片处理繁忙，请稍后重试 — [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `read_media` L48

### `media_unavailable`

- 图片服务暂不可用 — [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `file` L98

### `metadata_clock_invalid`

- 系统时钟无效，不能记录固定时间 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L240 → `POST /api/session/rewind`

### `process_control_unavailable`

- 无法结束外部会话进程：{error:?}
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1027 → `POST /api/session/stop`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `takeover` L729 → `POST /api/term/takeover`

### `process_scan_unavailable`

- 无法核对会话进程：{error}
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `external_processes` L518
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `external_scan_result` L46

### `reader_busy`

- 读取服务已关闭 — [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L121

### `runtime_closed`

- 进程观察服务已关闭 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `frozen_liveness` L76

### `runtime_unavailable`

- 受控 host 目录不可用或超出观察预算
- 无法读取受管进程状态
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `unavailable` L392
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `frozen_liveness` L88

### `search_cancelled`

- 服务正在退出，搜索已取消 — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L157 → `GET /api/search`

### `search_closed`

- 搜索服务正在关闭 — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L136 → `GET /api/search`

### `session_error`

- 父历史固定前缀在读取期间变化，请重试
- {source} 数据源目录暂时不可枚举
- Codex 名称索引：{message}
- 会话行尚未发布，请重试
- 已配置的会话文件暂时不可读取
- 原生输入在打开或读取期间变化，请重试
- 原生输入读取失败，不能发布不完整快照
- 原生输入索引内存分配失败
- 历史分页暂不可用
- 原生文本缓冲区分配失败
- 原生文本在读取期间变化，请重试
- 原生记录读取失败，未发布不完整快照
- 会话在校验期间变化，请重试
- 会话文件读取失败
- 会话或子代理元数据尚不是完整有效的 JSON
- 会话在读取期间变化，请重试
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `parse_prefix` L679
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `check_cut` L373
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `discover` L690, L692
- [`sessions/index/names.rs`](../crates/sessiondock/src/sessions/index/names.rs) `error` L23
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `file_stamp` L1184, L1188
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `prepare` L957
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `stamp` L1177
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `allocation_error` L24
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `changed` L15
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `read_error` L18
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L181
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `issue` L120, L134, L144
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `link_media` L158
- [`sessions/records/native_records.rs`](../crates/sessiondock/src/sessions/records/native_records.rs) `io_error` L74
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L89, L94, L96, L100
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `committed_records` L470
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `parse_candidate` L574, L579, L582
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `read_bounded_limit` L530

### `shutdown`

- 服务正在关闭
- [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L40 → `POST /api/audit/browser`
- [`api/delivery.rs`](../crates/sessiondock/src/api/delivery.rs) `shutting_down` L413
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `admission` L64
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `write_admission` L468
- [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `get` L79 → `GET /api/media/{token}`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `write` L111
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `observe` L406
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `shared` L348
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L278 → `GET /api/term/attach`
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim` L139 → `POST /api/term/claim`
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `send` L469 → `POST /api/term/send`

### `watch_closed`

- 会话观察已关闭，请重试 — [`observe.rs`](../crates/sessiondock/src/observe.rs) `closed` L208

### `服务正在关闭`

- (dynamic) — [`state.rs`](../crates/sessiondock/src/state.rs) `admit` L98
