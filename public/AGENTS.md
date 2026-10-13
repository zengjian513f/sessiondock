# SessionDock contributor instructions

This repository is self-contained. Build and validate using the local checkout;
no private machines, predecessor checkout, external applications or fleet
credentials are required.

- `web/` is the production frontend. Reuse existing components and styles.
- Keep HTTP handlers separate from session/domain logic. Preserve the local
  ptyhost protocol and independent process lifetime.
- Read relevant source contracts before changing native identity or file writes.
  Ordinary reads must preserve native session files. Clone/move may patch only
  required identity, path and dependent offset bytes, preserving all other bytes.
- Do not commit credentials, personal paths, deployment addresses, runtime data,
  local environment files or build outputs. Preserve concurrent edits.
- Tests use synthetic sessions, loopback listeners and private temporary runtime
  directories. Always pass an explicit `--dir` to ptyhost. Do not use production
  sessions in tests.
- Validate features and fixes through headless Chromium interactions covering the
  changed path. Add or extend a browser suite when needed; do not add unit tests
  or HTTP-only suites. For visual changes, inspect an actual browser screenshot.
- Run `python3 tests/check_docs_links.py` and
  `python3 tests/check_agents_md.py` after documentation changes. See
  [validation](docs/validation.md) for browser setup and suite selection.
- Real CLI suites are opt-in and may spend credits. Use temporary homes and the
  explicit models prescribed by the suites; do not change everyday CLI defaults.
- Commit only your task's changes. Follow the contributor's requested review and
  publication workflow; a source change does not authorize remote deployments,
  publishing, or sending messages.

Build the core with `cargo build --locked -p sessiondock -p ptyhost`.
The collector is an optional separate build; see
[resource-agent](docs/resource-agent.md). Public binary packaging is described in
[release](docs/release.md).
