# Isolated migration trust boundaries

The Rust service is deployed loopback-only behind the existing reverse proxy,
in parallel with the Python service, with its own private directories
([AGENTS.md](../AGENTS.md#scope-and-boundaries)); it has no authentication of
its own. Native histories stay read-only; missing configuration fails closed. Limits marked **not a sandbox**,
**not proven**, or still unchecked are repeated here.

## Not production
No HTTP authentication: `Config::validate` rejects non-loopback binds (“Rust
migration server has no authentication; only loopback binds are allowed”);
default bind is `127.0.0.1:8741`. **No TLS listener:** `Config.bind` is a TCP
`SocketAddr` with no certificate field; the runbook opens `http://127.0.0.1:8741`
([runbook-dev.md](runbook-dev.md#3-minimal-environment)). `security.rs` Origin
matching allows `http` or `https` against Host as a same-origin check, not a TLS
terminator. **No multi-user identity:** mode `local`, `hub:false`; `meta.protocol`
is `0` unless the node listener below is configured (then `1` with the node's
own id — still not a user account); lifecycle HTTP is not authentication or
production deployment ([lifecycle-http.md](lifecycle-http.md)); a page-lease
token is not an account.
**Not production DoS protection** (plan
[M0](../BACKEND_MIRGRATION_PLAN.md#m0运行边界与-legacy-接线第一批)): URI ≤16 KiB
and body ≤4 MiB are request limits only. `/api/meta` states `migration: "limited
native read-only; not a production replacement"`. Plan §5 is static analysis, not
a production benchmark.

## Loopback listener and Host / Origin
`security.rs` `local_only` is independent of bind-time loopback. `Host` / URI
authority must be `localhost`, a loopback IP, or one of the exact authorities
listed in `SESSIONDOCK_PUBLIC_HOSTS` (comma-separated `host` or `host:port`;
the deployment's authenticating reverse proxy forwards `Host $http_host`, so
the public name it serves is listed there; `Config::from_env` refuses an empty
list or an entry that is not a valid authority) — anything else is 403
`local_only`. Listing a public host does not add authentication: the proxy's
login gate is the only one, and Origin is still keyed on the request Host.
`x-agenthub-protocol` or `x-agenthub-node-token` → 403 `hub_unsupported` on every
route, whatever the value — even the node's real credential; never an
authentication bypass (hub traffic has its own listener). Then the policy both
listeners share (`security.rs` `api_policy`): `/api/` `sec-fetch-site: cross-site` → 403
`cross_site`. If `Origin` is present it must equal `http(s)://{Host}` (else 403
`cross_origin`). URI > 16 KiB → 414, `Content-Length` > 4 MiB → 413. Responses
set `nosniff`, default `Referrer-Policy: same-origin`, and API
`Cache-Control: no-store`.

## Node listener (hub traffic, batch 38 H1)
Python's node side (`server.py` `_allowed` / `_hub_protocol`, `federation.identity`)
is a second listener, never a relaxation of the loopback one. `SESSIONDOCK_NODE_BIND`
(one interface address, not `0.0.0.0`/`::`, distinct from `SESSIONDOCK_BIND`) is
honoured only when `SESSIONDOCK_NODE_TOKEN_FILE`, `SESSIONDOCK_NODE_ID_FILE` and
`SESSIONDOCK_NODE_PEERS` are all set; any proper subset is a startup error
(`… must be set together (the node listener fails closed)`), and `--check-config`
prints `node_bind`, `node_token_file`, `node_id_file`, `node_peers` without minting
anything. The token file must be an existing absolute regular file, mode without
group/other bits, holding 32–256 characters of `[A-Za-z0-9._~+/=-]`
(`hub::identity::NodeToken`); the id file is 32 lowercase hex, minted `O_EXCL` 0600
on first start (`hub::identity::node_id`) and only read afterwards; neither file may
lie inside the frontend, native, host, state, delivery, lifecycle, launcher, audit,
trash, Codex-index, Grok-active or file-root paths. Peers are a strict CIDR list
(host bits set → error); the default is nothing.

`api/node_auth.rs` gates every request on that listener, in this order, before
any handler: TCP peer (never a proxy header; `::ffff:` unwrapped) inside
`SESSIONDOCK_NODE_PEERS`, else 403 `{"error":"forbidden","code":"node_peer_denied"}`;
`X-AgentHub-Protocol` exactly `1` and `X-AgentHub-Node-Token` equal under constant-time
comparison (`subtle::ConstantTimeEq`, length mismatch is a mismatch), else 403
`{"error":"node authentication required","code":"node_auth_required"}` (Python's
text). There is no loopback Host gate (the hub addresses the private IP) and no
browser; `api_policy` still applies (cross-site 403, Origin match, size limits,
response headers). The listener serves the same `/api` router as
loopback (sessions, messages, media, files, terminal, SSE, WebSocket) so the hub
can proxy; every non-`/api` path is 404 `not_found` — the static page is never
served there. With the identity configured `/api/meta` on both listeners reports
`protocol: 1` and `node_id` (the file's id) and `/api/nodes` on loopback returns
Python's node-mode `{mode:"local", nodes:[{id, name: hostname, online:true}]}`;
`capabilities.hub` stays `false` — a node is not a hub. The token is never
printed or served (`NodeToken` `Debug` redacts). Registration stays a server-side
hub operation ([hub.md](hub.md)).

## Explicit configuration
No CLI-home discovery, no implicit production paths (plan
[§1](../BACKEND_MIRGRATION_PLAN.md#1-目标与边界)). Unset optional variables
leave capabilities off; empty directory values fail startup.

| Variable | Role (from `config.rs`) |
| --- | --- |
| `SESSIONDOCK_BIND` | Loopback `SocketAddr` |
| `SESSIONDOCK_WEB_DIR` | Static snapshot (`legacy-web` default) |
| `SESSIONDOCK_CLAUDE_ROOT` / `_CODEX_ROOT` / `_GROK_ROOT` | Native read roots; canonicalized existing dirs |
| `SESSIONDOCK_CODEX_INDEX` | Explicit names file; never infer its parent |
| `SESSIONDOCK_PTYHOST_DIR` | Isolated host dir; no implicit discovery |
| `SESSIONDOCK_STATE_DIR` | Preferences, the debug-run registry and the file manager's `file-trash`; never a native CLI directory |
| `SESSIONDOCK_SEARCH_CACHE_DIR` | Explicit existing 0700 search-text cache (entries 0600); outside frontend, native, host, state, ledger and file paths |
| `SESSIONDOCK_DELIVERY_DIR` | Delivery ledger; configuration never initializes it |
| `SESSIONDOCK_LIFECYCLE_DIR` | Creation receipts; never initialized by Web startup |
| `SESSIONDOCK_LAUNCHER_CONFIG` | Private adapter/profile JSON, not browser input |
| `SESSIONDOCK_FILE_ROOTS` | 1–16 explicit file dirs (session-reference scoped) |
| `SESSIONDOCK_AUDIT_DIR` | Diagnostics JSONL; unset keeps audit 501 |
| `SESSIONDOCK_NODE_BIND` | Node listener `SocketAddr` (one interface, not a wildcard) |
| `SESSIONDOCK_NODE_TOKEN_FILE` | Hub credential file (0600-style, 32–256 chars) |
| `SESSIONDOCK_NODE_ID_FILE` | Persistent 32-hex node id, minted on first start |
| `SESSIONDOCK_NODE_PEERS` | Strict CIDR list of allowed hub source addresses |

Overlap: frontend, native roots, ptyhost, state, delivery, lifecycle, launcher
JSON, Codex index, and file roots must be disjoint in both directions (lexical
paths plus resolved aliases). Exception, matching Python: a launcher **cwd root**
may be an *ancestor* of the native roots, the frontend and the Codex index (the
home directory is where sessions started from `~` resume); it must still not lie
inside them, and it stays fully disjoint from ptyhost/state/delivery/lifecycle
and the launcher JSON. Delivery / lifecycle / audit dirs must already
exist, be absolute without `..`, have no symlink or reparse ancestor, and be Unix
mode `0700`. File roots must not include private metadata, host, or native trees,
or the web snapshot. Runtime/native trees must never become public assets. Codex
index overlap uses canonicalize only for comparison; the stored path stays exact
for no-follow readers. Plan §1 / §3 and `capabilities.mutations:false`: native
JSONL is never written; enabling metadata does not enable session mutations
([capabilities.md](capabilities.md#deliberately-kept-false)). Agent IDs resolve
through inventory ownership, never by joining client-provided paths. Before M8
shadow comparison the read model is **not** claimed fully compatible (plan
[M1](../BACKEND_MIRGRATION_PLAN.md#m1只读会话纵向链路第一二批持续推进)).

## File access
Rust does **not** import Python’s authenticated operator-wide filesystem
browser. Plan §5: that manager is a trusted-operator navigator, **not a cwd
sandbox**; development roots are not full file-manager compatibility. Authority is
the intersection of ([files.md](files.md)): (1) a selected SessionStore view
(exact UID/agent, complete semantic branch, cwd); (2) an explicit development file
root that contains the target. Neither cwd nor a client-supplied absolute path is
a grant. A path mentioned in native text is not filesystem authority. Directory
navigation stays inside that directory’s configured root. `files/boundary.rs`:
ambient authority is used only for the explicit volume root; every component is
opened relative and no-follow. Symlinks/reparse points, devices, sockets, and
pipes are rejected. A regular file with several hard links is readable: the
link count of an inode already inside an authorized root is not a boundary
(Python's bug-report attachments are `os.link`ed into project trees), whereas
a symlink can point outside the root and stays refused. Honest limit: a hard
link is created by the same user the service runs as and can only alias a
file that user may already read (Linux `protected_hardlinks`), so this
exposes the operator's own access — the same ambient access Python's file
reads always had — not a new principal. The write side
(`boundary::unshared`) still refuses to rename/move/delete a multiply linked
file, so one alias of shared data is never detached. Deleted entries go to
`<SESSIONDOCK_STATE_DIR>/file-trash` (private, 0700, no-follow verified),
never into a project tree. `FileVersion` is an opaque checked
identity (leaf stamp plus root-ancestor and parent identities). `ResolvedTarget`
is unforgeable — display path only; never reopen it. `CheckedReader` retains the
handle through streaming. HOME/URLs are not expanded. **TOCTOU covered (Linux
file tests):** ancestor / intermediate / leaf identity and opened-file metadata
around open and read; parent identities so replacing a directory is visible even
if a leaf inode is reused; concurrent replacement and per-chunk modification;
nonblocking no-follow opens so a swapped-in FIFO cannot become a blocking
ordinary-file read. **Explicitly not covered:** not a snapshot filesystem; not
protection from an administrator changing mounts, OS ACLs, or writing the same
inode while concealing metadata; already transmitted bytes cannot be recalled;
ReFS identifier limitation; Linux tests are not Windows/macOS or network-FS
acceptance. Plan current-diff: “不宣称目录句柄链级无竞态沙箱.” Plan M6 still
unchecked: “Session/agent scope 内的可信路径解析，symlink/越界/TOCTOU/Windows
路径测试.”

## Media tokens
Process-local 128-bit random tokens (32 lowercase hex). Clones of one
`NativeImage` share a token; a new parse gets a new one. `semantic_key()` is a
history/cursor digest, **not** authorization. No durable URL; an evicted
descriptor is 404. File-backed tokens bind canonical UID, optional exact agent,
normalized reference, and `FileVersion`. Every GET, including a warm cache hit,
reauthorizes current selected-view membership, explicit roots, ancestor/file
identity and version; it never reopens a display path and never fetches remote
URLs (`media_remote:false`). Unconfigured roots project `media_files_disabled`
501, not a `src`. Embedded tokens keep in-memory snapshot bytes until eviction;
they do not implement native-span revocation on a later rewrite
([native-input.md](native-input.md)).

## Terminal authority

`terminal::ownership` has no host discovery, PTY I/O, or authentication
middleware; it is **not** an authorization substitute
([terminal-ownership.md](terminal-ownership.md)). HTTP must enforce origin/loopback
and verify the name is in the explicit host inventory **before** claim. `claim`
issues a 15s reservation plus a 256-bit browser token; `bind` consumes it;
`is_current` gates operations; old cleanup cannot drop a replacement. Browser WS
is binary PTY plus JSON resize/revoked; **the ptyhost host token and local
framing are never exposed** (plan
[§3.3](../BACKEND_MIRGRATION_PLAN.md#33-发送终端与写操作)). Instance-bound leases
pin source/SID/UID plus host instance and attach only with `guarded_v1`; PID
existence is not incarnation proof ([terminal-identity.md](terminal-identity.md)).
No `SESSIONDOCK_PTYHOST_DIR` means no terminal service. Server restart invalidates
browser leases; it must not stop the independent host.

## Launch allowlists

Launcher JSON is a server allowlist, not browser input
([lifecycle-launcher.md](lifecycle-launcher.md#private-configuration)). HTTP may
supply only source, allowlisted adapter/profile ID, and cwd. Executable, argv, and
environment come from the retained file. Placeholders are whole arguments
`{session_id}` / `{sid}` only. Profile `env` allowlist: `HOME`, `PATH`, `TERM`,
`LANG`, `LC_*`, `XDG_*`, `ANTHROPIC_*`, `CLAUDE_*`, `CODEX_*`, `OPENAI_*`,
`GROK_*`, `XAI_*`, `AGENTHUB_TEST_*`; never `NODE_OPTIONS`. Always refused:
`CLAUDE_CODE_SESSION_ID`, `CODEX_COMPANION_SESSION_ID`, `GROK_SESSION_ID`,
`CODEX_THREAD_ID`, `CODEX_SESSION_ID`, `CLAUDE_PID`, `TMUX`, `AGENTHUB_SESSION`.
`env_clear` plus denylist; the Python login-shell wrapper is
not reproduced. **Not a sandbox** for hostile code, same-user filesystem mutation,
or bind-mount aliasing. Rust tests use fake CLI scripts; Python suites that
drive real CLIs as the system under test run in normal validation with the
cheapest model at low effort (AGENTS.md) and isolated homes.

## Diagnostics redaction

`POST /api/audit/browser` only with explicit `SESSIONDOCK_AUDIT_DIR`; else
`audit:false` and 501 ([diagnostics.md](diagnostics.md#what-is-stored)). Stored:
event, ts, redacted uid (`source:<hash>`), ids, severity, build, bounded `data`.
Never stored: `content`, unknown fields, request headers. Keys matching
`authorization` / `cookie` / `api_key` / `token` / `password` become `<redacted>`;
absolute path-shaped values become `<path>`. Server-side bug-report events
share the segments (batch 41, [bug-report.md](bug-report.md)). Not implemented:
per-route tracing, SQLite/blob storage.

## debug_run views

`?debug_run=<id>` is Python's view selector (`debug_runs.filter_rows`), not a
gate: the registry `<SESSIONDOCK_STATE_DIR>/debug-runs.json` (Python's format,
written by the test tooling, only read here, reloaded by stamp) hides every
registered monkey/test session from the ordinary list, live status, terminal
list/pending and search, and a `debug_run` view shows exactly that run; an
unknown or malformed id is an empty view. Detail routes ignore the parameter
(a hidden session still opens by uid, like Python). The metadata store
tolerates exactly `debug-runs.json` / `debug-runs.json.tmp` beside its own
files ([read-model.md](read-model.md#debug_run-视图python-agenthubdebug_runspy)).

## 501 ledger (unimplemented writes)

Plan §1: unimplemented writes return explicit 501 JSON; unknown routes 404; bad
input 400; permission 403. Router stubs in `api/mod.rs`
([plan §4](../BACKEND_MIRGRATION_PLAN.md#4-路由迁移账本)): `POST /api/session/send`,
`draft-status`, `outbox/retry`, `outbox/discard`, `rewind`; `POST
/api/sessions/delete`; `POST /api/term/scroll`, `term/send`; `POST
/api/session/stop` (batch 29: managed instances only, 501
`session_stop_unmanaged` otherwise); `POST /api/session/attachment`, `files/action`, `files/upload`;
`/api/trash`, `trash/restore`, `trash/purge`; `POST /api/bug-report` (batch 41:
`501 bug_report_disabled` without `SESSIONDOCK_BUG_REPORT_DIR`/`REPO`, audit,
terminal, lifecycle and a worker profile; `501 bug_report_model_policy` when the
profile does not pin the cheapest model, [bug-report.md](bug-report.md));
`/api/session/{uid}` (legacy delete). Also 501 when unconfigured or still refused:
audit; outbox without a delivery ledger; file jobs / thumbnails / mutations;
`takeover` with `force:true`; rename; Grok native-scope
([delivery-scope.md](delivery-scope.md)). Do not fabricate empty success.

## Assumption → enforcement → test

| Assumption | Where enforced | Test that proves it |
| --- | --- | --- |
| Loopback bind; no public listener | `config.rs` `validate` | [`api_smoke.py`](../tests/api_smoke.py) (isolated loopback; Host/Origin unit coverage is cargo `http`) |
| Host / Origin / Hub-header rejection | `security.rs` `local_only` | [`legacy_browser.py`](../tests/legacy_browser.py) (loopback page); cargo `http` for 403 cases; [`security_suite.py`](../tests/security_suite.py) (real credential pair still 403) |
| Node listener: four settings or nothing; peer ∈ CIDR; protocol `1`; constant-time token; no static page | `config.rs` `validate_node`, `api/node_auth.rs` | cargo `node_auth`, [`node_auth_suite.py`](../tests/node_auth_suite.py), [`check_config_suite.py`](../tests/check_config_suite.py) (`node_*` cases) |
| No CLI-home discovery; disjoint roots | `config.rs` `from_env` / `validate` | [`files_browser.py`](../tests/files_browser.py) (explicit `FILE_ROOTS` only) |
| Native histories read-only; writes 501 | `api/mod.rs` stubs; `mutations:false` | [`api_smoke.py`](../tests/api_smoke.py), [`history_parity.py`](../tests/history_parity.py) |
| File: selected-view ∩ explicit roots; no-follow | `files/boundary.rs` | [`files_browser.py`](../tests/files_browser.py) |
| Media tokens process-local; GET reauth; no remote fetch | media store; `media_remote:false` | [`media_browser.py`](../tests/media_browser.py), [`media_files_browser.py`](../tests/media_files_browser.py), [`media_parity.py`](../tests/media_parity.py) |
| Current-branch native media authority | span/envelope GET | [`native_spans_authority.py`](../tests/native_spans_authority.py), [`native_envelopes_authority.py`](../tests/native_envelopes_authority.py) |
| Terminal lease; host token not on WS | ownership + term HTTP | [`terminal_browser.py`](../tests/terminal_browser.py), [`managed_terminal_browser.py`](../tests/managed_terminal_browser.py) |
| Instance identity / guarded attach | ptyhost `guarded_v1` | [`host_identity.py`](../tests/host_identity.py) |
| Launch allowlist, placeholders, env allow/deny | launcher config | [`lifecycle_browser.py`](../tests/lifecycle_browser.py), [`lifecycle_cli_browser.py`](../tests/lifecycle_cli_browser.py) |
| Diagnostics redaction; 501 if unset | audit service | [`audit_browser.py`](../tests/audit_browser.py) |
| Unimplemented writes stay 501 | `api/mod.rs` ledger | [`api_smoke.py`](../tests/api_smoke.py), [`route_ledger.py`](../tests/route_ledger.py) |

Linux compilation is not Windows/macOS validation. File and launcher checks are **not** a sandbox.
