# Module map

This file is produced by `tests/module_map.py`. It maps every `.rs` file under
the crate `src` trees below with the first module doc line, line count, `#[test]`
/ `#[tokio::test]` counts, and `mod` declarations. Regenerate:

```sh
python3 tests/module_map.py --write
```

## sessiondock

`crates/sessiondock/src`: 234 files, 126050 lines, 924 tests, 40 undocumented.

- `api/`
  - `audit.rs` — `POST /api/audit/browser`: bounded browser diagnostics intake. (79 lines, 0 tests)
  - `bug_report.rs` — `POST /api/bug-report`, `POST /api/bug-report/capture` and the (798 lines, 2 tests)
  - `conversation.rs` — Thin HTTP transport for server-owned conversation drafts, staging and one-shot SEND. (652 lines, 0 tests)
  - `events.rs` — Node UI invalidations share one cached observer per debug view. (40 lines, 0 tests)
  - `files.rs` — File transport. Every request resolves the selected session; opening a (977 lines, 1 tests)
  - `health.rs` — Local liveness JSON: version, `api_version` 1, and `stage: "read_only"`. (49 lines, 0 tests)
  - `hub.rs` — The hub's HTTP surface (`hub.py` `HubHandler.dispatch`, 518–584), served (892 lines, 0 tests)
  - `lifecycle.rs` — Explicit creation receipts; native identities and reliable send stay separate. (1382 lines, 1 tests)
  - `media.rs` — Opaque media transport. File tokens require current native-scope authorization. (242 lines, 1 tests)
  - `metadata.rs` — SessionDock-owned preferences only. No native session writes or CLI actions. (614 lines, 0 tests)
  - `mod.rs` — Axum transport router nested at `/api`. Handlers stay in sibling modules; (415 lines, 0 tests)
    - mods: `audit`, `bug_report`, `conversation`, `events`, `files`, `health`, `hub`, `lifecycle`, `media`, `metadata`, `node_auth`, `process_links`, `read`, `records`, `runtime`, `search`, `shell_env`, `terminal`, `transfer`, `trash`
  - `node_auth.rs` — Node listener gate (`server.py` `_allowed` / `_hub_protocol` for hub (215 lines, 2 tests)
  - `process_links.rs` — (no module doc) (105 lines, 0 tests)
  - `read.rs` — Read-only session list, messages, grant pages, input history, and SSE watch. (707 lines, 0 tests)
  - `records.rs` — `/api/term/records`: list session recordings; `/api/term/records/attach`: (119 lines, 0 tests)
  - `runtime.rs` — Read-only live status; never upgrades observations into CLI authority. (718 lines, 4 tests)
  - `search.rs` — JSON/NDJSON search transport queues work and applies stream backpressure. Search (222 lines, 1 tests)
  - `shell_env.rs` — `GET /api/shell-env` and `POST /api/shell-env/restart` (`crate::shell_env`). (36 lines, 0 tests)
  - `terminal.rs` — Explicit-directory development transport only; legacy CLI actions stay gated. (1216 lines, 0 tests)
  - `transfer.rs` — Browser-facing clone orchestration. Paths and identity maps never come from (764 lines, 0 tests)
  - `trash.rs` — Session recycle-bin HTTP routes, using delete protection. (627 lines, 0 tests)
- `assets.rs` — Startup snapshot of regular frontend files. Requests never walk the disk. (384 lines, 3 tests)
- `audit.rs` — Best-effort browser diagnostics intake. (413 lines, 0 tests)
  - mods: `intake`, `query`, `writer`, `tests`
- `audit/`
  - `intake.rs` — Request-shape validation and structured-metadata sanitization. (265 lines, 0 tests)
  - `query.rs` — Bug-report audit: server-side structured events into the same JSONL (319 lines, 2 tests)
  - `tests.rs` — (no module doc) (65 lines, 3 tests)
  - `writer.rs` — Dedicated writer thread: daily JSONL files with fourteen-day retention. (328 lines, 0 tests)
- `bin/`
  - `sessiondock-hub.rs` — The multi-machine hub. (201 lines, 0 tests)
  - `sessiondock-transfer.rs` — Offline transfer inspection/staging tool. No native publish or source cleanup. (144 lines, 0 tests)
- `bridge/`
  - `claude.rs` — Claude question cards. (703 lines, 6 tests)
  - `codex.rs` — Codex command approvals. (317 lines, 4 tests)
  - `live.rs` — The live `prompt` of a session view. (299 lines, 2 tests)
  - `menus/`
    - `claude.rs` — Claude Code 2.1.285's live selection surfaces. (653 lines, 0 tests)
    - `codex.rs` — Screen-only Codex menus, audited against the installed rust-v0.159.2 release. (637 lines, 0 tests)
    - `grok.rs` — Native Grok Build screen menus. Provenance and exceptions are in the menu inventory. (566 lines, 0 tests)
    - `mod.rs` — Current-screen projection of native CLI menus. Parsing describes actions; (16 lines, 0 tests)
      - mods: `claude`, `codex`, `grok`, `opencode`
    - `opencode.rs` — OpenCode v2.0.18 screen menus (official tag cd9a14a6b688d4021bee381dfd39d2cef9c0f862). (663 lines, 0 tests)
  - `mod.rs` — CLI question cards and approvals shown live on the conversation page: (14 lines, 0 tests)
    - mods: `claude`, `codex`, `live`, `menus`
- `bug_report/`
  - `mod.rs` — Bug-report bundles and their CLI workers. (1036 lines, 0 tests)
    - mods: `worker`, `tests`
  - `tests.rs` — Semantics for the bundle, the (641 lines, 10 tests)
  - `worker.rs` — The bug-report worker. (470 lines, 0 tests)
- `config.rs` — SessionDock configuration. Paths come from explicit environment variables; (1088 lines, 11 tests)
- `conversation/`
  - `cli_state.rs` — Per-session CLI state that native history cannot tell (docs/cli-state.md): (652 lines, 4 tests)
  - `input.rs` — Positive recognition of the CLI input surface, shared by CHECK and every (604 lines, 10 tests)
  - `mod.rs` — One conversation send path: drafts are server-owned, successful SEND belongs to the CLI. (1173 lines, 4 tests)
    - mods: `cli_state`, `input`, `report_name`, `rewind`, `store`
  - `report_name.rs` — Name a report through the Codex TUI before sending its first model task. (149 lines, 0 tests)
  - `rewind.rs` — A rewind made in Claude's own TUI (double Esc, restore the conversation) (210 lines, 3 tests)
  - `store.rs` — Session-owned drafts and one-shot submission identities. No CLI acknowledgment queue. (1286 lines, 12 tests)
- `delivery/`
  - `driver.rs` — Terminal driver for conversation SEND and the bug-report worker. (1171 lines, 0 tests)
    - mods: `tests`
  - `driver/`
    - `tests.rs` — (no module doc) (462 lines, 20 tests)
  - `mod.rs` — Server-side terminal writes for conversation SEND and the bug-report (6 lines, 0 tests)
    - mods: `driver`, `target`
  - `target.rs` — Managed-instance target resolution and the failure type shared by (163 lines, 0 tests)
- `error.rs` — HTTP JSON error envelope `{error, code}` shared by Axum handlers. (63 lines, 0 tests)
- `files/`
  - `boundary.rs` — (no module doc) (627 lines, 0 tests)
  - `grants.rs` — An authenticated file browser keeps its directory grant after the original (355 lines, 2 tests)
  - `info.rs` — Browser information describes the named leaf. (56 lines, 0 tests)
  - `jobs.rs` — In-memory file-operation jobs, bound to the session scope that created them. (224 lines, 0 tests)
  - `media.rs` — One trusted selected-view reference index shared by an entire media window. (132 lines, 0 tests)
    - mods: `tests`
  - `media/`
    - `tests.rs` — Synthetic explicit-root image handles. These bytes need not be images: format (431 lines, 12 tests)
  - `mod.rs` — Session-reference-scoped file reads and authenticated directory browsing. (350 lines, 0 tests)
    - mods: `boundary`, `grants`, `info`, `jobs`, `media`, `references`, `response`, `write`, `tests`, `write_tests`
  - `references.rs` — (no module doc) (281 lines, 0 tests)
  - `response.rs` — (no module doc) (519 lines, 0 tests)
  - `tests.rs` — (no module doc) (894 lines, 17 tests)
  - `write.rs` — Authenticated operator file mutations through checked parent handles. (2292 lines, 1 tests)
  - `write_tests.rs` — Synthetic-only write-side tests: authorization boundaries, TOCTOU, atomic (956 lines, 14 tests)
- `fingerprint.rs` — 128-bit streaming content fingerprint of native bytes. (191 lines, 4 tests)
- `hub/`
  - `aggregate.rs` — Hub aggregation (`hub.py` `HubHandler.selected/aggregate/search_aggregate/ (861 lines, 0 tests)
    - mods: `tests`
  - `aggregate/`
    - `tests.rs` — (no module doc) (469 lines, 11 tests)
  - `client.rs` — Hub → node HTTP/1.1 client: one connection per request over a plain or (845 lines, 0 tests)
    - mods: `tests`
  - `client/`
    - `tests.rs` — Wire-level cases against in-process tokio listeners: framing, limits, (574 lines, 11 tests)
  - `groups.rs` — Nodes own group catalogs. The Hub caches their union and sends it back; (143 lines, 0 tests)
  - `identity.rs` — Node identity and credential files (`federation.identity`, `server.py` (115 lines, 0 tests)
    - mods: `tests`
  - `identity/`
    - `tests.rs` — (no module doc) (72 lines, 4 tests)
  - `mod.rs` — Hub federation (batches 38–40): node identity, the hub's node registry (31 lines, 0 tests)
    - mods: `aggregate`, `client`, `groups`, `identity`, `namespace`, `nest`, `process_links`, `proxy`, `registry`, `resources`, `transfer`
  - `namespace.rs` — The hub's wire namespace (`federation.py` 31–113): every reference a node (321 lines, 0 tests)
    - mods: `tests`
  - `namespace/`
    - `tests.rs` — (no module doc) (238 lines, 10 tests)
  - `nest.rs` — Validate a display edge against the fleet before routing the write to its child. (60 lines, 0 tests)
  - `process_links.rs` — One fleet coordinator shared by session nesting and external CPU consumers. (74 lines, 0 tests)
  - `proxy.rs` — The hub's pass-through to one node (`hub.py` `HubHandler.resolve` (896 lines, 0 tests)
    - mods: `tests`
  - `proxy/`
    - `tests.rs` — `resolve`, `file_navigation`, the audit grouping and the SSE rewrite (513 lines, 10 tests)
  - `registry.rs` — The hub's node registry (`hub.py` `Registry`): `hub-nodes.json`, node (1463 lines, 0 tests)
    - mods: `tests`
  - `registry/`
    - `tests.rs` — Registry rules against an in-process fake node (tokio listener) so faults (1363 lines, 12 tests)
  - `resources.rs` — Session resource views preserve execution nodes and incomplete observations. (301 lines, 3 tests)
  - `transfer.rs` — Durable cross-node clone orchestration over the authenticated node channel. (906 lines, 0 tests)
- `hub_config.rs` — Configuration of the `sessiondock-hub` binary. Separate from (180 lines, 2 tests)
- `lib.rs` — Loopback development HTTP crate: config, router, and optional isolated services. (578 lines, 0 tests)
  - mods: `api`, `assets`, `audit`, `bridge`, `bug_report`, `config`, `conversation`, `delivery`, `error`, `files`, `fingerprint`, `hub`, `hub_config`, `lifecycle`, `list_sync`, `media`, `metadata`, `native_replay`, `observe`, `polls`, `runtime`, `search`, `security`, `sessions`, `shell_env`, `state`, `terminal`, `transfer`, `trash`, `ui_events`
- `lifecycle/`
  - `autobind.rs` — Process-evidence binding of pending Codex/Grok launches. (325 lines, 0 tests)
  - `clients.rs` — Installed agent CLI versions and manual updates for the machine settings. (574 lines, 2 tests)
  - `launcher.rs` — Configured adapters and one-authority process spawn. No discovery, (1167 lines, 1 tests)
    - mods: `tests`
  - `launcher_tests.rs` — (no module doc) (1193 lines, 20 tests)
  - `mod.rs` — Isolated durable process-creation intent. No launcher or native-session binding. (8 lines, 0 tests)
    - mods: `autobind`, `clients`, `launcher`, `model`, `models`, `service`, `store`
  - `model.rs` — Private creation-intent types: `LaunchSpec`, `Record`, and `BindingSpec`. (617 lines, 0 tests)
  - `models.rs` — Read-only model catalogs for the new-session picker. Each CLI's own list: (436 lines, 0 tests)
  - `service.rs` — Isolated lifecycle coordinator. HTTP cancellation never owns a spawn. (1747 lines, 0 tests)
    - mods: `tests`
  - `service_tests.rs` — (no module doc) (1267 lines, 23 tests)
  - `store/`
    - `disk.rs` — Adapted locally from delivery/store/disk.rs; keep its reviewed durability (247 lines, 0 tests)
    - `json.rs` — Strict JSON grammar without duplicating the lifecycle receipt schema. (99 lines, 0 tests)
    - `mod.rs` — Single-writer durable creation receipts. No process, native history or HTTP I/O. (801 lines, 0 tests)
      - mods: `disk`, `json`, `tests`
    - `tests.rs` — (no module doc) (1085 lines, 21 tests)
- `list_sync.rs` — Opt-in list transport. Cached revisions only save bytes: eviction, restart, (353 lines, 0 tests)
- `main.rs` — Loopback development binary. No option starts the Web service. (339 lines, 0 tests)
- `media.rs` — Private image projection. File capabilities require an explicit selected scope; (801 lines, 0 tests)
  - mods: `descriptors`, `discovery`, `file_media`, `formats`, `native_media`, `tests`
- `media/`
  - `descriptors.rs` — Bounded source capabilities, separate from decoded bytes. No snapshot or (303 lines, 0 tests)
    - mods: `tests`
  - `descriptors/`
    - `tests.rs` — (no module doc) (348 lines, 11 tests)
  - `discovery.rs` — Pure text discovery only. A candidate is not filesystem authority. (377 lines, 6 tests)
  - `file_media.rs` — File capabilities are revalidated against the current selected native view. (246 lines, 0 tests)
    - mods: `tests`
  - `file_media/`
    - `tests.rs` — (no module doc) (178 lines, 4 tests)
  - `formats.rs` — Image decoding belongs to the client. SessionDock does not add container, (9 lines, 0 tests)
  - `native_media.rs` — Private native-string descriptors. Paths are metadata, never open authority. (375 lines, 0 tests)
    - mods: `tests`
  - `native_media/`
    - `tests.rs` — (no module doc) (600 lines, 14 tests)
  - `tests.rs` — (no module doc) (368 lines, 13 tests)
- `metadata/`
  - `disk.rs` — Metadata reads and atomic replacement in the configured directory. (136 lines, 0 tests)
  - `mod.rs` — SessionDock preferences with reads and atomic publication. (248 lines, 0 tests)
    - mods: `disk`, `model`, `tests`
  - `model.rs` — Pure, versioned metadata transformations. No process or native-file access. (846 lines, 0 tests)
    - mods: `transfer`
  - `tests.rs` — (no module doc) (557 lines, 15 tests)
  - `transfer.rs` — Atomic comparison and restoration of display rows during history imports. (63 lines, 0 tests)
- `native_replay.rs` — Checked-source-independent replay of nested JSON string interiors. (222 lines, 0 tests)
  - mods: `tests`
- `native_replay/`
  - `tests.rs` — (no module doc) (325 lines, 12 tests)
- `observe.rs` — One bounded publisher per logical view. Subscribers keep their own cursor; (388 lines, 4 tests)
- `polls.rs` — Response caches of the two liveness polls every open tab repeats every (407 lines, 5 tests)
- `runtime/`
  - `freeze.rs` — Linux diagnostic process-tree pause. The independent PTY host stays live. (146 lines, 0 tests)
  - `mod.rs` — Read-only controlled-host observations against a frozen native inventory. (1432 lines, 0 tests)
    - mods: `freeze`, `process`, `process_links`, `procscan`, `tests`, `native_binding_tests`, `spawn`
  - `native_binding_tests.rs` — (no module doc) (353 lines, 7 tests)
  - `process.rs` — Process identity evidence for host-managed instances. (676 lines, 6 tests)
  - `process_links.rs` — Node adapter for the shared process-links protocol. Remote links are held (602 lines, 0 tests)
  - `procscan.rs` — Read-only `/proc` scan for external CLI processes. (1066 lines, 0 tests)
    - mods: `tests`
  - `procscan/`
    - `tests.rs` — Tests over a synthetic process tree (`FakeProc`): (781 lines, 13 tests)
  - `spawn.rs` — Initialize the single sidebar parent from an observed local CLI launch. (306 lines, 0 tests)
  - `tests.rs` — (no module doc) (1078 lines, 17 tests)
- `search.rs` — Bounded on-demand search over semantic session views, never raw JSONL. (1437 lines, 12 tests)
  - mods: `cache`, `fold`, `prefilter`, `service`
- `search/`
  - `cache.rs` — Persistent search-text cache: one file per main session holding the exact (1242 lines, 8 tests)
  - `fold.rs` — Case folding for the search prefilter, and the whole-word boundary class. (380 lines, 4 tests)
  - `prefilter.rs` — The candidate filter of one query over the folded copies of the cached (291 lines, 4 tests)
  - `service.rs` — Search-text production: where the body of one candidate comes from, in (514 lines, 0 tests)
- `security.rs` — Loopback Host gate and same-origin API policy. Not authentication. (130 lines, 0 tests)
- `sessions/`
  - `debug_runs.rs` — Debug-run registry: paid monkey/test (451 lines, 6 tests)
  - `grok_tests.rs` — (no module doc) (303 lines, 7 tests)
  - `history.rs` — View identity helpers shared by `views` (`native_identity`, `history_link`, (1550 lines, 16 tests)
  - `index/`
    - `agent_stops.rs` — Claude subagent stop points from the owner's main transcript. (227 lines, 0 tests)
      - mods: `tests`
    - `agent_stops/`
      - `tests.rs` — The ClaudeAgentItemTests mechanics at the scan level: notice (427 lines, 8 tests)
    - `codex_turn.rs` — Scalar Codex turn state, independent of the metadata head/tail window. (132 lines, 0 tests)
    - `graph.rs` — Ownership and fork graph over row summaries. (832 lines, 0 tests)
    - `mod.rs` — Lazy session index: directory walk + `stat` + bounded (1567 lines, 0 tests)
      - mods: `agent_stops`, `codex_turn`, `graph`, `names`, `summary`, `titles`, `tests`
    - `names.rs` — Codex `session_index.jsonl` names applied to summary rows. (203 lines, 0 tests)
      - mods: `tests`
    - `names/`
      - `tests.rs` — (no module doc) (494 lines, 9 tests)
    - `summary/`
      - `claude.rs` — Claude row summary: `ClaudeAdapter._meta` (main transcripts, including (460 lines, 0 tests)
      - `codex.rs` — Codex row summary: `CodexAdapter._raw_meta` (120 head pieces) plus the (301 lines, 0 tests)
      - `grok.rs` — Grok row summary: `GrokAdapter.session_meta` from `summary.json` plus the (164 lines, 0 tests)
      - `mod.rs` — Bounded per-file row summaries. (869 lines, 0 tests)
        - mods: `claude`, `codex`, `grok`, `opencode`, `tests`
      - `opencode.rs` — OpenCode row summary from the SessionDock mirror (`sessions::opencode`): (104 lines, 0 tests)
      - `tests.rs` — (no module doc) (1276 lines, 25 tests)
    - `tests.rs` — (no module doc) (2983 lines, 22 tests)
    - `titles.rs` — Point reads of current display titles. Only initial discovery builds a full (163 lines, 0 tests)
  - `media_projection.rs` — Window selection precedes file opens; authority uses the complete branch. (216 lines, 0 tests)
  - `media_tests.rs` — Synthetic native inputs only; projection and cursor boundaries for media. (257 lines, 6 tests)
  - `mod.rs` — Session read model: the lazy index (`index/`) is the only inventory, and (1521 lines, 0 tests)
    - mods: `debug_runs`, `history`, `index`, `native_input`, `native_media`, `opencode`, `pages`, `views`, `providers`, `records`, `scope`, `grok_tests`, `media_projection`, `media_tests`, `native_scope_tests`, `native_catalog_tests`, `tests`
  - `native_catalog_tests.rs` — (no module doc) (240 lines, 5 tests)
  - `native_input.rs` — Checked, chunked native input and a disposable raw-prefix index. (339 lines, 0 tests)
    - mods: `tests`
  - `native_input/`
    - `tests.rs` — (no module doc) (650 lines, 22 tests)
  - `native_media.rs` — Native span authority is the current full selected branch, not file_roots (89 lines, 0 tests)
  - `native_scope_tests.rs` — (no module doc) (379 lines, 8 tests)
  - `opencode.rs` — OpenCode session mirror. (591 lines, 0 tests)
  - `pages.rs` — Finite history pages. Grants hold checkpoints, never retained native views. (600 lines, 0 tests)
    - mods: `tests`
  - `pages/`
    - `tests.rs` — Synthetic pagination grants and semantic checkpoints; no native I/O or CLI. (1036 lines, 22 tests)
  - `providers.rs` — Pure native-record projection. File discovery, inheritance cutoffs and (1373 lines, 0 tests)
    - mods: `claude`, `envelopes`, `grok`, `image_content`, `media_tests`, `opencode`, `tests`, `tools`
  - `providers/`
    - `claude.rs` — Claude's append-only transcript is a tree, not a flat event log. (1007 lines, 0 tests)
    - `envelopes.rs` — Source-specific display envelopes. Never interpret arbitrary HTML as protocol (275 lines, 0 tests)
    - `grok.rs` — Summary-derived Grok metadata. The transcript never overrides these fields, (226 lines, 3 tests)
    - `image_content.rs` — Typed image extraction before native content becomes public text or JSON. (195 lines, 0 tests)
    - `media_tests.rs` — Synthetic embedded image records only; no paths, network or CLI are opened. (591 lines, 16 tests)
    - `opencode.rs` — OpenCode 2 message rows from the SessionDock mirror (`sessions::opencode`). (178 lines, 0 tests)
    - `tests.rs` — (no module doc) (1915 lines, 45 tests)
    - `tools.rs` — Pure presentation of known tool arguments. A shell command is text here: (1056 lines, 0 tests)
      - mods: `tests`
    - `tools/`
      - `tests.rs` — (no module doc) (447 lines, 16 tests)
  - `records.rs` — Bounded, disposable JSON AST reuse. Never an incremental timeline parser. (404 lines, 0 tests)
    - mods: `native_images`, `native_records`, `scanner`, `string_reader`, `tool_envelopes`, `scanner_contract_tests`, `tests`
  - `records/`
    - `native_images.rs` — Private structural image authority. JSON paths only locate already-reviewed (550 lines, 0 tests)
      - mods: `tests`
    - `native_images/`
      - `tests.rs` — (no module doc) (335 lines, 11 tests)
    - `native_records.rs` — Pull-based complete-record scanning. Large strings stay private spans; the (201 lines, 0 tests)
      - mods: `replay_source`, `tests`
    - `native_records/`
      - `replay_source.rs` — Replays only reviewed tool strings from the stamped current native record. (242 lines, 1 tests)
      - `tests.rs` — (no module doc) (531 lines, 13 tests)
    - `scanner.rs` — Private streaming JSON structure scanner. This is not a media classifier or (819 lines, 0 tests)
      - mods: `tests`
    - `scanner/`
      - `tests.rs` — (no module doc) (709 lines, 17 tests)
    - `scanner_contract_tests.rs` — Independent differential contract against serde_json, using only synthetic (427 lines, 11 tests)
    - `string_reader.rs` — A bounded decoder for the physical INSIDE of one JSON string (no quotes). (258 lines, 0 tests)
      - mods: `tests`
    - `string_reader/`
      - `tests.rs` — (no module doc) (327 lines, 12 tests)
    - `tests.rs` — (no module doc) (544 lines, 19 tests)
    - `tool_envelopes.rs` — Streaming discovery of a known Codex tool envelope inside ONE (247 lines, 0 tests)
      - mods: `tests`
    - `tool_envelopes/`
      - `tests.rs` — (no module doc) (639 lines, 17 tests)
  - `scope.rs` — Identity provenance captured once from already parsed, committed records. (108 lines, 0 tests)
  - `tests.rs` — (no module doc) (1692 lines, 42 tests)
  - `views/`
    - `body.rs` — Byte rendering of message batches: the `/api/messages` document and the (281 lines, 0 tests)
    - `body_tests.rs` — The byte renderer (`messages_body`, `history_page_body`) against the (785 lines, 10 tests)
    - `encoded.rs` — Serialized message bytes of one projected file, kept next to its events (427 lines, 3 tests)
    - `mod.rs` — Per-session views on demand: one opened session is (2134 lines, 0 tests)
      - mods: `body`, `encoded`, `body_tests`, `tests`
    - `tests.rs` — `Views` against private temporary roots: on-demand builds, incremental (954 lines, 17 tests)
- `shell_env.rs` — Login-shell environment drift. (224 lines, 0 tests)
- `state.rs` — (no module doc) (269 lines, 3 tests)
- `terminal/`
  - `device.rs` — Coarse device label from a browser `User-Agent` for ownership prompts. (101 lines, 2 tests)
  - `input.rs` — Raw HTTP terminal input: named-key mapping and host protocol bounds. (279 lines, 4 tests)
  - `mod.rs` — Pure browser ownership plus explicitly configured local PTY transport. (32 lines, 0 tests)
    - mods: `device`, `input`, `ownership`, `receipts`, `records`, `service`
  - `ownership.rs` — Exclusive browser terminal leases, independent of host and WebSocket I/O. (1677 lines, 24 tests)
    - mods: `launch_tests`
  - `ownership_launch_tests.rs` — (no module doc) (306 lines, 5 tests)
  - `receipts.rs` — Per-connection terminal I/O receipts for the diagnostic audit. (134 lines, 1 tests)
  - `records.rs` — Read-only access to ptyhost session recordings (`<ptyhost dir>/records/<id>/`). (1101 lines, 2 tests)
  - `service.rs` — Opt-in local transport: an explicit host directory, bounded forwarding, and (1378 lines, 3 tests)
    - mods: `bound_tests`, `launch_tests`, `native_binding_tests`
  - `service_bound_tests.rs` — (no module doc) (372 lines, 6 tests)
  - `service_launch_tests.rs` — Synthetic local TCP peer only; no shell, CLI, or native history discovery. (614 lines, 9 tests)
  - `service_native_binding_tests.rs` — No synthetic NativeBinding constructor: every target below comes from a (528 lines, 5 tests)
- `transfer/`
  - `bundle.rs` — A manifest-first tar stream. Archive paths are numbered slots, never native (692 lines, 0 tests)
    - mods: `dependencies`
  - `claude_tools.rs` — Native Claude agent destinations, scoped to calls in one transcript. (95 lines, 0 tests)
  - `cleanup.rs` — Node-owned cleanup for abandoned previews and standalone foreground copies. (107 lines, 0 tests)
  - `code_mode.rs` — Conservative lexical adapter for native code-mode agent calls. Only literal (347 lines, 0 tests)
  - `codex.rs` — Codex physical history cloning into a new private directory. This module (665 lines, 0 tests)
  - `codex_ids.rs` — Typed native identity traversal, shared by rollout and projection adapters. (149 lines, 0 tests)
  - `codex_tools.rs` — Native JSON agent tools only. Code-mode input and arbitrary output text need (266 lines, 0 tests)
  - `coordination.rs` — Short registry locks; work is serialized only for overlapping identities. (102 lines, 0 tests)
  - `dependencies.rs` — External native media and persisted-output files, checked in place. (186 lines, 0 tests)
  - `environment.rs` — Cross-node evidence: compare contents, not Git status or machine-local inodes. (308 lines, 0 tests)
  - `files.rs` — Native file bundles for Claude and Grok. Only structured native identity (1006 lines, 0 tests)
  - `grok_tools.rs` — Grok agent requests across the chat/update streams of one native session. (167 lines, 0 tests)
  - `group.rs` — Undirected connected components over logical and physical history edges. (333 lines, 0 tests)
    - mods: `claude_tools`, `grok_tools`
  - `json_bytes.rs` — Apply typed identity changes at their original JSON byte locations. (269 lines, 0 tests)
  - `mod.rs` — Whole-group planning and durable same-node Codex cloning. (56 lines, 0 tests)
    - mods: `bundle`, `cleanup`, `code_mode`, `codex`, `codex_ids`, `codex_tools`, `coordination`, `environment`, `files`, `group`, `json_bytes`, `moving`, `native`, `service`, `references`
  - `moving.rs` — Persistent move handoff. A received group stays fenced until the source (511 lines, 0 tests)
  - `native.rs` — Selected native rows for a same-store Codex clone. Schema is discovered and (1012 lines, 0 tests)
  - `prefix.rs` — File proofs and retained originals for identity-preserving prefix imports. (238 lines, 0 tests)
  - `references.rs` — Compact native relationship summaries. Cache file metadata, never transcripts. (229 lines, 0 tests)
  - `service.rs` — Durable same-node clone transaction. Plans contain server-derived paths only; (952 lines, 0 tests)
    - mods: `prefix`, `tool_requirements`
  - `tool_requirements.rs` — Persisted definitions identify executor dependencies, not runnable tools. (64 lines, 0 tests)
- `trash.rs` — Session recycle bin. Native files move into recoverable entries; fork (887 lines, 0 tests)
  - mods: `manifest`, `plan`, `tests`
- `trash/`
  - `manifest.rs` — Per-entry manifest: the only record of where trashed files came from. (292 lines, 0 tests)
  - `plan.rs` — Deletion planning from one published list snapshot. (175 lines, 0 tests)
  - `tests.rs` — Pure unit coverage: protection topology, manifest round trip, planning and (572 lines, 10 tests)
- `ui_events.rs` — Shared, subscriber-owned UI invalidations. No conversation bodies cross (224 lines, 0 tests)

## process-links

`crates/process-links/src`: 6 files, 1177 lines, 6 tests, 1 undocumented.

- `agent.rs` — Local protocol shared by resource-agent and application adapters. (98 lines, 0 tests)
- `engine.rs` — Attribution state independent of a session UI or transport. (235 lines, 0 tests)
- `lib.rs` — Shared process identities, SSH lineage and resource aggregation. (202 lines, 0 tests)
  - mods: `agent`, `engine`, `linux`, `resource_summary`, `tests`
- `linux.rs` — Linux adapter: only the current uid's processes, selected environment keys, (237 lines, 0 tests)
- `resource_summary.rs` — Shared per-session accounting. A process is selected once, even when several (201 lines, 3 tests)
- `tests.rs` — (no module doc) (204 lines, 3 tests)

## resource-agent

`crates/resource-agent/src`: 7 files, 2122 lines, 7 tests, 3 undocumented.

- `events.rs` — (no module doc) (73 lines, 0 tests)
- `gpu.rs` — NVIDIA compute-process residency, not whole-device utilization attribution. (176 lines, 2 tests)
- `io_bpf.rs` — Runtime libbpf loading; the object is compiled at build time and embedded. (288 lines, 0 tests)
- `io_events.rs` — Bounded, read-only application I/O accounting. Values are deltas per completed (186 lines, 0 tests)
  - mods: `bpf`
- `main.rs` — (no module doc) (22 lines, 0 tests)
  - mods: `events`, `gpu`, `io_events`, `memory`, `server`
- `memory.rs` — Proportional resident memory; unavailable rollups never fall back to RSS. (106 lines, 2 tests)
- `server.rs` — (no module doc) (1271 lines, 3 tests)

## ptyhost

`crates/ptyhost/src`: 13 files, 4635 lines, 48 tests, 3 undocumented.

- `client.rs` — 宿主会话的客户端：扫描会话目录、发控制请求、建立 attach 流。 (313 lines, 0 tests)
- `dsr.rs` — 从 pty 输出里切出设备状态查询（DSR），其余字节原样放行。 (241 lines, 8 tests)
- `guard.rs` — Optional identity-checked envelope. An old host rejects this *operation* (399 lines, 6 tests)
  - mods: `binding`, `launch_tests`
- `launch_guard_tests.rs` — (no module doc) (191 lines, 5 tests)
- `main.rs` — ptyhost：独立终端后端，tmux 的替代。 (435 lines, 0 tests)
  - mods: `client`, `dsr`, `guard`, `output`, `protocol`, `record`, `session`, `transport`
- `native_binding.rs` — One operator-declared association for the lifetime of one running host. (157 lines, 0 tests)
  - mods: `tests`
- `native_binding_tests.rs` — (no module doc) (247 lines, 4 tests)
- `output.rs` — One bounded FIFO and one socket writer per attachment. Publishers never write (338 lines, 6 tests)
- `protocol.rs` — 宿主与客户端之间的本地协议，与 Python 参考实现逐字节兼容。 (224 lines, 4 tests)
- `record.rs` — 会话录制接线：把 pty 输出、尺寸变化和退出按模型消费顺序写进 (242 lines, 1 tests)
- `session.rs` — 单个托管会话的宿主进程，与 Python 参考实现同协议、同线程结构。 (1617 lines, 11 tests)
  - mods: `stop_tests`
- `stop_tests.rs` — (no module doc) (109 lines, 3 tests)
- `transport.rs` — 本地传输：POSIX 用 unix socket（0600），Windows 用 127.0.0.1 端口 + 随机 token。 (122 lines, 0 tests)

## ptyhost-client

`crates/ptyhost-client/src`: 8 files, 2177 lines, 3 tests, 4 undocumented.

- `association.rs` — Reviewed immutable metadata only; arbitrary host metadata is never forwarded. (213 lines, 0 tests)
- `bound.rs` — (no module doc) (244 lines, 0 tests)
- `dto.rs` — (no module doc) (272 lines, 0 tests)
- `launch.rs` — Guarded access to one explicit launch instance, including pending sessions (259 lines, 0 tests)
- `lib.rs` — Bounded asynchronous access to explicitly selected local ptyhost records. (696 lines, 0 tests)
  - mods: `association`, `bound`, `dto`, `launch`, `native_binding`, `transport`, `wire`
- `native_binding.rs` — One-time host association declared by a trusted operator, not native CLI proof. (148 lines, 0 tests)
- `transport.rs` — (no module doc) (57 lines, 0 tests)
- `wire.rs` — (no module doc) (288 lines, 3 tests)

## ptyhost-record

`crates/ptyhost-record/src`: 5 files, 3537 lines, 57 tests, 0 undocumented.

- `format.rs` — 录制分段的字节级编解码：段头与帧。 (599 lines, 12 tests)
- `lib.rs` — ptyhost 会话录制（record）：每个会话一个目录，若干只追加的分段文件。 (153 lines, 0 tests)
  - mods: `format`, `reader`, `sanitize`, `store`
- `reader.rs` — 录制目录的只读读取器。 (1337 lines, 15 tests)
- `sanitize.rs` — 从录制的 pty 输出里剥掉终端 QUERY 序列，其余字节原样放行。 (657 lines, 17 tests)
- `store.rs` — 录制目录的只追加写入器。 (791 lines, 13 tests)

## ptyhost-screen

`crates/ptyhost-screen/src`: 3 files, 1243 lines, 19 tests, 0 undocumented.

- `grid.rs` — 服务端网格：把终端模型的画面抽成"行 → span"结构，和上一帧比较后产出 JSON 增量。 (482 lines, 7 tests)
- `lib.rs` — 终端模型与服务端网格，宿主（ptyhost）和 Web 服务（sessiondock）共用： (8 lines, 0 tests)
  - mods: `grid`, `screen`
- `screen.rs` — alacritty_terminal 之上的薄封装，提供与 Python 参考实现同语义的截屏 / 光标 / 回放。 (753 lines, 12 tests)
