# Validation suites

Tables of the headless Chromium browser suites run by
`python3 tests/run_validation.py`, plus the explicit real-CLI/SSH-peer browser
suites. A documented direct command does not imply inclusion in the default
sweep. Narrative rules stay in `AGENTS.md`.

```sh
python3 tests/run_validation.py            # --jobs 8, --browser-jobs 3 by default
python3 tests/run_validation.py --jobs 1   # fully serial (browser jobs forced to 1)
python3 tests/run_validation.py --list
python3 tests/run_validation.py --tags rust
python3 tests/run_validation.py --only legacy_browser
```

`--tags` defaults to `rust,python`. Validation is by headless Chromium browser
suites. `--only NAME[,NAME…]` and `--skip`
filter by suite name. `--keep-going` continues after a failure. Per-suite logs
go under `target/validation/<stamp>/`. Rust runs first as parallel lanes that
share no cargo build directory (`cargo_clippy`; `cargo_build`;
`cargo_check_windows`; `cargo_fmt`), then Python (`tests/*.py` with
`if __name__ == "__main__"`, sorted) — every suite already owns its loopback
port and temp directories. The few non-browser entries (the Rust lanes and
`brand_names_check`) run through a pool of `--jobs` workers; suites that drive Chromium (any that import `playwright`) run through a
separate `--browser-jobs` pool (default 3) at the same time, since more than a
few concurrent Chromiums make click/visibility waits flake without the suite
being wrong. A suite whose first 40 lines carry `# run_validation: serial` runs
alone after both pools (timing assertions). Summary and JSON keep the plan
order; `--jobs 1` runs everything serially in that order.

[quotas_browser.py](../tests/quotas_browser.py) covers the account quota page,
Codex/Agy collectors, Claude statusLine snapshots, Hub proxying and failure recovery.

## When to run which tier

Validation scope and delivery requirements are defined in
[AGENTS.md](../AGENTS.md#validation). Select suites by changed behavior and
shared dependencies; use a compatible binary for frontend-only work. Full
sweeps are for release acceptance or justified cross-cutting changes.

All UI changes also follow the global `references/web-ui.md` rule: interact
with the final page, personally inspect the whole page, and cover affected
sources and relevant combined states. A screenshot file or a passing DOM
assertion alone does not establish visual acceptance. See the
[developer workflow](runbook-dev.md#修改前验收与交付) and [SDK UI contract](sdk-ui.md).
Record actual final-version checks and limitations; do not change an expected
result before confirming the current contract and intended behavior.

The frontend `web/` is served as committed and needs no build.
`--web-dir` (or `SESSIONDOCK_TEST_WEB_DIR`) points the browser suites at another
frontend directory.

Shared browser helpers live in [`browser_runtime.py`](../tests/browser_runtime.py);
asynchronous polling awaits the resolved condition within the original deadline.
The original UI, identity, checkpoint and race assertions remain in place.

Tests that launch detached ptyhost sessions must stop their private hosts before
removing the temporary directory, including after assertion failures. Stopping
the web server intentionally preserves those sessions. Use the guarded
`private_hosts(root)` context in [private_hosts.py](../tests/private_hosts.py),
inside `TemporaryDirectory` and outside the browser and all server instances.
It survives service restarts, cleans up on success, assertion failure and SIGTERM,
and verifies socket removal and, on Linux, host/CLI process exit. The validation
runner sends SIGTERM on timeout and allows 20 seconds for teardown before a hard
kill; the suite still reports TIMEOUT. SIGKILL cannot run teardown.
[private_hosts_browser.py](../tests/private_hosts_browser.py) exercises
these exits with a browser-created fake CLI. The same module exports
`cleanup_hosts(root)` for existing explicit teardown paths. Private CLI processes must not leak after a suite exits.

- **Small change / one bug fix** — run the affected headless browser suite
  (`--only <name>`); a full sweep is not required for every edit. Once the change
  is complete and validated, commit and push only the task changes. Deploy
  committed runtime changes to affected targets under `AGENTS.md`; docs, rules,
  tests and deployment tooling alone do not require a production release.
- **Paid CLI checks** — the `*_real` suites spawn real Claude/Codex/Grok and are
  excluded by default; run them deliberately with `--include-real`, per batch,
  not unattended. `--real-only` runs just those suites: the release acceptance
  step ([release](release.md#release-acceptance)).

## Prerequisites

- `cargo` on `PATH`; the runner prepends `~/.cargo/bin` when needed
- For the MSVC check on Linux/macOS, install `cargo-xwin` with
  `cargo install cargo-xwin --locked`, plus LLVM (`clang-cl`, `llvm-lib`,
  `lld-link`) on `PATH`. If the distro installs only `clang`, a `clang-cl`
  symlink to it selects its MSVC driver. `cargo xwin check` downloads and
  caches the Microsoft CRT/SDK and compiles bundled C dependencies; it does
  not skip SQLite compilation. See the [cargo-xwin instructions](https://github.com/rust-cross/cargo-xwin).
- Release `target/release/sessiondock` for browser suites that take `--binary`
  (the runner's `cargo_build` suite produces it first)
- Built `target/debug/ptyhost` for terminal, lifecycle, and host suites
- Python with `playwright` installed; child suites use the runner's interpreter
- Playwright Chromium installed under `~/.cache/ms-playwright/chromium-*` via
  `PLAYWRIGHT_CHROMIUM_EXECUTABLE` (unset: highest `chromium-*` under that cache)

Isolation: synthetic fixtures copied into temporary directories;
loopback listeners only; suites that drive a real CLI as the system under test
always use the cheapest model at low effort (`claude-haiku-4-5-20251001`, `gpt-5.6-luna`, `grok-4.6`;
see AGENTS.md); a missing prerequisite must be reported and never counts as a pass; no production data, native
homes, or live sessions. Test isolation does not isolate source or deployment.
Linux only. The MSVC `cargo check` is a cross-compile, not Windows runtime coverage.

Windows 实机通过 OpenSSH 构建时，不得调用 `%USERPROFILE%\.cargo\bin` 中的 rustup
shim；真实工具链选择、错误 448 的处理及构建后滚动更新见
[Windows 节点原生构建与滚动部署](deploy-windows.md)。

## CLI diagnostic acceptance

`python3 tests/trash_interactions_browser.py --binary target/release/sessiondock`
uses real pointer and keyboard confirmation controls for delete, restore,
conflict retry, purge and cancellation. Private native files and recycle-bin
manifests independently verify the outcomes, including wide/narrow dismissal.

`python3 tests/terminal_log_volume_browser.py --binary target/release/sessiondock`
types through a real isolated console, verifies private shell receipts and
pagehide summary persistence, and records on/off latency and event/byte totals.

`python3 tests/cli_diagnostics_browser.py --binary target/release/sessiondock`
submits a private fake OpenCode session through the new-session dialog, verifies
failed CLI output is absent from runtime diagnostics, then retries successfully.

## Regex worker acceptance

`python3 tests/regex_worker_browser.py --binary target/release/sessiondock`
drives regex input, injected Worker failure, visible error, Enter retry and
Escape at wide and narrow widths, checking persisted metadata-only diagnostics.

## Hub diagnostic acceptance

`python3 tests/hub_diagnostics_browser.py --binary target/release/sessiondock`
uses two private nodes and a Hub, real machine settings controls, persisted
registry assertions and legacy/current log retention checks.

## Diagnostic retention acceptance

`python3 tests/diagnostic_retention_browser.py --binary target/release/sessiondock`
drives actual page navigation and session selection against a private server.
It checks mixed-age/numbered segments, 24-hour boundaries, idle cleanup, restart,
cleanup and sink failure, protected evidence, and records enabled/disabled audit
navigation timings. Artifacts are under `target/diagnostic-retention/`. These
timings are observations, not a claim of equivalent performance.

## Suites

The table lists the suites `--list` reports plus the opt-in `*_real` browser suites; rows for the removed non-browser suites were dropped on 2026-10-06.

| Suite | Command | Covers | Needs |
| --- | --- | --- | --- |
| cargo_fmt | `cargo fmt -p sessiondock -p ptyhost-client --check` | rustfmt on those two packages | cargo |
| cargo_clippy | `cargo clippy -p sessiondock -p ptyhost-client --all-targets --locked -- -D warnings` | clippy, warnings denied | cargo |
| cargo_check_windows | `cargo xwin check --workspace --all-targets --target x86_64-pc-windows-msvc --locked` | Linux/macOS cross-compile to MSVC with bundled C dependencies; native Windows uses `cargo check`; not a Windows run | cargo, cargo-xwin, LLVM, cached/downloadable CRT/SDK |
| cargo_build | `cargo build --release -p sessiondock --locked` | Release server used by `--binary` Python suites | cargo |
| agy_browser | `python3 tests/agy_browser.py --binary target/debug/sessiondock` | Legacy Agy pending launch, effort argv, terminal typing, reconnect, discard, unavailable CLI, 390 px and themes; fake CLI only, not native resume evidence. | binary, ptyhost, Chromium |
| agy_clients_browser | `python3 tests/agy_clients_browser.py --binary target/debug/sessiondock` | Private fake CLI and manifest: local/Hub model TSV, default and explicit effort argv, version/update success and failure, closed stdin, missing CLI and offline-node gates. | binary, hub, ptyhost, Chromium |
| agy_message_presentation_browser | `python3 tests/agy_message_presentation_browser.py --binary target/debug/sessiondock` | Agy summaries/read ranges, compact model notices, raw parameters/headers, height-clipped text and file diffs, compaction details, command exit headers versus ordinary output, error-only records, live append and 1280/675/390 px light/dark screenshots; synthetic records only. | binary, Chromium |
| agy_delete_browser | `python3 tests/agy_delete_browser.py` | Node and Hub native-delete adapter with synthetic CLI: cancel, single/batch, duplicate, same-title identity, wrong selected SID, native failure and worker cleanup. | binary, hub, ptyhost, Chromium |
| agy_history_browser | `python3 tests/agy_history_browser.py --binary target/debug/sessiondock` | Native-schema synthetic catalog/transcript seeds: Chromium list, search, pagination, append, old-row rewrite, rewind, atomic DB replacement, restart, missing/restored transcript with retained history notice, catalog-row removal, image/raw-tool/error rendering, and delete-confirm cancellation/missing-launcher and mixed-group transfer refusal; native bytes/mtime preserved. Append continuation also covers empty-input turns, skipped-line/kind counts, tool order, same-inode rewrite plus append, and media URI replacement. `--ast-cache-mb 0` or `--view-cache-mb 0` exercises full-parse fallback. Schema fixtures do not establish real CLI media/tool output. | binary, Chromium |
| agent_menu_browser | `python3 tests/agent_menu_browser.py --binary target/release/sessiondock` | Subagent title-bar menu (Python `agent_menu_e2e` port): end-time order and no running dot while the owner is not live, running-first with dot / open span from the backend's `agent_items[].active` (sidecar open turn), finish/resume as transcript edits reaching the next open; row geometry under a sticky turn toolbar at desktop + 390 px | binary, Chromium |
| audit_browser | `python3 tests/audit_browser.py --binary target/release/sessiondock` | Bounded browser-audit intake: receipts when configured, 501 and no posts without | binary, Chromium |
| autobind_browser | `python3 tests/autobind_browser.py` | Process-evidence autobind of a pending Codex pane: an older rollout the pane holds open (Python `before` set) never binds; the pane's own fresh rollout binds and the page follows it | debug server, ptyhost, proc scan, Chromium |
| brand_names_check | `python3 tests/brand_names_check.py` | Scans every tracked non-Markdown text file—source, tests, configuration, static assets and frozen reference—with no compatibility whitelist; also verifies the SessionDock manifest, install metadata, that no service worker is registered (and the retired shell caches are cleared), page/runtime titles and `<namespace>files-*` preference keys. | git checkout |
| bug_report_node_browser | `python3 tests/bug_report_node_browser.py` | Hub report dialog machine selection, named source buttons, two-up attachments on a 390px phone, disabled CLI choices and cross-machine capture/submit bodies against fake nodes; no CLI or real histories | hub binary, Chromium |
| bug_report_identity_browser | `python3 tests/bug_report_identity_browser.py` | Identical report titles, shared-daemon writer, delayed native history, current and old receipts with lost status cards, restart and terminal/history clicks; no duplicate SEND | binary, ptyhost, Chromium |
| optional_report_browser | `python3 tests/optional_report_browser.py` | Disabled report controls and capture-only submissions, wide/light and mobile/dark; no CLI launch. | binary, Chromium |
| bug_report_codex_browser_real | `python3 tests/bug_report_codex_browser_real.py` | Operator-only real Codex report dialog, native rename and first task confirmation; isolated home, Luna low, unchanged everyday defaults. | Codex CLI, ptyhost, Chromium |
| sessions_visibility_browser | `python3 tests/sessions_visibility_browser.py --binary target/release/sessiondock` | Chromium node/Hub list, open, search and SSE with obsolete registry/URL parameters; all data temporary. | binary, hub binary, Playwright |
| files_browser | `python3 tests/files_browser.py --binary target/release/sessiondock` | Conversation references and the title bar directory open FileDock with the exact node/path (the directory is plain text without FileDock); missing-reference errors and direct directory entry, desktop/mobile. | binary, Chromium |
| grok_metadata_browser | `python3 tests/grok_metadata_browser.py --binary target/release/sessiondock` | Grok summary/UID/title/cwd/model, cursor, corruption/recovery over HTTP, desktop/mobile Chromium SSE | binary, Chromium |
| grok_send_echo_browser | `python3 tests/grok_send_echo_browser.py` | Grok composer SEND on a 390 px page: the queued bubble survives a reload (server CLI state), a timestamp-less `chat_history.jsonl` user line retires it, and an older identical line does not (BUG-20260927-211850-1fa082). | Chromium, ptyhost |
| toolbar_options_browser | `python3 tests/toolbar_options_browser.py --binary target/release/sessiondock` | Desktop/mobile toolbar defaults, checkbox pinning independent of selection/filtering, ellipsis submenus, reload persistence, keyboard navigation and viewport bounds | binary, Chromium |
| session_menu_refresh_browser | `python3 tests/session_menu_refresh_browser.py` | Hub task HTTP refresh preserves an open session menu and focus; real menu navigation, Escape, outside click and resize at 675/390/1280 px in light/dark themes. | hub binary, fake node, Chromium |
| header_fold_browser | `python3 tests/header_fold_browser.py --binary target/release/sessiondock` | Header and title-bar fold order over 123 widths (1698 down to 320) plus divider drag (Python `fold_sweep_e2e` port, local mode): chrome labels → hostname → machine-chip abbreviations (no counts) → right-side buttons; suffix/prefix priorities (no branch item), no squeeze, one header height across tiers, monotonic within a tier | binary, Chromium |
| history_queue_browser | `python3 tests/history_queue_browser.py --binary target/release/sessiondock` | Chromium history selection with persisted queues: unrelated opens avoid terminal probes; native/alias queues recover, pending sends survive, unknown echo candidates still resolve identity; native bytes preserved. | binary, Chromium |
| history_browser | `python3 tests/history_browser.py --binary target/release/sessiondock` | Advanced native history in legacy UI: branches, agents, parent-chain, SSE reset | binary, Chromium |
| concurrent_views_browser | `python3 tests/concurrent_views_browser.py --binary target/release/sessiondock` | Concurrent real page opens with small shared cache budgets: different owners, duplicate owner, inherited prefixes, gap clicks and stable live cursors, append, rewrite, deletion and recreation. Route barriers align browser requests; they do not prove backend timing or a measured speedup. | binary, Chromium |
| summary_projection_browser | `python3 tests/summary_projection_browser.py --binary target/release/sessiondock` | List and selected-history/search flows with large unused JSON subtrees, discarded-field syntax/UTF-8/number validation, duplicate keys, deep nesting, agent ownership, equal-size rewrite and partial-line commits. | binary, Chromium |
| history_pages_browser | `python3 tests/history_pages_browser.py --binary target/release/sessiondock` | Bounded pages, SSE/render races, cancelled preparation preserves selected view/DOM, explicit 404/409/410 recovery | binary, Chromium |
| history_loading_browser | `python3 tests/history_loading_browser.py --screenshots /tmp/sessiondock-history-loading` | Delayed opening, inline failure/retry, cached reopen and paged rendering without global progress; explicit full-history progress retained; desktop/report/phone widths, light/dark | binary, Chromium |
| history_pages_resume_browser | `python3 tests/history_pages_resume_browser.py --binary target/release/sessiondock` | Lost grants across real server processes; partial pages, live appends, exact subagent scope and changed-checkpoint rejection | binary, Chromium |
| conversation_performance_browser | `python3 tests/conversation_performance_browser.py` | Header fold/expand preserves process nodes; lazy 180-tool group, live append and yielding regex search. | binary, Chromium |
| render_assets_browser | `python3 tests/render_assets_browser.py` | Diagrams, math and terminal assets load on demand through the page. | binary, Chromium |
| hub_cache_browser | `python3 tests/hub_cache_browser.py` | Repeated real page reloads over changing padded node lists: bounded Hub RSS, latest snapshot, unchanged refresh without disk rewrite, and offline cache after restart. | hub binary, Chromium, Linux procfs |
| hub_bulk_browser | `python3 tests/hub_bulk_browser.py` | Real Hub and node with a slow fake OpenCode remove: pick-bar 附属到… nests several rows under one clicked parent; a bulk delete that takes longer than 5 s removes every row without errors; desktop + 390px. | binary, Chromium, ptyhost |
| hub_nest_browser | `python3 tests/hub_nest_browser.py` | Two real isolated nodes and Hub: cross-machine manual attach via context menu, same native SID isolation, cycle/auth checks, persistence across node/Hub restart, detach/restore and local reattach. | binary, Chromium |
| hub_scale_browser | `python3 tests/hub_scale_browser.py` | Three isolated nodes and real Hub: 4,800 synthetic native sessions, 2,400 parent counts, cross-node summation and same-SID node/provider isolation; Chromium folds, reloads 40 times, checks RSS growth after cache warmup, filters, expands and opens a remote child. Reports list timings and Hub RSS/HWM; `--parents` and `--samples` scale the workload. | binary, hub, Chromium |
| bug_report_upload_browser | `python3 tests/bug_report_upload_browser.py` | Real Hub/node report staging recovers a lost reply without duplicate uploads or launches. | binary, Chromium |
| code_diagram_browser | `python3 tests/code_diagram_browser.py` | Fenced diagrams preserve terminal column alignment with a CJK browser fixed font. | binary, Chromium |
| send_native_codex_browser | `python3 tests/send_native_codex_browser.py` | Conversation SEND against a resumed native Codex session with a private fake CLI. | binary, Chromium |
| session_global_actions_browser | `python3 tests/session_global_actions_browser.py` | Global actions follow the hidden session list with synthetic capabilities; real term-list refresh toggles create availability while docked and keeps the conversation node. | binary, Chromium |
| opencode_spawn_browser | `python3 tests/opencode_spawn_browser.py` | An `opencode run` started from a Claude tool shell nests under that Claude; subagent child, older session and an ambiguous directory stay roots; synthetic process tree and OpenCode database. | binary, Chromium |
| opencode_mirror_browser | `python3 tests/opencode_mirror_browser.py --binary target/release/sessiondock` | Chromium: idle database queries skipped, WAL updates, streaming settlement, replacement, restart and deletion. | binary, Chromium |
| hub_transfer_browser | `python3 tests/hub_transfer_browser.py --binary target/release/sessiondock` | Chromium: terminal journals remain unread after startup, new tasks, leases, cancellation and offline restart recovery. | binary, hub binary, Chromium |
| opencode_browser | `python3 tests/opencode_browser.py --binary target/release/sessiondock` | Fake OpenCode CLI and SQLite store through list, search, create, send, stop, resume, report and delete in Chromium. | binary, Chromium |
| shell_env_browser | `python3 tests/shell_env_browser.py` | Real Hub and two nodes behind a synthetic shell wrapper: an rc edit reports changed variable names (no values), the hub and node notices at 390px, ignore, 全部重启 and a single restart exit 75 after graceful shutdown, a fresh start clears each row; removal/reappearance changes card identity, a later build notice follows the shell card, and row/bulk restart failures recover the same buttons without duplicate pending POSTs. | binary, Chromium |
| spawn_chronology_browser | `python3 tests/spawn_chronology_browser.py` | Legacy inferred parents newer than their Grok child are repaired; valid children, explicit user parents and native activity remain correct across Chromium/SSE and restart. | binary, Chromium |
| terminal_heartbeat_browser | `python3 tests/terminal_heartbeat_browser.py` | Stalled terminal sockets recover without replaying ambiguous input. | binary, Chromium |
| terminal_scrollback_browser | `python3 tests/terminal_scrollback_browser.py` | Real wheel input scrolls PTY history locally in both console renderers. | binary, Chromium |
| terminal_history_paging_browser | `python3 tests/terminal_history_paging_browser.py` | Main console pages host scrollback older than the 2000-row snapshot through `GET /api/term/grid/history`: scrollbar drag, wheel and Home reach row 1 of 5000 with contiguous seams and a stable viewport; live input after paging, no idle requests, reload, width change, no requests in final screen; a `--history 60` host streams 600 rows live past its full history without a reconnect and keeps them through a shorter then taller window. | binary, Chromium |
| terminal_reflow_browser | `python3 tests/terminal_reflow_browser.py` | ~9000 fully paged history rows with wrapped lines: five width changes measured (synchronous `term.resize` and longest main-thread task, budgets 60/120 ms), rows contiguous, a held selection keeps its text and copies it on release, the top logical line stays on top; rows the host moves between history and screen on wider/narrower/shorter/taller windows appear exactly once, also with the history-tail request failing. | binary, Chromium |
| hub_composer_startup_browser | `python3 tests/hub_composer_startup_browser.py` | Deep-link startup with a slow peer: early composer typing, full inventory completion, retained terminal layouts and cross-machine navigation in desktop/mobile and both themes. | debug hub binary, Chromium |
| hub_draft_recovery_browser | `python3 tests/hub_draft_recovery_browser.py --binary target/release/sessiondock` | Six-node draft discovery with stalled/503 peers: successful peers stay cached, per-node retry/backoff, offline recovery, real session opening and newly registered nodes. | binary, hub, Chromium |
| hub_browser | `python3 tests/hub_browser.py --binary target/release/sessiondock` | The `sessiondock-hub` page over three `hub_fake_node.py` nodes — machine filter chips, per-machine nesting (same native id never cross-nests), NDJSON search progress + one-machine failure, a session opened through the proxy with media/SSE, settings untick/tick/drag/keyboard reorder/rename, `sessiondock.hub.` prefix; `sessiondock-hub` taken from the `--binary` directory | binary, Chromium |
| hub_pending_state_browser | `python3 tests/hub_pending_state_browser.py` | New-session receipts remain visible during partial hub lists; recovery and terminal exit update the page. | binary, Chromium |
| hub_send_browser | `python3 tests/hub_send_browser.py` | Real hub → authenticated Rust node → fake Claude; different builds, desktop/390 px sends, refresh, real hub asset upgrade with draft preservation and stale-page refusal, raw-input/retry/local-spoof auth gates, hub metadata, and a second hub page following draft edits from the pushed `drafts` event, including one written directly on the node | debug binaries, Chromium |
| legacy_browser | `python3 tests/legacy_browser.py` | Legacy slice on the 16cc89c-resynced frontend: native fixtures, SSE append/partial/reset, errors/retry, console, mobile/dark | debug server, Chromium |
| lifecycle_browser | `python3 tests/lifecycle_browser.py` | CLI and bare SSH terminal creation, idempotency, pending input, Web restart, mobile cancel | debug server, ptyhost, Chromium |
| lifecycle_browser_native_binding | `python3 tests/lifecycle_browser.py --native-binding` | API-only operator binding followed by the page, pending/native leases, durable cancel across Web restart | debug server, ptyhost, Chromium |
| lifecycle_cli_browser | `python3 tests/lifecycle_cli_browser.py` | Fake-CLI argv/session-id on create; resume takeover echoes `resume <sid>`; a configured CLI executable rewritten in place after service start still launches (its new contents); a host binary that predates `--no-record` still creates agent sessions | debug server, ptyhost, Chromium |
| lifecycle_unavailable_cli_browser | `python3 tests/lifecycle_unavailable_cli_browser.py` | Missing CLI executable, dangling symlink, non-executable file and directory in profiles/fixed adapters: node and Hub keep serving history, disable only unavailable sources, create and interact with Codex, and preserve running hosts across restart with unchanged configuration | server, sessiondock-hub, ptyhost, Chromium |
| cli_menus_browser | `python3 tests/cli_menus_browser.py` | Native CLI menu fixtures exercised through Chromium and private fake-CLI PTYs; validates key bytes and semantic outcomes without model requests. | sessiondock binaries, ptyhost, Chromium |
| client_update_browser | `python3 tests/client_update_browser.py` | AI client matrix on a real node and Hub with fake CLIs and `curl`: installed/missing version probes reused across concurrent reads, lookup/update polling, settings reopen and reload; 60 s expiry detects external version changes with one shared refresh; successful and failed updates refresh the cache immediately; installed-only columns, outdated/current/empty cells, fixed args, closed stdin, stripped escapes; unknown ID 404, concurrent update 409; 390px without page overflow | server, sessiondock-hub, ptyhost, Chromium |
| loopback_status_browser | `python3 tests/loopback_status_browser.py --binary target/release/sessiondock` | Browser reload and live reads with a refused loopback host, a delayed healthy peer and same-record recovery; latency budget, unknown state and unchanged records. Run on Windows to exercise SYN retry tuning; `--report-only` records baseline. | binary, Chromium |
| live_browser | `python3 tests/live_browser.py` | Three-state `/api/live`: managed running then exited; unrelated stay unknown; obsolete registry/URL cannot hide live or terminal rows | debug server, ptyhost, Chromium |
| live_console_boundary_browser | `python3 tests/live_console_boundary_browser.py` | Nested Codex exec stays live without borrowing the parent CLI console; browser takeover requires confirmation, cancel preserves the parent and its terminal still attaches | debug server, ptyhost, Chromium |
| missing_roots_browser | `python3 tests/missing_roots_browser.py --binary target/release/sessiondock` | Missing native roots at startup, removal and recovery of each source; refresh and open surviving sessions in the real UI. | binary, Chromium |
| managed_terminal_browser | `python3 tests/managed_terminal_browser.py` | Console routed by UID/instance; live Codex rollout rotations and chat/terminal switching on node/Hub without new claims; no new CLI; force/revoke; Web restart | debug server, hub, ptyhost, Chromium |
| codex_empty_rewind_browser | `python3 tests/codex_empty_rewind_browser.py` | Synthetic TUI double Esc/Enter before the first prompt: zero-ordinal rollouts with absent/null history_base, empty history, repeated reset, same socket/guard, reload and input; node/Hub, desktop/mobile, light/dark screenshots. | binary, hub, ptyhost, Chromium |
| media_browser | `python3 tests/media_browser.py --binary target/release/sessiondock` | Embedded PNG/JPEG HTTP + Chromium decode; tool images; no remote fetch | binary, Chromium |
| media_continuation_browser | `python3 tests/media_continuation_browser.py --binary target/release/sessiondock` | Multi-image continuation preserves raw message/descriptor and existing DOM identity; live cursor untouched; 409 after rewrite | binary, Chromium |
| media_files_browser | `python3 tests/media_files_browser.py --binary target/release/sessiondock` | Disk images with configured or default path handling; replacement 409; native bytes unchanged | binary, Chromium |
| media_formats_browser | `python3 tests/media_formats_browser.py --binary target/release/sessiondock` | GIF/WebP/AVIF/BMP Chromium decode across providers and tool results | binary, Chromium |
| media_lazy_browser | `python3 tests/media_lazy_browser.py --binary target/release/sessiondock` | Offscreen zero GET; scroll decode; visible diagnostics; concurrent GET completion; inline fold/unfold cleanup and actual retry; 404/409 reload | binary, Chromium |
| groups_browser | `python3 tests/groups_browser.py --binary target/release/sessiondock` | Inline create/cancel/delete, empty groups, immediate second-level menus with hover/keyboard/touch, cross-node batches, offline deletion/rejoin, same-name recreation and restart persistence. | binary, Chromium |
| metadata_browser | `python3 tests/metadata_browser.py --binary target/release/sessiondock` | Preferences: cross-tab SSE star/unstar, parent visibility, writer restart | binary, Chromium |
| session_start_preference_browser | `python3 tests/session_start_preference_browser.py` | Five providers on node/Hub: per-agent settings, SDK creation and first SEND, continuation, report SDK handoff and lost-response recovery. | binary, Chromium |
| session_start_menu_browser | `python3 tests/session_start_menu_browser.py` | Five providers: saved CLI preference, single context-menu start, group submenu hover, real private CLI launch, terminal selection and stop on node/Hub, desktop/mobile. | binary, ptyhost, Chromium |
| sdk_start_stop_browser | `python3 tests/sdk_start_stop_browser.py --screenshots /tmp/sdk-start-stop` | Five providers on node/Hub: read-only selection, unchanged activity/order, explicit start, two-page stop and restart; desktop/light and mobile/dark. | binary, Chromium |
| codex_activity_browser | `python3 tests/codex_activity_browser.py` | Node/Hub Chromium opens an older running thread whose resume writes settings; checks unchanged order, child activity, genuine replies, partial lines, rewrites, replacement, truncation, search/detail and cold restart beyond metadata windows. | binary, Chromium |
| codex_sdk_browser | `python3 tests/codex_sdk_browser.py --binary target/release/sessiondock` | Codex SDK view through a scripted app-server: history bubbles, model/effort override, streaming, approval across reload, user-input answer, steer, interrupt ([contract](codex-sdk.md)) | binary, Chromium |
| codex_sdk_cancel_recovery_browser | `python3 tests/codex_sdk_cancel_recovery_browser.py --binary target/debug/sessiondock` | Lost submit/cancel replies, cancelled receipt recovery, retained draft text/files/quotes, reload and explicit resend; wide/narrow light/dark | binary, Chromium |
| codex_sdk_errors_browser | `python3 tests/codex_sdk_errors_browser.py --binary target/debug/sessiondock` | Recoverable retry, resumed progress, successful completion, terminal errors and snapshot recovery; wide/narrow light/dark | binary, Chromium |
| codex_sdk_interrupted_browser | `python3 tests/codex_sdk_interrupted_browser.py --screenshots target/sdk-interrupted` | Interrupted native RPC and snapshot reload, new/completed turns, native abort and missing-ending CLI fallback; node/Hub wide/narrow light/dark | binary, Hub binary, Chromium |
| codex_sdk_protocol_browser | `python3 tests/codex_sdk_protocol_browser.py --binary target/debug/sessiondock` | Native child-tool results, MCP invalid/valid JSON submission and sparse quota updates; wide/narrow light/dark | binary, Chromium |
| codex_sdk_questions_browser | `python3 tests/codex_sdk_questions_browser.py --screenshots /tmp/sessiondock-questions` | Async questions: live/native history, reload, radio/free text, failed-submit retry, active steer/idle start and draft preservation; wide/narrow light/dark | binary, Chromium |
| codex_sdk_subagent_browser | `python3 tests/codex_sdk_subagent_browser.py --binary target/release/sessiondock` | SDK child and nested views through node/Hub: history, live message/tool output, approval/question replies, send/steer/stop, exact selected-thread isolation, root/child view preferences, reload/reconnect; desktop/mobile, light/dark | binary, hub, Chromium |
| codex_sdk_create_browser | `python3 tests/codex_sdk_create_browser.py --binary target/release/sessiondock` | Node/Hub SDK default, Create retry, browser reload before native persistence, first SEND and approval; desktop/mobile light/dark | binary, Hub binary, Chromium |
| sdk_creation_retention_browser | `python3 tests/sdk_creation_retention_browser.py --binary target/release/sessiondock` | New Codex identity, draft and same-request result across node restart before first SEND; native handoff/deletion and durable discard | binary, Chromium |
| conversation_composer_visibility_browser | `python3 tests/conversation_composer_visibility_browser.py --screenshots /tmp/conversation-composer` | Composer/footer visibility, chat/terminal switching and retained drafts; desktop/mobile light/dark | binary, Chromium |
| conversation_status_browser | `python3 tests/conversation_status_browser.py --screenshots /tmp/sessiondock-footer` | Conversation footer status and cached usage through page interactions; desktop/mobile light/dark | binary, Chromium |
| sdk_status_disclosure_browser | `python3 tests/sdk_status_disclosure_browser.py --screenshots /tmp/sdk-status-disclosure` | Five SDKs and conversation footers: short statuses cannot open duplicate details; errors/retry reasons retain mouse and keyboard access, live transitions close stale details; 320–1912px light/dark | binary, ptyhost fixture, Chromium |
| sdk_shared_ui_browser | `python3 tests/sdk_shared_ui_browser.py --screenshots /tmp/sdk-shared-ui` | All five SDKs: history/tool expansion, parameters and wrapping survive updates, desktop/mobile Enter rules, SEND focus, edits during admission, queued Markdown, keyboard status details and resize cleanup; 1440px light/390px dark | binary, ptyhost fixture, Chromium |
| codex_sdk_status_browser | `python3 tests/codex_sdk_status_browser.py --binary target/release/sessiondock` | Node/Hub SDK blue markers, external CLI green, reload and disconnect/reconnect; desktop/mobile light/dark | binary, Hub binary, Chromium |
| codex_sdk_terminal_browser | `python3 tests/codex_sdk_terminal_browser.py --binary target/release/sessiondock` | SDK → PTY attaches to the same daemon without replacement; SDK remains usable, independent CLI still asks for confirmation | binary, ptyhost, Chromium |
| codex_sdk_recovery_browser | `python3 tests/codex_sdk_recovery_browser.py --binary target/debug/sessiondock` | Writer-lock rejection, private CLI stop, automatic SDK reopen, selected-button retry, retained history/draft and explicit SEND; desktop/mobile light/dark | binary, ptyhost, Chromium |
| codex_sdk_history_recovery_browser | `python3 tests/codex_sdk_history_recovery_browser.py --screenshots /tmp/sdk-history-recovery` | Full active-turn replies after cold attach, detached work and bridge reconnect; native turn boundary after compaction, duplicate prompts, passive short status; desktop/mobile light/dark | binary, Chromium |
| codex_sdk_history_pages_browser | `python3 tests/codex_sdk_history_pages_browser.py --screenshots /tmp/sdk-history-pages` | SDK native head/tail entry without a full fetch, explicit middle pages/chains, SSE races, failure/retry, detach guards and completion; desktop/mobile light/dark | binary, Chromium |
| codex_sdk_compose_browser | `python3 tests/codex_sdk_compose_browser.py --binary target/release/sessiondock` | SDK draft and staged image/file recovery through node/Hub, paste/drop/select, remove and failed SEND retention | binary, Hub binary, Chromium |
| sdk_attachment_display_browser | `python3 tests/sdk_attachment_display_browser.py --screenshots /tmp/sessiondock-attachments` | Four native SDKs: actual upload/SEND, image and document links, reload/chat and downloads; wide/narrow light/dark. Codex node/Hub live/history previews are covered by codex_sdk_compose_browser | binary, ptyhost, Chromium |
| codex_sdk_actions_browser | `python3 tests/codex_sdk_actions_browser.py --binary target/release/sessiondock` | Native fork/revert/compact/review, turn pagination, slash commands and errors, desktop/mobile light/dark | binary, Chromium |
| codex_sdk_rename_browser | `python3 tests/codex_sdk_rename_browser.py --binary target/release/sessiondock` | Native SDK rename, dialog/cancellation/errors, busy turn, draft race, titles/reload and pending threads, node/Hub and desktop/mobile light/dark | binary, Chromium |
| codex_sdk_controls_browser | `python3 tests/codex_sdk_controls_browser.py --binary target/release/sessiondock` | Flat action menu, native mode/permissions, removed goal controls, shell output, failure retention and reconnect, desktop/mobile light/dark | binary, Chromium |
| codex_sdk_bubbles_browser | `python3 tests/codex_sdk_bubbles_browser.py --screenshots /tmp/sessiondock-bubbles` | Type-based previews, full assistant prose, disclosure clicks during streaming and state across deltas; wide/narrow, light/dark | binary, Chromium |
| codex_sdk_layout_browser | `python3 tests/codex_sdk_layout_browser.py --binary target/debug/sessiondock` | Single-row priority overflow at 14 widths from 320–1440px, 100/125% scale, light/dark; expanded actions/permissions, actual selector values, long labels, drafts and sidebar resizing | binary, Chromium |
| sdk_overflow_browser | `python3 tests/sdk_overflow_browser.py --screenshots /tmp/sdk-overflow` | SDK overflow hides when empty; controls-only, message-only, reset-only and populated groups; live reset updates, policy clicks, resize, draft retention, light/dark and 100/125% scale | binary, Chromium |
| codex_sdk_compaction_browser | `python3 tests/codex_sdk_compaction_browser.py --binary target/debug/sessiondock` | Pre-input compaction, native consumption confirmation, interrupt/retry, reload and node restart preserve unconfirmed input; desktop/mobile light/dark | binary, Chromium |
| codex_sdk_queue_browser | `python3 tests/codex_sdk_queue_browser.py --binary target/debug/sessiondock` | Default FIFO, cancel/explicit steer/retry, already-running TUI turn recovery, lost HTTP acknowledgment, draft reload, daemon/node restart; node/Hub desktop/mobile light/dark | binary, Hub binary, Chromium |
| codex_sdk_message_actions_browser | `python3 tests/codex_sdk_message_actions_browser.py --binary target/debug/sessiondock` | User bubble left menu, revert/cancel and fork before selected native turn, source preservation, restored drafts/reload; node/Hub desktop/mobile light/dark | binary, Hub binary, Chromium |
| codex_sdk_stop_browser | `python3 tests/codex_sdk_stop_browser.py` | Middle-message fork excludes later turns; parent PTY and exact SDK thread stop independently of its branch on a shared daemon; node/Hub desktop/mobile | binary, Hub binary, ptyhost, Chromium |
| grok_sdk_browser | `python3 tests/grok_sdk_browser.py --binary target/debug/sessiondock` | Native ACP create/resume, streamed tools, exact approvals, dedup, serial input, interrupt, reconnect and external writer refusal | binary, Chromium |
| grok_sdk_lifecycle_browser | `python3 tests/grok_sdk_lifecycle_browser.py --binary target/debug/sessiondock` | Explicit interjection receipts, ten-minute idle release, active/approval/background protection and same-SID reopen | binary, Chromium, C compiler (Linux clock fixture) |
| native_sdk_queue_browser | `python3 tests/native_sdk_queue_browser.py --binary target/debug/sessiondock` | Durable queue, shared views, cancellation/retry receipts, draft preservation, stop, uncertain delivery and node restart | binary, Chromium |
| sdk_multitab_browser | `python3 tests/sdk_multitab_browser.py --binary target/debug/sessiondock` | Codex two-tab HTTP/1 cancellation, running-turn preservation, chat watch restoration/stop; replaced Codex/Grok history reads do not publish stale abort errors | binary, Chromium |
| agy_sdk_browser | `python3 tests/agy_sdk_browser.py --binary target/debug/sessiondock` | Native headless create/resume, FIFO, stream/tool/error, dedup, stop, drafts/reload and external writer refusal; wide/narrow light/dark | binary, Chromium |
| agy_model_browser | `python3 tests/agy_model_browser.py --binary target/debug/sessiondock` | Native model evidence in Agy header/list/footer, init/default, live updates, reload/service recovery, catalog failure and explicit selection; wide/narrow light/dark | binary, ptyhost fixture, Chromium |
| agy_effort_browser | `python3 tests/agy_effort_browser.py --binary target/debug/sessiondock` | Agy grouped model/effort choices, exact native variant IDs, base/default and plain flags, legacy conflicts, active-turn settings and reload; wide/narrow light/dark | binary, ptyhost fixture, Chromium |
| native_model_browser | `python3 tests/native_model_browser.py --binary target/debug/sessiondock` | Claude historical/live/unknown model, Grok currentModelId/config, Codex settings and OpenCode provider/model; header/list/footer, catalog failure, reconnect, no implicit override; wide/narrow light/dark | binary, Chromium |
| agy_usage_browser | `python3 tests/agy_usage_browser.py --binary target/debug/sessiondock` | Agy direct usage dropdown, token grid, grouped quota, refresh/loading/failure, long content and /usage; 1280/390/320px light/dark | binary, ptyhost fixture, Chromium |
| opencode_sdk_browser | `python3 tests/opencode_sdk_browser.py --binary target/debug/sessiondock` | Native v2 API, tail-page streaming/tools/errors, approvals/questions, create/model, dedup, stop/reconnect and standalone refusal | binary, ptyhost, Chromium |
| claude_sdk_browser | `python3 tests/claude_sdk_browser.py --binary target/debug/sessiondock` | Native stream-json send/stop, tools/errors, approvals/questions, reconnect, idempotency, lazy create and independent writer refusal; wide/narrow light/dark | binary, Chromium |
| native_sdk_hub_browser | `python3 tests/native_sdk_hub_browser.py --binary target/debug/sessiondock` | Hub capability/UID routing, create, first SEND, drafts/reload and real approval/question controls; wide/narrow light/dark, no PTY | binary, Hub binary, Chromium |
| native_sdk_recovery_browser | `python3 tests/native_sdk_recovery_browser.py --screenshots /tmp/native-sdk-recovery` | Four native SDKs: usage/billing retry/backoff, retained values, close/abort/late read isolation and pending queue controls across new snapshots; wide light/mobile dark | binary, ptyhost fixture, Chromium |
| native_sdk_footer_browser | `python3 tests/native_sdk_footer_browser.py --screenshots /tmp/native-sdk-footer` | Grok/Agy/OpenCode/Claude footer controls with synthetic native responses: model/effort uniqueness and catalog fallback, in-flight selections, shared settings/action overflow, retry details, exact gauges, full usage and drafts; 320/390/1440/1912px light/dark | binary, ptyhost fixture, Chromium |
| codex_names_browser | `python3 tests/codex_names_browser.py --binary target/release/sessiondock` | Codex names: file-order/unicode, fork/agent titles, search, desktop/mobile Chromium SSE | binary, Chromium |
| native_envelopes | `python3 tests/native_envelopes.py --browser --binary target/release/sessiondock` | Stringified Codex tool images and nested replay; HTTP + Chromium | binary, Chromium |
| native_spans | `python3 tests/native_spans.py --browser --binary target/release/sessiondock` | Native image spans, MIME/data-URL aliases and first-match field aliases | binary, Chromium |
| native_tags_browser | `python3 tests/native_tags_browser.py --binary target/release/sessiondock` | Audited native tag families in Chromium; each positive case has a literal or malformed control, and image delimiters are not file-read authority | binary, Chromium |
| child_modes_browser | `python3 tests/child_modes_browser.py --binary target/release/sessiondock` | Desktop/mobile three-mode clicks, lazy child counts and arrows, one-level expansion, collapse/release, stale-response race, reload reset, polling, flat/tree restoration and open child detail. | binary, Chromium |
| nest_tree_browser | `python3 tests/nest_tree_browser.py --binary target/release/sessiondock` | Sidebar nesting (Python `nest_tree_e2e` port) on the backend's own fields: legacy `spawned_by` migrated to `nest_parent` in `session-metadata.json`, `active` from the sidecar's open turn, `continued_in` from the tail record; flat list with subagent rows hanging under their owner (caret folds only the agents), tree/ordering/carets aligned with the group caret, `.agent-mark`, active-only filter refuses without a scan, hidden continued-in parent with its continuation listed and followed on open, in-place refresh after on-disk edits; right-click detach/attach-to with cancel; desktop + 390 px | binary, Chromium |
| pending_create_discard_browser | `python3 tests/pending_create_discard_browser.py --binary target/release/sessiondock` | Headless create-and-discard for Claude, Codex, Grok and SSH: unsent composer input, sidebar menu (unused AI discard; SSH stop then delete), header 删除, reload and a second page must not rebuild the row (Grok writes empty `summary.json` on start). | binary, ptyhost, Chromium |
| pending_session_link_browser | `python3 tests/pending_session_link_browser.py` | Pending launch links survive refresh, sharing and browser navigation on node and Hub. | binary, hub binary, ptyhost, Chromium |
| new_session_follow_browser | `python3 tests/new_session_follow_browser.py` | A new Claude session whose startup notice (SessionStart hook message) creates the native history before any input: the launch page switches to the native session while the user types — no loading interstitial, the composer never hides and keeps focus and draft, no extra browser history entry (fake CLI, desktop + 390 px). | Chromium, ptyhost |
| session_continue_browser | `python3 tests/session_continue_browser.py` | Continue from an indexed conversation: report-style dialog, prefixed first SEND and fake-CLI reply, cancellation, create retry, refused SEND with draft recovery across reload, and ordinary-create reset; desktop/phone, light/dark. | Chromium, ptyhost |
| new_session_model_browser | `python3 tests/new_session_model_browser.py --binary target/release/sessiondock` | Headless new-session model and effort picker with a free fake CLI for Claude, Codex, Grok and OpenCode: each CLI's own catalog (Codex/Grok caches with hidden models skipped, Codex config default, OpenCode `models` with a search box above ten), remembered choice per source, a row that never moves while switching sources, the choice in the launched argv or OpenCode's pre-created session, SSH disabled, phone wrap. | binary, ptyhost, Chromium |
| popup_browser | `python3 tests/popup_browser.py` | No native alert/confirm: a delete asks in a centered `.app-popup` with the dialog look (取消/Esc keep, 确定 deletes); the stale-build card sits in the centered float stack and 稍后 retains its hidden identity; an HTTP login redirect and actual login button open the local app with no opener; desktop + 390px. Other browser suites answer popups through `tests/popups.py` `on_popup`. | binary, Chromium |
| overlay_layout_browser | `python3 tests/overlay_layout_browser.py` | Phone, landscape, report viewport and desktop at 50–150% scale, both themes: environment notices, restart failure/progress, stacked update cards, settings/new/report/trash/cleanup dialogs and model menus; actual input, button reachability and screenshots. | binary, Chromium |
| pick_drag_browser | `python3 tests/pick_drag_browser.py` | 多选 mode: dragging with the left mouse button from an unpicked row picks the run, from a picked row unpicks it; dragging back restores rows that left the run, the release does not re-toggle the pressed row, no text is selected, holding at the bottom edge auto-scrolls and extends the run; a plain click still toggles one row, a double click selects no text; entering/leaving 多选 keeps the existing row nodes (checkboxes patched in place). | binary, Chromium |
| prefs_migration_browser | `python3 tests/prefs_migration_browser.py --binary target/release/sessiondock` | Seeds theme/font/layout/filter/cache/unread/selection and file-manager preferences under `sessiondock.*`; verifies they apply, changes persist across reload, `files.html` uses `sessiondock.files-*`, a fresh browser keeps defaults, and PWA identity is SessionDock. | binary, Chromium |
| frontend_framework_browser | `python3 tests/frontend_framework_browser.py --binary target/release/sessiondock` | Desktop and phone settings operated through Chromium: appearance/features controls, close/reopen, reload persistence, Escape and continued access to Machines. | binary, Chromium |
| frontend_cutover_browser | `python3 tests/frontend_cutover_browser.py --binary target/release/sessiondock` | Retired worker/cache cleanup, prefixed search/history/settings, HTML metadata injection and script availability for the frontend. | binary, Chromium |
| frontend_entry_browser | `python3 tests/frontend_entry_browser.py --binary target/release/sessiondock` | Real desktop/phone operation under a loopback `/sessiondock/` proxy: initial loading, late read isolation after a real session switch, manual 503 retry, session selection, theme/font/cache controls, restart of the private Rust fixture, restored conversation and preferences, browser history, and prefix-relative asset/API requests. Runs with either frontend via `SESSIONDOCK_TEST_WEB_DIR`. | binary, Chromium |
| frontend_responsiveness_browser | `python3 tests/frontend_responsiveness_browser.py --binary target/release/sessiondock` | Large sidebar with 2002 synthetic sessions (windowed; logical rows checked via `group._rows`): clicks and cached navigation update only the selected rows, live paints retain history, search filters and restores the list; decoded catalog updates reuse unchanged rows and agents while changed parent/child titles reach the UI. Reports opening times without machine-specific thresholds. | binary, Chromium |
| machine_controls_browser | `python3 tests/machine_controls_browser.py` | Hub machine palette, Escape/outside close, unsaved name/caret preservation through polling, stable rows, pending name/enabled requests, failed control rollback and no renderer field; actual controls with synthetic nodes and temporary display failures. | binary, sessiondock-hub, Chromium |
| server_log_browser | `python3 tests/server_log_browser.py` | Structured server log on a node and the Hub: with `SESSIONDOCK_LOG_REQUESTS=all` the page's requests are one JSON line each (method, path, status, ms), an ApiError keeps its code at level error, the trace id is kept, secret markers in query strings and bodies never reach the log, and the default mode logs only 5xx and slow requests. | binary, hub, Chromium |
| session_relations_browser | `python3 tests/session_relations_browser.py --binary target/release/sessiondock` | Top-level work adjacency, information page, cross-node handoffs, hints, persistence and offline references. | binary, Chromium |
| session_identity_browser | `python3 tests/session_identity_browser.py --binary target/release/sessiondock` | Session identity copying through real menus and clipboard with private fixtures. | binary, Chromium |
| sidebar_toggle_browser | `python3 tests/sidebar_toggle_browser.py --binary target/release/sessiondock` | Large sidebar (1200 open rows, windowed): resource viewport rendering, row reuse when nesting changes, 12 column toggles under CPU throttling, and scroll hydration of the last logical row; failure diagnostics include row/window geometry. | binary, Chromium |
| sidebar_flat_scroll_browser | `python3 tests/sidebar_flat_scroll_browser.py --binary target/release/sessiondock` | 6400 flat timeline sessions with long paths and titles at three viewport widths: wheel down/up across date groups and toggle resources; sample the viewport to reject visible spacer gaps far from the list end. | binary, Chromium |
| sidebar_directory_colors_browser | `python3 tests/sidebar_directory_colors_browser.py --binary target/release/sessiondock` | Timeline directory colors in light/dark and wide/narrow Chromium: distinct colors on screen, only the final directory tinted plus ancestors needed for same-name directories, colors kept while wheel-scrolling, restored after returning to the top and reloading; clicking a directory toggles the persisted tinted-only display without opening the session or moving the row; the session header shows the whole path with the sidebar's color even in that mode; a tall screen uses the second sixteen colors only after all primaries. Optional `--screenshots DIR`. | binary, Chromium |
| sidebar_scale_browser | `python3 tests/sidebar_scale_browser.py --binary target/release/sessiondock` | ~4000 synthetic sessions over 40 open groups with nested children and subagents: prints initial/rerender/live-paint timings, scroll long tasks and DOM size, asserts the visible window stays bounded (≤240 rows, rerender ≤250 ms), then by real input: jump to the bottom and click the last row, deep link to a row far below the window, 多选 drag range auto-scrolled across unrendered rows, group checkbox over unrendered rows, search filter, group and branch fold, date view, text selection kept while scrolling, scroll position across a list update. `--report-only` prints timings only. | binary, Chromium |
| renderer_fd_browser | `python3 tests/renderer_fd_browser.py --binary target/release/sessiondock` | The real page flushes ~75 audit batches in 30 s while the renderer's fd count is read from /proc (Chromium without sandbox, Linux only); growth must stay under 0.1 fd per batch — an unread fetch response pins a 2 MiB shared-memory pipe until GC and the 1024-fd renderer limit froze the tab in GPU code | binary, Chromium, /proc |
| prompt_claude_real | `python3 tests/prompt_claude_real.py --browser` | Real Claude question card, cheapest configuration (`claude-haiku-4-5-20251001`, effort low, `--settings` bridge, `--tools AskUserQuestion`): hook file, `prompt` field, SSE `prompt_only`, answer by clicking the card on the real page, cleared once the native answer lands; skips when the Claude CLI or its login is absent. | claude CLI, ptyhost, Chromium |
| question_browser | `python3 tests/question_browser.py` | Claude question cards answered from a 390 px page against the fake Claude menu (Claude Code 2.1.283 navigation: Up wraps, Down stops on "Chat about this", the text row eats digits and Left/Right): a two-question form from the default and from parked cursors and a single question from the text row each select exactly the clicked options; a fork's `agent_id` PreToolUse shows no card; cards written by the real `claude-hook`. | Chromium, ptyhost |
| rewind_cli_browser | `python3 tests/rewind_cli_browser.py` | A rewind made in Claude's own TUI (fake CLI, no native write) followed as a `cli` pin: rewound input and answer leave the view with the one-line notice, no Esc-return claim, reload keeps it, the next input settles it natively; recalling an answered input still on screen neither pins nor claims an Esc return. | debug binaries, Chromium, ptyhost |
| claude_explicit_rewind_browser | `python3 tests/claude_explicit_rewind_browser.py` | Explicit native rewind boundaries retract interrupted branches over SSE, reload and search; delayed markers, empty/nonempty leaves and later ordinary Esc siblings. Optional `--screenshots DIR` captures wide/narrow light/dark states. | binary, Chromium |
| claude_continuation_browser | `python3 tests/claude_continuation_browser.py` | Native continued-in with a saved attachment to the origin: compact hidden-child catalog, live selected-view follow, same PTY socket/instance, terminal input, old-SID links and reload; node/Hub, desktop/mobile, light/dark screenshots. | binary, hub, ptyhost, Chromium |
| search_browser | `python3 tests/search_browser.py --binary target/release/sessiondock` | NDJSON search, flags, navigation, unsupported regex error and recovery | binary, Chromium |
| search_cold_browser | `python3 tests/search_cold_browser.py --binary target/release/sessiondock` | Tool-heavy synthetic native history: cold/hot semantic search equality, tool output excluded, append freshness, subsequent history open/checkpoint and native bytes preserved; reports timings. | binary, Chromium |
| search_uuid_browser | `python3 tests/search_uuid_browser.py --binary target/release/sessiondock` | Desktop/mobile full and partial native UUID/UID filtering, Enter search with cold/hot cache, sidecar isolation, unreadable history and actual navigation. | binary, Chromium |
| search_no_fold_browser | `python3 tests/search_no_fold_browser.py --binary target/release/sessiondock` | Desktop/mobile search starts expanded, supports independent date/project and child folds, preserves them on refresh, resets them for new queries/runs, and restores saved list folds on exit. | binary, Chromium |
| send_claude_returned_browser | `python3 tests/send_claude_returned_browser.py` | Fake Claude SEND → enqueue → popAll/popOne; returned state, dismiss/reload, native echo, old receipts beyond history window, wide/narrow light/dark screenshots | Chromium, ptyhost |
| send_claude_tabs_browser | `python3 tests/send_claude_tabs_browser.py` | Claude tab-to-four-space native echo, repeated SEND, whitespace/time/role/session boundaries and old receipt recovery after restart beyond the history window | Chromium, ptyhost |
| send_browser | `python3 tests/send_browser.py` | Claude reliable send through the real legacy composer against the fake Claude CLI (desktop + 390 px); busy sends show as queued bubbles from the server CLI state (`check` returns `cli.queued`) until their native echo; a second page's send and composer Esc refused with the owner while the console is held, the 390 px Esc written with no console open; attachments staged on selection, surviving a reload, failed staging with retry, discard of removed staging, publication on SEND; a six-file paste confirm dismissed then accepted, five files unasked. | Chromium, ptyhost |
| composer_editor_browser | `python3 tests/composer_editor_browser.py --binary target/release/sessiondock` | Desktop and narrow composer: typing, Shift+Enter/mobile Enter, simulated IME key guard, native input-history selection and dismissal, attachment chooser/reference replacing a selection, and textarea growth/cap/shrink. | Chromium, fake CLI, ptyhost |
| draft_rotation_browser | `python3 tests/draft_rotation_browser.py` | Codex native rollout rotation through the same terminal: populated and empty drafts recover from a lower server revision, save and SEND once with native echo; direct node and Hub, wide/light and narrow/dark screenshots. | Chromium, ptyhost, Hub, fake Codex |
| draft_sync_browser | `python3 tests/draft_sync_browser.py` | Two pages on one draft: a second page opens the saved text with the server revision and no error, a refused save rebases and the editing page wins (no revision dead end), an idle page follows both ways, including from the pushed `drafts` event with its input CHECK blocked, an attachment staged on one page is sent from the other, that SEND empties the first page, an image staged on one page previews on the other from the staged bytes, and a console command sent from the composer clears the server draft. | Chromium, ptyhost |
| send_readiness_browser | `python3 tests/send_readiness_browser.py` | Grok login/unknown SEND refusal, recovery, post-paste recheck, and no auto-retry of an unknown write (fake CLI, desktop + 390 px). | Chromium, ptyhost |
| send_suggestion_browser | `python3 tests/send_suggestion_browser.py` | Claude dim suggestion matching the submitted text settles without the three-second timeout; unchanged ghost text cannot confirm a delayed paste. | Chromium, ptyhost |
| send_codex_attachments_browser | `python3 tests/send_codex_attachments_browser.py` | Codex composer sends text plus two PNG attachments while an unrelated transfer stalls, then text plus a `.txt`, against the fake Codex CLI; each batch is published to the session cwd and submitted with Enter. | Chromium, ptyhost |
| agy_delete_real_browser (`--real-only`) | `python3 tests/agy_delete_real_browser.py` | Real Agy 1.4.0 native permanent deletion in private HOME with loopback synthetic model; running protection, cancel, four viewport/theme confirmations, no model call during deletion, other SID resumes. | real agy, ptyhost, Chromium |
| agy_real_browser (`--real-only`) | `python3 tests/agy_real_browser.py` | Real Agy (1.3.0 on 2026-10-07) with private HOME/XDG and loopback synthetic gateway; checks models/argv, composer, native binding/history, menu guards and stopped-session `--conversation` continuation., outside the default sweep; no paid model or credentials. The separate title generator may use the second synthetic catalog model. Includes complete report-worker prompt submission and native reply. See [Agy](agy.md) for evidence boundaries. | real agy binary, ptyhost, Chromium |
| agy_interactions_real_browser (`--real-only`) | `python3 tests/agy_interactions_real_browser.py` | Real Agy private HOME and loopback model: command once/deny/cancel/amend, session/persistent grants and subsequent commands, create file, single/multiple/write-in questions, page navigation; five native slash menus without phantom echo queues. | real agy, ptyhost, Chromium |
| agy_tools_real_browser (`--real-only`) | `python3 tests/agy_tools_real_browser.py` | Real CLI executes two read-only `view_file` calls against temporary text/PNG fixtures through a loopback synthetic gateway. Chromium opens the native history, expands tools and loads the native image; asserts the actual mapped wire model. | real agy binary, Chromium |
| send_grok_browser_real (`--real-only`) | `python3 tests/send_grok_browser_real.py` | Real Grok in a private `GROK_HOME` (linked login, copied model cache): `grok-4.6` and low effort chosen in the new-session picker, one composer SEND, exactly one native user query, a reply, and the summary's recorded model/effort; the everyday config is unchanged. Spends real credits. | Chromium, ptyhost, Grok login |
| startup_claude_browser | `python3 tests/startup_claude_browser.py` | Fake Claude workspace-trust question before hooks/history, answered through the shared composer cards. | sessiondock binaries, ptyhost, Chromium |
| session_start_sdk_browser | `python3 tests/session_start_sdk_browser.py` | Saved Claude session starts from SDK view through row/header controls, connects the private PTY, accepts typed input and retains the terminal after discovery; node/Hub, desktop/mobile, light/dark | binary, Hub binary, ptyhost, Chromium |
| startup_question_browser | `python3 tests/startup_question_browser.py` | Fake Codex folder-trust startup question answered through the composer before a rollout exists. | sessiondock binaries, ptyhost, Chromium |
| send_codex_startup_browser_real | `python3 tests/send_codex_startup_browser_real.py` | Operator-only (`--include-real`): cold-start SEND with real Codex at Luna low, checking native user text and the turn's model/effort. | codex CLI, Chromium |
| session_tree_delete_browser | `python3 tests/session_tree_delete_browser.py` | Chromium/Hub desktop and mobile: same connected members as copy, preview cancellation, changed-member refusal, hidden ancestors, physical generations, agents, running hidden-agent refusal, mixed Claude/Codex/Grok ownership, one trash entry, monotonic progress, lost-response recovery, cross-filesystem exact restoration of bytes/titles/stars and durable retry after restore/restart. Private synthetic fixtures. | sessiondock binaries, Chromium |
| session_clone_browser | `python3 tests/session_clone_browser.py` | Real Hub/node UI: transfer table, target/mode/identity selection, mobile layout, native title hints without custom overlays (running action/menu, offline machine, empty agent; hover/touch/keyboard guards and live recovery) and unsupported-action guard; clone stopped complex Codex group, read inherited history and subagents; native metadata, code-mode mapping, unchanged source, repeated confirmation, restart compensation and second-database failure rollback. Synthetic private fixtures. | sessiondock binaries, Chromium |
| session_bundle_browser | `python3 tests/session_bundle_browser.py` | Real Hub/two-node Chromium copies Codex/Claude/Grok complex groups, opens history/agents, rejects corrupt/truncated/traversal bundles and mismatched roots/cwd, and retries after restart. `--preserve` covers unchanged identities/bytes and reuse; `--move` covers trash, persistent ownership, shared-storage rejection, moving back, interrupted cleanup, new outside references (including Claude SendMessage calls and resolved short destinations during partial cleanup), stale handoff compensation and interrupted withdrawal through the page, reopening tasks after browser reload (including narrow screens), and durable byte/phase progress. Optional `--peer SSH_ALIAS` uses independent remote temporary storage over loopback SSH forwarding. With a peer, `--dependencies` exercises missing and changed typed images, persisted outputs and external bundle symlinks through confirmation, then checks the recorded dependency inventory and successful copy/move without changing those external files. `session_transfer_peer.py` is its private fixture worker. | sessiondock binaries, Chromium; optional SSH peer with shared checkout |
| session_prefix_browser | `python3 tests/session_prefix_browser.py --peer SSH_ALIAS` | Explicit two-node Chromium test of Codex/Claude/Grok complex groups: preserved-ID prefix copy and move, divergent target rejection, Codex current-rollout conflict, native-row, display-preference and original-byte restoration after simulated receiver crash, and preservation of target continuation or concurrent preference changes during compensation. Private fixtures and remote loopback forwarding. | sessiondock binaries, Chromium, SSH peer with shared checkout |
| session_transfer_environment_browser | `python3 tests/session_transfer_environment_browser.py` | Chromium fourteen-session mixed family: missing, older or unknown target CLI advice; stale destination response isolation; native database and rollout dynamic-tool names (legacy and namespaces) without leaking definitions; older plans remain explicitly unverified; confirmation still copies the group. CLI replies are synthetic and no models run. | sessiondock binaries, Chromium |
| session_mixed_clone_browser | `python3 tests/session_mixed_clone_browser.py --binary target/release/sessiondock` | Reject changed staging before publication, replan a connected cross-provider clone after restart, and open native share links without altering sources. | binary, ptyhost, Chromium |
| session_local_recovery_browser | `python3 tests/session_local_recovery_browser.py` | Chromium local complex-group copies for Codex/Claude/Grok: actual publication failure, task recovery after reload and Hub/node restart, fixed identities and unchanged source; also crashes the Hub during a blocked Codex database publication and recovers the completed node result through the page. | sessiondock binaries, Chromium |
| session_files_clone_browser | `python3 tests/session_files_clone_browser.py` | Real Hub/node Chromium confirms Claude/Grok complex family copy, opens history/agents, checks unchanged source, failed publication rollback and restart retry preserving continued copies. Exact byte comparison covers layout, CRLF, EOF and escaped text; Claude metadata retains absent message fields. Also resumes a different parent’s agent, remaps a resolved short SendMessage destination and isolates same-named calls in other transcripts. | sessiondock binaries, Chromium |
| session_grok_checkpoint_browser (operator only) | `python3 tests/session_grok_checkpoint_browser.py` | Native Grok compaction in a private home, Chromium clone with new checkpoint IDs, native load/rewind and continuation recalling compacted context; source and daily configuration unchanged. Explicit Grok 4.6 / low; checks the selected model before the first request. | sessiondock binaries, Chromium, authenticated Grok |
| session_files_browser | `python3 tests/session_files_browser.py` | Synthetic Claude sibling branches, sidecar agents, tool results and file-history; Grok parent branches and full directories. Chromium opens byte-preserved and new-ID staging; source remains unchanged. Not native publication acceptance. | sessiondock binaries, Chromium |
| session_transfer_browser | `python3 tests/session_transfer_browser.py` | Rust offline environment/group/plan/stage entry: cwd existence without scanning project content, explicit dependency contents and stale-source checks; nonce probes distinguish shared storage; synthetic multi-generation Codex files and archived sibling; byte-exact move, clone identities/boundaries, unchanged source, retries/conflicts/stale inputs; Chromium clicks staged histories and remapped subagent menus. Staging stays non-publishable. | sessiondock-transfer binary, Chromium |
| session_transport_browser | `python3 tests/session_transport_browser.py` | Chromium exercises configured SCP priority, relay fallback, old-node compatibility, corrupt archives, lost responses, cancellation and Hub restart recovery. Private fixtures and fake SCP; real two-machine SCP uses `session_bundle_browser.py --peer` with the SCP arguments in [transport](session-transport.md). | sessiondock binaries, Chromium |
| session_transfer_isolation_browser | `python3 tests/session_transfer_isolation_browser.py` | Chromium switches move/copy without planning requests or execution labels; a stalled preparation does not block a different group, in-flight withdrawal, or leave the source locked. An 8 GiB sparse build output and dangling project link are not read. Uses private synthetic Claude/Grok groups and real Hub/node services. | sessiondock binaries, Chromium |
| session_transfer_cache_browser | `python3 tests/session_transfer_cache_browser.py` | Chromium clones a 2.4 MB Claude session; inotify verifies unchanged unrelated histories are not reopened during preview/execution, while added/removed reverse fork edges update the group immediately. | sessiondock binaries, Chromium, Linux inotify |
| session_large_manifest_browser | `python3 tests/session_large_manifest_browser.py --binary target/release/sessiondock` | Chromium copies a group with more than 64 MiB of native before/after images; verifies target history and exact native payload. Optional `--peer SSH_ALIAS` exercises a move across private independent filesystems and source retirement. | binaries, Chromium; optional SSH peer |
| session_link_lineage_browser | `python3 tests/session_link_lineage_browser.py --binary target/release/sessiondock` | Actual Hub/Chromium copy chains: original wins, nearest then oldest descendant, deleted copies, native/UID/subagent bookmarks, restart persistence, missing versus offline. | binaries, Chromium |
| session_deep_link_browser | `python3 tests/session_deep_link_browser.py --binary target/release/sessiondock` | `?sid=` opens the root and a nested subagent from synthetic history, without starting a CLI; flat and hierarchical sidebar modes both reveal the agent row (past the owner's fold) without flipping the user's mode preference. | binary, Chromium |
| session_link_reveal_browser | `python3 tests/session_link_reveal_browser.py --binary target/release/sessiondock` | Private node and Hub: explicit child/agent links reveal collapsed ancestors and selected rows in all three child modes; compact polling, manual folds, reload, Back, desktop/mobile and dark/light themes. | binary, hub, Chromium |
| session_freeze_browser | `python3 tests/session_freeze_browser.py` | Freeze/resume a verified Linux CLI tree, reload recovery, report snapshot and authenticated Hub/mobile controls. | Chromium, ptyhost |
| session_stop_browser | `python3 tests/session_stop_browser.py` | First-position start/stop controls with independent delete; exact-session resume from desktop and mobile sidebar, live transitions, graceful/concurrent stop, duplicate request serialization and retained pending shell receipts. Fake CLIs and private hosts. | Chromium, ptyhost |
| cleanup_counts_browser | `python3 tests/cleanup_counts_browser.py` | Chromium on a real node and Hub with synthetic native files/proc: background updates with pages closed, numeric-only summaries, 30/31/100/101 colors, fresh dialog, obsolete summary and failure state. | binary, hub, Chromium |
| session_cleanup_browser | `python3 tests/session_cleanup_browser.py` | Hub Chromium clicks verify active machine/Agent counts, configurable cleanup days and reload persistence, full-catalog cleanup despite filters/lazy children, cancel, fresh timestamp recheck, incremental rows, stable row heights with right-aligned status on desktop/mobile, offline snapshot exclusion, node routing, partial failures, false/contradictory stop replies, unavailable live refresh and mobile empty state. Synthetic nodes only; `session_stop_browser` also cleans up a real private fake-CLI host. | binary, hub, Chromium |
| session_status_browser | `python3 tests/session_status_browser.py` | Sidebar icon palette, pulse, inactive hiding, reduced motion and conversation clicks across desktop/mobile themes using synthetic live responses. | Chromium |
| turn_state_browser | `python3 tests/turn_state_browser.py` | Sidebar/header turn dots for resumed Claude and Codex sessions (fake CLI): native working/waiting/idle, live TUI state, background terminals and detached owned commands over a quiet screen; permanent helper isolation, reload and completion. | Chromium, ptyhost |
| process_activity_browser | `python3 tests/process_activity_browser.py` | Node and Hub Chromium paths: completed turns with attached, code-mode-launched, detached and sleeping command processes; helper/MCP/zombie/unrelated isolation; namespace, reload and completion repaint both dots without native history changes. | binary, Chromium |
| session_model_browser | `python3 tests/session_model_browser.py` | Chromium opens Claude/Codex titles and verifies latest native models beyond metadata windows, list-only title refresh, automatic observation, partial lines, rewrites, inode replacement and subagent isolation. | binary, Chromium |
| index_append_browser | `python3 tests/index_append_browser.py --binary target/release/sessiondock` | Chromium reloads during metadata-only touches and continuous append; native identity/model/turn remain readable, while same-size rewrites, truncation and inode replacement still refresh list and detail. | binary, Chromium |
| side_drag_browser | `python3 tests/side_drag_browser.py --binary target/release/sessiondock` | Sidebar divider under real CDP touch drag/cancel at 814x380 and mouse drag/dblclick at desktop; desktop row text selection survives the synthetic click and defers list refresh until selection clears; width stored under `sessiondock.` (Python `side_drag_e2e` port) | binary, Chromium |
| sidebar_select_scroll_browser | `python3 tests/sidebar_select_scroll_browser.py --binary target/release/sessiondock` | Scroll the date/nest sidebar to the last row and click it: the clicked session opens, membership is unchanged, `#side.scrollTop` and the last row's viewport Y stay put, and `renderSide` is not called | binary, Chromium |
| sidebar_closed_groups_browser | `python3 tests/sidebar_closed_groups_browser.py --binary target/release/sessiondock` | 5,000 synthetic sessions with 49 of 50 groups closed: hidden updates skip row generation, opening reveals current order, nested counts and closed-group selection stay correct, and date changes move sessions between groups | binary, Chromium |
| sidebar_path_performance_browser | `python3 tests/sidebar_path_performance_browser.py` | Same-basename directory plans, metadata-only updates, repeated branch/group/filter clicks and cache invalidation. | binary, Chromium |
| sidebar_unread_browser | `python3 tests/sidebar_unread_browser.py` | Twenty folded background views use one summary request on a node and through Hub; reset/error isolation and normal history opening. | binary, hub, Chromium |
| network_recovery_browser | `python3 tests/network_recovery_browser.py --binary target/release/sessiondock` | Real Chromium stalls before headers and inside bodies, silent OPEN SSE, offline notice, failed preparation save with request ID, and lost SEND reply recovered by receipt with exactly one SEND | binary, Chromium |
| attachment_resume_browser | `python3 tests/attachment_resume_browser.py --binary target/release/sessiondock` | Node and Hub file selection: lost chunk ACK, truncated request, durable restart, stalled reply, cancel/manual resume, exact final file and SEND publication | binary, hub, Chromium |
| ui_events_browser | `python3 tests/ui_events_browser.py` | Real node and Hub: ten seconds idle without list/live/terminal polling, inactive appends use summaries without body/list reads, user selection catches up, membership invalidation survives a failed metadata read, and event-stream disconnect/reconnect. | binary, hub, Chromium |
| list_delta_browser | `python3 tests/list_delta_browser.py` | Node and Hub with 1,200 synthetic sessions and 200 subagents: measured field/child deltas, exact deletion/order, unchanged terminal responses, revision/debug-view isolation, upstream delta transfer, restart recovery, and stale-build/login polling shutdown while preserving draft saves. | binary, hub, Chromium |
| idle_requests_browser | `python3 tests/idle_requests_browser.py` | Real node and Hub with a selected synthetic session: a 15 s idle window stays within a per-path request budget (no `api/groups` or fast transfer-task polling); a group created by another client arrives through the UI event channel; a private fake Grok composer backs off unchanged input CHECKs (at most 5 per window) and returns to fast checks on typing and screen change. `--report-only` prints counts without asserting. | binary, hub, ptyhost, Chromium |
| page_sleep_browser | `python3 tests/page_sleep_browser.py` | Real browser clock and private fake CLI: default one-hour inactivity, user activity resets, Features duration persistence/disable, full-page pause shade above dialogs, quiet HTTP/SSE/WebSocket, explicit Resume with draft/host preservation and fresh lists, suspended-clock expiry, and stale-build pause retained. | binary, ptyhost, Chromium |
| history_collector_browser | `python3 tests/history_collector_browser.py --binary target/release/sessiondock` | Chromium cold history clicks while three liveness reads consume a synthetic 100,000-connection collector report; latency and peak RSS budgets, complete coordinator evidence retained. `--report-only` records the old baseline. | binary, Chromium, Linux /proc |
| process_links_browser | `python3 tests/process_links_browser.py --binary target/release/sessiondock` | Two isolated nodes, Hub and Chromium: direct SSH attribution, automatic remote nesting, Chromium detach/reattach retained across scans, resumed session ownership, mux rejection, restart persistence and PID reuse | binary, Chromium |
| process_links_sync_browser | `python3 tests/process_links_sync_browser.py --binary target/release/sessiondock` | Chromium navigation with real Hub/node and 20,000 retained connections: delta bytes, updates/removals, collector outages, old nodes, invalid baselines, service/boot restart recovery | binary, Chromium |
| ssh_events_browser | `python3 tests/ssh_events_browser.py --binary target/release/sessiondock` | Durable SSH connect/close evidence, late descendants, restart, PID/tuple reuse and ambiguity through Hub and Chromium; optional privileged `--kernel --ssh-user USER` uses private loopback OpenSSH and real BPF for IPv4/IPv6 and ControlMaster exclusion | binary, Chromium; kernel mode also root, bpftrace, sshd and a non-root fixture user |
| content_scroll_browser | `python3 tests/content_scroll_browser.py` | Information/resource real wheel scrolling, fixed header, overflow, themes/zoom, retry and persisted visibility/scroll/modal diagnostics | binary, Chromium |
| session_resources_browser | `python3 tests/session_resources_browser.py` | Resource drawer clicks, scope, refresh, unavailable machines, mobile layout and errors using private fixtures | binary, Chromium |
| session_research_browser | `python3 tests/session_research_browser.py` | Session detail research: node/Hub identity, experiments and nested steps, direct/context reports, disclosures, deep links, failures/retry and light/dark wide/mobile layouts. Optional `--labdesk-dir CHECKOUT` uses the actual Labdesk API with a private SQLite ledger. | binary, hub, Chromium; optional Labdesk dependencies |
| resource_probe_browser | `python3 tests/resource_probe_browser.py --binary target/release/sessiondock` | Real Hub/node diagnostic controls with private fake collectors and Chromium | binary, Chromium |
| release_version_browser | `python3 tests/release_version_browser.py --binary target/release/sessiondock` | Private Linux release staging, Chromium session navigation and asset caching, date versions, rollback, repeated releases and development mtimes. | binary, Chromium, rsync |
| codex_exec_nest_browser | `python3 tests/codex_exec_nest_browser.py --binary target/release/sessiondock` | Six detached Codex exec children; background discovery, browser open/detach/attach, resume isolation and restart/exit persistence | binary, Chromium |
| terminal_alt_browser | `python3 tests/terminal_alt_browser.py` | Mobile Alt emits physical-key PTY bytes through the grid console. | debug server, ptyhost, Chromium |
| terminal_shift_browser | `python3 tests/terminal_shift_browser.py` | Touch Shift+Left opens a fake CLI question; exact PTY bytes for arrows, pages, Tab and Ctrl/Alt combinations, with latch reset. | debug server, ptyhost, Chromium |
| terminal_keyboard_browser | `python3 tests/terminal_keyboard_browser.py` | Keyboard inset keeps prompts and bottom editors visible without SIGWINCH. | debug server, ptyhost, Chromium |
| terminal_browser | `python3 tests/terminal_browser.py` | Temporary POSIX shell: bytes, resize, takeover, and PTY survival across Web restart. | debug server, ptyhost, Chromium |
| terminal_claim_browser | `python3 tests/terminal_claim_browser.py` | Real console clicks and shell input/output after a 6 s request/response delay; desktop and mobile grid, full claim timeout without automatic retry, and explicit recovery. | debug server, ptyhost, Chromium |
| terminal_diagnostics_browser | `python3 tests/terminal_diagnostics_browser.py` | Claim, header, body, and first grid-paint receipts on an isolated shell with delayed HTTP. | Chromium, ptyhost |
| terminal_rtt_browser | `python3 tests/terminal_rtt_browser.py --binary target/release/sessiondock` | Real node and authenticated Hub console clicks and sequence-numbered raw-shell echoes: individual/grouped keyboard input, exact delivery and grid drawing, no page errors or native fixture changes; reports browser-side round-trip phases without timing thresholds. Private remote-browser fixtures are documented in [diagnostics](diagnostics.md#reproducible-keyboard-round-trips). | Chromium, ptyhost, hub |
| terminal_exit_browser | `python3 tests/terminal_exit_browser.py` | Xterm and grid: complete vs incomplete exit, retained tail and visible reason, no automatic reclaim, gray button, manual replacement | debug server, ptyhost, Chromium |
| terminal_selection_browser | `python3 tests/terminal_selection_browser.py` | Main-console grid mouse selection and copy with and without CLI mouse capture, in isolated PTYs. | Chromium, ptyhost |
| terminal_final_screen_browser | `python3 tests/terminal_final_screen_browser.py` | An ended SSH session keeps its final screen: the watching page switches in place (history, last line, exit code, read-only, no timeline or socket), the exited row lists `final_screen` with `screens/` on disk and no `records/`, and a fresh page opens it on node and through the Hub, fitted to a narrow window. | binary, hub, ptyhost, Chromium |
| terminal_grid_render_browser | `python3 tests/terminal_grid_render_browser.py [--base URL]` | Grid renderer pixels in Chromium without a server: a row of tall glyphs leaves nothing in the neighbouring rows, cell height on whole device pixels at dpr 1/1.25/1.5, no seam in a multi-row background; `--base` checks a deployed instance's assets | Chromium |
| terminal_grid_browser | `python3 tests/terminal_grid_browser.py` | Main-console grid: the console button claims and attaches `mode=grid`, typing, `stty size` equals the grid view before and after a viewport resize, Ctrl+V clipboard text paste, takeover by a second page after confirmation with the first page notified, no page errors | debug server, ptyhost, Chromium |
| terminal_grid_theme_browser | `python3 tests/terminal_grid_theme_browser.py` | Main console theme switching with real PTY colors; OSC color queries stay dark-only. | Chromium, ptyhost |
| terminal_input_browser | `python3 tests/terminal_input_browser.py` | Legacy console raw HTTP input under the Rust `terminal_input` capability; console file paste off (hint) and on from 设置 › 功能 (clipboard image and two-file batch published to `sessiondock_attachments`, paths typed; six-file / 51 MB confirm dismissed then accepted). | Chromium, ptyhost |
| tool_group_fold_browser | `python3 tests/tool_group_fold_browser.py --binary target/release/sessiondock` | Real clicks and native fixture appends: streaming/final/idle sealing, user-opened group identity, result pairing once, refold and lazy result media. | binary, Chromium |
| rich_tools_browser | `python3 tests/rich_tools_browser.py --binary target/release/sessiondock` | Rich tool cards/split view in Chromium; oversize warning; recorded commands never executed | binary, Chromium |
| trash_browser | `python3 tests/trash_browser.py --binary target/release/sessiondock` | Real legacy page against the Rust session recycle bin. | binary, Chromium |
| parent_trash_browser | `python3 tests/parent_trash_browser.py` | Deferred parent hide/restore/restart, inherited history, last-dependent collection, descendant warning/cancel/scope/liveness and Hub clear-all; immutable rollout generations and unrelated sessions preserved | binaries, Chromium |

`python3 tests/codex_sdk_model_queue_browser.py` covers immediate Codex
model/effort/mode/tier/permission changes without SEND, delayed native confirmation,
rejection and queue/steer without stale overrides on node/Hub, desktop/mobile.
`python3 tests/native_sdk_selection_browser.py` covers Grok/Agy/OpenCode/Claude
immediate model/effort/mode/permission/Fast/agent changes and delayed confirmation,
including rejection, preserved drafts and no stale send overrides, desktop/mobile.
Both use private synthetic peers; `--screenshots DIR` records full pages.

`python3 tests/native_sdk_receipt_browser.py` covers four native SDKs sending
input, compact accepted receipts without duplicate text, failure recovery and
cancellation, on desktop/mobile in dark/light themes. `--screenshots DIR` records full pages.

Shared helpers imported by browser suites live in `tests/*_fixtures.py`
(extracted on 2026-10-06 from the removed HTTP/parity/real scripts); the runner
skips them.

`python3 tests/bug_report_shutdown_browser.py` uses Chromium and a private fake
Claude worker. It submits through the report dialog, sends SIGTERM while the
node captures diagnostics, then opens the retained worker after restart and
checks the preserved prompt, idempotent report replay and one bundle/launch.

## Adding a suite

`frontend_search_state_browser` 使用合成会话和隔离服务，通过桌面及手机实际输入、
点击和重载验证搜索的单一状态：本地筛选/全文结果切换、各选项请求与持久化、Esc
取消后迟到 NDJSON 响应不恢复结果，以及桌面已展开正文的 DOM 和滚动身份保持。
运行 `python3 tests/frontend_search_state_browser.py --binary target/release/sessiondock`。

Frontend performance regressions also run as ordinary browser suites:

- `conversation_performance_browser`: click to expand a lazy tool group,
  append synthetic native history, cancel a backtracking regex and search again.
- `render_assets_browser`: open ordinary/formula/code sessions, verify optional
  libraries load on demand, expand 16,000 lines of code through worker highlighting,
  and collapse it without accepting stale worker output.
- `sidebar_select_scroll_browser` additionally checks unchanged DOM identity,
  incremental membership, collapsed groups and overlapping list polls.
- `terminal_selection_browser` additionally checks a 5,000-line synthetic PTY
  history, cancellable lookup, bounded derived cell caching, resizing without
  losing the oldest history, and recovery from an optional renderer load failure.

`tests/run_validation.py` discovers suites after the fixed Rust checks:

1. Every `tests/*.py` with `if __name__ == "__main__"` and no
   `# run_validation: skip` marker becomes a suite, except `run_validation.py`.
   Shared helpers (`*_fixtures.py`, fake CLIs, `hub_fake_node.py`) carry the
   skip marker or have no main.
2. If argparse text contains `--browser`, `--browser` is appended.
3. If argparse text contains `--binary`, `--binary target/release/sessiondock`
   is appended (override with the runner's `--binary`).
4. `lifecycle_browser.py` is also run as `lifecycle_browser_native_binding` with
   `--native-binding`.
5. A suite named `*_real` or `*_real_browser` runs a real CLI and is left out
   unless `--include-real` or `--real-only` is given.

Name new suites `*_browser.py`. Print `PASS …` lines on success. Exit
non-zero on failure. Copy fixtures into temporary directories only; never write
into the repo, native homes, or production paths.


附属关系初始化、显式解除与整组操作的回归范围见
[nesting-regression-audit.md](nesting-regression-audit.md)。

## Functional review entry

`python3 tests/run_functional_review.py --binary target/release/sessiondock
--output target/functional-review` runs the reviewed browser portfolio. Use
`--only suite_browser,...` to select scenarios and `--jobs 1` for timing runs.
It uses the invoking Python with Playwright, repository fixtures and
[`functional_operations.json`](../tests/functional_operations.json), with no
external inventory directory dependency. The catalog lists expected inputs and
results for all current operation IDs; it is not a claim of execution. The output
contains current suite status, per-operation scenario evidence and explicit
remaining scope. A passed shared scenario does not imply every state combination
passed. Run Rust build/format and document checks separately.

The runner invalidates its previous result before launching browsers and exits
nonzero on failure. Diagnostic suites use `browser_evidence_fixtures` to replace
old results with `running`, then `passed` or `failed`, retaining the exception in
`failure.txt`. `SESSIONDOCK_TEST_EXPECT_BROWSER_FAILURE=1` makes
`audit_delivery_browser` assert absent rendered text after a real session click;
this is a browser-driven negative check of runner exit/evidence behavior, not
an accepted product pass.

`tests/audit_delivery_browser.py` drives navigation during HTTP, network,
timeout and beacon refusal faults, then checks persisted cumulative receipts.
`tests/runtime_log_fault_browser.py` drives navigation under normal, failing
and blocked runtime sinks and verifies shutdown and flush independently.

`tests/audit_shutdown_browser.py` types into a private shell, blocks only its
audit file sink, stops the node within a deadline, reconnects through the real
console after restart and independently checks both commands in the shell ledger.

`tests/startup_actions_browser.py` holds the later terminal script, clicks a
native session and checks the fixture answer before releasing that script; a
second session click verifies normal initialization without missing helpers.

`tests/sidebar_input_browser.py` reaches first/last sidebar rows with wheel
input, drags the divider by mouse and touch, checks reload persistence, cancels
a touch drag, scrolls by touch and expands the sidebar with Tab/Enter.

### Remaining UI operation scenarios

The `remaining_ui_*_browser.py` suites are discovered automatically. Each uses
private fixtures and writes a failure-safe `result.json` with explicit operation
IDs and observed results; an ID does not imply untested state combinations.

- [Composer](../tests/remaining_ui_composer_browser.py),
  [composer state](../tests/remaining_ui_composer_state_browser.py),
  [restart](../tests/remaining_ui_restart_browser.py) and
  [model pickers](../tests/remaining_ui_models_browser.py): native clipboard/file
  chooser/drop, durable drafts, quotes, unsaved refresh, fake CLI questions,
  failed-launch restart and local report receipts.
- [Sidebar](../tests/remaining_ui_sidebar_browser.py),
  [shell](../tests/remaining_ui_shell_browser.py),
  [relations](../tests/remaining_ui_relations_browser.py) and
  [lifecycle](../tests/remaining_ui_lifecycle_browser.py): keyboard/touch menus,
  settings, node scopes, parent visibility, ambiguous links and deletion.
- [Conversation](../tests/remaining_ui_conversation_browser.py),
  [history recovery](../tests/remaining_ui_history_browser.py) and
  [media](../tests/remaining_ui_media_browser.py): real scroll endpoints,
  tool/diff/search controls, cancellation before the first history page returns,
  stale cursors, offline recovery, media continuation and download retry.
- [Terminal inputs](../tests/remaining_ui_terminal_browser.py),
  [terminal state](../tests/remaining_ui_terminal_state_browser.py) and
  [exit/cleanup](../tests/remaining_ui_exit_browser.py): independent fake CLI byte
  receipts, native paste, selection, wheel protocols, lease refusal/acceptance,
  persistent split layout, final-screen scrolling and exact private process stop.
- [Transfer](../tests/remaining_ui_transfer_browser.py) and
  [pending tasks](../tests/remaining_ui_tasks_browser.py): real two-node plans,
  lost replies, retry identity, task recovery and withdrawal, checked against
  private journals and unchanged native source bytes.

Injected HTTP errors establish frontend failure handling only. OS IME candidate
windows, native application destinations and browser installation prompts are
not claimed as automated coverage. The unknown-input notice currently contains
text only; its terminal fallback is the existing title-bar action, not a separate
notice button. These suites do not replace runtime retention/performance suites.

Related independently maintained suites are [content input](../tests/content_input_browser.py)
for embedded-view native touch/keyboard/scrollbars, and
[runtime layers](../tests/runtime_layers_browser.py) for browser-driven private
CLI survival, runtime retention and sink faults.
[Runtime maintenance](../tests/runtime_final_review_browser.py) covers private
multi-host maintenance, abandoned scratch recovery and independent process
lifetimes, including service restart and cleanup after a host exits.

[HTTP contexts](../tests/http_context_browser.py) uses a private hostname mapped
to loopback to exercise a genuinely insecure HTTP origin, alongside secure
loopback. It creates a session and deletes/restores a private session tree at
wide and narrow widths, checking durable receipts and browser/server log
correlation. The insecure case asserts that `crypto.randomUUID` is unavailable;
ordinary localhost tests alone do not cover that environment.

[Input hit regression](../tests/input_hit_browser.py) checks named peer controls,
real pointer hits before editing, wheel reachability of long error states,
wide/narrow viewports and interface scaling. Relation saves are checked against
private metadata and reload; failed launches retry into a private fake CLI;
settings and filtering retain their business results. The long-error case also
regresses the new-session dialog missing from scale-aware sizing.
Explicit helpers in
[input hit fixtures](../tests/input_hit_fixtures.py) also guard existing group,
relation, model, continuation, question, search and settings inputs without
patching Playwright APIs or treating native selectors/popovers as peer overlap.

Agy 原生子代理归属：`python3 tests/agy_nesting_browser.py --binary target/debug/sessiondock`，
私有原生目录与 Chromium 验证父子链、历史增量、父关系变更、刷新重启及宽窄明暗画面。
