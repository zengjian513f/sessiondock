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

- 未知终端后端 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L938 → `POST /api/term/backend`

### `backend_unsupported`

- Rust 后端不支持 tmux；新建会话只能由 ptyhost 托管 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L933 → `POST /api/term/backend`

### `bad_body`

- bad body
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `capture` L504, L506 → `POST /api/bug-report/capture`
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report_inner` L136, L140, L197, L210, L265, L375, L456

### `create_cwd_failed`

- 创建启动目录失败：{error} — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `prepare_cwd` L434

### `file_absolute_path_required`

- 目录导航需要绝对路径
- 需要绝对路径
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `absolute_navigation` L407
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `mutation_path` L454

### `file_action_invalid`

- 未知文件操作 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L388

### `file_attachment_id`

- 附件目录编号无效 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L1986

### `file_conflict_invalid`

- 无效的重名处理方式 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `parse` L78

### `file_cwd_unavailable`

- 相对引用需要有效的所选会话工作目录 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `absolute_path` L436

### `file_directory_anchor_required`

- 文件操作入口必须是会话提及的目录
- 文件浏览入口必须是会话提及的目录
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `anchor` L503
- [`files/grants.rs`](../crates/sessiondock/src/files/grants.rs) `directory_grant` L138
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `target` L268
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L365

### `file_directory_required`

- 此操作需要目录
- 请选择目录浏览
- 目标必须是目录
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `directory` L298
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `list` L519
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `directory` L294

### `file_field_required`

- 缺少必需字段 {field} — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `required` L994

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

- 不能把目录放进自身或子目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L570

### `file_name_invalid`

- 名称无效：须为单个路径组件，不能包含 /、\、控制字符或为 . 与 .. — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `validate_name` L1005

### `file_parent_not_directory`

- 路径的父组件不是目录
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L332
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `entry` L316

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
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `delete` L652
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `move_recycle_entry` L1565
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L566

### `file_path_invalid`

- 需要绝对路径
- 文件目录需要标准绝对路径
- 路径与打开的文件系统卷不一致
- 路径组件无效
- 路径为空、过长或包含空字符
- 文件引用不能是 URL
- 无效的目录引用
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `absolute_path` L420
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L143
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L311, L316
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `validate_path_text` L386
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `volume_root` L101
- [`files/grants.rs`](../crates/sessiondock/src/files/grants.rs) `directory_grant` L130
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `entry` L310

### `file_paths_required`

- 请先选择项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `items` L394

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
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1259
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1194

### `file_root_not_directory`

- 文件入口必须是既有目录 — [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L160

### `file_same_path`

- 源路径和目标相同
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate` L577
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `rename` L518

### `file_scope_invalid`

- 文件访问需要已解析的有效会话视图 — [`files/references.rs`](../crates/sessiondock/src/files/references.rs) `validate_scope` L118

### `file_sha256_invalid`

- sha256 须为 64 位十六进制 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `parse_sha256` L1028

### `file_single_item_required`

- 请选择一个项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `rename` L507

### `file_upload_content_type`

- 上传分块必须使用 application/octet-stream — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L587 → `POST /api/session/files/upload`

### `file_upload_empty`

- 附件为空
- 附件为空或缺少 Content-Length
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `drop` L396
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L1966

### `file_upload_modified_invalid`

- 无效的修改时间 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L762

### `file_upload_offset_invalid`

- offset 必须是非负整数 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L575 → `POST /api/session/files/upload`

### `file_upload_size_invalid`

- 无效的上传大小 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L748

### `file_write_limits_invalid`

- 写入预算必须为正数 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `open` L167

### `invalid_attach`

- 终端连接参数无效 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L272 → `GET /api/term/attach`

### `invalid_audit_request`

- (dynamic) — [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L60 → `POST /api/audit/browser`

### `invalid_bug_report`

- (dynamic) — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `invalid` L92

### `invalid_claim`

- 终端预约请求格式无效 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim_inner` L173

### `invalid_file_request`

- 需要有效的会话、分支和文件参数 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `invalid` L47

### `invalid_final_screen`

- 最终画面参数无效 — [`api/final_screen.rs`](../crates/sessiondock/src/api/final_screen.rs) `get` L36 → `GET /api/term/final`

### `invalid_history`

- 历史行参数无效
- 历史行范围无效
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `grid_history` L511, L513 → `GET /api/term/grid/history`

### `invalid_launch_request`

- 创建请求格式或身份字段无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `invalid` L94

### `invalid_metadata_batch`

- 需要有效会话 uid — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `visibility` L270 → `POST /api/sessions/fork-visibility`

### `invalid_metadata_request`

- 需要有效的偏好 JSON 请求和布尔状态 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `invalid` L116

### `invalid_metadata_uid`

- 需要有效的会话 uid
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L359 → `POST /api/session/nest`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L478 → `POST /api/session/rewind`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `star` L233 → `POST /api/session/star`

### `invalid_path`

- 启动目录路径无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `complete_dir` L837 → `GET /api/term/complete-dir`

### `invalid_purge`

- 需要 id/ids，或 all:true / days:N（二者不能同时给出） — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L610 → `POST /api/trash/purge`

### `invalid_query`

- 查询参数无效
- force 必须为 0/1
- [`api/read.rs`](../crates/sessiondock/src/api/read.rs) `list` L57 → `GET /api/sessions`
- [`api/read.rs`](../crates/sessiondock/src/api/read.rs) `query_error` L45
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_session` L413, L415
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `list` L532 → `GET /api/trash`

### `invalid_rewind_target`

- target 必须是 Claude 记录节点 ID，或 null 表示取消固定 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L485 → `POST /api/session/rewind`

### `invalid_scope`

- (dynamic)
- [`api/process_links.rs`](../crates/sessiondock/src/api/process_links.rs) `session_probe` L100 → `POST /api/session/resources/probe`
- [`api/process_links.rs`](../crates/sessiondock/src/api/process_links.rs) `session_resources` L39 → `GET /api/session/resources`

### `invalid_scroll`

- 终端滚动请求格式无效 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `scroll` L726 → `POST /api/term/scroll`

### `invalid_search_query`

- (dynamic) — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L120 → `GET /api/search`

### `invalid_stop_request`

- 停止请求格式或会话 UID 无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `invalid_stop` L1103

### `invalid_terminal_input`

- (dynamic) — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `invalid_input` L485

### `invalid_trash_request`

- 请求体无效: {} — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `invalid` L43

### `invalid_uid`

- 会话 uid 无效
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_batch` L487 → `POST /api/sessions/delete`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_session` L422

### `launch_adapter`

- 来源没有唯一的可续接 CLI 配置
- 来源没有唯一的已配置 CLI
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `select_entry` L336, L341

### `launch_model`

- 模型或推理强度名称无效 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `create` L680 → `POST /api/term/create`

### `nest_conflict`

- 跨机器父会话信息无效 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L395 → `POST /api/session/nest`

### `nest_parent_missing`

- 目标会话缺少来源或会话 id — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L429 → `POST /api/session/nest`

### `nest_parent_node`

- 只能附属到同一台机器上的会话 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L420 → `POST /api/session/nest`

### `nest_parent_self`

- 不能附属到自己下面 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L404 → `POST /api/session/nest`

### `no_sessions`

- 没有选中任何会话 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `delete_batch` L498 → `POST /api/sessions/delete`

### `session_error`

- 历史页游标格式无效
- 图片分页游标格式无效
- 历史分页恢复检查点无效
- 历史分页恢复范围缺失
- append 和 window 只接受 0 或 1
- 这不是 Claude 主会话
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `refresh_within` L537
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L194
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `lookup_or_resume` L245, L258
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L527
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_message_query` L487

### `websocket_required`

- 需要有效的 WebSocket 升级请求 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L326 → `GET /api/term/attach`

## 403 Forbidden

### `cross_origin`

- 拒绝跨源 API 请求 — [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L91

### `cross_site`

- 拒绝跨站 API 请求 — [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L80

### `file_forbidden`

- 操作系统不允许访问此文件或目录 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `io` L77

### `file_private_dir_invalid`

- {name} 必须是目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1080

### `file_private_dir_permissions`

- {name} 目录必须仅所有者可访问 (0700) — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1093

### `file_root_immutable`

- 不能修改根目录、用户主目录或文件管理器的数据目录 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `guard` L279

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
- [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `authorize` L20
- [`media/native_media.rs`](../crates/sessiondock/src/media/native_media.rs) `materialize_native` L256

### `nest_remote_hub`

- 跨机器附属需要通过 Hub 验证 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L372 → `POST /api/session/nest`

### `node_auth_required`

- node authentication required
- 进程关联仅接受经过认证的 Hub
- 机器探测仅接受经过认证的 Hub
- [`api/node_auth.rs`](../crates/sessiondock/src/api/node_auth.rs) `auth_required` L73
- [`api/process_links.rs`](../crates/sessiondock/src/api/process_links.rs) `node_probe` L78 → `POST /api/resources/probe`
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
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `file_stamp` L1329
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `trusted_path` L1384
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L204
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `lookup_or_resume` L247
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L71
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `prepare` L154, L158
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_source` L479

### `terminal_disabled`

- (dynamic) — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `terminal_off` L56

## 404 Not Found

### `attachment_missing`

- 附件暂存字节已不在服务端 — [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `staged` L507

### `entry_not_found`

- 回收站条目不存在 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L633 → `POST /api/trash/purge`

### `file_job_unknown`

- 任务不存在或不属于此会话 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `find` L195, L200

### `file_not_found`

- 会话没有绝对工作目录
- 文件不存在，或会话未记录其完整路径
- 文件或目录不存在
- [`files/media.rs`](../crates/sessiondock/src/files/media.rs) `image` L101
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `io` L75
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L243

### `file_not_referenced`

- 该路径未出现在所选会话分支中
- [`files/media.rs`](../crates/sessiondock/src/files/media.rs) `image` L89
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L196

### `final_screen_not_found`

- 没有这份最终画面 — [`api/final_screen.rs`](../crates/sessiondock/src/api/final_screen.rs) `get` L48 → `GET /api/term/final`

### `launch_missing`

- 没有这个创建回执 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `failure` L90

### `media_not_found`

- 图片不存在或已过期，请重新加载会话 — [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `get` L72 → `GET /api/media/{token}`

### `nest_parent_missing`

- 目标会话不存在 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L411 → `POST /api/session/nest`

### `not_found`

- node listener serves /api only
- API route not found
- [`api/mod.rs`](../crates/sessiondock/src/api/mod.rs) `node_not_found` L412
- [`api/mod.rs`](../crates/sessiondock/src/api/mod.rs) `not_found` L420 → `ANY (fallback)`

### `session_error`

- 会话不存在
- 子代理必须通过所属主会话访问
- 子代理不存在或不属于此主会话
- 历史页不存在或已淘汰，请重新载入会话
- 图片分页不存在或已淘汰，请重新载入会话
- 目标不是这个 Claude 会话的记录节点
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `pending_attachment_scope` L705
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `select_main` L682
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `physical_chain` L313
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `agent_uid` L355
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `prepare` L365, L1076, L1081, L1087
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `search_version` L911, L916
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L202
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `lookup` L220
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `lookup_media` L231
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L535
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_request` L1584, L1587
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `view_meta` L1628

### `session_missing`

- 会话不存在
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `external_processes` L526
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1149 → `POST /api/session/stop`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L382 → `POST /api/session/nest`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L523 → `POST /api/session/rewind`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `session_group` L214 → `POST /api/session/group`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `star` L245 → `POST /api/session/star`

### `session_not_found`

- 会话不存在
- [`api/process_links.rs`](../crates/sessiondock/src/api/process_links.rs) `session_probe` L111 → `POST /api/session/resources/probe`
- [`api/process_links.rs`](../crates/sessiondock/src/api/process_links.rs) `session_resources` L50 → `GET /api/session/resources`

### `submission_missing`

- 报告提交不存在
- 提交不存在
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `get` L52, L79 → `GET /api/session/conversation`

### `terminal_missing`

- 指定目录中没有这个终端 host — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `scroll` L733 → `POST /api/term/scroll`

### `unknown_client`

- 这台机器没有配置该客户端 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `client_update` L883 → `POST /api/clients/update`

## 409 Conflict

### `attachment_conflict`

- 相同附件上传 ID 对应了不同文件 — [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `drop` L414

### `client_update_running`

- 该客户端正在更新，请等它结束 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `client_update` L888 → `POST /api/clients/update`

### `code`

- move_io
- error
- 节点迁移操作失败
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `remote_error` L1199

### `draft_revision`

- 另一页面已更新报告草稿，未发布附件和创建诊断 — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report_inner` L249

### `file_ambiguous`

- 会话中有多个同名文件，请点击完整路径 — [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `resolve_reference` L219, L234

### `file_attachment_dir`

- {attachment_dir} 不是安全目录
- 附件编号对应的不是安全目录
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L2003, L2020

### `file_changed`

- 文件或父目录在读取期间已变化，请刷新后重试
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open` L169
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `open_target` L341, L367
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `ordinary` L71
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `verify` L115, L119, L244, L249, L252
- [`files/boundary.rs`](../crates/sessiondock/src/files/boundary.rs) `verify_identity` L279, L287, L289
- [`files/info.rs`](../crates/sessiondock/src/files/info.rs) `browser_info` L52
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `changed` L67
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L2060
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_entry` L1454, L1471, L1492
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1292, L1299
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `private_subdir` L1089
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1207, L1210
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate_entry` L1714, L1716
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `remove_verified` L1363, L1372
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `trash_named` L726, L728
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `verify` L1337, L1343

### `file_exists`

- 目标已存在；写入服务不会覆盖，请改名、跳过或保留两份
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `exists_error` L1053
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L776

### `file_job_not_completed`

- 只能登记已完成的上传任务 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `completed_upload` L238

### `file_job_skipped`

- 该上传因重名被跳过，没有可登记的文件 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `completed_upload` L245

### `file_keep_exhausted`

- 无法生成唯一名称
- 同名附件过多，无法分配文件名
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L2154
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `keep_exhausted` L1060

### `file_move_cross_device`

- 需要跨文件系统复制后回收源项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `relocate_entry` L1732

### `file_upload_checksum`

- 上传数据的 SHA-256 与声明不符，已丢弃暂存数据 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `finalize` L938

### `file_upload_finished`

- 上传已结束或已取消 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L860

### `file_upload_offset`

- 重发的分块与已接收数据不一致，请刷新任务后从预期位置继续
- 上传位置不一致，请刷新任务后从预期位置继续
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L896, L904

### `freeze_instance_changed`

- 运行实例已变化或不可确认，请刷新后重试 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `freeze` L1316 → `POST /api/session/freeze`

### `launch_cwd_unknown`

- 该会话没有记录可用的工作目录；请通过创建接口明确指定目录续接 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `takeover` L794 → `POST /api/term/takeover`

### `launch_identity`

- 创建回执与附件目标实例不匹配
- 创建回执与进程实例不匹配
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `pending_attachment_cwd` L654, L663
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `check_instance` L118

### `launch_identity_declared`

- 该实例启动时已在命令行声明完整原生会话 ID；由运行时目录关联，不接受另行的操作者绑定 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `bind` L191 → `POST /api/term/bind`

### `launch_not_finished`

- 该创建实例尚未退出或取消，不能丢弃；请先停止它 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `discard` L1019 → `POST /api/term/discard`

### `launch_not_ready`

- 创建回执已被丢弃，不能上传附件 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `pending_attachment_cwd` L673

### `launch_source`

- 续接会话的数据源与请求来源不一致 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `create` L646 → `POST /api/term/create`

### `media_changed`

- 图片文件已变化，请重新加载会话 — [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `authorize` L24

### `media_native_changed`

- 原生图片来源已变化，请重新加载会话 — [`media/native_media.rs`](../crates/sessiondock/src/media/native_media.rs) `changed` L163

### `move_cancelled`

- 正在取消操作
- 本次移动已撤回，请重新查看清单
- 源会话已变化，本次移动已撤回，请重新查看清单
- 操作已取消，源会话保留
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `execute` L741
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L856, L1029
- [`transfer/coordination.rs`](../crates/sessiondock/src/transfer/coordination.rs) `check` L80

### `move_cleanup`

- (dynamic) — [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `retire_source` L389, L396, L421, L437, L450 → `POST /api/session/transfer/retire`

### `move_cleanup_pending`

- 移动已提交，服务端正在重试源端清理：{}
- 目标已可继续；源端清理待重试：{}
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L1049, L1089

### `move_conflict`

- 操作已绑定其他迁移目标
- 操作已绑定其他迁移选项
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
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `cancel_inner` L472
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `execute` L724
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `progress` L358
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L775, L888, L926
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `receive_bundle` L585
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L200
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stage` L456
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `stage` L924
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `insert_with_prefix` L726, L749
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_copy` L498, L515
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_prefix` L557, L578
- [`transfer/prefix.rs`](../crates/sessiondock/src/transfer/prefix.rs) `conflict` L20
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `execute` L855, L903, L912, L951, L959

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
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `plan_copy` L453

### `move_format`

- 未知的操作类型
- 未知的撤回步骤
- 确认选项缺少会话或身份选择
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
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `abort_move` L264 → `POST /api/session/transfer/abort`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `confirm_mode` L480
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `plan` L135 → `POST /api/session/clone/plan`
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `stream` L1141, L1145
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `invalid` L39
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L221, L236, L240
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `rewrite` L368, L389
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `rows` L123
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stage` L441, L497, L507
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `rewrite` L787
- [`transfer/json_bytes.rs`](../crates/sessiondock/src/transfer/json_bytes.rs) `shape` L36
- [`transfer/mod.rs`](../crates/sessiondock/src/transfer/mod.rs) `from` L53
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `confirm_mode` L86

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
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L165, L309
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `remap` L346
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `rewrite` L385, L402
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stage` L493, L566
- [`transfer/codex_tools.rs`](../crates/sessiondock/src/transfer/codex_tools.rs) `thread` L9
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `build` L549
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `capture` L209, L245
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `mapped` L274
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `rewrite` L380, L404

### `move_group_unsupported`

- 此节点未启用会话整组复制
- 源机器未配置回收站
- 目标版本不支持迁移归属交接
- 目标未配置本组所需的会话来源
- 此暂存适配器只处理 Codex；未发布移动或克隆能力
- 文件适配器仅处理 Claude 和 Grok
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `confirm_mode` L474
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `plan` L138 → `POST /api/session/clone/plan`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `retire_source` L346 → `POST /api/session/transfer/retire`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `service` L38
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L975
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `bundle_roots` L135
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L180
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `build` L572

### `move_identity`

- 无法确定原生 rollout 身份
- 工具调用 ID 对应多个工具
- 计划身份映射缺失、冲突或格式无效
- 缺少目标历史
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L188, L254
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `uuid` L95
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `validate` L607
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `member_target_uid` L695
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `planned_target_uid` L603

### `move_inventory`

- (dynamic)
- [`bin/sessiondock-transfer.rs`](../crates/sessiondock/src/bin/sessiondock-transfer.rs) `derive` L87
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `inventory_snapshot` L359

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
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `boundary` L319
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `capture` L262
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `extend_identities` L968
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `insert_with_prefix` L703
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_copy` L487
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_prefix` L541
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `read` L106
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `retire` L634
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `rewrite` L424, L434
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `rollback` L848
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `value` L48, L51

### `move_node_unavailable`

- 源机器不可用
- 迁移节点不可用
- 目标机器不可用
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `abort` L581, L584
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `network` L1196
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `progress` L378, L381
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `reconcile` L546, L549
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L802, L805, L808, L811

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
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L293
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `relative` L413, L416, L431
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stage` L448
- [`transfer/environment.rs`](../crates/sessiondock/src/transfer/environment.rs) `path` L263
- [`transfer/environment.rs`](../crates/sessiondock/src/transfer/environment.rs) `walk` L203
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `build` L629, L635
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `member_target` L888
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `rewrite` L720
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `stage` L932, L962, L972

### `move_plan_stale`

- 复制清单与所选会话不一致
- 迁移标识无效
- 操作与源会话不符
- 源节点未接受确认选项，请重新查看清单
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
- 确认选项与会话清单不一致
- 操作已开始，不能更改操作类型
- 此操作不是同机复制
- 此操作不是迁移源
- 迁移源尚未完成导出
- 此操作不是迁移接收记录
- 源端执行归属尚未切换
- 源会话自定义标题已变化，请重新查看清单
- 目标名称索引在发布前发生变化
- 原生数据库结构已变化
- 源端原生记录已变化，保留清理现场
- 目标原生数据在发布前发生变化
- 目标前缀在备份时发生变化
- 复制记录不存在
- 迁移操作不存在
- 会话组关联已变化，请重新查看复制清单
- 会话历史已变化，请重新查看复制清单
- 源会话显示设置已变化，请重新查看复制清单
- 会话附属文件已变化
- 原生会话元数据已变化，请重新查看复制清单
- 目标复用文件已变化
- 复制暂存数据已变化
- 目标前缀在发布前发生变化
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `cancel_clone` L207 → `POST /api/session/clone/cancel`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `clone_progress` L187 → `POST /api/session/clone/progress`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `execute` L522 → `POST /api/session/clone`
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `abort` L577, L599
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `cancel_inner` L504
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `path` L149
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `progress` L375, L398
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `reconcile` L543
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L799, L862, L872, L944
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `build_manifest` L226
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `export_bundle` L290
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `plan` L223
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `rows` L111
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stable_read` L145, L154
- [`transfer/codex.rs`](../crates/sessiondock/src/transfer/codex.rs) `stage` L482, L576
- [`transfer/environment.rs`](../crates/sessiondock/src/transfer/environment.rs) `stale` L81
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `rewrite` L712
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `stage` L909, L1008
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `abort_local` L193
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `activate_target` L237
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `changed` L20
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `confirm_mode` L90, L99
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `retire_source` L429
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `switch_source` L219, L225
- [`transfer/names.rs`](../crates/sessiondock/src/transfer/names.rs) `publish` L136
- [`transfer/names.rs`](../crates/sessiondock/src/transfer/names.rs) `recheck` L79
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `insert_with_prefix` L705, L716
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_copy` L489
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `preflight_prefix` L543
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `retire` L638
- [`transfer/prefix.rs`](../crates/sessiondock/src/transfer/prefix.rs) `prepare` L176
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `execute` L985, L994, L1031
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `load` L322
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `progress_status` L347
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `recheck_in` L631, L638, L650, L670, L674

### `move_platform`

- 跨机器迁移目前只支持 Linux
- 迁移链接要求 Unix
- 此平台不支持迁移符号链接
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `build_manifest` L152
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `receive_bundle` L673, L706
- [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `validate_bundle` L343
- [`transfer/files.rs`](../crates/sessiondock/src/transfer/files.rs) `stage` L976
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `execute` L1003

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
- 目标名称索引已变化，保留现场等待恢复
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
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `resolve_resume` L468
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `run` L796, L905, L1065
- [`transfer/environment.rs`](../crates/sessiondock/src/transfer/environment.rs) `remove` L299
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `abort_source` L127, L137
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `abort_target` L167
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `activate_target` L246
- [`transfer/moving.rs`](../crates/sessiondock/src/transfer/moving.rs) `next_ownership_sequence` L300
- [`transfer/names.rs`](../crates/sessiondock/src/transfer/names.rs) `rollback` L162
- [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `rollback` L818, L829, L864, L879, L885
- [`transfer/prefix.rs`](../crates/sessiondock/src/transfer/prefix.rs) `check_restore` L199, L213
- [`transfer/prefix.rs`](../crates/sessiondock/src/transfer/prefix.rs) `restore` L230
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `cleanup_markers` L768
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `execute` L847
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `plan_copy` L416
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `rollback` L802

### `move_reference_unsupported`

- code-mode 中存在无法静态解析的会话引用
- 子代理工具参数不是 JSON 字符串
- 子代理工具参数不是 JSON
- 子代理工具结果包含无法识别的内容项
- 子代理工具结果不是文本或原生内容数组
- ；
- [`transfer/code_mode.rs`](../crates/sessiondock/src/transfer/code_mode.rs) `failure` L79
- [`transfer/codex_tools.rs`](../crates/sessiondock/src/transfer/codex_tools.rs) `output` L75, L84
- [`transfer/codex_tools.rs`](../crates/sessiondock/src/transfer/codex_tools.rs) `rewrite` L42, L48
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `plan_copy` L440

### `move_root_mismatch`

- 两端 CLI 根目录路径不同 — [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `validate_bundle` L367

### `move_session_locked`

- 会话正在复制或等待恢复
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `send` L175 → `POST /api/session/conversation/send`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `resolve_resume` L470

### `move_session_running`

- 会话仍在运行：{title} — [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `stopped` L97

### `move_shared_storage`

- 两台机器共享会话存储，不能移动文件 — [`transfer/bundle.rs`](../crates/sessiondock/src/transfer/bundle.rs) `validate_bundle` L380

### `move_verify`

- 克隆后的历史关系不完整
- 克隆仍引用源组身份
- [`transfer/service.rs`](../crates/sessiondock/src/transfer/service.rs) `execute` L1069, L1085

### `nest_parent_cycle`

- 不能附属到自己的子会话下面 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `nest` L437 → `POST /api/session/nest`

### `not_found`

- 会话不在当前索引中 — [`transfer/group.rs`](../crates/sessiondock/src/transfer/group.rs) `derive_cached` L64

### `report_result_unknown`

- 此报告已开始保存，请核对诊断和处理会话；原输入保留，不会重复创建报告
- 诊断已开始保存，输入保留，不会重复创建
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report_inner` L239
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `get` L59 → `GET /api/session/conversation`

### `run_state_unknown`

- 该会话的受管实例运行状态未知（{reason}），未发送任何停止指令；未知不等于已退出，请稍后重试或检查宿主 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1254 → `POST /api/session/stop`

### `session_error`

- 父线程 ID 在已配置索引中存在歧义
- Codex 子代理 ID 在索引中存在歧义
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
- Claude 子代理声明的会话 ID 与主会话不一致
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `build` L783
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `history_parent` L469
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `rollout_parent` L216
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `select_main` L679
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `sid` L451
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `thread` L271
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `thread_from` L299
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `native_identity` L400
- [`sessions/media_projection.rs`](../crates/sessiondock/src/sessions/media_projection.rs) `project_range` L40
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `invalid_range` L20
- [`sessions/native_media.rs`](../crates/sessiondock/src/sessions/native_media.rs) `authorized_reader` L62, L64, L75, L84
- [`sessions/native_media.rs`](../crates/sessiondock/src/sessions/native_media.rs) `finish` L29, L32
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `history_page_body` L544
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `media_page` L605, L608, L614, L620
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `validate_grant_scope` L516
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L68, L86
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `prepare` L161, L170
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `replay_changed` L21
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `native_identity` L97
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L538, L541
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_scope` L1827
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_source` L470
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `validate_request` L1592

### `shell_env_unconfigured`

- 本机没有配置登录环境检查，不能从页面重启后端 — [`api/shell_env.rs`](../crates/sessiondock/src/api/shell_env.rs) `restart` L22 → `POST /api/shell-env/restart`

### `stale_build`

- 页面版本已过期，请刷新后再保存偏好 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `validate` L46

### `stop_superseded`

- 该回滚分支已不是当前运行分支，未停止共享的子会话 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1157 → `POST /api/session/stop`

### `takeover_superseded`

- 该回滚分支的运行实例已转移到更新的子会话，请先处理当前子会话 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `takeover` L755 → `POST /api/term/takeover`

### `terminal_binding_unavailable`

- 无法确认会话与终端实例的唯一关联；请刷新，不会降级按名称连接。 — [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `binding_unavailable` L444

### `tree_delete_refused`

- (dynamic) — [`api/trash_tree.rs`](../crates/sessiondock/src/api/trash_tree.rs) `error` L31

## 410 Gone

### `session_error`

- 历史页已过期，请重新载入会话
- 图片分页已过期，请重新载入会话
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L208

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
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `action` L520 → `POST /api/session/files/action`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `resolve` L125 → `POST /api/session/resolve-files`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `parse_body` L103
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1122 → `POST /api/session/stop`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `invalid` L110
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim_inner` L167
- [`security.rs`](../crates/sessiondock/src/security.rs) `api_policy` L107

### `file_image_budget`

- 单张磁盘图片超过 32 MiB 读取上限 — [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `new` L95

### `file_items_limit`

- 一次最多操作 {MAX_WRITE_ITEMS} 个项目 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `items` L397

### `file_job_too_large`

- 单个上传最多 {} 字节 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `start_upload` L750

### `file_raw_budget`

- 原始打开超过 32 MiB，请使用预览或下载
- 原始打开超过 32 MiB，请使用文本预览或下载
- [`files/response.rs`](../crates/sessiondock/src/files/response.rs) `read` L406, L424

### `file_upload_chunk_too_large`

- 单个分块最多 {limit} 字节
- 单个分块最多 {} 字节
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload` L600 → `POST /api/session/files/upload`
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L841

### `file_upload_overflow`

- 分块超过声明的上传大小 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `upload` L871

### `file_upload_too_large`

- 单个附件不能超过 512 MiB
- 单个附件不能超过 {} MB
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `upload` L302 → `POST /api/session/conversation/attachment`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `upload_attachment` L731
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L1973

### `session_error`

- 此历史页无法在读取预算内推进
- 此图片分页无法在读取预算内推进
- 图片分页响应超过 8 MiB 预算
- 继承历史预算溢出
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `media_page` L625, L659
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `page_selection` L572
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `inherit` L1932

### `terminal_input_too_large`

- 终端输入请求体过大
- 单次终端输入不能超过 1 MiB
- 单次终端粘贴不能超过 1 MiB
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `send` L558, L587, L613, L625 → `POST /api/term/send`

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

- 文件响应头无效 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `response_body` L435

### `file_worker_failed`

- 附件写入任务异常退出
- 文件读取任务异常退出
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `attachment` L716 → `POST /api/session/attachment`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `work` L94

### `live_failed`

- 进程表配对失败 — [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `assemble` L363

### `metadata_worker_failed`

- 偏好工作异常退出，请重新读取状态确认结果 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `write` L145

### `move_io`

- (dynamic)
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `abort_move` L274 → `POST /api/session/transfer/abort`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `activate_target` L334 → `POST /api/session/transfer/activate`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `bundle_manifest` L843, L865 → `POST /api/session/transfer/manifest`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `cancel_clone` L232 → `POST /api/session/clone/cancel`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `check_bundle` L903 → `POST /api/session/transfer/check`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `confirm_mode` L493
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `execute` L549 → `POST /api/session/clone`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `export_bundle` L588, L618 → `POST /api/session/transfer/export`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `plan` L163 → `POST /api/session/clone/plan`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `receive_bundle` L728 → `POST /api/session/transfer/receive`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `release_export` L751 → `POST /api/session/transfer/release`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `reserve_export` L778 → `POST /api/session/transfer/reserve`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `retire_source` L377, L436, L449, L459 → `POST /api/session/transfer/retire`
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `stopped` L57
- [`api/transfer.rs`](../crates/sessiondock/src/api/transfer.rs) `switch_source` L311 → `POST /api/session/transfer/switch`
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `call_work` L1237
- [`hub/transfer.rs`](../crates/sessiondock/src/hub/transfer.rs) `save` L203
- [`transfer/mod.rs`](../crates/sessiondock/src/transfer/mod.rs) `from` L47

### `move_native_database`

- (dynamic) — [`transfer/native.rs`](../crates/sessiondock/src/transfer/native.rs) `from` L39

### `reader_failed`

- 只读任务失败
- [`state.rs`](../crates/sessiondock/src/state.rs) `run` L160
- [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L135

### `runtime_encoding`

- 受控进程观察无法编码 — [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `assemble` L278

### `search_failed`

- 搜索任务失败 — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L156 → `GET /api/search`

### `session_error`

- 会话索引锁不可用
- 会话视图锁不可用
- 会话列表缓存锁不可用
- 历史消息序列化失败
- 历史页响应序列化失败
- 消息序列化失败
- 会话行不是对象
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `refresh_within` L542
- [`sessions/index/titles.rs`](../crates/sessiondock/src/sessions/index/titles.rs) `codex_name` L14
- [`sessions/index/titles.rs`](../crates/sessiondock/src/sessions/index/titles.rs) `titles` L28, L39
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `list_state` L525
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `published_view_bytes` L770
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `views` L535
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `history_page_body` L552
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `take` L310
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `validate_response` L367
- [`sessions/views/body.rs`](../crates/sessiondock/src/sessions/views/body.rs) `serialize_error` L160
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `encode_events` L118
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `view_meta` L1613

### `trash_encoding`

- 回收站结果无法编码 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `encoding` L460

### `trash_failed`

- 回收站任务失败
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `list` L545 → `GET /api/trash`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `purge` L622 → `POST /api/trash/purge`

## 501 Not Implemented

### `bug_report_disabled`

- 缺陷报告未启用：需要 SESSIONDOCK_BUG_REPORT_DIR/REPO、审计目录、终端传输和受控创建 — [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `disabled` L48

### `conversation_disabled`

- 服务端会话草稿未配置，无法转交报告
- 会话保存和发送未配置
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `report_inner` L184
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `enabled` L27

### `file_action_not_implemented`

- 写入服务尚未实现此文件操作：{}；不会伪造任务或成功结果 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `action` L380

### `file_mode_not_implemented`

- 只读文件服务尚未实现此模式；不会伪造空任务或成功结果
- 文件服务尚未实现此模式
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `get` L307, L336
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `validate` L177, L182
- [`files/mod.rs`](../crates/sessiondock/src/files/mod.rs) `unsupported` L64

### `file_trash_unconfigured`

- 未配置 SESSIONDOCK_STATE_DIR，没有回收目录；写入服务不会直接删除 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `trash_named` L684

### `files_disabled`

- 文件读取服务不可用 — [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `configured` L55

### `files_jobs_disabled`

- 文件写入服务未启用
- 文件写入未启用：必须显式配置 SESSIONDOCK_FILE_WRITE_ROOTS（只读目录不会隐式变为可写）
- [`api/bug_report.rs`](../crates/sessiondock/src/api/bug_report.rs) `attachment` L660 → `POST /api/session/attachment`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `write_configured` L452

### `media_files_disabled`

- 未配置图片读取目录；不会自动访问磁盘 — [`sessions/media_projection.rs`](../crates/sessiondock/src/sessions/media_projection.rs) `project_ranges` L71

### `metadata_disabled`

- 偏好保存未配置状态目录 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `configured` L100

### `not_implemented`

- Rust 后端尚未迁移此能力：{capability}。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：受控会话创建与 pending 生命周期。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：终端后端选择。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：受管实例停止。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：会话冻结。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：进程身份验证。当前是只读开发阶段。
- Rust 后端尚未迁移此能力：删除会话树。当前是只读开发阶段。
- [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L32 → `POST /api/audit/browser`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `backend` L920 → `POST /api/term/backend`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `enabled` L33
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `freeze` L1305, L1323 → `POST /api/session/freeze`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1117, L1165 → `POST /api/session/stop`
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `configured` L39
- [`api/trash_tree.rs`](../crates/sessiondock/src/api/trash_tree.rs) `services` L27
- [`error.rs`](../crates/sessiondock/src/error.rs) `unavailable` L31

### `terminal_disabled`

- 终端传输未启用：必须显式配置隔离的 ptyhost 目录
- [`api/final_screen.rs`](../crates/sessiondock/src/api/final_screen.rs) `get` L29 → `GET /api/term/final`
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `enabled` L40

### `unsupported_history`

- history_base 必须是对象
- history_base 缺少父线程 ID
- history_base 前缀偏移必须是非负整数
- 分叉历史不能把子代理文件当作主线程父历史
- Claude 子代理的主会话不在已配置索引中
- Claude 子代理的主会话路径存在歧义
- 历史依赖或子代理缺少父线程 ID
- 父线程不在已配置索引中，请确认显式数据源包含其原生文件
- 子代理归属关系存在循环
- 父历史固定前缀超出完整原生数据范围
- 父历史固定前缀不在完整 JSONL 行边界
- 分叉历史依赖存在循环
- 此数据源尚不支持原生操作范围
- 此数据源没有分叉父历史
- 原生记录缺少明确会话 ID，不能用文件名或显示名称推断
- 原生会话 ID 无效，不能确定操作范围
- 搜索投影不提供原生操作范围
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `history_link` L60, L69, L74
- [`sessions/history.rs`](../crates/sessiondock/src/sessions/history.rs) `unsupported` L37
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `chain` L540
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `check_cut` L510, L511
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `history_link` L750, L759, L763
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `history_parent` L473
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `native_scope` L693, L696
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `new` L380, L383
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `ownership` L494
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `rollout_parent` L215
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `sid` L445, L452
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `unsupported` L71
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `physical_chain` L324
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `thread` L251, L254, L264, L272
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `blank` L173
- [`sessions/index/summary/mod.rs`](../crates/sessiondock/src/sessions/index/summary/mod.rs) `native_identity` L394, L407
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `grok_native_identity` L31
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `native_identity` L65, L91, L104
- [`sessions/scope.rs`](../crates/sessiondock/src/sessions/scope.rs) `summary_native_identity` L50
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `build` L1717, L1725
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `claude_rewind_target` L530, L543
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `committed_records` L513
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `inherit` L1926
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `native_scope` L1817, L1821
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `parse_prefix` L1973

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

- 观察已取消 — [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L125

### `conversation_send`

- 发送任务异常退出 — [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `send` L194 → `POST /api/session/conversation/send`

### `conversation_start`

- 启动任务异常退出，输入保留 — [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `restart` L280 → `POST /api/session/conversation/restart`

### `conversation_storage`

- 草稿保存任务失败
- 草稿清理任务失败
- [`api/conversation.rs`](../crates/sessiondock/src/api/conversation.rs) `save` L148
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `forget_discarded_launch` L1074

### `cwd_check_failed`

- 启动目录检查失败 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `prepare_cwd` L451

### `file_attachment_id`

- 无法分配附件目录编号 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `attachment_upload` L2051

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
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `copy_publish` L1310
- [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `publish_file` L1213

### `file_random_unavailable`

- 系统随机源不可用，不能分配任务编号 — [`files/jobs.rs`](../crates/sessiondock/src/files/jobs.rs) `random_id` L139

### `file_trash_id`

- 无法分配回收目录编号 — [`files/write.rs`](../crates/sessiondock/src/files/write.rs) `fresh_private_child` L1117

### `freeze_failed`

- 无法冻结或恢复会话进程：{error} — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `freeze` L1328 → `POST /api/session/freeze`

### `freeze_resume_failed`

- 停止前无法恢复冻结进程：{error}；请先恢复运行后重试 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1214 → `POST /api/session/stop`

### `lifecycle_response`

- 创建状态序列化失败 — [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `serialize` L262

### `media_busy`

- 图片处理繁忙，请稍后重试 — [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `read_media` L48

### `media_unavailable`

- 图片服务暂不可用 — [`media/file_media.rs`](../crates/sessiondock/src/media/file_media.rs) `file` L71

### `metadata_clock_invalid`

- 系统时钟无效，不能记录固定时间 — [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `rewind` L499 → `POST /api/session/rewind`

### `process_control_unavailable`

- 无法结束外部会话进程：{error:?}
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `stop` L1188 → `POST /api/session/stop`
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `takeover` L782 → `POST /api/term/takeover`

### `process_scan_unavailable`

- 无法核对会话进程：{error}
- [`api/lifecycle.rs`](../crates/sessiondock/src/api/lifecycle.rs) `external_processes` L556
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `external_scan_result` L47

### `reader_busy`

- 读取服务已关闭 — [`state.rs`](../crates/sessiondock/src/state.rs) `run_wait` L126

### `runtime_closed`

- 进程观察服务已关闭 — [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `observe_liveness` L90

### `runtime_unavailable`

- 受控 host 目录不可用或超出观察预算
- 无法读取受管进程状态
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `unavailable` L505
- [`api/trash.rs`](../crates/sessiondock/src/api/trash.rs) `observe_liveness` L101

### `search_cancelled`

- 服务正在退出，搜索已取消 — [`api/search.rs`](../crates/sessiondock/src/api/search.rs) `get` L154 → `GET /api/search`

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
- [`sessions/index/graph.rs`](../crates/sessiondock/src/sessions/index/graph.rs) `check_cut` L512
- [`sessions/index/mod.rs`](../crates/sessiondock/src/sessions/index/mod.rs) `discover` L814, L821
- [`sessions/index/names.rs`](../crates/sessiondock/src/sessions/index/names.rs) `error` L23
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `file_stamp` L1323, L1327
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `prepare` L1093
- [`sessions/mod.rs`](../crates/sessiondock/src/sessions/mod.rs) `stamp` L1316
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `allocation_error` L23
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `changed` L14
- [`sessions/native_input.rs`](../crates/sessiondock/src/sessions/native_input.rs) `read_error` L17
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `find` L199
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `issue` L138, L152, L162
- [`sessions/pages.rs`](../crates/sessiondock/src/sessions/pages.rs) `link_media` L176
- [`sessions/records/native_records.rs`](../crates/sessiondock/src/sessions/records/native_records.rs) `io_error` L76
- [`sessions/records/native_records/replay_source.rs`](../crates/sessiondock/src/sessions/records/native_records/replay_source.rs) `materialize_text` L89, L94, L96, L100
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `committed_records` L509
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `parse_candidate_retaining` L617, L622, L625
- [`sessions/views/mod.rs`](../crates/sessiondock/src/sessions/views/mod.rs) `read_bounded_limit` L560

### `shutdown`

- 服务正在关闭
- [`api/audit.rs`](../crates/sessiondock/src/api/audit.rs) `browser` L40 → `POST /api/audit/browser`
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `admission` L64
- [`api/files.rs`](../crates/sessiondock/src/api/files.rs) `write_admission` L462
- [`api/media.rs`](../crates/sessiondock/src/api/media.rs) `get` L79 → `GET /api/media/{token}`
- [`api/metadata.rs`](../crates/sessiondock/src/api/metadata.rs) `write` L128
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `observe` L521
- [`api/runtime.rs`](../crates/sessiondock/src/api/runtime.rs) `shared` L464
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `attach` L335 → `GET /api/term/attach`
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `claim_inner` L195
- [`api/terminal.rs`](../crates/sessiondock/src/api/terminal.rs) `send` L654 → `POST /api/term/send`

### `watch_closed`

- 会话观察已关闭，请重试 — [`observe.rs`](../crates/sessiondock/src/observe.rs) `closed` L212

### `服务正在关闭`

- (dynamic) — [`state.rs`](../crates/sessiondock/src/state.rs) `admit` L103
