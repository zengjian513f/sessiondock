# SessionDock workspace

## Scope and boundaries

- This is an independent repository. The predecessor Python project and its
  deployment are retired; never require or restore them for ordinary development.
- The retired predecessor is not a SessionDock compatibility surface. No code,
  tests, configuration, static assets, or frozen source references may contain
  its identifiers or branding. Historical Markdown and recorded benchmark
  queries retain their original wording with an explicit historical context.
- Production runs as user unit `sessiondock.service` under `/srv/sessiondock`,
  behind the authenticated `/sessiondock/` proxy. Its writable directories are
  private. Ordinary reads do not modify CLI roots; explicitly confirmed clone,
  move and trash operations may publish, move or clean native files under their
  contracts, preserving clone sources and minimum-change identity rewriting.
  Ordinary releases preserve proxy routing; retirement maintenance requires an
  explicit request. See `docs/replacement-checklist.md`.
- Tests use loopback listeners and private runtime directories. This applies to
  test data, not source or deployment. Imported ptyhost retains
  its original default directory: always pass an explicit `--dir` when invoking
  it. Never use the deployed service's directories or production sessions for
  smoke tests.
- Real-CLI browser suites (`*_real`) run only with `--include-real`, `--real-only` (the
  release acceptance step, `docs/release.md`) or direct invocation. Use
  Claude `claude-haiku-4-5-20251001`, Codex `gpt-5.6-luna`, or Grok `grok-4.6`
  at low effort. Pass the model on the command line, assert the actual model,
  and use temporary homes. Never change everyday defaults or fall back to a
  costlier model. See `~/.claude/cli-model-isolation.md`.
- Never commit deployment addresses, personal absolute paths, credentials,
  runtime data, build outputs, or local environment files.
- `origin` is `zengjian513f/sessiondock`. Every completed change must be
  validated, committed, pushed and deployed immediately, without another
  confirmation. Do not stop at a local commit. Stage only this task's changes;
  preserve concurrent edits. Report validation, push and deployment failures
  accurately, including which targets remain incomplete.
- Hub SSH: `ecs-user@driftnode.cn`.
- Every change includes build as applicable, validation, push, deployment,
  restart and health check. Deployment is sufficient when at least one
  SessionDock node AND the Hub have successfully deployed, restarted and
  passed health checks. Other offline or unsuccessful targets are recorded
  for later delivery; they do not block engineering completion or its goal.
  Do not require every machine to succeed or keep polling remaining targets.
  If the minimum is not yet met, continue other actionable engineering work.
  Deploy the committed HEAD unless the user names another source; the build
  ignores other sessions' uncommitted edits (`--allow-dirty PATH` names extras).
  Preserve sessions, state and concurrent changes; keep a rollback.
  Use `python3 deploy/deploy.py deploy --all` (build once, push every target,
  verify, auto-rollback; `docs/deployment.md`) and `deploy/fleet_status.py`
  for the read-only fleet table; do not hand-roll scp/restart sequences.

## Shared development checkout

- The checkout used by lyra, cygnus and pavo is shared. Run Cargo on lyra
  (`~/.cargo/bin/cargo` in non-login SSH commands); these machines should use
  the same Rust build artifacts from that checkout. Check `hostname` and the
  actual toolchain before running commands.
- Lyra has the GitHub credentials for `git push`. A Git commit is a local
  repository operation and can be made on any of the three machines with Git
  identity configured; push from lyra. Do not describe GitHub credentials as
  a requirement for committing.
- Run Playwright Chromium validation on a machine where it is installed
  (cygnus; lyra only in its `p311` Conda Python, used for the Grok and Agy
  real-CLI suites). Cygnus lacks Cargo. The deploy test gate may need both, so arrange access to both tools
  before using it. If a gate cannot run, record which checks actually ran;
  `--test none` is not itself validation.

## Visual consistency

- 新增任何视觉元素前，必须参考已有同类视觉元素的风格，优先复用现有组件和样式。按钮、图标、字体、颜色、间距、边框及交互状态应与所在界面保持一致，不另起一套视觉设计。

## Structure

- Treat SessionDock as an independent project. Current contracts live in `docs/`
  and unfinished work lives only in `TODO.md`; do not use the archived migration
  history as current guidance. `legacy-web/` is the production frontend at
  `/sessiondock/`. Do not replace the production frontend or delete `legacy-web/` without an
  explicit user instruction to do so.
- `crates/sessiondock`: Rust HTTP service. Keep transport handlers separate
  from session/domain logic and future ptyhost client code.
- `crates/ptyhost`: imported independent session host. Preserve its local wire
  protocol and process lifetime unless a task explicitly changes them.
- `crates/ptyhost-client`: explicit-directory local protocol library. It must not
  launch processes, auto-clean unknown host records, or expose host credentials.
- `legacy-web` is the production frontend, based on the predecessor Python
  frontend at `e5b023a`. Keep changes small and capability-gated. Missing row
  fields degrade like Python. Keep console explanations visible.
  Preferences use `SessionDockCapabilities.stored` and only `sessiondock.*`
  keys. `tests/brand_names_check.py` checks the complete tracked source tree.
- `reference/legacy-web`: frozen migration reference, not served or bundled.
  Record intentional baseline changes in `reference/README.md`.

## Delegation

- Grok may draft one self-contained script, fixture or document. Review it and
  record `grok-4.6 headless 产出，人工审阅`. Never delegate delivery,
  authorization, native semantics or ptyhost protocol. See `docs/delegation.md`.
- Delegated packages may use isolated worktrees. This never applies to ordinary
  fixes or deployments.
- Target parity with the frozen Python behavioral baseline, not more. The
  production replacement is complete; do not introduce extra rejection policies
  absent from that baseline. The Python-oracle comparison tools were removed on
  2026-10-06; the baseline is enforced through the contracts in `docs/` and the
  browser suites that exercise them.

## Validation

- After every feature or bug fix, run a headless Chromium suite that actually
  exercises the changed path the way a user would: click, type, submit, open
  the affected page. Pick the `*_browser.py` that covers the surface; if none
  exists, add or extend one.
  Playwright Chromium is required; fixtures are temporary. A screenshot of a
  render is not enough.
- Validation is by headless Chromium browser suites only. The repository has
  no unit tests: Rust `#[test]` modules and `crates/*/tests`, Node
  `*_contract.mjs` and Python `unittest` suites were removed on 2026-10-06
  after an audit found every failing unit test (15 Node contract, 21
  `cargo test`, 1 Python unittest) was stale, never a product bug, while real
  regressions were caught by browser suites. The HTTP-only `*_suite.py`
  scripts, the Python-oracle `*_parity.py` tools, benchmarks and probes were
  removed the same day. Do not add unit tests or non-browser suites; add or
  extend a browser suite instead. Shared helpers live in `tests/*_fixtures.py`.
- Docs-only and deploy-script-only changes run the doc checks
  (`check_docs_links.py`, `check_agents_md.py`), not a token browser run. Anything the page can show still needs the browser path.
- Full sweep: `python3 tests/run_validation.py`. Use `--list`, `--dry-run`,
  `--tags` or `--only` to narrow it. See `docs/validation.md` for every suite.
- Before committing docs, run `python3 tests/check_docs_links.py`.
- History changes: `python3 tests/history_browser.py` and
  `python3 tests/history_pages_browser.py` (plus `rich_tools_browser.py`,
  `grok_metadata_browser.py` or `codex_names_browser.py` when those surfaces
  change). Every intentional difference from the frozen Python behavior must be
  a documented DELTA in the affected contract.
- Behavioral authority: match the frozen Python behavior. Remove
  Rust-only input, path, size, depth, capacity, format and queue rejection rules
  from implementation, tests and documentation. Do not retain an override switch
  that can restore a removed rule. Preserve actual Python/host protocol limits,
  authentication, reference grants and OS errors. Cache eviction and pagination
  organize work without making valid input fail.
- Read the affected contract before editing. Keep history, list and search
  semantics aligned. Pagination must not move live checkpoints.
- Native session files follow a minimum-change rule: clone/move may patch only
  required identity, path and dependent offset bytes. Preserve all other bytes,
  including whitespace, line endings, key order and escape spelling. Never
  reserialize entire records or insert missing fields during identity rewriting.
- If the user asks for one validation run, finish edits first. Ordinary tests
  use synthetic data and fake CLIs. Record final results; old results do not
  validate later changes.
