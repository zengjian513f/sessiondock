# Module map

This file is produced by `tests/module_map.py`. It maps every `.rs` file under
the crate `src` trees below with the first module doc line, line count and
`mod` declarations. Regenerate:

```sh
python3 tests/module_map.py --write
```

## sessiondock

`crates/sessiondock/src`: 193 files, 82999 lines, 5 undocumented.

- `api/`
  - `audit.rs` — `POST /api/audit/browser`: bounded browser diagnostics intake. (79 lines)
  - `bug_report.rs` — `POST /api/bug-report`, `POST /api/bug-report/capture` and the (769 lines)
  - `conversation.rs` — Thin HTTP transport for server-owned conversation drafts, staging and one-shot SEND. (652 lines)
  - `events.rs` — Node UI invalidations share one cached observer. (37 lines)
  - `files.rs` — File transport. Every request resolves the selected session; opening a (522 lines)
  - `final_screen.rs` — `GET /api/term/final?id=…`: the final screen an exited session left, as one (54 lines)
  - `health.rs` — Local liveness JSON: version, `api_version` 1, and `stage: "read_only"`. (49 lines)
  - `hub.rs` — The hub's HTTP surface (`hub.py` `HubHandler.dispatch`, 518–584), served (953 lines)
  - `lifecycle.rs` — Explicit creation receipts; native identities and reliable send stay separate. (1277 lines)
  - `media.rs` — Opaque media transport. File tokens require current native-scope authorization. (150 lines)
  - `metadata.rs` — SessionDock-owned preferences only. No native session writes or CLI actions. (542 lines)
  - `mod.rs` — Axum transport router nested at `/api`. Handlers stay in sibling modules; (390 lines)
    - mods: `audit`, `bug_report`, `conversation`, `events`, `files`, `final_screen`, `health`, `hub`, `lifecycle`, `media`, `metadata`, `node_auth`, `process_links`, `read`, `runtime`, `search`, `shell_env`, `terminal`, `transfer`, `trash`, `trash_tree`
  - `node_auth.rs` — Node listener gate (`server.py` `_allowed` / `_hub_protocol` for hub (103 lines)
  - `process_links.rs` — (no module doc) (130 lines)
  - `read.rs` — Read-only session list, messages, grant pages, input history, and SSE watch. (745 lines)
  - `runtime.rs` — Read-only live status; never upgrades observations into CLI authority. (533 lines)
  - `search.rs` — JSON/NDJSON search transport queues work and applies stream backpressure. Search (183 lines)
  - `shell_env.rs` — `GET /api/shell-env` and `POST /api/shell-env/restart` (`crate::shell_env`). (39 lines)
  - `terminal.rs` — Explicit-directory development transport only; legacy CLI actions stay gated. (1185 lines)
  - `transfer.rs` — Browser-facing clone orchestration. Paths and identity maps never come from (937 lines)
  - `trash.rs` — Session recycle-bin HTTP routes, using delete protection. (642 lines)
  - `trash_tree.rs` — Preview and confirm the same connected component used by whole-group copy. (163 lines)
- `assets.rs` — Startup snapshot of regular frontend files. Requests never walk the disk. (282 lines)
- `audit.rs` — Best-effort browser diagnostics intake. (358 lines)
  - mods: `intake`, `query`, `writer`
- `audit/`
  - `intake.rs` — Request-shape validation and structured-metadata sanitization. (265 lines)
  - `query.rs` — Bug-report audit: server-side structured events into the same JSONL (204 lines)
  - `writer.rs` — Dedicated writer thread: daily JSONL files with fourteen-day retention. (310 lines)
- `bin/`
  - `sessiondock-hub.rs` — The multi-machine hub. (199 lines)
  - `sessiondock-transfer.rs` — Offline transfer inspection/staging tool. No native publish or source cleanup. (146 lines)
- `bridge/`
  - `claude.rs` — Claude question cards. (440 lines)
  - `codex.rs` — Codex command approvals. (217 lines)
  - `live.rs` — The live `prompt` of a session view. (235 lines)
  - `menus/`
    - `agy.rs` — Agy 1.2.16/1.2.17 menus verified in an isolated real PTY. Unverified forms stay (314 lines)
    - `claude.rs` — Claude Code 2.1.285's live selection surfaces. (703 lines)
    - `codex.rs` — Screen-only Codex menus, audited against the installed rust-v0.159.2 release. (637 lines)
    - `grok.rs` — Native Grok Build screen menus. Provenance and exceptions are in the menu inventory. (566 lines)
    - `mod.rs` — Current-screen projection of native CLI menus. Parsing describes actions; (18 lines)
      - mods: `agy`, `claude`, `codex`, `grok`, `opencode`
    - `opencode.rs` — OpenCode v2.0.18 screen menus (official tag cd9a14a6b688d4021bee381dfd39d2cef9c0f862). (663 lines)
  - `mod.rs` — CLI question cards and approvals shown live on the conversation page: (14 lines)
    - mods: `claude`, `codex`, `live`, `menus`
- `bug_report/`
  - `mod.rs` — Bug-report bundles and their CLI workers. (1037 lines)
    - mods: `worker`
  - `worker.rs` — The bug-report worker. (465 lines)
- `config.rs` — SessionDock configuration. Paths come from explicit environment variables; (658 lines)
- `conversation/`
  - `cli_state.rs` — Per-session CLI state that native history cannot tell (docs/cli-state.md): (623 lines)
  - `input.rs` — Positive recognition of the CLI input surface, shared by CHECK and every (487 lines)
  - `mod.rs` — One conversation send path: drafts are server-owned, successful SEND belongs to the CLI. (1107 lines)
    - mods: `cli_state`, `input`, `report_name`, `rewind`, `store`
  - `report_name.rs` — Name a report through the Codex TUI before sending its first model task. (149 lines)
  - `rewind.rs` — A rewind made in Claude's own TUI (double Esc, restore the conversation) (134 lines)
  - `store.rs` — Session-owned drafts and one-shot submission identities. No CLI acknowledgment queue. (921 lines)
- `delivery/`
  - `driver.rs` — Terminal driver for conversation SEND and the bug-report worker. (1208 lines)
  - `mod.rs` — Server-side terminal writes for conversation SEND and the bug-report (6 lines)
    - mods: `driver`, `target`
  - `target.rs` — Managed-instance target resolution and the failure type shared by (163 lines)
- `error.rs` — HTTP JSON error envelope `{error, code}` shared by Axum handlers. (68 lines)
- `files/`
  - `boundary.rs` — (no module doc) (467 lines)
  - `grants.rs` — An authenticated file browser keeps its directory grant after the original (168 lines)
  - `media.rs` — One trusted selected-view reference index shared by an entire media window. (113 lines)
  - `mod.rs` — Session-reference-scoped file reads and authenticated directory browsing. (317 lines)
    - mods: `boundary`, `grants`, `media`, `references`, `response`, `write`
  - `references.rs` — (no module doc) (282 lines)
  - `response.rs` — (no module doc) (519 lines)
  - `write.rs` — Authenticated operator file mutations through checked parent handles. (949 lines)
- `fingerprint.rs` — 128-bit streaming content fingerprint of native bytes. (126 lines)
- `hub/`
  - `aggregate.rs` — Hub aggregation (`hub.py` `HubHandler.selected/aggregate/search_aggregate/ (889 lines)
  - `client.rs` — Hub → node HTTP/1.1 client: one connection per request over a plain or (835 lines)
  - `groups.rs` — Nodes own group catalogs. The Hub caches their union and sends it back; (143 lines)
  - `identity.rs` — Node identity and credential files (`federation.identity`, `server.py` (112 lines)
  - `mod.rs` — Hub federation (batches 38–40): node identity, the hub's node registry (32 lines)
    - mods: `aggregate`, `client`, `groups`, `identity`, `namespace`, `nest`, `process_links`, `proxy`, `registry`, `resources`, `session_links`, `transfer`
  - `namespace.rs` — The hub's wire namespace (`federation.py` 31–113): every reference a node (315 lines)
  - `nest.rs` — Validate a display edge against the fleet before routing the write to its child. (60 lines)
  - `process_links.rs` — One fleet coordinator shared by session nesting and external CPU consumers. (121 lines)
  - `proxy.rs` — The hub's pass-through to one node (`hub.py` `HubHandler.resolve` (912 lines)
  - `registry.rs` — The hub's node registry (`hub.py` `Registry`): `hub-nodes.json`, node (1421 lines)
  - `resources.rs` — Session resource views preserve execution nodes and incomplete observations. (340 lines)
  - `session_links.rs` — Resolve saved links through completed transfer records, never by guessing IDs. (252 lines)
  - `transfer.rs` — Durable cross-node clone orchestration over the authenticated node channel. (1302 lines)
- `hub_config.rs` — Configuration of the `sessiondock-hub` binary. Separate from (119 lines)
- `lib.rs` — Loopback development HTTP crate: config, router, and optional isolated services. (589 lines)
  - mods: `api`, `assets`, `audit`, `bridge`, `bug_report`, `config`, `conversation`, `delivery`, `error`, `files`, `fingerprint`, `hub`, `hub_config`, `lifecycle`, `list_sync`, `log`, `media`, `metadata`, `native_replay`, `observe`, `polls`, `runtime`, `search`, `security`, `sessions`, `shell_env`, `state`, `terminal`, `transfer`, `trash`, `ui_events`, `session_links`
- `lifecycle/`
  - `autobind.rs` — Process-evidence binding of pending Codex/Grok launches. (326 lines)
  - `clients.rs` — Installed agent CLI versions and manual updates for the machine settings. (562 lines)
  - `launcher.rs` — Configured adapters and one-authority process spawn. No discovery, (1160 lines)
  - `mod.rs` — Isolated durable process-creation intent. No launcher or native-session binding. (8 lines)
    - mods: `autobind`, `clients`, `launcher`, `model`, `models`, `service`, `store`
  - `model.rs` — Private creation-intent types: `LaunchSpec`, `Record`, and `BindingSpec`. (620 lines)
  - `models.rs` — Read-only model catalogs for the new-session picker. Each CLI's own list: (611 lines)
  - `service.rs` — Isolated lifecycle coordinator. HTTP cancellation never owns a spawn. (1743 lines)
  - `store/`
    - `disk.rs` — Adapted locally from delivery/store/disk.rs; keep its reviewed durability (211 lines)
    - `json.rs` — Strict JSON grammar without duplicating the lifecycle receipt schema. (99 lines)
    - `mod.rs` — Single-writer durable creation receipts. No process, native history or HTTP I/O. (782 lines)
      - mods: `disk`, `json`
- `list_sync.rs` — Opt-in list transport. Cached revisions only save bytes: eviction, restart, (353 lines)
- `log.rs` — Structured server log: one JSON object per line on stderr (journald keeps (132 lines)
- `main.rs` — Loopback development binary. No option starts the Web service. (329 lines)
- `media.rs` — Private image projection. File capabilities require an explicit selected scope; (616 lines)
  - mods: `descriptors`, `discovery`, `file_media`, `formats`, `native_media`
- `media/`
  - `descriptors.rs` — Bounded source capabilities, separate from decoded bytes. No snapshot or (299 lines)
  - `discovery.rs` — Pure text discovery only. A candidate is not filesystem authority. (248 lines)
  - `file_media.rs` — File capabilities are revalidated against the current selected native view. (201 lines)
  - `formats.rs` — Image decoding belongs to the client. SessionDock does not add container, (9 lines)
  - `native_media.rs` — Private native-string descriptors. Paths are metadata, never open authority. (370 lines)
- `metadata/`
  - `disk.rs` — Metadata reads and atomic replacement in the configured directory. (112 lines)
  - `mod.rs` — SessionDock preferences with reads and atomic publication. (214 lines)
    - mods: `disk`, `model`
  - `model.rs` — Pure, versioned metadata transformations. No process or native-file access. (712 lines)
    - mods: `transfer`
  - `transfer.rs` — Atomic comparison and restoration of display rows during history imports. (63 lines)
- `native_replay.rs` — Checked-source-independent replay of nested JSON string interiors. (219 lines)
- `observe.rs` — One bounded publisher per logical view. Subscribers keep their own cursor; (217 lines)
- `polls.rs` — Response caches of the two liveness polls every open tab repeats every (200 lines)
- `runtime/`
  - `freeze.rs` — Linux diagnostic process-tree pause. The independent PTY host stays live. (146 lines)
  - `mod.rs` — Read-only controlled-host observations against a frozen native inventory. (1426 lines)
    - mods: `freeze`, `process`, `process_links`, `procscan`, `spawn`
  - `process.rs` — Process identity evidence for host-managed instances. (466 lines)
  - `process_links.rs` — Node adapter for the shared process-links protocol. Remote links are held (748 lines)
  - `procscan.rs` — Read-only `/proc` scan for external CLI processes. (1114 lines)
    - mods: `activity`
  - `procscan/`
    - `activity.rs` — Work that outlives a turn, including detached commands carrying a native (168 lines)
  - `spawn.rs` — Initialize the single sidebar parent from an observed local CLI launch. (325 lines)
- `search.rs` — Bounded on-demand search over semantic session views, never raw JSONL. (874 lines)
  - mods: `cache`, `fold`, `prefilter`, `service`
- `search/`
  - `cache.rs` — Persistent search-text cache: one file per main session holding the exact (865 lines)
  - `fold.rs` — Case folding for the search prefilter, and the whole-word boundary class. (208 lines)
  - `prefilter.rs` — The candidate filter of one query over the folded copies of the cached (163 lines)
  - `service.rs` — Search-text production: where the body of one candidate comes from, in (383 lines)
- `security.rs` — Loopback Host gate and same-origin API policy. Not authentication. (130 lines)
- `session_links.rs` — Durable transfer identities and read-only external-link resolution. (146 lines)
- `sessions/`
  - `agy.rs` — Read-only Agy catalog and complete transcript projection. The binary (347 lines)
  - `history.rs` — View identity helpers shared by `views` (`native_identity`, `history_link`, (87 lines)
  - `index/`
    - `agent_stops.rs` — Subagent stop evidence from the owner's native transcript. (267 lines)
    - `graph.rs` — Ownership and fork graph over row summaries. (853 lines)
    - `mod.rs` — Lazy session index: directory walk + `stat` + bounded (1582 lines)
      - mods: `agent_stops`, `graph`, `names`, `native_state`, `summary`, `titles`
    - `names.rs` — Codex `session_index.jsonl` names applied to summary rows. (200 lines)
    - `native_state.rs` — Scalar Codex turn and Claude/Codex model state, independent of head/tail windows. (177 lines)
    - `summary/`
      - `agy.rs` — Agy row summary from the SessionDock mirror (`sessions::agy`): (113 lines)
      - `claude.rs` — Claude row summary: `ClaudeAdapter._meta` (main transcripts, including (471 lines)
      - `codex.rs` — Codex row summary: `CodexAdapter._raw_meta` (120 head pieces) plus the (308 lines)
      - `grok.rs` — Grok row summary: `GrokAdapter.session_meta` from `summary.json` plus the (164 lines)
      - `mod.rs` — Bounded per-file row summaries. (880 lines)
        - mods: `agy`, `claude`, `codex`, `grok`, `opencode`
      - `opencode.rs` — OpenCode row summary from the SessionDock mirror (`sessions::opencode`): (104 lines)
    - `titles.rs` — Codex native names read on demand, before the first rollout exists. (19 lines)
  - `media_projection.rs` — Window selection precedes file opens; authority uses the complete branch. (216 lines)
  - `mod.rs` — Session read model: the lazy index (`index/`) is the only inventory, and (1439 lines)
    - mods: `history`, `index`, `agy`, `native_input`, `native_media`, `opencode`, `pages`, `views`, `providers`, `records`, `scope`, `sidebar`, `media_projection`
  - `native_input.rs` — Checked, chunked native input and a disposable raw-prefix index. (336 lines)
  - `native_media.rs` — Native span authority is the current full selected branch, not file_roots (89 lines)
  - `opencode.rs` — OpenCode session mirror. (628 lines)
  - `pages.rs` — Finite history pages. Grants hold checkpoints, never retained native views. (662 lines)
  - `providers.rs` — Pure native-record projection. File discovery, inheritance cutoffs and (1354 lines)
    - mods: `agy`, `claude`, `envelopes`, `grok`, `image_content`, `opencode`, `tools`
  - `providers/`
    - `agy.rs` — Agy's complete, system-generated transcript (not the truncated transcript). (207 lines)
    - `claude.rs` — Claude's append-only transcript is a tree, not a flat event log. (1007 lines)
    - `envelopes.rs` — Source-specific display envelopes. Never interpret arbitrary HTML as protocol (275 lines)
    - `grok.rs` — Summary-derived Grok metadata. The transcript never overrides these fields, (166 lines)
    - `image_content.rs` — Typed image extraction before native content becomes public text or JSON. (195 lines)
    - `opencode.rs` — OpenCode 2 message rows from the SessionDock mirror (`sessions::opencode`). (178 lines)
    - `tools.rs` — Pure presentation of known tool arguments. A shell command is text here: (1053 lines)
  - `records.rs` — Bounded, disposable JSON AST reuse. Never an incremental timeline parser. (353 lines)
    - mods: `native_images`, `native_records`, `scanner`, `string_reader`, `tool_envelopes`
  - `records/`
    - `native_images.rs` — Private structural image authority. JSON paths only locate already-reviewed (532 lines)
    - `native_records.rs` — Pull-based complete-record scanning. Large strings stay private spans; the (194 lines)
      - mods: `replay_source`
    - `native_records/`
      - `replay_source.rs` — Replays only reviewed tool strings from the stamped current native record. (184 lines)
    - `scanner.rs` — Private streaming JSON structure scanner. This is not a media classifier or (801 lines)
    - `string_reader.rs` — A bounded decoder for the physical INSIDE of one JSON string (no quotes). (255 lines)
    - `tool_envelopes.rs` — Streaming discovery of a known Codex tool envelope inside ONE (244 lines)
  - `scope.rs` — Identity provenance captured once from already parsed, committed records. (108 lines)
  - `sidebar.rs` — Compact sidebar projections: child counts are cheap; child rows are opt-in. (92 lines)
  - `views/`
    - `body.rs` — Byte rendering of message batches: the `/api/messages` document and the (273 lines)
    - `encoded.rs` — Serialized message bytes of one projected file, kept next to its events (256 lines)
    - `mod.rs` — Per-session views on demand: one opened session is (2022 lines)
      - mods: `body`, `encoded`
- `shell_env.rs` — Login-shell environment drift. (224 lines)
- `state.rs` — (no module doc) (210 lines)
- `terminal/`
  - `device.rs` — Coarse device label from a browser `User-Agent` for ownership prompts. (44 lines)
  - `final_screen.rs` — Read-only access to the final screens exited ptyhost sessions leave under (165 lines)
  - `input.rs` — Raw HTTP terminal input: named-key mapping and host protocol bounds. (165 lines)
  - `mod.rs` — Pure browser ownership plus explicitly configured local PTY transport. (32 lines)
    - mods: `device`, `final_screen`, `input`, `ownership`, `receipts`, `service`
  - `ownership.rs` — Exclusive browser terminal leases, independent of host and WebSocket I/O. (820 lines)
  - `receipts.rs` — Per-connection terminal I/O receipts for the diagnostic audit. (96 lines)
  - `service.rs` — Opt-in local transport: an explicit host directory, bounded forwarding, and (1244 lines)
- `transfer/`
  - `bundle.rs` — A manifest-first tar stream. Archive paths are numbered slots, never native (742 lines)
    - mods: `dependencies`
  - `claude_tools.rs` — Native Claude agent destinations, scoped to calls in one transcript. (95 lines)
  - `cleanup.rs` — Node-owned cleanup for abandoned previews and standalone foreground copies. (107 lines)
  - `code_mode.rs` — Patch literal identity arguments of known agent calls. Executed code and (331 lines)
  - `codex.rs` — Codex physical history cloning into a new private directory. This module (682 lines)
  - `codex_ids.rs` — Typed native identity traversal, shared by rollout and projection adapters. (149 lines)
  - `codex_tools.rs` — Native JSON agent tools only. Code-mode input and arbitrary output text need (236 lines)
  - `coordination.rs` — Short registry locks; work is serialized only for overlapping identities. (102 lines)
  - `dependencies.rs` — External native media and persisted-output files, checked in place. (204 lines)
  - `environment.rs` — Cross-node evidence: compare contents, not Git status or machine-local inodes. (308 lines)
  - `files.rs` — Native file bundles for Claude and Grok. Only structured native identity (1022 lines)
  - `grok_tools.rs` — Grok agent requests across the chat/update streams of one native session. (167 lines)
  - `group.rs` — Undirected connected components over logical and physical history edges. (349 lines)
    - mods: `claude_tools`, `grok_tools`
  - `journal.rs` — Disposable, demand-read summaries of the existing operation journals. (106 lines)
  - `json_bytes.rs` — Apply typed identity changes at their original JSON byte locations. (269 lines)
  - `mod.rs` — Whole-group planning and durable same-node Codex cloning. (57 lines)
    - mods: `bundle`, `cleanup`, `code_mode`, `codex`, `codex_ids`, `codex_tools`, `coordination`, `environment`, `files`, `group`, `json_bytes`, `moving`, `native`, `progress`, `service`, `references`
  - `moving.rs` — Persistent move handoff. A received group stays fenced until the source (588 lines)
  - `names.rs` — Selected Codex rename records. Keep the append-only index's original bytes; (176 lines)
  - `native.rs` — Selected native rows for a same-store Codex clone. Schema is discovered and (1029 lines)
  - `prefix.rs` — File proofs and retained originals for identity-preserving prefix imports. (238 lines)
  - `progress.rs` — Ephemeral work counters. No journal writes, timers, or background scanning. (126 lines)
  - `references.rs` — Compact native relationship summaries. Cache file metadata, never transcripts. (391 lines)
  - `service.rs` — Durable same-node clone transaction. Plans contain server-derived paths only; (1188 lines)
    - mods: `journal`, `names`, `prefix`, `tool_requirements`
  - `tool_requirements.rs` — Persisted definitions identify executor dependencies, not runnable tools. (64 lines)
- `trash.rs` — Session recycle bin. Native files move into recoverable entries; fork (882 lines)
  - mods: `manifest`, `plan`, `tree`
- `trash/`
  - `manifest.rs` — Per-entry manifest: the only record of where trashed files came from. (294 lines)
  - `plan.rs` — Deletion planning from one published list snapshot. (184 lines)
  - `tree.rs` — Whole connected groups share one recoverable entry and one durable receipt. (265 lines)
- `ui_events.rs` — Shared, subscriber-owned UI invalidations. No conversation bodies cross (265 lines)

## process-links

`crates/process-links/src`: 6 files, 1549 lines, 0 undocumented.

- `agent.rs` — Local protocol shared by resource-agent and application adapters. (114 lines)
- `connections.rs` — Durable, boot-scoped connection evidence. No commands or credentials. (326 lines)
- `engine.rs` — Attribution state independent of a session UI or transport. (321 lines)
- `lib.rs` — Shared process identities, SSH lineage and resource aggregation. (268 lines)
  - mods: `agent`, `connections`, `engine`, `linux`, `resource_summary`
- `linux.rs` — Linux adapter: only the current uid's processes, selected environment keys, (265 lines)
- `resource_summary.rs` — Shared per-session accounting. A process is selected once, even when several (255 lines)

## resource-agent

`crates/resource-agent/src`: 8 files, 2505 lines, 3 undocumented.

- `bandwidth.rs` — Low-frequency resctrl MBM monitoring. Only MON groups are created: no (251 lines)
- `events.rs` — (no module doc) (104 lines)
- `gpu.rs` — NVIDIA compute-process residency, not whole-device utilization attribution. (144 lines)
- `io_bpf.rs` — Runtime libbpf loading; the object is compiled at build time and embedded. (334 lines)
- `io_events.rs` — Bounded, read-only application I/O accounting. Values are deltas per completed (219 lines)
  - mods: `bpf`
- `main.rs` — (no module doc) (24 lines)
  - mods: `bandwidth`, `events`, `gpu`, `io_events`, `memory`, `server`
- `memory.rs` — Proportional resident memory; unavailable rollups never fall back to RSS. (53 lines)
- `server.rs` — (no module doc) (1376 lines)

## ptyhost

`crates/ptyhost/src`: 10 files, 3277 lines, 0 undocumented.

- `client.rs` — 宿主会话的客户端：扫描会话目录、发控制请求、建立 attach 流。 (312 lines)
- `dsr.rs` — 从 pty 输出里切出设备状态查询（DSR），其余字节原样放行。 (146 lines)
- `final_screen.rs` — 会话最终画面：宿主退出时把终端模型的最后画面写成一份网格快照（带全部回滚 (98 lines)
- `guard.rs` — Optional identity-checked envelope. An old host rejects this *operation* (285 lines)
  - mods: `binding`
- `main.rs` — ptyhost：独立终端后端，tmux 的替代。 (424 lines)
  - mods: `client`, `dsr`, `final_screen`, `guard`, `output`, `protocol`, `session`, `transport`
- `native_binding.rs` — One operator-declared association for the lifetime of one running host. (153 lines)
- `output.rs` — One bounded FIFO and one socket writer per attachment. Publishers never write (205 lines)
- `protocol.rs` — 宿主与客户端之间的本地协议，与 Python 参考实现逐字节兼容。 (168 lines)
- `session.rs` — 单个托管会话的宿主进程，与 Python 参考实现同协议、同线程结构。 (1364 lines)
- `transport.rs` — 本地传输：POSIX 用 unix socket（0600），Windows 用 127.0.0.1 端口 + 随机 token。 (122 lines)

## ptyhost-client

`crates/ptyhost-client/src`: 8 files, 2128 lines, 4 undocumented.

- `association.rs` — Reviewed immutable metadata only; arbitrary host metadata is never forwarded. (216 lines)
- `bound.rs` — (no module doc) (244 lines)
- `dto.rs` — (no module doc) (272 lines)
- `launch.rs` — Guarded access to one explicit launch instance, including pending sessions (259 lines)
- `lib.rs` — Bounded asynchronous access to explicitly selected local ptyhost records. (696 lines)
  - mods: `association`, `bound`, `dto`, `launch`, `native_binding`, `transport`, `wire`
- `native_binding.rs` — One-time host association declared by a trusted operator, not native CLI proof. (148 lines)
- `transport.rs` — (no module doc) (57 lines)
- `wire.rs` — (no module doc) (236 lines)

## ptyhost-screen

`crates/ptyhost-screen/src`: 3 files, 1053 lines, 0 undocumented.

- `grid.rs` — 服务端网格：把终端模型的画面抽成"行 → span"结构，和上一帧比较后产出 JSON 增量。 (396 lines)
- `lib.rs` — 终端模型与服务端网格，宿主（ptyhost）和 Web 服务（sessiondock）共用： (8 lines)
  - mods: `grid`, `screen`
- `screen.rs` — alacritty_terminal 之上的薄封装，提供与 Python 参考实现同语义的截屏 / 光标 / 回放。 (649 lines)
