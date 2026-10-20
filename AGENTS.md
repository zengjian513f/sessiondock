# SessionDock workspace

## Scope and boundaries

- This is an independent repository. Development and deployment use this
  repository and the current contracts in `docs/`.
- The predecessor is not a SessionDock compatibility surface. No code,
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
  costlier model. See `/lyra-share/agent-rules/references/cli-model-isolation.md`.
- Never commit deployment addresses, personal absolute paths, credentials,
  runtime data, build outputs, or local environment files.
- `origin` is `zengjian513f/sessiondock`. Every completed change must be
  validated, committed and pushed immediately, without another confirmation.
  Deploy runtime changes under the scope rules below. Do not stop at a local commit. Stage only this task's changes;
  preserve concurrent edits. Report validation, push and deployment failures
  accurately, including which targets remain incomplete.
- Hub SSH and node addresses come from the untracked `deploy/targets.local.json`;
  verify target identity before remote operations.
- Match validation and delivery to the changed artifacts. This section is the
  workflow authority for this repository; command examples, historical delivery
  records and tool defaults do not expand the current task. Apply mixed changes
  as the union of their affected scopes, not an automatic full release.
  Determine artifacts and target coverage separately: limiting which artifacts
  change does not permit sampling machines. Inventory every enabled target in
  the deployment configuration and include every instance of each affected
  component. Shared frontend or session behavior (including list, history,
  search and SDK interaction) covers all SessionDock nodes AND the Hub.
  Do not select targets by the bug report's machine, the development machine,
  convenience, or a minimum success count. Before deployment, state the complete
  target set and explain exclusions by component scope or explicit user limits.
  - Frontend-only changes run the affected Chromium suites and visual checks,
    then deploy with `--web-only --targets <all-affected-node-and-hub-targets>`.
    Select every target that serves the affected frontend; exclude resource-agent targets.
    Do not rebuild or replace backend/ptyhost binaries, restart resource agents,
    or run unrelated suites for a frontend fix. Keep the required web-service
    restart, health check and rollback through the deployment tool.
  - Rules/docs-only changes run doc checks, commit and push; no production
    build, deployment or restart. Test-only and deploy-script-only changes also
    do not publish unchanged runtime artifacts; validate their affected scope.
  - Backend or resource-agent changes build, validate and deploy the affected
    artifacts to every instance in scope. Component-specific changes cover all
    instances of that component; do not restart unrelated components.
  - Inspect the scoped plan with `deploy --dry-run`. Use `--test-suites` for the
    affected behavior in shared files; `--test full` is explicit. Already completed
    checks can use `--test none` with the actual validation recorded.
  - Include ptyhost only with `--with-ptyhost` when affected. Standalone `push`
    requires an explicit `--stage`. Service unit/mount changes are runtime
    configuration, not docs-only work. Tests and service files use the same
    committed snapshot as the published artifacts.
- Runtime releases include build as applicable, validation, push, deployment,
  restart and health check. Attempt every target in the complete scope, including
  targets reported unreachable by the preliminary fleet check. Targets already
  verified healthy on the same runtime artifacts need no repeat restart.
  Report each target as deployed and healthy, already current, offline, failed,
  disabled, or excluded with its scope reason. Never call unattempted in-scope
  targets complete or omit them after a subset succeeds.
  After all in-scope targets have an outcome, offline or unsuccessful targets
  may remain explicitly pending without blocking engineering completion or its
  goal. Shared frontend/service releases still require at least one SessionDock
  node AND the Hub successfully deployed, restarted and healthy; component-only
  releases require a healthy instance of that component. These are fallback
  completion thresholds after all targets were attempted, never target-selection
  rules. Do not keep polling offline targets. If the minimum is not yet met,
  continue other actionable engineering work.
  Deploy the committed HEAD unless the user names another source; the build
  ignores other sessions' uncommitted edits (`--allow-dirty PATH` names extras).
  Preserve sessions, state and concurrent changes; keep a rollback.
  Use `python3 deploy/deploy.py deploy` with the scoped flags above (`--all`
  only when every target is affected; build once, push selected targets,
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

- 所有界面修改都遵守 `/lyra-share/agent-rules/references/web-ui.md`：实际操作最终页面，
  亲自检查完整页面的排版、美观和可读性；覆盖所有受影响来源、入口与相关组合状态。
  浏览器断言通过、生成截图或委派审查都不能代替执行者的视觉验收。
- 开发时的范围核对与证据记录见 [开发手册](docs/runbook-dev.md)；SDK 共同行为见
  [SDK 界面合同](docs/sdk-ui.md)。不将某次历史修复或过期方案作为当前设计依据。

- 新增任何视觉元素前，必须参考已有同类视觉元素的风格，优先复用现有组件和样式。按钮、图标、字体、颜色、间距、边框及交互状态应与所在界面保持一致，不另起一套视觉设计。

## Structure

- Treat SessionDock as an independent project. Current contracts live in `docs/`
  and unfinished work lives only in `TODO.md`; do not use the archived migration
  history as current guidance. `web/` is the production frontend at
  `/sessiondock/`. Do not replace the production frontend or delete `web/` without an
  explicit user instruction to do so.
- `crates/sessiondock`: Rust HTTP service. Keep transport handlers separate
  from session/domain logic and future ptyhost client code.
- `crates/ptyhost`: imported independent session host. Preserve its local wire
  protocol and process lifetime unless a task explicitly changes them.
- `crates/ptyhost-client`: explicit-directory local protocol library. It must not
  launch processes, auto-clean unknown host records, or expose host credentials.
- `web` is the production frontend, based on the predecessor Python
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
  absent from that baseline. Enforce behavior through the contracts in `docs/`
  and the browser suites that exercise them.

## Validation

- After every feature or bug fix, run a headless Chromium suite that actually
  exercises the changed path the way a user would: click, type, submit, open
  the affected page. Pick the `*_browser.py` that covers the surface; if none
  exists, add or extend one.
  Playwright Chromium is required; fixtures are temporary. A screenshot of a
  render is not enough.
- Behavioral validation uses headless Chromium browser suites. Do not add unit
  tests or non-browser suites; add or extend a browser suite instead. Shared
  helpers live in `tests/*_fixtures.py`.
- Docs/rules-only changes run `check_docs_links.py` and `check_agents_md.py`.
  Deploy-script-only changes also exercise the changed planning/CLI path with
  temporary inputs or a non-mutating dry run; doc checks alone do not validate
  script behavior. Neither needs a token browser run or a production rollout.
  Anything the page can show still needs the browser path.
- Select checks from the changed behavior and dependencies, not directory names
  alone. Reuse a compatible existing test binary for frontend-only checks; build
  only missing prerequisites. Passing checks need no repeat without a relevant
  change or unresolved failure. Full sweep (`python3 tests/run_validation.py`)
  is for explicit release acceptance or justified cross-cutting changes, not
  every fix. Use `--list`, `--dry-run`, `--tags` or `--only`; see
  `docs/validation.md` for suites.
- Before committing docs, run `python3 tests/check_docs_links.py`.
- History read-model/pagination changes run `python3 tests/history_browser.py`
  and `python3 tests/history_pages_browser.py`; presentation-only changes run
  the browser suite covering that presentation. Add `rich_tools_browser.py`,
  `grok_metadata_browser.py` or `codex_names_browser.py` only when affected.
  Every intentional difference from the frozen Python behavior must be
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
