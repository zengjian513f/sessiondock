# Capability flags

Defaults come from `state::capabilities()`. `lib.rs` then overwrites configured
flags after opening optional services. `assets.rs` injects the JSON as
`<meta name="sessiondock-capabilities">` (also `/api/meta`).
`allows(name)` is `config[name] !== false`, so a missing key stays allowed.

## Meta tag, `storage_namespace`, fail-closed

`legacy-web/capabilities.js` is the only parser. A page without the tag uses
the permissive fallback (`declared:false`) while retaining the SessionDock
storage namespace. A present tag must
`JSON.parse` to a non-array object; otherwise it **fails closed**:
`{read_only:true, live:false, outbox:false, audit:false, search:false,
files:false, configuration_error:true}` so the page does not start unsupported
background work. Only then does `#backend-notice` appear, saying
“能力配置无效，请检查服务配置。” — a healthy SessionDock page has no
standing banner (it is the replacement, not a development build).

`storage_namespace` is `"sessiondock."`. The parser uses a configured non-empty
string and otherwise defaults to `"sessiondock."`.
`SessionDockCapabilities.namespace` prefixes `localStorage` in `nodes.js` /
`app.js` (`STORAGE_PREFIX`), `typography.js`, the theme bootstrap in
`index.html`, and `files.js` (`<namespace>files-<key>`). Reads and writes use
only these SessionDock keys; there is no compatibility namespace or copy-forward
path. `SessionDockCapabilities.stored(key, prefix = namespace)` is the shared
read helper for `store.get`, `nodesOff` and typography.

## Flags

| Flag | Value | Legacy UI (`config.<flag>` / `allows('<flag>')`) | Doc |
| --- | --- | --- | --- |
| `backend` | `"rust"` | identifies the server implementation | [architecture.md](architecture.md) |
| `stage` | `"replacement"` | no `config`/`allows` gate | [architecture.md](architecture.md) |
| `read_only` | `false` (`true` only in the fail-closed fallback above) | `true` never comes from the server; the fallback shows `#backend-notice` | [architecture.md](architecture.md) |
| `storage_namespace` | `"sessiondock."` | `localStorage` prefix (above) | [architecture.md](architecture.md) |
| `sessions` | `true` | no `config`/`allows` gate | [architecture.md](architecture.md) |
| `watch` | `true` | no `config`/`allows` gate (SSE is always on) | [architecture.md](architecture.md) |
| `search` | `true` | else title-only filter, no NDJSON `/api/search` | [architecture.md](architecture.md) |
| `live` | `true` where native process discovery is supported | when false the page skips `/api/live` (“运行状态未知”); when true `/api/live` merges managed observations with native process discovery | [liveness.md](liveness.md), [processes.md](processes.md) |
| `terminal` | true when `SESSIONDOCK_PTYHOST_DIR` opens TerminalService | notice + 3s term-list poll if `live` is false | [terminal-ownership.md](terminal-ownership.md) |
| `terminal_transport` | same as `terminal` | no `config`/`allows` gate | [terminal-ownership.md](terminal-ownership.md) |
| `terminal_records` | same as `terminal` | recording list/replay availability; the main console opens replay from the session row's `recording` field | [terminal-records.md](terminal-records.md) |
| `terminal_backend` | same as `terminal` | no `config`/`allows` gate | [lifecycle-http.md](lifecycle-http.md) |
| `terminal_create` | true when lifecycle opens (`SESSIONDOCK_LIFECYCLE_DIR` + launcher) | unhides `#new-session`; notice “受控创建已配置” | [lifecycle-integration.md](lifecycle-integration.md) |
| `terminal_pending` | same as `terminal_create` | no `config`/`allows` gate | [lifecycle-http.md](lifecycle-http.md) |
| `terminal_bind` | same as `terminal_create` | none — `POST /api/term/bind` is API-only; the pending page follows a confirmed binding by itself | [lifecycle-http.md](lifecycle-http.md) |
| `terminal_takeover` | same as `terminal_create` | resume via `resume_sources`, never name-guess | [lifecycle-http.md](lifecycle-http.md) |
| `terminal_complete_dir` | same as `terminal_create` | enables cwd directory suggestions | [lifecycle-http.md](lifecycle-http.md) |
| `session_stop` | true when `terminal_create` and `terminal` are both true | stop control for listed managed instances and inline outcome/refusal notice | [lifecycle-http.md](lifecycle-http.md#stopping-a-session) |
| `outbox` | true when `SESSIONDOCK_DELIVERY_DIR` opens the ledger **and** `SESSIONDOCK_PTYHOST_DIR` configures the terminal transport | enables the legacy composer/outbox and the four send routes (supported managed CLI instances) | [delivery-executor.md](delivery-executor.md) |
| `outbox_read` | true when `SESSIONDOCK_DELIVERY_DIR` opens DeliveryService | no `config`/`allows` gate (does not enable `outbox`) | [delivery-http.md](delivery-http.md) |
| `audit` | true when `SESSIONDOCK_AUDIT_DIR` is configured | queues `POST /api/audit/browser`; else no posts | [diagnostics.md](diagnostics.md) |
| `bug_report` | true when `SESSIONDOCK_BUG_REPORT_DIR`/`REPO`, the audit directory, the terminal transport, the lifecycle service are all configured | no `config`/`allows` gate yet (the report dialog posts and shows the `501 bug_report_disabled` error); `POST /api/bug-report` and the `uid=bug-report` upload answer 501 while false | [bug-report.md](bug-report.md) |
| `metadata` | true when `SESSIONDOCK_STATE_DIR` opens MetadataStore | enables stars and display preferences | [metadata.md](metadata.md) |
| `files` | `true`; paths resolve from the selected session/cwd | opens referenced paths through the file browser | [files.md](files.md) |
| `files_jobs` | true when file writes are configured or terminal transport is available | enables the file job dialog | [files.md](files.md) |
| `file_thumbnails` | `false` | skips grid `mode=thumbnail` `<img>` | [files.md](files.md) |
| `mutations` | `false` | no `config`/`allows` gate | [metadata.md](metadata.md) |
| `hub` | `false` | no `config`/`allows` gate (`sessiondock-mode` is `local`) | [hub.md](hub.md) |
| `media` | `true` | no `config`/`allows` gate (local tokens still render) | [media.md](media.md) |
| `media_remote` | `true` | browser renders HTTP(S) image references directly | [media.md](media.md) |
| `media_lazy` | `true` | no eager `src`; GET on view + visible error/retry | [media.md](media.md) |
| `media_continuation` | `true` | gallery `.media-more` appends the next batch in place | [media.md](media.md) |
| `history_pages` | `true` | gap button loads `/page` instead of full history | [history-pages.md](history-pages.md) |
| `history_semantics` | `"limited_native"` | no `config`/`allows` gate | [native-input.md](native-input.md) |

`outbox_read` is the read-only projection; `outbox` additionally requires the
terminal transport because reliable send drives a real managed instance. When
the delivery ledger is configured but no terminal directory is, `outbox_read`
is true and `outbox` stays false, so the composer stays hidden and the four
send routes return `501 delivery_send_disabled`.

## Optional services

Node responses keep `hub:false`; the separate Hub server provides federation.
`mutations:false` is not a gate for the separately advertised trash, file-write,
timeline-pin, or delivery endpoints. `file_thumbnails:false` means thumbnail
rendering is not implemented.

`files_write` contains the configured chunk size and supported operations when
file writes are available, and is false otherwise. `trash` requires a trash
directory. `timeline_pin` uses the metadata store. These flags describe available
services; file roots do not restrict where a session can read or run.

HTTP(S) image references are returned with `external:true`; the browser loads
them directly. SessionDock does not fetch or proxy their bytes.
([media.md](media.md#file-references))
