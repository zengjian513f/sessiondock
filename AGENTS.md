# SessionDock workspace

## Scope and boundaries

- This is an independent repository. Do not modify the sibling Python project
  or its deployment while working here unless the user explicitly requests it.
- Production runs as user unit `sessiondock.service` under `/srv/sessiondock`,
  behind the authenticated `/sessiondock/` proxy. Its writable directories are
  private; CLI roots stay read-only. Do not stop Python or change proxy traffic
  without an explicit request. See `docs/replacement-checklist.md`.
- Tests use loopback listeners and private runtime directories. This applies to
  test data, not source or deployment. Imported ptyhost retains
  its original default directory: always pass an explicit `--dir` when invoking
  it. Never use the deployed service's directories or production sessions for
  smoke tests.
- Real-CLI tests run only with `--include-real` or direct invocation. Use
  Claude `claude-haiku-4-5-20251001`, Codex `gpt-5.6-luna`, or Grok `grok-4.6`
  at low effort. Pass the model on the command line, assert the actual model,
  and use temporary homes. Never change everyday defaults or fall back to a
  costlier model. See `~/.claude/cli-model-isolation.md`.
- Never commit deployment addresses, personal absolute paths, credentials,
  runtime data, build outputs, or local environment files.
- `origin` is `zengjian513f/sessiondock`. Push landed work, not every small fix.
- Hub SSH: `ecs-user@driftnode.cn`.
- A production bug fix includes build, validation, deployment, restart and
  health check. Deploy the current workspace unless the user names another
  source. Preserve sessions, state and concurrent changes; keep a rollback.

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
- `legacy-web` is the production frontend, based on Python `agenthub/static`
  `e5b023a`. Keep changes small and capability-gated. Missing row fields degrade
  like Python. Keep console explanations and outbox data visible. Preferences
  use `AgentHubCapabilities.stored`: read the old `agenthub.*` key once, then
  write only `sessiondock.*`. `tests/brand_names_check.py` checks shell branding.
- `web/src/api`: wire types, runtime validation, and network clients.
- `web/src/domain`: framework-independent state transitions and protocol logic.
- `web/src/stores`: small Pinia stores, split by responsibility.
- `web/src/components`: UI components. Do not put session synchronization,
  delivery confirmation, or terminal byte buffering in view callbacks.
- `reference/legacy-web`: frozen migration reference, not served or bundled.
  Record intentional baseline changes in `docs/migration.md`.

## Delegation

- Grok may draft one self-contained script, fixture or document. Review it and
  record `grok-4.6 headless 产出，人工审阅`. Never delegate delivery,
  authorization, native semantics or ptyhost protocol. See `docs/delegation.md`.
- Delegated packages may use isolated worktrees. This never applies to ordinary
  fixes or deployments.
- Target parity, not more: the backend should match the existing Python
  backend or exceed it only slightly. The goal is replacing Python soon; do not
  introduce extra rejection policies absent from Python.

## Validation

- Rust: `cargo test --workspace --locked`.
- Legacy: `node --test tests/legacy_contract.mjs`; build the server then run
  `python3 tests/legacy_browser.py` (Playwright Chromium required, temporary fixtures).
- Full sweep: `python3 tests/run_validation.py`. Use `--list`, `--dry-run`,
  `--tags` or `--only` to narrow it. See `docs/validation.md` for every suite.
- Before committing docs, run `python3 tests/check_docs_links.py`.
- History changes: `python3 tests/history_parity.py` and
  `python3 tests/history_browser.py`; optionally pass `--python-source ../agenthub`
  to the parity tool for adapter-only comparison against synthetic data.
  `python3 tests/advanced_parity.py --python-source ../agenthub` covers multi-level
  compaction/rewind/sidechains, fork-of-fork with subagents, rich tool cases and
  Grok envelopes; every difference must be a documented DELTA, never UNVERIFIED.
- Behavioral authority: match the frozen Python oracle for this batch. Remove
  Rust-only input, path, size, depth, capacity, format and queue rejection rules
  from implementation, tests and documentation. Do not retain an override switch
  that can restore a removed rule. Preserve actual Python/host protocol limits,
  authentication, reference grants and OS errors. Cache eviction and pagination
  organize work without making valid input fail.
- Read the affected contract before editing. Keep history, list and search
  semantics aligned. Pagination must not move live checkpoints.
- If the user asks for one validation run, finish edits first. Ordinary tests
  use synthetic data and fake CLIs. Record final results; old results do not
  validate later changes.
