# SessionDock workspace

## Scope and boundaries

- This is an independent repository. Do not modify the sibling Python project
  or its deployment while working here unless the user explicitly requests it.
- Deployment status: the Rust service runs on this machine as the user-level
  systemd unit `sessiondock.service` under its own private prefix
  `/srv/sessiondock` (bin, web, etc/env, and the private state/delivery/
  lifecycle/host/audit/trash directories), loopback-only behind the existing
  authenticated reverse proxy at the location `/sessiondock/`, in parallel with
  the Python service (the unit, the environment file and every address live
  outside the repository). The real CLI read roots are configured
  explicitly and stay read-only; every private directory (state, delivery,
  lifecycle, host, audit, trash) belongs to the Rust service alone and is
  disjoint from the Python data directory and the CLI homes. Switching production
  traffic (stopping Python, changing the proxy) is a separate authorized step
  (`docs/replacement-checklist.md`). Missing configuration must fail closed.
- Development and tests use loopback listeners and isolated runtime directories.
  Imported ptyhost retains its original default directory: always pass an
  explicit `--dir` when invoking it. Never use the deployed service's
  directories or production sessions for smoke tests.
- Real CLI test runs (a real claude/codex/grok as the CLI *under test*) use the
  cheapest model at low effort — Claude Code `claude-haiku-4-5-20251001` with `--effort low` (the
  full dated ID, never the `haiku` alias), Codex `gpt-5.6-luna` with
  `model_reasoning_effort="low"`, Grok `grok-4.6` at low effort — keep the
  JSONL model assertion enabled and never fall back to a more expensive
  model; anything pricier needs the user's authorization per run. The
  real-CLI suites live in `tests/` and run in the normal `run_validation`
  sweep (tag `real-cli`, skipped when the CLI binary is absent or via
  `--skip`); they never enter `cargo test`, use isolated config/home and host
  directories, throwaway working directories, a few short turns, and delete
  the sessions they create. This rule covers the CLI under test only.
- Never commit deployment addresses, personal absolute paths, credentials,
  runtime data, build outputs, or local environment files.
- `origin` is the public GitHub repository `zengjian513f/sessiondock`. Push when
  work lands or the user asks, not after
  every small fix; do not publish packages. Redeploying the running service
  (rebuild, replace the binary, restart) needs a user request per batch.

## Structure

- Treat SessionDock as an independent project. Current contracts live in `docs/`
  and unfinished work lives only in `TODO.md`; do not use the archived migration
  history as current guidance. `legacy-web/` is the production frontend. Do not
  expand the Vue scaffold unless the user selects that direction.
- `crates/sessiondock`: Rust HTTP service. Keep transport handlers separate
  from session/domain logic and future ptyhost client code.
- `crates/ptyhost`: imported independent session host. Preserve its local wire
  protocol and process lifetime unless a task explicitly changes them.
- `crates/ptyhost-client`: explicit-directory local protocol library. It must not
  launch processes, auto-clean unknown host records, or expose host credentials.
- `legacy-web`: first-stage working frontend, resynced to Python `agenthub/static` `e5b023a` (batch 37): keep changes small and capability gated, degrade like Python when a row field is absent, never hide/disable the console explanation or silently retire outbox data. Preferences are read through `AgentHubCapabilities.stored` (batch 44 WP-F: one-time fallback from the Python `agenthub.*` key, writes only under `sessiondock.`); the shell identity (manifest, service-worker cache `sessiondock-shell-*`, page titles) is SessionDock, checked by `tests/brand_names_check.py`.
- `web/src/api`: wire types, runtime validation, and network clients.
- `web/src/domain`: framework-independent state transitions and protocol logic.
- `web/src/stores`: small Pinia stores, split by responsibility.
- `web/src/components`: UI components. Do not put session synchronization,
  delivery confirmation, or terminal byte buffering in view callbacks.
- `reference/legacy-web`: frozen migration reference, not served or bundled.
  Record intentional baseline changes in `docs/migration.md`.

## Delegation

- Simple, self-contained tasks (one new script/fixture/doc, mechanically
  verifiable, no shared files) may be drafted headless by the local `grok` CLI
  (`grok-4.6`); see `docs/delegation.md` for the command, task template and the
  mandatory review checklist. Record such output in the batch ledger as
  "grok-4.6 headless 产出，人工审阅". Correctness-sensitive modules (delivery,
  authorization, native semantics, ptyhost protocol) are never delegated there.
  Run such delegations 8–12 in parallel, one self-contained task each.
  grok recipe and smoke test: `~/.claude/grok-cli.md`.
- Larger work is split into work packages with exclusive file ownership and run in
  parallel: correctness-sensitive Rust modules go to Claude subagents (Opus), self-contained
  scripts/fixtures/docs to grok; each package is validated in an isolated worktree and landed
  per batch with the full sweep (the batch 34 section of the plan is the worked example).
- Target parity, not more: the backend should match the existing Python
  backend or exceed it only slightly. The goal is replacing Python soon; do not
  go too far beyond its behaviour except where isolation/safety requires it.

## Validation

- Rust: `cargo test --workspace --locked`.
- Legacy: `node --test tests/legacy_contract.mjs`; build the server then run
  `python3 tests/legacy_browser.py` (Playwright Chromium required, isolated fixtures).
- Fast HTTP shape check without Chromium: `python3 tests/api_smoke.py` (every
  implemented read route, 501/404/405 ledger entries). Scale walks without
  Chromium: `python3 tests/history_pages_walk.py` (3000-record pagination,
  contiguity, grant reuse) and `python3 tests/media_pages_walk.py` (250-image
  continuation, 257 explicit failure). Pure legacy helpers:
  `node --test tests/legacy_pure_contract.mjs`. HTTP-only contract suites (no
  Chromium, all discovered by run_validation): `tests/sessions_list_suite.py`,
  `tests/messages_window_suite.py`, `tests/search_suite.py`, `tests/sse_suite.py`,
  `tests/security_suite.py`, `tests/static_assets_suite.py`,
  `tests/meta_capabilities_suite.py`, `tests/input_history_suite.py`,
  `tests/grants_edge_suite.py`, `tests/media_get_suite.py`,
  `tests/files_read_suite.py`, `tests/metadata_suite.py`, `tests/outbox_suite.py`,
  `tests/lifecycle_http_suite.py`, `tests/live_http_suite.py`,
  `tests/trash_http_suite.py`, `tests/rewind_http_suite.py`,
  `tests/term_send_http_suite.py`, `tests/shutdown_suite.py`,
  `tests/restart_state_suite.py`, `tests/unicode_paths_suite.py`,
  `tests/budget_boundaries_suite.py`; each asserts what the code does and
  reports doc discrepancies in its summary. Operational aids:
  `tests/python_route_gap.py` (Python routes vs Rust router),
  `tests/config_mapping.py` (Python knobs → Rust env vars),
  `tests/legacy_gating_check.py`, `docs/replacement-checklist.md`.
- Everything at once: `python3 tests/run_validation.py` (`--list` shows the plan,
  `--tags`, `--only`, `--keep-going`, `--json PATH`, `--rerun-failed PATH`,
  `--dry-run`); it runs the Rust checks, Node contracts and every Python suite
  serially with per-suite logs under `target/validation/`. `docs/validation.md`
  tabulates every suite. Report tools: `tests/route_ledger.py` (router vs route
  inventory vs legacy calls),
  `tests/legacy_asset_diff.py` (served vs frozen frontend),
  `tests/check_docs_links.py` (Markdown links/anchors; run before committing
  docs), `tests/bench_summary.py` (benchmark JSONL → comparison tables),
  `tests/rss_watch.py` (procfs memory sampling).
- History changes: `python3 tests/history_parity.py` and
  `python3 tests/history_browser.py`; optionally pass `--python-source ../agenthub`
  to the parity tool for adapter-only comparison against synthetic data.
  `python3 tests/advanced_parity.py --python-source ../agenthub` covers multi-level
  compaction/rewind/sidechains, fork-of-fork with subagents, rich tool cases and
  Grok envelopes; every difference must be a documented DELTA, never UNVERIFIED.
- Read model (batch 34, docs/read-model.md): the session list is the lazy
  index (`sessions/index/`: directory walk + `stat` + 96 KiB head / 512 KiB
  tail summaries cached by stamp, no file parsed whole, no session/byte/entry
  cap) and a session is opened on demand through `sessions/views/` (streamed
  once, extended on append, bounded LRU). Batch 44 WP-C aligned the rows with
  Python: the debug-run registry `<state dir>/debug-runs.json` filters every
  view (`?debug_run=` selects one run, `sessions/debug_runs.rs`), Claude
  subagent symlinks inside a configured root are followed, Grok `size` is the
  whole session directory, the Codex `/rename` command event is synthesized,
  and the default `hostname` is the system host name (`SESSIONDOCK_HOSTNAME`). `SessionStore` is only the facade:
  never reintroduce a full-parse inventory, a whole-list 503 on a changing
  file, or per-file budgets enforced by the list. List rows carry
  `cursor: {end, head}` from the index and `anchor` only for sessions with a
  current cached view; `sig` never changes because a session was opened.
  Lineage errors, content-block notes and record/LF/file budgets surface on
  open (501/413 for that session), not in the row. Verify with
  `cargo test -p sessiondock --lib sessions:: --locked`,
  `python3 tests/sessions_list_suite.py`, `python3 tests/inventory_scale_suite.py`
  (full mode; `--quick` for mechanics), `python3 tests/inventory_live_append_suite.py`,
  `python3 tests/list_rows_parity.py --python-source ../agenthub` (0 DIFF) and
  `python3 tests/budget_boundaries_suite.py`; the operator-only
  `python3 tests/real_roots_bench.py … --i-understand-this-reads-real-histories`
  measures the real read roots read-only (targets in docs/read-model.md) and
  is never part of the sweep.
- Read model vs current CLI versions (batch 33): an unknown record kind, Claude
  attachment kind, Codex `event_msg`/`response_item` kind, Grok record kind or
  unknown non-image content block is skipped exactly like the Python adapters
  and counted in the detail `meta.migration_warnings` as non-fatal notes
  (`跳过未知的Claude 记录类型：atis-latch ×N`, at most 32 kinds plus one
  overflow line; batch 44 WP-C removed them from the public list rows, which
  carry `migration_warnings` only when `supported:false`, like Python's rows
  that have no such field); `supported` stays true and the session lists,
  pages, streams, searches, resumes and confirms sends normally. Do not reintroduce a kind
  whitelist failure. Hard failures stay `supported:false` with
  `migration_warnings == [reason]`: scalar `content`, non-string text blocks,
  undecodable images, Codex bad `history_base`, fixed-prefix cuts off a line
  boundary, budgets (batch 35 moved corrupt lines, duplicate `session_meta`
  and broken Claude lineage to the Python-compatible rules below). Claude attachment
  records are graph nodes (the reply's parentUuid is the last attachment);
  fixtures for these shapes are the `tests/history_parity.py` helpers
  (`claude_control_rows`, `claude_attachment_chain`, `claude_turn_tail_rows`,
  `codex_telemetry_rows`), and `tests/fixture_gen.py` writes them by default
  (`--plain` omits). Verify with `cargo test -p sessiondock --lib
  sessions::providers --locked`, `python3 tests/history_parity.py
  --python-source ../agenthub`, `python3 tests/advanced_parity.py
  --python-source ../agenthub` and `python3 tests/sessions_list_suite.py`;
  `tests/send_claude_real.py` must then list the real session as supported,
  resume it and confirm the send from its JSONL.
- Real-root shapes Python reads (batch 35, Python `adapters.py` is the oracle):
  the first Codex `session_meta` is the only identity, later copies are counted
  as `跳过重复的Codex session_meta ×N`; `history_base: null` with
  `forked_from_id` is a self-contained legacy fork (nothing inherited);
  `history_base.thread_id` is the physical prefix parent even when
  `forked_from_id` names another thread; list decoration (`root_sid`,
  `fork_depth`, `created`, `title`, `size`) follows the `forked_from_id`
  chain exactly like Python `finalize_sessions`; an agent whose owner is not
  indexed (Claude sidecar without its main file, Codex subagent without its
  parent) is not a row at all (its uid still opens as a typed 501); a line that
  is not JSON is skipped (`跳过无效的JSONL 记录 ×N`, bytes and cursors kept) and a
  Claude lineage walk that reaches a missing/visited uuid or a missing declared
  leaf stops there with a non-fatal warning instead of 501. Verify with
  `cargo test -p sessiondock --lib sessions:: --locked`, the six parity
  suites with `--python-source ../agenthub`, `tests/sessions_list_suite.py`,
  the batch-35 contract suites (`codex_legacy_fork_suite`,
  `codex_fork_rewind_suite`, `orphan_agents_suite`, `claude_torn_lines_suite`,
  `fork_rows_parity`, `fork_messages_parity`, `claude_lineage_parity`), and —
  the actual gate — the operator-run real-root tools: `tests/shadow_compare.py`
  (0 DIFF), `tests/unsupported_rows_report.py` (0 rows Python would read) and
  `tests/real_roots_bench.py`. Acceptance for every batch includes such a
  read-only run against the real roots; temporary test sessions may be created
  and must be deleted afterwards; existing native files are never rewritten.
- History pages: `node --test tests/history_pages_contract.mjs` and
  `python3 tests/history_pages_browser.py`. Keep gap cursors separate from live
  checkpoints, preserve concurrent SSE updates, and discard stale view/reset
  responses. Page errors retain the visible snapshot with explicit recovery;
  never silently fall back to an unbounded history request.
- Native JSON reuse: preserve full committed-prefix byte verification and full
  provider projection; cached ASTs do not authorize appending old Event vectors.
  Run `tests/append_benchmark.py` with saved release binaries for performance
  comparisons; report cold-read costs as well as append improvements.
- Native input/scanner: follow `docs/native-input.md`. Preserve physical LF
  checkpoints, object insertion order and separate retained-index budgets.
  Compare serialized values as well as Value equality: the latter ignores
  object order. A private text span is not media authority or proof of large
  image support; native source/branch authorization must precede range reads.
  Run `python3 tests/native_streaming.py` for real HTTP row-boundary, full-prefix
  rewrite, repair and fixed-parent-cut checks. Linux memory observations may use
  `tests/append_benchmark.py --rss`, never substitute logical weights for RSS.
  Structured native spans additionally require `tests/native_spans.py --browser`
  and `tests/native_spans_authority.py`. Preserve full current-branch membership
  on both cold/warm GET, original candidate stamps, bounded JSON unescaping and
  final checked-handle verification before HTTP publication. Nested stringified
  Codex tool envelopes additionally require `tests/native_envelopes.py --browser`
  and `tests/native_envelopes_authority.py`: only reviewed tool-output positions
  are candidates, inner plan offsets address decoded parent text (never file
  bytes), every candidate open drains/finishes the checked source, and one shared
  work budget covers all layers. Multi-part tool outputs show every envelope
  chunk in order (batch 36, `docs/native-input.md`); per-message media
  continuation is batch 20. Image semantic-v2 deliberately resets
  pre-v2 image cursors once, not on every parse.
- Native media: `python3 tests/media_browser.py` uses synthetic embedded images
  and real legacy rendering. Preserve typed private payloads, window-before-decode,
  semantic cursors independent of random tokens, bounded cache/response ownership,
  and zero unconfigured/out-of-root file or automatic remote access. File-backed
  media uses explicit development roots, full selected-view reference authority,
  retained checked handles and versioned tokens reauthorized on every GET.
  Run `tests/media_formats_browser.py` and `tests/media_files_browser.py` for
  new-format and authorized disk-image changes, using only synthetic fixtures.
  `tests/media_parity.py --python-source ../agenthub` compares the actual Python
  adapter/media helpers using fixture-only paths; preserve explicit safety deltas
  rather than normalizing away message counts, image bytes or missing content.
- Media continuation: `node --test tests/media_continuation_contract.mjs` and
  `python3 tests/media_continuation_browser.py`. Messages inline at most 16 typed
  images; grants bind checkpoint, non-status event index and content identity;
  a media page never moves the live cursor and legacy appends in place.
- Lazy media: `node --test tests/media_lazy_contract.mjs` and
  `python3 tests/media_lazy_browser.py`. History only registers sources; GET
  materializes them under independent encoded-source/blob budgets. File versions
  bind at registration and are reauthorized even on a warm GET. Preserve explicit
  error/retry controls, bounded diagnostics and live updates during reload render.
- Search: `python3 tests/search_browser.py`. Rich tool presentation:
  `python3 tests/tool_parity.py --python-source ../agenthub --browser`.
  These create synthetic records and do not execute the recorded tool commands.
- Preferences: `python3 tests/metadata_browser.py`. Read performance smoke:
  `python3 tests/read_benchmark.py --binary target/release/sessiondock`.
  Runtime/native directories must not overlap served assets; metadata needs a
  dedicated directory separate from native and host roots. Tests create private
  directories explicitly, without altering process-wide umask.
- Files: `python3 tests/files_browser.py`; explicit development file roots must
  be disjoint from static/native/metadata/host paths. Preserve checked reader
  handles through streaming; never reopen their display paths. Jobs and
  thumbnails remain separately capability-gated until implemented.
- Instance-bound legacy console: `python3 tests/managed_terminal_browser.py`;
  requires built server and ptyhost, uses only a private synthetic shell/native
  fixture. Manual terminal access must not enable the reliable-send composer or
  CLI creation. Persisted terminal layouts include the full UID and instance.
- CLI launch profiles: `cargo test -p sessiondock --test lifecycle_cli_http --locked`
  and `python3 tests/lifecycle_cli_browser.py`; both use private fake CLI shell
  scripts only. Placeholders are whole arguments (`{session_id}`, `{sid}`), Claude
  new sessions get a server-minted UUID persisted before Prepared, Codex/Grok new
  sessions stay pending, resume SIDs come from the index's native catalog (head/tail summaries), never from
  the client; the Python login-shell/`env -u` wrapper is not reproduced by the
  launcher — a deployment gets the login environment by configuring the wrapper
  script as the profile executable with the CLI's PATH name as `args[0]`
  (batch 44 WP-F, docs/lifecycle-launcher.md "Login-shell wrapper").
- File writes: `cargo test -p sessiondock --test files_write --locked` and
  `python3 tests/files_write_browser.py`; write roots are explicit and inside
  read roots, every chunk is re-resolved through the session scope, nothing is
  ever overwritten or unlinked (linkat/O_EXCL publish, trash directory), and a
  swapped path component between resolve and write is 409 with nothing written.
- Timeline pins: `cargo test -p sessiondock --test rewind_http --locked` and
  `python3 tests/rewind_browser.py`; a pin is display-only parser options
  (declared_tip/abandoned_after), retires with a reason on any later lineage
  signal, changes the anchor (reset, never diff) and never touches native files
  or the CLI (`native_rewind:false`).
- Recycle bin: `cargo test -p sessiondock --test trash_http --locked` and
  `python3 tests/trash_browser.py`; file sets come from the published index rows
  only, fork parents stay protected, `running` is never deletable, `unknown`
  needs an explicit force, restore never overwrites, purge never touches native
  directories.
- Raw terminal input: `cargo test -p sessiondock --test terminal_input --locked`
  and `python3 tests/terminal_input_browser.py`; input needs the exact current
  lease, named keys never fall back to literal text, an unacknowledged write is
  504 and never retried, and `outbox` stays false (no reliable-send semantics).
- Managed instance stop: `cargo test -p sessiondock --test session_stop --locked`,
  `python3 tests/session_stop_http_suite.py` and `python3 tests/session_stop_browser.py`; only a managed instance resolved
  through the index's native catalog plus a fresh guarded runtime observation is
  stopped (Ctrl-D twice, then the host's own guarded stop), an unmanaged or
  external CLI is a typed 501, an unreachable/duplicate record is 409, the Web
  process never signals a PID, and an unobserved exit is reported `uncertain`.
- Codex native acknowledgment adapter (library only): `cargo test -p sessiondock
  --lib delivery::codex_adapter --locked` and `--test codex_ack`; records after a
  fixed, re-validated confirmation boundary classify to `PossibleTextMatch`
  (which the state machine still refuses), `Absent` or `Uncertain`
  (ambiguous duplicates, checkpoint mismatch, media), never a request-id or
  turn correlation; nothing here acknowledges a receipt or touches a CLI.
- Claude reliable send: `cargo test -p sessiondock --lib delivery::driver
  --lib delivery::claude_adapter --lib delivery::executor --locked` and the
  integration `--test delivery_send`; then `python3 tests/send_http_suite.py`
  (HTTP-only contract of the four routes against both fake CLIs, no Chromium)
  and `python3 tests/send_browser.py`
  (desktop + 390 px, real legacy composer against the fake CLI). Every write
  follows a durable commit, paste and Enter are two persisted steps, a crash
  between them is uncertain and never re-injected, confirmation comes only from
  the session's JSONL `user` record (association `VerifiedEnter`, screen text
  never confirms), retry is only for an unwritten local waiter, and the four
  send routes are `501` unless the delivery ledger and terminal transport are
  both configured (`outbox:true`). The fake Claude CLI (`tests/fake_claude_cli.py`)
  is a private script; no model binary is used. Real-CLI acceptance runs
  automatically with the cheapest configuration: `python3 tests/send_claude_real.py`
  (`--model claude-haiku-4-5-20251001 --effort low`, exact-ID JSONL assertion,
  isolated `CLAUDE_CONFIG_DIR` reused read-only, proxy variables passed through,
  one prompt, receipt persisted→injected→confirmed from the real JSONL) — it
  skips with a printed reason when `claude` is absent or cannot authenticate a
  standalone call. See [docs/delivery-executor.md](docs/delivery-executor.md).
- Codex reliable send (batch 32, same executor): `cargo test -p sessiondock
  --lib delivery::driver --lib delivery::codex --lib delivery::executor::codex_tests
  --locked` and the integration `--test delivery_send_codex`; then
  `python3 tests/send_codex_browser.py` (desktop + 390 px, real legacy composer
  against `tests/fake_codex_cli.py`, resumed through `resume_args
  ["resume","{sid}"]`). The Codex composer follows `codex_bridge.composer_state`
  (status footer or cursor-anchored `›`/`»` block, dim placeholder, braille
  particle glyphs blanked, `lag > 0` never idle); paste and Enter stay two
  persisted steps, Codex owns queueing while working (no idle wait), and a
  receipt is acknowledged only from a rollout user record after the fixed
  boundary that carries its own turn ID — the executor pairs it with its own
  persisted Enter as `Correlation::OperationTurn` (the adapter alone still
  emits only `PossibleTextMatch`); no turn ID, duplicates, swallowed lines and
  rewritten rollouts stay uncertain. `outbox/discard` of an attempted Codex
  row is a dismissal (hidden, tombstone kept, idempotent `200`), never a
  resend. Real-CLI acceptance `python3 tests/send_codex_real.py` runs in the
  normal sweep with the cheapest configuration only (`gpt-5.6-luna`,
  `-c model_reasoning_effort="low"`, isolated `CODEX_HOME` reusing `auth.json`
  read-only, proxy variables passed through, model asserted from
  `turn_context.payload.model`); it skips with the CLI's own error text when
  `codex` is absent, unauthenticated or over its usage limit. See
  [docs/delivery-codex-executor.md](docs/delivery-codex-executor.md).
- Live question cards and approvals (batch 44 WP-G, docs/delivery.md last
  section): `sessiondock claude-hook` is Claude's AskUserQuestion/SessionStart/
  SessionEnd hook (settings file from `--write-bridge-settings`, passed as
  `--settings` in every Claude launch profile; Codex profiles carry `--enable
  default_mode_request_user_input -c suppress_unstable_features_warning=true`);
  cards live under `<state dir>/claude-prompts/`, Codex approvals are parsed off
  the managed instance's screen, and `/api/messages` / `/api/watch` carry the
  Python-shaped `prompt` (`prompt_only` packets). Answering is the page's own
  `/api/term/send` under its console lease (single literal keys `1`–`9`, `y`,
  `p` are accepted; Claude 2.1.270 answers on the option digit). Verify with
  `cargo test -p sessiondock --lib bridge:: --locked`,
  `python3 tests/claude_prompt_suite.py` and the real-CLI
  `tests/prompt_claude_real.py --browser` / `tests/prompt_codex_real.py`
  (cheapest configuration, isolated homes, sessions deleted).
- Browser audit: `cargo test -p sessiondock --test audit_http --locked` and
  `python3 tests/audit_browser.py`; the intake admits before copying, never
  blocks on disk, stores structured redacted metadata only, and stays 501 with
  `audit:false` unless `SESSIONDOCK_AUDIT_DIR` names a private disjoint directory.
- Run state: `python3 tests/live_browser.py` plus the opt-in
  `SESSIONDOCK_TEST_PTYHOST_BINARY=$PWD/target/debug/ptyhost cargo test -p sessiondock
  --test runtime --test runtime_live --locked -- --ignored`. Identity is
  pid + `/proc` start ticks of processes named by a verified host record only;
  `exited` needs host exit, a confirmed lifecycle receipt or a verified identity
  that vanished in this process lifetime; everything else is a typed `unknown`
  and an empty list never means stopped. Keep `capabilities.live` false.
- Terminal exit: `python3 tests/terminal_exit_browser.py`; retain the rendered
  tail and distinguish explicit complete/incomplete exit from a bare socket EOF.
  A confirmed exited instance must not automatically reclaim/reconnect. The gray
  console button remains visible and clickable with the specific explanation.
- Launch identity changes: run host launch-guard and client launch tests. The
  opt-in client `launch_host` test requires `SESSIONDOCK_TEST_PTYHOST_BINARY` as an
  explicit absolute built binary path and `--ignored`; it runs only a private
  free shell. Do not substitute a model CLI or infer native SID from launch ID.
- Creation receipts: `cargo test -p sessiondock lifecycle --locked`; preserve
  persist-before-authority, request idempotency and restart uncertainty.
- Controlled creation: `python3 tests/lifecycle_browser.py`; requires built
  server/ptyhost and uses an explicit private launcher config with a fixed free
  shell. Verify create/status/input, duplicate requests, Web restart and mobile
  cancel. Pending identities never authorize native binding or reliable send.
- Operator binding: `python3 tests/lifecycle_browser.py --native-binding` uses
  the actual confirmation dialog and separate browser storage for cancellation.
  Preserve pending/native lease kinds and durable cancellation after Web restart.
  NativeScope proves record identity, not ownership of the process; no automatic
  cwd/time/filename matching and no reliable-delivery claim from operator binding.
- Optional native metadata: `python3 tests/names_parity.py --browser` and
  `python3 tests/grok_parity.py --browser`; adapter comparison may use
  `--python-source ../agenthub` against synthetic records only.
- Keep fork parent-prefix and leaf-byte cursor semantics separate; agent IDs
  resolve through inventory ownership, never by joining client-provided paths.
  Pure Claude timeline options do not authorize native rewind or state writes.
- Frozen Vue scaffold, when its contract is affected, in `web`: `npm ci`,
  `npm test`, `npm run build`.
- Browser-visible changes require a browser check, including errors and relevant
  mobile behavior. API success alone is not proof of a functioning UI.
- Do not claim Windows/macOS validation based only on a Linux build.
- Preserve tests for event ordering, cursor reset, missing versus null fields,
  and delivery confirmation as those capabilities are implemented.
- Test input is synthetic and copied into temporary directories before mutations.
  No native CLI home discovery or live session access; real-CLI suites use
  isolated homes and the cheapest configuration automatically.
