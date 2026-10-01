# HTTP error codes

This file is produced by `tests/error_codes.py`. Handlers return JSON `{"error": "<message>", "code": "<code>"}`. Status **501** means the route or capability is declared not implemented in this migration stage. Session errors use `unsupported_history` at 501 and `session_error` otherwise. Regenerate:

```sh
python3 tests/error_codes.py --write
```

Scanned `crates/sessiondock/src`: **232** (status, code) pairs.

## 400 Bad Request

### `attachment_interrupted`

- 附件上传中断 — [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `drop` L376

### `attachment_query`

- 附件请求无效 — [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `upload` L298 → `POST /api/session/conversation/attachment`

### `backend_unknown`

- 未知终端后端 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L967 → `POST /api/term/backend`

### `backend_unsupported`

- Rust 后端不支持 tmux；新建会话只能由 ptyhost 托管 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L962 → `POST /api/term/backend`

### `bad_body`

- bad body
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `capture` L504, L506 → `POST /api/bug-report/capture`
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report_inner` L136, L140, L197, L210, L265, L375, L456

### `create_cwd_failed`

- 创建启动目录失败：{error} — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `prepare_cwd` L454

### `file_absolute_path_required`

- 目录导航需要绝对路径
- 需要绝对路径
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `absolute_navigation` L407
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `mutation_path` L454

### `file_action_invalid`

- 未知文件操作 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L402

### `file_attachment_id`

- 附件目录编号无效 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L2028

### `file_conflict_invalid`

- 无效的重名处理方式 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `parse` L78

### `file_cwd_unavailable`

- 相对引用需要有效的所选会话工作目录 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `absolute_path` L436

### `file_directory_anchor_required`

- 文件操作入口必须是会话提及的目录
- 文件浏览入口必须是会话提及的目录
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `anchor` L509
- [`files/grants.rs`](../crates/sessiondock/src/files/grants.rs) `directory_grant` L138
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `target` L268
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L379

### `file_directory_required`

- 此操作需要目录
- 请选择目录浏览
- 目标必须是目录
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `directory` L298
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `list` L519
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `directory` L308

### `file_field_required`

- 缺少必需字段 {field} — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `required` L1014

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

- 不能把目录放进自身或子目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L586

### `file_name_invalid`

- 名称无效：须为单个路径组件，不能包含 /、\、控制字符或为 . 与 .. — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `validate_name` L1025

### `file_parent_not_directory`

- 路径的父组件不是目录
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L332
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `entry` L330

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
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `delete` L669
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `move_recycle_entry` L1607
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L582

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
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_then_remove_entry_for_test` L1584, L1587
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `entry` L324

### `file_paths_required`

- 请先选择项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `items` L408

### `file_preview_unsupported`

- 此格式请使用文本预览或下载 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `read` L417

### `file_reference_invalid`

- 文件引用为空、过长或包含空字符
- 文件引用不能为空
- [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `clean_ref` L98, L108

### `file_reference_limit`

- 一次最多解析 256 个文件引用 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_many` L285

### `file_required`

- 此操作需要普通文件
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `file` L260
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1279
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1214

### `file_root_not_directory`

- 文件入口必须是既有目录 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L160

### `file_same_path`

- 源路径和目标相同
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L593
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `rename` L534

### `file_scope_invalid`

- 文件访问需要已解析的有效会话视图 — [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `validate_scope` L118

### `file_sha256_invalid`

- sha256 须为 64 位十六进制 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `parse_sha256` L1048

### `file_single_item_required`

- 请选择一个项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `rename` L522

### `file_upload_content_type`

- 上传分块必须使用 application/octet-stream — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L597 → `POST /api/session/files/upload`

### `file_upload_empty`

- 附件为空
- 附件为空或缺少 Content-Length
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `drop` L396
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L2008

### `file_upload_modified_invalid`

- 无效的修改时间 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L780

### `file_upload_offset_invalid`

- offset 必须是非负整数 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L585 → `POST /api/session/files/upload`

### `file_upload_size_invalid`

- 无效的上传大小 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L766

### `file_write_limits_invalid`

- 写入预算必须为正数 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `open` L171

### `invalid_attach`

- 终端连接参数无效 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L277 → `GET /api/term/attach`

### `invalid_audit_request`

- (dynamic) — [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L60 → `POST /api/audit/browser`

### `invalid_bug_report`

- (dynamic) — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `invalid` L92

### `invalid_claim`

- 终端预约请求格式无效 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim_inner` L173

### `invalid_file_request`

- 需要有效的会话、分支和文件参数 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `invalid` L47

### `invalid_history`

- 历史行参数无效
- 历史行范围无效
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `grid_history` L518, L520 → `GET /api/term/grid/history`

### `invalid_launch_request`

- 创建请求格式或身份字段无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `invalid` L94

### `invalid_metadata_batch`

- 需要有效会话 uid — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `visibility` L246 → `POST /api/sessions/fork-visibility`

### `invalid_metadata_request`

- 需要有效的偏好 JSON 请求和布尔状态 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `invalid` L115

### `invalid_metadata_uid`

- 需要有效的会话 uid
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L333 → `POST /api/session/nest`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L450 → `POST /api/session/rewind`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `star` L211 → `POST /api/session/star`

### `invalid_path`

- 启动目录路径无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `complete_dir` L861 → `GET /api/term/complete-dir`

### `invalid_purge`

- 需要 id/ids，或 all:true / days:N（二者不能同时给出） — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L594 → `POST /api/trash/purge`

### `invalid_query`

- 查询参数无效
- force 必须为 0/1
- [`api/read.rs`](../crates/sessiondock/src/api/read.rs) `query_error` L45
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_session` L398, L400
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `list` L516 → `GET /api/trash`

### `invalid_record`

- 录制参数无效 — [`api/records.rs`](../crates/sessiondock/src/api/records.rs) `attach` L68 → `GET /api/term/records/attach`

### `invalid_rewind_target`

- target 必须是 Claude 记录节点 ID，或 null 表示取消固定 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L457 → `POST /api/session/rewind`

### `invalid_scope`

- (dynamic)
- [`api/process_links.rs`](../crates/sessiondock/src/api/process_links.rs) `session_probe` L90 → `POST /api/session/resources/probe`
- [`api/process_links.rs`](../crates/sessiondock/src/api/process_links.rs) `session_resources` L39 → `GET /api/session/resources`

### `invalid_scroll`

- 终端滚动请求格式无效 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `scroll` L733 → `POST /api/term/scroll`

### `invalid_search_query`

- (dynamic) — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L120 → `GET /api/search`

### `invalid_stop_request`

- 停止请求格式或会话 UID 无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `invalid_stop` L1137

### `invalid_terminal_input`

- (dynamic) — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `invalid_input` L490

### `invalid_trash_request`

- 请求体无效: {} — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `invalid` L43

### `invalid_uid`

- 会话 uid 无效
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_batch` L471 → `POST /api/sessions/delete`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_session` L407

### `launch_adapter`

- 来源没有唯一的可续接 CLI 配置
- 来源没有唯一的已配置 CLI
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `select_entry` L356, L361

### `launch_model`

- 模型或推理强度名称无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `create` L699 → `POST /api/term/create`

### `nest_conflict`

- 跨机器父会话信息无效 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L369 → `POST /api/session/nest`

### `nest_parent_missing`

- 目标会话缺少来源或会话 id — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L403 → `POST /api/session/nest`

### `nest_parent_node`

- 只能附属到同一台机器上的会话 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L394 → `POST /api/session/nest`

### `nest_parent_self`

- 不能附属到自己下面 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L378 → `POST /api/session/nest`

### `no_sessions`

- 没有选中任何会话 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_batch` L482 → `POST /api/sessions/delete`

### `session_error`

- 历史页游标格式无效
- 图片分页游标格式无效
- append 和 window 只接受 0 或 1
- 这不是 Claude 主会话
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `refresh_within` L553
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L176
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L570
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_message_query` L530

### `websocket_required`

- 需要有效的 WebSocket 升级请求
- [`api/records.rs`](../crates/sessiondock/src/api/records.rs) `attach` L93 → `GET /api/term/records/attach`
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L331 → `GET /api/term/attach`

## 403 Forbidden

### `cross_origin`

- 拒绝跨源 API 请求 — [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L91

### `cross_site`

- 拒绝跨站 API 请求 — [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L80

### `file_forbidden`

- 操作系统不允许访问此文件或目录 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `io` L77

### `file_private_dir_invalid`

- {name} 必须是目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1100

### `file_private_dir_permissions`

- {name} 目录必须仅所有者可访问 (0700) — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1113

### `file_root_immutable`

- 不能修改根目录、用户主目录或文件管理器的数据目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `guard` L293

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
- [`media/native_media.rs`](../crates/sessiondock/src/media/native_media.rs) `materialize_native` L259

### `media_scope`

- 图片不属于当前媒体服务
- 图片需要当前所选会话授权
- 图片不属于当前所选会话
- [`media/descriptors.rs`](../crates/sessiondock/src/media/descriptors.rs) `materialize` L208, L212
- [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `authorize` L26, L47
- [`media/native_media.rs`](../crates/sessiondock/src/media/native_media.rs) `materialize_native` L256

### `nest_remote_hub`

- 跨机器附属需要通过 Hub 验证 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L346 → `POST /api/session/nest`

### `node_auth_required`

- node authentication required
- 进程关联仅接受经过认证的 Hub
- 机器探测仅接受经过认证的 Hub
- [`api/node_auth.rs`](../crates/sessiondock/src/api/node_auth.rs) `auth_required` L73
- [`api/process_links.rs`](../crates/sessiondock/src/api/process_links.rs) `node_probe` L74 → `POST /api/resources/probe`
- [`api/process_links.rs`](../crates/sessiondock/src/api/process_links.rs) `post` L14

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
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `file_stamp` L1396
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `trusted_path` L1451
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L186
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L71
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `prepare` L154, L158
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_source` L522

### `terminal_disabled`

- (dynamic) — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `terminal_off` L56

## 404 Not Found

### `attachment_missing`

- 附件暂存字节已不在服务端 — [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `staged` L507

### `entry_not_found`

- 回收站条目不存在 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L617 → `POST /api/trash/purge`

### `file_job_unknown`

- 任务不存在或不属于此会话 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `find` L195, L200

### `file_not_found`

- 会话没有绝对工作目录
- 文件不存在，或会话未记录其完整路径
- 文件或目录不存在
- [`files/media.rs`](../crates/sessiondock/src/files/media.rs) `image` L117
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `io` L75
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L243

### `file_not_referenced`

- 该路径未出现在所选会话分支中
- [`files/media.rs`](../crates/sessiondock/src/files/media.rs) `image` L105
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L196

### `launch_missing`

- 没有这个创建回执 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `failure` L90

### `media_not_found`

- 图片不存在或已过期，请重新加载会话 — [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `get` L72 → `GET /api/media/{token}`

### `nest_parent_missing`

- 目标会话不存在 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L385 → `POST /api/session/nest`

### `not_found`

- node listener serves /api only
- API route not found
- [`api/mod.rs`](../crates/sessiondock/src/api/mod.rs) `node_not_found` L406
- [`api/mod.rs`](../crates/sessiondock/src/api/mod.rs) `not_found` L414 → `ANY (fallback)`

### `record_not_found`

- 没有这个录制 — [`api/records.rs`](../crates/sessiondock/src/api/records.rs) `attach` L79, L86 → `GET /api/term/records/attach`

### `session_error`

- 会话不存在
- 子代理必须通过所属主会话访问
- 子代理不存在或不属于此主会话
- 历史页不存在或已淘汰，请重新载入会话
- 图片分页不存在或已淘汰，请重新载入会话
- 目标不是这个 Claude 会话的记录节点
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `pending_attachment_scope` L717
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `select` L264, L273, L300
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `select_main` L664
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `physical_chain` L326
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `agent_uid` L357
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `prepare` L367, L1143, L1148, L1154
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `search_version` L972, L977
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L184
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `lookup` L202
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `lookup_media` L213
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L578
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_request` L1691, L1694
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `view_meta` L1735

### `session_missing`

- 会话不存在
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `external_processes` L546
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1183 → `POST /api/session/stop`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L356 → `POST /api/session/nest`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L495 → `POST /api/session/rewind`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `session_group` L193 → `POST /api/session/group`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `star` L223 → `POST /api/session/star`

### `session_not_found`

- 会话不存在
- [`api/process_links.rs`](../crates/sessiondock/src/api/process_links.rs) `session_probe` L101 → `POST /api/session/resources/probe`
- [`api/process_links.rs`](../crates/sessiondock/src/api/process_links.rs) `session_resources` L50 → `GET /api/session/resources`

### `submission_missing`

- 报告提交不存在
- 提交不存在
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `get` L52, L79 → `GET /api/session/conversation`

### `terminal_missing`

- 指定目录中没有这个终端 host — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `scroll` L740 → `POST /api/term/scroll`

### `unknown_client`

- 这台机器没有配置该客户端 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `client_update` L912 → `POST /api/clients/update`

## 409 Conflict

### `attachment_conflict`

- 相同附件上传 ID 对应了不同文件 — [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `drop` L414

### `client_update_running`

- 该客户端正在更新，请等它结束 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `client_update` L917 → `POST /api/clients/update`

### `code`

- move_io
- error
- 节点迁移操作失败
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `remote_error` L886

### `draft_revision`

- 另一页面已更新报告草稿，未发布附件和创建诊断 — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report_inner` L249

### `file_ambiguous`

- 会话中有多个同名文件，请点击完整路径 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L219, L234

### `file_attachment_dir`

- {attachment_dir} 不是安全目录
- 附件编号对应的不是安全目录
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L2045, L2062

### `file_changed`

- 文件或父目录在读取期间已变化，请刷新后重试
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L169
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L341, L367
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `ordinary` L71
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `verify` L115, L119, L244, L249, L252
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `verify_identity` L279, L287, L289
- [`files/info.rs`](../crates/sessiondock/src/files/info.rs) `browser_info` L52
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `changed` L67
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L2102
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_entry` L1474, L1491, L1509
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1312, L1319
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1109
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1227, L1230
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate_entry` L1756, L1758
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `remove_verified` L1383, L1392
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `trash_named` L744, L746
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `verify` L1357, L1363

### `file_exists`

- 目标已存在；写入服务不会覆盖，请改名、跳过或保留两份
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `exists_error` L1073
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L795

### `file_job_not_completed`

- 只能登记已完成的上传任务 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `completed_upload` L252

### `file_job_skipped`

- 该上传因重名被跳过，没有可登记的文件 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `completed_upload` L259

### `file_keep_exhausted`

- 无法生成唯一名称
- 同名附件过多，无法分配文件名
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L2196
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `keep_exhausted` L1080

### `file_move_cross_device`

- 需要跨文件系统复制后回收源项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate_entry` L1774

### `file_upload_checksum`

- 上传数据的 SHA-256 与声明不符，已丢弃暂存数据 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `finalize` L958

### `file_upload_finished`

- 上传已结束或已取消 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L879

### `file_upload_offset`

- 重发的分块与已接收数据不一致，请刷新任务后从预期位置继续
- 上传位置不一致，请刷新任务后从预期位置继续
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L916, L924

### `freeze_instance_changed`

- 运行实例已变化或不可确认，请刷新后重试 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `freeze` L1350 → `POST /api/session/freeze`

### `launch_cwd_unknown`

- 该会话没有记录可用的工作目录；请通过创建接口明确指定目录续接 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `takeover` L813 → `POST /api/term/takeover`

### `launch_identity`

- 创建回执与附件目标实例不匹配
- 创建回执与进程实例不匹配
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `pending_attachment_cwd` L666, L675
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `check_instance` L118

### `launch_identity_declared`

- 该实例启动时已在命令行声明完整原生会话 ID；由运行时目录关联，不接受另行的操作者绑定 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `bind` L191 → `POST /api/term/bind`

### `launch_not_finished`

- 该创建实例尚未退出或取消，不能丢弃；请先停止它 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `discard` L1053 → `POST /api/term/discard`

### `launch_not_ready`

- 创建回执已被丢弃，不能上传附件 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `pending_attachment_cwd` L685

### `launch_source`

- 续接会话的数据源与请求来源不一致 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `create` L665 → `POST /api/term/create`

### `media_changed`

- 图片文件已变化，请重新加载会话 — [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `authorize` L30, L51

### `media_native_changed`

- 原生图片来源已变化，请重新加载会话 — [`media/native_media.rs`](../crates/sessiondock/src/media/native_media.rs) `changed` L163

### `move_cancelled`

- 正在取消操作
- 本次移动已撤回，请重新查看清单
- 源会话已变化，本次移动已撤回，请重新查看清单
- 操作已取消，源会话保留
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `execute` L470
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L569, L721
- [`transfer/coordination.rs`](../crates/sessiondock/src/transfer/coordination.rs) `check` L80

### `move_cleanup`

- (dynamic) — [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `retire_source` L329, L336, L361, L377, L390 → `POST /api/session/transfer/retire`

### `move_cleanup_pending`

- 移动已提交，服务端正在重试源端清理：{}
- 目标已可继续；源端清理待重试：{}
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L741, L780

### `move_conflict`

- 操作已绑定其他迁移目标
- 同机操作只支持生成新身份的复制
- 目标操作并非迁移接收记录
- 目标已存在不同的迁移操作
- 同一 rollout 身份对应多份文件
- rollout 身份重复
- 暂存目录已存在
- 目标原生历史或当前版本不同
- 新身份已存在于原生数据库
- 目标存在源端没有的原生历史
- 目标原生元数据或当前版本不同
- 目标已有不同的项目或分组关联
- 目标历史不是同一物理历史的完整前缀
- 保留身份复制需要另一台机器
- 目标文件执行权限不同
- 目标文件已存在
- 目标会话显示设置不同且历史没有延长
- 目标会话附属关系不同
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `cancel` L234
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `progress` L149
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L503, L588, L626
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `receive_bundle` L551
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L197
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stage` L453
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `stage` L919
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `insert_with_prefix` L712, L732
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_copy` L493, L507
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_prefix` L549, L570
- [`transfer/prefix.rs`](../crates/sessiondock/src/transfer/prefix.rs) `conflict` L20
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `execute` L706, L749, L758, L797, L805

### `move_cwd_mismatch`

- 两端工作目录路径不同
- 工作目录或外部依赖内容不同
- [`transfer/environment.rs`](../crates/sessiondock/src/transfer/environment.rs) `compare` L233, L239

### `move_cwd_missing`

- 工作目录必须是绝对路径
- 工作目录不存在
- 目标工作目录不存在
- 会话的工作目录不存在
- [`transfer/environment.rs`](../crates/sessiondock/src/transfer/environment.rs) `capture` L99, L105
- [`transfer/environment.rs`](../crates/sessiondock/src/transfer/environment.rs) `recheck` L215
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `plan_copy` L366

### `move_format`

- 未知的操作类型
- 未知的撤回步骤
- 迁移包缺少长度
- 迁移包长度无效
- 原生历史记录必须是对象
- 历史缺少 session_meta
- history_base 缺少 rollout ID
- history_base 字节边界无效
- 原生历史 payload 必须是对象
- 父历史偏移不是完整记录边界
- 未知的计划版本
- 父历史边界超出文件
- 父历史 ordinal 与字节边界不一致
- 缺少原生记录
- 身份替换意外改变了原生 JSON 结构
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `abort_move` L206 → `POST /api/session/transfer/abort`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `plan` L88 → `POST /api/session/clone/plan`
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `stream` L832, L836
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `invalid` L39
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L218, L233, L237
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `rewrite` L365, L386
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `rows` L122
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stage` L438, L492, L502
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `rewrite` L783
- [`transfer/json_bytes.rs`](../crates/sessiondock/src/transfer/json_bytes.rs) `shape` L36
- [`transfer/mod.rs`](../crates/sessiondock/src/transfer/mod.rs) `from` L52

### `move_group_incomplete`

- 整组存在未解析依赖
- 缺少被引用的物理 rollout
- 未映射的 {key}: {id}
- 未解析的父历史边界
- 未映射的 {kind:?}: {id}
- 父历史缺失
- 物理历史存在循环或缺失依赖
- 工具引用未映射的子代理: {id}
- ；
- 原生子代理图存在组外成员
- 原生项目或分组关联缺失
- 原生数据库未映射的 {field}
- 当前 rollout 不在文件清单中
- 数据库子代理父身份未映射
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L162, L306
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `remap` L343
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `rewrite` L382, L399
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stage` L488, L551
- [`transfer/codex_tools.rs`](../crates/sessiondock/src/transfer/codex_tools.rs) `thread` L9
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `build` L547
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `capture` L207, L243
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `mapped` L272
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `rewrite` L375, L399

### `move_group_unsupported`

- 此节点未启用会话整组复制
- 源机器未配置回收站
- 目标版本不支持迁移归属交接
- 目标未配置本组所需的会话来源
- 此暂存适配器只处理 Codex；未发布移动或克隆能力
- 文件适配器仅处理 Claude 和 Grok
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `plan` L91 → `POST /api/session/clone/plan`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `retire_source` L287 → `POST /api/session/transfer/retire`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `service` L34
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L670
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `bundle_roots` L128
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L177
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `build` L570

### `move_identity`

- 无法确定原生 rollout 身份
- 工具调用 ID 对应多个工具
- 计划身份映射缺失、冲突或格式无效
- 缺少目标历史
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L185, L251
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `uuid` L95
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `validate` L590
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `member_target_uid` L554

### `move_inventory`

- (dynamic)
- [`bin/sessiondock-transfer.rs`](../crates/sessiondock/src/bin/sessiondock-transfer.rs) `derive` L85
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `outside_references` L243
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `group` L282

### `move_native_unsupported`

- 原生元数据不是 UTF-8
- 原生元数据包含尚未适配的二进制字段
- 无法识别 {name} 的所属线程
- 分页历史需要原生状态库及历史投影，当前未找到完整数据库
- 无法唯一定位原生数据库游标对应的物理历史
- 原生 item_json 缺失
- 原生投影身份未映射: {id}
- 目标表不存在
- 源端原生表不存在
- 回滚所需表缺失
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `boundary` L317
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `capture` L260
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `extend_identities` L951
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `insert_with_prefix` L689
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_copy` L482
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_prefix` L533
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `read` L106
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `retire` L626
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `rewrite` L419, L429
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `rollback` L831
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `value` L48, L51

### `move_node_unavailable`

- 源机器不可用
- 迁移节点不可用
- 目标机器不可用
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `abort` L327, L330
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `network` L883
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `progress` L169, L172
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `reconcile` L292, L295
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L529, L532, L535, L538

### `move_path`

- 原生文件位于配置根目录之外
- 文件名无效
- rollout 文件名无效
- 暂存文件必须是相对路径
- 暂存目录不能位于源会话根目录内
- 工作目录扫描越界
- 存储核对标识无效
- 会话文件不在原生根目录内
- 无效的会话相对路径
- 会话不在原生根目录中
- 无效的暂存目标路径
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L290
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `relative` L410, L413, L428
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stage` L445
- [`transfer/environment.rs`](../crates/sessiondock/src/transfer/environment.rs) `path` L263
- [`transfer/environment.rs`](../crates/sessiondock/src/transfer/environment.rs) `walk` L203
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `build` L627, L633
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `member_target` L883
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `rewrite` L718
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `stage` L927, L954, L964

### `move_plan_stale`

- 复制清单与所选会话不一致
- 迁移标识无效
- 操作与源会话不符
- 迁移操作与所选会话不符
- 暂存文件已变化
- 导出期间暂存文件发生变化
- 原生历史末行尚未写完
- 读取期间历史文件发生变化
- 读取期间历史文件被替换
- 历史身份与索引快照不同
- 历史文件与已确认计划不同
- 暂存期间源历史发生变化
- 工作目录或外部依赖在核对期间发生变化
- 原生文件在计划后发生变化
- 暂存期间源文件发生变化
- 源端内容或剩余引用已变化，保留文件等待清理
- 此操作不是同机复制
- 此操作不是迁移源
- 迁移源尚未完成导出
- 此操作不是迁移接收记录
- 源端执行归属尚未切换
- 原生数据库结构已变化
- 源端原生记录已变化，保留清理现场
- 目标原生数据在发布前发生变化
- 目标前缀在备份时发生变化
- 复制记录不存在
- 会话组关联已变化，请重新查看复制清单
- 会话历史已变化，请重新查看复制清单
- 源会话显示设置已变化，请重新查看复制清单
- 会话附属文件已变化
- 原生会话元数据已变化，请重新查看复制清单
- 目标复用文件已变化
- 复制暂存数据已变化
- 目标前缀在发布前发生变化
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `cancel_clone` L149 → `POST /api/session/clone/cancel`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `clone_progress` L129 → `POST /api/session/clone/progress`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `execute` L414 → `POST /api/session/clone`
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `abort` L323, L345
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `cancel` L250
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `path` L94
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `progress` L166, L189
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `reconcile` L289
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L526, L575, L642
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `bundle_manifest` L207
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `export_bundle` L264
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L220
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `rows` L111
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stable_read` L142, L151
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stage` L477, L559
- [`transfer/environment.rs`](../crates/sessiondock/src/transfer/environment.rs) `stale` L81
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `rewrite` L710
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `stage` L904, L992
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `abort_local` L119
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `activate_target` L157
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `changed` L19
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `retire_source` L357
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `switch_source` L140, L146
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `insert_with_prefix` L691, L702
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_copy` L484
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_prefix` L535
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `retire` L630
- [`transfer/prefix.rs`](../crates/sessiondock/src/transfer/prefix.rs) `prepare` L176
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `execute` L822, L831, L860
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `load` L265
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `recheck` L493, L500, L512, L532, L536

### `move_platform`

- 跨机器迁移目前只支持 Linux
- 迁移链接要求 Unix
- 此平台不支持迁移符号链接
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `bundle_manifest` L138
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `receive_bundle` L633, L666
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `validate_bundle` L314
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `stage` L968
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `execute` L840

### `move_recovery_required`

- 迁移结果缺失
- 本机复制结果尚未确认
- 目标复制尚未完成
- 存储核对文件已变化，保留现场
- 执行归属已交接，不能撤回；请继续完成移动
- 源端尚未记录撤回决定
- 目标已开放继续，不能撤回
- 目标历史尚未验证完成
- 迁移序号已耗尽
- 数据库复制凭据不匹配
- 合并后的数据库记录已变化，保留现场
- 克隆数据库记录已变化，保留现场等待恢复
- 本次导入的项目或分组已被其他记录引用，保留现场
- 目标原文件备份发生变化
- 合并后的目标文件已变化，保留现场
- 目标恢复文件已变化
- 会话组有尚未恢复的复制操作
- 目标合并临时文件已变化，保留现场
- 克隆文件已变化或不属于本次操作，保留现场等待恢复
- 复制操作需要恢复或重新发起
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `send` L173 → `POST /api/session/conversation/send`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `resolve_resume` L488
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L523, L605, L756
- [`transfer/environment.rs`](../crates/sessiondock/src/transfer/environment.rs) `remove` L299
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `abort_source` L55, L65
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `abort_target` L94
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `activate_target` L166
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `next_ownership_sequence` L234
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `rollback` L801, L812, L847, L862, L868
- [`transfer/prefix.rs`](../crates/sessiondock/src/transfer/prefix.rs) `check_restore` L199, L213
- [`transfer/prefix.rs`](../crates/sessiondock/src/transfer/prefix.rs) `restore` L230
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `cleanup_markers` L627
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `execute` L698
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `plan_copy` L329
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `rollback` L658

### `move_reference_unsupported`

- code-mode 中存在无法静态解析的会话引用
- 子代理工具参数不是 JSON 字符串
- 子代理工具参数不是 JSON
- 子代理工具文本包含无法结构化解析的会话引用
- 子代理工具结果包含无法识别的内容项
- 子代理工具结果不是文本或原生内容数组
- ；
- [`transfer/code_mode.rs`](../crates/sessiondock/src/transfer/code_mode.rs) `failure` L79
- [`transfer/codex_tools.rs`](../crates/sessiondock/src/transfer/codex_tools.rs) `output` L64, L81, L90
- [`transfer/codex_tools.rs`](../crates/sessiondock/src/transfer/codex_tools.rs) `rewrite` L42, L48
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `plan_copy` L353

### `move_root_mismatch`

- 两端 CLI 根目录路径不同 — [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `validate_bundle` L334

### `move_session_locked`

- 会话正在复制或等待恢复
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `send` L175 → `POST /api/session/conversation/send`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `resolve_resume` L490

### `move_session_running`

- 会话仍在运行：{} — [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `stopped` L56

### `move_shared_storage`

- 两台机器共享会话存储，不能移动文件 — [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `validate_bundle` L347

### `move_verify`

- 克隆后的历史关系不完整
- 克隆仍引用源组身份
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `execute` L894, L910

### `nest_parent_cycle`

- 不能附属到自己的子会话下面 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L411 → `POST /api/session/nest`

### `not_found`

- 会话不在当前索引中 — [`transfer/group.rs`](../crates/sessiondock/src/transfer/group.rs) `derive_cached` L64

### `report_result_unknown`

- 此报告已开始保存，请核对诊断和处理会话；原输入保留，不会重复创建报告
- 诊断已开始保存，输入保留，不会重复创建
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report_inner` L239
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `get` L59 → `GET /api/session/conversation`

### `run_state_unknown`

- 该会话的受管实例运行状态未知（{reason}），未发送任何停止指令；未知不等于已退出，请稍后重试或检查宿主 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1288 → `POST /api/session/stop`

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
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `native_scope` L119
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `select` L270, L293, L301
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `sid` L217
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `build` L762
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `history_parent` L469
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `rollout_parent` L216
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `select_main` L661
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `sid` L451
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `thread` L284
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `thread_from` L312
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `native_identity` L396
- [`sessions/media_projection.rs`](../crates/sessiondock/src/sessions/media_projection.rs) `project_range` L40
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `invalid_range` L20
- [`sessions/native_media.rs`](../crates/sessiondock/src/sessions/native_media.rs) `authorized_reader` L62, L64, L75, L84
- [`sessions/native_media.rs`](../crates/sessiondock/src/sessions/native_media.rs) `finish` L29, L32
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `history_page` L457
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `history_page_body` L479
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `media_page` L540, L543, L549, L555
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `validate_grant_scope` L429
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L68, L86
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `prepare` L161, L170
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `replay_changed` L21
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `native_identity` L97
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L581, L584
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_scope` L1934
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_source` L513
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_request` L1699

### `shell_env_unconfigured`

- 本机没有配置登录环境检查，不能从页面重启后端 — [`api/shell_env.rs`](../crates/sessiondock/src/api/shell_env.rs) `restart` L22 → `POST /api/shell-env/restart`

### `stale_build`

- 页面版本已过期，请刷新后再保存偏好 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `validate` L45

### `stop_superseded`

- 该回滚分支已不是当前运行分支，未停止共享的子会话 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1191 → `POST /api/session/stop`

### `takeover_superseded`

- 该回滚分支的运行实例已转移到更新的子会话，请先处理当前子会话 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `takeover` L774 → `POST /api/term/takeover`

### `terminal_binding_unavailable`

- 无法确认会话与终端实例的唯一关联；请刷新，不会降级按名称连接。 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `binding_unavailable` L449

## 410 Gone

### `session_error`

- 历史页已过期，请重新载入会话
- 图片分页已过期，请重新载入会话
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L190

## 413 Payload Too Large

### `body_too_large`

- 浏览器诊断请求体过大
- 缺陷报告请求体过大
- 文件引用请求体过大
- 文件操作请求体最多 512 KiB
- 创建请求体过大
- 停止请求体过大
- 偏好请求体超过大小限制
- 终端预约请求体过大
- 请求体过大
- [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L52 → `POST /api/audit/browser`
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report_inner` L130
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `action` L526 → `POST /api/session/files/action`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `resolve` L125 → `POST /api/session/resolve-files`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `parse_body` L103
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1156 → `POST /api/session/stop`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `invalid` L109
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim_inner` L167
- [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L107

### `file_image_budget`

- 单张磁盘图片超过 32 MiB 读取上限 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `new` L95

### `file_items_limit`

- 一次最多操作 {MAX_WRITE_ITEMS} 个项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `items` L411

### `file_job_too_large`

- 单个上传最多 {} 字节 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L768

### `file_raw_budget`

- 原始打开超过 32 MiB，请使用预览或下载
- 原始打开超过 32 MiB，请使用文本预览或下载
- [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `read` L406, L424

### `file_upload_chunk_too_large`

- 单个分块最多 {limit} 字节
- 单个分块最多 {} 字节
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L610 → `POST /api/session/files/upload`
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L860

### `file_upload_overflow`

- 分块超过声明的上传大小 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L891

### `file_upload_too_large`

- 单个附件不能超过 512 MiB
- 单个附件不能超过 {} MB
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `upload` L302 → `POST /api/session/conversation/attachment`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload_attachment` L743
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L2015

### `session_error`

- 继承历史预算溢出
- 此历史页无法在读取预算内推进
- 此图片分页无法在读取预算内推进
- 图片分页响应超过 8 MiB 预算
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `inherit` L648
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `media_page` L560, L594
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `page_selection` L507
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `inherit` L2039

### `terminal_input_too_large`

- 终端输入请求体过大
- 单次终端输入不能超过 1 MiB
- 单次终端粘贴不能超过 1 MiB
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `send` L565, L594, L620, L632 → `POST /api/term/send`

### `too_many_events`

- 单次诊断批次事件过多 — [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L65 → `POST /api/audit/browser`

## 414 Uri Too Long

### `uri_too_long`

- 请求 URI 过长 — [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L97

## 500 Internal Server Error

### `attachment_headers_invalid`

- 附件响应头无效 — [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `staged` L563

### `bug_report_failed`

- 报告捕获任务异常退出
- 审计查询任务异常退出
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `capture` L549 → `POST /api/bug-report/capture`
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report_inner` L381

### `file_headers_invalid`

- 文件响应头无效 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `response_body` L441

### `file_worker_failed`

- 附件写入任务异常退出
- 文件读取任务异常退出
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `attachment` L716 → `POST /api/session/attachment`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `work` L94

### `live_failed`

- 进程表配对失败 — [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `assemble` L353

### `metadata_worker_failed`

- 偏好工作异常退出，请重新读取状态确认结果 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `write` L143

### `move_io`

- (dynamic)
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `abort_move` L216 → `POST /api/session/transfer/abort`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `activate_target` L275 → `POST /api/session/transfer/activate`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `bundle_manifest` L711 → `POST /api/session/transfer/manifest`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `cancel_clone` L174 → `POST /api/session/clone/cancel`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `check_bundle` L748 → `POST /api/session/transfer/check`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `execute` L438 → `POST /api/session/clone`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `export_bundle` L469, L500 → `POST /api/session/transfer/export`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `plan` L108 → `POST /api/session/clone/plan`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `receive_bundle` L610 → `POST /api/session/transfer/receive`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `release_export` L633 → `POST /api/session/transfer/release`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `reserve_export` L660 → `POST /api/session/transfer/reserve`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `retire_source` L317, L376, L389, L399 → `POST /api/session/transfer/retire`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `switch_source` L252 → `POST /api/session/transfer/switch`
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `save` L118
- [`transfer/mod.rs`](../crates/sessiondock/src/transfer/mod.rs) `from` L46

### `move_native_database`

- (dynamic) — [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `from` L39

### `reader_failed`

- 只读任务失败
- [`state.rs`](../crates/sessiondock/src/state.rs) `run` L162
- [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L137

### `runtime_encoding`

- 受控进程观察无法编码 — [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `assemble` L274

### `search_failed`

- 搜索任务失败 — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L159 → `GET /api/search`

### `session_error`

- 历史消息序列化失败
- 消息序列化失败
- 会话索引锁不可用
- 会话视图锁不可用
- 会话列表缓存锁不可用
- 历史页响应序列化失败
- 会话行不是对象
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `push` L616
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `resolve` L772
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `refresh_within` L558
- [`sessions/index/titles.rs`](../crates/sessiondock/src/sessions/index/titles.rs) `codex_name` L14
- [`sessions/index/titles.rs`](../crates/sessiondock/src/sessions/index/titles.rs) `titles` L28, L39
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `list_state` L571
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `list_view_bytes` L766
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `views` L581
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `history_page_body` L487
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `take` L267
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `validate_response` L324
- [`sessions/views/body.rs`](../crates/sessiondock/src/sessions/views/body.rs) `serialize_error` L168
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `encode_events` L120
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `view_meta` L1720

### `trash_encoding`

- 回收站结果无法编码 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `encoding` L444

### `trash_failed`

- 回收站任务失败
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `list` L529 → `GET /api/trash`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L606 → `POST /api/trash/purge`

## 501 Not Implemented

### `bug_report_disabled`

- 缺陷报告未启用：需要 SESSIONDOCK_BUG_REPORT_DIR/REPO、审计目录、终端传输和受控创建 — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `disabled` L48

### `conversation_disabled`

- 服务端会话草稿未配置，无法转交报告
- 会话保存和发送未配置
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report_inner` L184
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `enabled` L27

### `file_action_not_implemented`

- 写入服务尚未实现此文件操作：{}；不会伪造任务或成功结果 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L394

### `file_mode_not_implemented`

- 只读文件服务尚未实现此模式；不会伪造空任务或成功结果
- 文件服务尚未实现此模式
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `get` L313, L342
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `validate` L183, L188
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `unsupported` L64

### `file_trash_unconfigured`

- 未配置 SESSIONDOCK_STATE_DIR，没有回收目录；写入服务不会直接删除 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `trash_named` L702

### `files_disabled`

- 文件读取服务不可用 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `configured` L55

### `files_jobs_disabled`

- 文件写入服务未启用
- 文件写入未启用：必须显式配置 SESSIONDOCK_FILE_WRITE_ROOTS（只读目录不会隐式变为可写）
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `attachment` L660 → `POST /api/session/attachment`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `write_configured` L458

### `media_files_disabled`

- 未配置图片读取目录；不会自动访问磁盘 — [`sessions/media_projection.rs`](../crates/sessiondock/src/sessions/media_projection.rs) `project_ranges` L71

### `metadata_disabled`

- 偏好保存未配置状态目录 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `configured` L99

### `not_implemented`

- Rust 后端尚未迁移此能力：{capability}。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：受控会话创建与 pending 生命周期。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：终端后端选择。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：受管实例停止。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：会话冻结。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：进程身份验证。当前是只读开发阶段。
- [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L32 → `POST /api/audit/browser`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L949 → `POST /api/term/backend`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `enabled` L33
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `freeze` L1339, L1357 → `POST /api/session/freeze`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1151, L1199 → `POST /api/session/stop`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `configured` L39
- [`error.rs`](../crates/sessiondock/src/error.rs) `unavailable` L31

### `terminal_disabled`

- 终端传输未启用：必须显式配置隔离的 ptyhost 目录
- [`api/records.rs`](../crates/sessiondock/src/api/records.rs) `root` L26
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `enabled` L40

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
- 搜索投影不提供原生操作范围
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `chain` L340
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `check_cut` L529, L532
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `dependencies` L317
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `history_link` L508, L517, L522
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `history_parent` L227
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `inherit` L640
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `native_scope` L109, L113
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `ownership` L248, L257
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `parse_prefix` L668, L688, L700, L703
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `resolve` L716
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `sid` L213, L218
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `unsupported` L48
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `chain` L540
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `check_cut` L510, L511
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `history_link` L729, L738, L742
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `history_parent` L473
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `native_scope` L672, L675
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `new` L380, L383
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `ownership` L494
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `rollout_parent` L215
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `sid` L445, L452
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `unsupported` L71
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `physical_chain` L337
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `thread` L264, L267, L277, L285
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `blank` L170
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `native_identity` L390, L403
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `grok_native_identity` L31
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `native_identity` L65, L91, L104
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `summary_native_identity` L50
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `build` L1824, L1832
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L573, L586
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `committed_records` L556
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `inherit` L2033
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_scope` L1924, L1928
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `parse_prefix` L2080

## 503 Service Unavailable

### `attachment_storage`

- 附件暂存目录创建失败
- 附件目录权限设置失败
- 附件暂存失败
- 附件写入失败
- 附件同步失败
- 附件发布到暂存区失败
- 附件目录同步任务失败
- 附件目录同步失败
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `drop` L363, L388, L403, L427, L439, L446
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `upload` L332, L344 → `POST /api/session/conversation/attachment`

### `bug_report_failed`

- 报告任务异常退出，输入保留 — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report` L111 → `POST /api/bug-report`

### `cancelled`

- 观察已取消 — [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L127

### `conversation_send`

- 发送任务异常退出 — [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `send` L194 → `POST /api/session/conversation/send`

### `conversation_start`

- 启动任务异常退出，输入保留 — [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `restart` L280 → `POST /api/session/conversation/restart`

### `conversation_storage`

- 草稿保存任务失败
- 草稿清理任务失败
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `save` L148
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `forget_discarded_launch` L1108

### `cwd_check_failed`

- 启动目录检查失败 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `prepare_cwd` L471

### `file_attachment_id`

- 无法分配附件目录编号 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L2093

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
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1330
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1233

### `file_random_unavailable`

- 系统随机源不可用，不能分配任务编号 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `random_id` L139

### `file_trash_id`

- 无法分配回收目录编号 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `fresh_private_child` L1137

### `freeze_failed`

- 无法冻结或恢复会话进程：{error} — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `freeze` L1362 → `POST /api/session/freeze`

### `freeze_resume_failed`

- 停止前无法恢复冻结进程：{error}；请先恢复运行后重试 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1248 → `POST /api/session/stop`

### `lifecycle_response`

- 创建状态序列化失败 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `serialize` L262

### `media_busy`

- 图片处理繁忙，请稍后重试 — [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `read_media` L48

### `media_unavailable`

- 图片服务暂不可用 — [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `file` L98

### `metadata_clock_invalid`

- 系统时钟无效，不能记录固定时间 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L471 → `POST /api/session/rewind`

### `process_control_unavailable`

- 无法结束外部会话进程：{error:?}
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1222 → `POST /api/session/stop`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `takeover` L801 → `POST /api/term/takeover`

### `process_scan_unavailable`

- 无法核对会话进程：{error}
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `external_processes` L576
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `external_scan_result` L47

### `reader_busy`

- 读取服务已关闭 — [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L128

### `records_unreadable`

- 无法读取录制目录 — [`api/records.rs`](../crates/sessiondock/src/api/records.rs) `unreadable` L35

### `runtime_closed`

- 进程观察服务已关闭 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `frozen_liveness` L79

### `runtime_unavailable`

- 受控 host 目录不可用或超出观察预算
- 无法读取受管进程状态
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `unavailable` L487
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `frozen_liveness` L91

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
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `parse_prefix` L681
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `check_cut` L512
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `discover` L804, L811
- [`sessions/index/names.rs`](../crates/sessiondock/src/sessions/index/names.rs) `error` L23
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `file_stamp` L1390, L1394
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `prepare` L1160
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `stamp` L1383
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `allocation_error` L23
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `changed` L14
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `read_error` L17
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L181
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `issue` L120, L134, L144
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `link_media` L158
- [`sessions/records/native_records.rs`](../crates/sessiondock/src/sessions/records/native_records.rs) `io_error` L76
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L89, L94, L96, L100
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `committed_records` L552
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `parse_candidate_retaining` L669, L674, L677
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `read_bounded_limit` L612

### `shutdown`

- 服务正在关闭
- [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L40 → `POST /api/audit/browser`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `admission` L64
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `write_admission` L468
- [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `get` L79 → `GET /api/media/{token}`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `write` L127
- [`api/records.rs`](../crates/sessiondock/src/api/records.rs) `attach` L100 → `GET /api/term/records/attach`
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `observe` L503
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `shared` L446
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L340 → `GET /api/term/attach`
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim_inner` L195
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `send` L661 → `POST /api/term/send`

### `watch_closed`

- 会话观察已关闭，请重试 — [`observe.rs`](../crates/sessiondock/src/observe.rs) `closed` L208

### `服务正在关闭`

- (dynamic) — [`state.rs`](../crates/sessiondock/src/state.rs) `admit` L105
