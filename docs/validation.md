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
suites only: on 2026-10-06 the unit tests (Rust `#[test]` modules,
`crates/*/tests`, Node `*_contract.mjs`, Python `unittest`), the HTTP-only
`*_suite.py` scripts, the Python-oracle `*_parity.py` tools and the benchmarks
and probes were removed. `--only NAME[,NAME…]` and `--skip`
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

## When to run which tier

The full sweep covers many independent surfaces and is not a per-edit gate.
Historical timings below describe their dated runs, not today's full-sweep duration.

**Headless browser is the default gate for a feature or bug fix.** After
the change, run the `*_browser.py` suite that covers the affected path end to
end in Chromium. If no suite covers it, add or extend one. Docs-only and
deploy-script-only work run the doc checks (`check_docs_links.py`,
`check_agents_md.py`).

**The repository has no unit tests or non-browser test suites; do not add any.** A 2026-10-06 audit
found every failing unit test — 15 Node contract, 21 `cargo test` and one Python
unittest failure — was a test not updated after intentional changes (none was a
product bug), while the regressions that mattered were caught by browser
suites. All Rust `#[test]` modules and `crates/*/tests/*.rs`, the
`tests/*_contract.mjs` files and the Python `unittest` suites were then removed.
The same day the HTTP-only suites, oracle parity tools, benchmarks and probes
followed, so that only browser suites remain. Cover new behavior by adding or
extending a browser suite.

The frontend `legacy-web/` is served as committed and needs no build.
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
`cleanup_hosts(root)` for existing explicit teardown paths. Otherwise a
leaked fake CLI can remain attributed to the initiating session through SSH and
keep its activity dot breathing after the turn ends (BUG-20261004-070702-c1ac6b).

- **Small change / one bug fix** — run the affected headless browser suite
  (`--only <name>`); a full sweep is not required for every edit. Once the change
  is complete and validated, commit, push and deploy the current workspace to
  the fleet under `AGENTS.md`, without asking for confirmation again.
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

Rules that never change: synthetic fixtures copied into temporary directories;
loopback listeners only; suites that drive a real CLI as the system under test
always use the cheapest model at low effort (`claude-haiku-4-5-20251001`, `gpt-5.6-luna`, `grok-4.6`;
see AGENTS.md) and skip only when the CLI is absent; no production data, native
homes, or live sessions. Test isolation does not isolate source or deployment.
Linux only. The MSVC `cargo check` is a cross-compile, not Windows runtime coverage.

Windows 实机通过 OpenSSH 构建时，不得调用 `%USERPROFILE%\.cargo\bin` 中的 rustup
shim；真实工具链选择、错误 448 的处理及构建后滚动更新见
[Windows 节点原生构建与滚动部署](deploy-windows.md)。

Typical time is the wall-clock measured on 2026-09-12 during the batch-19 and
batch-20 acceptance runs on the development machine (release binary, warm page
cache); suites that were not part of those runs are `n/a`.

## Suites

The table lists the suites `--list` reports plus the opt-in `*_real` browser suites; rows for the removed non-browser suites were dropped on 2026-10-06.

| Suite | Command | Covers | Needs | Typical time |
| --- | --- | --- | --- | --- |
| cargo_fmt | `cargo fmt -p sessiondock -p ptyhost-client --check` | rustfmt on those two packages | cargo | n/a |
| cargo_clippy | `cargo clippy -p sessiondock -p ptyhost-client --all-targets --locked -- -D warnings` | clippy, warnings denied | cargo | n/a |
| cargo_check_windows | `cargo xwin check --workspace --all-targets --target x86_64-pc-windows-msvc --locked` | Linux/macOS cross-compile to MSVC with bundled C dependencies; native Windows uses `cargo check`; not a Windows run | cargo, cargo-xwin, LLVM, cached/downloadable CRT/SDK | n/a |
| cargo_build | `cargo build --release -p sessiondock --locked` | Release server used by `--binary` Python suites | cargo | n/a |
| agy_browser | `python3 tests/agy_browser.py --binary target/debug/sessiondock` | Legacy Agy pending launch, effort argv, terminal typing, reconnect, discard, unavailable CLI, 390 px and themes; fake CLI only, not native resume evidence. | binary, ptyhost, Chromium | n/a |
| agy_clients_browser | `python3 tests/agy_clients_browser.py --binary target/debug/sessiondock` | Private fake CLI and manifest: local/Hub model TSV, default and explicit effort argv, version/update success and failure, closed stdin, missing CLI and offline-node gates. | binary, hub, ptyhost, Chromium | n/a |
| agy_history_browser | `python3 tests/agy_history_browser.py --binary target/debug/sessiondock` | Native-schema synthetic catalog/transcript seeds: Chromium list, search, pagination, append, old-row rewrite, rewind, atomic DB replacement, restart, missing/restored transcript with retained history notice, catalog-row removal, image/raw-tool/error rendering, and unsupported deletion/mixed-group transfer refusal; native bytes/mtime preserved. Schema fixtures do not establish real CLI media/tool output. | binary, Chromium | n/a |
| agent_menu_browser | `python3 tests/agent_menu_browser.py --binary target/release/sessiondock` | Subagent title-bar menu (Python `agent_menu_e2e` port): end-time order and no running dot while the owner is not live, running-first with dot / open span from the backend's `agent_items[].active` (sidecar open turn), finish/resume as transcript edits reaching the next open; row geometry under a sticky turn toolbar at desktop + 390 px | binary, Chromium | 6s |
| audit_browser | `python3 tests/audit_browser.py --binary target/release/sessiondock` | Bounded browser-audit intake: receipts when configured, 501 and no posts without | binary, Chromium | n/a |
| autobind_browser | `python3 tests/autobind_browser.py` | Process-evidence autobind of a pending Codex pane: an older rollout the pane holds open (Python `before` set) never binds; the pane's own fresh rollout binds and the page follows it | debug server, ptyhost, proc scan, Chromium | n/a |
| brand_names_check | `python3 tests/brand_names_check.py` | Scans every tracked non-Markdown text file—source, tests, configuration, static assets and frozen reference—with no compatibility whitelist; also verifies the SessionDock manifest, install metadata, that no service worker is registered (and the retired shell caches are cleared), page/runtime titles and `<namespace>files-*` preference keys. | git checkout | <1s |
| bug_report_node_browser | `python3 tests/bug_report_node_browser.py` | Hub report dialog machine selection, named source buttons, two-up attachments on a 390px phone, disabled CLI choices and cross-machine capture/submit bodies against fake nodes; no CLI or real histories | hub binary, Chromium | n/a |
| bug_report_codex_browser_real | `python3 tests/bug_report_codex_browser_real.py` | Operator-only real Codex report dialog, native rename and first task confirmation; isolated home, Luna low, unchanged everyday defaults. | Codex CLI, ptyhost, Chromium | n/a |
| sessions_visibility_browser | `python3 tests/sessions_visibility_browser.py --binary target/release/sessiondock` | Chromium node/Hub list, open, search and SSE with obsolete registry/URL parameters; all data temporary. | binary, hub binary, Playwright | n/a |
| files_browser | `python3 tests/files_browser.py --binary target/release/sessiondock` | Conversation references open FileDock with the exact node/path; missing-reference errors and direct directory entry, desktop/mobile. | binary, Chromium | n/a |
| grok_metadata_browser | `python3 tests/grok_metadata_browser.py --binary target/release/sessiondock` | Grok summary/UID/title/cwd/model, cursor, corruption/recovery over HTTP, desktop/mobile Chromium SSE | binary, Chromium | 7s |
| grok_send_echo_browser | `python3 tests/grok_send_echo_browser.py` | Grok composer SEND on a 390 px page: the queued bubble survives a reload (server CLI state), a timestamp-less `chat_history.jsonl` user line retires it, and an older identical line does not (BUG-20260927-211850-1fa082). | Chromium, ptyhost | n/a |
| header_fold_browser | `python3 tests/header_fold_browser.py --binary target/release/sessiondock` | Header and title-bar fold order over 123 widths (1698 down to 320) plus divider drag (Python `fold_sweep_e2e` port, local mode): chrome labels → hostname → machine-chip abbreviations (no counts) → right-side buttons; suffix/prefix priorities (no branch item), no squeeze, one header height across tiers, monotonic within a tier | binary, Chromium | 26s |
| history_browser | `python3 tests/history_browser.py --binary target/release/sessiondock` | Advanced native history in legacy UI: branches, agents, parent-chain, SSE reset | binary, Chromium | 9s |
| history_pages_browser | `python3 tests/history_pages_browser.py --binary target/release/sessiondock` | Bounded pages, SSE/render races, cancelled preparation preserves selected view/DOM, explicit 404/409/410 recovery | binary, Chromium | 11s |
| history_pages_resume_browser | `python3 tests/history_pages_resume_browser.py --binary target/release/sessiondock` | Lost grants across real server processes; partial pages, live appends, exact subagent scope and changed-checkpoint rejection | binary, Chromium | 4s |
| conversation_performance_browser | `python3 tests/conversation_performance_browser.py` | Header fold/expand preserves process nodes; lazy 180-tool group, live append and yielding regex search. | binary, Chromium | n/a |
| render_assets_browser | `python3 tests/render_assets_browser.py` | Diagrams, math and terminal assets load on demand through the page. | binary, Chromium | n/a |
| hub_cache_browser | `python3 tests/hub_cache_browser.py` | Repeated real page reloads over changing padded node lists: bounded Hub RSS, latest snapshot, unchanged refresh without disk rewrite, and offline cache after restart. | hub binary, Chromium, Linux procfs | n/a |
| hub_bulk_browser | `python3 tests/hub_bulk_browser.py` | Real Hub and node with a slow fake OpenCode remove: pick-bar 附属到… nests several rows under one clicked parent; a bulk delete that takes longer than 5 s removes every row without errors; desktop + 390px. | binary, Chromium, ptyhost | n/a |
| hub_nest_browser | `python3 tests/hub_nest_browser.py` | Two real isolated nodes and Hub: cross-machine manual attach via context menu, same native SID isolation, cycle/auth checks, persistence across node/Hub restart, detach/restore and local reattach. | binary, Chromium | n/a |
| bug_report_upload_browser | `python3 tests/bug_report_upload_browser.py` | Real Hub/node report staging recovers a lost reply without duplicate uploads or launches. | binary, Chromium | n/a |
| code_diagram_browser | `python3 tests/code_diagram_browser.py` | Fenced diagrams preserve terminal column alignment with a CJK browser fixed font. | binary, Chromium | n/a |
| send_native_codex_browser | `python3 tests/send_native_codex_browser.py` | Conversation SEND against a resumed native Codex session with a private fake CLI. | binary, Chromium | n/a |
| session_global_actions_browser | `python3 tests/session_global_actions_browser.py` | Global actions follow the hidden session list with synthetic capabilities; real term-list refresh toggles create availability while docked and keeps the conversation node. | binary, Chromium | n/a |
| opencode_spawn_browser | `python3 tests/opencode_spawn_browser.py` | An `opencode run` started from a Claude tool shell nests under that Claude; subagent child, older session and an ambiguous directory stay roots; synthetic process tree and OpenCode database. | binary, Chromium | n/a |
| opencode_mirror_browser | `python3 tests/opencode_mirror_browser.py --binary target/release/sessiondock` | Chromium: idle database queries skipped, WAL updates, streaming settlement, replacement, restart and deletion. | binary, Chromium | n/a |
| hub_transfer_browser | `python3 tests/hub_transfer_browser.py --binary target/release/sessiondock` | Chromium: terminal journals remain unread after startup, new tasks, leases, cancellation and offline restart recovery. | binary, hub binary, Chromium | n/a |
| opencode_browser | `python3 tests/opencode_browser.py --binary target/release/sessiondock` | Fake OpenCode CLI and SQLite store through list, search, create, send, stop, resume, report and delete in Chromium. | binary, Chromium | n/a |
| shell_env_browser | `python3 tests/shell_env_browser.py` | Real Hub and two nodes behind a synthetic shell wrapper: an rc edit reports changed variable names (no values), the hub and node notices at 390px, ignore, 全部重启 and a single restart exit 75 after graceful shutdown, a fresh start clears each row; removal/reappearance changes card identity, a later build notice follows the shell card, and row/bulk restart failures recover the same buttons without duplicate pending POSTs. | binary, Chromium | n/a |
| spawn_chronology_browser | `python3 tests/spawn_chronology_browser.py` | Legacy inferred parents newer than their Grok child are repaired; valid children, explicit user parents and native activity remain correct across Chromium/SSE and restart. | binary, Chromium | n/a |
| terminal_heartbeat_browser | `python3 tests/terminal_heartbeat_browser.py` | Stalled terminal sockets recover without replaying ambiguous input. | binary, Chromium | n/a |
| terminal_scrollback_browser | `python3 tests/terminal_scrollback_browser.py` | Real wheel input scrolls PTY history locally in both console renderers. | binary, Chromium | n/a |
| terminal_history_paging_browser | `python3 tests/terminal_history_paging_browser.py` | Main console pages host scrollback older than the 2000-row snapshot through `GET /api/term/grid/history`: scrollbar drag, wheel and Home reach row 1 of 5000 with contiguous seams and a stable viewport; live input after paging, no idle requests, reload, width change, no requests in final screen; a `--history 60` host streams 600 rows live past its full history without a reconnect and keeps them through a shorter then taller window. | binary, Chromium | n/a |
| terminal_reflow_browser | `python3 tests/terminal_reflow_browser.py` | ~9000 fully paged history rows with wrapped lines: five width changes measured (synchronous `term.resize` and longest main-thread task, budgets 60/120 ms), rows contiguous, a held selection keeps its text and copies it on release, the top logical line stays on top; rows the host moves between history and screen on wider/narrower/shorter/taller windows appear exactly once, also with the history-tail request failing. | binary, Chromium | n/a |
| hub_draft_recovery_browser | `python3 tests/hub_draft_recovery_browser.py --binary target/release/sessiondock` | Six-node draft discovery with stalled/503 peers: successful peers stay cached, per-node retry/backoff, offline recovery, real session opening and newly registered nodes. | binary, hub, Chromium | n/a |
| hub_browser | `python3 tests/hub_browser.py --binary target/release/sessiondock` | The `sessiondock-hub` page over three `hub_fake_node.py` nodes — machine filter chips, per-machine nesting (same native id never cross-nests), NDJSON search progress + one-machine failure, a session opened through the proxy with media/SSE, settings untick/tick/drag/keyboard reorder/rename, `sessiondock.hub.` prefix; `sessiondock-hub` taken from the `--binary` directory | binary, Chromium | 12s |
| hub_pending_state_browser | `python3 tests/hub_pending_state_browser.py` | New-session receipts remain visible during partial hub lists; recovery and terminal exit update the page. | binary, Chromium | n/a |
| hub_send_browser | `python3 tests/hub_send_browser.py` | Real hub → authenticated Rust node → fake Claude; different builds, desktop/390 px sends, refresh, real hub asset upgrade with draft preservation and stale-page refusal, raw-input/retry/local-spoof auth gates, hub metadata, and a second hub page following draft edits from the pushed `drafts` event, including one written directly on the node | debug binaries, Chromium | 30s |
| legacy_browser | `python3 tests/legacy_browser.py` | Legacy slice on the 16cc89c-resynced frontend: native fixtures, SSE append/partial/reset, errors/retry, console, mobile/dark | debug server, Chromium | 7s |
| lifecycle_browser | `python3 tests/lifecycle_browser.py` | CLI and bare SSH terminal creation, idempotency, pending input, Web restart, mobile cancel | debug server, ptyhost, Chromium | n/a |
| lifecycle_browser_native_binding | `python3 tests/lifecycle_browser.py --native-binding` | API-only operator binding followed by the page, pending/native leases, durable cancel across Web restart | debug server, ptyhost, Chromium | 13s |
| lifecycle_cli_browser | `python3 tests/lifecycle_cli_browser.py` | Fake-CLI argv/session-id on create; resume takeover echoes `resume <sid>`; a configured CLI executable rewritten in place after service start still launches (its new contents); a host binary that predates `--no-record` still creates agent sessions | debug server, ptyhost, Chromium | n/a |
| cli_menus_browser | `python3 tests/cli_menus_browser.py` | Native CLI menu fixtures exercised through Chromium and private fake-CLI PTYs; validates key bytes and semantic outcomes without model requests. | sessiondock binaries, ptyhost, Chromium | n/a |
| client_update_browser | `python3 tests/client_update_browser.py` | AI client matrix in the Machines tab on a real node with fake CLI profiles and a fake `curl` for newest-version lookups, on the node page and through a real `sessiondock-hub` next to a Codex-only fake node: columns only for installed clients, outdated/current marks, an empty cell, a successful update (profile args + `update`, closed stdin, escapes stripped) turns the cell current, a failing update reports its exit status and last line; unknown ID 404, concurrent update 409; 390px without page overflow | debug server, sessiondock-hub, ptyhost, Chromium | n/a |
| live_browser | `python3 tests/live_browser.py` | Three-state `/api/live`: managed running then exited; unrelated stay unknown; obsolete registry/URL cannot hide live or terminal rows | debug server, ptyhost, Chromium | n/a |
| missing_roots_browser | `python3 tests/missing_roots_browser.py --binary target/release/sessiondock` | Missing native roots at startup, removal and recovery of each source; refresh and open surviving sessions in the real UI. | binary, Chromium | n/a |
| managed_terminal_browser | `python3 tests/managed_terminal_browser.py` | Console routed by UID/instance; live Codex rollout rotations and chat/terminal switching on node/Hub without new claims; no new CLI; force/revoke; Web restart | debug server, hub, ptyhost, Chromium | 7s |
| media_browser | `python3 tests/media_browser.py --binary target/release/sessiondock` | Embedded PNG/JPEG HTTP + Chromium decode; tool images; no remote fetch | binary, Chromium | 9s |
| media_continuation_browser | `python3 tests/media_continuation_browser.py --binary target/release/sessiondock` | Multi-image continuation preserves raw message/descriptor and existing DOM identity; live cursor untouched; 409 after rewrite | binary, Chromium | 7s |
| media_files_browser | `python3 tests/media_files_browser.py --binary target/release/sessiondock` | Disk images with configured or default path handling; replacement 409; native bytes unchanged | binary, Chromium | 7s |
| media_formats_browser | `python3 tests/media_formats_browser.py --binary target/release/sessiondock` | GIF/WebP/AVIF/BMP Chromium decode across providers and tool results | binary, Chromium | 14s |
| media_lazy_browser | `python3 tests/media_lazy_browser.py --binary target/release/sessiondock` | Offscreen zero GET; scroll decode; visible diagnostics; concurrent GET completion; inline fold/unfold cleanup and actual retry; 404/409 reload | binary, Chromium | 8s |
| groups_browser | `python3 tests/groups_browser.py --binary target/release/sessiondock` | Inline create/cancel/delete, empty groups, immediate second-level menus with hover/keyboard/touch, cross-node batches, offline deletion/rejoin, same-name recreation and restart persistence. | binary, Chromium | n/a |
| metadata_browser | `python3 tests/metadata_browser.py --binary target/release/sessiondock` | Preferences: cross-tab SSE star/unstar, parent visibility, writer restart | binary, Chromium | 20s |
| codex_names_browser | `python3 tests/codex_names_browser.py --binary target/release/sessiondock` | Codex names: file-order/unicode, fork/agent titles, search, desktop/mobile Chromium SSE | binary, Chromium | 5s |
| native_envelopes | `python3 tests/native_envelopes.py --browser --binary target/release/sessiondock` | Stringified Codex tool images and nested replay; HTTP + Chromium | binary, Chromium | 21s |
| native_spans | `python3 tests/native_spans.py --browser --binary target/release/sessiondock` | Native image spans, MIME/data-URL aliases and first-match field aliases | binary, Chromium | 9s |
| native_tags_browser | `python3 tests/native_tags_browser.py --binary target/release/sessiondock` | Audited native tag families in Chromium; each positive case has a literal or malformed control, and image delimiters are not file-read authority | binary, Chromium | n/a |
| child_modes_browser | `python3 tests/child_modes_browser.py --binary target/release/sessiondock` | Desktop/mobile three-mode clicks, lazy child counts and arrows, one-level expansion, collapse/release, stale-response race, reload reset, polling, flat/tree restoration and open child detail. | binary, Chromium | 5s |
| nest_tree_browser | `python3 tests/nest_tree_browser.py --binary target/release/sessiondock` | Sidebar nesting (Python `nest_tree_e2e` port) on the backend's own fields: legacy `spawned_by` migrated to `nest_parent` in `session-metadata.json`, `active` from the sidecar's open turn, `continued_in` from the tail record; flat list with subagent rows hanging under their owner (caret folds only the agents), tree/ordering/carets aligned with the group caret, `.agent-mark`, active-only filter refuses without a scan, hidden continued-in parent with its continuation listed and followed on open, in-place refresh after on-disk edits; right-click detach/attach-to with cancel; desktop + 390 px | binary, Chromium | 6s |
| pending_create_discard_browser | `python3 tests/pending_create_discard_browser.py --binary target/release/sessiondock` | Headless create-and-discard for Claude, Codex, Grok and SSH: unsent composer input, sidebar menu (unused AI discard; SSH stop then delete), header 删除, reload and a second page must not rebuild the row (Grok writes empty `summary.json` on start). | binary, ptyhost, Chromium | n/a |
| pending_session_link_browser | `python3 tests/pending_session_link_browser.py` | Pending launch links survive refresh, sharing and browser navigation on node and Hub. | binary, hub binary, ptyhost, Chromium | n/a |
| new_session_follow_browser | `python3 tests/new_session_follow_browser.py` | A new Claude session whose startup notice (SessionStart hook message) creates the native history before any input: the launch page switches to the native session while the user types — no loading interstitial, the composer never hides and keeps focus and draft, no extra browser history entry (fake CLI, desktop + 390 px). | Chromium, ptyhost | n/a |
| new_session_model_browser | `python3 tests/new_session_model_browser.py --binary target/release/sessiondock` | Headless new-session model and effort picker with a free fake CLI for Claude, Codex, Grok and OpenCode: each CLI's own catalog (Codex/Grok caches with hidden models skipped, Codex config default, OpenCode `models` with a search box above ten), remembered choice per source, a row that never moves while switching sources, the choice in the launched argv or OpenCode's pre-created session, SSH disabled, phone wrap. | binary, ptyhost, Chromium | n/a |
| popup_browser | `python3 tests/popup_browser.py` | No native alert/confirm: a delete asks in a centered `.app-popup` with the dialog look (取消/Esc keep, 确定 deletes); the stale-build card sits in the centered float stack and 稍后 retains its hidden identity; an HTTP login redirect and actual login button open the local app with no opener; desktop + 390px. Other browser suites answer popups through `tests/popups.py` `on_popup`. | binary, Chromium | n/a |
| pick_drag_browser | `python3 tests/pick_drag_browser.py` | 多选 mode: dragging with the left mouse button from an unpicked row picks the run, from a picked row unpicks it; dragging back restores rows that left the run, the release does not re-toggle the pressed row, no text is selected, holding at the bottom edge auto-scrolls and extends the run; a plain click still toggles one row, a double click selects no text; entering/leaving 多选 keeps the existing row nodes (checkboxes patched in place). | binary, Chromium | n/a |
| prefs_migration_browser | `python3 tests/prefs_migration_browser.py --binary target/release/sessiondock` | Seeds theme/font/layout/filter/cache/unread/selection and file-manager preferences under `sessiondock.*`; verifies they apply, changes persist across reload, `files.html` uses `sessiondock.files-*`, a fresh browser keeps defaults, and PWA identity is SessionDock. | binary, Chromium | 5s |
| frontend_framework_browser | `python3 tests/frontend_framework_browser.py --binary target/release/sessiondock` | Desktop and phone settings operated through Chromium: appearance/features controls, close/reopen, reload persistence, Escape and continued access to Machines. | binary, Chromium | n/a |
| frontend_cutover_browser | `python3 tests/frontend_cutover_browser.py --binary target/release/sessiondock` | Retired worker/cache cleanup, prefixed search/history/settings, HTML metadata injection and script availability for the frontend. | binary, Chromium | n/a |
| frontend_entry_browser | `python3 tests/frontend_entry_browser.py --binary target/release/sessiondock` | Real desktop/phone operation under a loopback `/sessiondock/` proxy: initial loading, late read isolation after a real session switch, manual 503 retry, session selection, theme/font/cache controls, restart of the private Rust fixture, restored conversation and preferences, browser history, and prefix-relative asset/API requests. Runs with either frontend via `SESSIONDOCK_TEST_WEB_DIR`. | binary, Chromium | n/a |
| frontend_responsiveness_browser | `python3 tests/frontend_responsiveness_browser.py --binary target/release/sessiondock` | Large sidebar with 2002 synthetic sessions (windowed; logical rows checked via `group._rows`): clicks and cached navigation update only the selected rows, live paints retain history, search filters and restores the list; decoded catalog updates reuse unchanged rows and agents while changed parent/child titles reach the UI. Reports opening times without machine-specific thresholds. | binary, Chromium | n/a |
| machine_controls_browser | `python3 tests/machine_controls_browser.py` | Hub machine palette, Escape/outside close, unsaved name/caret preservation through polling, stable rows, pending name/enabled requests, failed control rollback and no renderer field; actual controls with synthetic nodes and temporary display failures. | binary, sessiondock-hub, Chromium | n/a |
| server_log_browser | `python3 tests/server_log_browser.py` | Structured server log on a node and the Hub: with `SESSIONDOCK_LOG_REQUESTS=all` the page's requests are one JSON line each (method, path, status, ms), an ApiError keeps its code at level error, the trace id is kept, secret markers in query strings and bodies never reach the log, and the default mode logs only 5xx and slow requests. | binary, hub, Chromium | 15s |
| session_identity_browser | `python3 tests/session_identity_browser.py --binary target/release/sessiondock` | Session identity copying through real menus and clipboard with private fixtures. | binary, Chromium | n/a |
| sidebar_toggle_browser | `python3 tests/sidebar_toggle_browser.py --binary target/release/sessiondock` | Large sidebar (1200 open rows, windowed): resource viewport rendering, row reuse when nesting changes and scroll hydration of the last logical row. | binary, Chromium | n/a |
| sidebar_scale_browser | `python3 tests/sidebar_scale_browser.py --binary target/release/sessiondock` | ~4000 synthetic sessions over 40 open groups with nested children and subagents: prints initial/rerender/live-paint timings, scroll long tasks and DOM size, asserts the visible window stays bounded (≤240 rows, rerender ≤250 ms), then by real input: jump to the bottom and click the last row, deep link to a row far below the window, 多选 drag range auto-scrolled across unrendered rows, group checkbox over unrendered rows, search filter, group and branch fold, date view, text selection kept while scrolling, scroll position across a list update. `--report-only` prints timings only. | binary, Chromium | n/a |
| renderer_fd_browser | `python3 tests/renderer_fd_browser.py --binary target/release/sessiondock` | The real page flushes ~75 audit batches in 30 s while the renderer's fd count is read from /proc (Chromium without sandbox, Linux only); growth must stay under 0.1 fd per batch — an unread fetch response pins a 2 MiB shared-memory pipe until GC and the 1024-fd renderer limit froze the tab in GPU code | binary, Chromium, /proc | 35s |
| prompt_claude_real | `python3 tests/prompt_claude_real.py --browser` | Real Claude question card, cheapest configuration (`claude-haiku-4-5-20251001`, effort low, `--settings` bridge, `--tools AskUserQuestion`): hook file, `prompt` field, SSE `prompt_only`, answer by clicking the card on the real page, cleared once the native answer lands; skips when the Claude CLI or its login is absent. | claude CLI, ptyhost, Chromium | 17s |
| question_browser | `python3 tests/question_browser.py` | Claude question cards answered from a 390 px page against the fake Claude menu (Claude Code 2.1.283 navigation: Up wraps, Down stops on "Chat about this", the text row eats digits and Left/Right): a two-question form from the default and from parked cursors and a single question from the text row each select exactly the clicked options; a fork's `agent_id` PreToolUse shows no card; cards written by the real `claude-hook`. | Chromium, ptyhost | n/a |
| rewind_browser | `python3 tests/rewind_browser.py --binary target/release/sessiondock` | Persisted Claude timeline pins through the real legacy UI (desktop + 390px). | binary, Chromium | n/a |
| rewind_cli_browser | `python3 tests/rewind_cli_browser.py` | A rewind made in Claude's own TUI (fake CLI, no native write) followed as a `cli` pin: rewound input and answer leave the view with the one-line notice, no Esc-return claim, reload keeps it, the next input settles it natively; recalling an answered input still on screen neither pins nor claims an Esc return. | debug binaries, Chromium, ptyhost | 30s |
| search_browser | `python3 tests/search_browser.py --binary target/release/sessiondock` | NDJSON search, flags, navigation, unsupported regex error and recovery | binary, Chromium | 4s |
| search_uuid_browser | `python3 tests/search_uuid_browser.py --binary target/release/sessiondock` | Desktop/mobile full and partial native UUID/UID filtering, Enter search with cold/hot cache, sidecar isolation, unreadable history and actual navigation. | binary, Chromium | n/a |
| search_no_fold_browser | `python3 tests/search_no_fold_browser.py --binary target/release/sessiondock` | Desktop/mobile search starts expanded, supports independent date/project and child folds, preserves them on refresh, resets them for new queries/runs, and restores saved list folds on exit. | binary, Chromium | n/a |
| send_browser | `python3 tests/send_browser.py` | Claude reliable send through the real legacy composer against the fake Claude CLI (desktop + 390 px); busy sends show as queued bubbles from the server CLI state (`check` returns `cli.queued`) until their native echo; a second page's send and composer Esc refused with the owner while the console is held, the 390 px Esc written with no console open; attachments staged on selection, surviving a reload, failed staging with retry, discard of removed staging, publication on SEND; a six-file paste confirm dismissed then accepted, five files unasked. | Chromium, ptyhost | n/a |
| composer_editor_browser | `python3 tests/composer_editor_browser.py --binary target/release/sessiondock` | Desktop and narrow composer: typing, Shift+Enter/mobile Enter, simulated IME key guard, native input-history selection and dismissal, attachment chooser/reference replacing a selection, and textarea growth/cap/shrink. | Chromium, fake CLI, ptyhost | n/a |
| draft_sync_browser | `python3 tests/draft_sync_browser.py` | Two pages on one draft: a second page opens the saved text with the server revision and no error, a refused save rebases and the editing page wins (no revision dead end), an idle page follows both ways, including from the pushed `drafts` event with its input CHECK blocked, an attachment staged on one page is sent from the other, that SEND empties the first page, an image staged on one page previews on the other from the staged bytes, and a console command sent from the composer clears the server draft. | Chromium, ptyhost | n/a |
| send_readiness_browser | `python3 tests/send_readiness_browser.py` | Grok login/unknown SEND refusal, recovery, post-paste recheck, and no auto-retry of an unknown write (fake CLI, desktop + 390 px). | Chromium, ptyhost | n/a |
| send_codex_attachments_browser | `python3 tests/send_codex_attachments_browser.py` | Codex composer sends text plus two PNG attachments while an unrelated transfer stalls, then text plus a `.txt`, against the fake Codex CLI; each batch is published to the session cwd and submitted with Enter. | Chromium, ptyhost | n/a |
| agy_real_browser (operator only) | `python3 tests/agy_real_browser.py --agy <absolute CLI path> --binary target/debug/sessiondock --ptyhost target/debug/ptyhost` | Real Agy 1.2.16 with private HOME/XDG and loopback synthetic gateway; checks models/argv, composer, native binding/history, menu guards and stopped-session `--conversation` continuation. Direct explicit `--agy` invocation only (`run_validation: skip`), outside the default sweep; no paid model or credentials. The separate title generator may use the second synthetic catalog model. Includes complete report-worker prompt submission and native reply. See [Agy](agy.md) for evidence boundaries. | real agy binary, ptyhost, Chromium | explicit invocation only |
| agy_interactions_real_browser (operator only) | `python3 tests/agy_interactions_real_browser.py --agy <absolute CLI path> --binary target/debug/sessiondock --ptyhost target/debug/ptyhost` | Real 1.2.17 private HOME and loopback model: command once/deny/cancel/amend, session/persistent grants and subsequent commands, create file, single/multiple/write-in questions, page navigation; five native slash menus without phantom echo queues. | real agy, ptyhost, Chromium | explicit invocation only |
| agy_tools_real_browser (operator only) | `python3 tests/agy_tools_real_browser.py --agy <absolute CLI path> --binary target/debug/sessiondock` | Real CLI executes two read-only `view_file` calls against temporary text/PNG fixtures through a loopback synthetic gateway. Chromium opens the native history, expands tools and loads the native image; asserts the actual mapped wire model. No credentials or paid model; `run_validation: skip`. | real agy binary, Chromium | explicit invocation only |
| startup_claude_browser | `python3 tests/startup_claude_browser.py` | Fake Claude workspace-trust question before hooks/history, answered through the shared composer cards. | sessiondock binaries, ptyhost, Chromium | n/a |
| startup_question_browser | `python3 tests/startup_question_browser.py` | Fake Codex folder-trust startup question answered through the composer before a rollout exists. | sessiondock binaries, ptyhost, Chromium | n/a |
| send_codex_startup_browser_real | `python3 tests/send_codex_startup_browser_real.py` | Operator-only (`--include-real`): cold-start SEND with real Codex at Luna low, checking native user text and the turn's model/effort. | codex CLI, Chromium | n/a |
| session_tree_delete_browser | `python3 tests/session_tree_delete_browser.py` | Chromium/Hub desktop and mobile: same connected members as copy, preview cancellation, changed-member refusal, hidden ancestors, physical generations, agents, running hidden-agent refusal, mixed Claude/Codex/Grok ownership, one trash entry, monotonic progress, lost-response recovery, cross-filesystem exact restoration of bytes/titles/stars and durable retry after restore/restart. Private synthetic fixtures. | sessiondock binaries, Chromium | n/a |
| session_clone_browser | `python3 tests/session_clone_browser.py` | Real Hub/node UI: transfer table, target/mode/identity selection, mobile layout, native title hints without custom overlays (running action/menu, offline machine, empty agent; hover/touch/keyboard guards and live recovery) and unsupported-action guard; clone stopped complex Codex group, read inherited history and subagents; native metadata, code-mode mapping, unchanged source, repeated confirmation, restart compensation and second-database failure rollback. Synthetic private fixtures. | sessiondock binaries, Chromium | n/a |
| session_bundle_browser | `python3 tests/session_bundle_browser.py` | Real Hub/two-node Chromium copies Codex/Claude/Grok complex groups, opens history/agents, rejects corrupt/truncated/traversal bundles and mismatched roots/cwd, and retries after restart. `--preserve` covers unchanged identities/bytes and reuse; `--move` covers trash, persistent ownership, shared-storage rejection, moving back, interrupted cleanup, new outside references (including Claude SendMessage calls and resolved short destinations during partial cleanup), stale handoff compensation and interrupted withdrawal through the page, reopening tasks after browser reload (including narrow screens), and durable byte/phase progress. Optional `--peer SSH_ALIAS` uses independent remote temporary storage over loopback SSH forwarding. With a peer, `--dependencies` exercises missing and changed typed images, persisted outputs and external bundle symlinks through confirmation, then checks the recorded dependency inventory and successful copy/move without changing those external files. `session_transfer_peer.py` is its private fixture worker. | sessiondock binaries, Chromium; optional SSH peer with shared checkout | n/a |
| session_prefix_browser | `python3 tests/session_prefix_browser.py --peer SSH_ALIAS` | Explicit two-node Chromium test of Codex/Claude/Grok complex groups: preserved-ID prefix copy and move, divergent target rejection, Codex current-rollout conflict, native-row, display-preference and original-byte restoration after simulated receiver crash, and preservation of target continuation or concurrent preference changes during compensation. Private fixtures and remote loopback forwarding. | sessiondock binaries, Chromium, SSH peer with shared checkout | n/a |
| session_transfer_environment_browser | `python3 tests/session_transfer_environment_browser.py` | Chromium fourteen-session mixed family: missing, older or unknown target CLI advice; stale destination response isolation; native database and rollout dynamic-tool names (legacy and namespaces) without leaking definitions; older plans remain explicitly unverified; confirmation still copies the group. CLI replies are synthetic and no models run. | sessiondock binaries, Chromium | n/a |
| session_mixed_clone_browser | `python3 tests/session_mixed_clone_browser.py --binary target/release/sessiondock` | Reject changed staging before publication, replan a connected cross-provider clone after restart, and open native share links without altering sources. | binary, ptyhost, Chromium | n/a |
| session_local_recovery_browser | `python3 tests/session_local_recovery_browser.py` | Chromium local complex-group copies for Codex/Claude/Grok: actual publication failure, task recovery after reload and Hub/node restart, fixed identities and unchanged source; also crashes the Hub during a blocked Codex database publication and recovers the completed node result through the page. | sessiondock binaries, Chromium | n/a |
| session_files_clone_browser | `python3 tests/session_files_clone_browser.py` | Real Hub/node Chromium confirms Claude/Grok complex family copy, opens history/agents, checks unchanged source, failed publication rollback and restart retry preserving continued copies. Exact byte comparison covers layout, CRLF, EOF and escaped text; Claude metadata retains absent message fields. Also resumes a different parent’s agent, remaps a resolved short SendMessage destination and isolates same-named calls in other transcripts. | sessiondock binaries, Chromium | n/a |
| session_grok_checkpoint_browser (operator only) | `python3 tests/session_grok_checkpoint_browser.py` | Native Grok compaction in a private home, Chromium clone with new checkpoint IDs, native load/rewind and continuation recalling compacted context; source and daily configuration unchanged. Explicit Grok 4.6 / low; checks the selected model before the first request. | sessiondock binaries, Chromium, authenticated Grok | explicit invocation only |
| session_files_browser | `python3 tests/session_files_browser.py` | Synthetic Claude sibling branches, sidecar agents, tool results and file-history; Grok parent branches and full directories. Chromium opens byte-preserved and new-ID staging; source remains unchanged. Not native publication acceptance. | sessiondock binaries, Chromium | n/a |
| session_transfer_browser | `python3 tests/session_transfer_browser.py` | Rust offline environment/group/plan/stage entry: cwd existence without scanning project content, explicit dependency contents and stale-source checks; nonce probes distinguish shared storage; synthetic multi-generation Codex files and archived sibling; byte-exact move, clone identities/boundaries, unchanged source, retries/conflicts/stale inputs; Chromium clicks staged histories and remapped subagent menus. Staging stays non-publishable. | sessiondock-transfer binary, Chromium | n/a |
| session_transfer_isolation_browser | `python3 tests/session_transfer_isolation_browser.py` | Chromium switches move/copy without planning requests or execution labels; a stalled preparation does not block a different group, in-flight withdrawal, or leave the source locked. An 8 GiB sparse build output and dangling project link are not read. Uses private synthetic Claude/Grok groups and real Hub/node services. | sessiondock binaries, Chromium | n/a |
| session_transfer_cache_browser | `python3 tests/session_transfer_cache_browser.py` | Chromium clones a 2.4 MB Claude session; inotify verifies unchanged unrelated histories are not reopened during preview/execution, while added/removed reverse fork edges update the group immediately. | sessiondock binaries, Chromium, Linux inotify | n/a |
| session_large_manifest_browser | `python3 tests/session_large_manifest_browser.py --binary target/release/sessiondock` | Chromium copies a group with more than 64 MiB of native before/after images; verifies target history and exact native payload. Optional `--peer SSH_ALIAS` exercises a move across private independent filesystems and source retirement. | binaries, Chromium; optional SSH peer | n/a |
| session_link_lineage_browser | `python3 tests/session_link_lineage_browser.py --binary target/release/sessiondock` | Actual Hub/Chromium copy chains: original wins, nearest then oldest descendant, deleted copies, native/UID/subagent bookmarks, restart persistence, missing versus offline. | binaries, Chromium | n/a |
| session_deep_link_browser | `python3 tests/session_deep_link_browser.py --binary target/release/sessiondock` | `?sid=` opens the root and a nested subagent from synthetic history, without starting a CLI; flat and hierarchical sidebar modes both reveal the agent row (past the owner's fold) without flipping the user's mode preference. | binary, Chromium | n/a |
| session_freeze_browser | `python3 tests/session_freeze_browser.py` | Freeze/resume a verified Linux CLI tree, reload recovery, report snapshot and authenticated Hub/mobile controls. | Chromium, ptyhost | n/a |
| session_stop_browser | `python3 tests/session_stop_browser.py` | First-position start/stop controls with independent delete; exact-session resume from desktop and mobile sidebar, live transitions, graceful/concurrent stop, duplicate request serialization and retained pending shell receipts. Fake CLIs and private hosts. | Chromium, ptyhost | n/a |
| turn_state_browser | `python3 tests/turn_state_browser.py` | Sidebar/header turn dots for resumed Claude and Codex sessions (fake CLI): native working/waiting/idle, live TUI state, background terminals and detached owned commands over a quiet screen; permanent helper isolation, reload and completion. | Chromium, ptyhost | n/a |
| process_activity_browser | `python3 tests/process_activity_browser.py` | Node and Hub Chromium paths: completed turns with attached, code-mode-launched, detached and sleeping command processes; helper/MCP/zombie/unrelated isolation; namespace, reload and completion repaint both dots without native history changes. | binary, Chromium | n/a |
| session_model_browser | `python3 tests/session_model_browser.py` | Chromium opens Claude/Codex titles and verifies latest native models beyond metadata windows, list-only title refresh, automatic observation, partial lines, rewrites, inode replacement and subagent isolation. | binary, Chromium | n/a |
| session_titles_browser | `python3 tests/session_titles_browser.py --binary target/release/sessiondock` | Point title queries, native rename, and no hot discovery, on synthetic data. | binary, Chromium | n/a |
| side_drag_browser | `python3 tests/side_drag_browser.py --binary target/release/sessiondock` | Sidebar divider under real CDP touch drag/cancel at 814x380 and mouse drag/dblclick at desktop; desktop row text selection survives the synthetic click and defers list refresh until selection clears; width stored under `sessiondock.` (Python `side_drag_e2e` port) | binary, Chromium | 6s |
| sidebar_select_scroll_browser | `python3 tests/sidebar_select_scroll_browser.py --binary target/release/sessiondock` | Scroll the date/nest sidebar to the last row and click it: the clicked session opens, membership is unchanged, `#side.scrollTop` and the last row's viewport Y stay put, and `renderSide` is not called | binary, Chromium | n/a |
| sidebar_closed_groups_browser | `python3 tests/sidebar_closed_groups_browser.py --binary target/release/sessiondock` | 5,000 synthetic sessions with 49 of 50 groups closed: hidden updates skip row generation, opening reveals current order, nested counts and closed-group selection stay correct, and date changes move sessions between groups | binary, Chromium | n/a |
| sidebar_path_performance_browser | `python3 tests/sidebar_path_performance_browser.py` | Same-basename directory plans, metadata-only updates, repeated branch/group/filter clicks and cache invalidation. | binary, Chromium | n/a |
| sidebar_unread_browser | `python3 tests/sidebar_unread_browser.py` | Twenty folded background views use one summary request on a node and through Hub; reset/error isolation and normal history opening. | binary, hub, Chromium | n/a |
| ui_events_browser | `python3 tests/ui_events_browser.py` | Real node and Hub: ten seconds idle without list/live/terminal polling, inactive appends use summaries without body/list reads, user selection catches up, membership invalidation survives a failed metadata read, and event-stream disconnect/reconnect. | binary, hub, Chromium | n/a |
| list_delta_browser | `python3 tests/list_delta_browser.py` | Node and Hub with 1,200 synthetic sessions and 200 subagents: measured field/child deltas, exact deletion/order, unchanged terminal responses, revision/debug-view isolation, upstream delta transfer, restart recovery, and stale-build/login polling shutdown while preserving draft saves. | binary, hub, Chromium | n/a |
| idle_requests_browser | `python3 tests/idle_requests_browser.py` | Real node and Hub with a selected synthetic session: a 15 s idle window stays within a per-path request budget (no `api/groups` or fast transfer-task polling); a group created by another client arrives through the UI event channel; a private fake Grok composer backs off unchanged input CHECKs (at most 5 per window) and returns to fast checks on typing and screen change. `--report-only` prints counts without asserting. | binary, hub, ptyhost, Chromium | n/a |
| page_sleep_browser | `python3 tests/page_sleep_browser.py` | Real browser clock and private fake CLI: default one-hour inactivity, user activity resets, Features duration persistence/disable, full-page pause shade above dialogs, quiet HTTP/SSE/WebSocket, explicit Resume with draft/host preservation and fresh lists, suspended-clock expiry, and stale-build pause retained. | binary, ptyhost, Chromium | n/a |
| process_links_browser | `python3 tests/process_links_browser.py --binary target/release/sessiondock` | Two isolated nodes, Hub and Chromium: direct SSH attribution, automatic remote nesting, Chromium detach/reattach retained across scans, resumed session ownership, mux rejection, restart persistence and PID reuse | binary, Chromium | n/a |
| ssh_events_browser | `python3 tests/ssh_events_browser.py --binary target/release/sessiondock` | Durable SSH connect/close evidence, late descendants, restart, PID/tuple reuse and ambiguity through Hub and Chromium; optional privileged `--kernel --ssh-user USER` uses private loopback OpenSSH and real BPF for IPv4/IPv6 and ControlMaster exclusion | binary, Chromium; kernel mode also root, bpftrace, sshd and a non-root fixture user | n/a |
| session_resources_browser | `python3 tests/session_resources_browser.py` | Resource drawer clicks, scope, refresh, unavailable machines, mobile layout and errors using private fixtures | binary, Chromium | n/a |
| resource_probe_browser | `python3 tests/resource_probe_browser.py --binary target/release/sessiondock` | Real Hub/node diagnostic controls with private fake collectors and Chromium | binary, Chromium | n/a |
| codex_exec_nest_browser | `python3 tests/codex_exec_nest_browser.py --binary target/release/sessiondock` | Six detached Codex exec children; background discovery, browser open/detach/attach, resume isolation and restart/exit persistence | binary, Chromium | 10s |
| terminal_alt_browser | `python3 tests/terminal_alt_browser.py` | Mobile Alt emits physical-key PTY bytes through the grid console. | debug server, ptyhost, Chromium | n/a |
| terminal_keyboard_browser | `python3 tests/terminal_keyboard_browser.py` | Keyboard inset keeps prompts and bottom editors visible without SIGWINCH. | debug server, ptyhost, Chromium | n/a |
| terminal_browser | `python3 tests/terminal_browser.py` | Temporary POSIX shell: bytes, resize, takeover, and PTY survival across Web restart. | debug server, ptyhost, Chromium | 4s |
| terminal_claim_browser | `python3 tests/terminal_claim_browser.py` | Real console clicks and shell input/output after a 6 s request/response delay; desktop and mobile grid, full claim timeout without automatic retry, and explicit recovery. | debug server, ptyhost, Chromium | 40s |
| terminal_diagnostics_browser | `python3 tests/terminal_diagnostics_browser.py` | Claim, header, body, and first grid-paint receipts on an isolated shell with delayed HTTP. | Chromium, ptyhost | n/a |
| terminal_exit_browser | `python3 tests/terminal_exit_browser.py` | Xterm and grid: complete vs incomplete exit, retained tail and visible reason, no automatic reclaim, gray button, manual replacement | debug server, ptyhost, Chromium | 14s |
| terminal_selection_browser | `python3 tests/terminal_selection_browser.py` | Main-console grid mouse selection and copy with and without CLI mouse capture, in isolated PTYs. | Chromium, ptyhost | n/a |
| terminal_final_screen_browser | `python3 tests/terminal_final_screen_browser.py` | An ended SSH session keeps its final screen: the watching page switches in place (history, last line, exit code, read-only, no timeline or socket), the exited row lists `final_screen` with `screens/` on disk and no `records/`, and a fresh page opens it on node and through the Hub, fitted to a narrow window. | binary, hub, ptyhost, Chromium | 20s |
| terminal_grid_render_browser | `python3 tests/terminal_grid_render_browser.py [--base URL]` | Grid renderer pixels in Chromium without a server: a row of tall glyphs leaves nothing in the neighbouring rows, cell height on whole device pixels at dpr 1/1.25/1.5, no seam in a multi-row background; `--base` checks a deployed instance's assets | Chromium | 5s |
| terminal_grid_browser | `python3 tests/terminal_grid_browser.py` | Main-console grid: the console button claims and attaches `mode=grid`, typing, `stty size` equals the grid view before and after a viewport resize, Ctrl+V clipboard text paste, takeover by a second page after confirmation with the first page notified, no page errors | debug server, ptyhost, Chromium | 15s |
| terminal_grid_theme_browser | `python3 tests/terminal_grid_theme_browser.py` | Main console theme switching with real PTY colors; OSC color queries stay dark-only. | Chromium, ptyhost | n/a |
| terminal_input_browser | `python3 tests/terminal_input_browser.py` | Legacy console raw HTTP input under the Rust `terminal_input` capability; console file paste off (hint) and on from 设置 › 功能 (clipboard image and two-file batch published to `sessiondock_attachments`, paths typed; six-file / 51 MB confirm dismissed then accepted). | Chromium, ptyhost | n/a |
| tool_group_fold_browser | `python3 tests/tool_group_fold_browser.py --binary target/release/sessiondock` | Real clicks and native fixture appends: streaming/final/idle sealing, user-opened group identity, result pairing once, refold and lazy result media. | binary, Chromium | 8s |
| rich_tools_browser | `python3 tests/rich_tools_browser.py --binary target/release/sessiondock` | Rich tool cards/split view in Chromium; oversize warning; recorded commands never executed | binary, Chromium | 10s |
| trash_browser | `python3 tests/trash_browser.py --binary target/release/sessiondock` | Real legacy page against the Rust session recycle bin. | binary, Chromium | n/a |

Shared helpers imported by browser suites live in `tests/*_fixtures.py`
(extracted on 2026-10-06 from the removed HTTP/parity/real scripts); the runner
skips them.

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

Name new suites `*_browser.py`. Print `PASS …` lines on success. Exit
non-zero on failure. Copy fixtures into temporary directories only; never write
into the repo, native homes, or production paths.


本次单一附属关系改动的回归清单及对应浏览器路径见
[nesting-regression-audit.md](nesting-regression-audit.md)。
