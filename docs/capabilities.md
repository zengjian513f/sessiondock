# Capability flags

Defaults come from `state::capabilities()`. `lib.rs` then overwrites configured
flags after opening optional services. `assets.rs` injects the JSON as
`<meta name="agenthub-capabilities">` (also `/api/meta`).
`allows(name)` is `config[name] !== false`, so a missing key stays allowed.

## Meta tag, `storage_namespace`, fail-closed

`legacy-web/capabilities.js` is the only parser. A Python-served page has **no**
such tag (`declared:false`) and keeps historical behaviour. A present tag must
`JSON.parse` to a non-array object; otherwise it **fails closed**:
`{read_only:true, live:false, outbox:false, audit:false, search:false,
files:false, configuration_error:true}` so the page does not start unsupported
background work. Only then does `#backend-notice` appear, saying
“能力配置无效，请检查服务配置。” — a healthy SessionDock page has no
standing banner (batch 44 WP-A: it is the replacement, not a development build).

`storage_namespace` is `"sessiondock."`. The parser uses a non-empty string,
else `"sessiondock."` when `backend==="rust"`, else `""`.
`AgentHubCapabilities.namespace` prefixes `localStorage` in `nodes.js` /
`app.js` (`STORAGE_PREFIX`), `typography.js`, the theme bootstrap in
`index.html`, and `files.js` (`<namespace>files-<key>`), so Rust keys never
collide with Python `agenthub.` (hub `agenthub.hub.<path>.` applies only if the
namespace is empty).

Batch 44 WP-F adds the one-time migration for a same-origin replacement:
`AgentHubCapabilities.stored(key, prefix = namespace)` reads `<prefix><key>`
and, only when the Rust namespace is set and the value is missing, reads the
Python key of the same page (`agenthub.<key>`, hub `agenthub.hub.<path>.<key>`),
copies it to the Rust key and returns it; the Python key is never written.
Every preference read (`store.get`, `nodesOff`, `font`, the pre-capabilities
theme bootstrap with the same inline rule, and `files.js` with the extra
pre-rename spelling `<namespace>agenthub-files-<key>` before the bare Python
`agenthub-files-<key>`) goes through it; writes stay on the Rust key only.
A page without the tag (empty namespace) neither falls back nor copies. See
[migration.md](migration.md#第四十四批wp-f改名收尾偏好一次性迁移登录-shell-包装).

## Flags

| Flag | Value | Legacy UI (`config.<flag>` / `allows('<flag>')`) | Doc |
| --- | --- | --- | --- |
| `backend` | `"rust"` | regex-option tooltip; many rust-only branches | [architecture.md](architecture.md) |
| `stage` | `"replacement"` | no `config`/`allows` gate | [architecture.md](architecture.md) |
| `read_only` | `false` (`true` only in the fail-closed fallback above) | `true` never comes from the server; the fallback shows `#backend-notice` | [architecture.md](architecture.md) |
| `storage_namespace` | `"sessiondock."` | `localStorage` prefix (above) | [migration.md](migration.md) |
| `sessions` | `true` | no `config`/`allows` gate | [architecture.md](architecture.md) |
| `watch` | `true` | no `config`/`allows` gate (SSE is always on) | [architecture.md](architecture.md) |
| `search` | `true` | else title-only filter, no NDJSON `/api/search` | [architecture.md](architecture.md) |
| `live` | `false` by default; `true` on Linux with `SESSIONDOCK_PROC_SCAN=1` (batch 36) | when false the page skips `/api/live` (“运行状态未知”); when true `/api/live` is the managed observations plus the read-only `/proc` scan | [liveness.md](liveness.md), [processes.md](processes.md) |
| `terminal` | true when `SESSIONDOCK_PTYHOST_DIR` opens TerminalService | notice + 3s term-list poll if `live` is false | [terminal-ownership.md](terminal-ownership.md) |
| `terminal_transport` | same as `terminal` | no `config`/`allows` gate | [terminal-ownership.md](terminal-ownership.md) |
| `terminal_backend` | same as `terminal` | no `config`/`allows` gate | [lifecycle-http.md](lifecycle-http.md) |
| `terminal_create` | true when lifecycle opens (`SESSIONDOCK_LIFECYCLE_DIR` + launcher) | unhides `#new-session`; notice “受控创建已配置” | [lifecycle-integration.md](lifecycle-integration.md) |
| `terminal_pending` | same as `terminal_create` | no `config`/`allows` gate | [lifecycle-http.md](lifecycle-http.md) |
| `terminal_bind` | same as `terminal_create` | pending “关联原生会话” / “释放本页控制台” | [lifecycle-http.md](lifecycle-http.md) |
| `terminal_takeover` | same as `terminal_create` | resume via `resume_sources`, never name-guess | [lifecycle-http.md](lifecycle-http.md) |
| `terminal_complete_dir` | same as `terminal_create` | else cwd box stays whitelist-only, no complete-dir | [lifecycle-http.md](lifecycle-http.md) |
| `session_stop` | true when `terminal_create` and `terminal` are both true | stop control for listed managed instances, `request_id`, inline outcome/refusal notice | [lifecycle-http.md](lifecycle-http.md#stopping-a-managed-instance-batch-29) |
| `outbox` | true when `SESSIONDOCK_DELIVERY_DIR` opens the ledger **and** `SESSIONDOCK_PTYHOST_DIR` configures the terminal transport | enables the legacy composer/outbox and the four send routes (Claude managed instances only) | [delivery-executor.md](delivery-executor.md) |
| `outbox_read` | true when `SESSIONDOCK_DELIVERY_DIR` opens DeliveryService | no `config`/`allows` gate (does not enable `outbox`) | [delivery-http.md](delivery-http.md) |
| `audit` | true when `SESSIONDOCK_AUDIT_DIR` is configured | queues `POST /api/audit/browser`; else no posts | [diagnostics.md](diagnostics.md) |
| `bug_report` | true when `SESSIONDOCK_BUG_REPORT_DIR`/`REPO`, the audit directory, the terminal transport, the lifecycle service and a launcher `bug_report_profiles` entry are all configured | no `config`/`allows` gate yet (the report dialog posts and shows the `501 bug_report_disabled` error); `POST /api/bug-report` and the `uid=bug-report` upload answer 501 while false | [bug-report.md](bug-report.md) |
| `metadata` | true when `SESSIONDOCK_STATE_DIR` opens MetadataStore | notice “星标和显示偏好保存到独立开发目录” | [metadata.md](metadata.md) |
| `files` | true when `SESSIONDOCK_FILE_ROOTS` is non-empty | else “尚未实现文件解析与文件操作” | [files.md](files.md) |
| `files_jobs` | `false` | tasks dialog: no write/transfer jobs | [files.md](files.md) |
| `file_thumbnails` | `false` | skips grid `mode=thumbnail` `<img>` | [files.md](files.md) |
| `mutations` | `false` | no `config`/`allows` gate | [metadata.md](metadata.md) |
| `hub` | `false` | no `config`/`allows` gate (`agenthub-mode` is `local`) | [../BACKEND_MIRGRATION_PLAN.md](../BACKEND_MIRGRATION_PLAN.md) |
| `media` | `true` | no `config`/`allows` gate (local tokens still render) | [media.md](media.md) |
| `media_remote` | `false` | `safeMediaSrc` returns `''` for Markdown http(s) images | [media.md](media.md) |
| `media_lazy` | `true` | no eager `src`; GET on view + visible error/retry | [media.md](media.md) |
| `media_continuation` | `true` | gallery `.media-more` appends the next batch in place | [media.md](media.md) |
| `history_pages` | `true` | gap button loads `/page` instead of full history | [history-pages.md](history-pages.md) |
| `history_semantics` | `"limited_native"` | no `config`/`allows` gate | [native-input.md](native-input.md) |

`outbox_read` is the read-only projection; `outbox` additionally requires the
terminal transport because reliable send drives a real managed instance. When
the delivery ledger is configured but no terminal directory is, `outbox_read`
is true and `outbox` stays false, so the composer stays hidden and the four
send routes return `501 delivery_send_disabled`.

## Deliberately kept false

- **`live` without the process scan.** Partial host observations must not look
  like a complete census: with the scan off the legacy `live:true` semantics
  would treat the managed list as the complete set and mark unlisted sessions
  as stopped. With `SESSIONDOCK_PROC_SCAN=1` on Linux the census is the same
  `/proc` scan Python performs ([liveness.md](liveness.md)) and `live` is true.
  ([processes.md](processes.md#http-and-resource-bounds);
  [AGENTS.md](../AGENTS.md): “Keep `capabilities.live` false.”)
- **`hub`.** Node federation is M8, not this local stage: “不连接旧 Hub、生产
  host、队列或注册表”; “local 前端兼容不等于 node protocol 兼容”; “不靠
  `/api/meta` 自称兼容.” ([BACKEND_MIRGRATION_PLAN.md](../BACKEND_MIRGRATION_PLAN.md))
- **`mutations`.** Native histories stay read-only. “Enabling metadata does not
  enable general session mutations.” File jobs/trash and delivery commands still
  return 501. ([metadata.md](metadata.md), [files.md](files.md),
  [delivery-http.md](delivery-http.md))
- **`files_jobs`** and **`file_thumbnails`.** M6 is read-only roots only: “It
  does not upload, rename, delete, extract archives, generate thumbnails, create
  jobs”; “Jobs polling and automatic thumbnail requests need separate capability
  gates.” `lib.rs` hard-codes both false. ([files.md](files.md);
  [AGENTS.md](../AGENTS.md): “Jobs and thumbnails remain separately
  capability-gated until implemented.”)
- **`media_remote`.** “No server-side remote proxy is enabled. Rust declares
  `media_remote:false`, so the legacy renderer keeps a text placeholder instead
  of automatically requesting an external Markdown image.”
  ([media.md](media.md#async-integration-responsibilities))
