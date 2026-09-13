# Glossary

Contract terms from repository docs and code comments only; not a product spec.

## Names

**SessionDock.** This Rust project: crate and binary `sessiondock`, hub binary `sessiondock-hub`, environment prefix `SESSIONDOCK_` ([environment.md](environment.md)), test hook `SESSIONDOCK_TEST_PTYHOST_BINARY`, `localStorage` namespaces `sessiondock.` / `sessiondock.hub.<path>.`, `/api/meta` hostname fallback `SessionDock` (the default is the system host name since batch 44 WP-C), the installable identity (`manifest.webmanifest` name/short_name, `apple-mobile-web-app-title`, service-worker cache `sessiondock-shell-*`, page titles `… · SessionDock`), deployment unit `sessiondock.service` under the prefix `/srv/sessiondock` behind the proxy location `/sessiondock/`. Internal server leases use `sessiondock-delivery-executor` and `sessiondock-bug-report`; these transient page names are not persisted ledger formats. UIDs are path hashes and did not change with the name. User-facing product names say SessionDock. References to Python, historical artifacts and compatible identifiers keep their original names as listed below.

**Kept `agenthub` identifiers.** The wire/DOM contract shared with the Python frontend and Python Hub nodes keeps its spelling: headers `X-AgentHub-Protocol`, `X-AgentHub-Node-Token`, `X-AgentHub-Page`, `X-AgentHub-Trace`, `X-AgentHub-Build`, `X-AgentHub-Decoded-Length`; template markers `__AGENTHUB_MODE__`, `__AGENTHUB_HOSTNAME__`, `__AGENTHUB_ASSET_VERSION__`; meta tags `agenthub-capabilities`, `agenthub-mode`; the JS globals and identifiers `AgentHubCapabilities`, `agenthubCli`, `AGENTHUB_CLIS`, `agenthubHighlight*`, `__agenthubConnectionId`, `__agenthubPageId`, `agenthub-build`, `agenthub-highlight-ready`, `AgentHubFilePreview`, `AgentHubTypography`, the `AgentHub …` font family names and the `agenthub-files-*` / `agenthub-shell-` page storage keys; the Python namespaces `agenthub.` / `agenthub.hub.<path>.` used when `storage_namespace` is empty; the persisted ledger format tags `agenthub-delivery` / `agenthub-lifecycle` (renaming them would orphan deployed ledgers); the Python project name `agenthub` and its paths (`../agenthub`, `agenthub/static`, `~/.local/share/agenthub`, `agenthub_attachments/`, `.agenthub-trash`, `.agenthub-upload`, `deploy/agenthub*.service`, `nginx-agenthub*.conf`); the Python-side environment names quoted in comparison tables (`AGENTHUB_HOST_*`, `AGENTHUB_TERM_BACKEND`, `AGENTHUB_SESSION`, which ptyhost still exports); the shared fake-CLI test-hook prefix `AGENTHUB_TEST_*` and the hooks under it (`AGENTHUB_TEST_LABEL`, `AGENTHUB_TEST_CLAUDE_ROOT`, `AGENTHUB_TEST_CODEX_ROOT`, `AGENTHUB_TEST_FREE_SHELL_BINARY`, …), `AGENTHUB_PYTHON_SOURCE`, `AGENTHUB_DELIVERY_TEST_CHILD_DIRECTORY`; and the imported crates `ptyhost` / `ptyhost-client` with `AGENTHUB_HOST_DIR`. `reference/legacy-web/` is Python's frozen static tree and is never renamed.

## Identifiers

**UID.** Local list key `source:sha1(path)[:16]`; Hub node namespaces must not mix UUIDs. It is an opaque inventory key; host metadata associates it with its source. See [history pages](history-pages.md).

**SID.** Native session id from the record (`sessionId` / `session_meta.payload.id`), listed beside `uid/source/sid`. Display `sid`, filenames and cursor hashes are not native identity. See [native scope](delivery-scope.md#trusted-native-scope-selection).

**Canonical owner UID.** Inventory owner of a view. History-page and media grants bind this UID; Claude/Codex children keep the parent's owner UID, never a child-path UID. See [history pages](history-pages.md#checkpoints-lifetime-and-errors).

**Agent id.** Exact owned inventory entry from `?agent=`. Prefixes, `agent-` filenames, another owner's child, a child UID or a client path do not bypass ownership. See [scope evidence](delivery-scope.md#identity-evidence-and-snapshot-consistency).

## Native history

**Native record.** One committed JSONL object projected by `providers/` (that layer does not open files). One record may emit several events. See [read model](read-model.md).

**Index (lazy).** The session list source: directory walk + stat + a bounded head (96 KiB, ≤ 40 records) and tail (512 KiB) summary per file, cached by file stamp (dev/ino/size/mtime_ns), read in parallel; no startup parse, no session/byte caps, one file's change never fails the list. Ownership/fork graphs, native ids for the runtime catalog, lifecycle resume and trash file sets come from these summaries. See [read model](read-model.md).

**Row summary.** The per-file result of the bounded head/tail read: title (custom > ai > generated), cwd, branch, created, model, native id, fork/agent relations, `supported` and `migration_warnings`. Derivations mirror the Python adapters' `list_sessions`. See [read model](read-model.md).

**Logical view.** On-demand projection of one opened session's selected branch/agent: events, parent cuts and native-scope evidence, built only when the session is opened and kept in a bounded LRU; the session list never builds views. See [read model](read-model.md).

**Event.** Display unit after provider projection. Indices count all non-status events; several events may share an offset, and inherited events may have offset zero. See [history pages](history-pages.md#wire-contract).

**Message.** Public JSON built from those events for HTTP/SSE. Codex/Grok may group a content array into one message; Claude can emit one visible event per block. See [media grouping](media.md#recognition-and-grouping).

**counted:false.** Excludes an event from `message_total`. Status records form activity, not body; page indices still include `counted:false`. See [history pages](history-pages.md).

## Cursors

**Byte cursor (`start`/`end`/`head`).** UTF-8 offsets in the selected leaf file, not message indexes. Public `end` is that file's committed EOF; `head` lives on `version`. Unfinished JSONL lines are not consumed. See [history pages](history-pages.md).

**Semantic anchor.** Cursor field binding view identity, the fixed inherited prefix and already displayed events (`rs-m1-2` / `rs-m2-1`). A branch switch or parent-prefix rewrite resets even on a leaf-only append. See [read model](read-model.md).

**Checkpoint.** Physical LF offset/digest (`RawIndex`) and the live byte/semantic snapshot a grant pins. Gap-page tokens stay separate from the live append checkpoint. See [physical input](native-input.md#native-input-and-structural-scanning).

**Reset.** Required on truncate, replace, same-size rewrite, or semantic fork/prefix mismatch. SSE then sends a bounded window, not an unbounded replay. See [history pages](history-pages.md).

**Append.** Ordinary new leaf bytes: incremental messages, old page grants and unchanged native spans may be kept. Not an incremental timeline parser; last-prompt/abort can still rewrite older events and reset. See [append cache](append-cache.md#semantics-deliberately-recalculated).

## Windows and pages

**Window.** First-screen slice of already parsed semantic history (`?window=1`): up to 500 latest then 100 earliest events, shrunk by media/JSON budgets. Not an unparsed file tail. See [history pages](history-pages.md#wire-contract).

**History page.** `GET /api/messages/{uid}/page` filling the omitted middle range. No live `end`/`head`/`anchor`; inserting a page must not move the append checkpoint or watch stream. See [history pages](history-pages.md#wire-contract).

**Media page.** `GET /api/messages/{uid}/media-page` continuing the typed images of one message (`media_more`). A page read never moves the live cursor. See [media continuation](media.md#continuation-and-errors).

**Grant.** In-memory 128-bit token in `PageStore` (1024 entries, ten minutes, oldest eviction) binding canonical owner UID, exact agent and the producing view's checkpoint. Shared by history pages and media continuation. See [grants](history-pages.md#checkpoints-lifetime-and-errors).

## Media

**Native span.** Private `TextSpan`/`NativeSpan`: physical quote range, decoded length/SHA-1 and optional `DecodePlan`. GET re-checks full current-branch membership; a private text span is not filesystem or image authority. See [spans](native-input.md#tool-envelopes-and-native-images).

**Inline image.** Image data embedded in native history. Python’s 32 MiB decoded-image limit applies equally to inline data and retained native spans. See [media limits](media.md#size-and-cache-behavior).

**File reference.** Markdown or raw path discovered in projected text, or a typed native file block. Discovery does not open files; HTTP resolves the reference against the selected session and its working directory. See [text discovery](media.md#file-references).

**Descriptor.** Registered private source (`src` token, no decoded bytes). Its retention is separate from decoded-image caching. See [media cache](media.md#size-and-cache-behavior).

**Blob.** Decoded bytes in `MediaBlob`, retained in an evicting cache. Cache pressure does not reject a valid GET; GET materializes on demand; descriptor eviction is 404 even if a blob remains. See [media cache](media.md#size-and-cache-behavior).

## Nested decode

**DecodePlan.** One outer physical range and inner `StringRange`s whose offsets address the preceding **decoded** stream, never file bytes. See [envelopes](native-input.md#tool-envelopes-and-native-images).


**Replay.** GET rebuilds the layered reader from the immutable plan, verifies each layer's length/SHA-1 and outer EOF, then publishes. Identical image bytes are not enough if a parent digest changed. See [envelopes](native-input.md#tool-envelopes-and-native-images).

## Topology

**Fixed parent prefix (`history_base`).** Codex `{thread_id, end_byte_offset}` cut: only `[0, cut)` is inherited, reparsed on a complete JSONL boundary. Missing/inconsistent `history_base` on a fork is unsupported; the leaf byte cursor stays separate. See [history pages](history-pages.md).

**Fork.** A distinct native session that inherits a parent prefix. Forks keep their own SID/UID; they are not subagents. See [history pages](history-pages.md).

**Subagent.** Child agent owned through inventory (`?agent=`), not by joining paths. `forked_from_id` on a subagent is ownership, not necessarily history; subagents cannot be the main native bind. See [native scope](delivery-scope.md#trusted-native-scope-selection).

**Sidechain.** Claude side branch in the main view; it does not choose the main leaf. Compact/rewind/sidechain chains are covered by advanced parity, not by guessing hidden Python trees. See [read model](read-model.md).

**Compaction.** Claude compact events that rejoin across the tree and hide finished abandoned branches (two compact forms). Codex also filters compacted/internal context. See [read model](read-model.md).

**Rewind.** Native branch signal already on disk; live CLI-screen rewind and durable pin are not opened as writes. Page grants treat rewind/prefix change as 409 unless the full checkpoint still matches. See [history pages](history-pages.md).

**Last-prompt.** Claude record that cuts the active leaf. An append-only last-prompt can withdraw older displayed events and reset. See [read model](read-model.md).

## Host and terminals

**ptyhost.** Imported per-session PTY host: JSON-line control, then `kind:u8 + length:u32 BE + payload` after attach. Always pass an explicit development `--dir`. See [ptyhost](session-host.md#ptyhost-开发说明).

**Host record.** Private ptyhost directory row. Probe rereads it before/after Info; name, PIDs, times, endpoint, token, cwd and reviewed metadata must agree. Liveness uses only processes named by a verified record. See [process evidence](processes.md#evidence-not-name-inference).

**Instance id.** Immutable per-process nonce (16–128 ASCII) in spawn metadata. Bound leases pin UID plus this nonce; PID or SID alone is not an incarnation. See [launch identity](host-launch-identity.md#capability-and-request).

**Launch id.** Immutable spawn nonce carried by `launch_guard_v1` / `launch_bind_v1` with source and instance. It is not a native SID/UID. See [launch vs native](host-launch-identity.md#deliberately-separate-from-native-association).

**Lease.** In-memory page/connection write right (`BoundLease`). A claim reservation lasts 15 s; a live bound connection does not. Restart drops every browser lease and must not kill the host. See [ownership](terminal-ownership.md#api-boundary).

**Claim.** `claim` / `claim_bound` reserves a verified host name (or unique native UID+instance) for one page. Force notifies the previous page; a failed fresh probe does not revoke an existing lease. See [bound leases](terminal-ownership.md#instance-bound-service-leases).

**Bind.** Ownership `bind` consumes an unexpired reservation once and allocates a connection id. Operator `POST /api/term/bind` and host `launch_bind_v1` are separate one-time native associations. See [ownership](terminal-ownership.md#api-boundary).

**guarded_v1.** Outer host operation matching immutable instance/source/SID (or UID) before the inner request. Success carries `instance_guard`; it is not `launch_guard_v1` and must not wrap or be wrapped by launch envelopes. See [instance protocol](terminal-identity.md#backward-compatible-envelope).

## Launch and run state

**Pending identity.** Create/attach key is the receipt + `launch_id` + `instance_id` tuple (`new_pending` for Codex/Grok). It never authorizes native binding or reliable send. See [pending](lifecycle-http.md#pending-terminal-identity-and-cancellation).

**Declared identity.** `declared_sid` / `declared_uid` on a launch (`new_assigned` or resume). Binding a launch that already declared identity is `409 launch_identity_declared`. See [launch kinds](lifecycle-http.md#launch-identity-kinds-batch-24).

**Running.** Host Info `exited:false` and verified child/host identity for a uniquely matched UID (`evidence:"host_info"`). See [three states](processes.md#process-identity-and-the-three-run-states-batch-22).

**Exited.** Host `exited:true`, a confirmed lifecycle exit receipt for the same instance, or (Linux, this Web process only) a previously verified identity gone from `/proc`. See [three states](processes.md#process-identity-and-the-three-run-states-batch-22).

**Unknown.** Typed reason (`no_instance`, unreachable, duplicate, unverifiable, unsupported platform, …). Managed observations retain this state when they cannot prove running or exited. See [three states](processes.md#process-identity-and-the-three-run-states-batch-22).

## Delivery

**Delivery ledger.** Isolated `delivery-ledger.json` opened from an explicit private directory; missing data is not permission to initialize. Independent of native history and of send execution. See [store](delivery-store.md#one-envelope-existing-provider-schemas).

**Outbox.** `GET /api/session/outbox` is a committed display projection of receipts for a verified NativeScope. `outbox_read` does not enable the `outbox` send/retry/discard capability. See [delivery HTTP](delivery-http.md).

**Receipt.** Durable delivery-domain row (Codex/Claude state machine) or a lifecycle creation record. HTTP/terminal write success is not native acknowledgment. See [delivery states](delivery.md#interface-and-state-transitions).

## Flags and parity

**Capability flags.** HTML/`/api/meta` booleans such as `history_pages`, `media_lazy`, `media_continuation`, platform-dependent `live`, and `outbox` versus `outbox_read`. Ptyhost Info uses integer `instance_guard` / `launch_guard` / `launch_bind`. See [capabilities](capabilities.md).

**DELTA.** A named, asserted Python/Rust difference in a parity tool. Disappearance or a different result fails. See [media parity](media-parity.md).

**UNVERIFIED.** A parity difference that is not a documented DELTA and not a clear FAIL. Advanced history parity requires every remaining difference to be a DELTA, never UNVERIFIED. See [validation](../AGENTS.md#validation).
