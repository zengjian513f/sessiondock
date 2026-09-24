# Validation suites

One table of every default check in `python3 tests/run_validation.py`, plus the
opt-in Python benchmarks the runner skips. Narrative rules stay in `AGENTS.md`.

```sh
python3 tests/run_validation.py            # --jobs 8, --browser-jobs 3 by default
python3 tests/run_validation.py --jobs 1   # fully serial (browser jobs forced to 1)
python3 tests/run_validation.py --list
python3 tests/run_validation.py --tags rust,node
python3 tests/run_validation.py --only node_contracts,legacy_browser
```

`--tags` defaults to `rust,node,python`. `--only NAME[,NAME…]` and `--skip`
filter by suite name. `--keep-going` continues after a failure. Per-suite logs
go under `target/validation/<stamp>/`. Rust runs first as parallel lanes that
share no cargo build directory (`cargo_clippy` with optional `cargo_test`;
`cargo_build`; `cargo_check_windows`; `cargo_fmt`), then Node and Python (`tests/*.py` with
`if __name__ == "__main__"`, sorted) — every suite already owns its loopback
port and temp directories. Non-browser suites run through a pool of `--jobs`
workers; suites that drive Chromium (any that import `playwright`) run through a
separate `--browser-jobs` pool (default 3) at the same time, since more than a
few concurrent Chromiums make click/visibility waits flake without the suite
being wrong. A suite whose first 40 lines carry `# run_validation: serial` runs
alone after both pools (timing assertions). Summary and JSON keep the plan
order; `--jobs 1` runs everything serially in that order.

## When to run which tier

The full sweep is ~5 min even in parallel, so it is not a per-edit gate.

**Headless browser is the default gate for a feature or bug fix.** After
the change, run the `*_browser.py` (or `--browser` parity) that covers the
affected path end to end in Chromium. If no suite covers it, add or extend
one. HTTP / `--test` / node suites may run alongside; they are not a
substitute. Docs-only and deploy-script-only work use the doc/deploy suites.

**Never run unit tests on your own**; validate the changed surface with the
headless browser suite that covers it.

- **Small change / one bug fix** — run the affected headless browser suite
  (`--only <name>`). Do not reflexively full-sweep, push, and deploy-to-all
  after every small fix — the cost adds up; batch the full sweep, push, and
  deploy for when the work is ready to land.
- **Paid CLI checks** — the `*_real` suites spawn real Claude/Codex/Grok and are
  excluded by default; run them deliberately with `--include-real`, per batch,
  not unattended.

## Prerequisites

- `cargo` on `PATH`; the runner prepends `~/.cargo/bin` when needed
- Release `target/release/sessiondock` for Python suites that take `--binary`
  (the runner's `cargo_build` suite produces it first)
- Built `target/debug/ptyhost` for terminal, lifecycle, and host suites
- Python with `playwright` installed; child suites use the runner's interpreter
- Playwright Chromium installed under `~/.cache/ms-playwright/chromium-*` via
  `PLAYWRIGHT_CHROMIUM_EXECUTABLE` (unset: highest `chromium-*` under that cache)
- One sibling Python oracle checkout containing an adapter package, or an
  explicit `SESSIONDOCK_PYTHON_SOURCE`/`--python-source PATH`; when none can be
  resolved, oracle-dependent suites are skipped instead of failing

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

The table lists the suites `--list` reports (plus the opt-in benchmarks and the operator-run real-roots bench); it was regenerated on 2026-09-12 and the batch-40 H4 rows (`hub_http_suite`, `hub_browser`) were appended.

| Suite | Command | Covers | Needs | Typical time |
| --- | --- | --- | --- | --- |
| cargo_test | `cargo test --workspace --locked` | Workspace unit and integration tests. Opt-in only (`--include-unit` / `--only cargo_test`). Opt-in ignored `launch_host` needs `SESSIONDOCK_TEST_PTYHOST_BINARY` | cargo | n/a |
| cargo_fmt | `cargo fmt -p sessiondock -p ptyhost-client --check` | rustfmt on those two packages | cargo | n/a |
| cargo_clippy | `cargo clippy -p sessiondock -p ptyhost-client --all-targets --locked -- -D warnings` | clippy, warnings denied | cargo | n/a |
| cargo_check_windows | `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc --locked` | Linux cross-compile to MSVC; not a Windows run | cargo | n/a |
| cargo_build | `cargo build --release -p sessiondock --locked` | Release server used by `--binary` Python suites | cargo | n/a |
| node_contracts | `node --test tests/composer_input_contract.mjs tests/grid_facade_contract.mjs tests/grid_input_contract.mjs tests/grid_model_contract.mjs tests/grid_render_contract.mjs tests/history_pages_contract.mjs tests/legacy_contract.mjs tests/legacy_pure_contract.mjs tests/media_continuation_contract.mjs tests/media_lazy_contract.mjs` | `composer_input_contract.mjs` (PTY editor classification: ready/starting/blocked/unknown over the frame fixtures); `grid_facade_contract.mjs` (xterm-compatible `GridTerm` surface: write of JSON lines, buffer shim, selection, modes, resize/title events); `grid_model_contract.mjs` (grid wire decoder, snapshot/diff application, scrollback reflow, selection text); `grid_render_contract.mjs` (every row clipped to its own box, cell size on whole device pixels for fractional dpr); `grid_input_contract.mjs` (xterm-compatible key/mouse/paste/focus encoding); `legacy_contract.mjs` (capabilities, media tokens, terminal identity, SSE retry, batched audit flush via `auditPayload` / beacon gate); `legacy_pure_contract.mjs` (pure helpers incl. the nest tree `nestParentOf`/`nestTree`/`expandRows`/`agentRunning` degrade rules, continued-in hiding/no-nesting, unread badge state classes); `history_pages_contract.mjs` (gap cursor vs live checkpoint); `media_continuation_contract.mjs` (per-message pages); `media_lazy_contract.mjs` (lazy GET/diagnostics) | node | 0s |
| advanced_parity | `python3 tests/advanced_parity.py --python-source PATH --binary target/release/sessiondock` | Compaction/rewind/sidechains, fork-of-fork, rich tools, Grok envelopes, batch-35 real-root shapes (copied `session_meta` forks, rewind past fork point, orphan agents hidden, torn line / missing leaf / cycle as warnings, `claude-missing-parent` now PASS); duplicate-key objects follow Python's last-key-wins behavior | binary, python-source | n/a |
| agent_active_suite | `python3 tests/agent_active_suite.py --binary target/release/sessiondock` | `agent_items[].active/created/updated` for Claude sidecars (end_turn, stop notification, refusal) and Codex subagents (last event_msg), flips on append (grok-4.6 headless draft, reviewed) | binary | n/a |
| agent_menu_browser | `python3 tests/agent_menu_browser.py --binary target/release/sessiondock` | Subagent title-bar menu (Python `agent_menu_e2e` port): end-time order and no running dot while the owner is not live, running-first with dot / open span from the backend's `agent_items[].active` (sidecar open turn), finish/resume as transcript edits reaching the next open; row geometry under a sticky turn toolbar at desktop + 390 px | binary, Chromium | 6s |
| api_smoke | `python3 tests/api_smoke.py --binary target/release/sessiondock` | Five-second loopback HTTP smoke test of implemented read routes. | binary | n/a |
| audit_browser | `python3 tests/audit_browser.py --binary target/release/sessiondock` | Bounded browser-audit intake: receipts when configured, 501 and no posts without | binary, Chromium | n/a |
| audit_events_suite | `python3 tests/audit_events_suite.py --binary target/release/sessiondock` | Browser-audit intake accepts the resynced frontend's event names, 3 MiB batches, top-level page_id; 5 MiB refused; 400 on malformed JSON; 501 + `audit:false` unconfigured (grok-4.6 headless draft, reviewed) | binary | n/a |
| brand_names_check | `python3 tests/brand_names_check.py` | Scans every tracked non-Markdown text file—source, tests, configuration, static assets and frozen reference—with no compatibility whitelist; also verifies the SessionDock manifest, install metadata, that no service worker is registered (and the retired shell caches are cleared), page/runtime titles and `<namespace>files-*` preference keys. | git checkout | <1s |
| budget_boundaries_suite | `python3 tests/budget_boundaries_suite.py --binary target/release/sessiondock` | Large native-input and pagination coverage over HTTP. | binary | n/a |
| bug_report_http_suite | `python3 tests/bug_report_http_suite.py --binary target/release/sessiondock` | `POST /api/bug-report` against the fake Claude and Codex CLIs — 501 unconfigured, 503 source without a CLI, validation, raw attachment upload, 202 shape, bundle files, Claude `submitted` from the synthetic native record, Codex `submitted_unconfirmed`, audit trail | binary, ptyhost | 12s |
| bug_report_node_browser | `python3 tests/bug_report_node_browser.py` | Hub report dialog machine selection, named source buttons, two-up attachments on a 390px phone, disabled CLI choices and cross-machine capture/submit bodies against fake nodes; no CLI or real histories | hub binary, Chromium | n/a |
| bug_report_real | `python3 tests/bug_report_real.py` | Real Claude worker (the suite pins the cheapest test model per the real-CLI rule) with a temporary config; verifies native confirmation and cleanup. | claude CLI, ptyhost | 25s |
| check_config_suite | `python3 tests/check_config_suite.py --binary target/release/sessiondock` | Startup-validation matrix through `sessiondock --check-config` (no server start, no Chromium), including normal filesystem aliases and configuration. | binary | 2s |
| claude_interrupt_parity | `python3 tests/claude_interrupt_parity.py --python-source PATH --binary target/release/sessiondock` | Interrupted-turn corpora (Python test cases, two-level offshoot, deferred abort) vs `ClaudeAdapter.read`: role/text/turn_id/interrupted sequence and final activity equal (grok-4.6 headless draft, reviewed) | binary, python-source | n/a |
| claude_lineage_parity | `python3 tests/claude_lineage_parity.py --python-source PATH --binary target/release/sessiondock` | Claude torn NUL line, parent cycle, missing last-prompt leaf and duplicate-key objects vs the Python adapter `read`; `跳过无效的JSONL 记录 ×1` on the torn row (grok-4.6 headless draft, reviewed) | binary, python-source | n/a |
| claude_prompt_suite | `python3 tests/claude_prompt_suite.py` | HTTP contract: `sessiondock claude-hook` file semantics as a process (0600, waiting/submitted/cancelled/clear), `--write-bridge-settings`, `/api/messages` `prompt`, `/api/watch` `prompt`/`prompt_only` packets, clearing by the native answer, restart with `claude-prompts/` present. | binary | 3s |
| claude_torn_lines_suite | `python3 tests/claude_torn_lines_suite.py --binary target/release/sessiondock` | HTTP contract: torn line skipped with warning, lineage break/cycle/missing-leaf warnings, `supported:true`, cursor covers the torn bytes and incremental reads continue past it (grok-4.6 headless draft, reviewed) | binary | n/a |
| codex_rename_suite | `python3 tests/codex_rename_suite.py --binary target/release/sessiondock` | Synthesized Codex `/rename` command events in list, message-window and cursor responses. | binary | n/a |
| codex_fork_rewind_suite | `python3 tests/codex_fork_rewind_suite.py --binary target/release/sessiondock` | HTTP contract: `forked_from_id` ≠ `history_base.thread_id` — prefix from the physical `history_base` file, list decoration along `forked_from_id`, off-boundary cut still 501 (grok-4.6 headless draft, reviewed) | binary | n/a |
| codex_legacy_fork_suite | `python3 tests/codex_legacy_fork_suite.py --binary target/release/sessiondock` | HTTP contract: legacy self-contained forks with copied `session_meta` records (parent present/absent): `跳过重复的Codex session_meta ×N`, root_sid/fork_depth/created/title/size, own records readable (grok-4.6 headless draft, reviewed) | binary | n/a |
| continued_in_suite | `python3 tests/continued_in_suite.py --binary target/release/sessiondock` | Claude `continued_in` row field from the tail `continued-in` record, resolved within the list only, equal to Python `finalize_sessions`; record produces no message (grok-4.6 headless draft, reviewed) | binary | n/a |
| debug_runs_suite | `python3 tests/debug_runs_suite.py --binary target/release/sessiondock` | Debug-run filtering and reload over temporary histories. | binary | n/a |
| deploy_dry_run | `python3 tests/deploy_dry_run.py` | Offline regression for `deploy/deploy.py`: temporary checkout, `build --web-only`, `push --dry-run` against a private prefix (PLANNED / SKIPPED / UNSUPPORTED rows, plan lines, JSON report, fixture untouched), unknown target and refused rollback dir; tracked workspace archive preserves the real index and excludes untracked files; no ssh, no cargo (grok-4.6 headless draft, reviewed) | git checkout | 2s |
| deploy_lock | `python3 tests/deploy_lock.py` | Temporary-process contention and owner diagnostics, latest state after waiting, timeout, crash release without unlink, shared worktree lock and all four deployment commands locking before target reads | git | 1s |
| deploy_native_handlers | `python3 tests/deploy_native_handlers.py` | Offline command-sequence pins for the `macos-node` and `windows-node` deploy handlers with a recording fake Shell (stage/backup/swap/restart/verify/rollback, rendered `.cmd` templates, the per-platform native test step driven by `DeployOptions.test_mode`: absent for `none`, between extraction and build otherwise, a failing run stops before anything is staged); the Windows path has no real-machine run yet | none | <1s |
| deploy_testplan | `python3 tests/deploy_testplan.py` | Offline pins for the deploy test gate (`deploy/testplan.py`, `deploy.py --test`): changed-path → suite-pattern mapping against a stubbed `--list` plus the alias table against the real `--list`, base commit from `--test-base` / the OLDEST `etc/deployed-commit` marker / origin/main / HEAD~1, `build --test none` skipping, `--test affected` printing the plan and handing `--only`/`--binary`/`--log-dir`/`--json` to a stubbed runner, `test_*` recorded in artifacts.json, exit 1 with failing suite names and log paths before push, unmatched path → full sweep, `push --dry-run` echoing the recorded mode | git checkout | 3s |
| files_browser | `python3 tests/files_browser.py --binary target/release/sessiondock` | Conversation references open FileDock with the exact node/path; missing-reference errors and direct directory entry, desktop/mobile. | binary, Chromium | n/a |
| files_grants_suite | `python3 tests/files_grants_suite.py --binary target/release/sessiondock` | File-browser grants across rename, deletion, restart and session scopes. | binary | n/a |
| files_read_suite | `python3 tests/files_read_suite.py --binary target/release/sessiondock` | HTTP contract of the read-only file service (resolve-files, file, files). | binary | n/a |
| fork_messages_parity | `python3 tests/fork_messages_parity.py --python-source PATH --binary target/release/sessiondock` | `/api/messages` (user/assistant role+text) vs Python `CodexAdapter.read` for legacy forks, rewind-past-fork and plain threads (grok-4.6 headless draft, reviewed) | binary, python-source | n/a |
| fork_rows_parity | `python3 tests/fork_rows_parity.py --python-source PATH --binary target/release/sessiondock` | `/api/sessions` rows vs Python `list_sessions` for legacy forks, orphan/copied subagents and rewind-past-fork (root_sid, fork_depth, size, agent_items; orphans absent on both sides; `updated` is the documented DELTA) (grok-4.6 headless draft, reviewed) | binary, python-source | n/a |
| grants_edge_suite | `python3 tests/grants_edge_suite.py --binary target/release/sessiondock` | Edge behaviour of history-page and media-page grants. | binary | n/a |
| grok_size_suite | `python3 tests/grok_size_suite.py --binary target/release/sessiondock` | Grok directory-size rows and tool output whose text begins with an exit-code phrase. | binary | n/a |
| grok_parity | `python3 tests/grok_parity.py --python-source PATH --browser --binary target/release/sessiondock` | Grok summary/UID/title/cwd/model, cursor, corruption/recovery, desktop/mobile SSE | binary, python-source, Chromium | 7s |
| grok_raw_send_browser | `python3 tests/grok_raw_send_browser.py` | Grok composer text from a 390 px page without a console: `/api/term/send` paste + Enter with an empty token and the pane identity (no claim, no reliable-send call), shell reply visible once the console opens. | Chromium, ptyhost | 5s |
| header_fold_browser | `python3 tests/header_fold_browser.py --binary target/release/sessiondock` | Header and title-bar fold order over 123 widths (1698 down to 320) plus divider drag (Python `fold_sweep_e2e` port, local mode): chrome labels → hostname → machine picker → right-side buttons; suffix/prefix priorities (no branch item), no squeeze, one header height across tiers, monotonic within a tier | binary, Chromium | 26s |
| history_browser | `python3 tests/history_browser.py --binary target/release/sessiondock` | Advanced native history in legacy UI: branches, agents, parent-chain, SSE reset | binary, Chromium | 9s |
| history_pages_browser | `python3 tests/history_pages_browser.py --binary target/release/sessiondock` | Bounded pages, SSE races, stale view discarded, explicit 404/409/410 recovery | binary, Chromium | 11s |
| history_pages_walk | `python3 tests/history_pages_walk.py --binary target/release/sessiondock` | Pagination correctness at scale without Chromium. | binary | n/a |
| history_parity | `python3 tests/history_parity.py --python-source PATH --binary target/release/sessiondock` | Synthetic Claude/Codex history plus adapter differential; current-CLI kinds skipped with counted warnings; batch-35 shapes equal Python `read`/`finalize_sessions` (first `session_meta` is the identity, `跳过重复的Codex session_meta ×N`, null `history_base` forks self-contained, `history_base.thread_id` ≠ `forked_from_id`, orphan agents not rows + typed 501, `跳过无效的JSONL 记录 ×N` + lineage warnings); cyclic agent ownership stays visible 501; scalar `content` is the unreadable fixture | binary, python-source | 0s |
| host_identity | `python3 tests/host_identity.py` | POSIX host-instance guard: wrong instance rejected; guarded attach on the real child | ptyhost | n/a |
| hub_browser | `python3 tests/hub_browser.py --binary target/release/sessiondock` | The `sessiondock-hub` page over three `hub_fake_node.py` nodes — machine filter chips, per-machine nesting (same native id never cross-nests), NDJSON search progress + one-machine failure, a session opened through the proxy with media/SSE, settings untick/tick/drag/keyboard reorder/rename, `sessiondock.hub.<path>.` prefix; `sessiondock-hub` taken from the `--binary` directory | binary, Chromium | 12s |
| hub_console_availability_browser | `python3 tests/hub_console_availability_browser.py` | Console-button availability from each synthetic hub node's resume capabilities. | Chromium | n/a |
| hub_send_browser | `python3 tests/hub_send_browser.py` | Real hub → authenticated Rust node → fake Claude; different builds, desktop/390 px sends, refresh, real hub asset upgrade with draft preservation and stale-page refusal, raw-input/retry/local-spoof auth gates and hub metadata | debug binaries, Chromium | 30s |
| hub_http_suite | `python3 tests/hub_http_suite.py --binary target/release/sessiondock` | Over the wire: `sessiondock-hub --check-config` + fail-closed config, `register`/`list`/`remove` subcommands (server-side, private registry), `/api/meta`, `/api/nodes`, hub-mode page, gate, resolve uniqueness 400, `_build` 409, JSON/SSE rewrite, offline 503 + recovery, chunked upload, display (name/colour/enabled/renderer)/order + audit, explicit node pass-through, bulk writes, browser-audit routing, NDJSON search | binary | 2s |
| hub_namespace_parity | `python3 tests/hub_namespace_parity.py --python-source PATH` | Regenerates the fixed hub namespace corpus from Python `federation.public_payload` and requires the committed fixture to match. | python-source | n/a |
| input_history_suite | `python3 tests/input_history_suite.py --binary target/release/sessiondock` | GET /api/session/input-history contract: shape, oldest-first order, duplicates, hidden protocol/injection, Codex fork prefix, Claude ?agent=, uid 404, limit bu. | binary | n/a |
| inventory_live_append_suite | `python3 tests/inventory_live_append_suite.py --binary target/release/sessiondock` | Lazy index under concurrent appenders: 30 consecutive lists with 0 failures while synthetic CLIs append, rows keep growing, sig changes only with content | binary | 2s |
| inventory_scale_suite | `python3 tests/inventory_scale_suite.py --binary target/release/sessiondock` | Read-model scale targets on a synthetic 1500-session / 1 GiB corpus (two 50 MiB files, the rest spread over 128 files): cold ≤ 2 s, warm p50 ≤ 100 ms, RSS ≤ 300 MB after list / ≤ 512 MB after 20 unpadded opens, the two giants open with `window=1` in ≤ 2 s (RSS reported), sig stable across opens; `--quick` = mechanics only | binary | 7s |
| legacy_browser | `python3 tests/legacy_browser.py` | Legacy slice on the 16cc89c-resynced frontend: native fixtures, SSE append/partial/reset, errors/retry, console, mobile/dark | debug server, Chromium | 7s |
| lifecycle_browser | `python3 tests/lifecycle_browser.py` | CLI and bare SSH terminal creation, idempotency, pending input, Web restart, mobile cancel | debug server, ptyhost, Chromium | n/a |
| lifecycle_browser_native_binding | `python3 tests/lifecycle_browser.py --native-binding` | API-only operator binding followed by the page, pending/native leases, durable cancel across Web restart | debug server, ptyhost, Chromium | 13s |
| lifecycle_cli_browser | `python3 tests/lifecycle_cli_browser.py` | Fake-CLI argv/session-id on create; resume takeover echoes `resume <sid>`; a configured CLI executable rewritten in place after service start still launches (its new contents); a host binary that predates `--no-record` still creates agent sessions | debug server, ptyhost, Chromium | n/a |
| lifecycle_http_suite | `python3 tests/lifecycle_http_suite.py --binary target/release/sessiondock` | HTTP-only controlled creation: free shell plus one fake Claude CLI profile. | binary, ptyhost | n/a |
| list_rows_parity | `python3 tests/list_rows_parity.py --python-source PATH --binary target/release/sessiondock` | Every `/api/sessions` row field vs the Python adapters' `list_sessions` on the fixture_gen corpus plus head/tail edge sessions and the batch-35 shapes (`--batch35-shapes`: root_sid/fork_depth/created/title/size/agent_items/supported of legacy copied-meta forks and rewind past fork point, orphans absent on both sides, exact `migration_warnings` on those rows); 0 DIFF required, `updated` timestamp form is the documented DELTA | binary, python-source | n/a |
| live_browser | `python3 tests/live_browser.py` | Three-state `/api/live`: managed running then exited; unrelated stay unknown | debug server, ptyhost, Chromium | n/a |
| missing_roots_browser | `python3 tests/missing_roots_browser.py --binary target/release/sessiondock` | Missing native roots at startup, removal and recovery of each source; refresh and open surviving sessions in the real UI. | binary, Chromium | n/a |
| live_http_suite | `python3 tests/live_http_suite.py --binary target/release/sessiondock` | HTTP-only contract of GET /api/live: empty inventory, cache hit/force miss, bound free-shell running then exited; synthetic /proc tree end-to-end (uids/tmux_uids/started_at, cache/force, `spawned_by` rows and on-disk write-once, GROK_ACTIVE) | binary, ptyhost | n/a |
| managed_terminal_browser | `python3 tests/managed_terminal_browser.py` | Console routed by UID/instance; no new CLI; force/revoke; Web restart | debug server, ptyhost, Chromium | 7s |
| media_browser | `python3 tests/media_browser.py --binary target/release/sessiondock` | Embedded PNG/JPEG HTTP + Chromium decode; tool images; no remote fetch | binary, Chromium | 9s |
| media_continuation_browser | `python3 tests/media_continuation_browser.py --binary target/release/sessiondock` | Multi-image continuation pages; live cursor untouched; 409 after rewrite | binary, Chromium | 7s |
| media_files_browser | `python3 tests/media_files_browser.py --binary target/release/sessiondock` | Disk images with configured or default path handling; replacement 409; native bytes unchanged | binary, Chromium | 7s |
| media_formats_browser | `python3 tests/media_formats_browser.py --binary target/release/sessiondock` | GIF/WebP/AVIF/BMP Chromium decode across providers and tool results | binary, Chromium | 14s |
| media_get_suite | `python3 tests/media_get_suite.py --binary target/release/sessiondock` | HTTP contract of GET /api/media/{token}, lazy descriptor registration, concurrent completion and cache eviction. | binary | n/a |
| media_lazy_browser | `python3 tests/media_lazy_browser.py --binary target/release/sessiondock` | Offscreen zero GET; scroll decode; visible diagnostics; concurrent GET completion; 404/409 reload | binary, Chromium | 8s |
| media_hardlink_suite | `python3 tests/media_hardlink_suite.py --binary target/release/sessiondock` | Referenced hard-linked media remains readable and retains its aliases through a file-manager rename. | binary | n/a |
| media_pages_walk | `python3 tests/media_pages_walk.py --binary target/release/sessiondock` | Scale walk of per-message media continuation over real HTTP pages. | binary | n/a |
| media_parity | `python3 tests/media_parity.py --python-source PATH --browser --binary target/release/sessiondock` | Python adapter/media behavior compared with Rust HTTP, including paths, grouping, formats and item size | binary, python-source, Chromium | 5s |
| messages_window_suite | `python3 tests/messages_window_suite.py --binary target/release/sessiondock` | HTTP contract of GET /api/messages/{uid} beyond history_parity. | binary | n/a |
| meta_capabilities_suite | `python3 tests/meta_capabilities_suite.py --binary target/release/sessiondock` | Prove /api/meta capability flags flip only with explicit configuration (`bug_report` stays false in every partial configuration). | binary | n/a |
| meta_hostname_check | `python3 tests/meta_hostname_check.py --binary target/release/sessiondock` | Hostname, titles, and warnings over temporary data. | binary | n/a |
| metadata_browser | `python3 tests/metadata_browser.py --binary target/release/sessiondock` | Preferences: cross-tab SSE star/unstar, parent visibility, writer restart | binary, Chromium | 20s |
| metadata_suite | `python3 tests/metadata_suite.py --binary target/release/sessiondock` | HTTP contract of persisted preferences (star, fork-visibility, nest) vs /api/sessions. | binary | n/a |
| names_parity | `python3 tests/names_parity.py --python-source PATH --browser --binary target/release/sessiondock` | Codex names: file-order/unicode, fork/agent titles, search, desktop/mobile SSE | binary, python-source, Chromium | 5s |
| native_envelopes | `python3 tests/native_envelopes.py --browser --binary target/release/sessiondock` | Stringified Codex tool images and nested replay; HTTP + Chromium | binary, Chromium | 21s |
| native_envelopes_authority | `python3 tests/native_envelopes_authority.py --binary target/release/sessiondock` | Inherited/agent-owned envelope authority; outer-tail rewrite revokes tokens | binary | 4s |
| native_spans | `python3 tests/native_spans.py --browser --binary target/release/sessiondock` | Native image spans, MIME/data-URL aliases and first-match field aliases | binary, Chromium | 9s |
| native_spans_authority | `python3 tests/native_spans_authority.py --binary target/release/sessiondock` | Fixed-cut and inventory-owned Claude span authority; prefix rewrite revokes | binary | 2s |
| native_streaming | `python3 tests/native_streaming.py --binary target/release/sessiondock` | HTTP JSONL row boundaries, full-prefix rewrite, repair, fixed-parent cut | binary | 0s |
| native_tags_browser | `python3 tests/native_tags_browser.py --binary target/release/sessiondock` | Audited native tag families in Chromium; each positive case has a literal or malformed control, and image delimiters are not file-read authority | binary, Chromium | n/a |
| nest_tree_browser | `python3 tests/nest_tree_browser.py --binary target/release/sessiondock` | Sidebar nesting (Python `nest_tree_e2e` port) on the backend's own fields: `spawned_by` seeded in `session-metadata.json`, `active` from the sidecar's open turn, `continued_in` from the tail record; flat list, tree/ordering/carets aligned with the group caret, `.agent-mark`, active-only filter refuses without a scan, hidden continued-in parent with its continuation listed and followed on open, in-place refresh after on-disk edits; right-click detach/restore/attach-to with cancel; desktop + 390 px | binary, Chromium | 6s |
| node_auth_suite | `python3 tests/node_auth_suite.py --binary target/release/sessiondock` | Node listener fail-closed matrix (four variables together), peer CIDR / token / protocol 403 bodies, loopback listener still 403 on hub headers, `/api/meta` protocol/node_id, static page 404 on the node listener | binary | n/a |
| orphan_agents_suite | `python3 tests/orphan_agents_suite.py --binary target/release/sessiondock` | HTTP contract: Claude sidecar / Codex subagent without an indexed owner are not rows, their uid opens as 501 JSON; owned agents only inside `agent_items`; deleting the owner hides the sidecar on the next refresh (grok-4.6 headless draft, reviewed) | binary | n/a |
| outbox_suite | `python3 tests/outbox_suite.py --binary target/release/sessiondock` | Read-only outbox contract on a temporary loopback server. | binary | n/a |
| outbox_discard_browser | `python3 tests/outbox_discard_browser.py` | Pending receipt dismissal, server snapshot merging and reopen behavior with intercepted loopback requests. | Chromium | n/a |
| nginx_upload_auth | `python3 tests/nginx_upload_auth.py` | Upload bodies and auth subrequests using a private loopback Nginx instance. | Nginx | n/a |
| pending_discard_suite | `python3 tests/pending_discard_suite.py --binary target/release/sessiondock` | Discarding an exited pending lifecycle receipt removes it from terminal-list pending rows; deleting the native session of a new_assigned launch also discards that receipt and its retained draft. | binary, ptyhost | n/a |
| pending_create_discard_browser | `python3 tests/pending_create_discard_browser.py --binary target/release/sessiondock` | Headless create-and-discard for Claude, Codex, Grok and SSH: unsent composer input, sidebar menu (unused AI discard; SSH stop then delete), header 删除, reload and a second page must not rebuild the row (Grok writes empty `summary.json` on start). | binary, ptyhost, Chromium | n/a |
| prefs_migration_browser | `python3 tests/prefs_migration_browser.py --binary target/release/sessiondock` | Seeds theme/font/layout/filter/cache/unread/selection and file-manager preferences under `sessiondock.*`; verifies they apply, changes persist across reload, `files.html` uses `sessiondock.files-*`, a fresh browser keeps defaults, and PWA identity is SessionDock. | binary, Chromium | 5s |
| renderer_fd_browser | `python3 tests/renderer_fd_browser.py --binary target/release/sessiondock` | The real page flushes ~75 audit batches in 30 s while the renderer's fd count is read from /proc (Chromium without sandbox, Linux only); growth must stay under 0.1 fd per batch — an unread fetch response pins a 2 MiB shared-memory pipe until GC and the 1024-fd renderer limit froze the tab in GPU code | binary, Chromium, /proc | 35s |
| reader_pool_suite | `python3 tests/reader_pool_suite.py --binary target/release/sessiondock` | Concurrent history reads wait for the shared reader pool and complete after capacity becomes available. | binary | n/a |
| prompt_claude_real | `python3 tests/prompt_claude_real.py --browser` | Real Claude question card, cheapest configuration (`claude-haiku-4-5-20251001`, effort low, `--settings` bridge, `--tools AskUserQuestion`): hook file, `prompt` field, SSE `prompt_only`, answer by clicking the card on the real page, cleared once the native answer lands; skips like send_claude_real. | claude CLI, ptyhost, Chromium | 17s |
| prompt_codex_real | `python3 tests/prompt_codex_real.py` | Real Codex command approval, cheapest configuration (`gpt-5.6-luna`, effort low, Python TUI args, `-a on-request`, read-only sandbox): screen approval as `prompt` (kind approval), SSE `prompt_only`, 拒绝 through `/api/term/send` Escape, cleared, nothing run; skips like send_codex_real. | codex CLI, ptyhost | n/a |
| restart_state_suite | `python3 tests/restart_state_suite.py --binary target/release/sessiondock` | State retained across a Web restart using the same temporary directories. | binary, ptyhost | n/a |
| rewind_browser | `python3 tests/rewind_browser.py --binary target/release/sessiondock` | Persisted Claude timeline pins through the real legacy UI (desktop + 390px). | binary, Chromium | n/a |
| rewind_http_suite | `python3 tests/rewind_http_suite.py --binary target/release/sessiondock` | HTTP-only contract of persisted Claude timeline pins (POST /api/session/rewind). | binary | n/a |
| search_browser | `python3 tests/search_browser.py --binary target/release/sessiondock` | NDJSON search, flags, navigation, unsupported regex error and recovery | binary, Chromium | 4s |
| search_cache_suite | `python3 tests/search_cache_suite.py --binary target/release/sessiondock` | Search-text cache contract: cold/hot identity, append/rewrite invalidation, cached unsupported rows, eviction, ordinary directory aliases and permissions, warm-up, concurrent searches + list, NDJSON order, memory-only mode, `--check-config`. | binary | n/a |
| search_suite | `python3 tests/search_suite.py --binary target/release/sessiondock` | Search contract coverage over 12 synthetic sessions; no Chromium. | binary | n/a |
| security_suite | `python3 tests/security_suite.py --binary target/release/sessiondock` | Raw-socket checks of the local-only security middleware. | binary | n/a |
| send_browser | `python3 tests/send_browser.py` | Claude reliable send through the real legacy composer against the fake Claude CLI (desktop + 390 px); a second page's send and composer Esc refused with the owner while the console is held, the 390 px Esc written with no console open; attachments staged on selection, surviving a reload, failed staging with retry, discard of removed staging, publication on SEND. | Chromium, ptyhost | n/a |
| draft_sync_browser | `python3 tests/draft_sync_browser.py` | Two pages on one draft: a second page opens the saved text with the server revision and no error, a refused save rebases and the editing page wins (no revision dead end), an idle page follows within the poll both ways, an attachment staged on one page is sent from the other, that SEND empties the first page, an image staged on one page previews on the other from the staged bytes, and a console command sent from the composer clears the server draft. | Chromium, ptyhost | n/a |
| send_readiness_browser | `python3 tests/send_readiness_browser.py` | Grok login/unknown SEND refusal, recovery, post-paste recheck, and no auto-retry of an unknown write (fake CLI, desktop + 390 px). | Chromium, ptyhost | n/a |
| send_codex_attachments_browser | `python3 tests/send_codex_attachments_browser.py` | Codex composer sends text plus two PNG attachments, then text plus a `.txt`, against the fake Codex CLI; each batch is published to the session cwd and submitted with Enter. | Chromium, ptyhost | n/a |
| send_claude_real | `python3 tests/send_claude_real.py` | Real Claude CLI reliable send, cheapest configuration (`claude-haiku-4-5-20251001`, effort low); skips with a printed reason when `claude` is absent or unauthenticated. | claude CLI, ptyhost | n/a |
| send_codex_browser | `python3 tests/send_codex_browser.py` | Codex reliable send through the real legacy composer against the fake Codex CLI resumed via the console button (desktop + 390 px). | Chromium, ptyhost | 17s |
| send_codex_real | `python3 tests/send_codex_real.py` | Real Codex CLI reliable send, cheapest configuration (`gpt-5.6-luna`, `model_reasoning_effort="low"`); skips with the CLI's own error when `codex` is absent, unauthenticated or over its usage limit. | codex CLI, ptyhost | 13s |
| send_codex_startup_browser_real | `python3 tests/send_codex_startup_browser_real.py` | Operator-only (`--include-real`): cold-start SEND with real Codex at Luna low, checking native user text and the turn's model/effort. | codex CLI, Chromium | n/a |
| send_http_suite | `python3 tests/send_http_suite.py --binary target/release/sessiondock` | HTTP-only contract of the reliable-send routes against the fake Claude and Codex CLIs (batches 31–32). No Chromium. | binary, ptyhost | 12s |
| session_deep_link_browser | `python3 tests/session_deep_link_browser.py --binary target/release/sessiondock` | `?sid=` opens the root and a nested subagent from synthetic history, without starting a CLI. | binary, Chromium | n/a |
| session_stop_browser | `python3 tests/session_stop_browser.py` | Legacy stop control under the Rust `session_stop` capability. | Chromium, ptyhost | n/a |
| session_stop_http_suite | `python3 tests/session_stop_http_suite.py --binary target/release/sessiondock` | HTTP-only contract of POST /api/session/stop for managed instances. | binary, ptyhost | 6s |
| session_titles_browser | `python3 tests/session_titles_browser.py --binary target/release/sessiondock` | Point title queries, native rename, and no hot discovery, on synthetic data. | binary, Chromium | n/a |
| term_records_http_suite | `python3 tests/term_records_http_suite.py --binary target/release/sessiondock` | `/api/term/records` list and the read-only replay WebSocket: disabled 501, bad ids, upgrade required, `timeline`+`record`, live follow, resize, exit/end (socket stays open), ended replay, input ignored, timeline seek/play/pause. | binary, ptyhost | 10s |
| sessions_list_suite | `python3 tests/sessions_list_suite.py --binary target/release/sessiondock` | Contract coverage for GET /api/sessions (shape, topology, sig, large inputs, supported:true + migration_warnings; duplicate Codex session_meta and invalid JSONL warnings remain readable, while orphan agents are not rows and their UID is a typed 501). | binary | n/a |
| shutdown_suite | `python3 tests/shutdown_suite.py --binary target/release/sessiondock` | Graceful shutdown (M7): SIGTERM/SIGINT drain SSE, media, search, and audit. | binary | n/a |
| side_drag_browser | `python3 tests/side_drag_browser.py --binary target/release/sessiondock` | Sidebar divider under real CDP touch drag/cancel at 814x380 and mouse drag/dblclick at desktop; desktop row text selection survives the synthetic click and defers list refresh until selection clears; width stored under `sessiondock.` (Python `side_drag_e2e` port) | binary, Chromium | 6s |
| sidebar_select_scroll_browser | `python3 tests/sidebar_select_scroll_browser.py --binary target/release/sessiondock` | Scroll the date/nest sidebar to the last row and click it: the clicked session opens, membership is unchanged, `#side.scrollTop` and the last row's viewport Y stay put, and `renderSide` is not called | binary, Chromium | n/a |
| spawned_by_suite | `python3 tests/spawned_by_suite.py --binary target/release/sessiondock` | Synthetic `/proc` tree through `SESSIONDOCK_PROC_ROOT`: `/api/live` uids/started_at and `spawned_by` rows written once into the state dir | binary | n/a |
| symlink_agents_suite | `python3 tests/symlink_agents_suite.py --binary target/release/sessiondock` | Claude subagent sidecar aliases, including in-root, dangling and external symlink fixtures. | binary | n/a |
| sse_suite | `python3 tests/sse_suite.py --binary target/release/sessiondock` | Raw-HTTP coverage of GET /api/watch SSE for Claude, Codex and Grok. | binary | n/a |
| static_assets_suite | `python3 tests/static_assets_suite.py --binary target/release/sessiondock` | Contract coverage for static serving and HTML template injection. | binary | n/a |
| term_send_http_suite | `python3 tests/term_send_http_suite.py --binary target/release/sessiondock` | HTTP-only contract of POST /api/term/send and /api/term/scroll. | binary, ptyhost | n/a |
| terminal_browser | `python3 tests/terminal_browser.py` | Temporary POSIX shell: bytes, resize, takeover, and PTY survival across Web restart. | debug server, ptyhost, Chromium | 4s |
| terminal_claim_browser | `python3 tests/terminal_claim_browser.py` | Real console clicks and shell input/output after a 6 s request/response delay; desktop xterm, mobile grid, full claim timeout without automatic retry, and explicit recovery. | debug server, ptyhost, Chromium | 40s |
| terminal_diagnostics_browser | `python3 tests/terminal_diagnostics_browser.py` | Claim, header, body, and first grid-paint receipts on an isolated shell with delayed HTTP. | Chromium, ptyhost | n/a |
| terminal_exit_browser | `python3 tests/terminal_exit_browser.py` | Xterm and grid: complete vs incomplete exit, retained tail and visible reason, no automatic reclaim, gray button, manual replacement | debug server, ptyhost, Chromium | 14s |
| terminal_records_browser | `python3 tests/terminal_records_browser.py` | `records.html`: list, live follow, resize follow, read-only keyboard, exit status, `?id=` reload, fit toggle, no page errors | debug server, ptyhost, Chromium | 12s |
| terminal_selection_browser | `python3 tests/terminal_selection_browser.py` | Mouse selection and copy with and without CLI mouse capture, in isolated PTYs. | Chromium, ptyhost | n/a |
| terminal_timeline_browser | `python3 tests/terminal_timeline_browser.py` | Console replay timeline of an ended SSH session, xterm and grid renderers: live exit on the same page switches to read-only replay with the timeline (not a clipped live tail), then a list click after reload does the same; seek to start, play 16x to the end, seek to end, keyboard ignored, timeline hidden when the pane closes, no page errors | debug server, ptyhost, Chromium | 25s |
| terminal_grid_render_browser | `python3 tests/terminal_grid_render_browser.py [--base URL]` | Grid renderer pixels in Chromium without a server: a row of tall glyphs leaves nothing in the neighbouring rows, cell height on whole device pixels at dpr 1/1.25/1.5, no seam in a multi-row background; `--base` checks a deployed instance's assets | Chromium | 5s |
| terminal_grid_browser | `python3 tests/terminal_grid_browser.py` | `grid.html` server-side grid console: connect under the claim, typing, pty resize follow, scrollback wheel/follow, selection copy, paste, host exit, no page errors | debug server, ptyhost, Chromium | 15s |
| terminal_grid_theme_browser | `python3 tests/terminal_grid_theme_browser.py` | Main console theme switching with real PTY colors; OSC color queries stay dark-only. | Chromium, ptyhost | n/a |
| terminal_input_browser | `python3 tests/terminal_input_browser.py` | Legacy console raw HTTP input under the Rust `terminal_input` capability. | Chromium, ptyhost | n/a |
| tool_group_fold_browser | `python3 tests/tool_group_fold_browser.py --binary target/release/sessiondock` | Tool groups: streaming tail opens/seals, user-opened groups survive later tool/result appends (`syncGroupNode`/`appendToolResult`), Rust `media_more` kept on a paired result (Python `tool_group_fold_e2e` port) | binary, Chromium | 4s |
| tool_parity | `python3 tests/tool_parity.py --python-source PATH --browser --binary target/release/sessiondock` | Rich tool cards/split view; oversize warning; recorded commands never executed | binary, python-source, Chromium | 10s |
| trash_browser | `python3 tests/trash_browser.py --binary target/release/sessiondock` | Real legacy page against the Rust session recycle bin. | binary, Chromium | n/a |
| trash_http_suite | `python3 tests/trash_http_suite.py --binary target/release/sessiondock` | HTTP-only recycle-bin contract (docs/trash.md vs api/trash.rs). | binary | n/a |
| unicode_paths_suite | `python3 tests/unicode_paths_suite.py --binary target/release/sessiondock` | Unicode and awkward file/directory names across the read model. | binary | n/a |

`provider_parity.py` is a helper imported by history suites; the runner skips it.
`bench_summary.py` and `rss_watch.py` are discovered (they have `__main__`) but
need their own CLI args; the runner does not supply them.

## Adding a suite

`tests/run_validation.py` discovers suites after the fixed Rust checks:

1. Every `tests/*_contract.mjs` is passed to one `node --test` suite named
   `node_contracts`.
2. Every `tests/*.py` with `if __name__ == "__main__"` becomes a Python suite,
   except `run_validation.py`, `provider_parity.py`, and `*_benchmark.py`.
3. `*_parity.py` gets `--python-source` from the explicit setting or the unique
   structurally discovered sibling oracle checkout; otherwise the suite is SKIP.
4. If argparse text contains `--browser`, `--browser` is appended.
5. If argparse text contains `--binary`, `--binary target/release/sessiondock`
   is appended (override with the runner's `--binary`).
6. `lifecycle_browser.py` is also run as `lifecycle_browser_native_binding` with
   `--native-binding`.

Name HTTP+Chromium checks `*_browser.py`, adapter differentials `*_parity.py`,
and legacy-web vm tests `*_contract.mjs`. Print `PASS …` lines on success. Exit
non-zero on failure. Copy fixtures into temporary directories only; never write
into the repo, native homes, or production paths.

## Opt-in benchmarks

| Suite | Command | Covers | Needs | Typical time |
| --- | --- | --- | --- | --- |
| append_benchmark | `python3 tests/append_benchmark.py --binary target/release/sessiondock` | Opt-in; not in `run_validation`. Synthetic append timings; `--rss` for Linux VmRSS | binary | n/a |
| native_envelopes_benchmark | `python3 tests/native_envelopes_benchmark.py --binary target/release/sessiondock` | Opt-in; not in `run_validation`. Nested stringified-tool GET timing and child RSS | binary | n/a |
| native_spans_benchmark | `python3 tests/native_spans_benchmark.py --binary target/release/sessiondock` | Opt-in; not in `run_validation`. Native-span HTTP timing and exact-child RSS | binary | n/a |
| bench_term_echo_browser | `python3 tests/bench_term_echo_browser.py --modes served,timer,frame,none` | Opt-in; not in `run_validation`. Console keystroke→echo latency (WebSocket byte → xterm `onWriteParsed`) and `seq` burst time in headless Chromium against an isolated ptyhost shell with tty echo; `--modes` swaps the page's output write policy in place to compare batching strategies | debug binary, ptyhost, Chromium | 30s |
| read_benchmark | `python3 tests/read_benchmark.py --binary target/release/sessiondock` | Opt-in; not in `run_validation`. Synthetic read smoke (list/window/delta) | binary | n/a |
| real_roots_bench | `python3 tests/real_roots_bench.py --claude-root … --codex-root … --grok-root … --binary target/release/sessiondock --open 20 --i-understand-this-reads-real-histories` | Operator-only read-only benchmark on a separate loopback server; checks latency, RSS, and unchanged roots. | binary, real roots | n/a |
| bench_polls_real | `python3 tests/bench_polls_real.py --claude-root … --codex-root … --grok-root … [--codex-index …] [--registry debug-runs.json] --binary … --ptyhost target/release/ptyhost --hosts 26 --label after --i-understand-this-reads-real-histories` | Operator-only, read-only roots, scratch state/host/lifecycle: medians of 5 for `/api/sessions`, `?sig=`, `/api/live`, `/api/term/list`, eight concurrent lists, the `force=1` paths and the browser cadence, plus VmRSS, with `--hosts` free-shell ptyhost instances it creates and kills; run once per binary to compare ([performance.md](performance.md#轮询路径最终响应缓存2026-09-15)) | binary, ptyhost, real roots | n/a |
| agent_active_real_check | `python3 tests/agent_active_real_check.py --rust http://127.0.0.1:8741 --python-source PATH --i-understand-this-reads-real-histories` | Operator, read-only: `agent_items[].active/created/updated` and `continued_in` of every real owner row vs the Python adapters in-process (`# run_validation: skip`; grok-4.6 headless draft, reviewed) | running server, python-source | n/a |
| live_shadow_compare | `python3 tests/live_shadow_compare.py --rust http://127.0.0.1:8741 --python http://127.0.0.1:8710` | Operator, GET-only: `/api/live` uids/tmux_uids/started_at of the Rust service vs the deployed Python service (`# run_validation: skip`; grok-4.6 headless draft, reviewed) | both services | 0s |
| spawn_real | `python3 tests/spawn_real.py --binary target/release/sessiondock` | Real CLI (`claude-haiku-4-5-20251001 --effort low` running `grok-4.6` low): a session spawned by another session is listed with `spawned_by` by a Rust instance reading the real roots with the scan on; deletes exactly what it created (`# run_validation: skip` — operator run per batch: whether the haiku session actually executes the grok command is up to the model, so it is not a sweep gate; `--dry-run` prints the commands; grok-4.6 headless draft, reviewed) | claude, grok, binary | n/a |
| cutover_drill | `python3 tests/cutover_drill.py --binary target/release/sessiondock --ptyhost target/debug/ptyhost` | Operator-only cutover rehearsal in a temporary directory; checks PTY survival and untouched fake Python data. | binary, ptyhost | n/a |
| unsupported_rows_report | `python3 tests/unsupported_rows_report.py --base http://127.0.0.1:8741` | Operator report over a running server's `/api/sessions`: `supported:false` rows grouped by fatal reason with examples, per-source totals, top non-fatal warnings; JSON only, never reads session files (`# run_validation: skip`; grok-4.6 headless draft, reviewed) | running server | 0s |
